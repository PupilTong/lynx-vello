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
use w3c_dom::{Document, StyleEngine, StylesheetOrigin};

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
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_str(
        ".parent { color: green; } .child { color: red; }",
        StylesheetOrigin::Author,
    );

    let mut doc: Document<()> = engine.new_document();
    let parent = doc.create_node("section", ());
    let child = doc.create_node("span", ());
    doc.add_class(parent, "parent");
    doc.add_class(child, "child");
    doc.append(parent, child);

    let parent_style = engine.resolve(doc.get(parent).unwrap(), None);
    assert_eq!(
        parent_style.clone_color(),
        AbsoluteColor::srgb_legacy(0, 128, 0, 1.0)
    );

    doc.set_inline_style(child, "color: blue");
    let child_style = engine.resolve(doc.get(child).unwrap(), Some(parent_style.as_ref()));
    assert_eq!(
        child_style.clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "standard inline declarations outrank author class rules"
    );
}

#[test]
fn style_traversal_skips_text_nodes_and_reaches_element_siblings() {
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_str("span { color: red; }", StylesheetOrigin::Author);

    let mut doc: Document<()> = engine.new_document();
    let root = doc.create_element("page", ());
    let text = doc.create_text_node("hello", ());
    let span = doc.create_element("span", ());
    doc.append(root, text);
    doc.append(root, span);
    doc.set_root(root);

    engine.flush_document(&mut doc);

    assert!(doc.get(root).unwrap().has_style_data());
    assert!(
        !doc.get(text).unwrap().has_style_data(),
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
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_str(
        ".box { color: blue; } .box:empty { color: red; }",
        StylesheetOrigin::Author,
    );

    let mut doc: Document<()> = engine.new_document();
    let root = doc.create_element("page", ());
    let box_element = doc.create_element("view", ());
    let text = doc.create_text_node("", ());
    doc.add_class(box_element, "box");
    doc.append(box_element, text);
    doc.append(root, box_element);
    doc.set_root(root);

    engine.flush_document(&mut doc);
    assert_eq!(
        doc.get(box_element)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0),
        "an empty text node preserves :empty"
    );

    doc.set_text_data(text, "hello");
    assert!(doc.needs_flush());
    engine.flush_document(&mut doc);
    assert_eq!(
        doc.get(box_element)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "non-empty text makes the parent fail :empty"
    );

    doc.set_text_data(text, "");
    engine.flush_document(&mut doc);
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
    engine.flush_document(&mut doc);
    doc.set_text_data(text, "reattached");
    assert!(
        !doc.needs_flush(),
        "mutating detached text cannot affect a styled element"
    );
    doc.append(box_element, text);
    engine.flush_document(&mut doc);
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
    engine.flush_document(&mut doc);
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
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_str(
        ".item { color: blue; } .item:first-child { color: red; } \
         .item:last-child { color: green; }",
        StylesheetOrigin::Author,
    );

    let mut doc: Document<()> = engine.new_document();
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
        doc.append(root, child);
    }
    doc.set_root(root);
    engine.flush_document(&mut doc);

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
    engine.flush_document(&mut doc);
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
    doc.append(root, new_last);
    engine.flush_document(&mut doc);
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
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_with_media(
        ".box { color: red; }",
        StylesheetOrigin::Author,
        "(min-width: 600px)",
    );

    let mut doc: Document<()> = engine.new_document();
    let element = doc.create_node("div", ());
    doc.add_class(element, "box");

    let wide = engine.resolve(doc.get(element).unwrap(), None);
    assert_eq!(
        wide.clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );

    engine.set_viewport(400.0, 600.0);
    let narrow = engine.resolve(doc.get(element).unwrap(), None);
    assert_ne!(narrow.clone_color(), wide.clone_color());
}

#[test]
#[should_panic(expected = "not paired with this StyleEngine")]
fn flushing_a_foreign_document_crashes_at_the_boundary() {
    let engine_a = StyleEngine::new(device(800.0, 600.0));
    let engine_b = StyleEngine::new(device(800.0, 600.0));
    let mut doc: Document<()> = engine_a.new_document();
    let root = doc.create_node("page", ());
    doc.set_root(root);
    // Without the boundary check this dies deep inside stylo
    // ("Locked::read_with called with a guard from an unrelated
    // SharedRwLock") — or silently cascades against the wrong stylist when
    // no inline styles exist.
    engine_b.flush_document(&mut doc);
}

#[test]
#[should_panic(expected = "not paired with this StyleEngine")]
fn resolving_through_a_foreign_engine_crashes() {
    let engine_a = StyleEngine::new(device(800.0, 600.0));
    let engine_b = StyleEngine::new(device(800.0, 600.0));
    let mut doc: Document<()> = engine_a.new_document();
    let el = doc.create_node("view", ());
    let _ = engine_b.resolve(doc.get(el).unwrap(), None);
}

#[test]
#[should_panic(expected = "not paired with this StyleEngine")]
fn standalone_documents_cannot_be_flushed() {
    let engine = StyleEngine::new(device(800.0, 600.0));
    let mut doc: Document<()> = Document::new();
    let root = doc.create_node("page", ());
    doc.set_root(root);
    engine.flush_document(&mut doc);
}
