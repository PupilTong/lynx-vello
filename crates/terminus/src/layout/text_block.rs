//! A Lynx text element: one element, one flattened paragraph.
//!
//! A text block swallows its subtree. The element that establishes it owns the
//! shaped paragraph; every text node and nested text scope inside it is
//! *content* rather than a box, and generates none. That is the whole
//! difference from the path this replaces, where each text node was its own
//! leaf and sibling runs could not share a line.
//!
//! # Classifying a child
//!
//! By the child's kind and computed display ([`crate::layout::style::display_mode`]),
//! never by tag name. `crates/terminus` must not contain Lynx element vocabulary,
//! and it does not need to: the host defines the Lynx text tags as Lynx text
//! elements ([`crate::tree::custom::CustomElement::is_lynx_text`]), and the Lynx UA sheet
//! encodes the rest of the structure as display values, so the rules below
//! reproduce it without naming `text`, `wrapper`, `raw-text` or `view`. The
//! rows are checked in order.
//!
//! | child | role |
//! |---|---|
//! | (text node) | a run, carrying its innermost element ancestor's style |
//! | `display: none` | skipped, and hidden |
//! | `display: contents` | transparent: recurse, contributing no scope of its own |
//! | a Lynx text element | a nested scope: recurse, its runs carry *its* style |
//! | anything else | exactly one atomic inline box, never recursed into |
//!
//! The last row is what makes an inline `<view>` atomic: its inner text can
//! never join this paragraph, because the walk does not descend through a box.

use std::borrow::Cow;

use seldon::geometry::{Edges, Size};
use seldon::style::{CoreStyle, PositionProperty};
use seldon::text::block::{
    BlockStyle, InlineBoxSpec, InlineItem, PlacedBox, RunStyle, TextBlock, TextRunItem,
    VerticalAlign,
};

use crate::layout::paragraph::{
    AtomOffset, BlockBox, Collected, OutOfFlowBox, OwnedItem, Placement, TextStore,
    collect_generated, context_and_store, generated_content, hash_flow, place_and_hide,
    run_text_security, run_white_space,
};
use crate::layout::style::{DisplayMode, StyleView, TextRunView};
use crate::tree::document::{DocumentLayoutState, NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;

pub(crate) struct TextBlockStore {
    pub(crate) block: TextBlock,
    pub(crate) source_ids: Vec<NodeId>,
    pub(crate) truncation_source_ids: Vec<NodeId>,
    fingerprint: u64,
    pub(crate) rebuilds: u32,
}

impl TextBlockStore {
    pub(crate) fn committed(&self) -> Option<&TextBlock> {
        self.block.has_committed().then_some(&self.block)
    }
}

pub(crate) fn inline_scopes<T>(
    tree: &TreeArenas<T>,
    node: NodeId,
    paragraph: NodeId,
) -> impl Iterator<Item = NodeId> + '_ {
    let container = tree.anonymous_container(paragraph);
    std::iter::successors(Some(node), |&id| tree.get(id)?.flat_parent_id())
        .take_while(move |&id| id != paragraph && Some(id) != container && tree.get(id).is_some())
}

struct Flattened<'tree> {
    content: Vec<Collected<'tree>>,
    truncation: Vec<Collected<'tree>>,
    markers: Vec<NodeSlot>,
}

pub(crate) const fn out_of_flow(position: PositionProperty) -> bool {
    matches!(
        position,
        PositionProperty::Absolute | PositionProperty::Fixed
    )
}

fn is_truncation_marker<T>(node: &Node<T>) -> bool {
    if !node.is_element() {
        return false;
    }
    let view = StyleView::of(node);
    !out_of_flow(view.position()) && view.display_mode() == DisplayMode::Text && {
        use seldon::style::LynxTextStyle;
        view.is_inline_truncation()
    }
}

fn collect_block<T>(tree: &TreeArenas<T>, element: NodeSlot) -> Flattened<'_> {
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

pub(crate) fn source_text<T>(tree: &TreeArenas<T>, element: NodeSlot) -> String {
    super::paragraph::source_text_of(&collect_block(tree, element).content)
}

fn collect_content<'tree, T>(
    tree: &'tree TreeArenas<T>,
    element: NodeSlot,
    skip: &[NodeSlot],
) -> Vec<Collected<'tree>> {
    let mut collected = Vec::new();
    if collect_generated(tree, element, &mut collected) {
        return collected;
    }
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
                    text: Cow::Borrowed(text),
                    style: RunStyle::from_run_style(&run),
                    white_space: run_white_space(node),
                    text_security: run_text_security(node),
                },
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
        if out_of_flow(view.position()) {
            continue;
        }
        match view.display_mode() {
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

fn fingerprint(flat: &Flattened<'_>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    flat.markers.hash(&mut hasher);
    for collected in [&flat.content, &flat.truncation] {
        hash_flow(collected, &mut hasher);
    }
    hasher.finish()
}

fn as_items<'a>(collected: &'a [Collected<'_>], id_offset: usize) -> Vec<InlineItem<'a>> {
    collected
        .iter()
        .enumerate()
        .map(|(index, entry)| match &entry.item {
            OwnedItem::Run {
                text,
                style,
                white_space,
                text_security,
            } => InlineItem::Run(TextRunItem {
                text,
                style,
                white_space: *white_space,
                text_security: *text_security,
            }),
            OwnedItem::Atom { vertical_align } => InlineItem::Box(InlineBoxSpec {
                id: (id_offset + index) as u64,
                size: Size::ZERO,
                baseline: None,
                vertical_align: *vertical_align,
            }),
            OwnedItem::Open { .. } | OwnedItem::Close => {
                unreachable!("the Lynx flatten walk opens no inline box")
            }
        })
        .collect()
}

fn block_style<T>(tree: &TreeArenas<T>, element: NodeSlot) -> BlockStyle {
    let node = tree.at(element);
    let view = StyleView::of(node);
    BlockStyle {
        strut_line_when_empty: crate::editing::has_line_if_empty(node),
        ..BlockStyle::from_container_style(&view, &TextRunView::from_values(view.values()))
    }
}

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

    let needs_build = state.text_block(element).is_none_or(|store| {
        store.fingerprint != stamp
            || !store.block.shaped_with(
                &style,
                flat.content
                    .iter()
                    .chain(&flat.truncation)
                    .filter_map(|entry| match &entry.item {
                        OwnedItem::Run { style, .. } => Some(style),
                        _ => None,
                    }),
            )
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
        let (context, slot) = state.text_store_parts(element);
        let rebuilds = slot.as_deref().map_or(0, |store| store.rebuilds() + 1);
        let truncation = (!flat.markers.is_empty()).then_some(&truncation_items[..]);
        let block = TextBlock::new(context, style, &items, truncation);
        *slot = Some(Box::new(TextStore::Block(TextBlockStore {
            block,
            source_ids,
            truncation_source_ids,
            fingerprint: stamp,
            rebuilds,
        })));
    } else if let Some((_, store)) = context_and_store(state, element)
        && let Some(store) = store.as_block_mut()
    {
        store.block.set_style(style);
    }
    atoms
}

fn atoms_of(collected: &[Collected<'_>], id_offset: usize) -> Vec<(NodeSlot, u64)> {
    collected
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry.item, OwnedItem::Atom { .. }))
        .map(|(index, entry)| (entry.source, (id_offset + index) as u64))
        .collect()
}

pub(crate) const ATOM_SPACE: seldon::tree::AvailableSpace =
    seldon::tree::AvailableSpace::MaxContent;

#[derive(Clone, Copy)]
pub(crate) struct Atom {
    pub(crate) slot: NodeSlot,
    pub(crate) id: u64,
    pub(crate) margin: Edges<f32>,
}

pub(crate) fn margin_box(border_box: Size<f32>, margin: Edges<f32>) -> Size<f32> {
    Size::new(
        (border_box.width + margin.horizontal_sum()).max(0.0),
        (border_box.height + margin.vertical_sum()).max(0.0),
    )
}

pub(crate) fn compute_text_block_layout<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    element: NodeSlot,
    input: seldon::tree::LayoutInput,
) -> seldon::tree::LayoutOutput {
    use seldon::compute::compute_text_block_layout as compute_block_box;
    use seldon::tree::LayoutTree;

    let atoms = refresh(tree, state, element);

    let mut placed_atoms: Vec<Atom> = Vec::with_capacity(atoms.len());
    for &(atom, id) in &atoms {
        let output = if input.goal.commits() {
            seldon::compute::compute_inline_box_layout(
                tree,
                state,
                atom,
                Size::new(None, None),
                input.parent_size,
                Size::new(ATOM_SPACE, ATOM_SPACE),
            )
        } else {
            tree.compute_layout(
                state,
                atom,
                seldon::tree::LayoutInput::measure(
                    Size::new(None, None),
                    input.parent_size,
                    Size::new(ATOM_SPACE, ATOM_SPACE),
                    seldon::tree::RequestedAxis::Both,
                )
                .reading_first_baseline(true),
            )
        };
        let margin = seldon::compute::used_margins(&tree.style(atom), input.parent_size.width);
        if let Some((_, store)) = context_and_store(state, element)
            && let Some(store) = store.as_block_mut()
        {
            let baseline = output
                .first_baseline
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

    let basis = input.known_dimensions.width.or(input.parent_size.width);
    let view = crate::layout::style::StyleView::of(tree.at(element));
    let output = compute_block_box(input, &view, |measure_input| {
        crate::layout::paragraph::measure(tree, state, element, basis, measure_input)
    });

    if input.goal.commits() {
        let block = BlockBox::of(&view, output.size, input.parent_size.width);
        let placements: Vec<PlacedBox> = state
            .text_block(element)
            .and_then(TextBlockStore::committed)
            .map(|block| block.boxes().to_vec())
            .unwrap_or_default();
        let content_origin = block.content_origin();
        let out_of_flow_children: Vec<OutOfFlowBox> = tree
            .children(element)
            .filter(|&child| {
                let node = tree.at(child);
                node.is_element() && out_of_flow(StyleView::of(node).position())
            })
            .map(|slot| OutOfFlowBox {
                slot,
                static_position: content_origin,
                from_right: false,
                containing: None,
            })
            .collect();
        place_and_hide(
            tree,
            state,
            element,
            &Placement {
                atoms: &placed_atoms,
                placements: &placements,
                offset: AtomOffset::Ignored,
                out_of_flow: &out_of_flow_children,
            },
            &block,
        );
        state.note_text_layout(element);
    } else {
        state.note_probed_text(element);
    }
    output
}

#[cfg(test)]
mod generated_content_tests {
    use seldon::text::FontBlob;

    use super::*;
    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::Document;

    fn document(css: &str) -> (Document<()>, NodeId) {
        let mut doc = Document::new(crate::tree::document::tests::device(), "page", ());
        crate::test_common::define_lynx_text(&mut doc, "text");
        doc.register_fonts(FontBlob::from_static(include_bytes!(
            "../../../seldon/tests/fixtures/Ahem.ttf"
        )));
        doc.add_stylesheet("page { display: flex; } text { font-family: Ahem; font-size: 20px; } scope { display: contents; }", StylesheetOrigin::Author);
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
                _ => None,
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
            assert_eq!(doc.get(text).unwrap().child_ids(), []);
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
    fn generated_content_wraps_and_keeps_its_newlines() {
        let (mut doc, text) = document("text { width: 40px; } text { content: attr(text); }");
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
        width(&doc, text, 40.0);
        assert!((doc.text_block_size(text).unwrap().height - 40.0).abs() < 0.001);
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

#[cfg(test)]
mod range_box_tests {
    use euclid::default::{Point2D, Rect, Size2D};
    use seldon::text::FontBlob;

    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::Document;

    #[test]
    fn a_laid_out_paragraph_answers_its_range_boxes_in_content_box_coordinates() {
        let mut doc = Document::new(crate::tree::document::tests::device(), "page", ());
        crate::test_common::define_lynx_text(&mut doc, "text");
        doc.register_fonts(FontBlob::from_static(include_bytes!(
            "../../../seldon/tests/fixtures/Ahem.ttf"
        )));
        doc.add_stylesheet(
            "page { display: flex; align-items: flex-start; } \
             text { font-family: Ahem; font-size: 10px; }",
            StylesheetOrigin::Author,
        );
        let text = doc.create_element("text", ());
        doc.set_inline_style(text, "width: 35px; padding: 5px; border: 2px solid;");
        doc.append_child(doc.document_element().id(), text);
        let run = doc.create_text_node("abc ", ());
        doc.append_child(text, run);
        let nested = doc.create_element("text", ());
        doc.append_child(text, nested);
        let tail = doc.create_text_node("def", ());
        doc.append_child(nested, tail);
        assert_eq!(doc.text_range_boxes(text, 0, 1), None, "no pass has run");
        doc.layout();

        let rect = |left: f32, top: f32, width: f32| {
            Rect::new(Point2D::new(left, top), Size2D::new(width, 10.0))
        };
        assert_eq!(
            doc.text_range_boxes(text, 1, 6),
            Some(vec![rect(10.0, 0.0, 20.0), rect(0.0, 10.0, 20.0)]),
            "one box per line, across the nested run's style boundary"
        );
        assert_eq!(
            doc.text_range_boxes(text, 7, 7),
            Some(vec![rect(0.0, 0.0, 0.0)])
        );
        assert_eq!(doc.text_range_boxes(text, 3, 4), Some(Vec::new()));
        assert_eq!(doc.text_range_boxes(text, 0, 8), None, "past the source");
        assert_eq!(doc.text_range_boxes(text, 2, 1), None);
        assert_eq!(
            doc.text_range_boxes(nested, 0, 1),
            None,
            "a nested scope is content of its paragraph, not one of its own"
        );
        assert_eq!(
            doc.text_range_boxes(doc.document_element().id(), 0, 0),
            None
        );
    }
}

#[cfg(test)]
mod hit_tests {
    use euclid::default::Point2D;
    use seldon::text::FontBlob;

    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::{Document, NodeId};

    fn paragraph(inline: &str) -> (Document<()>, NodeId) {
        let mut doc = Document::new(crate::tree::document::tests::device(), "page", ());
        for tag in ["text", "a", "strong", "span"] {
            crate::test_common::define_lynx_text(&mut doc, tag);
        }
        doc.register_fonts(FontBlob::from_static(include_bytes!(
            "../../../seldon/tests/fixtures/Ahem.ttf"
        )));
        doc.add_stylesheet(
            "page { display: flex; align-items: flex-start; } \
             text { font-family: Ahem; font-size: 10px; } \
             view { display: flex; }",
            StylesheetOrigin::Author,
        );
        let text = doc.create_element("text", ());
        doc.set_inline_style(text, inline);
        doc.append_child(doc.document_element().id(), text);
        (doc, text)
    }

    fn append_text(doc: &mut Document<()>, parent: NodeId, text: &str) -> NodeId {
        let node = doc.create_text_node(text, ());
        doc.append_child(parent, node);
        node
    }

    fn append_element(doc: &mut Document<()>, parent: NodeId, tag: &str) -> NodeId {
        let element = doc.create_element(tag, ());
        doc.append_child(parent, element);
        element
    }

    fn at(doc: &Document<()>, text: NodeId, x: f32, y: f32) -> Option<usize> {
        doc.text_index_at_point(text, Point2D::new(x, y))
    }

    #[test]
    fn a_point_answers_the_source_offset_of_the_cluster_under_it() {
        let (mut doc, text) = paragraph("width: 35px; padding: 5px; border: 2px solid;");
        append_text(&mut doc, text, "abc ");
        let nested = append_element(&mut doc, text, "span");
        append_text(&mut doc, nested, "def");
        assert_eq!(at(&doc, text, 0.0, 0.0), None, "no pass has run");
        doc.layout();

        assert_eq!(at(&doc, text, 0.0, 0.0), Some(0), "the first glyph");
        assert_eq!(
            at(&doc, text, 29.9, 9.9),
            Some(2),
            "the first line's last glyph"
        );
        assert_eq!(
            at(&doc, text, 32.0, 5.0),
            None,
            "the space the soft wrap left hanging"
        );
        assert_eq!(at(&doc, text, 34.9, 5.0), None, "past the line's end");
        assert_eq!(
            at(&doc, text, 0.0, 10.0),
            Some(4),
            "the wrapped line, in the nested run"
        );
        assert_eq!(at(&doc, text, 25.0, 15.0), Some(6));
        assert_eq!(at(&doc, text, 30.0, 15.0), None, "past the last glyph");
        assert_eq!(at(&doc, text, 5.0, -0.1), None, "above the paragraph");
        assert_eq!(at(&doc, text, 5.0, 20.0), None, "below the last line");
        assert_eq!(
            at(&doc, nested, 0.0, 10.0),
            None,
            "a nested scope is content of its paragraph, not one of its own"
        );
        assert_eq!(at(&doc, doc.document_element().id(), 0.0, 0.0), None);

        let hit = at(&doc, text, 15.0, 15.0).expect("on `e`");
        let unit = u32::try_from(hit).unwrap();
        let boxes = doc.text_range_boxes(text, unit, unit + 1).unwrap();
        assert!(boxes[0].contains(Point2D::new(15.0, 15.0)));
    }

    #[test]
    fn the_leading_between_two_lines_is_the_upper_lines() {
        let (mut doc, text) = paragraph("width: 35px; line-height: 20px;");
        append_text(&mut doc, text, "abc def");
        doc.layout();
        assert_eq!(at(&doc, text, 5.0, 2.0), Some(0), "the leading above");
        assert_eq!(at(&doc, text, 5.0, 19.9), Some(0), "the leading below");
        assert_eq!(at(&doc, text, 5.0, 20.0), Some(4), "the next line's top");
        assert_eq!(at(&doc, text, 5.0, 40.0), None);
    }

    #[test]
    fn a_point_on_an_atomic_inline_answers_its_one_unit() {
        let (mut doc, text) = paragraph("");
        append_text(&mut doc, text, "ab");
        let atom = append_element(&mut doc, text, "view");
        doc.set_inline_style(atom, "width: 20px; height: 10px;");
        append_text(&mut doc, text, "cd");
        doc.layout();
        assert_eq!(at(&doc, text, 19.9, 5.0), Some(1));
        assert_eq!(at(&doc, text, 20.0, 5.0), Some(2), "the atom's left edge");
        assert_eq!(at(&doc, text, 39.9, 5.0), Some(2));
        assert_eq!(at(&doc, text, 40.0, 5.0), Some(3));
        assert_eq!(doc.inline_source_range(atom), Some(2..3));
    }

    #[test]
    fn an_inline_element_answers_the_source_range_of_its_subtree() {
        let (mut doc, text) = paragraph("");
        let plain = append_text(&mut doc, text, "ab");
        let link = append_element(&mut doc, text, "a");
        append_text(&mut doc, link, "cd");
        let strong = append_element(&mut doc, link, "strong");
        append_text(&mut doc, strong, "ef");
        let inner_atom = append_element(&mut doc, link, "view");
        doc.set_inline_style(inner_atom, "width: 10px; height: 10px;");
        append_text(&mut doc, text, "g");
        let atom = append_element(&mut doc, text, "view");
        doc.set_inline_style(atom, "width: 10px; height: 10px;");
        let in_atom = append_element(&mut doc, atom, "view");
        doc.set_inline_style(in_atom, "width: 5px; height: 5px;");
        let empty = append_element(&mut doc, text, "span");
        append_text(&mut doc, text, "h");
        assert_eq!(doc.inline_source_range(link), None, "no pass has run");
        assert_eq!(doc.paragraph_of(link), None, "no pass has run");
        doc.layout();

        assert_eq!(doc.inline_source_range(strong), Some(4..6), "a plain run");
        assert_eq!(
            doc.inline_source_range(link),
            Some(2..7),
            "its own text, the nested run and the nested atom"
        );
        assert_eq!(doc.inline_source_range(inner_atom), Some(6..7));
        assert_eq!(doc.inline_source_range(atom), Some(8..9));
        assert_eq!(doc.inline_source_range(empty), None, "contributes nothing");
        assert_eq!(doc.inline_source_range(in_atom), None, "inside an atom");
        assert_eq!(doc.inline_source_range(text), None, "the paragraph itself");
        assert_eq!(doc.inline_source_range(plain), None, "a text node");

        for node in [text, plain, link, strong, inner_atom, atom, empty] {
            assert_eq!(doc.paragraph_of(node), Some(text), "{node:?}");
        }
        assert_eq!(doc.paragraph_of(in_atom), None, "an atom's own subtree");
        assert_eq!(doc.paragraph_of(doc.document_element().id()), None);

        let hit = at(&doc, text, 55.0, 5.0).expect("on `f`");
        assert_eq!(hit, 5);
        assert!(doc.inline_source_range(strong).unwrap().contains(&hit));
    }

    #[test]
    fn a_measured_paragraph_reads_a_linear_atom_s_first_baseline() {
        let (mut doc, text) = paragraph("");
        append_text(&mut doc, text, "ab");
        let atom = append_element(&mut doc, text, "view");
        doc.set_inline_style(atom, "display: linear; width: 20px; height: 20px;");
        let inner = append_element(&mut doc, atom, "text");
        doc.set_inline_style(inner, "width: 10px; height: 20px;");
        append_text(&mut doc, inner, "c");
        doc.layout();
        let measured = doc.text_block_size(text).expect("laid out paragraph");
        let outer = doc.rounded_layout(text).expect("placed");
        let placed = doc.rounded_layout(atom).expect("placed");
        assert_eq!(
            (measured.height, outer.size.height, placed.location.y),
            (20.0, 20.0, 0.0)
        );
    }

    #[test]
    fn an_atom_with_a_paragraph_of_its_own_answers_that_one() {
        let (mut doc, text) = paragraph("");
        append_text(&mut doc, text, "ab");
        let atom = append_element(&mut doc, text, "view");
        let inner = append_element(&mut doc, atom, "text");
        let run = append_text(&mut doc, inner, "c");
        let out_of_flow = append_element(&mut doc, text, "view");
        doc.set_inline_style(
            out_of_flow,
            "position: absolute; width: 10px; height: 10px;",
        );
        doc.layout();
        assert_eq!(doc.paragraph_of(atom), Some(text));
        assert_eq!(doc.paragraph_of(inner), Some(inner));
        assert_eq!(doc.paragraph_of(run), Some(inner));
        assert_eq!(doc.paragraph_of(out_of_flow), None, "not inline content");
        assert_eq!(doc.inline_source_range(atom), Some(2..3));
        assert_eq!(at(&doc, inner, 5.0, 5.0), Some(0));
    }
}

#[cfg(test)]
mod empty_host_tests {
    #![allow(clippy::float_cmp)]

    use seldon::geometry::Size;
    use seldon::text::FontBlob;

    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::{Document, NodeId};

    fn document() -> Document<()> {
        let mut doc = Document::new(crate::tree::document::tests::device(), "page", ());
        crate::test_common::define_lynx_text(&mut doc, "text");
        doc.register_fonts(FontBlob::from_static(include_bytes!(
            "../../../seldon/tests/fixtures/Ahem.ttf"
        )));
        doc.add_stylesheet(".flow { display: block; }", StylesheetOrigin::UserAgent);
        doc.add_stylesheet(
            "page { display: flex; flex-direction: column; align-items: flex-start; \
             font-family: Ahem; font-size: 10px; line-height: 24px; }",
            StylesheetOrigin::Author,
        );
        doc
    }

    fn empty(doc: &mut Document<()>, block: bool) -> NodeId {
        let element = if block {
            let element = doc.create_element("div", ());
            doc.set_attribute(element, "class", "flow");
            element
        } else {
            doc.create_element("text", ())
        };
        doc.append_child(doc.document_element().id(), element);
        element
    }

    fn height(doc: &Document<()>, id: NodeId) -> f32 {
        doc.rounded_layout(id).expect("laid out").size.height
    }

    #[test]
    fn an_empty_editing_host_lays_out_one_line_of_its_strut() {
        for block in [false, true] {
            let mut doc = document();
            let host = empty(&mut doc, block);
            doc.set_attribute(host, "contenteditable", "plaintext-only");
            doc.layout();
            assert_eq!(doc.paragraph_line_count(host), Some(1), "block {block}");
            assert_eq!(doc.text_block_size(host), Some(Size::new(0.0, 24.0)));
            assert_eq!(height(&doc, host), 24.0);

            doc.remove_attribute(host, "contenteditable");
            doc.layout();
            assert_eq!(doc.paragraph_line_count(host), Some(0), "block {block}");
            assert_eq!(doc.text_block_size(host), Some(Size::new(0.0, 0.0)));
            assert_eq!(height(&doc, host), 0.0);

            doc.set_attribute(host, "contenteditable", "FALSE");
            doc.layout();
            assert_eq!(doc.paragraph_line_count(host), Some(0), "no host");
            assert_eq!(height(&doc, host), 0.0);

            doc.set_attribute(host, "contenteditable", "");
            doc.layout();
            assert_eq!(doc.paragraph_line_count(host), Some(1), "block {block}");
            assert_eq!(height(&doc, host), 24.0);

            let text = doc.create_text_node("ab", ());
            doc.append_child(host, text);
            doc.layout();
            assert_eq!(doc.paragraph_line_count(host), Some(1));
            assert_eq!(doc.text_block_size(host), Some(Size::new(20.0, 24.0)));
            doc.set_text_node_data(text, "");
            doc.layout();
            assert_eq!(doc.paragraph_line_count(host), Some(1), "empty again");
            assert_eq!(height(&doc, host), 24.0);
        }
    }

    #[test]
    fn an_empty_paragraph_that_is_no_host_has_no_line() {
        for block in [false, true] {
            let mut doc = document();
            let element = empty(&mut doc, block);
            doc.layout();
            assert_eq!(doc.paragraph_line_count(element), Some(0), "block {block}");
            assert_eq!(doc.text_block_size(element), Some(Size::new(0.0, 0.0)));
            assert_eq!(height(&doc, element), 0.0);
        }
    }
}

#[cfg(test)]
mod layout_report_tests {
    use seldon::text::FontBlob;

    use crate::layout::TextLineReport;
    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::{Document, NodeId};

    fn paragraph(inline: &str, content: &str) -> (Document<()>, NodeId, NodeId) {
        let mut doc = Document::new(crate::tree::document::tests::device(), "page", ());
        crate::test_common::define_lynx_text(&mut doc, "text");
        doc.register_fonts(FontBlob::from_static(include_bytes!(
            "../../../seldon/tests/fixtures/Ahem.ttf"
        )));
        doc.add_stylesheet(
            "page { display: flex; flex-direction: column; align-items: flex-start; \
                    width: 300px; } \
             text { font-family: Ahem; font-size: 20px; line-height: 21px; } \
             view { display: flex; }",
            StylesheetOrigin::Author,
        );
        let text = doc.create_element("text", ());
        doc.set_inline_style(text, inline);
        doc.append_child(doc.document_element().id(), text);
        let run = doc.create_text_node(content, ());
        doc.append_child(text, run);
        let sibling = doc.create_element("view", ());
        doc.set_inline_style(sibling, "width: 10px; height: 10px;");
        doc.append_child(doc.document_element().id(), sibling);
        (doc, text, sibling)
    }

    fn line(start: u32, end: u32, ellipsis_count: u32) -> TextLineReport {
        TextLineReport {
            start,
            end,
            ellipsis_count,
        }
    }

    #[test]
    fn a_reporting_text_reports_each_committing_layout_and_no_cache_hit() {
        let (mut doc, text, sibling) = paragraph("width: 100px;", "abcde fghij");
        doc.set_text_layout_reporting(text, true);
        doc.layout();
        assert_eq!(doc.take_text_layout_reports(), [text]);
        assert_eq!(
            doc.take_text_layout_reports(),
            [],
            "the take empties the list"
        );

        doc.layout();
        assert_eq!(doc.take_text_layout_reports(), []);

        doc.set_inline_style(sibling, "width: 30px; height: 10px;");
        doc.layout();
        assert_eq!(
            doc.rounded_layout(sibling).expect("laid out").size,
            seldon::geometry::Size::new(30.0, 10.0),
            "the pass ran"
        );
        assert_eq!(doc.take_text_layout_reports(), []);

        assert_eq!(doc.text_lines(text).map(|lines| lines.len()), Some(2));
        doc.set_inline_style(text, "width: 60px;");
        doc.layout();
        assert_eq!(doc.take_text_layout_reports(), [text]);
        assert_eq!(doc.text_lines(text).map(|lines| lines.len()), Some(4));
    }

    #[test]
    fn a_text_without_reporting_never_reports() {
        let (mut doc, text, _) = paragraph("width: 100px;", "abcde fghij");
        doc.layout();
        doc.set_inline_style(text, "width: 60px;");
        doc.layout();
        assert_eq!(doc.take_text_layout_reports(), []);

        doc.set_text_layout_reporting(text, true);
        doc.set_inline_style(text, "width: 100px;");
        doc.layout();
        doc.set_text_layout_reporting(text, false);
        assert_eq!(doc.take_text_layout_reports(), []);
        doc.set_inline_style(text, "width: 60px;");
        doc.layout();
        assert_eq!(doc.take_text_layout_reports(), []);
    }

    #[test]
    fn the_take_lists_a_paragraph_reported_by_two_layouts_once() {
        let (mut doc, text, _) = paragraph("width: 100px;", "abcde fghij");
        let other = doc.create_element("text", ());
        doc.append_child(doc.document_element().id(), other);
        let run = doc.create_text_node("abc", ());
        doc.append_child(other, run);
        doc.set_text_layout_reporting(text, true);
        doc.set_text_layout_reporting(other, true);
        doc.layout();
        doc.set_inline_style(text, "width: 120px;");
        doc.layout();
        assert_eq!(
            doc.layout_state().text_layout_reports,
            [text, other, text],
            "two committing runs of `text` before a take"
        );
        assert_eq!(
            doc.take_text_layout_reports(),
            [text, other],
            "first occurrence order"
        );
    }

    #[test]
    fn a_paragraph_committed_twice_in_one_layout_is_taken_once() {
        let (mut doc, text, _) = paragraph("width: 100px;", "abcde fghij");
        let container = doc.create_element("view", ());
        doc.set_inline_style(
            container,
            "container-type: inline-size; width: 100px; flex-direction: column;",
        );
        let page = doc.document_element().id();
        doc.insert_before(page, container, Some(text));
        let inner = doc.create_element("view", ());
        doc.set_inline_style(inner, "width: 10px; height: 50cqw;");
        doc.append_child(container, inner);
        doc.set_text_layout_reporting(text, true);
        doc.layout();
        assert_eq!(
            doc.rounded_layout(container).expect("laid out").size,
            seldon::geometry::Size::new(100.0, 50.0)
        );
        assert_eq!(
            doc.layout_state().text_layout_reports,
            [text, text],
            "both passes of the one layout committed the paragraph"
        );
        assert_eq!(doc.take_text_layout_reports(), [text]);
    }

    #[test]
    fn freeing_a_reporting_text_drops_its_pending_report() {
        let (mut doc, text, _) = paragraph("width: 100px;", "abcde fghij");
        doc.set_text_layout_reporting(text, true);
        doc.layout();
        let _ = doc.drop_subtree(text);
        assert_eq!(doc.take_text_layout_reports(), []);
        assert!(doc.layout_state().text_layout_reporters.is_empty());
    }

    #[test]
    fn text_lines_answers_each_line_source_range() {
        let (mut doc, text, sibling) = paragraph("width: 100px;", "abcde fghij");
        assert_eq!(doc.text_lines(text), None, "no pass has run");
        doc.layout();
        let lines = doc.text_lines(text).expect("a committed paragraph");
        assert_eq!(lines, [line(0, 5, 0), line(6, 11, 0)]);
        assert_eq!(doc.text_lines(sibling), None, "not a Lynx text element");
        assert_eq!(doc.text_lines(doc.document_element().id()), None);
    }

    #[test]
    fn text_lines_states_a_maxline_cut_in_its_ellipsis_count() {
        let (mut doc, text, _) = paragraph(
            "width: 100px; text-overflow: ellipsis; --lynx-text-maxline: 1;",
            "abcde fghij",
        );
        doc.add_stylesheet(
            r#"@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: 0; }"#,
            StylesheetOrigin::UserAgent,
        );
        doc.layout();
        let lines = doc.text_lines(text).expect("a committed paragraph");
        assert_eq!(lines, [line(0, 5, 3)]);
    }

    #[test]
    #[should_panic(expected = "non-Lynx-text NodeId passed to Document::set_text_layout_reporting")]
    fn reporting_on_an_element_that_is_not_lynx_text_panics() {
        let (mut doc, _, sibling) = paragraph("width: 100px;", "abcde fghij");
        doc.set_text_layout_reporting(sibling, true);
    }
}
