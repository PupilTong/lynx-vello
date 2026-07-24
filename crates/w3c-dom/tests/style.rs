//! Direct tests for the embedder-neutral style engine.

mod common;

use common::device;
use stylo::color::AbsoluteColor;
use w3c_dom::{Document, FlushStatus, NodeId, StylesheetOrigin};

type TestDocument = Document<()>;

const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const RED: [u8; 3] = [255, 0, 0];

fn document() -> TestDocument {
    Document::new(device(800.0, 600.0))
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

fn assert_flush(doc: &mut TestDocument, expected: FlushStatus) {
    assert_eq!(doc.flush_styles().status, expected);
}

fn assert_clean(doc: &mut TestDocument) {
    let summary = doc.flush_styles();
    assert_eq!(summary.status, FlushStatus::Skipped);
    assert!(!summary.has_damage());
}

fn assert_restyle_color(doc: &mut TestDocument, id: NodeId, expected: [u8; 3]) {
    assert_flush(doc, FlushStatus::Traversed);
    assert_eq!(computed_color(doc, id), rgb(expected));
}

#[test]
fn standard_cascade_is_embedder_neutral() {
    let mut doc = document();
    doc.add_stylesheet(
        ".parent { color: green; } .child { color: red; }",
        StylesheetOrigin::Author,
    );

    let parent = doc.create_element("section", ());
    let child = doc.create_element("span", ());
    doc.add_class(parent, "parent");
    doc.add_class(child, "child");
    doc.append_child(parent, child);

    let parent_style = doc.resolve_style(doc.get(parent).unwrap(), None);
    assert_eq!(parent_style.clone_color(), rgb(GREEN));

    doc.set_inline_style(child, "color: blue");
    let child_style = doc.resolve_style(doc.get(child).unwrap(), Some(parent_style.as_ref()));
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
    let root = doc.create_element("page", ());
    let target = doc.create_element("view", ());
    doc.append_document_element(root);
    doc.append_child(root, target);

    doc.set_attribute(target, "id", "target");
    doc.set_attribute(target, "class", "hot other");
    doc.set_attribute(target, "style", "width: 10px");
    doc.flush_styles();

    let node = doc.get(target).unwrap();
    assert_eq!(node.id_attribute(), Some("target"));
    assert!(node.has_class("hot"));
    assert_eq!(node.attribute("style"), Some("width: 10px"));
    assert_eq!(node.computed_style().unwrap().clone_color(), rgb(RED));

    doc.remove_attribute(target, "class");
    doc.flush_styles();
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
        doc.flush_styles();
        assert_eq!(computed_color(&doc, target) == rgb(RED), matches);
    }
}

#[test]
fn style_traversal_skips_text_nodes_and_reaches_element_siblings() {
    let mut doc = document();
    doc.add_stylesheet("span { color: red; }", StylesheetOrigin::Author);
    let root = doc.create_element("page", ());
    let text = doc.create_text_node("hello", ());
    let span = doc.create_element("span", ());
    doc.append_child(root, text);
    doc.append_child(root, span);
    doc.append_document_element(root);

    doc.flush_styles();

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

    let root = doc.create_element("page", ());
    let box_element = doc.create_element("view", ());
    let text = doc.create_text_node("", ());
    doc.add_class(box_element, "box");
    doc.append_child(box_element, text);
    doc.append_child(root, box_element);
    doc.append_document_element(root);

    doc.flush_styles();
    assert_color!(doc, box_element, RED, "an empty text node preserves :empty");

    doc.set_text_node_data(text, "hello");
    doc.flush_styles();
    assert_color!(
        doc,
        box_element,
        BLUE,
        "non-empty text makes the parent fail :empty"
    );

    doc.set_text_node_data(text, "");
    doc.flush_styles();
    assert_color!(doc, box_element, RED, "clearing text restores :empty");

    doc.detach(text);
    doc.flush_styles();
    doc.set_text_node_data(text, "reattached");
    doc.append_child(box_element, text);
    doc.flush_styles();
    assert_color!(
        doc,
        box_element,
        BLUE,
        "inserting non-empty text clears :empty"
    );

    doc.detach(text);
    doc.flush_styles();
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

    let root = doc.create_element("page", ());
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
    doc.append_document_element(root);
    doc.flush_styles();

    assert_color!(doc, first, RED);
    assert_color!(doc, last, GREEN);

    let new_first = doc.create_element("view", ());
    doc.add_class(new_first, "item");
    doc.insert_before(root, new_first, Some(first));
    doc.flush_styles();
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
    doc.flush_styles();
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
    doc.add_stylesheet_with_media(
        ".box { color: red; }",
        StylesheetOrigin::Author,
        "(min-width: 600px)",
    );

    let element = doc.create_element("div", ());
    doc.add_class(element, "box");

    let wide = doc.resolve_style(doc.get(element).unwrap(), None);
    assert_eq!(wide.clone_color(), rgb(RED));

    doc.set_viewport(400.0, 600.0);
    let narrow = doc.resolve_style(doc.get(element).unwrap(), None);
    assert_ne!(narrow.clone_color(), wide.clone_color());
}

#[test]
fn first_attachment_traverses_and_repeated_clean_flushes_skip() {
    let mut doc = document();
    assert_clean(&mut doc);

    let root = doc.create_element("page", ());
    doc.append_document_element(root);
    assert_flush(&mut doc, FlushStatus::Traversed);
    assert!(doc.get(root).unwrap().computed_style().is_some());

    for _ in 0..3 {
        assert_clean(&mut doc);
    }
}

#[test]
fn dom_stylesheet_and_device_mutations_rearm_clean_style_flushes() {
    let mut doc = document();
    doc.add_stylesheet(".hot { color: rgb(255, 0, 0); }", StylesheetOrigin::Author);
    let root = doc.create_element("page", ());
    let target = doc.create_element("view", ());
    doc.set_classes(target, "hot");
    doc.append_child(root, target);
    doc.append_document_element(root);

    assert_restyle_color(&mut doc, target, RED);
    assert_clean(&mut doc);

    doc.set_inline_style(target, "color: rgb(0, 0, 255)");
    assert_restyle_color(&mut doc, target, BLUE);
    assert_clean(&mut doc);

    doc.add_stylesheet(
        ".hot { color: rgb(0, 128, 0) !important; }",
        StylesheetOrigin::Author,
    );
    assert_restyle_color(&mut doc, target, GREEN);
    assert_clean(&mut doc);

    doc.add_stylesheet_with_media(
        ".hot { color: rgb(1, 2, 3) !important; }",
        StylesheetOrigin::Author,
        "(max-width: 500px)",
    );
    assert_flush(&mut doc, FlushStatus::Traversed);
    assert_clean(&mut doc);

    doc.set_viewport(400.0, 600.0);
    assert_restyle_color(&mut doc, target, [1, 2, 3]);
    assert_clean(&mut doc);
}

#[test]
fn documents_own_independent_stylesheets() {
    let mut first = document();
    let mut second = document();
    first.add_stylesheet(".probe { color: red; }", StylesheetOrigin::Author);

    let first_probe = first.create_element("view", ());
    first.add_class(first_probe, "probe");
    let second_probe = second.create_element("view", ());
    second.add_class(second_probe, "probe");

    let first_style = first.resolve_style(first.get(first_probe).unwrap(), None);
    let second_style = second.resolve_style(second.get(second_probe).unwrap(), None);
    assert_eq!(first_style.clone_color(), rgb(RED));
    assert_ne!(second_style.clone_color(), first_style.clone_color());
}

#[test]
#[should_panic(expected = "CSS rule belongs to another Document")]
fn prebuilt_rules_cannot_cross_document_contexts() {
    let first = document();
    let mut second = document();
    let rule = first
        .build_style_rule(
            ".probe",
            [w3c_dom::CssDeclaration {
                property: "color",
                value: "red".into(),
                important: false,
            }],
        )
        .unwrap();
    second.append_rules(vec![rule], StylesheetOrigin::Author);
}
