//! Direct tests for the embedder-neutral style engine.

use euclid::{Scale, Size2D};
use stylo::color::AbsoluteColor;
use stylo::device::Device;
use stylo::device::servo::FontMetricsProvider;
use stylo::font_metrics::FontMetrics;
use stylo::media_queries::MediaType;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::queries::values::PrefersColorScheme;
use stylo::servo::media_features::PointerCapabilities;
use stylo::values::computed::font::GenericFontFamily;
use stylo::values::computed::{CSSPixelLength, Length};
use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
use stylo_traits::{CSSPixel, DevicePixel};
use w3c_dom::{Document, StylesheetOrigin};

fn device(width: f32, height: f32) -> Device {
    Device::new(
        MediaType::screen(),
        stylo::context::QuirksMode::NoQuirks,
        Size2D::<f32, CSSPixel>::new(width, height),
        Size2D::<f32, DevicePixel>::new(width, height),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(TestFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

#[derive(Debug)]
struct TestFontMetricsProvider;

impl FontMetricsProvider for TestFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        base_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics {
            ascent: Length::new(base_size.px()),
            ..FontMetrics::default()
        }
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        Length::new(FONT_MEDIUM_PX)
    }
}

#[test]
fn standard_cascade_is_embedder_neutral() {
    let mut doc: Document<()> = Document::new(device(800.0, 600.0));
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
    assert_eq!(
        parent_style.clone_color(),
        AbsoluteColor::srgb_legacy(0, 128, 0, 1.0)
    );

    doc.set_inline_style(child, "color: blue");
    let child_style = doc.resolve_style(doc.get(child).unwrap(), Some(parent_style.as_ref()));
    assert_eq!(
        child_style.clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "standard inline declarations outrank author class rules"
    );
}

#[test]
fn id_class_and_style_attributes_are_reflected_dom_state() {
    let mut doc: Document<()> = Document::new(device(800.0, 600.0));
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
    let red = AbsoluteColor::srgb_legacy(255, 0, 0, 1.0);
    assert_eq!(node.computed_style().unwrap().clone_color(), red);

    doc.remove_attribute(target, "class");
    doc.flush_styles();
    let node = doc.get(target).unwrap();
    assert!(!node.has_class("hot"));
    assert_eq!(node.attribute("class"), None);
    assert_ne!(node.computed_style().unwrap().clone_color(), red);

    doc.set_attribute(target, "class", "hot");
    doc.flush_styles();
    assert_eq!(
        doc.get(target)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        red
    );

    doc.remove_attribute(target, "style");
    doc.flush_styles();
    assert_ne!(
        doc.get(target)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        red
    );

    doc.set_attribute(target, "style", "width: 20px");
    doc.flush_styles();
    assert_eq!(
        doc.get(target)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        red
    );

    doc.remove_attribute(target, "id");
    doc.flush_styles();
    assert_ne!(
        doc.get(target)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        red
    );
}

#[test]
fn style_traversal_skips_text_nodes_and_reaches_element_siblings() {
    let mut doc: Document<()> = Document::new(device(800.0, 600.0));
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
    assert_eq!(
        doc.get(span)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );
}

#[test]
fn text_data_changes_invalidate_the_parent_empty_selector() {
    let mut doc: Document<()> = Document::new(device(800.0, 600.0));
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
    assert_eq!(
        doc.get(box_element)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0),
        "an empty text node preserves :empty"
    );

    doc.set_text_node_data(text, "hello");
    doc.flush_styles();
    assert_eq!(
        doc.get(box_element)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "non-empty text makes the parent fail :empty"
    );

    doc.set_text_node_data(text, "");
    doc.flush_styles();
    assert_eq!(
        doc.get(box_element)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0),
        "clearing text restores :empty"
    );

    doc.detach(text);
    doc.flush_styles();
    doc.set_text_node_data(text, "reattached");
    doc.append_child(box_element, text);
    doc.flush_styles();
    assert_eq!(
        doc.get(box_element)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "inserting non-empty text clears :empty"
    );

    doc.detach(text);
    doc.flush_styles();
    assert_eq!(
        doc.get(box_element)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0),
        "removing the only non-empty text restores :empty"
    );
}

#[test]
fn edge_child_selectors_ignore_interleaved_text_nodes_during_restyle() {
    let mut doc: Document<()> = Document::new(device(800.0, 600.0));
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

    let color =
        |doc: &Document<()>, id| doc.get(id).unwrap().computed_style().unwrap().clone_color();
    assert_eq!(
        color(&doc, first),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );
    assert_eq!(
        color(&doc, last),
        AbsoluteColor::srgb_legacy(0, 128, 0, 1.0)
    );

    let new_first = doc.create_element("view", ());
    doc.add_class(new_first, "item");
    doc.insert_before(root, new_first, Some(first));
    doc.flush_styles();
    assert_eq!(
        color(&doc, new_first),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );
    assert_eq!(
        color(&doc, first),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "the displaced first element must lose :first-child"
    );

    let new_last = doc.create_element("view", ());
    doc.add_class(new_last, "item");
    doc.append_child(root, new_last);
    doc.flush_styles();
    assert_eq!(
        color(&doc, new_last),
        AbsoluteColor::srgb_legacy(0, 128, 0, 1.0)
    );
    assert_eq!(
        color(&doc, last),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "the displaced last element must lose :last-child"
    );
}

#[test]
fn media_queries_follow_standard_viewport_updates() {
    let mut doc: Document<()> = Document::new(device(800.0, 600.0));
    doc.add_stylesheet_with_media(
        ".box { color: red; }",
        StylesheetOrigin::Author,
        "(min-width: 600px)",
    );

    let element = doc.create_element("div", ());
    doc.add_class(element, "box");

    let wide = doc.resolve_style(doc.get(element).unwrap(), None);
    assert_eq!(
        wide.clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );

    doc.set_viewport(400.0, 600.0);
    let narrow = doc.resolve_style(doc.get(element).unwrap(), None);
    assert_ne!(narrow.clone_color(), wide.clone_color());
}

#[test]
fn documents_own_independent_stylesheets() {
    let mut first: Document<()> = Document::new(device(800.0, 600.0));
    let mut second: Document<()> = Document::new(device(800.0, 600.0));
    first.add_stylesheet(".probe { color: red; }", StylesheetOrigin::Author);

    let first_probe = first.create_element("view", ());
    first.add_class(first_probe, "probe");
    let second_probe = second.create_element("view", ());
    second.add_class(second_probe, "probe");

    let first_style = first.resolve_style(first.get(first_probe).unwrap(), None);
    let second_style = second.resolve_style(second.get(second_probe).unwrap(), None);
    assert_eq!(
        first_style.clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );
    assert_ne!(second_style.clone_color(), first_style.clone_color());
}

#[test]
#[should_panic(expected = "CSS rule belongs to another Document")]
fn prebuilt_rules_cannot_cross_document_contexts() {
    let first: Document<()> = Document::new(device(800.0, 600.0));
    let mut second: Document<()> = Document::new(device(800.0, 600.0));
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
