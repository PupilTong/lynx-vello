//! Core value grammar (lengths, numbers, keywords, strings, colors, fonts)
//! — ported from `lynx/core/renderer/css/parser/length_handler_unittest.cc`,
//! `number_handler_unittest.cc`, `enum_handler_unittest.cc`,
//! `bool_handler_unittest.cc`, `string_handler_unittest.cc`,
//! `unit_handler_unittest.cc`, `font_length_handler_unittest.cc`,
//! `auto_font_size*_handler_unittest.cc`, and the value-grammar half of
//! `css_string_parser_unittest.cc`.

mod common;

use common::{Doc, parses, rgb, rgba, specified};
use w3c_dom::property_is_supported;

#[test]
fn length_grammar() {
    for (value, expected) in [
        ("10px", "10px"),
        ("1em", "1em"),
        ("2.1rem", "2.1rem"),
        ("0.7rpx", "0.7rpx"),
        ("0.7vw", "0.7vw"),
        ("0.7vh", "0.7vh"),
        ("10%", "10%"),
        ("0.1px", "0.1px"),
        (".1px", "0.1px"),
        ("auto", "auto"),
        ("max-content", "max-content"),
        ("fit-content(10%)", "fit-content(10%)"),
        (
            "fit-content(calc(10% - 0.5em))",
            "fit-content(calc(10% - 0.5em))",
        ),
    ] {
        assert_eq!(
            specified("width", value).as_deref(),
            Some(expected),
            "`{value}`"
        );
    }
    assert!(parses("width", "calc(2px + 3rpx)"), "rpx joins calc");
    assert!(parses("width", "0"), "unitless zero is a length");
    for invalid in ["abcd", "100 px", "1.px"] {
        assert!(!parses("width", invalid), "`{invalid}` must be rejected");
    }
}

#[test]
fn opacity_numbers() {
    assert_eq!(specified("opacity", "0.85").as_deref(), Some("0.85"));
    assert_eq!(
        specified("opacity", "85%").as_deref(),
        Some("0.85"),
        "percentages are equivalent numbers"
    );
    for invalid in ["test", "true"] {
        assert!(!parses("opacity", invalid), "`{invalid}` must be rejected");
    }
}

#[test]
fn enum_keywords() {
    for keyword in [
        "flex-start",
        "flex-end",
        "center",
        "stretch",
        "space-between",
        "space-around",
    ] {
        assert_eq!(
            specified("align-content", keyword).as_deref(),
            Some(keyword),
            "`{keyword}`"
        );
    }
    assert!(!parses("align-content", "align"));
}

#[test]
fn implicit_animation_is_absent() {
    assert!(
        !property_is_supported("implicit-animation"),
        "implicit-animation appeared — port the Lynx-faithful bool rows"
    );
}

#[test]
fn font_family_strings() {
    assert_eq!(specified("font-family", "test").as_deref(), Some("test"));
    assert!(!parses("font-family", "+(*^%$."));
    assert_eq!(
        specified("font-family", "\"+(*^%$.\"").as_deref(),
        Some("\"+(*^%$.\""),
        "quoted arbitrary family names are valid strings"
    );
}

#[test]
fn line_height_grammar() {
    for (value, expected) in [
        ("5", "5"),
        ("10", "10"),
        ("20px", "20px"),
        ("30rpx", "30rpx"),
        ("normal", "normal"),
    ] {
        assert_eq!(
            specified("line-height", value).as_deref(),
            Some(expected),
            "`{value}`"
        );
    }
    assert!(!parses("line-height", "40px 123456"));
}

#[test]
fn auto_font_size_family_is_absent() {
    for missing in [
        "-x-auto-font-size",
        "-x-auto-font-size-preset-sizes",
        "-x-auto-font-size-line-ranges",
    ] {
        assert!(
            !property_is_supported(missing),
            "{missing} appeared — port the Lynx-faithful grammar rows"
        );
    }
}

#[test]
fn rgb_and_rgba_colors() {
    type ColorRow = (&'static str, (u8, u8, u8, f32));
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    let rows: &[ColorRow] = &[
        ("rgb(255, 128, 0)", (255, 128, 0, 1.0)),
        ("rgb(255, 128, 0, 0.5)", (255, 128, 0, 0.5)),
        ("rgb(255 128 0)", (255, 128, 0, 1.0)),
        ("rgb(255 128 0 / 0.5)", (255, 128, 0, 0.5)),
        ("rgb(100% 50% 0%)", (255, 128, 0, 1.0)),
        ("rgb(none 128 none / 0.5)", (0, 128, 0, 0.5)),
        ("rgba(255, 128, 0, 0.5)", (255, 128, 0, 0.5)),
        ("rgba(0, 128, 0)", (0, 128, 0, 1.0)),
        ("rgba(0 128 0)", (0, 128, 0, 1.0)),
        ("rgba(0 128 0 / 1)", (0, 128, 0, 1.0)),
        ("rgba(0% 50% 0% / 100%)", (0, 128, 0, 1.0)),
    ];
    for &(input, (r, g, b, alpha)) in rows {
        doc.set_inline(el, &format!("color: {input}"));
        doc.flush();
        let expected = if (alpha - 1.0).abs() < f32::EPSILON {
            rgb(r, g, b)
        } else {
            rgba(r, g, b, alpha)
        };
        let actual = doc.color(el);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(
            close(actual.components.0, expected.components.0)
                && close(actual.components.1, expected.components.1)
                && close(actual.components.2, expected.components.2)
                && close(actual.alpha, expected.alpha),
            "`{input}`: {actual:?} != {expected:?}"
        );
    }
    for invalid in [
        "rgb(none, 128, 0)",
        "rgba(255, 128 0, 0.5)",
        "rgba(255, 128, 0 / 0.5)",
        "rgba(255 128 0, 0.5)",
        "rgba(255)",
        "rgba(255 128)",
        "rgba(255 128 0 /)",
        "rgba()",
        "rgba(none, 128, 0, 1)",
    ] {
        assert!(!parses("color", invalid), "`{invalid}` must be rejected");
    }
    assert!(parses("color", "rgb(255, 128, 0"));
}

#[test]
fn hsl_edge_cases() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for input in [
        "hsl(240, 100%, 50%)",
        "hsl(480, 100%, 50%)",
        "hsl(-120, 100%, 50%)",
        "hsl(1000000, 1000%, 1000%)",
        "hsl(0.0001, 0.0001%, 0.0001%)",
    ] {
        assert!(parses("color", input), "`{input}` must parse");
    }
    doc.set_inline(el, "color: hsl(480, 100%, 50%)");
    doc.flush();
    assert_eq!(doc.color(el), rgb(0, 255, 0));
    doc.set_inline(el, "color: hsl(-120, 100%, 50%)");
    doc.flush();
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}

#[test]
fn filter_value_forms() {
    for (a, b) in [
        ("grayscale(0.5)", "grayscale(50%)"),
        ("grayscale(.5)", "grayscale(50%)"),
        ("brightness(0.5)", "brightness(50%)"),
        ("contrast(.5)", "contrast(50%)"),
        ("saturate(0.5)", "saturate(50%)"),
    ] {
        let mut doc = Doc::new();
        let left = doc.el(doc.root, "view");
        let right = doc.el(doc.root, "view");
        doc.set_inline(left, &format!("filter: {a}"));
        doc.set_inline(right, &format!("filter: {b}"));
        doc.flush();
        assert_eq!(
            doc.value(left, "filter"),
            doc.value(right, "filter"),
            "`{a}` == `{b}`"
        );
    }
    assert!(parses("filter", "blur(1.5rpx)"), "rpx blur radius");
    assert!(
        parses("filter", "BlUr(20px)"),
        "case-insensitive function name"
    );
    assert_eq!(specified("filter", "none").as_deref(), Some("none"));
    for invalid in [
        "blur(10%)",
        "blur(2px9)",
        "blur(px)",
        "grayscale(ab%)",
        "grayscale(50,5%)",
        "grayscale(50.5 percent)",
        "abd(2px)",
        "blur(2px), grayscale(0.2)",
        "12px",
    ] {
        assert!(!parses("filter", invalid), "`{invalid}` must be rejected");
    }
}

#[test]
fn gap_grammar() {
    assert_eq!(specified("gap", "10px").as_deref(), Some("10px"));
    assert_eq!(specified("gap", "10px 20px").as_deref(), Some("10px 20px"));
    assert_eq!(specified("gap", "40%").as_deref(), Some("40%"));
    for invalid in ["abc 20px", "30px cde", "fghijk"] {
        assert!(!parses("gap", invalid), "`{invalid}` must be rejected");
    }
}

#[test]
fn aspect_ratio_string_rows() {
    assert_eq!(specified("aspect-ratio", "6").as_deref(), Some("6 / 1"));
    assert_eq!(specified("aspect-ratio", "1/2").as_deref(), Some("1 / 2"));
    assert_eq!(specified("aspect-ratio", "2/2").as_deref(), Some("2 / 2"));
    assert_eq!(
        specified("aspect-ratio", "2/0").as_deref(),
        Some("2 / 0"),
        "degenerate ratios parse and behave as auto"
    );
    for invalid in [" ", "2 chaos 1", "50%", "-7"] {
        assert!(
            !parses("aspect-ratio", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn inset_basic_shapes() {
    assert_eq!(
        specified("clip-path", "inset(5%)").as_deref(),
        Some("inset(5%)")
    );
    for valid in [
        "inset(10px 20px)",
        "inset(10px 20% 30px)",
        "inset(10px 20% 30px 40rpx)",
        "inset(10px round 10px / 20px)",
        "inset(10px round 10px 20px / 20px 30% 40%)",
    ] {
        assert!(parses("clip-path", valid), "`{valid}` must parse");
    }
    for invalid in [
        "inset(10 10 10 10)",
        "inset(10px 10px 10px 10px) asd",
        "inset(10px 10px circle)",
        "inset(10px 10px 10px 10px 10px)",
        "inset(10px round 10px // 10px)",
        "inset(10px round 10px 10px 10px 10px 10px / 10px)",
        "inset(10px round 10px 10px / 10px 10px 10px 10px 10px)",
        "inset(10px super-ellipse 10 10px 10px / 10px 10px 10px 10px)",
    ] {
        assert!(
            !parses("clip-path", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn font_feature_and_variation_settings() {
    assert_eq!(
        specified("font-variation-settings", "'wght' 800").as_deref(),
        Some("\"wght\" 800")
    );
    assert_eq!(
        specified("font-variation-settings", "\"wdth\" 125.0, 'wght' 750").as_deref(),
        Some("\"wdth\" 125, \"wght\" 750")
    );
    assert_eq!(
        specified("font-variation-settings", "normal").as_deref(),
        Some("normal")
    );
    for invalid in [
        "'wght'",
        "'wght', 100",
        "'badtagname' 100",
        "'abc' 1",
        "'abcde' 1",
    ] {
        assert!(
            !parses("font-variation-settings", invalid),
            "`{invalid}` must be rejected"
        );
    }

    assert_eq!(
        specified("font-feature-settings", "'dlig'").as_deref(),
        Some("\"dlig\"")
    );
    assert_eq!(
        specified("font-feature-settings", "'dlig' on, \"smcp\" off, 'c2sc' 2").as_deref(),
        Some("\"dlig\", \"smcp\" 0, \"c2sc\" 2"),
        "on/off map to 1 (implicit) and 0"
    );
    assert_eq!(
        specified("font-feature-settings", "normal").as_deref(),
        Some("normal")
    );
    for invalid in ["'dlig' , on off", "'badtagname'", "'dlig',, 'smcp'"] {
        assert!(
            !parses("font-feature-settings", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn url_tokens() {
    let url = "https://example.com/calcfakq/env?x-/..&";
    let value =
        specified("background-image", &format!("url({url})")).expect("unquoted url token parses");
    assert!(value.contains(url), "verbatim body: {value}");
    let _ = specified("background-image", "url(https://example.com/calc()fakq/..)");
}
