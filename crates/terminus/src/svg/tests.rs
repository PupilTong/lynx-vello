//! The markup builder (`build.rs`), its markup (`tree/xml`) and the `use`
//! instances (`use_element.rs`) over a document: these are crate-private
//! until the image pipeline calls them, so their tests live here rather
//! than in `tests/svg_dom.rs`.

use crate::svg::image::{SvgError, VectorDocument};
use crate::test_common::Doc;
use crate::tree::document::NodeId;
use crate::tree::xml::{XmlMarkup, XmlNamespace};

const SVG_NS: &str = "@namespace svg url(http://www.w3.org/2000/svg);";

const PAGE: &str = "page { display: flex; align-items: flex-start; width: 400px; height: 300px; }";

fn size(doc: &Doc, id: NodeId) -> (f32, f32) {
    let layout = doc.dom.rounded_layout(id).expect("laid out");
    (layout.size.width, layout.size.height)
}

// ---------------------------------------------------------------------------
// Markup to DOM

const SVG_OPEN: &str =
    r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">"#;

fn markup(body: &str) -> XmlMarkup {
    XmlMarkup::parse(
        format!("{SVG_OPEN}{body}</svg>").as_bytes(),
        Some(XmlNamespace::Svg),
    )
    .expect("the markup parses")
}

/// Builds `body` inside an `svg` root under the document element.
fn build(doc: &mut Doc, body: &str) -> NodeId {
    let markup = markup(body);
    doc.dom.append_svg_markup(doc.root, &markup, || ())
}

fn by_id(doc: &Doc, id: &str) -> NodeId {
    doc.dom
        .query_selector(doc.root, &format!("#{id}"))
        .expect("parses")
        .expect("an element with that id")
}

/// The element children of the `use` element `id`'s instance.
fn instance(doc: &Doc, id: &str) -> Vec<NodeId> {
    let shadow = doc
        .dom
        .shadow_root(by_id(doc, id))
        .expect("the use has an instance");
    doc.dom.get(shadow).expect("live").child_ids().to_vec()
}

#[test]
fn markup_parse_errors() {
    assert!(matches!(
        VectorDocument::parse(b"<html/>"),
        Err(SvgError::NotSvg)
    ));
    assert!(matches!(
        VectorDocument::parse(br#"<x:svg xmlns:x="urn:x"/>"#),
        Err(SvgError::NotSvg)
    ));
}

#[test]
fn markup_without_a_namespace_builds_svg_elements() {
    let mut doc = Doc::new();
    let markup = XmlMarkup::parse(
        br#"<svg viewBox="0 0 4 4"><path d="M0 0h4"/></svg>"#,
        Some(XmlNamespace::Svg),
    )
    .expect("markup without xmlns parses");
    let svg = doc.dom.append_svg_markup(doc.root, &markup, || ());
    let node = |id| doc.dom.get(id).expect("live");
    let path = node(svg).child_ids()[0];
    assert!(node(svg).is_svg_element());
    assert_eq!(node(path).tag_name(), Some("path"));
    assert!(node(path).is_svg_element());
}

#[test]
fn a_foreign_namespace_subtree_is_dropped() {
    let mut doc = Doc::new();
    let markup = XmlMarkup::parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg"><foo:bar xmlns:foo="urn:x"><path/></foo:bar></svg>"#,
Some(XmlNamespace::Svg))
    .expect("parses");
    let svg = doc.dom.append_svg_markup(doc.root, &markup, || ());
    assert_eq!(doc.dom.get(svg).expect("live").child_ids(), []);
}

#[test]
fn built_elements_are_svg_with_their_case_and_hints() {
    let mut doc = Doc::with_css(&format!(
        "{SVG_NS} svg|path.a {{ stroke: rgb(0, 0, 255); }} #b {{ opacity: 0.5; }}"
    ));
    let svg = build(
        &mut doc,
        r#"<linearGradient id="g"/><path id="b" class="a" fill="red"/>"#,
    );
    doc.flush();
    let gradient = by_id(&doc, "g");
    let path = by_id(&doc, "b");
    let node = |id| doc.dom.get(id).expect("live");
    assert!(node(svg).is_svg_element());
    assert_eq!(node(gradient).tag_name(), Some("linearGradient"));
    assert!(node(gradient).is_svg_element());
    assert_eq!(doc.value(path, "fill"), "rgb(255, 0, 0)");
    assert_eq!(doc.value(path, "stroke"), "rgb(0, 0, 255)");
    assert_eq!(doc.value(path, "opacity"), "0.5");
    assert_eq!(node(svg).parent_id(), Some(doc.root));
}

#[test]
fn style_elements_are_document_sheets() {
    let mut doc = Doc::new();
    build(
        &mut doc,
        r#"<style>.a { fill: rgb(0, 0, 255); }</style>
           <style type="text/x-other">.b { fill: rgb(0, 255, 0); }</style>
           <path id="a" class="a" fill="red"/><path id="b" class="b"/>"#,
    );
    doc.flush();
    assert_eq!(
        doc.value(by_id(&doc, "a"), "fill"),
        "rgb(0, 0, 255)",
        "the author rule beats the presentation attribute"
    );
    assert_eq!(
        doc.value(by_id(&doc, "b"), "fill"),
        "rgb(0, 0, 0)",
        "a style of another type contributes nothing"
    );
}

#[test]
fn a_use_instance_inherits_from_the_use_and_takes_document_rules() {
    let mut doc = Doc::with_css(&format!("{SVG_NS} svg|path {{ stroke: rgb(255, 0, 0); }}"));
    build(
        &mut doc,
        r##"<defs><path id="p" d="M0 0 L1 1"/></defs>
            <use id="u" fill="rgb(0, 128, 0)" href="#p"/>
            <use id="x" xlink:href="#p"/>"##,
    );
    doc.flush();
    let clones = instance(&doc, "u");
    assert_eq!(clones.len(), 1);
    let clone = clones[0];
    let node = doc.dom.get(clone).expect("live");
    assert_eq!(node.tag_name(), Some("path"));
    assert!(node.is_svg_element());
    assert_eq!(doc.value(clone, "fill"), "rgb(0, 128, 0)");
    assert_eq!(doc.value(clone, "stroke"), "rgb(255, 0, 0)");
    assert_eq!(
        by_id(&doc, "p"),
        doc.dom
            .query_selector(doc.root, "defs > path")
            .unwrap()
            .unwrap(),
        "the clone's id does not shadow the original"
    );
    assert_eq!(instance(&doc, "x").len(), 1, "xlink:href works");
}

/// A `use` resolves through `xlink:href`, kept under its own name, and
/// `href` wins when both are present (SVG 2).
#[test]
fn href_takes_precedence_over_xlink_href() {
    let mut doc = Doc::new();
    build(
        &mut doc,
        r##"<defs><path id="p"/><rect id="q"/></defs>
            <use id="x" xlink:href="#p"/>
            <use id="both" xlink:href="#p" href="#q"/>"##,
    );
    doc.flush();
    let tag = |id| {
        let clone = instance(&doc, id)[0];
        doc.dom
            .get(clone)
            .expect("live")
            .tag_name()
            .map(str::to_owned)
    };
    assert_eq!(
        doc.dom
            .get(by_id(&doc, "x"))
            .expect("live")
            .attribute("xlink:href")
            .as_deref(),
        Some("#p")
    );
    assert_eq!(tag("x").as_deref(), Some("path"));
    assert_eq!(tag("both").as_deref(), Some("rect"));
}

#[test]
fn reference_cycles_instantiate_nothing() {
    let mut doc = Doc::new();
    build(
        &mut doc,
        r##"<use id="self" href="#self"/>
            <use id="a" href="#b"/><use id="b" href="#a"/>
            <g id="outer"><use id="inside" href="#outer"/></g>"##,
    );
    doc.flush();
    for id in ["self", "a", "b", "inside"] {
        assert!(doc.dom.shadow_root(by_id(&doc, id)).is_none(), "{id}");
    }
}

#[test]
fn a_use_of_a_group_holding_a_use_nests() {
    let mut doc = Doc::new();
    build(
        &mut doc,
        r##"<defs><rect id="r"/><g id="g"><use href="#r"/></g></defs>
            <use id="outer" href="#g"/>"##,
    );
    doc.flush();
    let group = instance(&doc, "outer")[0];
    let inner_use = doc.dom.get(group).expect("live").child_ids()[0];
    let inner = doc
        .dom
        .shadow_root(inner_use)
        .expect("the inner use has an instance");
    let rect = doc.dom.get(inner).expect("live").child_ids()[0];
    assert_eq!(doc.dom.get(rect).expect("live").tag_name(), Some("rect"));
}

#[test]
fn a_built_root_lays_out_at_its_natural_size() {
    let mut doc = Doc::with_css(PAGE);
    let markup = XmlMarkup::parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="20">
            <g><rect id="r" width="5" height="5"/></g></svg>"#,
        Some(XmlNamespace::Svg),
    )
    .expect("parses");
    let svg = doc.dom.append_svg_markup(doc.root, &markup, || ());
    doc.flush();
    assert_eq!(size(&doc, svg), (30.0, 20.0));
    let rect = by_id(&doc, "r");
    assert!(
        doc.dom
            .rounded_layout(rect)
            .is_none_or(|layout| layout.size.width == 0.0 && layout.size.height == 0.0)
    );
}
