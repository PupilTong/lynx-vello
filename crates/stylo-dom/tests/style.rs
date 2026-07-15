//! Direct tests for the embedder-neutral style document.

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
use stylo_atoms::Atom;
use stylo_dom::{Document, StylesheetOrigin};
use stylo_traits::{CSSPixel, DevicePixel};

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
fn documents_are_independent_instances() {
    let mut first = Document::new(device(800.0, 600.0));
    let mut second = Document::new(device(400.0, 300.0));
    first.add_stylesheet_str(".probe { color: red; }", StylesheetOrigin::Author);
    second.add_stylesheet_str(".probe { color: blue; }", StylesheetOrigin::Author);

    let first_id = first.create_element("div", ());
    let second_id = second.create_element("div", ());
    assert_eq!(first_id, second_id, "node ids are local to each document");
    first
        .classes_mut(first_id)
        .unwrap()
        .push(Atom::from("probe"));
    second
        .classes_mut(second_id)
        .unwrap()
        .push(Atom::from("probe"));

    let first_style = first.resolve(first.node(first_id).unwrap(), None);
    let second_style = second.resolve(second.node(second_id).unwrap(), None);
    assert_eq!(
        first_style.clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );
    assert_eq!(
        second_style.clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0)
    );
    assert!((first.device().viewport_size().width - 800.0).abs() < f32::EPSILON);
    assert!((second.device().viewport_size().width - 400.0).abs() < f32::EPSILON);
}

#[test]
fn standard_cascade_is_embedder_neutral() {
    let mut document = Document::new(device(800.0, 600.0));
    document.add_stylesheet_str(
        ".parent { color: green; } .child { color: red; }",
        StylesheetOrigin::Author,
    );

    let parent = document.create_element("section", ());
    let child = document.create_element("span", ());
    document
        .classes_mut(parent)
        .unwrap()
        .push(Atom::from("parent"));
    document
        .classes_mut(child)
        .unwrap()
        .push(Atom::from("child"));
    document.attach_at(parent, child, 0);

    let parent_style = document.resolve(document.element_ref(parent).unwrap(), None);
    assert_eq!(
        parent_style.clone_color(),
        AbsoluteColor::srgb_legacy(0, 128, 0, 1.0)
    );

    document.set_inline_styles(child, "color: blue");
    let child_style = document.resolve(
        document.element_ref(child).unwrap(),
        Some(parent_style.as_ref()),
    );
    assert_eq!(
        child_style.clone_color(),
        AbsoluteColor::srgb_legacy(0, 0, 255, 1.0),
        "standard inline declarations outrank author class rules"
    );
}

#[test]
fn media_queries_follow_standard_viewport_updates() {
    let mut document = Document::new(device(800.0, 600.0));
    document.add_stylesheet_with_media(
        ".box { color: red; }",
        StylesheetOrigin::Author,
        "(min-width: 600px)",
    );

    let element = document.create_element("div", ());
    document
        .classes_mut(element)
        .unwrap()
        .push(Atom::from("box"));

    let wide = document.resolve(document.element_ref(element).unwrap(), None);
    assert_eq!(
        wide.clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );

    document.set_viewport(400.0, 600.0);
    let narrow = document.resolve(document.element_ref(element).unwrap(), None);
    assert_ne!(narrow.clone_color(), wide.clone_color());
}
