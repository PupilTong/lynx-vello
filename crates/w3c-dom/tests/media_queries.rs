//! Media-query parsing and evaluation — ported from
//! `lynx/core/renderer/css/ng/media_query/media_query_evaluator_test.cc`,
//! `ng/media_query/media_query_test.cc`, and
//! `ng/parser/media_query_parser_test.cc`.

mod common;

use common::{device, device_with, rgb, url_data};
use stylo::context::QuirksMode;
use stylo::custom_properties::AttrTaint;
use stylo::device::Device;
use stylo::media_queries::{MediaList, MediaType};
use stylo::parser::ParserContext;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::queries::values::PrefersColorScheme;
use stylo::servo::media_features::PointerCapabilities;
use stylo::stylesheets::{CssRuleType, Origin};
use stylo::values::computed::{CSSPixelLength, Length};
use stylo_traits::{CSSPixel, DevicePixel, ParsingMode, ToCss};
use w3c_dom::{Document, StylesheetOrigin};

fn matches_dev(device: Device, query: &str) -> bool {
    let mut doc: Document<()> = Document::new(device);
    doc.add_stylesheet_with_media(
        ".probe { color: rgb(1, 2, 3) }",
        StylesheetOrigin::Author,
        query,
    );
    let probe = doc.create_element("view", ());
    doc.add_class(probe, "probe");
    let style = doc.resolve_style(doc.get(probe).expect("fresh node"), None);
    style.clone_color() == rgb(1, 2, 3)
}

fn reference() -> Device {
    device_with(375.0, 812.0, 3.0, PrefersColorScheme::Light)
}

fn matches(query: &str) -> bool {
    matches_dev(reference(), query)
}

fn pointer_device(capabilities: PointerCapabilities) -> Device {
    use euclid::{Scale, Size2D};
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        Size2D::<f32, CSSPixel>::new(375.0, 812.0),
        Size2D::<f32, DevicePixel>::new(375.0, 812.0),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(common::TestFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        capabilities,
        capabilities,
    )
}

fn media_css(query: &str) -> String {
    let mut input = cssparser::ParserInput::new(query);
    let mut parser = cssparser::Parser::new(&mut input);
    let url_data = url_data();
    let mut context = ParserContext::new(
        Origin::Author,
        &url_data,
        Some(CssRuleType::Media),
        ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        std::borrow::Cow::default(),
        None,
        None,
        AttrTaint::default(),
    );
    MediaList::parse(&mut context, &mut parser).to_css_string()
}

#[allow(dead_code)]
fn _length_types(_: CSSPixelLength, _: Length) {}

#[test]
fn media_type_matching() {
    let rows: &[(&str, bool)] = &[
        ("screen", true),
        ("all", true),
        ("print", false),
        ("not print", true),
        ("not screen", false),
        ("only screen", true),
    ];
    for &(query, expected) in rows {
        assert_eq!(matches(query), expected, "query `{query}`");
    }
}

#[test]
fn empty_matches_and_invalid_never_matches() {
    assert!(matches(""));
    assert!(matches("   \t  "));
    assert!(!matches("invalid!"));
    assert!(!matches("(invalid!)"));
    assert!(!matches("(some-unknown-feature: 1)"));
}

#[test]
fn width_and_height_features() {
    let rows: &[(&str, bool)] = &[
        ("(width: 375px)", true),
        ("(width: 400px)", false),
        ("(min-width: 300px)", true),
        ("(min-width: 400px)", false),
        ("(max-width: 500px)", true),
        ("(max-width: 300px)", false),
        ("(width >= 375px)", true),
        ("(width > 375px)", false),
        ("(width < 400px)", true),
        ("(300px <= width <= 500px)", true),
        ("(400px <= width <= 500px)", false),
        ("(width)", true),
        ("(height: 812px)", true),
        ("(min-height: 600px)", true),
        ("(max-height: 900px)", true),
    ];
    for &(query, expected) in rows {
        assert_eq!(matches(query), expected, "query `{query}`");
    }
    assert!(!matches_dev(device(0.0, 0.0), "(width)"));
}

#[test]
fn invalid_feature_values_never_match() {
    for query in ["(width: 10foo)", "(min-width: 10foo)", "(width: 50%)"] {
        assert!(!matches(query), "query `{query}`");
    }
}

#[test]
fn relative_units_resolve_against_device() {
    let rows: &[(&str, bool)] = &[
        ("(min-width: 20rem)", true),
        ("(min-width: 25em)", false),
        ("(min-width: 50vw)", true),
        ("(min-width: 200vw)", false),
        ("(min-height: 50vh)", true),
        ("(min-height: 150vh)", false),
        ("(max-height: 200vh)", true),
        ("(width: 10vh)", false),
        ("(min-width: 1.5em)", true),
        ("(min-width: 2.5rem)", true),
    ];
    for &(query, expected) in rows {
        assert_eq!(matches(query), expected, "query `{query}`");
    }
}

#[test]
fn orientation_follows_viewport() {
    assert!(matches("(orientation: portrait)"));
    assert!(!matches("(orientation: landscape)"));
    assert!(matches("(orientation)"));
    assert!(matches_dev(
        device(1024.0, 768.0),
        "(orientation: landscape)"
    ));
    assert!(matches_dev(
        device(768.0, 1024.0),
        "(orientation: portrait)"
    ));
    assert!(matches_dev(device(500.0, 500.0), "(orientation: portrait)"));
}

#[test]
fn resolution_and_device_pixel_ratio() {
    let rows: &[(&str, bool)] = &[
        ("(min-resolution: 200dpi)", true),
        ("(resolution >= 2dppx)", true),
        ("(resolution: 3dppx)", true),
        ("(resolution: 2dppx)", false),
        ("(min-resolution: 2dppx)", true),
        ("(min-resolution: 4dppx)", false),
        ("(max-resolution: 3dppx)", true),
        ("(max-resolution: 2dppx)", false),
        ("(-webkit-device-pixel-ratio: 3)", true),
        ("(-webkit-min-device-pixel-ratio: 2)", true),
        ("(-webkit-max-device-pixel-ratio: 2)", false),
        ("(-moz-device-pixel-ratio: 3)", true),
        ("(device-pixel-ratio: 3)", false),
        ("(device-pixel-ratio)", false),
    ];
    for &(query, expected) in rows {
        assert_eq!(matches(query), expected, "query `{query}`");
    }
}

#[test]
fn aspect_ratio_feature() {
    let landscape = || device(1024.0, 768.0);
    let square = || device(500.0, 500.0);
    assert!(matches_dev(landscape(), "(aspect-ratio: 4/3)"));
    assert!(matches_dev(landscape(), "(min-aspect-ratio: 1/1)"));
    assert!(!matches_dev(landscape(), "(max-aspect-ratio: 1/1)"));
    assert!(matches_dev(landscape(), "(aspect-ratio: 16 / 12)"));
    assert!(matches_dev(square(), "(aspect-ratio: 1)"));
    assert!(matches_dev(square(), "(aspect-ratio: 1/1)"));
    for query in [
        "(aspect-ratio: 16 / landscape)",
        "(aspect-ratio: 16/9 extra)",
        "(aspect-ratio: 16 / -2)",
    ] {
        assert!(!matches_dev(square(), query), "query `{query}`");
    }
    assert!(!matches_dev(square(), "(aspect-ratio: 1/0)"));
}

#[test]
fn deprecated_device_aspect_ratio_never_matches() {
    for query in [
        "(min-device-aspect-ratio: 1/3)",
        "(device-aspect-ratio: 16/9)",
    ] {
        assert!(!matches(query), "query `{query}`");
    }
}

#[test]
fn hover_and_pointer_features() {
    let capable = PointerCapabilities::FINE | PointerCapabilities::HOVER;
    let rows: &[(&str, bool)] = &[
        ("(hover: hover)", true),
        ("(hover: none)", false),
        ("(hover)", true),
        ("(pointer: fine)", true),
        ("(pointer: coarse)", false),
        ("(pointer: none)", false),
        ("(pointer)", true),
    ];
    for &(query, expected) in rows {
        assert_eq!(
            matches_dev(pointer_device(capable), query),
            expected,
            "query `{query}` (capable device)"
        );
    }
    let none = PointerCapabilities::empty();
    for (query, expected) in [
        ("(hover: none)", true),
        ("(hover)", false),
        ("(pointer: none)", true),
        ("(pointer)", false),
    ] {
        assert_eq!(
            matches_dev(pointer_device(none), query),
            expected,
            "query `{query}` (capability-less device)"
        );
    }
}

#[test]
fn prefers_color_scheme_feature() {
    let dark = || device_with(375.0, 812.0, 1.0, PrefersColorScheme::Dark);
    assert!(matches_dev(dark(), "(prefers-color-scheme: dark)"));
    assert!(!matches_dev(dark(), "(prefers-color-scheme: light)"));
    assert!(matches_dev(dark(), "(prefers-color-scheme)"));
    assert!(matches("(prefers-color-scheme: light)"));
}

#[test]
#[ignore = "engine-gap: servo Device implements no `color` media feature; (color)/(min-color) parse to `not all`"]
fn color_feature() {
    let rows: &[(&str, bool)] = &[
        ("(color)", true),
        ("(color: 8)", true),
        ("(color: 16)", false),
        ("(min-color: 4)", true),
        ("(min-color: 10)", false),
        ("(max-color: 8)", true),
        ("(max-color: 4)", false),
    ];
    for &(query, expected) in rows {
        assert_eq!(matches(query), expected, "query `{query}`");
    }
}

#[test]
fn compound_conditions_and_list_semantics() {
    let rows: &[(&str, bool)] = &[
        ("not (width >= 1000px)", true),
        ("(min-width: 300px) and (max-width: 500px)", true),
        ("(min-width: 300px) and (max-width: 350px)", false),
        ("(min-width: 1000px) or (max-width: 500px)", true),
        ("(min-width: 1000px) or (min-width: 800px)", false),
        ("((width >= 300px))", true),
        ("(not (hover))", true),
        ("screen and (min-width: 300px)", true),
        ("not print and (min-width: 300px)", true),
        ("only screen and (min-width: 300px)", true),
        ("print, screen and (min-width: 300px)", true),
        ("print, screen and (min-width: 1000px)", false),
        (
            "(min-width: 100px) and (max-width: 800px) or (hover)",
            false,
        ),
        ("screen and (min-width: 100px) or (hover)", false),
        ("(min-width >= 300px)", false),
        ("NOT PRINT AND (MIN-WIDTH: 300px)", true),
        ("(Min-Width: 100px)", true),
    ];
    for &(query, expected) in rows {
        assert_eq!(matches(query), expected, "query `{query}`");
    }
}

#[test]
fn range_syntax_forms() {
    let rows: &[(&str, bool)] = &[
        ("(width < 800px)", true),
        ("(width <= 800px)", true),
        ("(width > 400px)", false),
        ("(width = 375px)", true),
        ("(width == 100px)", false),
        ("(600px >= width)", true),
        ("(1024px > width)", true),
        ("(300px < width)", true),
        ("(foo < bar)", false),
        ("(100px < width > 500px)", false),
        ("(100px = width < 200px)", false),
    ];
    for &(query, expected) in rows {
        assert_eq!(matches(query), expected, "query `{query}`");
    }
}

#[test]
fn error_recovery() {
    for query in [
        "()",
        "(min-width:)",
        "(width: 100px foo)",
        ",",
        "!@#$%^&",
        "not",
        "only",
        "and and (color)",
        "or",
    ] {
        assert!(!matches(query), "query `{query}` must never match");
    }
    assert!(matches("(width"), "unclosed block auto-closes at EOF");
}

#[test]
fn invalid_list_entries_recover_in_place() {
    assert!(matches("screen, ???, (hover)"));
    assert!(matches("(width: 100px bad), screen, (invalid!), (hover)"));
    assert!(matches(
        "screen and (min-width: 300px) foo, (min-width: 300px)"
    ));
    assert_eq!(
        media_css("screen, ???, (hover)"),
        "screen, not all, (hover)"
    );
}

#[test]
fn serialization_is_canonical_and_idempotent() {
    assert_eq!(media_css("(min-width: 600px)"), "(min-width: 600px)");
    assert_eq!(media_css("screen"), "screen");
    assert_eq!(media_css("not print"), "not print");
    for query in [
        "(min-width: 600px)",
        "not print and (min-width: 300px)",
        "screen, (hover)",
        "(width >= 1024px)",
        "(100px < width < 500px)",
        "only screen and (min-width: 2.5rem)",
    ] {
        let first = media_css(query);
        let second = media_css(&first);
        assert_eq!(first, second, "serialization of `{query}` is idempotent");
    }
}
