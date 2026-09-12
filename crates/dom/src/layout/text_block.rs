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
fn fingerprint(collected: &[Collected]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
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
    use hughie::tree::LayoutTree;

    let atoms = refresh(tree, state, element);

    // Phase 2. Each atom is laid out as its own subtree; the paragraph only
    // ever sees the margin-box result.
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
        let metrics = if measure_input.goal.commits() {
            store.block.commit(context, constraint)
        } else {
            store.block.probe(context, constraint)
        };
        LeafMetrics::new(metrics.size)
            .with_first_baselines(hughie::geometry::Point::new(None, metrics.first_baseline))
    });

    if input.goal.commits() {
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
        collect(doc.arenas(), doc.slot(text).unwrap())
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
