//! Border, shadow, text-stroke/decoration, and filter grammar —
//! ported from `lynx/core/renderer/css/parser/border_handler_unittest.cc`,
//! `border_radius_handler_unittest.cc`, `shadow_handler_unittest.cc`,
//! `text_stroke_handler_unittest.cc`, `text_decoration_handler_unittest.cc`,
//! and `filter_handler_unittest.cc`.
//!
//! Scope: `enableCSSSelector = true` / `enableRemoveCSSScope = true`. W3C
//! corrections: shorthands reset omitted longhands to initial values (border
//! color → currentColor, not "absent"); corner-radius
//! longhands reject Lynx's compat slash form; unitless non-zero lengths
//! reject (no `enable_length_unit_check` leniency); single-value unprefixed
//! `text-stroke` is valid per the `||` grammar (Lynx rejected it).

mod common;

use common::{Doc, parses, rgb, specified};

/// Computed value of `property` after applying `declaration` inline.
fn computed(declaration: &str, property: &str) -> String {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, declaration);
    doc.flush();
    doc.value(el, property)
}

// C++: border_handler_unittest.cc::border_shorthand_expansion.
#[test]
fn border_shorthand_expansion() {
    let mut doc = Doc::with_css("view { color: rgb(1, 2, 3) }");
    let el = doc.el(doc.root, "view");

    doc.set_inline(el, "border: 10px double red");
    doc.flush();
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(doc.value(el, &format!("border-{side}-width")), "10px");
        assert_eq!(doc.value(el, &format!("border-{side}-style")), "double");
        assert_eq!(doc.color(el), rgb(1, 2, 3));
        assert_eq!(
            doc.value(el, &format!("border-{side}-color")),
            "rgb(255, 0, 0)"
        );
    }

    // Omitted color resets to currentColor (resolves through `color`) —
    // W3C-corrected over Lynx's emit-nothing.
    doc.set_inline(el, "border: 10px double");
    doc.flush();
    assert_eq!(doc.value(el, "border-top-width"), "10px");
    assert_eq!(
        doc.value(el, "border-top-color"),
        "rgb(1, 2, 3)",
        "omitted border color is currentColor"
    );

    // Style-only shorthand: width resets to medium (3px used value).
    doc.set_inline(el, "border: double");
    doc.flush();
    assert_eq!(doc.value(el, "border-top-style"), "double");
    assert_eq!(
        doc.value(el, "border-top-width"),
        "3px",
        "initial width is medium = 3px once a style exists"
    );

    assert!(!parses("border", "double double"));
}

// C++: border_handler_unittest.cc::border_side_and_outline_shorthands.
// Border sides are in the supported set; the undocumented outline family is
// intentionally absent from the Lynx author surface.
#[test]
fn per_side_border_shorthands_and_outline_exclusion() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for side in ["border-right", "border-left", "border-top", "border-bottom"] {
        doc.set_inline(el, &format!("{side}: 10px ridge black"));
        doc.flush();
        assert_eq!(doc.value(el, &format!("{side}-width")), "10px");
        assert_eq!(doc.value(el, &format!("{side}-style")), "ridge");
        assert_eq!(doc.value(el, &format!("{side}-color")), "rgb(0, 0, 0)");
    }

    for (name, value) in [
        ("outline", "1px solid red"),
        ("outline-color", "red"),
        ("outline-style", "solid"),
        ("outline-width", "1px"),
        ("outline-offset", "1px"),
    ] {
        assert!(!parses(name, value), "`{name}` must be absent");
    }
}

// C++: border_handler_unittest.cc keyword tables.
#[test]
fn border_style_and_width_keywords() {
    for style in [
        "none", "hidden", "dotted", "dashed", "solid", "double", "groove", "ridge", "inset",
        "outset",
    ] {
        assert_eq!(
            specified("border-top-style", style).as_deref(),
            Some(style),
            "`{style}`"
        );
    }
    assert!(!parses("border-top-style", "invalid"));

    let mut doc = Doc::with_css("view { border-style: solid }");
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, "border-top-width: thin");
    doc.flush();
    assert_eq!(doc.value(el, "border-top-width"), "1px");
    doc.set_inline(el, "border-top-width: 10px");
    doc.flush();
    assert_eq!(doc.value(el, "border-top-width"), "10px");
    assert!(!parses("border-top-width", "invalid"));
}

// C++: border_radius_handler_unittest.cc shorthand expansion + calc.
#[test]
fn border_radius_expansion() {
    // (input, [TL, TR, BR, BL] as "x y" computed pairs)
    let rows: &[(&str, [&str; 4])] = &[
        // Equal x/y radii collapse to one value in serialization.
        ("50%", ["50%", "50%", "50%", "50%"]),
        ("50px 10%", ["50px", "10%", "50px", "10%"]),
        ("50px           10%", ["50px", "10%", "50px", "10%"]),
        ("50px/10%", ["50px 10%", "50px 10%", "50px 10%", "50px 10%"]),
        (
            " 50px   /  10%    ",
            ["50px 10%", "50px 10%", "50px 10%", "50px 10%"],
        ),
        (
            "50px 10px/ 10%",
            ["50px 10%", "10px 10%", "50px 10%", "10px 10%"],
        ),
        (
            "50px 10px/ 10% 20%",
            ["50px 10%", "10px 20%", "50px 10%", "10px 20%"],
        ),
        ("12px 0 /12px 0", ["12px", "0px", "12px", "0px"]),
        (
            "10px 0 /12px",
            ["10px 12px", "0px 12px", "10px 12px", "0px 12px"],
        ),
        // calc folds at computed-value time (length ×/÷ number is valid).
        (
            "calc(12px*3)  3px 6px / 5px calc(20px*2)",
            ["36px 5px", "3px 40px", "6px 5px", "3px 40px"],
        ),
        (
            "calc(20px/5) 3px 6px / calc(2px + 3px) calc(2px + 2px)",
            ["4px 5px", "3px 4px", "6px 5px", "3px 4px"],
        ),
    ];
    let corners = [
        "border-top-left-radius",
        "border-top-right-radius",
        "border-bottom-right-radius",
        "border-bottom-left-radius",
    ];
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for (input, expected) in rows {
        doc.set_inline(el, &format!("border-radius: {input}"));
        doc.flush();
        for (corner, pair) in corners.iter().zip(expected) {
            assert_eq!(&doc.value(el, corner), pair, "`{input}` -> {corner}");
        }
    }
}

// C++: border_radius_handler_unittest.cc longhand forms + invalid rows —
// W3C-corrected: corner longhands take <length-percentage>{1,2}; Lynx's
// compat slash form on a longhand must reject, as must unitless non-zero
// numbers (no enable_length_unit_check=false leniency).
#[test]
fn border_radius_longhands_and_rejects() {
    assert_eq!(
        computed("border-top-left-radius: 50%", "border-top-left-radius"),
        "50%"
    );
    assert_eq!(
        computed("border-top-left-radius: 50% 10px", "border-top-left-radius"),
        "50% 10px"
    );
    // Slash forms are valid for the SHORTHAND but not for corner longhands
    // (Lynx accepted them on longhands "for compatibility").
    for slashed in ["50%/10px", "50%/calc(10px + 10px)"] {
        assert!(
            !parses("border-top-right-radius", slashed),
            "`{slashed}` must be rejected on a longhand"
        );
        assert!(
            parses("border-radius", slashed),
            "`{slashed}` stays valid on the shorthand"
        );
    }
    for invalid in ["hello", "100test/0", "100test 100/0"] {
        assert!(
            !parses("border-top-right-radius", invalid) && !parses("border-radius", invalid),
            "`{invalid}` must be rejected"
        );
    }
    // W3C-corrected over the C++ calc row: `calc(2px +2px)` has no
    // whitespace after the `+`, so `+2px` is a signed dimension with no
    // operator — invalid per css-values (Lynx's lax calc accepted it).
    assert!(!parses("border-radius", "1px / calc(2px +2px)"));
    assert!(parses("border-radius", "0/0"), "unitless zero is a length");
    assert!(parses("border-radius", "0 /     0"));
    assert!(
        !parses("border-radius", "100/0"),
        "bare non-zero numbers are not lengths"
    );
}

// C++: shadow_handler_unittest.cc box-shadow cases.
#[test]
fn box_shadow_grammar() {
    let full = computed("box-shadow: 1px 2px 3px 4px red", "box-shadow");
    for fragment in ["1px", "2px", "3px", "4px", "rgb(255, 0, 0)"] {
        assert!(full.contains(fragment), "`{fragment}` in: {full}");
    }
    assert_eq!(
        computed(
            "box-shadow: 1px 2px 3px rgb(255, 0, 0), 4px 5px 6px blue",
            "box-shadow",
        )
        .matches("rgb(")
        .count(),
        2,
        "two layers in order"
    );
    for none in ["none", "NONE", "none "] {
        assert_eq!(
            computed(&format!("box-shadow: {none}"), "box-shadow"),
            "none"
        );
    }
    for inset in ["1px 2px 3px inset red", "inset 1px 2px 3px red"] {
        assert!(
            computed(&format!("box-shadow: {inset}"), "box-shadow").contains("inset"),
            "`{inset}` must retain the standard inset keyword"
        );
    }
    for invalid in [
        "none,",
        "1px 2px 3px inset red,",
        "inset 1px 2px 3px inset red",
        "1px red",
        "red",
    ] {
        assert!(
            !parses("box-shadow", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

// C++: shadow_handler_unittest.cc text-shadow rows — no inset, no spread.
#[test]
fn text_shadow_grammar() {
    let valid = computed("text-shadow: 1px 2px 3px red", "text-shadow");
    for fragment in ["1px", "2px", "3px", "rgb(255, 0, 0)"] {
        assert!(valid.contains(fragment), "`{fragment}` in: {valid}");
    }
    for invalid in [
        "1px 2px 3px inset red",
        "inset 1px 2px 3px red",
        "1px 2px 3px 4px red",
    ] {
        assert!(
            !parses("text-shadow", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

// C++: text_stroke_handler_unittest.cc — W3C-corrected: the `||` grammar
// makes single-value forms valid (Lynx rejected them while still emitting
// empty longhands), and there is no `none` value.
#[test]
fn text_stroke_grammar() {
    let mut doc = Doc::with_css("view { color: rgb(9, 9, 9) }");
    let el = doc.el(doc.root, "view");

    doc.set_inline(el, "text-stroke: 1px yellow");
    doc.flush();
    assert_eq!(doc.value(el, "text-stroke-width"), "1px");
    assert_eq!(doc.value(el, "text-stroke-color"), "rgb(255, 255, 0)");

    // Order-irrelevant + whitespace-tolerant.
    doc.set_inline(el, "         yellow         1px       ");
    doc.set_inline(el, "text-stroke:         yellow         1px       ");
    doc.flush();
    assert_eq!(doc.value(el, "text-stroke-width"), "1px");
    assert_eq!(doc.value(el, "text-stroke-color"), "rgb(255, 255, 0)");

    // Single-value forms are valid; the other longhand resets to initial
    // (width 0, color currentColor).
    doc.set_inline(el, "text-stroke: 1px");
    doc.flush();
    assert_eq!(doc.value(el, "text-stroke-width"), "1px");
    assert_eq!(
        doc.value(el, "text-stroke-color"),
        "rgb(9, 9, 9)",
        "omitted stroke color is currentColor"
    );
    doc.set_inline(el, "text-stroke: yellow");
    doc.flush();
    assert_eq!(doc.value(el, "text-stroke-width"), "0px");
    assert_eq!(doc.value(el, "text-stroke-color"), "rgb(255, 255, 0)");

    // Keep Stylo's upstream <line-width> grammar. There is no Lynx-only
    // text-stroke-width parser: the standard width keywords remain valid.
    for (keyword, computed_width) in [("thin", "1px"), ("medium", "3px"), ("thick", "5px")] {
        doc.set_inline(el, &format!("text-stroke-width: {keyword}"));
        doc.flush();
        assert_eq!(doc.value(el, "text-stroke-width"), computed_width);
    }

    assert!(
        !parses("text-stroke", "none"),
        "no `none` value in the text-stroke grammar"
    );
}

// C++: text_decoration_handler_unittest.cc — line/style/color via `||`;
// thickness rows live with the engine-gap tests in inheritance_computed.rs.
#[test]
fn text_decoration_grammar() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");

    doc.set_inline(el, "text-decoration: none");
    doc.flush();
    assert_eq!(doc.value(el, "text-decoration-line"), "none");

    assert!(
        !parses("text-decoration", "underline line-through"),
        "Lynx accepts exactly one line keyword"
    );
    doc.set_inline(el, "text-decoration: line-through");
    doc.flush();
    assert_eq!(doc.value(el, "text-decoration-line"), "line-through");

    doc.set_inline(el, "text-decoration: yellow dashed underline");
    doc.flush();
    assert_eq!(doc.value(el, "text-decoration-line"), "underline");
    assert_eq!(doc.value(el, "text-decoration-style"), "dashed");
    assert_eq!(doc.value(el, "text-decoration-color"), "rgb(255, 255, 0)");

    // `none` cannot combine with other values; Lynx silently dropped the
    // trailing token instead (W3C-corrected to a rejection).
    assert!(!parses("text-decoration", "none 2px"));
    assert!(!parses("text-decoration", "underline 1px 2px"));
}

// C++: filter_handler_unittest.cc — plus the multi-function chain Lynx's
// parser could not handle (W3C-corrected addition).
#[test]
fn filter_grammar() {
    assert_eq!(
        computed("filter: grayscale(80%)", "filter"),
        "grayscale(0.8)",
        "percentage amounts compute to numbers"
    );
    assert_eq!(computed("filter: blur(20px)", "filter"), "blur(20px)");
    assert_eq!(
        computed("filter: grayscale(0.28)", "filter"),
        "grayscale(0.28)"
    );
    assert_eq!(
        computed("filter: blur(2px) grayscale(50%)", "filter"),
        "blur(2px) grayscale(0.5)",
        "multi-function chains are valid per filter-effects-1"
    );
    for invalid in ["grays(1", "blur(20)", "grayscale(-1)"] {
        assert!(!parses("filter", invalid), "`{invalid}` must be rejected");
    }
}

// Skipped (skip-internal): every lepus non-string/type-guard row (numbers,
// bools, empty lepus::Value()) — no CSS-text analog.
// Skipped (skip-legacy): border_handler enable_new_border_handler=false OLD
// default-fill behavior and border_radius/flex enable_length_unit_check
// leniency — pre-W3C pipelines out of scope under enableCSSSelector=true.
