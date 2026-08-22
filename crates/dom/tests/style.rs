//! Direct tests for the embedder-neutral style engine.

mod common;

use common::device;
use dom::{Document, NodeId, StylesheetOrigin};
use stylo::color::AbsoluteColor;

type TestDocument = Document<()>;

const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const RED: [u8; 3] = [255, 0, 0];

fn document() -> TestDocument {
    Document::new(device(800.0, 600.0), "page", ())
}

fn rgb([red, green, blue]: [u8; 3]) -> AbsoluteColor {
    AbsoluteColor::srgb_legacy(red, green, blue, 1.0)
}

fn computed_color(doc: &TestDocument, id: NodeId) -> AbsoluteColor {
    doc.get(id).unwrap().computed_style().unwrap().clone_color()
}

macro_rules! assert_color {
    ($doc:expr, $id:expr, $expected:expr) => {
        assert_eq!(computed_color(&$doc, $id), rgb($expected))
    };
    ($doc:expr, $id:expr, $expected:expr, $message:literal) => {
        assert_eq!(computed_color(&$doc, $id), rgb($expected), $message)
    };
}

fn assert_restyle_color(doc: &mut TestDocument, id: NodeId, expected: [u8; 3]) {
    doc.layout();
    assert_eq!(computed_color(doc, id), rgb(expected));
}

#[test]
fn standard_cascade_is_embedder_neutral() {
    let mut doc = document();
    doc.add_stylesheet(
        ".parent { color: green; } .child { color: red; }",
        StylesheetOrigin::Author,
    );

    let parent = doc.document_element().id();
    let child = doc.create_element("span", ());
    doc.add_class(parent, "parent");
    doc.add_class(child, "child");
    doc.append_child(parent, child);
    doc.layout();

    let parent_style = doc.get(parent).unwrap().computed_style().unwrap();
    assert_eq!(parent_style.clone_color(), rgb(GREEN));

    doc.set_inline_style(child, "color: blue");
    doc.layout();
    let child_style = doc.get(child).unwrap().computed_style().unwrap();
    assert_eq!(
        child_style.clone_color(),
        rgb(BLUE),
        "standard inline declarations outrank author class rules"
    );
}

#[test]
fn id_class_and_style_attributes_are_reflected_dom_state() {
    let mut doc = document();
    doc.add_stylesheet(
        r#"[id="target"][class~="hot"][style] { color: red; }"#,
        StylesheetOrigin::Author,
    );
    let root = doc.document_element().id();
    let target = doc.create_element("view", ());
    doc.append_child(root, target);

    doc.set_attribute(target, "id", "target");
    doc.set_attribute(target, "class", "hot other");
    doc.set_attribute(target, "style", "width: 10px");
    doc.layout();

    let node = doc.get(target).unwrap();
    assert_eq!(node.id_attribute(), Some("target"));
    assert!(node.has_class("hot"));
    assert_eq!(node.attribute("style"), Some("width: 10px"));
    assert_eq!(node.computed_style().unwrap().clone_color(), rgb(RED));

    doc.remove_attribute(target, "class");
    doc.layout();
    let node = doc.get(target).unwrap();
    assert!(!node.has_class("hot"));
    assert_eq!(node.attribute("class"), None);
    assert_ne!(node.computed_style().unwrap().clone_color(), rgb(RED));

    for (name, value, matches) in [
        ("class", Some("hot"), true),
        ("style", None, false),
        ("style", Some("width: 20px"), true),
        ("id", None, false),
    ] {
        match value {
            Some(value) => doc.set_attribute(target, name, value),
            None => doc.remove_attribute(target, name),
        }
        doc.layout();
        assert_eq!(computed_color(&doc, target) == rgb(RED), matches);
    }
}

#[test]
fn style_traversal_skips_text_nodes_and_reaches_element_siblings() {
    let mut doc = document();
    doc.add_stylesheet("span { color: red; }", StylesheetOrigin::Author);
    let root = doc.document_element().id();
    let text = doc.create_text_node("hello", ());
    let span = doc.create_element("span", ());
    doc.append_child(root, text);
    doc.append_child(root, span);

    doc.layout();

    assert!(doc.get(root).unwrap().computed_style().is_some());
    assert!(
        doc.get(text).unwrap().computed_style().is_none(),
        "text nodes are DOM/layout children, not styled elements"
    );
    assert_color!(doc, span, RED);
}

#[test]
fn text_data_changes_invalidate_the_parent_empty_selector() {
    let mut doc = document();
    doc.add_stylesheet(
        ".box { color: blue; } .box:empty { color: red; }",
        StylesheetOrigin::Author,
    );

    let root = doc.document_element().id();
    let box_element = doc.create_element("view", ());
    let text = doc.create_text_node("", ());
    doc.add_class(box_element, "box");
    doc.append_child(box_element, text);
    doc.append_child(root, box_element);

    doc.layout();
    assert_color!(doc, box_element, RED, "an empty text node preserves :empty");

    doc.set_text_node_data(text, "hello");
    doc.layout();
    assert_color!(
        doc,
        box_element,
        BLUE,
        "non-empty text makes the parent fail :empty"
    );

    doc.set_text_node_data(text, "");
    doc.layout();
    assert_color!(doc, box_element, RED, "clearing text restores :empty");

    doc.remove_element(text);
    doc.layout();
    doc.set_text_node_data(text, "reattached");
    doc.append_child(box_element, text);
    doc.layout();
    assert_color!(
        doc,
        box_element,
        BLUE,
        "inserting non-empty text clears :empty"
    );

    doc.remove_element(text);
    doc.layout();
    assert_color!(
        doc,
        box_element,
        RED,
        "removing the only non-empty text restores :empty"
    );
}

#[test]
fn edge_child_selectors_ignore_interleaved_text_nodes_during_restyle() {
    let mut doc = document();
    doc.add_stylesheet(
        ".item { color: blue; } .item:first-child { color: red; } \
         .item:last-child { color: green; }",
        StylesheetOrigin::Author,
    );

    let root = doc.document_element().id();
    let leading_a = doc.create_text_node("a", ());
    let leading_b = doc.create_text_node("b", ());
    let first = doc.create_element("view", ());
    let last = doc.create_element("view", ());
    let trailing_a = doc.create_text_node("c", ());
    let trailing_b = doc.create_text_node("d", ());
    doc.add_class(first, "item");
    doc.add_class(last, "item");
    for child in [leading_a, leading_b, first, last, trailing_a, trailing_b] {
        doc.append_child(root, child);
    }
    doc.layout();

    assert_color!(doc, first, RED);
    assert_color!(doc, last, GREEN);

    let new_first = doc.create_element("view", ());
    doc.add_class(new_first, "item");
    doc.insert_before(root, new_first, Some(first));
    doc.layout();
    assert_color!(doc, new_first, RED);
    assert_color!(
        doc,
        first,
        BLUE,
        "the displaced first element must lose :first-child"
    );

    let new_last = doc.create_element("view", ());
    doc.add_class(new_last, "item");
    doc.append_child(root, new_last);
    doc.layout();
    assert_color!(doc, new_last, GREEN);
    assert_color!(
        doc,
        last,
        BLUE,
        "the displaced last element must lose :last-child"
    );
}

#[test]
fn media_queries_follow_standard_viewport_updates() {
    let mut doc = document();
    doc.add_stylesheet(
        "@media (min-width: 600px) { .box { color: red; } }",
        StylesheetOrigin::Author,
    );

    let root = doc.document_element().id();
    let element = doc.create_element("div", ());
    doc.add_class(element, "box");
    doc.append_child(root, element);
    doc.layout();

    let wide = doc.get(element).unwrap().computed_style().unwrap();
    assert_eq!(wide.clone_color(), rgb(RED));

    doc.set_viewport(400.0, 600.0);
    doc.layout();
    let narrow = doc.get(element).unwrap().computed_style().unwrap();
    assert_ne!(narrow.clone_color(), wide.clone_color());
}

#[test]
fn first_attachment_computes_style_and_clean_flushes_preserve_it() {
    let mut doc = document();

    let root = doc.document_element().id();
    doc.layout();
    let first = computed_color(&doc, root);

    for _ in 0..3 {
        doc.layout();
        assert_eq!(computed_color(&doc, root), first);
    }
}

#[test]
fn dom_stylesheet_and_device_mutations_rearm_clean_style_flushes() {
    let mut doc = document();
    doc.add_stylesheet(".hot { color: rgb(255, 0, 0); }", StylesheetOrigin::Author);
    let root = doc.document_element().id();
    let target = doc.create_element("view", ());
    doc.set_classes(target, "hot");
    doc.append_child(root, target);

    assert_restyle_color(&mut doc, target, RED);

    doc.set_inline_style(target, "color: rgb(0, 0, 255)");
    assert_restyle_color(&mut doc, target, BLUE);

    doc.add_stylesheet(
        ".hot { color: rgb(0, 128, 0) !important; }",
        StylesheetOrigin::Author,
    );
    assert_restyle_color(&mut doc, target, GREEN);

    doc.add_stylesheet(
        "@media (max-width: 500px) { .hot { color: rgb(1, 2, 3) !important; } }",
        StylesheetOrigin::Author,
    );
    doc.layout();
    assert_color!(doc, target, GREEN);

    doc.set_viewport(400.0, 600.0);
    assert_restyle_color(&mut doc, target, [1, 2, 3]);
}

#[test]
fn documents_own_independent_stylesheets() {
    let mut first = document();
    let mut second = document();
    first.add_stylesheet(".probe { color: red; }", StylesheetOrigin::Author);

    let first_probe = first.create_element("view", ());
    first.add_class(first_probe, "probe");
    let first_root = first.document_element().id();
    first.append_child(first_root, first_probe);
    let second_probe = second.create_element("view", ());
    second.add_class(second_probe, "probe");
    let second_root = second.document_element().id();
    second.append_child(second_root, second_probe);
    first.layout();
    second.layout();

    let first_style = first.get(first_probe).unwrap().computed_style().unwrap();
    let second_style = second.get(second_probe).unwrap().computed_style().unwrap();
    assert_eq!(first_style.clone_color(), rgb(RED));
    assert_ne!(second_style.clone_color(), first_style.clone_color());
}

/// `:nth-last-child` and the `-of-type` family count from the end, so adding or
/// removing any child can change which siblings match — Stylo asks for every
/// child to be restyled by setting `HAS_SLOW_SELECTOR` on the parent. The
/// invalidation reaches descendants of those children too, not just the
/// children themselves.
#[test]
fn counting_from_the_end_restyles_every_child_and_their_descendants() {
    let mut doc = document();
    doc.add_stylesheet(
        ".row { color: blue; } .label { color: blue; } \
         .row:nth-last-child(2) { color: red; } \
         .row:nth-last-child(1) .label { color: green; }",
        StylesheetOrigin::Author,
    );

    let root = doc.document_element().id();
    let list = doc.create_element("view", ());
    doc.append_child(root, list);

    let mut rows = Vec::new();
    let mut labels = Vec::new();
    for _ in 0..4 {
        let row = doc.create_element("view", ());
        doc.add_class(row, "row");
        let label = doc.create_element("view", ());
        doc.add_class(label, "label");
        doc.append_child(row, label);
        doc.append_child(list, row);
        rows.push(row);
        labels.push(label);
    }
    doc.layout();

    assert_color!(doc, rows[2], RED, "the second-from-last row matches");
    assert_color!(doc, rows[3], BLUE);
    assert_color!(
        doc,
        labels[3],
        GREEN,
        "the last row's label matches through its parent"
    );

    // Appending shifts every position counted from the end by one.
    let appended = doc.create_element("view", ());
    doc.add_class(appended, "row");
    let appended_label = doc.create_element("view", ());
    doc.add_class(appended_label, "label");
    doc.append_child(appended, appended_label);
    doc.append_child(list, appended);
    doc.layout();

    assert_color!(doc, rows[2], BLUE, "row 2 is no longer second-from-last");
    assert_color!(doc, rows[3], RED, "row 3 became second-from-last");
    assert_color!(
        doc,
        labels[3],
        BLUE,
        "the displaced last row's label must lose the descendant match"
    );
    assert_color!(
        doc,
        appended_label,
        GREEN,
        "the new last row's label must gain it"
    );

    // Removing from the front shifts nothing counted from the end, but the
    // parent is still asked to restyle every child.
    doc.remove_element(rows[0]);
    doc.layout();

    assert_color!(doc, rows[3], RED, "row 3 is still second-from-last");
    assert_color!(doc, appended_label, GREEN);
    assert_color!(doc, labels[1], BLUE);
}

/// `:only-of-type` matches only while a child is the sole element of its tag,
/// so it flips on both the second insertion and the return to one.
#[test]
fn only_of_type_flips_on_the_second_child_and_back() {
    let mut doc = document();
    doc.add_stylesheet(
        "view { color: blue; } view:only-of-type { color: red; }",
        StylesheetOrigin::Author,
    );

    let root = doc.document_element().id();
    let list = doc.create_element("view", ());
    doc.append_child(root, list);
    let first = doc.create_element("view", ());
    doc.append_child(list, first);
    doc.layout();
    assert_color!(doc, first, RED, "the sole view of its type matches");

    let second = doc.create_element("view", ());
    doc.append_child(list, second);
    doc.layout();
    assert_color!(doc, first, BLUE, "a second view of the type ends the match");
    assert_color!(doc, second, BLUE);

    doc.remove_element(second);
    doc.layout();
    assert_color!(doc, first, RED, "removing it restores the match");
}

/// An attribute no rule mentions still lands on the element, and a stylesheet
/// mounted afterwards matches against it.
///
/// Writing such an attribute skips the style snapshot entirely — nothing could
/// read it — which is only sound because a later stylesheet re-styles from the
/// root, covering every element whose snapshot was skipped. This is that
/// argument, executed.
#[test]
fn an_attribute_no_rule_mentions_is_matched_by_a_stylesheet_added_later() {
    let mut doc = document();
    doc.add_stylesheet("view { color: rgb(0, 0, 255) }", StylesheetOrigin::Author);
    let root = doc.document_element().id();
    let el = doc.create_element("view", ());
    doc.append_child(root, el);
    doc.layout();
    assert_color!(doc, el, BLUE);

    // No rule mentions `data-state`, so this write schedules no restyle.
    doc.set_attribute(el, "data-state", "ready");
    doc.layout();
    assert_color!(doc, el, BLUE);
    assert_eq!(
        doc.get(el).unwrap().attributes().collect::<Vec<_>>(),
        [("data-state", "ready")],
        "the attribute is on the element either way"
    );

    // The sheet arrives after the write and must still see it.
    doc.add_stylesheet(
        "view[data-state=\"ready\"] { color: rgb(255, 0, 0) }",
        StylesheetOrigin::Author,
    );
    doc.layout();
    assert_color!(
        doc,
        el,
        RED,
        "a stylesheet mounted after the write matches the attribute it skipped"
    );
}

/// The gate is per attribute name, not per element: an attribute some rule does
/// mention keeps invalidating immediately, including onto other elements.
#[test]
fn an_attribute_a_rule_mentions_still_invalidates_immediately() {
    let mut doc = document();
    doc.add_stylesheet(
        "view { color: rgb(0, 0, 255) } \
         view[data-on] { color: rgb(255, 0, 0) } \
         view[data-on] + view { color: rgb(0, 128, 0) }",
        StylesheetOrigin::Author,
    );
    let root = doc.document_element().id();
    let first = doc.create_element("view", ());
    let second = doc.create_element("view", ());
    doc.append_child(root, first);
    doc.append_child(root, second);
    doc.layout();
    assert_color!(doc, first, BLUE);
    assert_color!(doc, second, BLUE);

    doc.set_attribute(first, "data-on", "");
    doc.layout();
    assert_color!(doc, first, RED);
    assert_color!(doc, second, GREEN, "the sibling selector still fires");

    doc.remove_attribute(first, "data-on");
    doc.layout();
    assert_color!(doc, first, BLUE);
    assert_color!(doc, second, BLUE, "and unfires on removal");
}

/// Element state is gated the same way and by the same bits: `:hover` keeps
/// working while a state no rule selects on costs no restyle.
#[test]
fn element_state_invalidates_only_for_states_a_rule_selects_on() {
    let mut doc = document();
    doc.add_stylesheet(
        "view { color: rgb(0, 0, 255) } view:hover { color: rgb(255, 0, 0) }",
        StylesheetOrigin::Author,
    );
    let root = doc.document_element().id();
    let el = doc.create_element("view", ());
    doc.append_child(root, el);
    doc.layout();

    // No rule mentions `:focus`.
    doc.add_element_state(el, dom::ElementState::FOCUS);
    doc.layout();
    assert_color!(doc, el, BLUE);

    doc.add_element_state(el, dom::ElementState::HOVER);
    doc.layout();
    assert_color!(doc, el, RED, ":hover is selected on, so it restyles");

    doc.remove_element_state(el, dom::ElementState::HOVER);
    doc.layout();
    assert_color!(doc, el, BLUE);
    assert!(
        doc.get(el)
            .unwrap()
            .element_state()
            .contains(dom::ElementState::FOCUS),
        "the ungated state is still recorded on the element"
    );
}

/// The "a later stylesheet covers it" argument has to hold inside a
/// `display: none` subtree too, which a root restyle does not descend into —
/// `recalc_style_at` cuts `traverse_children` on `styles.is_display_none()`.
#[test]
fn an_ungated_attribute_written_inside_display_none_is_matched_when_shown() {
    let mut doc = document();
    doc.add_stylesheet(
        ".hidden { display: none } view { color: rgb(0, 0, 255) }",
        StylesheetOrigin::Author,
    );
    let root = doc.document_element().id();
    let container = doc.create_element("view", ());
    doc.add_class(container, "hidden");
    doc.append_child(root, container);
    let buried = doc.create_element("view", ());
    doc.append_child(container, buried);
    doc.layout();

    // No rule mentions `data-state`, so this write inside the hidden subtree
    // schedules nothing.
    doc.set_attribute(buried, "data-state", "ready");
    doc.layout();

    // The sheet arrives afterwards, and only then is the subtree shown.
    doc.add_stylesheet(
        "view[data-state=\"ready\"] { color: rgb(255, 0, 0) }",
        StylesheetOrigin::Author,
    );
    doc.layout();
    doc.remove_class(container, "hidden");
    doc.layout();

    assert_color!(
        doc,
        buried,
        RED,
        "an attribute written while hidden must match once the subtree is shown"
    );
}
