//! Core value grammar (lengths, numbers, keywords, strings, colors, fonts)
//! — ported from `lynx/core/renderer/css/parser/length_handler_unittest.cc`,
//! `number_handler_unittest.cc`, `enum_handler_unittest.cc`,
//! `bool_handler_unittest.cc`, `string_handler_unittest.cc`,
//! `unit_handler_unittest.cc`, `font_length_handler_unittest.cc`,
//! `auto_font_size*_handler_unittest.cc`, and the value-grammar half of
//! `css_string_parser_unittest.cc`.
//!
//! Scope: `enableCSSSelector = true` / `enableRemoveCSSScope = true`. Lynx's
//! lepus type-guard rows (bools/ints/null fed to handlers) have no CSS-text
//! analog and are skipped throughout; `rpx` is fork-native, `ppx` is a
//! pinned gap (see `grammar_background.rs`).

mod common;

use common::{Doc, parses, rgb, rgba, specified};
use stylo::values::specified::WillChangeBits;
use stylo_dom::property_is_supported;

// C++: length_handler_unittest.cc width table + css_string_parser
// length_valid_and_value / length_invalid_space / decimal_points.
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

// C++: number_handler_unittest.cc opacity table (+ percentage form, which
// computes to the same number).
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
fn containment_hints_survive_cascade() {
    let mut doc =
        Doc::with_css("view { contain: layout paint; will-change: contain, transform, opacity; }");
    let element = doc.el(doc.root, "view");
    doc.flush();
    assert_eq!(doc.value(element, "contain"), "layout paint");
    assert_eq!(
        doc.value(element, "will-change"),
        "contain, transform, opacity"
    );
    let bits = doc.style(element).clone_will_change().bits;
    assert!(bits.contains(WillChangeBits::CONTAIN));
    assert!(bits.contains(WillChangeBits::TRANSFORM));
    assert!(bits.contains(WillChangeBits::OPACITY));
}

// C++: enum_handler_unittest.cc align-content keywords (the enum handlers
// are code-generated; one property proves the family).
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

// C++: bool_handler_unittest.cc — `implicit-animation` is a Lynx-only
// boolean property outside the fork's surface; pinned so a fork addition is
// noticed and the true/false keyword rows get ported.
#[test]
fn implicit_animation_is_absent() {
    assert!(
        !property_is_supported("implicit-animation"),
        "implicit-animation appeared — port the Lynx-faithful bool rows"
    );
}

// C++: string_handler_unittest.cc font-family table — W3C-corrected: a
// symbol run is not a valid <custom-ident> family (Lynx stored it
// verbatim); quoting it makes it a valid <string> family.
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

// C++: font_length_handler_unittest.cc line-height table.
#[test]
fn line_height_grammar() {
    for (value, expected) in [
        ("5", "5"),
        ("10", "10"),
        ("20px", "20px"),
        ("30rpx", "30rpx"),
        ("120%", "120%"),
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
fn font_property_grammars_stay_upstream() {
    for value in ["medium", "smaller", "larger"] {
        assert!(parses("font-size", value), "font-size: {value}");
    }
    assert!(parses("font-style", "oblique 10deg"));
    for value in ["bolder", "lighter", "1", "100.5", "1000"] {
        assert!(parses("font-weight", value), "font-weight: {value}");
    }
    assert!(!parses("font-weight", "0"));
    assert!(!parses("font-weight", "1001"));
}

// C++: auto_font_size*_handler_unittest.cc — the `-x-auto-font-size` family
// is Lynx-only (bespoke flag+lengths / line-range() grammar) and absent
// from the fork; pinned for the same port-on-addition contract.
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

// C++: css_string_parser_unittest.cc rgb/rgba tables (legacy commas, modern
// spaces, percentages, `none` components).
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
        // Compare channels, not flags: modern `none` components resolve to
        // zero but carry is-none flag bits.
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
        "rgb(none, 128, 0)", // `none` needs modern syntax
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
    // Unclosed rgb( at the end of input auto-closes per CSS Syntax EOF
    // rules (the C++ raw-parser rejected it — tokenizer-level difference).
    assert!(parses("color", "rgb(255, 128, 0"));
}

// C++: css_string_parser_unittest.cc hsl edge cases — hue wraps, s/l clamp.
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
    // 480 == 120 (mod 360); -120 == 240.
    doc.set_inline(el, "color: hsl(480, 100%, 50%)");
    doc.flush();
    assert_eq!(doc.color(el), rgb(0, 255, 0));
    doc.set_inline(el, "color: hsl(-120, 100%, 50%)");
    doc.flush();
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}

// C++: css_string_parser_unittest.cc filter value tables — number and
// percentage amounts are equivalent; blur takes a bare <length>.
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
        "blur(2px), grayscale(0.2)", // chains are space-separated, not comma
        "12px",
    ] {
        assert!(!parses("filter", invalid), "`{invalid}` must be rejected");
    }
}

// C++: css_string_parser_unittest.cc gap_value — W3C-corrected: Lynx's
// substitute-0px-for-junk leniency becomes whole-declaration rejection.
#[test]
fn gap_grammar() {
    assert_eq!(specified("gap", "10px").as_deref(), Some("10px"));
    assert_eq!(specified("gap", "10px 20px").as_deref(), Some("10px 20px"));
    assert_eq!(specified("gap", "40%").as_deref(), Some("40%"));
    for invalid in ["abc 20px", "30px cde", "fghijk"] {
        assert!(!parses("gap", invalid), "`{invalid}` must be rejected");
    }
}

// C++: css_string_parser_unittest.cc aspect_ratio_value — W3C-corrected:
// ratios stay pairs, degenerate `2/0` PARSES (behaves as auto per
// css-values-4; Lynx rejected it), negatives reject.
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

// C++: css_string_parser_unittest.cc inset basic shapes — edge and corner
// expansion per W3C; `rpx` participates; `ppx`/`super-ellipse` stay pinned
// gaps (grammar_background.rs).
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

// C++: css_string_parser_unittest.cc OpenType tag + font settings tables.
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

// C++: css_string_parser_unittest.cc url token parsing — verbatim body plus
// the historical crash input as a no-panic check.
#[test]
fn url_tokens() {
    let url = "https://example.com/calcfakq/env?x-/..&";
    let value =
        specified("background-image", &format!("url({url})")).expect("unquoted url token parses");
    assert!(value.contains(url), "verbatim body: {value}");
    // No-crash robustness input (result unspecified).
    let _ = specified("background-image", "url(https://example.com/calc()fakq/..)");
}

// Skipped (skip-internal): unit_handler_unittest.cc property-id bounds
// checks, lepus type guards across every file, LerpColor blending helper,
// and Lynx VarReference byte-offset tracking (var() behavior itself is
// covered in custom_properties.rs).
// Skipped (folded): css_string_parser font-face src/weight rows — covered
// at the descriptor level in at_rules.rs; the -x-text-decoration length
// rows — pinned as engine gaps in inheritance_computed.rs.
