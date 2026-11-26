//! What the two paragraph-establishing boxes share.
//!
//! A Lynx text element lays its flattened subtree out as one Lynx paragraph
//! (`text_block`); a block container — the `(block, flow)`
//! display — lays its inline content out as a CSS inline formatting context
//! (`inline_flow`). Both keep one retained parley paragraph in layout state,
//! collect their runs with the same per-run reads, measure and place their
//! atoms the same way, go through seldon's leaf box wrapper with the same
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

use std::borrow::Cow;
use std::hash::{Hash, Hasher};

use seldon::compute::{LeafMeasureInput, LeafMetrics};
use seldon::geometry::{Edges, Point, Rect, Size};
use seldon::style::{CoreStyle, PositionProperty};
use seldon::text::TextContext;
use seldon::text::block::{BlockConstraint, BlockMetrics, PlacedBox, SourceItem, TextBlock};
use seldon::text::inline::{FlowSource, InlineEdges, InlineFlow, InlineFragment};
use seldon::text::paragraph::{
    BreakStyle, Direction, OffsetMap, RunStyle, TextSecurity, WhiteSpaceCollapse,
};
use seldon::tree::AvailableSpace;
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

#[allow(
    clippy::large_enum_variant,
    reason = "held boxed, once per paragraph-establishing element; the variants' size \
              difference costs no other node anything. `allow`, not `expect`: the \
              difference crosses the lint's threshold on 64-bit targets only"
)]
pub(crate) enum TextStore {
    Block(TextBlockStore),
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

    pub(crate) const fn rebuilds(&self) -> u32 {
        match self {
            Self::Block(store) => store.rebuilds,
            Self::Flow(store) => store.rebuilds,
        }
    }

    pub(crate) fn is_probe_dirty(&self) -> bool {
        match self {
            Self::Block(store) => store.block.is_probe_dirty(),
            Self::Flow(store) => store.flow.is_probe_dirty(),
        }
    }

    pub(crate) fn restore_committed(&mut self, context: &mut TextContext) -> bool {
        match self {
            Self::Block(store) => store.block.restore_committed(context),
            Self::Flow(store) => store.flow.restore_committed(context),
        }
    }

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

    pub(crate) fn painted(&self) -> Option<PaintedParagraph<'_>> {
        match self {
            Self::Block(store) => {
                let block = store.committed()?;
                Some(PaintedParagraph {
                    layout: block.display(),
                    line_count: block.lines().len(),
                    sources: &store.source_ids,
                    truncation_sources: &store.truncation_source_ids,
                    kind: PaintedKind::Block(block),
                })
            }
            Self::Flow(store) => {
                let flow = store.committed()?;
                Some(PaintedParagraph {
                    layout: flow.display(),
                    line_count: flow.line_count(),
                    sources: &store.source_ids,
                    truncation_sources: &[],
                    kind: PaintedKind::Flow(flow, &store.insets),
                })
            }
        }
    }

    pub(crate) fn editable(&self) -> Option<EditableView<'_>> {
        match self {
            Self::Block(store) => {
                let block = store.committed()?;
                Some(EditableView {
                    layout: block.display(),
                    map: block.offset_map(),
                    sources: &store.source_ids,
                })
            }
            Self::Flow(store) => {
                let flow = store.committed()?;
                Some(EditableView {
                    layout: flow.display(),
                    map: flow.offset_map(),
                    sources: &store.source_ids,
                })
            }
        }
    }

    pub(crate) fn range_boxes(
        &self,
        start: u32,
        end: u32,
    ) -> Option<Vec<seldon::geometry::Rect<f32>>> {
        match self {
            Self::Block(store) => store.committed()?.range_boxes(start, end),
            Self::Flow(store) => store.committed()?.range_boxes(start, end),
        }
    }

    pub(crate) fn index_at_point(&self, point: Point<f32>) -> Option<u32> {
        match self {
            Self::Block(store) => store.committed()?.index_at_point(point),
            Self::Flow(store) => store.committed()?.index_at_point(point),
        }
    }

    pub(crate) fn committed_size(&self) -> Option<seldon::geometry::Size<f32>> {
        match self {
            Self::Block(store) => Some(store.committed()?.size()),
            Self::Flow(store) => Some(store.committed()?.size()),
        }
    }

    pub(crate) fn has_committed(&self) -> bool {
        match self {
            Self::Block(store) => store.committed().is_some(),
            Self::Flow(store) => store.committed().is_some(),
        }
    }

    fn source_ids(&self) -> &[NodeId] {
        match self {
            Self::Block(store) => &store.source_ids,
            Self::Flow(store) => &store.source_ids,
        }
    }

    fn item_source_range(&self, item: u32) -> Option<core::ops::Range<u32>> {
        match self {
            Self::Block(store) => store.committed()?.item_source_range(item),
            Self::Flow(store) => store.committed()?.item_source_range(item),
        }
    }
}

pub(crate) struct EditableView<'a> {
    pub(crate) layout: &'a parley::Layout<TextBrush>,
    pub(crate) map: OffsetMap<'a>,
    pub(crate) sources: &'a [NodeId],
}

pub(crate) struct PaintedParagraph<'a> {
    pub(crate) layout: &'a parley::Layout<TextBrush>,
    pub(crate) line_count: usize,
    pub(crate) sources: &'a [NodeId],
    pub(crate) truncation_sources: &'a [NodeId],
    kind: PaintedKind<'a>,
}

enum PaintedKind<'a> {
    Block(&'a TextBlock),
    Flow(&'a InlineFlow, &'a [InlineBoxInsets]),
}

impl PaintedParagraph<'_> {
    pub(crate) fn style_count(&self) -> usize {
        match self.kind {
            PaintedKind::Block(block) => block.style_count(),
            PaintedKind::Flow(flow, _) => flow.style_count(),
        }
    }

    pub(crate) fn source_of(&self, style_index: u16) -> SourceItem {
        match self.kind {
            PaintedKind::Block(block) => block.source_of(style_index),
            PaintedKind::Flow(flow, _) => match flow.source_of(style_index) {
                FlowSource::Run(item) => SourceItem::Content(item),
                FlowSource::Strut | FlowSource::Edge(_) => SourceItem::Strut,
            },
        }
    }

    pub(crate) fn draws(&self, style_index: u16) -> bool {
        match self.kind {
            PaintedKind::Block(block) => !matches!(block.source_of(style_index), SourceItem::Strut),
            PaintedKind::Flow(flow, _) => !matches!(
                flow.source_of(style_index),
                FlowSource::Edge(_) | FlowSource::Strut
            ),
        }
    }

    pub(crate) fn boxes(&self) -> &[PlacedBox] {
        match self.kind {
            PaintedKind::Block(block) => block.boxes(),
            PaintedKind::Flow(flow, _) => flow.atoms(),
        }
    }

    pub(crate) fn fragments(&self) -> &[InlineFragment] {
        match self.kind {
            PaintedKind::Block(_) => &[],
            PaintedKind::Flow(flow, _) => flow.fragments(),
        }
    }

    pub(crate) fn insets(&self, item: u32) -> Option<&InlineBoxInsets> {
        match self.kind {
            PaintedKind::Block(_) => None,
            PaintedKind::Flow(_, insets) => insets.iter().find(|entry| entry.item == item),
        }
    }

    pub(crate) const fn paints_scope_backgrounds(&self) -> bool {
        matches!(self.kind, PaintedKind::Block(_))
    }
}

pub(crate) fn context_and_store(
    state: &mut DocumentLayoutState,
    element: NodeSlot,
) -> Option<(&mut TextContext, &mut TextStore)> {
    let (context, slot) = state.text_store_parts(element);
    slot.as_deref_mut().map(|store| (context, store))
}

pub(crate) struct Collected<'tree> {
    pub(crate) item: OwnedItem<'tree>,
    pub(crate) source: NodeSlot,
}

pub(crate) enum OwnedItem<'tree> {
    Run {
        text: Cow<'tree, str>,
        style: RunStyle,
        white_space: WhiteSpaceCollapse,
        text_security: TextSecurity,
    },
    Atom {
        vertical_align: seldon::text::paragraph::VerticalAlign,
    },
    Open {
        style: RunStyle,
        breaking: BreakStyle,
        direction: Direction,
        edges: InlineEdges,
    },
    Close,
}

pub(crate) fn source_text_of(collected: &[Collected<'_>]) -> String {
    let mut text = String::new();
    for entry in collected {
        match &entry.item {
            OwnedItem::Run { text: run, .. } => text.push_str(run),
            OwnedItem::Atom { .. } => text.push('\u{FFFC}'),
            OwnedItem::Open { .. } | OwnedItem::Close => {}
        }
    }
    text
}

pub(crate) fn hash_flow(collected: &[Collected<'_>], hasher: &mut impl Hasher) {
    collected.len().hash(hasher);
    for entry in collected {
        entry.source.hash(hasher);
        match &entry.item {
            OwnedItem::Run {
                text,
                white_space,
                text_security,
                ..
            } => {
                0_u8.hash(hasher);
                text.hash(hasher);
                white_space.hash(hasher);
                text_security.hash(hasher);
            }
            OwnedItem::Atom { .. } => 1_u8.hash(hasher),
            OwnedItem::Open { .. } => 2_u8.hash(hasher),
            OwnedItem::Close => 3_u8.hash(hasher),
        }
    }
}

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

pub(crate) const fn establishes_paragraph(mode: DisplayMode) -> bool {
    matches!(mode, DisplayMode::Text | DisplayMode::BlockFlow)
}

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

pub(crate) fn collect_generated<T>(
    tree: &TreeArenas<T>,
    slot: NodeSlot,
    out: &mut Vec<Collected<'_>>,
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
                text: Cow::Owned(text),
                style: RunStyle::from_run_style(&TextRunView::from_values(style)),
                white_space: style_white_space(style),
                text_security: style_text_security(style),
            },
            source: slot,
        });
    }
    true
}

pub(crate) fn run_white_space<T>(node: &Node<T>) -> WhiteSpaceCollapse {
    style_white_space(inline_style_of(node))
}

pub(crate) fn style_white_space(style: &ComputedValues) -> WhiteSpaceCollapse {
    use stylo::computed_values::white_space_collapse::T as Collapse;
    match *style.get_inherited_text().get_white_space_collapse() {
        Collapse::Preserve => WhiteSpaceCollapse::Preserve,
        Collapse::PreserveBreaks => WhiteSpaceCollapse::PreserveBreaks,
        Collapse::BreakSpaces => WhiteSpaceCollapse::BreakSpaces,
        Collapse::Collapse => WhiteSpaceCollapse::Collapse,
    }
}

pub(crate) fn run_text_security<T>(node: &Node<T>) -> TextSecurity {
    style_text_security(inline_style_of(node))
}

pub(crate) fn style_text_security(style: &ComputedValues) -> TextSecurity {
    use stylo::values::computed::TextSecurity as Security;
    match *style.get_inherited_text().get__webkit_text_security() {
        Security::None => TextSecurity::None,
        Security::Circle => TextSecurity::Circle,
        Security::Disc => TextSecurity::Disc,
        Security::Square => TextSecurity::Square,
    }
}

pub(crate) fn resolve_indent<T>(
    tree: &TreeArenas<T>,
    element: NodeSlot,
    basis: Option<f32>,
) -> f32 {
    use seldon::style::TextContainerStyle;
    use stylo::values::generics::text::GenericTextIndent;

    let view = StyleView::of(tree.at(element));
    let indent: GenericTextIndent<_> = view.text_indent();
    indent
        .length
        .to_used_value(app_units::Au::from_f32_px(basis.unwrap_or(0.0)))
        .to_f32_px()
}

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
        .with_first_baseline(metrics.first_baseline)
        .with_last_baseline(metrics.last_baseline)
}

pub(crate) fn paragraph_of<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    node: NodeSlot,
) -> Option<NodeSlot> {
    let mut current = node;
    loop {
        let here = tree.at(current);
        if here.is_element() {
            let inline_box = is_inline_box(here);
            if !inline_box
                && state
                    .text_store(current)
                    .is_some_and(TextStore::has_committed)
            {
                return (current == node || generated_content(here).is_none()).then_some(current);
            }
            let view = StyleView::try_of(here)?;
            if out_of_flow(seldon::style::CoreStyle::position(&view)) {
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
            border: seldon::compute::used_border(view),
            padding: seldon::compute::used_padding(view, inline_basis),
        }
    }

    pub(crate) fn content_origin(&self) -> Point<f32> {
        Point::new(
            self.border.left + self.padding.left,
            self.border.top + self.padding.top,
        )
    }

    pub(crate) fn padding_box(&self) -> Size<f32> {
        Size::new(
            (self.size.width - self.border.horizontal_sum()).max(0.0),
            (self.size.height - self.border.vertical_sum()).max(0.0),
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) enum AtomOffset {
    Ignored,
    Relative(Size<Option<f32>>),
}

#[derive(Clone, Copy)]
pub(crate) struct OutOfFlowBox {
    pub(crate) slot: NodeSlot,
    pub(crate) static_position: Point<f32>,
    pub(crate) from_right: bool,
    pub(crate) containing: Option<Rect<f32>>,
}

pub(crate) struct Placement<'a> {
    pub(crate) atoms: &'a [Atom],
    pub(crate) placements: &'a [PlacedBox],
    pub(crate) offset: AtomOffset,
    pub(crate) out_of_flow: &'a [OutOfFlowBox],
}

pub(crate) fn place_and_hide<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    placement: &Placement<'_>,
    block: &BlockBox,
) {
    use seldon::tree::LayoutTree;

    let &Placement {
        atoms,
        placements,
        offset,
        out_of_flow,
    } = placement;
    if generated_content(tree.at(element)).is_some() {
        let children: Vec<NodeSlot> = tree.children(element).collect();
        for child in children {
            seldon::compute::hide_subtree(tree, state, child);
        }
        return;
    }

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
            seldon::compute::hide_subtree(tree, state, atom.slot);
            continue;
        };
        let shift = match offset {
            AtomOffset::Relative(containing) => {
                let view = tree.style(atom.slot);
                if CoreStyle::position(&view) == PositionProperty::Relative {
                    seldon::compute::used_relative_offset(&view, containing)
                } else {
                    Point::ZERO
                }
            }
            AtomOffset::Ignored => Point::ZERO,
        };
        let slot_layout = tree.layout_mut(state, atom.slot);
        let mut placed_layout = slot_layout.unrounded.clone();
        placed_layout.location = Point::new(
            content_origin.x + origin.x + atom.margin.left + shift.x,
            content_origin.y + origin.y + atom.margin.top + shift.y,
        );
        tree.set_unrounded_layout(state, atom.slot, placed_layout);
    }

    let mut stack: Vec<NodeSlot> = tree.children(element).collect();
    while let Some(child) = stack.pop() {
        if atoms.iter().any(|atom| atom.slot == child)
            || out_of_flow.iter().any(|entry| entry.slot == child)
        {
            continue;
        }
        if !carriers.contains(&child) {
            seldon::compute::hide_subtree(tree, state, child);
            continue;
        }
        let order = tree.layout_mut(state, child).unrounded.order;
        tree.set_unrounded_layout(state, child, seldon::tree::Layout::with_order(order));
        stack.extend(tree.children(child));
    }

    lay_out_out_of_flow(tree, state, element, block, out_of_flow);
}

fn lay_out_out_of_flow<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    block: &BlockBox,
    out_of_flow: &[OutOfFlowBox],
) {
    use seldon::tree::LayoutTree;

    let padding_box = block.padding_box();
    let padding_origin = Point::new(block.border.left, block.border.top);
    for entry in out_of_flow {
        let view = StyleView::of(tree.at(entry.slot));
        if CoreStyle::position(&view) == PositionProperty::Fixed {
            tree.set_static_position(state, entry.slot, entry.static_position);
            continue;
        }
        let containing = entry
            .containing
            .unwrap_or_else(|| Rect::new(padding_origin, padding_box));
        let mut layout = seldon::compute::compute_absolute_layout(
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
            layout.location.x -= layout.margin.left + layout.size.width + layout.margin.right;
        }
        layout.location.x += containing.origin.x;
        layout.location.y += containing.origin.y;
        tree.set_unrounded_layout(state, entry.slot, layout);
    }
    seldon::compute::compute_hoisted_children(
        tree,
        state,
        element,
        padding_box,
        block.border,
        StyleView::of(tree.at(element)).direction() == seldon::style::direction::T::Rtl,
    );
}

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
