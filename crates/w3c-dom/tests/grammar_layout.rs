//! Flex / grid / aspect-ratio / four-sides layout-property grammar — ported
//! from `lynx/core/renderer/css/parser/flex_handler_unittest.cc`,
//! `flex_flow_handler_unittest.cc`, `grid_shorthand_handler_unittest.cc`,
//! `grid_template_handler_unittest.cc`, `aspect_ratio_handler_unittest.cc`,
//! `list_gap_handler_unittest.cc`, and
//! `four_sides_shorthand_handler_unittest.cc`.
//!
//! Scope: `enableCSSSelector = true` / `enableRemoveCSSScope = true`. W3C
//! corrections: `flex: <number>` gives `flex-basis: 0%`; unitless non-zero
//! lengths reject everywhere (flex-basis, margins, radii); grid placement
//! shorthands always parse (no `enable_grid_placement_shorthands` gate) with
//! spans encoded on the start/end longhands (no deprecated *-span
//! properties); `span -2` rejects.

mod common;

use common::{Doc, parses, specified};
use w3c_dom::property_is_supported;

/// Computed value of `property` after applying `declaration` inline.
fn computed(declaration: &str, property: &str) -> String {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, declaration);
    doc.flush();
    doc.value(el, property)
}

// C++: flex_handler_unittest.cc (all portable cases).
#[test]
fn flex_shorthand_grammar() {
    // (input, grow, shrink, basis)
    let rows: &[(&str, &str, &str, &str)] = &[
        ("2", "2", "1", "0%"),
        ("20px", "1", "1", "20px"),
        ("3 100px", "3", "1", "100px"),
        ("2 3", "2", "3", "0%"),
        ("2 3 10%", "2", "3", "10%"),
        ("10% 2 3", "2", "3", "10%"), // basis-first ordering
        ("2 3 0", "2", "3", "0px"),
        ("1 0 100px", "1", "0", "100px"),
    ];
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for &(input, grow, shrink, basis) in rows {
        doc.set_inline(el, &format!("flex: {input}"));
        doc.flush();
        assert_eq!(doc.value(el, "flex-grow"), grow, "`{input}` grow");
        assert_eq!(doc.value(el, "flex-shrink"), shrink, "`{input}` shrink");
        assert_eq!(doc.value(el, "flex-basis"), basis, "`{input}` basis");
    }
    // W3C-corrected: a unitless non-zero flex-basis is invalid.
    assert!(!parses("flex", "2 3 5"));
    assert!(!parses("flex", "10 2 3"));
    assert!(!parses("flex", "hello"));
}

// C++: flex_flow_handler_unittest.cc (all four tests). The shorthand resets
// the omitted longhand to its initial value.
#[test]
fn flex_flow_grammar() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for direction in ["row", "row-reverse", "column", "column-reverse"] {
        doc.set_inline(el, &format!("flex-flow: {direction}"));
        doc.flush();
        assert_eq!(doc.value(el, "flex-direction"), direction);
        assert_eq!(doc.value(el, "flex-wrap"), "nowrap", "wrap resets");
    }
    for wrap in ["nowrap", "wrap", "wrap-reverse"] {
        doc.set_inline(el, &format!("flex-flow: {wrap}"));
        doc.flush();
        assert_eq!(doc.value(el, "flex-wrap"), wrap);
        assert_eq!(doc.value(el, "flex-direction"), "row", "direction resets");
    }
    for (input, direction, wrap) in [
        ("row nowrap", "row", "nowrap"),
        (
            "column-reverse wrap-reverse",
            "column-reverse",
            "wrap-reverse",
        ),
        (
            "wrap-reverse column-reverse ",
            "column-reverse",
            "wrap-reverse",
        ),
    ] {
        doc.set_inline(el, &format!("flex-flow: {input}"));
        doc.flush();
        assert_eq!(doc.value(el, "flex-direction"), direction, "`{input}`");
        assert_eq!(doc.value(el, "flex-wrap"), wrap, "`{input}`");
    }
    for invalid in ["invalid", "row row", "wrap wrap", "column row"] {
        assert!(
            !parses("flex-flow", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

// C++: grid_shorthand_handler_unittest.cc — W3C-corrected throughout: the
// shorthands always parse; spans live on the start/end longhands.
#[test]
fn grid_placement_shorthands() {
    // (shorthand declaration, start longhand, start value, end longhand,
    //  end value)
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        (
            "grid-column: auto",
            "grid-column-start",
            "auto",
            "grid-column-end",
            "auto",
        ),
        (
            "grid-row: auto",
            "grid-row-start",
            "auto",
            "grid-row-end",
            "auto",
        ),
        (
            "grid-column: 3",
            "grid-column-start",
            "3",
            "grid-column-end",
            "auto",
        ),
        ("grid-row: 2", "grid-row-start", "2", "grid-row-end", "auto"),
        (
            "grid-column: span 2",
            "grid-column-start",
            "span 2",
            "grid-column-end",
            "auto",
        ),
        (
            "grid-column: 1 / 4",
            "grid-column-start",
            "1",
            "grid-column-end",
            "4",
        ),
        (
            "grid-column:   1  /  4  ",
            "grid-column-start",
            "1",
            "grid-column-end",
            "4",
        ),
        (
            "grid-column: span 2 / 5",
            "grid-column-start",
            "span 2",
            "grid-column-end",
            "5",
        ),
        (
            "grid-column: 2 / span 3",
            "grid-column-start",
            "2",
            "grid-column-end",
            "span 3",
        ),
        (
            "grid-row: 1 / span 2",
            "grid-row-start",
            "1",
            "grid-row-end",
            "span 2",
        ),
    ];
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for &(declaration, start, start_value, end, end_value) in rows {
        doc.set_inline(el, declaration);
        doc.flush();
        assert_eq!(doc.value(el, start), start_value, "`{declaration}`");
        assert_eq!(doc.value(el, end), end_value, "`{declaration}`");
    }
    for invalid in ["1 / 2 / 3", " / 2", "1 / ", "", "span -2"] {
        assert!(
            !parses("grid-column", invalid),
            "`grid-column: {invalid}` must be rejected"
        );
    }
}

// C++: grid_template_handler_unittest.cc — track lists asserted through
// computed serialization; rpx (Lynx unit, fork-native) participates.
#[test]
fn grid_template_tracks() {
    assert_eq!(
        computed("grid-template-rows: 2px", "grid-template-rows"),
        "2px"
    );
    assert_eq!(
        computed("grid-template-rows: 2px auto", "grid-template-rows"),
        "2px auto"
    );
    assert!(parses("grid-template-rows", "2rpx auto auto"));
    assert_eq!(
        computed(
            "grid-template-rows: 1fr 0.2fr auto 2fr",
            "grid-template-rows"
        ),
        "1fr 0.2fr auto 2fr"
    );

    // Integer repeat() forms parse; the computed value PRESERVES the
    // repeat() structure (expansion to concrete tracks is the layout
    // engine's track-resolution step, not a computed-value operation —
    // Lynx materialized the expansion at parse time instead).
    for (input, serialized) in [
        ("2px repeat(2, auto)", "2px repeat(2, auto)"),
        ("2px repeat(1, auto 100px)", "2px repeat(1, auto 100px)"),
        ("2px repeat(2, auto 100px)", "2px repeat(2, auto 100px)"),
        ("repeat(1, 100px)", "repeat(1, 100px)"),
    ] {
        assert_eq!(
            computed(
                &format!("grid-template-rows: {input}"),
                "grid-template-rows"
            ),
            serialized,
            "`{input}`"
        );
    }
    assert!(parses(
        "grid-template-rows",
        "repeat(2, auto 100px) 2px auto"
    ));
    assert!(parses(
        "grid-template-rows",
        "auto repeat(2, auto) 120rpx repeat(2, 100vh)"
    ));

    // Whitespace-glued dimensions must tokenize per CSS Syntax:
    // `100pxrepeat(...)` is one (unknown) dimension token — reject. Two
    // adjacent function tokens remain distinct component values — accept.
    assert!(parses(
        "grid-template-rows",
        "repeat(1, 100px)repeat(1, 100px)"
    ));
    assert!(!parses(
        "grid-template-rows",
        "repeat(1,100px)100pxrepeat(1,100px)"
    ));

    // calc and minmax/fit-content forms.
    for valid in [
        "calc(2px + 3rpx)",
        "calc(2px + 3rpx) calc(100px + (2vh - 100px))",
        "calc(2px + (1px - 3rpx)) 100rpx 20vw",
        "minmax(max-content, calc(10px + 0.5em)) minmax(auto, 4%) fit-content(calc(10px + 0.5em))",
        "repeat(2, minmax(max-content, calc(10px + 0.5em))) 1fr auto repeat(1, minmax(calc(100px + 10vw), auto))",
    ] {
        assert!(parses("grid-template-rows", valid), "`{valid}` must parse");
    }
    // W3C-corrected over the C++ accept row: fit-content() is not a valid
    // minmax() argument per css-grid-1 (Lynx's parser over-accepted it).
    assert!(!parses(
        "grid-template-rows",
        "repeat(1, minmax(fit-content(100px), 2fr))"
    ));

    // W3C-corrected: the empty string is not a declaration; `none` is the
    // keyword form of the initial value.
    assert!(!parses("grid-template-rows", ""));
    assert_eq!(
        computed("grid-template-rows: none", "grid-template-rows"),
        "none"
    );
}

// C++: aspect_ratio_handler_unittest.cc — W3C-corrected: ratios are kept
// as number pairs (not pre-divided floats) and negatives reject.
#[test]
fn aspect_ratio_grammar() {
    assert_eq!(
        specified("aspect-ratio", "10/100").as_deref(),
        Some("10 / 100")
    );
    assert_eq!(
        specified("aspect-ratio", "10").as_deref(),
        Some("10 / 1"),
        "a bare number is a ratio with an explicit /1 denominator"
    );
    assert_eq!(
        specified("aspect-ratio", "0.25").as_deref(),
        Some("0.25 / 1")
    );
    assert!(
        !parses("aspect-ratio", "-0.75"),
        "negative ratios are invalid (Lynx accepted them)"
    );
    assert!(!parses("aspect-ratio", ""));
}

// C++: list_gap_handler_unittest.cc — `list-main-axis-gap` is a Lynx-only
// <list>-component property not present in the fork's surface; pinned so a
// fork addition is noticed and the px-only grammar rows get ported.
#[test]
fn list_main_axis_gap_is_absent() {
    assert!(
        !property_is_supported("list-main-axis-gap"),
        "list-main-axis-gap appeared — port the Lynx-faithful px-only rows"
    );
    // The standard `gap` family (which Lynx also exposes) parses normally.
    assert_eq!(specified("row-gap", "20px").as_deref(), Some("20px"));
    assert_eq!(specified("column-gap", "5%").as_deref(), Some("5%"));
    assert_eq!(specified("gap", "20px 5%").as_deref(), Some("20px 5%"));
}

// C++: four_sides_shorthand_handler_unittest.cc margin/padding/border-*.
#[test]
fn four_sides_shorthand_expansion() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    // (input, [top, right, bottom, left])
    let margin_rows: &[(&str, [&str; 4])] = &[
        ("2px", ["2px", "2px", "2px", "2px"]),
        ("2px 3px", ["2px", "3px", "2px", "3px"]),
        ("2px 3px 4px", ["2px", "3px", "4px", "3px"]),
        ("2px 3px 4px 5px", ["2px", "3px", "4px", "5px"]),
        (" 2px  3px    4px     5px  ", ["2px", "3px", "4px", "5px"]),
        // em = 16px default font, rem = 16px root font.
        ("2px 3em 4rem 5px", ["2px", "48px", "64px", "5px"]),
    ];
    for (input, expected) in margin_rows {
        doc.set_inline(el, &format!("margin: {input}"));
        doc.flush();
        for (side, value) in ["top", "right", "bottom", "left"].iter().zip(expected) {
            assert_eq!(
                &doc.value(el, &format!("margin-{side}")),
                value,
                "`{input}`"
            );
        }
    }
    // rpx resolves against the 750-based viewport policy at computed time;
    // grammar acceptance is what this row pins at the w3c-dom layer.
    assert!(parses("margin", "2px 3em 4rem 5rpx"));

    // W3C-corrected: unitless non-zero components reject the declaration.
    assert!(!parses("margin", "2% 3"));
    for invalid in ["2test", "test"] {
        assert!(!parses("margin", invalid), "`{invalid}` must be rejected");
    }

    doc.set_inline(el, "padding: 2px");
    doc.flush();
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(doc.value(el, &format!("padding-{side}")), "2px");
    }

    doc.set_inline(el, "border-style: solid; border-width: 2px");
    doc.flush();
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(doc.value(el, &format!("border-{side}-width")), "2px");
    }

    doc.set_inline(el, "border-style: solid dashed dotted double");
    doc.flush();
    assert_eq!(doc.value(el, "border-top-style"), "solid");
    assert_eq!(doc.value(el, "border-right-style"), "dashed");
    assert_eq!(doc.value(el, "border-bottom-style"), "dotted");
    assert_eq!(doc.value(el, "border-left-style"), "double");

    doc.set_inline(el, "border-style: groove ridge inset outset");
    doc.flush();
    assert_eq!(doc.value(el, "border-top-style"), "groove");
    assert_eq!(doc.value(el, "border-right-style"), "ridge");
    assert_eq!(doc.value(el, "border-bottom-style"), "inset");
    assert_eq!(doc.value(el, "border-left-style"), "outset");

    doc.set_inline(el, "border-style: hidden none");
    doc.flush();
    assert_eq!(doc.value(el, "border-top-style"), "hidden");
    assert_eq!(doc.value(el, "border-right-style"), "none");
    assert_eq!(doc.value(el, "border-bottom-style"), "hidden");
    assert_eq!(doc.value(el, "border-left-style"), "none");
    assert!(!parses("border-style", "notstyle"));

    doc.set_inline(
        el,
        "border-style: solid; border-color: red #00ff00 #00ff00ee rgb(0, 0, 255)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "border-top-color"), "rgb(255, 0, 0)");
    assert_eq!(doc.value(el, "border-right-color"), "rgb(0, 255, 0)");
    assert!(
        doc.value(el, "border-bottom-color")
            .starts_with("rgba(0, 255, 0, 0.9"),
        "#RRGGBBAA reads alpha last: {}",
        doc.value(el, "border-bottom-color")
    );
    assert_eq!(doc.value(el, "border-left-color"), "rgb(0, 0, 255)");
}

// Skipped (skip-internal): lepus non-string/bool type-guard rows and the
// enable_parse_int_flex numeric-input quirk — no CSS-text analog.
// Skipped (skip-legacy): the deprecated legacy flex handler and the
// enable_grid_placement_shorthands gate-off path — pre-W3C pipelines.
