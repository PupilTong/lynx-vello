//! The style protocol: how the engine reads computed style.
//!
//! The engine never sees the host's style representation. Instead, the node
//! handle's [`style`](crate::tree::LayoutNode::style) hands out borrowed
//! **style views** —
//! cheap borrowed values implementing [`CoreStyle`] plus the per-algorithm traits
//! ([`FlexContainerStyle`]/[`FlexItemStyle`], [`GridContainerStyle`]/
//! [`GridItemStyle`], [`LinearContainerStyle`]/[`LinearItemStyle`],
//! [`RelativeContainerStyle`]/[`RelativeItemStyle`], and
//! [`TextContainerStyle`]/[`TextRunStyle`]) — and
//! every accessor returns a small `Copy` value from the style value modules
//! ([`value`], [`alignment`], [`linear`], [`relative`], [`text`], and the
//! algorithm modules). A host backed by stylo implements the accessors
//! as direct translations of `ComputedValues` fields; a test host returns
//! struct fields. Blanket impls are provided for `&S`, so implementing a
//! trait on your style struct makes plain references usable as views.
//!
//! # Defaults match the owning layout specification
//!
//! [`CoreStyle`], Flex, Grid, and scalar Text methods default to their **CSS
//! initial values**. Linear and Relative methods default to the initial values
//! documented by their Starlight specifications. Host-specific computed
//! defaults — like Lynx defaulting `box-sizing` to `border-box`, `overflow`
//! to `hidden`, `position` to a `relative` that means CSS `static`, or
//! `relative-layout-once` to `true` — belong in the host's style system,
//! which resolves them before layout runs. The engine defines what the values
//! mean, not which compatibility profile materializes them.
//!
//! # Units contract
//!
//! Lengths are resolved CSS-pixel `f32`s; font-relative (`em`/`rem`),
//! viewport-relative (`vw`/`vh`), and host-specific (`rpx`/`ppx`/`sp`) units
//! must be resolved by the host's computed-style stage. Percentages stay
//! symbolic ([`value::LengthPercentage::Percent`]) because their basis is
//! only known during layout; `calc()` stays symbolic as a
//! [`value::CalcHandle`] resolved through
//! [`LayoutNode::resolve_calc`](crate::tree::LayoutNode::resolve_calc). All
//! values must be finite — `NaN`/`±∞` at the boundary is a host bug
//! (debug-asserted by the algorithms, not defended against in release).

pub mod alignment;
pub mod flex;
pub mod grid;
pub mod linear;
pub mod relative;
pub mod text;
pub mod value;

pub use alignment::{
    AlignContent, AlignItems, AlignSelf, JustifyContent, JustifyItems, JustifySelf,
};
pub use flex::{FlexContainerStyle, FlexDirection, FlexItemStyle, FlexWrap};
pub use grid::{
    GridAutoFlow, GridContainerStyle, GridItemStyle, GridLine, GridPlacement,
    GridTemplateComponent, GridTemplateRepetition, MaxTrackSizingFunction, MinTrackSizingFunction,
    RepetitionCount, TrackSizingFunction,
};
pub use linear::{
    LinearContainerStyle, LinearCrossGravity, LinearDirection, LinearGravity, LinearItemStyle,
    LinearLayoutGravity, LinearOrientation,
};
pub use relative::{RelativeCenter, RelativeContainerStyle, RelativeItemStyle, RelativeReference};
pub use text::{
    FontFamily, FontFeatureSetting, FontStyle, FontVariationSetting, FontWeight, GenericFontFamily,
    LineHeight, OpenTypeTag, TextAlign, TextBrush, TextContainerStyle, TextRun, TextRunStyle,
    WhiteSpace, WordBreak,
};
pub use value::{CalcHandle, Dimension, LengthPercentage, LengthPercentageAuto};

use crate::geometry::{Edges, Point, Size};

/// Whether a node generates a box at all (the layout-relevant projection of
/// `display: none`).
///
/// The dispatch protocol handles `None` before any algorithm runs (see
/// [`hide_subtree`](crate::compute::hide_subtree)); which
/// *algorithm* a generated box uses is the host's dispatch decision, so the
/// engine deliberately has no `Display` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BoxGenerationMode {
    /// The node generates a box and participates in layout.
    #[default]
    Normal,
    /// `display: none`: no box, no layout, zeroed outputs.
    None,
}

/// The layout-relevant projection of CSS `visibility`.
///
/// `hidden` generates and lays out a box exactly like `visible`; painting is
/// the host renderer's concern. `collapse` only changes geometry for flex
/// items: the item is removed from main-axis layout while leaving behind the
/// cross-size strut required by Flexbox §4.4. In every other formatting
/// context it currently behaves like `hidden`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Visibility {
    /// Generate, lay out, and paint the box normally.
    #[default]
    Visible,
    /// Generate and lay out the box, but do not paint it.
    Hidden,
    /// Collapse a flex item to a cross-axis strut.
    Collapse,
}

/// The positioning scheme of a node (the engine-relevant projection of CSS
/// `position`).
///
/// The layout tree is always the **formatting** structure — out-of-flow
/// nodes stay children of their formatting parent, never reparented, so the
/// parent's algorithm can compute their CSS-correct *static position*
/// (Flexbox §4.1: as if the sole flex item; Grid §10.2: the content-edge
/// area). What varies is *where the containing block is*, which the host
/// resolves from computed style and encodes per node:
///
/// Marked `#[non_exhaustive]`: `sticky` (a host post-pass today, see the
/// architecture doc) may become first-class later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Position {
    /// In-flow. Inset offsets, when definite, nudge the node visually after
    /// layout without affecting siblings (CSS `position: relative`; with all
    /// insets `auto` this is exactly CSS `static`, which is also what Lynx's
    /// default `position: relative` means).
    #[default]
    Relative,
    /// Out-of-flow, containing block **is** the layout parent — the common
    /// case, and the only one Lynx `position: absolute` produces (every Lynx
    /// element is positioned, so the nearest positioned ancestor is always
    /// the parent). The parent's algorithm sizes/places the node fully:
    /// insets and percentages resolve against the parent's padding box, and
    /// auto insets fall back to the static position the parent just
    /// computed. The node does not affect sibling layout.
    Absolute,
    /// Out-of-flow, containing block is **not** the layout parent (CSS
    /// `position: fixed`, or `absolute` escaping non-positioned ancestors in
    /// non-Lynx hosts). The parent's algorithm computes and records the
    /// node's static position via
    /// [`LayoutNode::set_static_position`](crate::tree::LayoutNode::set_static_position)
    /// but does **not** size or place it; the host completes it after
    /// in-flow layout in a positioned pass via
    /// [`compute_absolute_layout`](crate::compute::compute_absolute_layout)
    /// against the real containing block (for Lynx `fixed`: the viewport
    /// root, or the nearest transformed/filtered/`will-change` ancestor per
    /// the W3C rule the tracking doc mandates).
    AbsoluteHoisted,
}

/// The `overflow` value of one axis, as layout cares about it.
///
/// Layout consumes overflow in exactly two ways: a non-`Visible` value makes
/// the node a **scroll container**, changing its automatic minimum size
/// (CSS Overflow §3 / Flexbox §4.5), and `Scroll` additionally reserves
/// [`CoreStyle::scrollbar_width`] of space. Actual clipping/scrolling is the
/// host renderer's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Overflow {
    /// Content may paint outside the box; contributes to scrollable
    /// overflow of ancestors.
    #[default]
    Visible,
    /// Clipped, not scrollable, and *not* a scroll container for sizing.
    Clip,
    /// Clipped and programmatically scrollable (Lynx's default).
    Hidden,
    /// Clipped, scrollable, and reserving scrollbar space.
    Scroll,
}

impl Overflow {
    /// Does this value make the box a scroll container for automatic
    /// minimum-size purposes?
    #[must_use]
    pub const fn is_scroll_container(self) -> bool {
        matches!(self, Self::Hidden | Self::Scroll)
    }
}

/// `box-sizing`: which box `width`/`height`/`flex-basis` measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BoxSizing {
    /// Sizes measure the content box (the CSS initial value; note Lynx
    /// *computes* its `box-sizing: auto` default to `BorderBox`).
    #[default]
    ContentBox,
    /// Sizes measure the border box.
    BorderBox,
}

/// Resolved inline-axis direction (`direction`, including Lynx's
/// `lynx-rtl`, which the host lowers to `Rtl`).
///
/// A physical-axis engine consumes this only where CSS says it matters for
/// box layout: flipping the main axis of `row` flex containers and the
/// inline axis of grid placement/alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Direction {
    /// Left-to-right.
    #[default]
    Ltr,
    /// Right-to-left.
    Rtl,
}

/// Style every box has, regardless of which algorithm lays it out.
///
/// This is the supertrait of all container/item style traits and the only
/// style the generic machinery (dispatch, hidden-subtree cleanup, leaf layout,
/// rounding) reads. Defaults are the CSS initial values.
///
/// Percentage bases (all per CSS Sizing/Box):
/// - `size.width`, `min/max width`, `margin` (all four edges!), `padding` (all four edges),
///   `inset.left/right` — the containing block's **width**.
/// - `size.height`, `min/max height`, `inset.top/bottom` — the containing block's **height**.
pub trait CoreStyle: Sized {
    /// Whether this node generates a box (`display: none` ⇒
    /// [`BoxGenerationMode::None`]).
    fn box_generation_mode(&self) -> BoxGenerationMode {
        BoxGenerationMode::Normal
    }

    /// `visibility`. Only [`Visibility::Collapse`] affects box geometry, and
    /// only when this node is a flex item.
    fn visibility(&self) -> Visibility {
        Visibility::Visible
    }

    /// The positioning scheme (see [`Position`]).
    fn position(&self) -> Position {
        Position::Relative
    }

    /// `top`/`right`/`bottom`/`left`.
    fn inset(&self) -> Edges<LengthPercentageAuto> {
        Edges::uniform(LengthPercentageAuto::Auto)
    }

    /// `width`/`height` (interpreted per [`box_sizing`](Self::box_sizing)).
    fn size(&self) -> Size<Dimension> {
        Size::new(Dimension::Auto, Dimension::Auto)
    }

    /// `min-width`/`min-height`.
    fn min_size(&self) -> Size<Dimension> {
        Size::new(Dimension::Auto, Dimension::Auto)
    }

    /// `max-width`/`max-height`.
    fn max_size(&self) -> Size<Dimension> {
        Size::new(Dimension::Auto, Dimension::Auto)
    }

    /// `aspect-ratio` as `width / height`. `None` = `auto` (no ratio — Lynx
    /// has no `auto` keyword, matching this default).
    fn aspect_ratio(&self) -> Option<f32> {
        None
    }

    /// `margin`. `Auto` margins absorb free space per the algorithm's rules.
    fn margin(&self) -> Edges<LengthPercentageAuto> {
        Edges::uniform(LengthPercentageAuto::ZERO)
    }

    /// `padding`.
    fn padding(&self) -> Edges<LengthPercentage> {
        Edges::uniform(LengthPercentage::ZERO)
    }

    /// Used `border-*-width` (i.e. `0` when the border style is `none`).
    fn border(&self) -> Edges<LengthPercentage> {
        Edges::uniform(LengthPercentage::ZERO)
    }

    /// `overflow-x`/`overflow-y` (see [`Overflow`] for how layout uses it).
    fn overflow(&self) -> Point<Overflow> {
        Point::new(Overflow::Visible, Overflow::Visible)
    }

    /// Space to reserve for a scrollbar on axes whose overflow is
    /// [`Overflow::Scroll`], in CSS pixels. Overlay-scrollbar hosts (Lynx)
    /// pass `0.0`.
    fn scrollbar_width(&self) -> f32 {
        0.0
    }

    /// `box-sizing`.
    fn box_sizing(&self) -> BoxSizing {
        BoxSizing::ContentBox
    }

    /// Resolved inline direction (see [`Direction`]).
    fn direction(&self) -> Direction {
        Direction::Ltr
    }
}

impl<S: CoreStyle> CoreStyle for &S {
    fn box_generation_mode(&self) -> BoxGenerationMode {
        (**self).box_generation_mode()
    }

    fn visibility(&self) -> Visibility {
        (**self).visibility()
    }

    fn position(&self) -> Position {
        (**self).position()
    }

    fn inset(&self) -> Edges<LengthPercentageAuto> {
        (**self).inset()
    }

    fn size(&self) -> Size<Dimension> {
        (**self).size()
    }

    fn min_size(&self) -> Size<Dimension> {
        (**self).min_size()
    }

    fn max_size(&self) -> Size<Dimension> {
        (**self).max_size()
    }

    fn aspect_ratio(&self) -> Option<f32> {
        (**self).aspect_ratio()
    }

    fn margin(&self) -> Edges<LengthPercentageAuto> {
        (**self).margin()
    }

    fn padding(&self) -> Edges<LengthPercentage> {
        (**self).padding()
    }

    fn border(&self) -> Edges<LengthPercentage> {
        (**self).border()
    }

    fn overflow(&self) -> Point<Overflow> {
        (**self).overflow()
    }

    fn scrollbar_width(&self) -> f32 {
        (**self).scrollbar_width()
    }

    fn box_sizing(&self) -> BoxSizing {
        (**self).box_sizing()
    }

    fn direction(&self) -> Direction {
        (**self).direction()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {}

    #[test]
    fn core_style_defaults_are_css_initial_values() {
        let style = Defaults;

        assert_eq!(style.box_generation_mode(), BoxGenerationMode::Normal);
        assert_eq!(style.position(), Position::Relative);
        assert_eq!(style.inset(), Edges::uniform(LengthPercentageAuto::Auto));
        assert_eq!(style.size(), Size::new(Dimension::Auto, Dimension::Auto));
        assert_eq!(
            style.min_size(),
            Size::new(Dimension::Auto, Dimension::Auto)
        );
        assert_eq!(
            style.max_size(),
            Size::new(Dimension::Auto, Dimension::Auto)
        );
        assert_eq!(style.aspect_ratio(), None);
        assert_eq!(style.margin(), Edges::uniform(LengthPercentageAuto::ZERO));
        assert_eq!(style.padding(), Edges::uniform(LengthPercentage::ZERO));
        assert_eq!(style.border(), Edges::uniform(LengthPercentage::ZERO));
        assert_eq!(
            style.overflow(),
            Point::new(Overflow::Visible, Overflow::Visible)
        );
        assert_eq!(style.scrollbar_width(), 0.0);
        assert_eq!(style.box_sizing(), BoxSizing::ContentBox);
        assert_eq!(style.direction(), Direction::Ltr);
    }

    #[test]
    fn overflow_only_treats_scrollable_values_as_scroll_containers() {
        assert!(!Overflow::Visible.is_scroll_container());
        assert!(!Overflow::Clip.is_scroll_container());
        assert!(Overflow::Hidden.is_scroll_container());
        assert!(Overflow::Scroll.is_scroll_container());
    }
}
