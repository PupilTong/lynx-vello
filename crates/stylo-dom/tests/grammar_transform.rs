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

// C++: offset_distance_handler_unittest.cc — Lynx treats both literal
// numbers and percentages as a normalized path fraction. Lengths are absent.
#[test]
fn offset_distance_grammar() {
    assert_eq!(specified("offset-distance", "0").as_deref(), Some("0%"));
    assert_eq!(specified("offset-distance", "0.5").as_deref(), Some("50%"));
    assert_eq!(specified("offset-distance", "1").as_deref(), Some("100%"));
    assert_eq!(specified("offset-distance", "0%").as_deref(), Some("0%"));
    assert_eq!(specified("offset-distance", "50%").as_deref(), Some("50%"));
    assert_eq!(
        specified("offset-distance", "100%").as_deref(),
        Some("100%")
    );
    for invalid in ["-0.1", "1.1", "101%", "10px", "calc(50%)", "auto"] {
        assert!(
            !parses("offset-distance", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

// C++: css_string_parser_unittest.cc offset_rotate_value — Lynx's documented
// subset is `auto` or one angle in the inclusive 0deg..=360deg range.
#[test]
fn offset_rotate_grammar() {
    for (input, expected) in [
        ("auto", "auto"),
        ("45deg", "45deg"),
        ("0.25turn", "0.25turn"),
        ("360deg", "360deg"),
    ] {
        assert_eq!(
            specified("offset-rotate", input).as_deref(),
            Some(expected),
            "`{input}`"
        );
    }
    for invalid in [
        "reverse",
        "-90deg",
        "361deg",
        "450deg",
        "auto 45deg",
        "45deg auto",
        "100%",
        "wrongvalue",
    ] {
        assert!(
            !parses("offset-rotate", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

// C++: css_string_parser_unittest.cc parse_cursor — the Lynx subset exposes
// keyword cursors only; URL cursor lists are deliberately absent.
#[test]
fn cursor_grammar() {
    for keyword in ["auto", "help", "pointer", "grab", "zoom-in"] {
        assert_eq!(specified("cursor", keyword).as_deref(), Some(keyword));
    }
    for invalid in [
        "url(hand.cur), pointer",
        "url(hand.cur) 10 20, pointer",
        "url(hand.cur) 10, pointer",
    ] {
        assert!(!parses("cursor", invalid), "`{invalid}` must be rejected");
    }
}

// Skipped (skip-internal): the lepus null/type-guard rows and Lynx's
// [value, pattern] flat-array encodings across all four files.
// Skipped (skip-legacy): the enable_new_transform_handler=false lenient
// branch — pre-W3C pipeline out of scope under enableCSSSelector=true.
