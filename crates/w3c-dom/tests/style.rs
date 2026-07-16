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
    doc.append_child(root);
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
    doc.append_child(root);
    engine.flush_document(&mut doc);
}
