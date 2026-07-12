//! Transform, transform-origin, and motion-path grammar — ported from
//! `lynx/core/renderer/css/parser/transform_handler_unittest.cc`,
//! `transform_origin_handler_unittest.cc`,
//! `offset_distance_handler_unittest.cc`, and the offset-rotate rows of
//! `css_string_parser_unittest.cc`.
//!
//! Scope: `enableCSSSelector = true` / `enableRemoveCSSScope = true`. W3C
//! corrections: the legacy lenient forms (`rotate(20)` without a unit,
//! three-argument `translate()`, comma-separated transform-origin) reject;
//! angles are kept literal (no mod-360 at parse); percentages stay
//! percentages.

mod common;

use common::{Doc, parses, specified};

// C++: transform_handler_unittest.cc valid function lists.
#[test]
fn transform_function_lists() {
    for valid in [
        "translate(1px, 2px) scale(0.1) rotate(10deg)",
        "translate(1px, 2px) scale(1.1, 1.5) rotateX(15deg)",
        "translate3d(1px, 2px, 3px) rotateY(30deg)",
        "rotateX(-30deg) rotateY(-10deg)",
        "scale(-10, 20) translate3d(2px, -4px, 5px)",
        "translateX(2px) translateY(3px) translateZ(-4px) rotateX(10deg) rotateY(20deg) rotateZ(-10deg)",
        "translate(1px)",
    ] {
        assert!(parses("transform", valid), "`{valid}` must parse");
    }
    assert_eq!(
        specified("transform", "translate(1px, 2px) scale(0.1) rotate(10deg)").as_deref(),
        Some("translate(1px, 2px) scale(0.1) rotate(10deg)"),
        "function order and operands preserved"
    );
    assert_eq!(specified("transform", "none").as_deref(), Some("none"));
}

// C++: transform_handler_unittest.cc strict invalid rows + the legacy
// lenient equivalences (W3C-corrected to rejections).
#[test]
fn transform_rejects() {
    for invalid in [
        "",
        "translate(1px,",
        "rotate(20)",
        "skew(20deg, 20)",
        "scale(20px)",
        "scale(2, 20px)",
        "translate(1px, 10",
        "translate(1px, 10px, 10px)",
        "translate3d(2px, -4px)",
    ] {
        assert!(
            !parses("transform", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

// C++: transform_origin_handler_unittest.cc — keywords resolve to
// percentages at computed value; comma form rejects (W3C-corrected).
#[test]
fn transform_origin_grammar() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    // (input, computed) — computed origin serializes "x y z".
    let rows: &[(&str, &str)] = &[
        ("10px", "10px 50% 0px"),
        ("10px 10%", "10px 10% 0px"),
        ("left top", "0% 0% 0px"),
        ("bottom right", "100% 100% 0px"),
        ("right bottom", "100% 100% 0px"),
        ("center  center ", "50% 50% 0px"),
    ];
    for &(input, expected) in rows {
        doc.set_inline(el, &format!("transform-origin: {input}"));
        doc.flush();
        assert_eq!(doc.value(el, "transform-origin"), expected, "`{input}`");
    }
    assert!(
        !parses("transform-origin", "center, center"),
        "comma-separated components are invalid (Lynx legacy leniency)"
    );
}

// C++: offset_distance_handler_unittest.cc — W3C-corrected: unitless
// non-zero rejects; percentages stay percentages.
#[test]
fn offset_distance_grammar() {
    assert!(parses("offset-distance", "0"), "unitless zero length");
    assert_eq!(specified("offset-distance", "0%").as_deref(), Some("0%"));
    assert_eq!(specified("offset-distance", "50%").as_deref(), Some("50%"));
    assert_eq!(
        specified("offset-distance", "100%").as_deref(),
        Some("100%")
    );
    assert_eq!(
        specified("offset-distance", "10px").as_deref(),
        Some("10px")
    );
    for invalid in ["1", "100% foo", "auto"] {
        assert!(
            !parses("offset-distance", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

// C++: css_string_parser_unittest.cc offset_rotate_value — W3C grammar
// `[ auto | reverse ] || <angle>`; angles stay literal (no mod-360, no
// sentinel packing).
#[test]
fn offset_rotate_grammar() {
    for (input, expected) in [
        ("auto", "auto"),
        ("reverse", "reverse"),
        ("45deg", "45deg"),
        ("0.25turn", "0.25turn"),
        ("-90deg", "-90deg"),
        ("450deg", "450deg"),
        ("auto 45deg", "auto 45deg"),
        ("reverse 45deg", "reverse 45deg"),
    ] {
        assert_eq!(
            specified("offset-rotate", input).as_deref(),
            Some(expected),
            "`{input}`"
        );
    }
    // `45deg auto` reorders to canonical keyword-first form.
    assert_eq!(
        specified("offset-rotate", "45deg auto").as_deref(),
        Some("auto 45deg")
    );
    for invalid in ["100%", "auto reverse", "wrongvalue"] {
        assert!(
            !parses("offset-rotate", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

// C++: css_string_parser_unittest.cc parse_cursor — url images carry an
// optional two-number hotspot; a lone hotspot number is invalid per W3C
// (Lynx defaulted it to 0,0).
#[test]
fn cursor_grammar() {
    assert_eq!(specified("cursor", "help").as_deref(), Some("help"));
    assert_eq!(
        specified("cursor", "url(hand.cur), pointer").as_deref(),
        Some("url(\"hand.cur\"), pointer")
    );
    assert_eq!(
        specified("cursor", "url(hand.cur) 10 20, pointer").as_deref(),
        Some("url(\"hand.cur\") 10 20, pointer")
    );
    assert!(
        !parses("cursor", "url(hand.cur) 10, pointer"),
        "a lone hotspot coordinate is invalid (W3C-corrected)"
    );
}

// Skipped (skip-internal): the lepus null/type-guard rows and Lynx's
// [value, pattern] flat-array encodings across all four files.
// Skipped (skip-legacy): the enable_new_transform_handler=false lenient
// branch — pre-W3C pipeline out of scope under enableCSSSelector=true.
