//! SVG-namespace elements: the namespace, presentation attributes reflected
//! into the cascade, the `svg` element's replaced box and the SVG
//! user-agent sheet (`crates/terminus/src/svg/`).

#![allow(clippy::float_cmp)]

mod common;

use common::Doc;
use terminus::link::{CustomElement, Document, MethodCall, MethodOutcome, NodeId};

const SVG_NS: &str = "@namespace svg url(http://www.w3.org/2000/svg);";

/// A definition whose one method answers `Done`, so a test can tell an
/// element that took it from one that did not.
struct Plain;

impl CustomElement<()> for Plain {
    fn invoke(
        &self,
        _document: &mut Document<()>,
        _element: NodeId,
        _call: MethodCall<'_>,
    ) -> MethodOutcome {
        MethodOutcome::Done
    }
}

fn took_definition(doc: &mut Doc, id: NodeId) -> bool {
    let call = MethodCall {
        name: "probe",
        params: "",
    };
    doc.dom.invoke_element_method(id, call) == MethodOutcome::Done
}

fn svg_child(doc: &mut Doc, parent: NodeId, tag: &str) -> NodeId {
    let id = doc.dom.create_svg_element(tag, ());
    doc.dom.append_child(parent, id);
    id
}

fn svg_root(doc: &mut Doc) -> NodeId {
    let svg = doc.dom.create_element("svg", ());
    doc.dom.append_child(doc.root, svg);
    svg
}

// ---------------------------------------------------------------------------
// Namespace

#[test]
fn create_element_makes_only_svg_an_svg_element() {
    let mut doc = Doc::new();
    let svg = doc.dom.create_element("svg", ());
    let html_path = doc.dom.create_element("path", ());
    let svg_path = doc.dom.create_svg_element("path", ());
    let node = |id| doc.dom.get(id).expect("live");
    assert!(node(svg).is_svg_element());
    assert!(!node(html_path).is_svg_element());
    assert!(node(svg_path).is_svg_element());
}

#[test]
fn namespace_selectors_match_the_svg_element_only() {
    let mut doc = Doc::with_css(&format!(
        "{SVG_NS} svg|path {{ opacity: 0.5; }} |path {{ opacity: 0.25; }}"
    ));
    let html_path = doc.el(doc.root, "path");
    let svg = svg_root(&mut doc);
    let svg_path = svg_child(&mut doc, svg, "path");
    doc.flush();
    assert_eq!(doc.value(svg_path, "opacity"), "0.5");
    assert_eq!(doc.value(html_path, "opacity"), "1");
}

#[test]
fn svg_local_names_match_case_sensitively() {
    let mut doc = Doc::with_css(
        "linearGradient { opacity: 0.5; } lineargradient { opacity: 0.25; }
         [viewBox] { opacity: 0.75; }",
    );
    let svg = svg_root(&mut doc);
    doc.dom.set_attribute(svg, "viewBox", "0 0 10 10");
    let gradient = svg_child(&mut doc, svg, "linearGradient");
    let lower = svg_child(&mut doc, svg, "lineargradient");
    doc.flush();
    assert_eq!(doc.value(gradient, "opacity"), "0.5");
    assert_eq!(doc.value(lower, "opacity"), "0.25");
    assert_eq!(doc.value(svg, "opacity"), "0.75");
}

#[test]
#[should_panic(expected = "SVG namespace's `svg` element")]
fn defining_svg_panics() {
    let mut doc = Doc::new();
    doc.dom.define("svg", Box::new(Plain));
}

#[test]
fn an_svg_text_takes_no_definition_for_text() {
    let mut doc = Doc::new();
    // An SVG `text` made before the definition does not count as an
    // element the definition missed.
    let early = doc.dom.create_svg_element("text", ());
    doc.dom.define("text", Box::new(Plain));
    let svg = svg_root(&mut doc);
    doc.dom.append_child(svg, early);
    let late = svg_child(&mut doc, svg, "text");
    let html_text = doc.el(doc.root, "text");
    assert!(!took_definition(&mut doc, early));
    assert!(!took_definition(&mut doc, late));
    assert!(took_definition(&mut doc, html_text));
}

// ---------------------------------------------------------------------------
// Presentation attributes

#[test]
fn presentation_attributes_sit_between_ua_and_author_rules() {
    let mut doc = Doc::new();
    doc.add_ua_css(&format!("{SVG_NS} svg|path {{ fill: rgb(1, 1, 1); }}"));
    let svg = svg_root(&mut doc);
    let path = svg_child(&mut doc, svg, "path");
    doc.flush();
    assert_eq!(doc.value(path, "fill"), "rgb(1, 1, 1)");

    doc.dom.set_attribute(path, "fill", "rgb(2, 2, 2)");
    doc.flush();
    assert_eq!(
        doc.value(path, "fill"),
        "rgb(2, 2, 2)",
        "the attribute beats the UA rule"
    );

    doc.add_css(&format!("{SVG_NS} svg|path {{ fill: rgb(3, 3, 3); }}"));
    doc.flush();
    assert_eq!(
        doc.value(path, "fill"),
        "rgb(3, 3, 3)",
        "an author rule beats the attribute"
    );

    doc.dom.set_inline_style(path, "fill: rgb(4, 4, 4)");
    doc.flush();
    assert_eq!(
        doc.value(path, "fill"),
        "rgb(4, 4, 4)",
        "the style attribute beats both"
    );
}

#[test]
fn unitless_lengths_compute_to_px() {
    let mut doc = Doc::new();
    let svg = svg_root(&mut doc);
    let circle = svg_child(&mut doc, svg, "circle");
    for (name, value) in [
        ("stroke-width", "2"),
        ("font-size", "12"),
        ("r", "5"),
        ("stroke-dasharray", "4 2"),
    ] {
        doc.dom.set_attribute(circle, name, value);
    }
    doc.flush();
    assert_eq!(doc.value(circle, "stroke-width"), "2px");
    assert_eq!(doc.value(circle, "font-size"), "12px");
    assert_eq!(doc.value(circle, "r"), "5px");
    assert_eq!(doc.value(circle, "stroke-dasharray"), "4px, 2px");
}

#[test]
fn paint_attributes_compute_to_their_variants() {
    let mut doc = Doc::new();
    let svg = svg_root(&mut doc);
    let current = svg_child(&mut doc, svg, "path");
    let server = svg_child(&mut doc, svg, "path");
    doc.dom.set_attribute(current, "fill", "currentColor");
    doc.dom.set_attribute(current, "color", "rgb(9, 9, 9)");
    doc.dom.set_attribute(server, "fill", "url(#g) red");
    doc.dom.set_attribute(server, "clip-path", "url(#c)");
    doc.flush();
    // The serialization resolves `currentcolor` against `color`.
    assert_eq!(doc.value(current, "fill"), "rgb(9, 9, 9)");
    assert_eq!(
        doc.value(server, "fill"),
        "url(\"about:blank#g\") rgb(255, 0, 0)"
    );
    assert_eq!(doc.value(server, "clip-path"), "url(\"about:blank#c\")");
    doc.dom.set_attribute(current, "color", "rgb(8, 8, 8)");
    doc.flush();
    assert_eq!(
        doc.value(current, "fill"),
        "rgb(8, 8, 8)",
        "the fill follows `color`"
    );
}

#[test]
fn an_invalid_or_removed_attribute_leaves_no_hint() {
    let mut doc = Doc::new();
    let svg = svg_root(&mut doc);
    let path = svg_child(&mut doc, svg, "path");
    doc.dom.set_attribute(path, "stroke-width", "3");
    doc.flush();
    assert_eq!(doc.value(path, "stroke-width"), "3px");

    doc.dom.set_attribute(path, "stroke-width", "three");
    doc.flush();
    assert_eq!(
        doc.value(path, "stroke-width"),
        "1px",
        "an invalid value is no hint"
    );

    doc.dom.set_attribute(path, "stroke-width", "4");
    doc.flush();
    assert_eq!(doc.value(path, "stroke-width"), "4px");
    doc.dom.remove_attribute(path, "stroke-width");
    doc.flush();
    assert_eq!(
        doc.value(path, "stroke-width"),
        "1px",
        "a removed attribute is no hint"
    );
}

#[test]
fn geometry_attributes_map_on_their_elements_only() {
    let mut doc = Doc::new();
    let svg = svg_root(&mut doc);
    let rect = svg_child(&mut doc, svg, "rect");
    let text = svg_child(&mut doc, svg, "text");
    doc.dom.set_attribute(rect, "x", "7");
    doc.dom.set_attribute(rect, "rx", "3");
    doc.dom.set_attribute(text, "x", "7");
    doc.flush();
    assert_eq!(doc.value(rect, "x"), "7px");
    assert_eq!(doc.value(rect, "rx"), "3px");
    assert_eq!(
        doc.value(text, "x"),
        "0px",
        "`x` on `text` is an attribute only"
    );
}

#[test]
fn an_html_element_reflects_nothing() {
    let mut doc = Doc::new();
    let path = doc.el(doc.root, "path[fill=red][stroke-width=2]");
    doc.flush();
    assert_eq!(doc.value(path, "stroke-width"), "1px");
    assert_eq!(doc.value(path, "fill"), "rgb(0, 0, 0)");
}

#[test]
fn transform_attribute_computes_to_its_matrix() {
    let mut doc = Doc::new();
    let svg = svg_root(&mut doc);
    let g = svg_child(&mut doc, svg, "g");
    doc.dom
        .set_attribute(g, "transform", "translate(10 20) rotate(45 5 5)");
    doc.flush();
    // translate(10 20) · translate(5 5) · rotate(45°) · translate(-5 -5).
    let (sin, cos) = 45f64.to_radians().sin_cos();
    let e = 10.0 + 5.0 - 5.0 * cos + 5.0 * sin;
    let f = 20.0 + 5.0 - 5.0 * sin - 5.0 * cos;
    let value = doc.value(g, "transform");
    let numbers: Vec<f64> = value
        .trim_start_matches("matrix(")
        .trim_end_matches(')')
        .split(", ")
        .map(|number| number.parse().expect("a matrix entry"))
        .collect();
    let expected = [cos, sin, -sin, cos, e, f];
    assert_eq!(numbers.len(), 6, "{value}");
    for (got, want) in numbers.iter().zip(expected) {
        assert!((got - want).abs() < 1e-4, "{value} against {expected:?}");
    }

    doc.dom.set_attribute(g, "transform", "translate(10");
    doc.flush();
    assert_eq!(
        doc.value(g, "transform"),
        "none",
        "an invalid list is no hint"
    );
}

#[test]
fn d_attribute_keeps_the_path_up_to_its_first_error() {
    let mut doc = Doc::new();
    let svg = svg_root(&mut doc);
    let path = svg_child(&mut doc, svg, "path");
    let g = svg_child(&mut doc, svg, "g");
    doc.dom.set_attribute(path, "d", "M0 0 L10 0 L10 10 x");
    doc.dom.set_attribute(g, "d", "M0 0 L10 0");
    doc.flush();
    assert_eq!(doc.value(path, "d"), "path(\"M 0 0 L 10 0 L 10 10\")");
    assert_eq!(doc.value(g, "d"), "none", "`d` maps on `path` only");

    doc.dom.set_attribute(path, "d", "x");
    doc.flush();
    assert_eq!(doc.value(path, "d"), "none", "no command parsed is no hint");
}

#[test]
fn fill_inherits_into_a_child_and_opacity_does_not() {
    let mut doc = Doc::new();
    let svg = svg_root(&mut doc);
    let g = svg_child(&mut doc, svg, "g");
    let path = svg_child(&mut doc, g, "path");
    doc.dom.set_attribute(g, "fill", "rgb(5, 6, 7)");
    doc.dom.set_attribute(g, "opacity", "0.5");
    doc.flush();
    assert_eq!(doc.value(path, "fill"), "rgb(5, 6, 7)");
    assert_eq!(doc.value(g, "opacity"), "0.5");
    assert_eq!(doc.value(path, "opacity"), "1");
}

// ---------------------------------------------------------------------------
// The `svg` element's box

const PAGE: &str = "page { display: flex; align-items: flex-start; width: 400px; height: 300px; }";

fn size(doc: &Doc, id: NodeId) -> (f32, f32) {
    let layout = doc.dom.rounded_layout(id).expect("laid out");
    (layout.size.width, layout.size.height)
}

#[test]
fn root_box_follows_its_attributes() {
    let mut doc = Doc::with_css(PAGE);
    let unsized_box = svg_root(&mut doc);
    doc.dom.set_attribute(unsized_box, "viewBox", "0 0 40 20");
    let icon = svg_root(&mut doc);
    doc.dom.set_attribute(icon, "width", "24");
    doc.dom.set_attribute(icon, "height", "24");
    let bare = svg_root(&mut doc);
    doc.flush();
    assert_eq!(size(&doc, unsized_box), (300.0, 150.0));
    assert_eq!(size(&doc, icon), (24.0, 24.0));
    assert_eq!(size(&doc, bare), (300.0, 150.0));

    doc.dom.set_attribute(icon, "width", "48");
    doc.dom.remove_attribute(icon, "height");
    doc.flush();
    assert_eq!(size(&doc, icon), (48.0, 150.0), "a width change relayouts");
}

#[test]
fn root_children_have_no_box() {
    let mut doc = Doc::with_css(&format!(
        "{PAGE} rect {{ display: flex; width: 50px; height: 50px; }}"
    ));
    let svg = svg_root(&mut doc);
    doc.dom.set_attribute(svg, "width", "24");
    doc.dom.set_attribute(svg, "height", "24");
    let rect = svg_child(&mut doc, svg, "rect");
    doc.flush();
    assert_eq!(size(&doc, svg), (24.0, 24.0));
    assert!(
        doc.dom
            .rounded_layout(rect)
            .is_none_or(|layout| layout.size.width == 0.0 && layout.size.height == 0.0),
        "a child of the root gets no box"
    );
}

#[test]
fn percentage_width_fills_the_container() {
    let mut doc = Doc::with_css(PAGE);
    let svg = svg_root(&mut doc);
    doc.dom.set_attribute(svg, "width", "100%");
    doc.flush();
    assert_eq!(size(&doc, svg), (400.0, 150.0));
}

// ---------------------------------------------------------------------------
// The SVG user-agent sheet

#[test]
fn ua_sheet_sets_transform_origin_on_svg_content() {
    let mut doc = Doc::new();
    let svg = svg_root(&mut doc);
    let g = svg_child(&mut doc, svg, "g");
    let html = doc.el(doc.root, "g");
    doc.flush();
    let before = doc.value(g, "transform-origin");
    assert_ne!(before, "0px 0px 0px", "no sheet, the CSS initial value");

    doc.dom.add_svg_user_agent_stylesheet();
    doc.flush();
    assert_eq!(doc.value(g, "transform-origin"), "0px 0px 0px");
    assert_eq!(
        doc.value(svg, "transform-origin"),
        before,
        "the outermost svg keeps CSS's"
    );
    assert_eq!(
        doc.value(html, "transform-origin"),
        before,
        "an HTML element keeps CSS's"
    );
}

/// Absolute units parse in a presentation attribute, the one place the
/// `lynx` grammar takes them; the `style` attribute keeps refusing them.
#[test]
fn absolute_units_are_lengths_in_presentation_attributes_only() {
    let mut doc = Doc::new();
    let svg = svg_root(&mut doc);
    let rect = svg_child(&mut doc, svg, "rect");
    doc.dom.set_attribute(rect, "font-size", "12pt");
    doc.dom.set_attribute(rect, "stroke-width", "1pt");
    doc.dom.set_attribute(rect, "width", "10");
    doc.dom.set_inline_style(rect, "width: 1in");
    doc.flush();
    assert_eq!(doc.value(rect, "font-size"), "16px");
    let width: f64 = doc
        .value(rect, "stroke-width")
        .trim_end_matches("px")
        .parse()
        .expect("a px length");
    assert!(
        (width - 4.0 / 3.0).abs() < 1e-4,
        "1pt is 4/3 px, got {width}"
    );
    assert_eq!(
        doc.value(rect, "width"),
        "10px",
        "`width: 1in` in the style attribute is no declaration"
    );
}
