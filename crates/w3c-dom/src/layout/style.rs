//! The computed-style **view**: lending stylo [`ComputedValues`] fields to
//! neutron-star.
//!
//! The engine's style protocol speaks stylo's computed-value vocabulary, so
//! this view is nearly translation-free: every accessor lends the
//! `ComputedValues` field the trait asks for (per-field references for the
//! `LengthPercentage`-family geometry, owned copies for keyword enums). Two
//! places carry real logic:
//!
//! - [`CoreStyle::position`] reports the engine's positioning **scheme** (`absolute` = containing
//!   block is the layout parent, `fixed` = hoisted to the positioned pass) after resolving the W3C
//!   containing-block rule against the parent — see [`resolve_position`].
//! - The Lynx logical `relative-*-inline-*` longhands are lowered onto physical edges by
//!   `direction`, with the physical property winning when both are set.
//!
//! Text nodes carry no computed style; the view lends them the fork's
//! initial values instead ([`super::ANONYMOUS_STYLE`]) — the anonymous box
//! CSS wraps a text run in.

use neutron_star::geometry::{Edges, Point, Size};
use neutron_star::style::{
    AspectRatio, Au, BorderSideWidth, Contain, ContainIntrinsicSize, ContentDistribution,
    CoreStyle, Display, FlexBasis, FlexContainerStyle, FlexItemStyle, GridAutoFlow,
    GridContainerStyle, GridItemStyle, GridLine, GridTemplateComponent, ImplicitGridTracks, Inset,
    ItemPlacement, JustifyItems, LinearContainerStyle, LinearItemStyle, Margin, MaxSize,
    NonNegativeLengthPercentage, NonNegativeLengthPercentageOrNormal, NonNegativeNumber, Overflow,
    PositionProperty, RelativeAlign, RelativeContainerStyle, RelativeItemStyle, RelativeReference,
    SelfAlignment, StyleSize, box_sizing, direction, flex_direction, flex_wrap, linear_direction,
    relative_center, relative_layout_once, visibility,
};
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Position as PositionStruct;
use stylo::servo_arc::Arc;
use stylo::values::computed::motion::OffsetPath;
use stylo::values::specified::box_::{DisplayInside, DisplayOutside, WillChangeBits};

use crate::contain::{ContentVisibility, effective_containment};
use crate::node::Node;

/// How the host dispatch lays a generated box out — the `display` projection
/// this integration routes on (neutron-star deliberately has no such enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    /// `display: none` — no box; the subtree is hidden.
    None,
    /// CSS flexbox.
    Flex,
    /// CSS Grid.
    Grid,
    /// Starlight `display: linear`.
    Linear,
    /// Starlight `display: relative`.
    Relative,
    /// Flow (block/inline) and `display: contents` — **not implemented** as
    /// container layout: the node is laid out as a leaf (its own box styles
    /// apply, content measured through the embedder hook) and any children
    /// are zeroed. Flow containers never survive in a Lynx tree (the
    /// embedder's UA sheet assigns every element a supported display) and
    /// neutron-star has no block/inline-flow algorithm yet.
    Leaf,
}

/// Project stylo's `display` onto the dispatch routing.
pub(crate) fn display_mode(display: Display) -> DisplayMode {
    if display.outside() == DisplayOutside::None {
        // `none` and `contents` both have no outside display. `contents`
        // should splice its children into the parent's formatting context;
        // until that flattening exists, treat it as a leaf so its own box
        // vanishes-ish (zero-size) rather than hiding content silently.
        return if display.inside() == DisplayInside::Contents {
            DisplayMode::Leaf
        } else {
            DisplayMode::None
        };
    }
    match display.inside() {
        DisplayInside::None => DisplayMode::None,
        DisplayInside::Flex => DisplayMode::Flex,
        DisplayInside::Grid => DisplayMode::Grid,
        DisplayInside::LynxLinear => DisplayMode::Linear,
        DisplayInside::LynxRelative => DisplayMode::Relative,
        DisplayInside::Contents | DisplayInside::Flow => DisplayMode::Leaf,
    }
}

/// Whether `node` (the element `style` was computed for) is the document's
/// root element — the sole element child of the document node.
fn is_root_element<T>(node: &Node<T>) -> bool {
    node.parent().is_none_or(Node::is_document)
}

/// Whether a box with `style` skips laying out its contents —
/// `content-visibility: hidden`.
///
/// `auto` deliberately stays `false`: v1 has no relevance tracking, so an
/// `auto` box is always treated as on-screen and lays its contents out
/// normally (the relevance/skipping signal is the deferred, host-pushed bit
/// recorded in `docs/style-assumptions.md` §F.19). Only `hidden` — whose
/// contents are *always* skipped — returns `true`. The single source of truth
/// for both [`StyleView::skips_contents`](CoreStyle::skips_contents) and the
/// positioned pass's skip-root pruning.
pub(crate) fn skips_contents(style: &ComputedValues) -> bool {
    style.clone_content_visibility() == ContentVisibility::Hidden
}

/// Whether `node` establishes the containing block for `position: fixed`
/// descendants (and, a fortiori, for `absolute` ones) per CSS Transforms /
/// Motion Path / Filter Effects / `will-change` / css-contain-2.
///
/// This is the **real W3C rule** the repository's standards policy mandates
/// for `position: fixed` — not Lynx's unconditional escape-to-root behavior.
/// The node itself is needed for one spec carve-out: Filter Effects §5
/// exempts the **document root element**, whose `filter` does not create a
/// containing block.
///
/// The css-contain-2 leg reads **effective** containment
/// ([`effective_containment`], folding in what `content-visibility` implies),
/// so a `content-visibility: hidden` *or* `auto` box establishes the CB too —
/// each contributes layout + paint containment — not only a raw
/// `contain: layout`/`paint`. The effect bits are queried individually
/// (`LAYOUT`/`PAINT`), never the `CONTENT`/`STRICT` composite markers.
pub(crate) fn establishes_fixed_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    let box_style = style.get_box();
    !box_style.transform.0.is_empty()
        || !matches!(
            box_style.perspective,
            stylo::values::generics::box_::Perspective::None
        )
        // Motion Path: a non-none `offset-path` has "the usual transform
        // property effects", containing-block creation included.
        || !matches!(box_style.offset_path, OffsetPath::None)
        // Will Change §2: naming a property whose non-initial values create
        // a containing block must create one too. `transform`-family and
        // `contain` names set TRANSFORM/PERSPECTIVE/CONTAIN — no root
        // carve-out, exactly like the real properties.
        || box_style.will_change.bits.intersects(
            WillChangeBits::TRANSFORM | WillChangeBits::PERSPECTIVE | WillChangeBits::CONTAIN,
        )
        // FIXPOS_CB_NON_SVG is the `filter`-family proxy bit; like the real
        // `filter` below it must reproduce Filter Effects §5's document-root
        // exemption (the WPT will-change fixed-CB suite pins this).
        || (box_style
            .will_change
            .bits
            .intersects(WillChangeBits::FIXPOS_CB_NON_SVG)
            && !is_root_element(node))
        // css-contain-2: layout **or** paint containment establishes the CB.
        // Read *effective* containment so `content-visibility` (hidden/auto,
        // both of which imply layout+paint) counts, not only a raw `contain`.
        // `skipped` mirrors P1-1 (derived from `hidden`); it only gates the
        // SIZE bit, which this predicate does not read, so it is consistent
        // either way. Effect bits queried individually — never CONTENT/STRICT.
        || effective_containment(style, skips_contents(style))
            .intersects(Contain::LAYOUT | Contain::PAINT)
        || (!style.get_effects().filter.0.is_empty() && !is_root_element(node))
}

/// Whether `node` establishes a containing block for `position: absolute`
/// descendants: any positioned element — including `will-change: position`,
/// which per Will Change §2 must reproduce the containing block a
/// non-initial `position` would create — plus everything that would already
/// capture a `fixed` descendant.
pub(crate) fn establishes_absolute_containing_block<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> bool {
    style.clone_position() != PositionProperty::Static
        || style
            .get_box()
            .will_change
            .bits
            .intersects(WillChangeBits::POSITION)
        || establishes_fixed_containing_block(node, style)
}

/// Resolve a node's engine positioning **scheme** — the one style
/// translation that must look at the parent.
///
/// The protocol reads [`PositionProperty::Absolute`] as "the layout parent
/// is the containing block" and [`PositionProperty::Fixed`] as "hoisted:
/// record the static position, the host's positioned pass completes it".
/// This maps the *computed* position onto that scheme per the W3C
/// containing-block rules; `static`/`relative`/`sticky` pass through
/// (`sticky` offsets are scroll-time, applied by a host post-pass that does
/// not exist yet).
pub(crate) fn resolve_position<T>(node: &Node<T>, style: &ComputedValues) -> PositionProperty {
    let parent_establishes = |fixed: bool| {
        node.parent().is_some_and(|parent| {
            parent.computed_style().is_some_and(|parent_style| {
                if fixed {
                    establishes_fixed_containing_block(parent, &parent_style)
                } else {
                    establishes_absolute_containing_block(parent, &parent_style)
                }
            })
        })
    };
    match style.clone_position() {
        computed @ (PositionProperty::Static
        | PositionProperty::Relative
        | PositionProperty::Sticky) => computed,
        PositionProperty::Absolute => {
            if parent_establishes(false) {
                PositionProperty::Absolute
            } else {
                PositionProperty::Fixed
            }
        }
        PositionProperty::Fixed => {
            if parent_establishes(true) {
                PositionProperty::Absolute
            } else {
                PositionProperty::Fixed
            }
        }
    }
}

/// Lower a Lynx logical relative reference pair onto a physical edge:
/// `inline-start`/`inline-end` map to `left`/`right` under `ltr` and
/// `right`/`left` under `rtl`; the logical value fills a physical side only
/// where the physical property is unset (`-1`).
fn lower_relative_logical(physical: i32, logical: i32) -> i32 {
    if physical == -1 { logical } else { physical }
}

/// The computed-style view neutron-star reads: the node handle (for the
/// parent-dependent `resolve_position`) plus its `ComputedValues`.
///
/// Constructed **when the engine requests the style** (`StyleView::of`) —
/// nothing is pre-collected. The view owns an `Arc` handle to the node's
/// own computed style (a refcount bump; the values themselves were
/// materialized by the style flush, once per style change) and lends field
/// references from it for as long as the engine holds the view, exactly the
/// lending discipline the engine's style protocol documents.
pub struct StyleView<'dom, T> {
    node: &'dom Node<T>,
    style: Arc<ComputedValues>,
}

impl<T> std::fmt::Debug for StyleView<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("StyleView")
            .field(&self.node.id())
            .finish()
    }
}

impl<'dom, T> StyleView<'dom, T> {
    /// The style lent for `node`: its computed style, fetched now, or the
    /// anonymous-box initial values for text nodes (and, defensively, any
    /// style-less node — only `display: none` descendants qualify, and only
    /// [`hide_subtree`](neutron_star::compute::hide_subtree) ever visits
    /// them, without reading styles).
    pub(crate) fn of(node: &'dom Node<T>) -> Self {
        let style = if node.is_text_node() {
            None
        } else {
            node.computed_style()
        };
        Self {
            node,
            style: style.unwrap_or_else(|| super::ANONYMOUS_STYLE.clone()),
        }
    }

    fn position_struct(&self) -> &PositionStruct {
        self.style.get_position()
    }
}

impl<T> CoreStyle for StyleView<'_, T> {
    fn display(&self) -> Display {
        self.style.clone_display()
    }

    fn visibility(&self) -> visibility::T {
        self.style.clone_visibility()
    }

    fn position(&self) -> PositionProperty {
        resolve_position(self.node, &self.style)
    }

    fn inset(&self) -> Edges<&Inset> {
        let position = self.position_struct();
        Edges {
            left: &position.left,
            right: &position.right,
            top: &position.top,
            bottom: &position.bottom,
        }
    }

    fn size(&self) -> Size<&StyleSize> {
        let position = self.position_struct();
        Size::new(&position.width, &position.height)
    }

    fn min_size(&self) -> Size<&StyleSize> {
        let position = self.position_struct();
        Size::new(&position.min_width, &position.min_height)
    }

    fn max_size(&self) -> Size<&MaxSize> {
        let position = self.position_struct();
        Size::new(&position.max_width, &position.max_height)
    }

    fn aspect_ratio(&self) -> AspectRatio {
        self.style.clone_aspect_ratio()
    }

    fn margin(&self) -> Edges<&Margin> {
        let margin = self.style.get_margin();
        Edges {
            left: &margin.margin_left,
            right: &margin.margin_right,
            top: &margin.margin_top,
            bottom: &margin.margin_bottom,
        }
    }

    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
        let padding = self.style.get_padding();
        Edges {
            left: &padding.padding_left,
            right: &padding.padding_right,
            top: &padding.padding_top,
            bottom: &padding.padding_bottom,
        }
    }

    fn border(&self) -> Edges<BorderSideWidth> {
        let border = self.style.get_border();
        let used = |width: Au, style: stylo::values::specified::BorderStyle| {
            BorderSideWidth(if style.none_or_hidden() { Au(0) } else { width })
        };
        Edges {
            left: used(border.border_left_width.0, border.border_left_style),
            right: used(border.border_right_width.0, border.border_right_style),
            top: used(border.border_top_width.0, border.border_top_style),
            bottom: used(border.border_bottom_width.0, border.border_bottom_style),
        }
    }

    fn overflow(&self) -> Point<Overflow> {
        Point::new(self.style.clone_overflow_x(), self.style.clone_overflow_y())
    }

    fn box_sizing(&self) -> box_sizing::T {
        self.style.clone_box_sizing()
    }

    fn direction(&self) -> direction::T {
        self.style.clone_direction()
    }

    /// The box's **effective** CSS containment: the raw `contain` value folded
    /// with what `content-visibility` implies (see [`effective_containment`]).
    ///
    /// The `skipped` argument is hard-coded `false` here, which stays correct
    /// for both `content-visibility` values this view reports:
    /// - **`hidden`** — the fold's `hidden` arm adds `SIZE | LAYOUT | PAINT | STYLE` *regardless*
    ///   of `skipped`, so a hidden box is size-contained (hence a [relayout
    ///   boundary](neutron_star::invalidate::is_relayout_boundary)) even with `false`.
    ///   [`skips_contents`](CoreStyle::skips_contents) reports `true` for it, routing dispatch to
    ///   the skipped-contents layout.
    /// - **`auto`** — v1 has no relevance tracking, so an `auto` box is treated as on-screen and
    ///   must *not* be size-contained, which `false` gives (the fold's `auto` arm adds `SIZE` only
    ///   when `skipped`). It still contributes `LAYOUT | PAINT | STYLE` — enough to establish a
    ///   containing block for fixed/absolute descendants, but not a relayout boundary.
    ///
    /// A `contain: strict` box is likewise a relayout boundary, through the raw
    /// `SIZE | LAYOUT` effect bits.
    fn containment(&self) -> Contain {
        effective_containment(&self.style, false)
    }

    fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
        self.style.clone_contain_intrinsic_width()
    }

    fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
        self.style.clone_contain_intrinsic_height()
    }

    /// Whether this box skips laying out its contents: `content-visibility:
    /// hidden` returns `true`, `auto` returns `false`.
    ///
    /// `auto`'s relevance signal is deferred in v1 (there is no event layer to
    /// flip it — `docs/style-assumptions.md` §F.19), so an `auto` box is always
    /// treated as on-screen and lays its contents out normally; only `hidden`
    /// skips. When this is `true`, host dispatch routes the node to
    /// [`compute_skipped_contents_layout`](neutron_star::compute::compute_skipped_contents_layout)
    /// (before the cache wrapper), and [`containment`](CoreStyle::containment)
    /// already reports `SIZE | LAYOUT | PAINT | STYLE` for `hidden` — so a hidden
    /// box is a relayout boundary and a fixed/absolute containing block
    /// independently of this flag.
    fn skips_contents(&self) -> bool {
        skips_contents(&self.style)
    }
}

impl<T> FlexContainerStyle for StyleView<'_, T> {
    fn flex_direction(&self) -> flex_direction::T {
        self.style.clone_flex_direction()
    }

    fn flex_wrap(&self) -> flex_wrap::T {
        self.style.clone_flex_wrap()
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        let position = self.position_struct();
        Size::new(&position.column_gap, &position.row_gap)
    }

    fn align_content(&self) -> ContentDistribution {
        self.position_struct().align_content
    }

    fn align_items(&self) -> ItemPlacement {
        self.position_struct().align_items
    }

    fn justify_content(&self) -> ContentDistribution {
        self.position_struct().justify_content
    }
}

impl<T> FlexItemStyle for StyleView<'_, T> {
    fn flex_basis(&self) -> &FlexBasis {
        &self.position_struct().flex_basis
    }

    fn flex_grow(&self) -> NonNegativeNumber {
        self.position_struct().flex_grow
    }

    fn flex_shrink(&self) -> NonNegativeNumber {
        self.position_struct().flex_shrink
    }

    fn align_self(&self) -> SelfAlignment {
        self.position_struct().align_self
    }

    fn order(&self) -> i32 {
        self.position_struct().order
    }
}

impl<T> GridContainerStyle for StyleView<'_, T> {
    fn grid_template_rows(&self) -> &GridTemplateComponent {
        &self.position_struct().grid_template_rows
    }

    fn grid_template_columns(&self) -> &GridTemplateComponent {
        &self.position_struct().grid_template_columns
    }

    fn grid_auto_rows(&self) -> &ImplicitGridTracks {
        &self.position_struct().grid_auto_rows
    }

    fn grid_auto_columns(&self) -> &ImplicitGridTracks {
        &self.position_struct().grid_auto_columns
    }

    fn grid_auto_flow(&self) -> GridAutoFlow {
        self.position_struct().grid_auto_flow
    }

    fn gap(&self) -> Size<&NonNegativeLengthPercentageOrNormal> {
        FlexContainerStyle::gap(self)
    }

    fn align_content(&self) -> ContentDistribution {
        FlexContainerStyle::align_content(self)
    }

    fn justify_content(&self) -> ContentDistribution {
        FlexContainerStyle::justify_content(self)
    }

    fn align_items(&self) -> ItemPlacement {
        FlexContainerStyle::align_items(self)
    }

    fn justify_items(&self) -> JustifyItems {
        self.position_struct().justify_items
    }
}

impl<T> GridItemStyle for StyleView<'_, T> {
    fn grid_row_start(&self) -> &GridLine {
        &self.position_struct().grid_row_start
    }

    fn grid_row_end(&self) -> &GridLine {
        &self.position_struct().grid_row_end
    }

    fn grid_column_start(&self) -> &GridLine {
        &self.position_struct().grid_column_start
    }

    fn grid_column_end(&self) -> &GridLine {
        &self.position_struct().grid_column_end
    }

    fn align_self(&self) -> SelfAlignment {
        FlexItemStyle::align_self(self)
    }

    fn justify_self(&self) -> SelfAlignment {
        self.position_struct().justify_self
    }

    fn order(&self) -> i32 {
        FlexItemStyle::order(self)
    }
}

impl<T> LinearContainerStyle for StyleView<'_, T> {
    fn linear_direction(&self) -> linear_direction::T {
        self.style.clone_linear_direction()
    }

    fn linear_weight_sum(&self) -> NonNegativeNumber {
        self.style.clone_linear_weight_sum()
    }

    fn justify_content(&self) -> ContentDistribution {
        FlexContainerStyle::justify_content(self)
    }

    fn align_items(&self) -> ItemPlacement {
        FlexContainerStyle::align_items(self)
    }
}

impl<T> LinearItemStyle for StyleView<'_, T> {
    fn linear_weight(&self) -> NonNegativeNumber {
        self.style.clone_linear_weight()
    }

    fn align_self(&self) -> SelfAlignment {
        FlexItemStyle::align_self(self)
    }

    fn order(&self) -> i32 {
        FlexItemStyle::order(self)
    }
}

impl<T> RelativeContainerStyle for StyleView<'_, T> {
    fn relative_layout_once(&self) -> relative_layout_once::T {
        self.style.clone_relative_layout_once()
    }
}

impl<T> RelativeItemStyle for StyleView<'_, T> {
    fn relative_id(&self) -> RelativeReference {
        self.style.clone_relative_id()
    }

    fn relative_align(&self) -> Edges<RelativeAlign> {
        let ltr = self.style.clone_direction() == direction::T::Ltr;
        let (inline_start, inline_end) = (
            self.style.clone_relative_align_inline_start(),
            self.style.clone_relative_align_inline_end(),
        );
        let (logical_left, logical_right) = if ltr {
            (inline_start, inline_end)
        } else {
            (inline_end, inline_start)
        };
        Edges {
            left: lower_relative_logical(self.style.clone_relative_align_left(), logical_left),
            right: lower_relative_logical(self.style.clone_relative_align_right(), logical_right),
            top: self.style.clone_relative_align_top(),
            bottom: self.style.clone_relative_align_bottom(),
        }
    }

    fn relative_adjacent(&self) -> Edges<RelativeReference> {
        let ltr = self.style.clone_direction() == direction::T::Ltr;
        let (inline_start, inline_end) = (
            self.style.clone_relative_inline_start_of(),
            self.style.clone_relative_inline_end_of(),
        );
        let (logical_left, logical_right) = if ltr {
            (inline_start, inline_end)
        } else {
            (inline_end, inline_start)
        };
        Edges {
            left: lower_relative_logical(self.style.clone_relative_left_of(), logical_left),
            right: lower_relative_logical(self.style.clone_relative_right_of(), logical_right),
            top: self.style.clone_relative_top_of(),
            bottom: self.style.clone_relative_bottom_of(),
        }
    }

    fn relative_center(&self) -> relative_center::T {
        self.style.clone_relative_center()
    }

    fn order(&self) -> i32 {
        FlexItemStyle::order(self)
    }
}
