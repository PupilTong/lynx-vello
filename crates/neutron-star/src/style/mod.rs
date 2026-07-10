//! The style protocol: how the engine reads computed style.
//!
//! The engine never sees the host's style representation. Instead, the tree
//! traits hand out short-lived **style views** — cheap borrowed values
//! implementing [`CoreStyle`] plus the per-algorithm traits
//! ([`FlexContainerStyle`]/[`FlexItemStyle`], [`GridContainerStyle`]/
//! [`GridItemStyle`]) — and every accessor returns a small `Copy` value from
//! [`value`]/[`alignment`]. A host backed by stylo implements the accessors
//! as direct translations of `ComputedValues` fields; a test host returns
//! struct fields. Blanket impls are provided for `&S`, so implementing a
//! trait on your style struct makes plain references usable as views.
//!
//! # Defaults are CSS initial values
//!
//! Every defaulted trait method returns the **CSS initial value** for its
//! property. Host-specific *defaults* — like Lynx defaulting `box-sizing` to
//! `border-box`, `overflow` to `hidden`, and `position` to a `relative` that
//! means CSS `static` — are computed-style policy and belong in the host's
//! style system, which already resolves them before layout runs. The engine
//! only defines what the values *mean*.
//!
//! # Units contract
//!
//! Lengths are resolved CSS-pixel `f32`s; font-relative (`em`/`rem`),
//! viewport-relative (`vw`/`vh`), and host-specific (`rpx`/`ppx`/`sp`) units
//! must be resolved by the host's computed-style stage. Percentages stay
//! symbolic ([`value::LengthPercentage::Percent`]) because their basis is
//! only known during layout; `calc()` stays symbolic as a
//! [`value::CalcHandle`] resolved through
//! [`LayoutTree::resolve_calc`](crate::tree::LayoutTree::resolve_calc). All
//! values must be finite — `NaN`/`±∞` at the boundary is a host bug
//! (debug-asserted by the algorithms, not defended against in release).

pub mod alignment;
pub mod flex;
pub mod grid;
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
pub use value::{CalcHandle, Dimension, LengthPercentage, LengthPercentageAuto};

use crate::geometry::{Edges, Point, Size};

/// Whether a node generates a box at all (the layout-relevant projection of
/// `display: none`).
///
/// The dispatch protocol handles `None` before any algorithm runs (see
/// [`compute_hidden_layout`](crate::compute::compute_hidden_layout)); which
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

/// The positioning scheme of a node (the engine-relevant projection of CSS
/// `position`).
///
/// Marked `#[non_exhaustive]`: `sticky` (a host post-pass today, see the
/// architecture doc) may become first-class later. `fixed` is *not* planned
/// as a variant — per the CSS containing-block rules the host lowers a fixed
/// node to `Absolute` under the layout-tree node that is its containing
/// block (the viewport root, or the nearest transformed/filtered ancestor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Position {
    /// In-flow. Inset offsets, when definite, nudge the node visually after
    /// layout without affecting siblings (CSS `position: relative`; with all
    /// insets `auto` this is exactly CSS `static`, which is also what Lynx's
    /// default `position: relative` means).
    #[default]
    Relative,
    /// Out-of-flow. The node is sized/placed against its layout parent's
    /// padding box using its insets; it does not affect sibling layout. The
    /// host must arrange the layout tree so an absolute node's parent *is*
    /// its CSS containing block (for Lynx this is automatic: every element
    /// is positioned, so the containing block is always the parent).
    Absolute,
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
/// style the generic machinery (dispatch, hidden layout, leaf layout,
/// rounding) reads. Defaults are the CSS initial values.
///
/// Percentage bases (all per CSS Sizing/Box):
/// - `size.width`, `min/max width`, `margin` (all four edges!), `padding` (all four edges),
///   `inset.left/right` — the containing block's **width**.
/// - `size.height`, `min/max height`, `inset.top/bottom` — the containing block's **height**.
pub trait CoreStyle {
    /// Whether this node generates a box (`display: none` ⇒
    /// [`BoxGenerationMode::None`]).
    fn box_generation_mode(&self) -> BoxGenerationMode {
        BoxGenerationMode::Normal
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
