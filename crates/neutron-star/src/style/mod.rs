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
//! # Accessor conventions: `Copy` values owned, everything else borrowed
//!
//! Small `Copy` values (keyword enums, alignment flags, `Au` border widths,
//! numbers) are returned owned. The `LengthPercentage`-family geometry
//! properties — inset, size, min/max size, margin, padding, flex-basis, gap,
//! and grid-line placements — are returned **borrowed** as per-field
//! references inside the geometry wrappers (`Edges<&Margin>`,
//! `Size<&StyleSize>`, `&FlexBasis`, …), and large sequences (grid track
//! lists) are returned borrowed whole. Per-field reference wrappers (built
//! with [`Edges::as_ref`]/[`Size::as_ref`] or field-by-field) are lendable
//! from any host storage — a stylo `ComputedValues` host keeps its four
//! margin edges in separate fields and still lends `Edges<&Margin>`. Borrowed
//! returns mean a read never clones a `calc()` tree or bumps a refcount; the
//! resolvers lower values to `f32` without ever owning them. The borrow is
//! tied to the style-view binding, so bind the view first (`let style =
//! node.style();`) — the discipline documented in [`tree`](crate::tree). A
//! host whose style views *synthesize* values on the fly must materialize
//! them in per-node storage and lend from there (the convert-once-per-style-
//! change pattern); temporaries cannot be lent.
//!
//! Text accessors ([`TextRunStyle`], [`TextContainerStyle`]) keep owned
//! returns: they run once per (re)shape, amortized by the measurement cache.
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

pub mod containment;
pub mod flex;
pub mod grid;
pub mod linear;
pub mod relative;
pub mod text;

pub use containment::effective_containment;
pub use flex::{FlexContainerStyle, FlexItemStyle};
pub use grid::{GridContainerStyle, GridItemStyle};
pub use linear::{LinearContainerStyle, LinearItemStyle};
pub use relative::{RelativeContainerStyle, RelativeItemStyle};
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
    AspectRatio, Au, BorderSideWidth, Contain, ContainIntrinsicSize, ContentDistribution,
    ContentVisibility, Display, FlexBasis, FontFamily, FontFeatureSettings, FontStyle,
    FontVariationSettings, FontWeight, GridAutoFlow, GridLine, GridTemplateComponent,
    ImplicitGridTracks, Inset, ItemPlacement, JustifyItems, LengthPercentage, LetterSpacing,
    LineHeight, Margin, MaxSize, NonNegativeLengthPercentage, NonNegativeNumber, Overflow,
    PositionProperty, SelfAlignment, Size as StyleSize, TextAlign, TextIndent, WordBreak,
};
pub use stylo::values::specified::align::AlignFlags;
pub use text::{TextBrush, TextContainerStyle, TextRun, TextRunStyle};

use crate::geometry::{Edges, Point, Size};

/// Lendable initial values for the defaulted borrowed accessors.
///
/// Trait defaults must return references that outlive the style view, and
/// the stylo constructors are not `const`, so the fork's initial values live
/// in lazily-initialized statics. Visible to the sibling style modules only.
pub(in crate::style) mod defaults {
    use std::sync::LazyLock;

    use stylo::Zero;

    use super::{Inset, Margin, MaxSize, NonNegativeLengthPercentage, StyleSize};

    pub(in crate::style) static INSET_AUTO: LazyLock<Inset> = LazyLock::new(Inset::auto);
    pub(in crate::style) static SIZE_AUTO: LazyLock<StyleSize> = LazyLock::new(StyleSize::auto);
    pub(in crate::style) static MAX_SIZE_NONE: LazyLock<MaxSize> = LazyLock::new(MaxSize::none);
    pub(in crate::style) static MARGIN_ZERO: LazyLock<Margin> = LazyLock::new(Margin::zero);
    pub(in crate::style) static PADDING_ZERO: LazyLock<NonNegativeLengthPercentage> =
        LazyLock::new(NonNegativeLengthPercentage::zero);
}

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

    /// `top`/`right`/`bottom`/`left`, lent from the style view.
    fn inset(&self) -> Edges<&Inset> {
        Edges::uniform(&*defaults::INSET_AUTO)
    }

    /// `width`/`height` (interpreted per [`box_sizing`](Self::box_sizing)),
    /// lent from the style view.
    ///
    /// The lynx-parseable keywords Starlight has no sizing behavior for
    /// (bare `fit-content`, `stretch`, `-webkit-fill-available`) are treated
    /// as `auto` by every algorithm.
    fn size(&self) -> Size<&StyleSize> {
        Size::new(&*defaults::SIZE_AUTO, &*defaults::SIZE_AUTO)
    }

    /// `min-width`/`min-height`, lent from the style view.
    fn min_size(&self) -> Size<&StyleSize> {
        Size::new(&*defaults::SIZE_AUTO, &*defaults::SIZE_AUTO)
    }

    /// `max-width`/`max-height`, lent from the style view.
    fn max_size(&self) -> Size<&MaxSize> {
        Size::new(&*defaults::MAX_SIZE_NONE, &*defaults::MAX_SIZE_NONE)
    }

    /// `aspect-ratio`. The engine uses the preferred ratio as `width /
    /// height`; degenerate ratios behave as `auto` per CSS Sizing 4.
    fn aspect_ratio(&self) -> AspectRatio {
        AspectRatio::auto()
    }

    /// `margin`, lent from the style view. `Auto` margins absorb free space
    /// per the algorithm's rules.
    fn margin(&self) -> Edges<&Margin> {
        Edges::uniform(&*defaults::MARGIN_ZERO)
    }

    /// `padding`, lent from the style view.
    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
        Edges::uniform(&*defaults::PADDING_ZERO)
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

    /// The box's **effective** CSS containment ([`Contain`]).
    ///
    /// The host folds `content-visibility` into the raw `contain` value exactly
    /// as stylo's gecko-mode adjuster does — see [`effective_containment`].
    /// `content-visibility: hidden` (and `auto` while skipped) therefore
    /// contributes `SIZE | LAYOUT | PAINT | STYLE`. When
    /// [`skips_contents`](Self::skips_contents) is `true`, this **must** report
    /// at least `SIZE | LAYOUT | PAINT | STYLE`. Consumers query the effect bits
    /// ([`Contain::contains`]), never the `CONTENT`/`STRICT` marker composites
    /// (see the [`containment`] module).
    fn containment(&self) -> Contain {
        Contain::empty()
    }

    /// `contain-intrinsic-width`: the substitute content-box width a
    /// size-contained box reports instead of measuring its contents.
    fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
        ContainIntrinsicSize::None
    }

    /// `contain-intrinsic-height`: the substitute content-box height a
    /// size-contained box reports instead of measuring its contents.
    fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
        ContainIntrinsicSize::None
    }

    /// Whether the box skips laying out its contents (`content-visibility:
    /// hidden`, or `auto` while off-screen — the host supplies the relevance
    /// signal).
    ///
    /// When `true`, host dispatch routes the node to
    /// [`compute_skipped_contents_layout`](crate::compute::compute_skipped_contents_layout)
    /// instead of an algorithm, and [`containment`](Self::containment) must
    /// report at least `SIZE | LAYOUT | PAINT | STYLE`.
    fn skips_contents(&self) -> bool {
        false
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

    fn inset(&self) -> Edges<&Inset> {
        (**self).inset()
    }

    fn size(&self) -> Size<&StyleSize> {
        (**self).size()
    }

    fn min_size(&self) -> Size<&StyleSize> {
        (**self).min_size()
    }

    fn max_size(&self) -> Size<&MaxSize> {
        (**self).max_size()
    }

    fn aspect_ratio(&self) -> AspectRatio {
        (**self).aspect_ratio()
    }

    fn margin(&self) -> Edges<&Margin> {
        (**self).margin()
    }

    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
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

    fn containment(&self) -> Contain {
        (**self).containment()
    }

    fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
        (**self).contain_intrinsic_width()
    }

    fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
        (**self).contain_intrinsic_height()
    }

    fn skips_contents(&self) -> bool {
        (**self).skips_contents()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::Zero;

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
        assert_eq!(style.inset(), Edges::uniform(&Inset::auto()));
        assert_eq!(
            style.size(),
            Size::new(&StyleSize::auto(), &StyleSize::auto())
        );
        assert_eq!(
            style.min_size(),
            Size::new(&StyleSize::auto(), &StyleSize::auto())
        );
        assert_eq!(
            style.max_size(),
            Size::new(&MaxSize::none(), &MaxSize::none())
        );
        assert!(style.aspect_ratio().auto);
        assert_eq!(style.margin(), Edges::uniform(&Margin::zero()));
        assert_eq!(
            style.padding(),
            Edges::uniform(&NonNegativeLengthPercentage::zero())
        );
        assert_eq!(style.border(), Edges::uniform(BorderSideWidth(Au(0))));
        assert_eq!(
            style.overflow(),
            Point::new(Overflow::Visible, Overflow::Visible)
        );
        assert_eq!(style.box_sizing(), box_sizing::T::ContentBox);
        assert_eq!(style.direction(), direction::T::Ltr);
        assert_eq!(style.containment(), Contain::empty());
        assert_eq!(style.contain_intrinsic_width(), ContainIntrinsicSize::None);
        assert_eq!(style.contain_intrinsic_height(), ContainIntrinsicSize::None);
        assert!(!style.skips_contents());

        // The blanket `&S` view forwards the new accessors too.
        let view = &style;
        assert_eq!(view.containment(), Contain::empty());
        assert_eq!(view.contain_intrinsic_width(), ContainIntrinsicSize::None);
        assert_eq!(view.contain_intrinsic_height(), ContainIntrinsicSize::None);
        assert!(!view.skips_contents());
    }

    #[test]
    fn overflow_scroll_containers_follow_stylo_is_scrollable() {
        assert!(!Overflow::Visible.is_scrollable());
        assert!(Overflow::Hidden.is_scrollable());
    }

    #[test]
    fn reference_views_forward_core_accessors() {
        // Engines consume `N::Style` views that are usually references; the
        // blanket `&S` impl must serve the same values as the underlying
        // style.
        let style = Defaults;
        let view = &style;
        assert_eq!(CoreStyle::visibility(&view), visibility::T::Visible);
        assert_eq!(CoreStyle::position(&view), PositionProperty::Static);
        assert!(!CoreStyle::display(&view).is_none());
    }
}
