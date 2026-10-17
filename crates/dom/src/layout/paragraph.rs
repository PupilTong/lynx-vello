//! What the two paragraph-establishing boxes share.
//!
//! A `display: -lynx-text` element lays its flattened subtree out as one
//! Lynx paragraph (`text_block`); a block container — the internal
//! `(block, flow)` display — lays its text runs out as a CSS inline
//! formatting context (`inline_flow`). Both keep one retained parley paragraph
//! in layout state, collect their runs with the same per-run reads, go through
//! hughie's leaf box wrapper with the same measure closure, and are painted
//! from the same record. This module holds those shared parts:
//!
//! - [`TextStore`], the one retained paragraph a node's layout state holds, of either kind, and the
//!   probe/restore calls the pass routes through it;
//! - [`PaintedParagraph`], a committed paragraph as the painter and the text clip read it;
//! - the run collection pieces: [`Collected`], the generated-content replacement, and the
//!   `white-space-collapse` read;
//! - [`measure`], the closure both box wrappers run, and the `text-indent` resolution in it.

use std::hash::{Hash, Hasher};

use hughie::compute::{LeafMeasureInput, LeafMetrics};
use hughie::geometry::{Point, Size};
use hughie::text::TextContext;
use hughie::text::block::{BlockConstraint, BlockMetrics, PlacedBox, SourceItem, TextBlock};
use hughie::text::inline::InlineFlow;
use hughie::text::paragraph::{LineGeometry, RunStyle, WhiteSpaceCollapse};
use hughie::tree::AvailableSpace;
use stylo::properties::ComputedValues;
use stylo::values::computed::{Content, ContentItem};

use crate::layout::TextBrush;
use crate::layout::inline_flow::InlineFlowStore;
use crate::layout::style::{
    DisplayMode, StyleView, TextRunView, box_parent, display_mode, inline_style_of,
};
use crate::layout::text_block::TextBlockStore;
use crate::tree::document::{DocumentLayoutState, NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;

/// The retained paragraph one element's layout state holds.
#[expect(
    clippy::large_enum_variant,
    reason = "held boxed, once per paragraph-establishing element; the variants' size \
              difference costs no other node anything"
)]
pub(crate) enum TextStore {
    /// A `display: -lynx-text` element's flattened Lynx paragraph.
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
                    kind: PaintedKind::Flow(flow),
                })
            }
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
    Flow(&'a InlineFlow),
}

impl PaintedParagraph<'_> {
    /// How many parley style indices the layout has.
    pub(crate) fn style_count(&self) -> usize {
        match self.kind {
            PaintedKind::Block(block) => block.style_count(),
            PaintedKind::Flow(flow) => flow.style_count(),
        }
    }

    /// The paint identity of one parley style index. Every run of an inline
    /// formatting context is content; style 0 of either kind is the strut,
    /// which no text is shaped in.
    pub(crate) fn source_of(&self, style_index: u16) -> SourceItem {
        match self.kind {
            PaintedKind::Block(block) => block.source_of(style_index),
            PaintedKind::Flow(flow) => flow
                .source_of(style_index)
                .map_or(SourceItem::Strut, SourceItem::Content),
        }
    }

    /// Every atomic box's outcome; an inline formatting context of text runs
    /// has none.
    pub(crate) fn boxes(&self) -> &[PlacedBox] {
        match self.kind {
            PaintedKind::Block(block) => block.boxes(),
            PaintedKind::Flow(_) => &[],
        }
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
