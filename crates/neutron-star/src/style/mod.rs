//! The style protocol: how the engine reads computed style.
//!
//! The engine never sees the host's style representation. Instead, the node
//! handle's [`style`](crate::tree::LayoutNode::style) hands out borrowed
//! **style views** —
//! cheap borrowed values implementing [`CoreStyle`] plus the per-algorithm traits
//! ([`FlexContainerStyle`]/[`FlexItemStyle`], [`GridContainerStyle`]/
//! [`GridItemStyle`], [`LinearContainerStyle`]/[`LinearItemStyle`],
//! [`RelativeContainerStyle`]/[`RelativeItemStyle`], and
//! [`TextContainerStyle`]/[`TextRunStyle`]) — and every accessor speaks
//! **stylo's computed-value vocabulary** directly: the same `Display`,
//! `LengthPercentage`, `Margin`, `AlignFlags`-based alignment wrappers, grid
//! track lists, and keyword enums the stylo cascade produces. A host backed
//! by stylo implements the accessors as direct field reads of its
//! `ComputedValues`; a cascade-less host (tests, benches) constructs the same
//! stylo values by hand. Blanket impls are provided for `&S`, so implementing
//! a trait on your style struct makes plain references usable as views.
//!
//! Small `Copy` values and Clone-cheap `LengthPercentage`-family values are
//! returned owned (cloned) inside the geometry wrappers; large sequences
//! (grid track lists) are returned borrowed from the style view.
//!
//! # Defaults are the fork's initial values
//!
//! Every defaulted accessor returns the lynx stylo fork's initial value for
//! the property (which is the CSS initial value except where Lynx documents
//! otherwise, e.g. `relative-layout-once: true`). Host-specific *computed*
//! defaults — like Lynx computing `box-sizing: auto` to `border-box` or
//! `overflow` to `hidden` — belong in the host's style system, which resolves
//! them before layout runs. [`CoreStyle::display`] and the borrowed grid
//! track accessors are required.
//!
//! # Units contract
//!
//! Lengths inside computed values are resolved CSS-pixel values;
//! font-relative (`em`/`rem`), viewport-relative (`vw`/`vh`), and
//! host-specific (`rpx`/`ppx`/`sp`) units must be resolved by the host's
//! computed-style stage. Percentages stay symbolic inside
//! [`LengthPercentage`] because their basis is only known during layout;
//! `calc()` is carried by the same self-resolving value (length-only `calc()`
//! folds to a length at computed-value time). All values must be finite —
//! `NaN`/`±∞` at the boundary is a host bug (debug-asserted by the
//! algorithms, not defended against in release).

pub mod flex;
pub mod grid;
pub mod linear;
pub mod relative;
pub mod text;

pub use flex::{FlexContainerStyle, FlexItemStyle};
pub use grid::{GridContainerStyle, GridItemStyle};
pub use linear::{LinearContainerStyle, LinearItemStyle};
pub use relative::{RelativeContainerStyle, RelativeItemStyle};
use stylo::Zero;
// Re-export every stylo type the protocol mentions so hosts can write
// `neutron_star::style::Margin` etc. without naming the stylo crate. The
// keyword enums live in per-property modules (`visibility::T`, …), matching
// stylo's own layout.
pub use stylo::computed_values::{
    box_sizing, direction, flex_direction, flex_wrap, linear_direction, relative_center,
    relative_layout_once, text_wrap_mode, visibility, white_space_collapse,
};
pub use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
pub use stylo::values::computed::lynx_layout::{RelativeAlign, RelativeReference};
pub use stylo::values::computed::{
    AspectRatio, Au, BorderSideWidth, ContentDistribution, Display, FlexBasis, FontFamily,
    FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight, GridAutoFlow, GridLine,
    GridTemplateComponent, ImplicitGridTracks, Inset, ItemPlacement, JustifyItems,
    LengthPercentage, LetterSpacing, LineHeight, Margin, MaxSize, NonNegativeLengthPercentage,
    NonNegativeNumber, Overflow, PositionProperty, SelfAlignment, Size as StyleSize, TextAlign,
    TextIndent, WordBreak,
};
pub use stylo::values::specified::align::AlignFlags;
pub use text::{TextBrush, TextContainerStyle, TextRun, TextRunStyle};

use crate::geometry::{Edges, Point, Size};

/// Style every box has, regardless of which algorithm lays it out.
///
/// This is the supertrait of all container/item style traits and the only
/// style the generic machinery (dispatch, hidden-subtree cleanup, leaf layout,
/// rounding) reads. Defaults are the fork's initial values;
/// [`display`](Self::display) is required.
///
/// Percentage bases (all per CSS Sizing/Box):
/// - `size.width`, `min/max width`, `margin` (all four edges!), `padding` (all four edges),
///   `inset.left/right` — the containing block's **width**.
/// - `size.height`, `min/max height`, `inset.top/bottom` — the containing block's **height**.
///
/// The engine consumes `display` only through [`Display::is_none`] (a
/// non-generated box); which *algorithm* a generated box uses is the host's
/// dispatch decision made from the same value. `display: contents` is a
/// box-tree-construction concern and must be resolved by the host before
/// layout.
pub trait CoreStyle: Sized {
    /// `display` — required; the engine reads only [`Display::is_none`].
    fn display(&self) -> Display;

    /// `visibility`. Painting is the host renderer's concern; the lynx
    /// grammar has no `collapse`, so visibility never affects box geometry.
    fn visibility(&self) -> visibility::T {
        visibility::T::Visible
    }

    /// `position`. The engine bakes the Lynx containing-block policy:
    /// `static`/`relative`/`sticky` lay out in flow (`relative` gets the
    /// definite-inset visual nudge; `sticky` is nudged by the host at scroll
    /// time), `absolute` is sized and placed by the layout parent (containing
    /// block = the parent's padding box), and `fixed` is hoisted — the parent
    /// records the static position and the host completes layout in its
    /// positioned pass.
    fn position(&self) -> PositionProperty {
        PositionProperty::Static
    }

    /// `top`/`right`/`bottom`/`left`.
    fn inset(&self) -> Edges<Inset> {
        Edges::uniform(Inset::auto())
    }

    /// `width`/`height` (interpreted per [`box_sizing`](Self::box_sizing)).
    ///
    /// The lynx-parseable keywords Starlight has no sizing behavior for
    /// (bare `fit-content`, `stretch`, `-webkit-fill-available`) are treated
    /// as `auto` by every algorithm.
    fn size(&self) -> Size<StyleSize> {
        Size::new(StyleSize::auto(), StyleSize::auto())
    }

    /// `min-width`/`min-height`.
    fn min_size(&self) -> Size<StyleSize> {
        Size::new(StyleSize::auto(), StyleSize::auto())
    }

    /// `max-width`/`max-height`.
    fn max_size(&self) -> Size<MaxSize> {
        Size::new(MaxSize::none(), MaxSize::none())
    }

    /// `aspect-ratio`. The engine uses the preferred ratio as `width /
    /// height`; degenerate ratios behave as `auto` per CSS Sizing 4.
    fn aspect_ratio(&self) -> AspectRatio {
        AspectRatio::auto()
    }

    /// `margin`. `Auto` margins absorb free space per the algorithm's rules.
    fn margin(&self) -> Edges<Margin> {
        Edges::uniform(Margin::zero())
    }

    /// `padding`.
    fn padding(&self) -> Edges<NonNegativeLengthPercentage> {
        Edges::uniform(NonNegativeLengthPercentage::zero())
    }

    /// Used `border-*-width` (i.e. `0` when the border style is `none`).
    /// Computed border widths are absolute (`Au`) and never depend on a
    /// percentage basis.
    fn border(&self) -> Edges<BorderSideWidth> {
        Edges::uniform(BorderSideWidth(Au(0)))
    }

    /// `overflow-x`/`overflow-y`. A non-`Visible` value makes the node a
    /// scroll container, changing its automatic minimum size (CSS Overflow
    /// §3 / Flexbox §4.5). Lynx scrollbars are overlay-only, so no scrollbar
    /// space is ever reserved; clipping/scrolling is the host renderer's job.
    fn overflow(&self) -> Point<Overflow> {
        Point::new(Overflow::Visible, Overflow::Visible)
    }

    /// `box-sizing`: which box `width`/`height`/`flex-basis` measure. (Note
    /// Lynx *computes* its `box-sizing: auto` default to `border-box`.)
    fn box_sizing(&self) -> box_sizing::T {
        box_sizing::T::ContentBox
    }

    /// Resolved inline direction (Lynx's `lynx-rtl` is lowered to `Rtl` by
    /// the host). A physical-axis engine consumes this only where CSS says
    /// it matters for box layout: flipping the main axis of `row` flex
    /// containers, the inline axis of grid placement/alignment, and the
    /// physical `left`/`right` alignment keywords.
    fn direction(&self) -> direction::T {
        direction::T::Ltr
    }
}

impl<S: CoreStyle> CoreStyle for &S {
    fn display(&self) -> Display {
        (**self).display()
    }

    fn visibility(&self) -> visibility::T {
        (**self).visibility()
    }

    fn position(&self) -> PositionProperty {
        (**self).position()
    }

    fn inset(&self) -> Edges<Inset> {
        (**self).inset()
    }

    fn size(&self) -> Size<StyleSize> {
        (**self).size()
    }

    fn min_size(&self) -> Size<StyleSize> {
        (**self).min_size()
    }

    fn max_size(&self) -> Size<MaxSize> {
        (**self).max_size()
    }

    fn aspect_ratio(&self) -> AspectRatio {
        (**self).aspect_ratio()
    }

    fn margin(&self) -> Edges<Margin> {
        (**self).margin()
    }

    fn padding(&self) -> Edges<NonNegativeLengthPercentage> {
        (**self).padding()
    }

    fn border(&self) -> Edges<BorderSideWidth> {
        (**self).border()
    }

    fn overflow(&self) -> Point<Overflow> {
        (**self).overflow()
    }

    fn box_sizing(&self) -> box_sizing::T {
        (**self).box_sizing()
    }

    fn direction(&self) -> direction::T {
        (**self).direction()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {
        fn display(&self) -> Display {
            Display::Flex
        }
    }

    #[test]
    fn core_style_defaults_are_fork_initial_values() {
        let style = Defaults;

        assert!(!style.display().is_none());
        assert_eq!(style.visibility(), visibility::T::Visible);
        assert_eq!(style.position(), PositionProperty::Static);
        assert_eq!(style.inset(), Edges::uniform(Inset::auto()));
        assert_eq!(
            style.size(),
            Size::new(StyleSize::auto(), StyleSize::auto())
        );
        assert_eq!(
            style.min_size(),
            Size::new(StyleSize::auto(), StyleSize::auto())
        );
        assert_eq!(
            style.max_size(),
            Size::new(MaxSize::none(), MaxSize::none())
        );
        assert!(style.aspect_ratio().auto);
        assert_eq!(style.margin(), Edges::uniform(Margin::zero()));
        assert_eq!(
            style.padding(),
            Edges::uniform(NonNegativeLengthPercentage::zero())
        );
        assert_eq!(style.border(), Edges::uniform(BorderSideWidth(Au(0))));
        assert_eq!(
            style.overflow(),
            Point::new(Overflow::Visible, Overflow::Visible)
        );
        assert_eq!(style.box_sizing(), box_sizing::T::ContentBox);
        assert_eq!(style.direction(), direction::T::Ltr);
    }

    #[test]
    fn overflow_scroll_containers_follow_stylo_is_scrollable() {
        assert!(!Overflow::Visible.is_scrollable());
        assert!(Overflow::Hidden.is_scrollable());
    }
}
