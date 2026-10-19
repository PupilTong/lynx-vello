//! What the two paragraph-establishing boxes share.
//!
//! A Lynx text element lays its flattened subtree out as one Lynx paragraph
//! (`text_block`); a block container — the `(block, flow)`
//! display — lays its inline content out as a CSS inline formatting context
//! (`inline_flow`). Both keep one retained parley paragraph in layout state,
//! collect their runs with the same per-run reads, measure and place their
//! atoms the same way, go through hughie's leaf box wrapper with the same
//! measure closure, and are painted from the same record. This module holds
//! those shared parts:
//!
//! - [`TextStore`], the one retained paragraph a node's layout state holds, of either kind, and the
//!   probe/restore calls the pass routes through it;
//! - [`PaintedParagraph`], a committed paragraph as the painter and the text clip read it;
//! - the run collection pieces: [`Collected`], the generated-content replacement, and the
//!   `white-space-collapse` read;
//! - [`measure`], the closure both box wrappers run, and the `text-indent` resolution in it;
//! - [`place_and_hide`], the commit's placement: the atoms where the paragraph put them, an empty
//!   layout for each consumed element on the way to one, every other consumed node hidden, then the
//!   out-of-flow boxes, laid out or recorded for their containing block, from the static position
//!   each kind gives them;
//! - [`paragraph_of`] and [`inline_source_range`], which answer for either kind which paragraph a
//!   node is content of and which source units an element contributes to it.

use std::hash::{Hash, Hasher};

use hughie::compute::{LeafMeasureInput, LeafMetrics};
use hughie::geometry::{Edges, Point, Rect, Size};
use hughie::style::{CoreStyle, PositionProperty};
use hughie::text::TextContext;
use hughie::text::block::{BlockConstraint, BlockMetrics, PlacedBox, SourceItem, TextBlock};
use hughie::text::inline::{FlowSource, InlineEdges, InlineFlow, InlineFragment};
use hughie::text::paragraph::{BreakStyle, Direction, LineGeometry, RunStyle, WhiteSpaceCollapse};
use hughie::tree::AvailableSpace;
use stylo::properties::ComputedValues;
use stylo::values::computed::{Content, ContentItem};

use crate::layout::TextBrush;
use crate::layout::inline_flow::{InlineBoxInsets, InlineFlowStore, is_inline_box};
use crate::layout::style::{
    DisplayMode, StyleView, TextRunView, box_parent, display_mode, inline_style_of,
};
use crate::layout::text_block::{Atom, TextBlockStore, inline_scopes, out_of_flow};
use crate::tree::document::{DocumentLayoutState, NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;

/// The retained paragraph one element's layout state holds.
#[expect(
    clippy::large_enum_variant,
    reason = "held boxed, once per paragraph-establishing element; the variants' size \
              difference costs no other node anything"
)]
pub(crate) enum TextStore {
    /// A Lynx text element's flattened Lynx paragraph.
    Block(TextBlockStore),
    /// A block container's inline formatting context.
    Flow(InlineFlowStore),
}

impl TextStore {
    pub(crate) const fn as_block(&self) -> Option<&TextBlockStore> {
        match self {
            Self::Block(store) => Some(store),
            Self::Flow(_) => None,
        }
    }

    pub(crate) const fn as_block_mut(&mut self) -> Option<&mut TextBlockStore> {
        match self {
            Self::Block(store) => Some(store),
            Self::Flow(_) => None,
        }
    }

    pub(crate) const fn as_flow(&self) -> Option<&InlineFlowStore> {
        match self {
            Self::Flow(store) => Some(store),
            Self::Block(_) => None,
        }
    }

    pub(crate) const fn as_flow_mut(&mut self) -> Option<&mut InlineFlowStore> {
        match self {
            Self::Flow(store) => Some(store),
            Self::Block(_) => None,
        }
    }

    /// How many times this element's paragraph has been shaped from scratch.
    pub(crate) const fn rebuilds(&self) -> u32 {
        match self {
            Self::Block(store) => store.rebuilds,
            Self::Flow(store) => store.rebuilds,
        }
    }

    /// Whether a probe left the retained paragraph off its committed lines.
    pub(crate) fn is_probe_dirty(&self) -> bool {
        match self {
            Self::Block(store) => store.block.is_probe_dirty(),
            Self::Flow(store) => store.flow.is_probe_dirty(),
        }
    }

    /// Puts the retained paragraph back on its committed lines.
    pub(crate) fn restore_committed(&mut self, context: &mut TextContext) -> bool {
        match self {
            Self::Block(store) => store.block.restore_committed(context),
            Self::Flow(store) => store.flow.restore_committed(context),
        }
    }

    /// The narrowest the paragraph can be at `indent`, which both kinds count
    /// on the lines it applies to.
    fn min_content_width(&mut self, context: &mut TextContext, indent: f32) -> f32 {
        match self {
            Self::Block(store) => store.block.min_content_width(context, indent),
            Self::Flow(store) => store.flow.min_content_width(context, indent),
        }
    }

    fn probe(&mut self, context: &mut TextContext, constraint: BlockConstraint) -> BlockMetrics {
        match self {
            Self::Block(store) => store.block.probe(context, constraint),
            Self::Flow(store) => store.flow.probe(context, constraint),
        }
    }

    fn commit(&mut self, context: &mut TextContext, constraint: BlockConstraint) -> BlockMetrics {
        match self {
            Self::Block(store) => store.block.commit(context, constraint),
            Self::Flow(store) => store.flow.commit(context, constraint),
        }
    }

    /// The paragraph as paint reads it, if a commit has produced one.
    ///
    /// `None` rather than a panic, as for [`TextBlockStore::committed`]: a
    /// paragraph no commit laid out does not paint.
    pub(crate) fn painted(&self) -> Option<PaintedParagraph<'_>> {
        match self {
            Self::Block(store) => {
                let block = store.committed()?;
                Some(PaintedParagraph {
                    layout: block.display(),
                    lines: block.line_geometry(),
                    sources: &store.source_ids,
                    truncation_sources: &store.truncation_source_ids,
                    kind: PaintedKind::Block(block),
                })
            }
            Self::Flow(store) => {
                let flow = store.committed()?;
                Some(PaintedParagraph {
                    layout: flow.display(),
                    lines: flow.line_geometry(),
                    sources: &store.source_ids,
                    truncation_sources: &[],
                    kind: PaintedKind::Flow(flow, &store.insets),
                })
            }
        }
    }

    /// The committed paragraph's source-range boxes
    /// ([`TextBlock::range_boxes`], [`InlineFlow::range_boxes`]).
    pub(crate) fn range_boxes(
        &self,
        start: u32,
        end: u32,
    ) -> Option<Vec<hughie::geometry::Rect<f32>>> {
        match self {
            Self::Block(store) => store.committed()?.range_boxes(start, end),
            Self::Flow(store) => store.committed()?.range_boxes(start, end),
        }
    }

    /// The source unit under `point` in the committed paragraph.
    pub(crate) fn index_at_point(&self, point: Point<f32>) -> Option<u32> {
        match self {
            Self::Block(store) => store.committed()?.index_at_point(point),
            Self::Flow(store) => store.committed()?.index_at_point(point),
        }
    }

    /// The committed paragraph's measured size ([`TextBlock::size`],
    /// [`InlineFlow::size`]).
    pub(crate) fn committed_size(&self) -> Option<hughie::geometry::Size<f32>> {
        match self {
            Self::Block(store) => Some(store.committed()?.size()),
            Self::Flow(store) => Some(store.committed()?.size()),
        }
    }

    /// Whether a commit has produced the paragraph.
    pub(crate) fn has_committed(&self) -> bool {
        match self {
            Self::Block(store) => store.committed().is_some(),
            Self::Flow(store) => store.committed().is_some(),
        }
    }

    /// Item index → the node that contributed it.
    fn source_ids(&self) -> &[NodeId] {
        match self {
            Self::Block(store) => &store.source_ids,
            Self::Flow(store) => &store.source_ids,
        }
    }

    /// The source units item `item` contributed.
    fn item_source_range(&self, item: u32) -> Option<core::ops::Range<u32>> {
        match self {
            Self::Block(store) => store.committed()?.item_source_range(item),
            Self::Flow(store) => store.committed()?.item_source_range(item),
        }
    }
}

/// One committed paragraph, of either kind, as paint reads it.
pub(crate) struct PaintedParagraph<'a> {
    /// The laid-out glyph runs, placed on their line boxes.
    pub(crate) layout: &'a parley::Layout<TextBrush>,
    /// Where each reported line sits.
    pub(crate) lines: &'a [LineGeometry],
    /// Item index → the node that contributed it, for
    /// [`SourceItem::Content`] and [`SourceItem::Ellipsis`].
    pub(crate) sources: &'a [NodeId],
    /// The same for [`SourceItem::Truncation`]; empty for an inline
    /// formatting context, which has no truncation content.
    pub(crate) truncation_sources: &'a [NodeId],
    kind: PaintedKind<'a>,
}

enum PaintedKind<'a> {
    Block(&'a TextBlock),
    /// The flow, and its inline boxes' border and padding.
    Flow(&'a InlineFlow, &'a [InlineBoxInsets]),
}

impl PaintedParagraph<'_> {
    /// How many parley style indices the layout has.
    pub(crate) fn style_count(&self) -> usize {
        match self.kind {
            PaintedKind::Block(block) => block.style_count(),
            PaintedKind::Flow(flow, _) => flow.style_count(),
        }
    }

    /// The paint identity of one parley style index. Every run of an inline
    /// formatting context is content; style 0 of either kind is the strut,
    /// which no text is shaped in, and so is an inline box's edge cluster,
    /// whose glyph run draws nothing ([`Self::draws`]).
    pub(crate) fn source_of(&self, style_index: u16) -> SourceItem {
        match self.kind {
            PaintedKind::Block(block) => block.source_of(style_index),
            PaintedKind::Flow(flow, _) => match flow.source_of(style_index) {
                FlowSource::Run(item) => SourceItem::Content(item),
                FlowSource::Strut | FlowSource::Edge(_) => SourceItem::Strut,
            },
        }
    }

    /// Whether the glyph run of a parley style index is drawn: every one but
    /// an inline box's edge cluster, which is advance and no ink — no glyph,
    /// no decoration, no shadow (css-text-decor-3 draws a decoration across a
    /// box's text, not across its padding).
    pub(crate) fn draws(&self, style_index: u16) -> bool {
        match self.kind {
            PaintedKind::Block(_) => true,
            PaintedKind::Flow(flow, _) => {
                !matches!(flow.source_of(style_index), FlowSource::Edge(_))
            }
        }
    }

    /// Every atomic box's outcome.
    pub(crate) fn boxes(&self) -> &[PlacedBox] {
        match self.kind {
            PaintedKind::Block(block) => block.boxes(),
            PaintedKind::Flow(flow, _) => flow.atoms(),
        }
    }

    /// The inline boxes' fragments, by box in tree order: only an inline
    /// formatting context has inline boxes.
    pub(crate) fn fragments(&self) -> &[InlineFragment] {
        match self.kind {
            PaintedKind::Block(_) => &[],
            PaintedKind::Flow(flow, _) => flow.fragments(),
        }
    }

    /// The border and padding of the inline box whose `Open` is item `item`.
    pub(crate) fn insets(&self, item: u32) -> Option<&InlineBoxInsets> {
        match self.kind {
            PaintedKind::Block(_) => None,
            PaintedKind::Flow(_, insets) => insets.iter().find(|entry| entry.item == item),
        }
    }

    /// Whether the elements around this paragraph's runs paint their
    /// backgrounds as Lynx inline scopes — one fragment per line from their
    /// runs' extents (`paint::text::inline_background_fragments`).
    /// A `<text>`'s nested scopes do; an inline formatting context's inline
    /// boxes paint from their own fragments, with their borders and padding.
    pub(crate) const fn paints_scope_backgrounds(&self) -> bool {
        matches!(self.kind, PaintedKind::Block(_))
    }
}

/// The retained paragraph `element`'s layout state holds, beside the shared
/// shaping context.
pub(crate) fn context_and_store(
    state: &mut DocumentLayoutState,
    element: NodeSlot,
) -> Option<(&mut TextContext, &mut TextStore)> {
    let (context, slot) = state.text_store_parts(element);
    slot.as_deref_mut().map(|store| (context, store))
}

/// One flattened item and the node behind it, before ownership is taken.
pub(crate) struct Collected {
    pub(crate) item: OwnedItem,
    pub(crate) source: NodeSlot,
}

pub(crate) enum OwnedItem {
    Run {
        text: String,
        style: RunStyle,
        white_space: WhiteSpaceCollapse,
    },
    Atom {
        vertical_align: hughie::text::paragraph::VerticalAlign,
    },
    /// The start of a non-atomic inline box; only a CSS inline formatting
    /// context has one.
    Open {
        style: RunStyle,
        breaking: BreakStyle,
        direction: Direction,
        edges: InlineEdges,
    },
    /// The end of the innermost open inline box.
    Close,
}

/// Hashes one flow's structure: its length, and per item the node behind it
/// and a run's text and `white-space-collapse`.
pub(crate) fn hash_flow(collected: &[Collected], hasher: &mut impl Hasher) {
    collected.len().hash(hasher);
    for entry in collected {
        entry.source.hash(hasher);
        match &entry.item {
            OwnedItem::Run {
                text, white_space, ..
            } => {
                0_u8.hash(hasher);
                text.hash(hasher);
                white_space.hash(hasher);
            }
            OwnedItem::Atom { .. } => 1_u8.hash(hasher),
            OwnedItem::Open { .. } => 2_u8.hash(hasher),
            OwnedItem::Close => 3_u8.hash(hasher),
        }
    }
}

/// The supported content list replaces rendered children, including when its
/// strings resolve to empty. Unsupported lists fall back to ordinary children
/// as a whole; never render only a supported prefix of a list.
pub(crate) fn generated_content<T>(node: &Node<T>) -> Option<(&ComputedValues, &[ContentItem])> {
    if node.is_replaced() {
        return None;
    }
    let style = node.layout_computed_style()?;
    let Content::Items(content) = &style.get_counters().content else {
        return None;
    };
    let items = &content.items[..content.alt_start];
    items
        .iter()
        .all(|item| matches!(item, ContentItem::String(_)))
        .then_some((style, items))
}

/// Whether a paragraph-establishing display: one whose content is laid out
/// as runs rather than as child boxes.
pub(crate) const fn establishes_paragraph(mode: DisplayMode) -> bool {
    matches!(mode, DisplayMode::Text | DisplayMode::BlockFlow)
}

/// Whether this scope suppresses descendants during the later positioning walk.
/// `contents` only consumes text content when it belongs to a paragraph.
pub(crate) fn replaces_children<T>(node: &Node<T>) -> bool {
    let Some(style) = node.layout_computed_style() else {
        return false;
    };
    match display_mode(node, *style.get_display()) {
        mode if establishes_paragraph(mode) => generated_content(node).is_some(),
        DisplayMode::Contents => {
            generated_content(node).is_some()
                && box_parent(node).is_some_and(|parent| {
                    parent.layout_computed_style().is_some_and(|style| {
                        establishes_paragraph(display_mode(parent, *style.get_display()))
                    })
                })
        }
        _ => false,
    }
}

/// Returns whether generated text replaced this scope's rendered children.
pub(crate) fn collect_generated<T>(
    tree: &TreeArenas<T>,
    slot: NodeSlot,
    out: &mut Vec<Collected>,
) -> bool {
    let node = tree.at(slot);
    let Some((style, items)) = generated_content(node) else {
        return false;
    };
    let mut text = String::new();
    for item in items {
        match item {
            ContentItem::String(value) => text.push_str(value),
            _ => unreachable!("generated_content validated the list"),
        }
    }
    if !text.is_empty() {
        out.push(Collected {
            item: OwnedItem::Run {
                text,
                style: RunStyle::from_run_style(&TextRunView::from_values(style)),
                white_space: style_white_space(style),
            },
            source: slot,
        });
    }
    true
}

/// Which of its whitespace a run keeps.
///
/// Read from computed style, so no tag is named: a raw-text carrier's UA
/// rule sets `white-space-collapse: preserve-breaks`, an author's `pre` or
/// `white-space: pre` sets `preserve`, and either inherits into text runs.
pub(crate) fn run_white_space<T>(node: &Node<T>) -> WhiteSpaceCollapse {
    style_white_space(inline_style_of(node))
}

/// The fork's four keywords, one for one. A `<text>`'s verbatim paragraph
/// reads none of them (`BlockStyle::verbatim`).
pub(crate) fn style_white_space(style: &ComputedValues) -> WhiteSpaceCollapse {
    use stylo::computed_values::white_space_collapse::T as Collapse;
    match *style.get_inherited_text().get_white_space_collapse() {
        Collapse::Preserve => WhiteSpaceCollapse::Preserve,
        Collapse::PreserveBreaks => WhiteSpaceCollapse::PreserveBreaks,
        Collapse::BreakSpaces => WhiteSpaceCollapse::BreakSpaces,
        Collapse::Collapse => WhiteSpaceCollapse::Collapse,
    }
}

/// Resolves `text-indent`'s length against the definite inline size, which
/// is the caller's to know — the paragraph only ever sees a break width.
pub(crate) fn resolve_indent<T>(
    tree: &TreeArenas<T>,
    element: NodeSlot,
    basis: Option<f32>,
) -> f32 {
    use hughie::style::TextContainerStyle;
    use stylo::values::generics::text::GenericTextIndent;

    let view = StyleView::of(tree.at(element));
    let indent: GenericTextIndent<_> = view.text_indent();
    indent
        .length
        .to_used_value(app_units::Au::from_f32_px(basis.unwrap_or(0.0)))
        .to_f32_px()
}

/// The measure closure both paragraph boxes run inside hughie's leaf box
/// wrapper: resolves the break width the wrapper's input names, then probes
/// or commits the retained paragraph at it.
///
/// `basis` is the inline size a percentage `text-indent` resolves against.
/// A min-content request needs a width before a constraint exists, so it is
/// the one answer the memo cannot key on: the paragraph measures it first.
pub(crate) fn measure<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    basis: Option<f32>,
    measure_input: LeafMeasureInput,
) -> LeafMetrics {
    let indent = resolve_indent(tree, element, basis);
    let width = match measure_input.available_space.width {
        AvailableSpace::Definite(value) => Some(value),
        AvailableSpace::MaxContent => None,
        AvailableSpace::MinContent => {
            let Some((context, store)) = context_and_store(state, element) else {
                return LeafMetrics::new(Size::ZERO);
            };
            Some(store.min_content_width(context, indent))
        }
    };
    let width = measure_input.known_dimensions.width.or(width);
    let constraint = BlockConstraint::new(width, indent);
    let Some((context, store)) = context_and_store(state, element) else {
        return LeafMetrics::new(Size::ZERO);
    };
    let metrics = if measure_input.goal.commits() {
        store.commit(context, constraint)
    } else {
        store.probe(context, constraint)
    };
    LeafMetrics::new(metrics.size)
        .with_first_baselines(Point::new(None, metrics.first_baseline))
        .with_last_baselines(Point::new(None, metrics.last_baseline))
}

/// The element establishing the committed paragraph whose content `node` is
/// — `node` itself when it establishes one — of either kind.
///
/// Walks up by the two content walks' own rules (`text_block::collect_content`
/// and `inline_flow::collect_flow`), so a node answers exactly when one of
/// them would reach it: through text nodes, `contents` wrappers, a Lynx
/// paragraph's nested Lynx text scopes and an inline formatting context's
/// inline boxes, with `node` itself also allowed to be an in-flow atom.
/// Anything the walks never enter answers `None`: a node inside an atom's own
/// subtree (unless a paragraph nearer to it is found first), a `display: none`
/// or out-of-flow element and what is under it, and the children a
/// generated-content list replaced. A node in the `inline-truncation` content
/// answers the paragraph it truncates.
///
/// Loose text answers `None`: the anonymous block box that lays it out is the
/// engine's own node, never handed out (`tree::anonymous`).
pub(crate) fn paragraph_of<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    node: NodeSlot,
) -> Option<NodeSlot> {
    let mut current = node;
    loop {
        let here = tree.at(current);
        if here.is_element() {
            // An inline box is content of the flow around it, whatever its
            // layout state still holds from a time it laid itself out.
            let inline_box = is_inline_box(here);
            if !inline_box
                && state
                    .text_store(current)
                    .is_some_and(TextStore::has_committed)
            {
                // A replacing root shows none of its children.
                return (current == node || generated_content(here).is_none()).then_some(current);
            }
            let view = StyleView::try_of(here)?;
            if out_of_flow(hughie::style::CoreStyle::position(&view)) {
                return None;
            }
            match view.display_mode() {
                DisplayMode::Text | DisplayMode::Contents => {
                    if current != node && generated_content(here).is_some() {
                        return None;
                    }
                }
                DisplayMode::BlockFlow if inline_box => {
                    if current != node && generated_content(here).is_some() {
                        return None;
                    }
                }
                DisplayMode::None => return None,
                // An atom is one item of the paragraph above it; what is
                // inside it is not.
                _ if current == node => {}
                _ => return None,
            }
        } else if !here.is_text_node() {
            return None;
        }
        current = here
            .flat_parent()
            .and_then(|parent| tree.slot(parent.id()))?;
    }
}

/// The source units `element` contributes to the committed paragraph it is
/// inline content of: the union of its items' and its descendants'.
///
/// `None` for a text node (a run is attributed to the element whose style it
/// carries, not to its node), for the paragraph's own root, outside a
/// committed paragraph, and for an element that contributes no content item
/// — an empty scope or inline box, or one inside the truncation content,
/// which is no source text. An inline box's own boundaries count no unit.
pub(crate) fn inline_source_range<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    element: NodeSlot,
) -> Option<core::ops::Range<u32>> {
    if !tree.at(element).is_element() {
        return None;
    }
    let paragraph = paragraph_of(tree, state, element)?;
    if paragraph == element {
        return None;
    }
    let store = state.text_store(paragraph)?;
    if !store.has_committed() {
        return None;
    }
    let mut range: Option<core::ops::Range<u32>> = None;
    for (item, &source) in store.source_ids().iter().enumerate() {
        if !inline_scopes(tree, source, paragraph).any(|scope| scope == element) {
            continue;
        }
        let units = store.item_source_range(u32::try_from(item).ok()?)?;
        if units.is_empty() {
            continue;
        }
        range = Some(match range {
            Some(seen) => seen.start.min(units.start)..seen.end.max(units.end),
            None => units,
        });
    }
    range
}

/// The establishing element's own box model for this pass.
///
/// Resolved from style and from the size this pass just produced, never from
/// the element's layout slot: a parent writes a child's layout only once that
/// child's `compute_layout` has returned, so while this one runs its own slot
/// still holds the previous pass's box — or, on the first pass, none at all.
/// The inline basis is the one the box wrapper resolved its own padding
/// against, so the two cannot disagree.
pub(crate) struct BlockBox {
    size: Size<f32>,
    border: Edges<f32>,
    padding: Edges<f32>,
}

impl BlockBox {
    pub(crate) fn of<T>(
        view: &StyleView<'_, T>,
        size: Size<f32>,
        inline_basis: Option<f32>,
    ) -> Self {
        Self {
            size,
            border: hughie::compute::used_border(view),
            padding: hughie::compute::used_padding(view, inline_basis),
        }
    }

    /// The content-box origin, in the border-box space `Layout::location` means.
    pub(crate) fn content_origin(&self) -> Point<f32> {
        Point::new(
            self.border.left + self.padding.left,
            self.border.top + self.padding.top,
        )
    }

    /// The padding box: the containing block of an out-of-flow child.
    pub(crate) fn padding_box(&self) -> Size<f32> {
        Size::new(
            (self.size.width - self.border.horizontal_sum()).max(0.0),
            (self.size.height - self.border.vertical_sum()).max(0.0),
        )
    }
}

/// How a committed paragraph moves an atom from the place its line gave it.
#[derive(Clone, Copy)]
pub(crate) enum AtomOffset {
    /// Not at all: `left`/`top`/`right`/`bottom` on an in-flow atom of a Lynx
    /// paragraph are ignored, following native Lynx. `CalcRelativePosition`
    /// runs only over a `LayoutAlgorithm`'s in-flow items
    /// (`starlight/layout/layout_algorithm.cc:215-228`), and a `<text>` has a
    /// `measure_func_`, so it never builds one (`layout_object.cc:684-696`).
    /// This deviates from web-core, where `x-view` is a `position: relative`
    /// `inline-flex` box the browser shifts; user ruling 2026-09-17, recorded
    /// in `docs/tracking/deviations.md`. A relative atom sits exactly where a
    /// static one does.
    Ignored,
    /// CSS 2.2 §9.4.3: a `position: relative` atom keeps its place on the line
    /// and is moved from there by its insets, resolved against its containing
    /// block as the line was laid out in.
    Relative(Size<Option<f32>>),
}

/// An out-of-flow box a paragraph's element lays out, or records the static
/// position of for the containing block above it that lays it out.
#[derive(Clone, Copy)]
pub(crate) struct OutOfFlowBox {
    pub(crate) slot: NodeSlot,
    /// Its static position (CSS 2.2 §10.3.7, §10.6.4), in the element's
    /// border-box space: where the box's left margin edge and its top margin
    /// edge would have been.
    pub(crate) static_position: Point<f32>,
    /// Whether `static_position` is where the right margin edge would have
    /// been instead: the containing block is right-to-left, and CSS 2.2
    /// §10.3.7 sets `right` to the static position there.
    pub(crate) from_right: bool,
    /// Its containing block, in the element's border-box space, when that is
    /// not the element's padding box: a non-atomic inline box of the
    /// paragraph (CSS 2.2 §10.1.4).
    pub(crate) containing: Option<Rect<f32>>,
}

/// What one commit of a paragraph places.
pub(crate) struct Placement<'a> {
    /// Every atom, with the id the paragraph holds it by.
    pub(crate) atoms: &'a [Atom],
    /// Where the committed paragraph put them.
    pub(crate) placements: &'a [PlacedBox],
    pub(crate) offset: AtomOffset,
    /// The out-of-flow boxes the element lays out or records.
    pub(crate) out_of_flow: &'a [OutOfFlowBox],
}

/// Positions the atoms a committed paragraph placed, hides everything else it
/// consumed, then lays its out-of-flow boxes out: one pass for both kinds.
///
/// An atom keeps the box its own layout produced; the paragraph decides only
/// where it sits. A [`PlacedBox::Hidden`] atom — the Lynx `HideView` surface,
/// an atom truncation cut away; an inline formatting context hides none —
/// generates no box this frame.
pub(crate) fn place_and_hide<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    placement: &Placement<'_>,
    block: &BlockBox,
) {
    use hughie::tree::LayoutTree;

    let &Placement {
        atoms,
        placements,
        offset,
        out_of_flow,
    } = placement;
    if generated_content(tree.at(element)).is_some() {
        // Replacement suppresses every descendant, out-of-flow boxes included.
        let children: Vec<NodeSlot> = tree.children(element).collect();
        for child in children {
            hughie::compute::hide_subtree(tree, state, child);
        }
        return;
    }

    // Every slot the paragraph gave a real box, and every out-of-flow box it
    // lays out or records; then, separately, the consumed elements on the
    // path down to one. A consumed element generates no box of its own, but
    // hiding its subtree would hide the box under it, so it keeps an empty
    // layout instead.
    let placed_slots: Vec<NodeSlot> = placements
        .iter()
        .filter_map(|placed| match *placed {
            PlacedBox::Visible { id, .. } => atoms
                .iter()
                .find(|atom| atom.id == id)
                .map(|atom| atom.slot),
            PlacedBox::Hidden { .. } => None,
        })
        .collect();
    let reached: Vec<NodeSlot> = placed_slots
        .iter()
        .copied()
        .chain(out_of_flow.iter().map(|entry| entry.slot))
        .collect();
    let carriers = atom_carriers(tree, element, &reached);

    let content_origin = block.content_origin();
    for &placed in placements {
        let (id, origin) = match placed {
            PlacedBox::Visible { id, origin, .. } => (id, Some(origin)),
            PlacedBox::Hidden { id } => (id, None),
        };
        let Some(&atom) = atoms.iter().find(|atom| atom.id == id) else {
            continue;
        };
        let Some(origin) = origin else {
            hughie::compute::hide_subtree(tree, state, atom.slot);
            continue;
        };
        let shift = match offset {
            AtomOffset::Relative(containing) => {
                let view = tree.style(atom.slot);
                if CoreStyle::position(&view) == PositionProperty::Relative {
                    hughie::compute::used_relative_offset(&view, containing)
                } else {
                    Point::ZERO
                }
            }
            AtomOffset::Ignored => Point::ZERO,
        };
        let slot_layout = tree.layout_mut(state, atom.slot);
        let mut placed_layout = hughie::tree::Layout::with_order(slot_layout.unrounded.order);
        placed_layout.size = slot_layout.unrounded.size;
        placed_layout.content_size = slot_layout.unrounded.content_size;
        placed_layout.border = slot_layout.unrounded.border;
        placed_layout.padding = slot_layout.unrounded.padding;
        placed_layout.margin = slot_layout.unrounded.margin;
        // Two changes of space, in order. The paragraph's origin is the
        // element's *content* box, while `location` is read against its
        // border box; and the paragraph placed the atom's *margin* box, while
        // `location` names its border box.
        placed_layout.location = Point::new(
            content_origin.x + origin.x + atom.margin.left + shift.x,
            content_origin.y + origin.y + atom.margin.top + shift.y,
        );
        tree.set_unrounded_layout(state, atom.slot, placed_layout);
    }

    // Everything the paragraph swallowed generates no box. Hiding the whole
    // child rather than each consumed node keeps this O(children) wherever
    // the child holds no atom and no out-of-flow box: a nested scope's own
    // subtree is consumed with it.
    //
    // A consumed element that *does* hold one is written with an empty layout
    // instead of being marked hidden, and its children are visited under the
    // same rule: the marker subtree of a Lynx paragraph, whose atoms are
    // placed at the clamp, a `display: contents` wrapper, an inline box. An
    // empty layout is not a hidden one — it keeps the slot's paint order and,
    // because it is a write, keeps the rounding walk descending to the box
    // underneath. It sits at the element's border-box origin, so a static
    // position recorded in the element's space holds in the carrier's too.
    let mut stack: Vec<NodeSlot> = tree.children(element).collect();
    while let Some(child) = stack.pop() {
        // Atoms were just placed, and hiding one would zero the geometry this
        // pass gave it; an out-of-flow box is laid out below, or by its
        // containing block.
        if atoms.iter().any(|atom| atom.slot == child)
            || out_of_flow.iter().any(|entry| entry.slot == child)
        {
            continue;
        }
        if !carriers.contains(&child) {
            hughie::compute::hide_subtree(tree, state, child);
            continue;
        }
        let order = tree.layout_mut(state, child).unrounded.order;
        tree.set_unrounded_layout(state, child, hughie::tree::Layout::with_order(order));
        stack.extend(tree.children(child));
    }

    lay_out_out_of_flow(tree, state, element, block, out_of_flow);
}

/// The paragraph's absolute pass: the out-of-flow boxes its element lays out
/// or records, then the boxes hoisted to it.
fn lay_out_out_of_flow<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    block: &BlockBox,
    out_of_flow: &[OutOfFlowBox],
) {
    use hughie::tree::LayoutTree;

    let padding_box = block.padding_box();
    let padding_origin = Point::new(block.border.left, block.border.top);
    for entry in out_of_flow {
        let view = StyleView::of(tree.at(entry.slot));
        if CoreStyle::position(&view) == PositionProperty::Fixed {
            // Its containing block is above the element, whose pass lays it
            // out from this static position (`layout::hoisted`).
            tree.set_static_position(state, entry.slot, entry.static_position);
            continue;
        }
        let containing = entry
            .containing
            .unwrap_or_else(|| Rect::new(padding_origin, padding_box));
        let mut layout = hughie::compute::compute_absolute_layout(
            tree,
            state,
            entry.slot,
            containing.size,
            Point::new(
                entry.static_position.x - containing.origin.x,
                entry.static_position.y - containing.origin.y,
            ),
        );
        let inset = CoreStyle::inset(&view);
        if entry.from_right && inset.left.is_auto() && inset.right.is_auto() {
            // The static position was taken as the left margin edge's; it is
            // the right one's.
            layout.location.x -= layout.margin.left + layout.size.width + layout.margin.right;
        }
        // And back out to the border-box space `location` is read in.
        layout.location.x += containing.origin.x;
        layout.location.y += containing.origin.y;
        tree.set_unrounded_layout(state, entry.slot, layout);
    }
    // The boxes deeper down whose containing block this element is, after
    // its own.
    hughie::compute::compute_hoisted_children(
        tree,
        state,
        element,
        padding_box,
        block.border,
        StyleView::of(tree.at(element)).direction() == hughie::style::direction::T::Rtl,
    );
}

/// The consumed elements between `element` and each of `placed`, exclusive of
/// both ends.
///
/// Walks up from the boxes rather than down from the element: a paragraph has
/// far fewer placed boxes than nodes, and a shared prefix stops the walk.
fn atom_carriers<T>(tree: &TreeArenas<T>, element: NodeSlot, placed: &[NodeSlot]) -> Vec<NodeSlot> {
    let mut carriers = Vec::new();
    for &atom in placed {
        let mut current = atom;
        while current != element {
            let Some(parent) = tree
                .at(current)
                .flat_parent()
                .and_then(|parent| tree.slot(parent.id()))
            else {
                break;
            };
            if parent == element || carriers.contains(&parent) {
                break;
            }
            carriers.push(parent);
            current = parent;
        }
    }
    carriers
}
