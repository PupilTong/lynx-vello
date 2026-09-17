//! `display: -lynx-text`: one element, one flattened paragraph.
//!
//! A text block swallows its subtree. The element that establishes it owns the
//! shaped paragraph; every text node and nested text scope inside it is
//! *content* rather than a box, and generates none. That is the whole
//! difference from the path this replaces, where each text node was its own
//! leaf and sibling runs could not share a line.
//!
//! # Classifying a child
//!
//! By computed display alone — never by tag. `crates/dom` must not contain
//! Lynx element vocabulary, and it does not need to: the Lynx UA sheet already
//! encodes the structure as display values, so the rules below reproduce it
//! without naming `text`, `wrapper`, `raw-text` or `view`.
//!
//! | computed display | role |
//! |---|---|
//! | (text node) | a run, carrying its innermost element ancestor's style |
//! | `-lynx-text` | a nested scope: recurse, its runs carry *its* style |
//! | `contents` | transparent: recurse, contributing no scope of its own |
//! | `none` | skipped, and hidden |
//! | anything else | exactly one atomic inline box, never recursed into |
//!
//! The last row is what makes an inline `<view>` atomic: its inner text can
//! never join this paragraph, because the walk does not descend through a box.

use hughie::geometry::{Edges, Point, Size};
use hughie::style::{CoreStyle, PositionProperty};
use hughie::text::TextContext;
use hughie::text::block::{
    BlockConstraint, BlockStyle, InlineBoxSpec, InlineItem, RunStyle, TextBlock, TextRunItem,
    VerticalAlign,
};
use stylo::properties::ComputedValues;
use stylo::values::computed::{Content, ContentItem};

use crate::layout::style::{DisplayMode, StyleView, TextRunView, display_mode, inline_style_of};
use crate::tree::document::{DocumentLayoutState, NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;

/// One element's paragraph, plus the table that maps it back to the DOM.
pub(crate) struct TextBlockStore {
    pub(crate) block: TextBlock,
    ///
    /// Item index → the node that contributed it. This *is*
    /// `SourceItem::Content(u32)`'s index space and *is* `InlineBoxSpec::id`,
    /// so one table serves painting, box placement and hiding.
    pub(crate) source_ids: Vec<NodeId>,
    /// The same table for the custom truncation content, in
    /// `SourceItem::Truncation(u32)`'s index space. Empty where the paragraph
    /// has no `inline-truncation` child. A truncation atom's
    /// `InlineBoxSpec::id` is offset past the content items, so the two share
    /// one box-id space and one placement pass.
    pub(crate) truncation_source_ids: Vec<NodeId>,
    /// What the block was built from. A paragraph is rebuilt, never patched —
    /// parley's own mutability contract — so this is the whole invalidation
    /// question for the shaped half.
    ///
    /// Style is compared, not hashed: `BlockStyle` and `RunStyle` both derive
    /// `PartialEq` over exactly the values parley shapes from, so this cannot
    /// drift the way a hand-maintained field list can. Getting it wrong is
    /// invisible — the paragraph would keep painting glyphs shaped at the old
    /// font — which is why it is an equality check rather than a digest.
    fingerprint: u64,
    style: BlockStyle,
    run_styles: Vec<RunStyle>,
    /// How many times this element's paragraph has been shaped from scratch.
    ///
    /// The observable that separates the two evictions: re-breaking is cheap
    /// and leaves this alone, re-shaping is the expensive half and moves it.
    pub(crate) rebuilds: u32,
}

impl TextBlockStore {
    /// The paragraph, if a commit has produced one.
    ///
    /// `None` rather than a panic: every dom reader keeps its existing
    /// `let Some(..) else { return }` shape, so a paragraph no commit produced
    /// is one that does not paint. The alternative is a panic on the paint
    /// thread, where `Painter::paint` fails closed and the symptom is a frozen
    /// frame rather than an error.
    pub(crate) fn committed(&self) -> Option<&TextBlock> {
        self.block.has_committed().then_some(&self.block)
    }
}

/// One flattened item and the node behind it, before ownership is taken.
struct Collected {
    item: OwnedItem,
    source: NodeSlot,
}

enum OwnedItem {
    Run {
        text: String,
        style: RunStyle,
        preserve_newlines: bool,
    },
    Atom {
        vertical_align: VerticalAlign,
    },
}

/// The establishing element's subtree, split into the two flows a paragraph
/// takes: its own content, and the custom truncation content.
struct Flattened {
    content: Vec<Collected>,
    truncation: Vec<Collected>,
    /// The direct flat children the truncation flow consumed — the first
    /// `inline-truncation` child, whose subtree became `truncation`, and any
    /// later one, which contributes nothing at all.
    markers: Vec<NodeSlot>,
}

/// Whether a positioned child leaves the paragraph's flow.
///
/// The paragraph asks this in four places — the flatten walk, the truncation
/// marker test, the out-of-flow pass and the post-placement hide loop — so it
/// is one function, and they cannot drift apart.
///
/// Only `absolute` and `fixed` leave the flow, which is the split every other
/// algorithm here uses. The value is the one `StyleView::position` resolves,
/// so an `absolute` with no containing-block ancestor has already been lowered
/// to `fixed` and is still out of flow. `relative` and `sticky` are ordinary
/// inline content: a relative atom advances the line and sits exactly where a
/// static one would — its insets are ignored, per native Lynx
/// (`docs/tracking/deviations.md`) — and a sticky one is left unpinned exactly
/// as it is everywhere else in this engine.
const fn out_of_flow(position: PositionProperty) -> bool {
    matches!(
        position,
        PositionProperty::Absolute | PositionProperty::Fixed
    )
}

/// Whether this child is the paragraph's custom truncation content.
///
/// A computed-style fact, never a tag: the Lynx UA sheet flags an
/// `inline-truncation` written directly inside a `text` with
/// `--lynx-inline-truncation`, and flags nothing else, so this reproduces
/// web-core's `:scope > inline-truncation` scope without `crates/dom` naming
/// the element.
fn is_truncation_marker<T>(node: &Node<T>) -> bool {
    if !node.is_element() {
        return false;
    }
    let view = StyleView::of(node);
    !out_of_flow(view.position()) && display_mode(view.display()) == DisplayMode::Text && {
        use hughie::style::TextContainerStyle;
        view.is_inline_truncation()
    }
}

/// Splits `element`'s flat subtree into the paragraph's content and the
/// marker's.
///
/// Only the **first** `inline-truncation` child supplies truncation content,
/// as on the web; later ones are excluded from the content walk and
/// contribute nothing. Generated content on the establishing element replaces
/// every child, marker included.
fn collect_block<T>(tree: &TreeArenas<T>, element: NodeSlot) -> Flattened {
    let mut markers = Vec::new();
    if generated_content(tree.at(element)).is_none() {
        markers.extend(
            tree.at(element)
                .flat_children()
                .iter()
                .copied()
                .filter(|&child| is_truncation_marker(tree.at(child))),
        );
    }
    let truncation = markers
        .first()
        .map(|&marker| collect_content(tree, marker, &[]))
        .unwrap_or_default();
    Flattened {
        content: collect_content(tree, element, &markers),
        truncation,
        markers,
    }
}

/// Walks `element`'s flat subtree into runs and atomic boxes, skipping the
/// children `skip` names.
///
/// Iterative rather than recursive: nesting depth is author-controlled, and a
/// Lynx text scope may nest without limit.
fn collect_content<T>(
    tree: &TreeArenas<T>,
    element: NodeSlot,
    skip: &[NodeSlot],
) -> Vec<Collected> {
    let mut collected = Vec::new();
    if collect_generated(tree, element, &mut collected) {
        return collected;
    }
    // (node, whether its own children still need visiting)
    let mut stack: Vec<NodeSlot> = tree
        .at(element)
        .flat_children()
        .iter()
        .rev()
        .copied()
        .collect();

    while let Some(slot) = stack.pop() {
        if skip.contains(&slot) {
            continue;
        }
        let node = tree.at(slot);
        if node.is_text_node() {
            let Some(text) = node.text() else { continue };
            if text.is_empty() {
                continue;
            }
            let run = TextRunView::of(node);
            collected.push(Collected {
                item: OwnedItem::Run {
                    text: text.to_owned(),
                    style: RunStyle::from_run_style(&run),
                    preserve_newlines: preserves_newlines(node),
                },
                // The *element* the run's style came from, not the text node:
                // a text node carries no computed style, so it could never
                // answer the painter, and this is the same node
                // `TextRunView` read the run's font and colour from.
                source: node
                    .flat_parent()
                    .map_or(slot, |parent| tree.slot(parent.id()).unwrap_or(slot)),
            });
            continue;
        }
        if !node.is_element() {
            continue;
        }
        let view = StyleView::of(node);
        // Out-of-flow children are not inline content: an absolutely
        // positioned or fixed box is placed by the absolute pass against its
        // containing block, and swallowing it as an atom would both grow the
        // paragraph and lay it out twice. A `relative` or `sticky` child is
        // still in flow and is collected below like any other.
        if out_of_flow(view.position()) {
            continue;
        }
        match display_mode(view.display()) {
            // A nested scope and a transparent box are both walked through;
            // the difference is only which style its runs carry, and that is
            // read per run from the innermost element ancestor anyway.
            //
            // Neither generates a box, so `position: relative` on one is inert
            // twice over: its insets are ignored like any in-flow child's, and
            // there would be nothing for them to move anyway.
            DisplayMode::Text | DisplayMode::Contents => {
                if !collect_generated(tree, slot, &mut collected) {
                    stack.extend(node.flat_children().iter().rev().copied());
                }
            }
            DisplayMode::None => {}
            _ => collected.push(Collected {
                item: OwnedItem::Atom {
                    vertical_align: VerticalAlign::Baseline,
                },
                source: slot,
            }),
        }
    }
    collected
}

/// The supported content list replaces rendered children, including when its
/// strings resolve to empty. Unsupported lists fall back to ordinary children
/// as a whole; never render only a supported prefix of a list.
fn generated_content<T>(node: &Node<T>) -> Option<(&ComputedValues, &[ContentItem])> {
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
        .all(|item| match item {
            ContentItem::String(_) => true,
            ContentItem::Attr(attr) => attr.namespace_url.is_empty(),
            _ => false,
        })
        .then_some((style, items))
}

/// Whether this scope suppresses descendants during the later positioning walk.
/// `contents` only consumes text content when it belongs to a text paragraph.
pub(super) fn replaces_children<T>(node: &Node<T>) -> bool {
    let Some(style) = node.layout_computed_style() else {
        return false;
    };
    match display_mode(style.clone_display()) {
        DisplayMode::Text => generated_content(node).is_some(),
        DisplayMode::Contents => {
            generated_content(node).is_some()
                && crate::layout::style::box_parent(node).is_some_and(|parent| {
                    parent.layout_computed_style().is_some_and(|style| {
                        display_mode(style.clone_display()) == DisplayMode::Text
                    })
                })
        }
        _ => false,
    }
}

/// Returns whether generated text replaced this scope's rendered children.
fn collect_generated<T>(tree: &TreeArenas<T>, slot: NodeSlot, out: &mut Vec<Collected>) -> bool {
    let node = tree.at(slot);
    let Some((style, items)) = generated_content(node) else {
        return false;
    };
    let mut text = String::new();
    for item in items {
        match item {
            ContentItem::String(value) => text.push_str(value),
            ContentItem::Attr(attr) => {
                text.push_str(node.attribute(&attr.attribute).unwrap_or(&attr.fallback));
            }
            _ => unreachable!("generated_content validated the list"),
        }
    }
    if !text.is_empty() {
        out.push(Collected {
            item: OwnedItem::Run {
                text,
                style: RunStyle::from_run_style(&TextRunView::from_values(style)),
                preserve_newlines: style_preserves_newlines(style),
            },
            source: slot,
        });
    }
    true
}

/// Whether a run keeps the literal newlines in its source.
///
/// The one place Lynx preserves one: a carrier's UA rule sets
/// `white-space-collapse: preserve-breaks`, which inherits into text runs.
/// Read from computed style, so no tag is named.
fn preserves_newlines<T>(node: &Node<T>) -> bool {
    style_preserves_newlines(inline_style_of(node))
}

fn style_preserves_newlines(style: &ComputedValues) -> bool {
    use stylo::computed_values::white_space_collapse::T as Collapse;
    style.get_inherited_text().clone_white_space_collapse() == Collapse::PreserveBreaks
}

/// A cheap structural identity of the flattened content.
///
/// Not a correctness mechanism — the eviction paths are — but a backstop that
/// turns a missed invalidation into a rebuild rather than into stale glyphs.
fn fingerprint(flat: &Flattened) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    // Both flows, and the marker list: gaining or losing an `inline-truncation`
    // child rebuilds the block even where neither flow's items moved, because
    // the marker's mere presence suppresses the dots.
    flat.markers.hash(&mut hasher);
    for collected in [&flat.content, &flat.truncation] {
        collected.len().hash(&mut hasher);
        for entry in collected {
            entry.source.hash(&mut hasher);
            match &entry.item {
                OwnedItem::Run {
                    text,
                    preserve_newlines,
                    ..
                } => {
                    0_u8.hash(&mut hasher);
                    text.hash(&mut hasher);
                    preserve_newlines.hash(&mut hasher);
                }
                OwnedItem::Atom { .. } => 1_u8.hash(&mut hasher),
            }
        }
    }
    hasher.finish()
}

/// Borrows the collected items as the block's input vocabulary.
///
/// `id_offset` puts the truncation flow's box ids past the content flow's, so
/// one id space and one placement pass serve both.
fn as_items(collected: &[Collected], id_offset: usize) -> Vec<InlineItem<'_>> {
    collected
        .iter()
        .enumerate()
        .map(|(index, entry)| match &entry.item {
            OwnedItem::Run {
                text,
                style,
                preserve_newlines,
            } => InlineItem::Run(TextRunItem {
                text,
                style,
                preserve_newlines: *preserve_newlines,
            }),
            OwnedItem::Atom { vertical_align } => InlineItem::Box(InlineBoxSpec {
                // The item index is the box id, so one table answers for
                // painting, placement and hiding alike.
                id: (id_offset + index) as u64,
                size: Size::ZERO,
                baseline: None,
                vertical_align: *vertical_align,
            }),
        })
        .collect()
}

/// The paragraph style, from the establishing element's CSS and attributes.
fn block_style<T>(tree: &TreeArenas<T>, element: NodeSlot) -> BlockStyle {
    BlockStyle::from_container_style(&StyleView::of(tree.at(element)))
}

/// Rebuilds `element`'s paragraph when its content or style moved, and reports
/// which of its children are atomic boxes.
pub(crate) fn refresh<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
) -> Vec<(NodeSlot, u64)> {
    let flat = collect_block(tree, element);
    let stamp = fingerprint(&flat);
    let style = block_style(tree, element);
    let offset = flat.content.len();

    let mut atoms = atoms_of(&flat.content, 0);
    atoms.extend(atoms_of(&flat.truncation, offset));

    let run_styles: Vec<RunStyle> = flat
        .content
        .iter()
        .chain(flat.truncation.iter())
        .filter_map(|entry| match &entry.item {
            OwnedItem::Run { style, .. } => Some(style.clone()),
            OwnedItem::Atom { .. } => None,
        })
        .collect();

    // Only the values parley *shapes* from force a new paragraph. Alignment
    // and truncation are applied to a finished layout, so a change there is a
    // re-break at most — which is the whole point of keeping the two evictions
    // apart.
    let needs_build = state.text_block(element).is_none_or(|store| {
        store.fingerprint != stamp
            || store.style.word_break != style.word_break
            || store.style.text_wrap != style.text_wrap
            || store.run_styles != run_styles
    });

    if needs_build {
        let items = as_items(&flat.content, 0);
        let truncation_items = as_items(&flat.truncation, offset);
        let source_ids = flat
            .content
            .iter()
            .map(|entry| tree.at(entry.source).id())
            .collect();
        let truncation_source_ids = flat
            .truncation
            .iter()
            .map(|entry| tree.at(entry.source).id())
            .collect();
        let (context, slot) = state.text_block_parts(element);
        let rebuilds = slot.as_deref().map_or(0, |store| store.rebuilds + 1);
        // The marker's *presence* is the input, not its content: an empty
        // `inline-truncation` still suppresses the dots, which is what the web
        // reference does with the same subtree.
        let truncation = (!flat.markers.is_empty()).then_some(&truncation_items[..]);
        let block = TextBlock::new(context, style.clone(), &items, truncation);
        *slot = Some(Box::new(TextBlockStore {
            block,
            source_ids,
            truncation_source_ids,
            fingerprint: stamp,
            style,
            run_styles,
            rebuilds,
        }));
    } else if let Some((_, store)) = context_and_block(state, element) {
        // Same paragraph, possibly different alignment or truncation.
        store.block.set_style(style.clone());
        store.style = style;
    }
    atoms
}

/// The `(node, box id)` pairs of one flow's atomic boxes.
fn atoms_of(collected: &[Collected], id_offset: usize) -> Vec<(NodeSlot, u64)> {
    collected
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry.item, OwnedItem::Atom { .. }))
        .map(|(index, entry)| (entry.source, (id_offset + index) as u64))
        .collect()
}

/// Resolves `text-indent` against the definite inline size, which is the
/// caller's to know — the block only ever sees a break width.
pub(crate) fn constraint_for<T>(
    tree: &TreeArenas<T>,
    element: NodeSlot,
    max_advance: Option<f32>,
    basis: Option<f32>,
) -> BlockConstraint {
    use hughie::style::TextContainerStyle;
    use stylo::values::generics::text::GenericTextIndent;

    let view = StyleView::of(tree.at(element));
    let indent: GenericTextIndent<_> = view.text_indent();
    let resolved = indent
        .length
        .to_used_value(app_units::Au::from_f32_px(basis.unwrap_or(0.0)));
    BlockConstraint::new(max_advance, resolved.to_f32_px())
}

/// A text block never asks its atoms to reflow: their measured margin box is a
/// constraint-independent fact, taken once per pass at max-content, exactly as
/// Lynx measures an inline view as an independent subtree.
pub(crate) const ATOM_SPACE: hughie::tree::AvailableSpace =
    hughie::tree::AvailableSpace::MaxContent;

/// One atomic inline box: the node behind it, the box id it holds in the
/// paragraph, and the margins the line advanced by.
///
/// The margins are carried rather than re-read at placement because the same
/// numbers have to answer twice, and the two answers must agree: once as the
/// margin box the paragraph breaks against, once as the step from that margin
/// box's origin down to the border box's.
#[derive(Clone, Copy)]
struct Atom {
    slot: NodeSlot,
    id: u64,
    margin: Edges<f32>,
}

/// The establishing element's own box model for this pass.
///
/// Resolved from style and from the size this pass just produced, never from
/// the element's layout slot: a parent writes a child's layout only once that
/// child's `compute_layout` has returned, so while this one runs its own slot
/// still holds the previous pass's box — or, on the first pass, none at all.
/// The inline basis is the one the box wrapper resolved its own padding
/// against, so the two cannot disagree.
struct BlockBox {
    size: Size<f32>,
    border: Edges<f32>,
    padding: Edges<f32>,
}

impl BlockBox {
    fn of<T>(view: &StyleView<'_, T>, size: Size<f32>, inline_basis: Option<f32>) -> Self {
        Self {
            size,
            border: hughie::compute::used_border(view),
            padding: hughie::compute::used_padding(view, inline_basis),
        }
    }

    /// The content-box origin, in the border-box space `Layout::location` means.
    fn content_origin(&self) -> Point<f32> {
        Point::new(
            self.border.left + self.padding.left,
            self.border.top + self.padding.top,
        )
    }

    /// The padding box: the containing block of an out-of-flow child.
    fn padding_box(&self) -> Size<f32> {
        Size::new(
            (self.size.width - self.border.horizontal_sum()).max(0.0),
            (self.size.height - self.border.vertical_sum()).max(0.0),
        )
    }
}

/// The margin box the line advances by.
///
/// Floored at zero on each axis: negative margins legitimately shrink the
/// advance, but a box dimension the breaker sees must never go below nothing.
fn margin_box(border_box: Size<f32>, margin: Edges<f32>) -> Size<f32> {
    Size::new(
        (border_box.width + margin.horizontal_sum()).max(0.0),
        (border_box.height + margin.vertical_sum()).max(0.0),
    )
}

pub(crate) fn context_and_block(
    state: &mut DocumentLayoutState,
    element: NodeSlot,
) -> Option<(&mut TextContext, &mut TextBlockStore)> {
    let (context, slot) = state.text_block_parts(element);
    slot.as_deref_mut().map(|store| (context, store))
}

/// Lays `element` out as one flattened paragraph.
///
/// Three phases, borrow-disjoint by construction: child layout needs the whole
/// layout state, and the block comes out of that same state, so the two can
/// never be live at once.
///
/// 1. **Refresh** — walk the subtree, rebuild the paragraph if its content moved.
/// 2. **Atoms** — measure each inline box once, at max-content, and write its *margin* box in.
///    Constraint-independent by construction: Lynx measures an inline view as an independent
///    subtree, and a size that moved between a probe and its commit would poison both the width
///    memo and the committed break. That is why the margins come from style on both paths: a
///    measurement writes no layout, so a margin read back from a slot would be the last pass's.
/// 3. **Paragraph** — probe or commit, then place the atoms and hide the nodes the paragraph
///    consumed.
pub(crate) fn compute_text_block_layout<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    input: hughie::tree::LayoutInput,
) -> hughie::tree::LayoutOutput {
    use hughie::compute::{LeafMetrics, compute_text_block_layout as compute_block_box};
    use hughie::tree::LayoutTree;

    let atoms = refresh(tree, state, element);

    // Phase 2. Each atom is laid out as its own subtree; the paragraph only
    // ever sees the margin-box result.
    let mut placed_atoms: Vec<Atom> = Vec::with_capacity(atoms.len());
    for &(atom, id) in &atoms {
        let output = if input.goal.commits() {
            hughie::compute::compute_inline_box_layout(
                tree,
                state,
                atom,
                input.parent_size,
                Size::new(ATOM_SPACE, ATOM_SPACE),
            )
        } else {
            tree.compute_layout(
                state,
                atom,
                hughie::tree::LayoutInput::measure(
                    Size::new(None, None),
                    input.parent_size,
                    Size::new(ATOM_SPACE, ATOM_SPACE),
                    hughie::tree::RequestedAxis::Both,
                ),
            )
        };
        let margin = hughie::compute::used_margins(&tree.style(atom), input.parent_size.width);
        if let Some((_, store)) = context_and_block(state, element) {
            // A baseline is measured from the border box's top edge, so it
            // moves down with the margin box's. Without one the block's own
            // fallback puts the box's bottom edge on the baseline — which, the
            // box being the margin box, is the bottom *margin* edge CSS asks
            // for an inline-block that has no baseline of its own.
            let baseline = output
                .first_baselines
                .y
                .map(|baseline| (baseline + margin.top).max(0.0));
            store
                .block
                .set_box_size(id, margin_box(output.size, margin), baseline);
        }
        placed_atoms.push(Atom {
            slot: atom,
            id,
            margin,
        });
    }

    // Phase 3.
    let basis = input.known_dimensions.width.or(input.parent_size.width);
    let view = crate::layout::style::StyleView::of(tree.at(element));
    let output = compute_block_box(input, &view, |measure_input| {
        let width = match measure_input.available_space.width {
            hughie::tree::AvailableSpace::Definite(value) => Some(value),
            hughie::tree::AvailableSpace::MaxContent => None,
            hughie::tree::AvailableSpace::MinContent => {
                // Min-content needs a width before a constraint exists, so
                // it is the one answer the memo cannot key on.
                let Some((context, store)) = context_and_block(state, element) else {
                    return LeafMetrics::new(Size::ZERO);
                };
                Some(store.block.min_content_width(context))
            }
        };
        let width = measure_input.known_dimensions.width.or(width);
        let constraint = constraint_for(tree, element, width, basis);
        let Some((context, store)) = context_and_block(state, element) else {
            return LeafMetrics::new(Size::ZERO);
        };
        let metrics = if measure_input.goal.commits() {
            store.block.commit(context, constraint)
        } else {
            store.block.probe(context, constraint)
        };
        LeafMetrics::new(metrics.size)
            .with_first_baselines(hughie::geometry::Point::new(None, metrics.first_baseline))
    });

    if input.goal.commits() {
        let block = BlockBox::of(&view, output.size, input.parent_size.width);
        place_and_hide(tree, state, element, &placed_atoms, &block);
    } else {
        state.note_probed_text(element);
    }
    output
}

/// Positions the atoms the paragraph placed, and hides everything it consumed.
fn place_and_hide<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    atoms: &[Atom],
    block: &BlockBox,
) {
    use hughie::text::block::PlacedBox;
    use hughie::tree::LayoutTree;

    if generated_content(tree.at(element)).is_some() {
        // Replacement suppresses every descendant, including out-of-flow boxes.
        for child in tree.children(element) {
            hughie::compute::hide_subtree(tree, state, child);
        }
        return;
    }

    let placements: Vec<PlacedBox> = state
        .text_block(element)
        .and_then(TextBlockStore::committed)
        .map(|block| block.boxes().to_vec())
        .unwrap_or_default();

    // Every slot the paragraph gave a real box, and — separately — the
    // consumed elements on the path down to one. A consumed element generates
    // no box of its own, but hiding its subtree would zero the atom under it,
    // so it keeps an empty layout instead.
    let mut placed_slots: Vec<NodeSlot> = Vec::new();
    for placed in &placements {
        let PlacedBox::Visible { id, .. } = *placed else {
            continue;
        };
        if let Some(atom) = atoms.iter().find(|atom| atom.id == id) {
            placed_slots.push(atom.slot);
        }
    }
    let carriers = atom_carriers(tree, element, &placed_slots);

    let content_origin = block.content_origin();
    for placed in placements {
        let (id, origin) = match placed {
            PlacedBox::Visible { id, origin, .. } => (id, Some(origin)),
            // The Lynx `HideView` surface: a truncated-away atom has no
            // position, so it generates no box this frame.
            PlacedBox::Hidden { id } => (id, None),
        };
        let Some(&atom) = atoms.iter().find(|atom| atom.id == id) else {
            continue;
        };
        match origin {
            Some(origin) => {
                // `left`/`top`/`right`/`bottom` on an in-flow atom are ignored,
                // following native Lynx: `CalcRelativePosition` runs only over
                // a `LayoutAlgorithm`'s in-flow items
                // (`starlight/layout/layout_algorithm.cc:215-228`), and a
                // `<text>` has a `measure_func_`, so it never builds one
                // (`layout_object.cc:684-696`). This deviates from web-core,
                // where `x-view` is a `position: relative` `inline-flex` box
                // the browser shifts; user ruling 2026-09-17, recorded in
                // `docs/tracking/deviations.md`. A relative atom therefore
                // sits exactly where a static one does.
                //
                // The atom keeps the box its own layout produced; the
                // paragraph decides only where it sits.
                let slot_layout = tree.layout_mut(state, atom.slot);
                let mut placed_layout =
                    hughie::tree::Layout::with_order(slot_layout.unrounded.order);
                placed_layout.size = slot_layout.unrounded.size;
                placed_layout.content_size = slot_layout.unrounded.content_size;
                placed_layout.border = slot_layout.unrounded.border;
                placed_layout.padding = slot_layout.unrounded.padding;
                placed_layout.margin = slot_layout.unrounded.margin;
                // Two changes of space, in order. The paragraph's origin is
                // the element's *content* box, while `location` is read
                // against its border box; and the paragraph placed the atom's
                // *margin* box, while `location` names its border box.
                placed_layout.location = Point::new(
                    content_origin.x + origin.x + atom.margin.left,
                    content_origin.y + origin.y + atom.margin.top,
                );
                tree.set_unrounded_layout(state, atom.slot, placed_layout);
            }
            None => hughie::compute::hide_subtree(tree, state, atom.slot),
        }
    }

    // Out-of-flow children never entered the paragraph, so the block lays
    // them out itself against its own padding box — the same thing every
    // other container algorithm does for the children it does not flow.
    let padding_box = block.padding_box();
    for child in tree.children(element) {
        let node = tree.at(child);
        if !node.is_element() || !out_of_flow(StyleView::of(node).position()) {
            continue;
        }
        let mut layout = hughie::compute::compute_absolute_layout(
            tree,
            state,
            child,
            padding_box,
            // The static position, in the padding-box space the containing
            // block is: where an in-flow box would have started, which is the
            // content-box origin.
            Point::new(block.padding.left, block.padding.top),
        );
        // And back out to the border-box space `location` is read in.
        layout.location.x += block.border.left;
        layout.location.y += block.border.top;
        tree.set_unrounded_layout(state, child, layout);
    }

    // Everything the paragraph swallowed generates no box. Hiding the whole
    // child rather than each consumed node keeps this O(children) wherever the
    // child holds no atom: a nested scope's own subtree is consumed with it.
    //
    // A consumed element that *does* hold a placed atom is written with an
    // empty layout instead of being marked hidden, and its children are
    // visited under the same rule. Two things need that: the marker subtree,
    // whose atoms are placed at the clamp, and a `display: contents` wrapper
    // over an atom. An empty layout is not a hidden one — it keeps the slot's
    // paint order and, because it is a write, keeps the rounding walk
    // descending to the atom underneath.
    let mut stack: Vec<NodeSlot> = tree.children(element).collect();
    while let Some(child) = stack.pop() {
        // Atoms are exempt — they were just placed — and hiding one would zero
        // the geometry this pass gave it.
        if atoms.iter().any(|atom| atom.slot == child) {
            continue;
        }
        // An out-of-flow child is not the paragraph's to hide: it never
        // entered the flatten walk, and the absolute pass places it against
        // its containing block.
        let node = tree.at(child);
        if node.is_element() && out_of_flow(StyleView::of(node).position()) {
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
}

/// The consumed elements between `element` and each of `placed`, exclusive of
/// both ends.
///
/// Walks up from the atoms rather than down from the element: a paragraph has
/// far fewer placed atoms than nodes, and a shared prefix stops the walk.
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

#[cfg(test)]
mod generated_content_tests {
    use super::*;
    use crate::{Document, FontBlob, StylesheetOrigin};

    fn document(css: &str) -> (Document<()>, NodeId) {
        let mut doc = Document::new(crate::tree::document::tests::device(), "page", ());
        doc.register_fonts(FontBlob::from_static(include_bytes!(
            "../../../hughie/tests/fixtures/Ahem.ttf"
        )));
        doc.add_stylesheet("page { display: flex; } text { display: -lynx-text; font-family: Ahem; font-size: 20px; } scope { display: contents; }", StylesheetOrigin::Author);
        doc.add_stylesheet(css, StylesheetOrigin::Author);
        let text = doc.create_element("text", ());
        doc.append_child(doc.document_element().id(), text);
        (doc, text)
    }

    fn contents(doc: &Document<()>, text: NodeId) -> String {
        collect_content(doc.arenas(), doc.slot(text).unwrap(), &[])
            .into_iter()
            .filter_map(|entry| match entry.item {
                OwnedItem::Run { text, .. } => Some(text),
                OwnedItem::Atom { .. } => None,
            })
            .collect()
    }

    fn width(doc: &Document<()>, text: NodeId, expected: f32) {
        let measured = doc.text_block_size(text).expect("laid out paragraph");
        assert!(
            (measured.width - expected).abs() < 0.001,
            "{measured:?} != {expected}"
        );
    }

    #[test]
    fn generated_content_updates_attributes_without_selector_dependencies() {
        let (mut doc, text) = document("text { content: attr(text); }");
        for (value, expected) in [
            (Some("hello"), 100.0),
            (Some("hi"), 40.0),
            (Some(""), 0.0),
            (Some("again"), 100.0),
            (None, 0.0),
        ] {
            match value {
                Some(value) => doc.set_attribute(text, "text", value),
                None => doc.remove_attribute(text, "text"),
            }
            doc.layout();
            assert_eq!(contents(&doc, text), value.unwrap_or_default());
            width(&doc, text, expected);
            assert!(doc.get(text).unwrap().child_ids().is_empty());
            assert_eq!(
                doc.query_selector(doc.document_element().id(), "text:empty")
                    .unwrap(),
                Some(text)
            );
        }
    }

    #[test]
    fn generated_content_replaces_children_without_changing_dom_selectors() {
        let (mut doc, text) =
            document("text[text] { content: '[' attr(text) ']'; } scope { content: attr(label); }");
        let scope = doc.create_element("scope", ());
        doc.set_attribute(scope, "label", "B");
        doc.append_child(text, scope);
        let literal = doc.create_text_node("C", ());
        doc.append_child(scope, literal);
        doc.layout();
        assert_eq!(contents(&doc, text), "B");
        width(&doc, text, 20.0);
        doc.set_attribute(text, "text", "A");
        doc.layout();
        assert_eq!(contents(&doc, text), "[A]");
        width(&doc, text, 60.0);
        assert_eq!(doc.get(text).unwrap().child_ids(), [scope]);
        assert_eq!(doc.get(scope).unwrap().child_ids(), [literal]);
        assert_eq!(
            doc.query_selector(doc.document_element().id(), "scope:first-child")
                .unwrap(),
            Some(scope)
        );
        doc.set_attribute(scope, "label", "DEF");
        doc.remove_attribute(text, "text");
        doc.layout();
        assert_eq!(contents(&doc, text), "DEF");
        width(&doc, text, 60.0);
        doc.set_attribute(scope, "label", "");
        doc.layout();
        assert_eq!(contents(&doc, text), "");
        width(&doc, text, 0.0);
        doc.add_stylesheet("scope { content: normal; }", StylesheetOrigin::Author);
        doc.layout();
        assert_eq!(contents(&doc, text), "C");
        width(&doc, text, 20.0);
    }

    #[test]
    fn generated_content_replaces_atomic_and_out_of_flow_children() {
        let (mut doc, text) = document("text[text] { content: attr(text); }");
        let atom = doc.create_element("view", ());
        doc.set_inline_style(atom, "display: flex; width: 30px; height: 20px;");
        doc.append_child(text, atom);
        let absolute = doc.create_element("view", ());
        doc.set_inline_style(absolute, "position: absolute; width: 50px; height: 20px;");
        doc.append_child(text, absolute);
        doc.layout();
        width(&doc, text, 30.0);
        assert!((doc.rounded_layout(absolute).unwrap().size.width - 50.0).abs() < f32::EPSILON);
        for (value, expected) in [("AB", 40.0), ("", 0.0)] {
            doc.set_attribute(text, "text", value);
            doc.layout();
            width(&doc, text, expected);
            for child in [atom, absolute] {
                assert_eq!(doc.rounded_layout(child).unwrap().size, Size::ZERO);
            }
        }
        // Mutations under a replacement must stay hidden and take effect when
        // ordinary children become visible again.
        doc.set_inline_style_property(atom, "width", "60px");
        doc.set_inline_style_property(absolute, "width", "70px");
        doc.layout();
        for child in [atom, absolute] {
            assert_eq!(doc.rounded_layout(child).unwrap().size, Size::ZERO);
        }
        doc.remove_attribute(text, "text");
        doc.layout();
        width(&doc, text, 60.0);
        assert!((doc.rounded_layout(atom).unwrap().size.width - 60.0).abs() < f32::EPSILON);
        assert!((doc.rounded_layout(absolute).unwrap().size.width - 70.0).abs() < f32::EPSILON);
    }

    #[test]
    fn unsupported_generated_lists_leave_children_and_pseudos_do_not_generate_text() {
        let (mut doc, text) =
            document("text { content: 'prefix' open-quote; } text::before { content: 'before'; }");
        let literal = doc.create_text_node("AB", ());
        doc.append_child(text, literal);
        doc.layout();
        assert_eq!(contents(&doc, text), "AB");
        width(&doc, text, 40.0);
        doc.add_stylesheet("text { content: none; }", StylesheetOrigin::Author);
        doc.layout();
        assert_eq!(contents(&doc, text), "AB");
        width(&doc, text, 40.0);
    }

    #[test]
    fn generated_content_cascade_changes_invalidate_the_paragraph() {
        let (mut doc, text) = document(
            "text { content: attr(text); } text.off { content: none; } text.large { font-size: 40px; }",
        );
        doc.set_attribute(text, "text", "AB");
        for (class, expected) in [("", 40.0), ("off", 0.0), ("", 40.0), ("large", 80.0)] {
            doc.set_classes(text, class);
            doc.layout();
            width(&doc, text, expected);
        }
        doc.add_stylesheet("text { content: 'X'; }", StylesheetOrigin::Author);
        doc.layout();
        assert_eq!(contents(&doc, text), "X");
        width(&doc, text, 40.0);
    }

    #[test]
    fn generated_content_special_attribute_paths_and_fallback() {
        let (mut doc, text) =
            document("text { content: attr(class) attr(id) attr(missing, 'Z'); }");
        doc.layout();
        width(&doc, text, 20.0);
        doc.add_class(text, "AB");
        doc.set_id_attribute(text, Some("C"));
        doc.layout();
        assert_eq!(contents(&doc, text), "ABCZ");
        width(&doc, text, 80.0);
        doc.remove_class(text, "AB");
        doc.remove_attribute(text, "id");
        doc.layout();
        width(&doc, text, 20.0);
    }

    #[test]
    fn generated_content_survives_hiding_and_reattaching_its_scope() {
        let (mut doc, text) =
            document("scope[label] { content: attr(label); } scope.hidden { display: none; }");
        let wrapper = doc.create_element("scope", ());
        let scope = doc.create_element("scope", ());
        doc.append_child(text, wrapper);
        doc.append_child(wrapper, scope);
        doc.set_attribute(scope, "label", "AB");
        doc.layout();
        width(&doc, text, 40.0);
        doc.add_class(scope, "hidden");
        doc.layout();
        width(&doc, text, 0.0);
        doc.set_attribute(scope, "label", "C");
        doc.remove_class(scope, "hidden");
        doc.layout();
        width(&doc, text, 20.0);
        doc.remove_element(scope);
        doc.layout();
        width(&doc, text, 0.0);
        doc.append_child(wrapper, scope);
        doc.layout();
        width(&doc, text, 20.0);
    }

    #[test]
    fn generated_content_shares_a_paragraph_with_resized_atomic_boxes() {
        let (mut doc, text) = document("scope { content: 'A'; }");
        let scope = doc.create_element("scope", ());
        doc.append_child(text, scope);
        let atom = doc.create_element("view", ());
        doc.set_inline_style(atom, "display: flex; width: 30px; height: 20px;");
        doc.append_child(text, atom);
        doc.layout();
        width(&doc, text, 50.0);
        doc.set_inline_style_property(atom, "width", "50px");
        doc.layout();
        width(&doc, text, 70.0);
    }

    #[test]
    fn generated_content_reads_attributes_written_before_its_rule_arrives() {
        let (mut doc, text) = document("");
        doc.set_attribute(text, "text", "AB");
        doc.layout();
        width(&doc, text, 0.0);
        doc.add_stylesheet("text { content: attr(text); }", StylesheetOrigin::Author);
        doc.layout();
        width(&doc, text, 40.0);
        doc.add_stylesheet("text { content: attr(style); }", StylesheetOrigin::Author);
        doc.set_inline_style(text, "color: red;");
        doc.layout();
        assert_eq!(
            contents(&doc, text),
            doc.get(text).unwrap().attribute("style").unwrap()
        );
        doc.set_inline_style_property(text, "color", "blue");
        doc.layout();
        assert_eq!(
            contents(&doc, text),
            doc.get(text).unwrap().attribute("style").unwrap()
        );
        width(&doc, text, 240.0);
        doc.remove_attribute(text, "style");
        doc.layout();
        width(&doc, text, 0.0);
    }

    #[test]
    fn generated_content_wraps_and_preserves_requested_newlines() {
        let (mut doc, text) = document(
            "text { width: 40px; } text { content: attr(text); white-space-collapse: preserve-breaks; }",
        );
        doc.set_attribute(text, "text", "AB\nCD");
        doc.layout();
        width(&doc, text, 40.0);
        assert!((doc.text_block_size(text).unwrap().height - 40.0).abs() < 0.001);
        doc.set_attribute(text, "text", "A B C");
        doc.layout();
        assert!(doc.text_block_size(text).unwrap().height >= 60.0);
        doc.set_attribute(text, "text", "AB\nCD");
        doc.add_stylesheet(
            "text { width: 200px; } text { white-space-collapse: collapse; }",
            StylesheetOrigin::Author,
        );
        doc.layout();
        width(&doc, text, 100.0);
        assert!((doc.text_block_size(text).unwrap().height - 20.0).abs() < 0.001);
    }

    #[test]
    fn generated_content_repeated_values_reuse_shaping_and_nested_style_changes_rebuild() {
        let (mut doc, text) =
            document("scope { content: attr(label); } scope.large { font-size: 40px; }");
        let scope = doc.create_element("scope", ());
        doc.append_child(text, scope);
        doc.set_attribute(scope, "label", "AB");
        doc.layout();
        let builds = doc.text_block_rebuilds(text);
        doc.set_attribute(scope, "label", "AB");
        doc.layout();
        assert_eq!(doc.text_block_rebuilds(text), builds);
        doc.add_class(scope, "large");
        doc.layout();
        width(&doc, text, 80.0);
        assert_ne!(doc.text_block_rebuilds(text), builds);
    }
}
