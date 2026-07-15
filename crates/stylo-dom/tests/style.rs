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
use stylo_atoms::Atom;
use stylo_dom::{StyleEngine, StylesheetOrigin};
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
fn standard_cascade_is_embedder_neutral() {
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_str(
        ".parent { color: green; } .child { color: red; }",
        StylesheetOrigin::Author,
    );

    let mut arena = engine.new_arena();
    let parent = arena.create_element("section", ());
    let child = arena.create_element("span", ());
    arena
        .classes_mut(parent)
        .unwrap()
        .push(Atom::from("parent"));
    arena.classes_mut(child).unwrap().push(Atom::from("child"));
    arena.attach_at(parent, child, 0);

    let parent_style = engine.resolve(arena.element_ref(parent).unwrap(), None);
    assert_eq!(
        parent_style.clone_color(),
        AbsoluteColor::srgb_legacy(0, 128, 0, 1.0)
    );

    arena.set_inline_styles(child, "color: blue");
    let child_style = engine.resolve(
        arena.element_ref(child).unwrap(),
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
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_with_media(
        ".box { color: red; }",
        StylesheetOrigin::Author,
        "(min-width: 600px)",
    );

    let mut arena = engine.new_arena();
    let element = arena.create_element("div", ());
    arena.classes_mut(element).unwrap().push(Atom::from("box"));

    let wide = engine.resolve(arena.element_ref(element).unwrap(), None);
    assert_eq!(
        wide.clone_color(),
        AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)
    );

    engine.set_viewport(400.0, 600.0);
    let narrow = engine.resolve(arena.element_ref(element).unwrap(), None);
    assert_ne!(narrow.clone_color(), wide.clone_color());
}
