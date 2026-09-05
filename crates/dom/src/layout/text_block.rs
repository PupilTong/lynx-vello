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

use hughie::geometry::Size;
use hughie::style::{CoreStyle, PositionProperty};
use hughie::text::TextContext;
use hughie::text::block::{
    BlockConstraint, BlockStyle, InlineBoxSpec, InlineItem, RunStyle, TextBlock, TextRunItem,
    VerticalAlign,
};

use crate::layout::style::{
    DisplayMode, StyleView, TextContainerView, TextRunView, display_mode, inline_style_of,
};
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

/// Walks `element`'s flat subtree into runs and atomic boxes.
///
/// Iterative rather than recursive: nesting depth is author-controlled, and a
/// Lynx text scope may nest without limit.
fn collect<T>(tree: &TreeArenas<T>, element: NodeSlot) -> Vec<Collected> {
    let mut collected = Vec::new();
    // (node, whether its own children still need visiting)
    let mut stack: Vec<NodeSlot> = tree
        .at(element)
        .flat_children()
        .iter()
        .rev()
        .copied()
        .collect();

    while let Some(slot) = stack.pop() {
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
        // paragraph and lay it out twice.
        if view.position() != PositionProperty::Static {
            continue;
        }
        match display_mode(view.display()) {
            // A nested scope and a transparent box are both walked through;
            // the difference is only which style its runs carry, and that is
            // read per run from the innermost element ancestor anyway.
            DisplayMode::Text | DisplayMode::Contents => {
                stack.extend(node.flat_children().iter().rev().copied());
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

/// Whether a run keeps the literal newlines in its source.
///
/// The one place Lynx preserves one: a carrier's UA rule sets
/// `white-space-collapse: preserve-breaks`, which inherits into the reflected
/// text node. Read from computed style, so no tag is named.
fn preserves_newlines<T>(node: &Node<T>) -> bool {
    use stylo::computed_values::white_space_collapse::T as Collapse;
    inline_style_of(node)
        .get_inherited_text()
        .clone_white_space_collapse()
        == Collapse::PreserveBreaks
}

/// A cheap structural identity of the flattened content.
///
/// Not a correctness mechanism — the eviction paths are — but a backstop that
/// turns a missed invalidation into a rebuild rather than into stale glyphs.
fn fingerprint(collected: &[Collected]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    collected.len().hash(&mut hasher);
    for entry in collected {
        entry.source.hash(&mut hasher);
        match &entry.item {
            OwnedItem::Run { text, .. } => {
                0_u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
            OwnedItem::Atom { .. } => 1_u8.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// Borrows the collected items as the block's input vocabulary.
fn as_items(collected: &[Collected]) -> Vec<InlineItem<'_>> {
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
                id: index as u64,
                size: Size::ZERO,
                baseline: None,
                vertical_align: *vertical_align,
            }),
        })
        .collect()
}

/// The paragraph style, from the establishing element's inherited values.
fn block_style<T>(tree: &TreeArenas<T>, element: NodeSlot) -> BlockStyle {
    let node = tree.at(element);
    let view = TextContainerView::of_establishing_element(node);
    let limits = node.text_constraints();
    BlockStyle {
        max_lines: limits.max_lines,
        max_chars: limits.max_chars,
        ..BlockStyle::from_container_style(&view)
    }
}

/// Rebuilds `element`'s paragraph when its content or style moved, and reports
/// which of its children are atomic boxes.
pub(crate) fn refresh<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
) -> Vec<(NodeSlot, u64)> {
    let collected = collect(tree, element);
    let stamp = fingerprint(&collected);
    let style = block_style(tree, element);

    let atoms: Vec<(NodeSlot, u64)> = collected
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry.item, OwnedItem::Atom { .. }))
        .map(|(index, entry)| (entry.source, index as u64))
        .collect();

    let run_styles: Vec<RunStyle> = collected
        .iter()
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
        let items = as_items(&collected);
        let source_ids = collected
            .iter()
            .map(|entry| tree.at(entry.source).id())
            .collect();
        let (context, slot) = state.text_block_parts(element);
        let rebuilds = slot.as_deref().map_or(0, |store| store.rebuilds + 1);
        let block = TextBlock::new(context, style.clone(), &items, None);
        *slot = Some(Box::new(TextBlockStore {
            block,
            source_ids,
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

    let view = TextContainerView::of_establishing_element(tree.at(element));
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
/// 2. **Atoms** — measure each inline box once, at max-content, and write the sizes in.
///    Constraint-independent by construction: Lynx measures an inline view as an independent
///    subtree, and a size that moved between a probe and its commit would poison both the width
///    memo and the committed break.
/// 3. **Paragraph** — probe or commit, then place the atoms and hide the nodes the paragraph
///    consumed.
pub(crate) fn compute_text_block_layout<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    input: hughie::tree::LayoutInput,
) -> hughie::tree::LayoutOutput {
    use hughie::compute::{LeafMetrics, compute_text_block_layout as compute_block_box};
    use hughie::tree::{LayoutGoal, LayoutTree};

    let atoms = refresh(tree, state, element);

    // Phase 2. Each atom is laid out as its own subtree; the paragraph only
    // ever sees the margin-box result.
    for &(atom, id) in &atoms {
        let output = tree.compute_layout(
            state,
            atom,
            hughie::tree::LayoutInput::measure(
                Size::new(None, None),
                input.parent_size,
                Size::new(ATOM_SPACE, ATOM_SPACE),
                hughie::tree::RequestedAxis::Both,
            ),
        );
        if let Some((_, store)) = context_and_block(state, element) {
            store
                .block
                .set_box_size(id, output.size, output.first_baselines.y);
        }
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
        let metrics = if measure_input.goal == LayoutGoal::Commit {
            store.block.commit(context, constraint)
        } else {
            store.block.probe(context, constraint)
        };
        LeafMetrics::new(metrics.size)
            .with_first_baselines(hughie::geometry::Point::new(None, metrics.first_baseline))
    });

    if input.goal == LayoutGoal::Commit {
        place_and_hide(tree, state, element, &atoms);
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
    atoms: &[(NodeSlot, u64)],
) {
    use hughie::text::block::PlacedBox;
    use hughie::tree::LayoutTree;

    let placements: Vec<PlacedBox> = state
        .text_block(element)
        .and_then(TextBlockStore::committed)
        .map(|block| block.boxes().to_vec())
        .unwrap_or_default();

    for placed in placements {
        let (id, origin) = match placed {
            PlacedBox::Visible { id, origin, .. } => (id, Some(origin)),
            // The Lynx `HideView` surface: a truncated-away atom has no
            // position, so it generates no box this frame.
            PlacedBox::Hidden { id } => (id, None),
        };
        let Some(&(slot, _)) = atoms.iter().find(|(_, atom_id)| *atom_id == id) else {
            continue;
        };
        match origin {
            Some(origin) => {
                // The atom keeps the box its own layout produced; the
                // paragraph decides only where it sits.
                let slot_layout = tree.layout_mut(state, slot);
                let mut placed_layout =
                    hughie::tree::Layout::with_order(slot_layout.unrounded.order);
                placed_layout.size = slot_layout.unrounded.size;
                placed_layout.content_size = slot_layout.unrounded.content_size;
                placed_layout.border = slot_layout.unrounded.border;
                placed_layout.padding = slot_layout.unrounded.padding;
                placed_layout.margin = slot_layout.unrounded.margin;
                placed_layout.location = origin;
                tree.set_unrounded_layout(state, slot, placed_layout);
            }
            None => hughie::compute::hide_subtree(tree, state, slot),
        }
    }

    // Out-of-flow children never entered the paragraph, so the block lays
    // them out itself against its own padding box — the same thing every
    // other container algorithm does for the children it does not flow.
    let container = tree.layout_mut(state, element).unrounded.size;
    let border = tree.layout_mut(state, element).unrounded.border;
    let padding = tree.layout_mut(state, element).unrounded.padding;
    let padding_box = Size::new(
        (container.width - border.left - border.right).max(0.0),
        (container.height - border.top - border.bottom).max(0.0),
    );
    let content_origin =
        hughie::geometry::Point::new(border.left + padding.left, border.top + padding.top);
    for child in tree.children(element) {
        let node = tree.at(child);
        if !node.is_element() || StyleView::of(node).position() == PositionProperty::Static {
            continue;
        }
        let mut layout = hughie::compute::compute_absolute_layout(
            tree,
            state,
            child,
            padding_box,
            content_origin,
        );
        layout.location.x += border.left;
        layout.location.y += border.top;
        tree.set_unrounded_layout(state, child, layout);
    }

    // Everything the paragraph swallowed generates no box. Hiding the whole
    // child rather than each consumed node keeps this O(children): a nested
    // scope's own subtree is consumed with it.
    //
    // Atoms are exempt — they were just placed — and hiding one would zero the
    // geometry this pass gave it.
    for child in tree.children(element) {
        if atoms.iter().any(|(slot, _)| *slot == child) {
            continue;
        }
        // An out-of-flow child is not the paragraph's to hide: it never
        // entered the flatten walk, and the absolute pass places it against
        // its containing block.
        let node = tree.at(child);
        if node.is_element() && StyleView::of(node).position() != PositionProperty::Static {
            continue;
        }
        hughie::compute::hide_subtree(tree, state, child);
    }
}
