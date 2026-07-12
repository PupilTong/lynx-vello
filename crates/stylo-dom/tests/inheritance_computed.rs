//! Inheritance and computed-value semantics — ported from
//! `lynx/core/renderer/css/computed_css_style_unittest.cc` and
//! `computed_css_style_css_text_helper_unittest.cc`.
//!
//! Scope: `enableCSSSelector = true` / `enableRemoveCSSScope = true`.
//! Inheritance policy is web-core parity (deviations.md): stylo's always-on
//! W3C inheritance — each property's intrinsic `inherited` flag — replaces
//! native Lynx's `enableCSSInheritance` allowlist, with identical observable
//! results for the properties exercised here. Custom properties inherit
//! unconditionally in both worlds.

mod common;

use common::{Doc, parses, rgb, specified};
use stylo_dom::property_is_supported;

// C++: ComputedCSSStyleTest inherit-helper cases (W3C-corrected: intrinsic
// inherited-flags instead of an explicit allowlist; same outcomes).
#[test]
fn inherited_and_non_inherited_properties() {
    let mut doc = Doc::with_css(
        ".parent { font-size: 20px; direction: rtl; opacity: 0.3; color: rgb(1, 2, 3); }",
    );
    let parent = doc.el(doc.root, "view.parent");
    let child = doc.el(parent, "view");
    let grandchild = doc.el(child, "text");
    doc.flush();

    for id in [child, grandchild] {
        assert_eq!(doc.value(id, "font-size"), "20px", "font-size inherits");
        assert_eq!(doc.value(id, "direction"), "rtl", "direction inherits");
        assert_eq!(doc.color(id), rgb(1, 2, 3), "color inherits");
        assert_eq!(doc.value(id, "opacity"), "1", "opacity does not inherit");
    }
}

// C++: ComputedCSSStyleTest.FinalizeCustomPropertiesResolvesVariables +
// CopyFromPreservesComputedCssStateIndependently — modeled as sibling
// subtrees whose --base differs; dependent --accent re-resolves per subtree
// with no aliasing.
#[test]
fn custom_properties_resolve_and_inherit_independently() {
    let mut doc = Doc::with_css(
        ".blue { --base: blue; } .green { --base: green; } \
         view { --accent: var(--base); } \
         .probe { color: var(--accent); }",
    );
    let blue_parent = doc.el(doc.root, "view.blue");
    let blue_probe = doc.el(blue_parent, "view.probe");
    let green_parent = doc.el(doc.root, "view.green");
    let green_probe = doc.el(green_parent, "view.probe");
    doc.flush();

    assert_eq!(doc.color(blue_probe), rgb(0, 0, 255));
    assert_eq!(doc.color(green_probe), rgb(0, 128, 0));
}

// C++: ComputedCSSStyleCSSTextHelperTest.direction_css_text — kNormal/
// kLynxRtl are Lynx-internal enum states; the author-facing surface is
// ltr/rtl with initial ltr.
#[test]
fn direction_computed_values() {
    let mut doc = Doc::with_css(".rtl { direction: rtl } .ltr { direction: ltr }");
    let plain = doc.el(doc.root, "view");
    let rtl = doc.el(doc.root, "view.rtl");
    let ltr = doc.el(rtl, "view.ltr");
    let inherited = doc.el(rtl, "view");
    doc.flush();
    assert_eq!(doc.value(plain, "direction"), "ltr", "initial is ltr");
    assert_eq!(doc.value(rtl, "direction"), "rtl");
    assert_eq!(doc.value(ltr, "direction"), "ltr", "explicit ltr under rtl");
    assert_eq!(doc.value(inherited, "direction"), "rtl", "inherits");
}

// C++: ComputedCSSStyleCSSTextHelperTest.background_position_one_value_syntax
// — the missing axis defaults to center (50%).
#[test]
fn background_position_one_value_syntax() {
    let rows: &[(&str, &str, &str)] = &[
        ("top", "50%", "0%"),
        ("bottom", "50%", "100%"),
        ("left", "0%", "50%"),
        ("right", "100%", "50%"),
        ("center", "50%", "50%"),
        ("25%", "25%", "50%"),
        ("25px", "25px", "50%"),
    ];
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.flush();
    assert_eq!(doc.value(el, "background-position-x"), "0%", "initial x");
    assert_eq!(doc.value(el, "background-position-y"), "0%", "initial y");
    for &(input, x, y) in rows {
        doc.set_inline(el, &format!("background-position: {input}"));
        doc.flush();
        assert_eq!(doc.value(el, "background-position-x"), x, "`{input}` x");
        assert_eq!(doc.value(el, "background-position-y"), y, "`{input}` y");
    }
}

// C++: ComputedCSSStyleCSSTextHelperTest.background_position_two_value_syntax
// — all 30 rows; edge keywords are order-independent across axes, units are
// preserved.
#[test]
#[allow(clippy::too_many_lines)]
fn background_position_two_value_syntax() {
    let rows: &[(&str, &str, &str)] = &[
        ("top left", "0%", "0%"),
        ("left top", "0%", "0%"),
        ("bottom left", "0%", "100%"),
        ("left bottom", "0%", "100%"),
        ("top right", "100%", "0%"),
        ("right top", "100%", "0%"),
        ("bottom right", "100%", "100%"),
        ("right bottom", "100%", "100%"),
        ("center center", "50%", "50%"),
        ("center top", "50%", "0%"),
        ("top center", "50%", "0%"),
        ("center bottom", "50%", "100%"),
        ("bottom center", "50%", "100%"),
        ("center right", "100%", "50%"),
        ("right center", "100%", "50%"),
        ("center left", "0%", "50%"),
        ("left center", "0%", "50%"),
        ("25% center", "25%", "50%"),
        ("25px center", "25px", "50%"),
        ("25% top", "25%", "0%"),
        ("25px top", "25px", "0%"),
        ("25% bottom", "25%", "100%"),
        ("25px bottom", "25px", "100%"),
        ("center 25%", "50%", "25%"),
        ("center 25px", "50%", "25px"),
        ("left 25%", "0%", "25%"),
        ("left 25px", "0%", "25px"),
        ("right 25%", "100%", "25%"),
        ("right 25px", "100%", "25px"),
        ("25px 25%", "25px", "25%"),
    ];
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for &(input, x, y) in rows {
        doc.set_inline(el, &format!("background-position: {input}"));
        doc.flush();
        assert_eq!(doc.value(el, "background-position-x"), x, "`{input}` x");
        assert_eq!(doc.value(el, "background-position-y"), y, "`{input}` y");
    }
    // A vertical/horizontal keyword pair in the wrong slots without being
    // reorderable is invalid: `top bottom` never parses.
    assert!(!parses("background-position", "top bottom"));
    assert!(!parses("background-position", "left right"));
}

// C++: ComputedCSSStyleCSSTextHelperTest.background_position_multi_
// background_images — W3C-corrected: per css-backgrounds-3 the COMPUTED list
// is the specified list; repeating it across N image layers is a used-value
// (paint-time) operation, not a computed-value one. Lynx materialized the
// repetition in computed style.
#[test]
fn background_position_list_stays_specified_length() {
    let mut doc = Doc::with_css(
        ".multi { background-image: url(a.png), url(b.png), url(c.png), url(d.png); \
                  background-position: top left, right bottom; }",
    );
    let el = doc.el(doc.root, "view.multi");
    doc.flush();
    assert_eq!(
        doc.value(el, "background-position-x"),
        "0%, 100%",
        "computed x list keeps the specified length (repetition is paint-time)"
    );
    assert_eq!(doc.value(el, "background-position-y"), "0%, 100%");
    assert_eq!(
        doc.value(el, "background-image").matches("url(").count(),
        4,
        "all four image layers survive"
    );
}

// C++: ComputedCSSStyleCSSTextHelperTest.border_width_css_text — computed
// border width is 0 when border-style is none (Lynx flag-off default ==
// stylo == W3C used-value rule applied at computed time). The
// css_align_with_legacy_w3c_ 3px branch is native-only config, not ported.
#[test]
fn border_widths_require_a_border_style() {
    let mut doc = Doc::with_css(
        ".widths { border-width: 10px 20px 30px 40px; } \
         .solid { border-style: solid; }",
    );
    let bare = doc.el(doc.root, "view.widths");
    let solid = doc.el(doc.root, "view.widths.solid");
    doc.flush();
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(
            doc.value(bare, &format!("border-{side}-width")),
            "0px",
            "style:none zeroes {side} width"
        );
    }
    assert_eq!(doc.value(solid, "border-top-width"), "10px");
    assert_eq!(doc.value(solid, "border-right-width"), "20px");
    assert_eq!(doc.value(solid, "border-bottom-width"), "30px");
    assert_eq!(doc.value(solid, "border-left-width"), "40px");

    // Fractional widths: computed border widths snap DOWN to whole device
    // pixels (minimum one device pixel for nonzero values) — Gecko/stylo
    // behavior, intentionally diverging from native Lynx's raw floats. At
    // DPR 1 the C++ float rows (2.5/3.75/1.25/0.5) land on whole px:
    doc.set_inline(solid, "border-width: 2.5px 3.75px 1.25px 0.5px");
    doc.flush();
    assert_eq!(doc.value(solid, "border-top-width"), "2px");
    assert_eq!(doc.value(solid, "border-right-width"), "3px");
    assert_eq!(doc.value(solid, "border-bottom-width"), "1px");
    assert_eq!(
        doc.value(solid, "border-left-width"),
        "1px",
        "nonzero widths never snap to zero"
    );

    // On a DPR-2 device half-pixel granularity survives, so the fractional
    // intent of the C++ rows is still observable.
    let mut hidpi = Doc::with_device(common::device_with(
        800.0,
        600.0,
        2.0,
        stylo::queries::values::PrefersColorScheme::Light,
    ));
    hidpi.add_css(".s { border-style: solid; }");
    let el = hidpi.el(hidpi.root, "view.s");
    hidpi.set_inline(el, "border-width: 2.5px 3.75px 1.25px 0.5px");
    hidpi.flush();
    assert_eq!(hidpi.value(el, "border-top-width"), "2.5px");
    assert_eq!(hidpi.value(el, "border-right-width"), "3.5px");
    assert_eq!(hidpi.value(el, "border-bottom-width"), "1px");
    assert_eq!(hidpi.value(el, "border-left-width"), "0.5px");
}

// C++: ComputedCSSStyleCSSTextHelperTest border-color cases — W3C-corrected:
// the initial border color is `currentColor` (resolving to the computed
// `color`), not Lynx's hardcoded black. Exposed with a non-black color.
#[test]
fn border_color_defaults_to_current_color() {
    let mut doc = Doc::with_css(
        ".c { color: rgb(10, 20, 30); border-style: solid; } \
         .explicit { border-top-color: #ff0000; border-right-color: #0000ff; \
                     border-bottom-color: #00ff00; border-left-color: #ffffff; }",
    );
    let current = doc.el(doc.root, "view.c");
    let explicit = doc.el(doc.root, "view.c.explicit");
    doc.flush();
    for side in ["top", "right", "bottom", "left"] {
        assert_eq!(
            doc.value(current, &format!("border-{side}-color")),
            "rgb(10, 20, 30)",
            "unset border-{side}-color resolves through currentColor"
        );
    }
    assert_eq!(doc.value(explicit, "border-top-color"), "rgb(255, 0, 0)");
    assert_eq!(doc.value(explicit, "border-right-color"), "rgb(0, 0, 255)");
    assert_eq!(doc.value(explicit, "border-bottom-color"), "rgb(0, 255, 0)");
    assert_eq!(
        doc.value(explicit, "border-left-color"),
        "rgb(255, 255, 255)"
    );
}

// C++: ComputedCSSStyleTest text-decoration extension cases — the whole
// thickness family is an ENGINE GAP today: the vendored servo build
// implements no `text-decoration-thickness` longhand at all (gecko-only
// upstream), and the Lynx-only `-x-text-decoration-width`/`-x-…-gap`
// longhands are not in the fork's property surface. Spec-correct
// expectations kept below under #[ignore]; this live test pins the current
// absence so any change is noticed.
#[test]
fn text_decoration_thickness_family_is_absent() {
    for missing in [
        "text-decoration-thickness",
        "-x-text-decoration-width",
        "-x-text-decoration-gap",
    ] {
        assert!(
            !property_is_supported(missing),
            "{missing} appeared in the property surface — unignore \
             `text_decoration_thickness_values` and port the C++ rows"
        );
    }
    // Without a thickness longhand the shorthand grammar has no <length>
    // slot, so a thickness-bearing shorthand drops entirely (which also
    // matches the C++ negative-thickness rejection row).
    assert!(!parses("text-decoration", "underline 2px"));
    assert!(!parses("text-decoration", "underline -1px"));
}

// C++: ComputedCSSStyleTest.StoresTextDecorationExtensionValues +
// AppliesTextDecorationThicknessFromShorthand — spec-correct expectations,
// blocked on the engine gap above.
#[test]
#[ignore = "engine-gap: no text-decoration-thickness longhand in the servo build (gecko-only upstream)"]
fn text_decoration_thickness_values() {
    assert_eq!(
        specified("text-decoration-thickness", "2px").as_deref(),
        Some("2px")
    );
    assert!(
        specified("text-decoration-thickness", "-1px").is_none(),
        "negative thickness is rejected"
    );
    assert_eq!(
        specified("text-decoration-thickness", "0px").as_deref(),
        Some("0px")
    );

    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, "text-decoration: underline 2px");
    doc.flush();
    assert_eq!(doc.value(el, "text-decoration-line"), "underline");
    assert_eq!(doc.value(el, "text-decoration-thickness"), "2px");
    doc.set_inline(el, "text-decoration: underline");
    doc.flush();
    assert_eq!(doc.value(el, "text-decoration-thickness"), "auto");
}

// C++: RejectsInvalidTextDecorationThicknessInShorthand — the observable
// half that works without the thickness longhand: invalid shorthands drop
// entirely, valid line-only shorthands apply.
#[test]
fn text_decoration_shorthand_applies_line() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, "text-decoration: underline");
    doc.flush();
    assert_eq!(doc.value(el, "text-decoration-line"), "underline");

    doc.set_inline(el, "text-decoration: overline -1px");
    doc.flush();
    assert_eq!(
        doc.value(el, "text-decoration-line"),
        "none",
        "invalid shorthand drops entirely (no partial application)"
    );
}

// Skipped (skip-internal): resolved-values map bookkeeping, dirty/reset
// bitsets, IsPlatformProperty categorization, CanonicalComputedValue
// variants and transition extraction, Lepus serialization helpers
// (FloatToPixelString, Uint32ToRGBString, ConcatStrings…), and the
// GetComputedStyleByPropertyID router — native data-structure plumbing.
// Skipped (skip-out-of-scope): width/height/four-sides/padding/margin/
// border-radius/filter CSSText — they read layout_result (layout output),
// not cascade output; layout serialization belongs to the layout engine.
// Skipped (skip-out-of-scope): canonical transition property surface — the
// animation runtime lands with the render engine (§C.11/§C.12).
