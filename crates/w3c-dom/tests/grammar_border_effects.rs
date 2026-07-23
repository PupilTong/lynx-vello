//! Border, outline, shadow, text-stroke/decoration, and filter grammar —
//! ported from `lynx/core/renderer/css/parser/border_handler_unittest.cc`,
//! `border_radius_handler_unittest.cc`, `shadow_handler_unittest.cc`,
//! `text_stroke_handler_unittest.cc`, `text_decoration_handler_unittest.cc`,
//! and `filter_handler_unittest.cc`.

mod common;

use common::{Doc, parses, rgb, specified};
use w3c_dom::property_is_supported;

fn computed(declaration: &str, property: &str) -> String {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, declaration);
    doc.flush();
    doc.value(el, property)
}

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

    doc.set_inline(el, "border: 10px double");
    doc.flush();
    assert_eq!(doc.value(el, "border-top-width"), "10px");
    assert_eq!(
        doc.value(el, "border-top-color"),
        "rgb(1, 2, 3)",
        "omitted border color is currentColor"
    );

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

#[test]
fn per_side_border_shorthands() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    for side in ["border-right", "border-left", "border-top", "border-bottom"] {
        doc.set_inline(el, &format!("{side}: 10px ridge black"));
        doc.flush();
        assert_eq!(doc.value(el, &format!("{side}-width")), "10px");
        assert_eq!(doc.value(el, &format!("{side}-style")), "ridge");
        assert_eq!(doc.value(el, &format!("{side}-color")), "rgb(0, 0, 0)");
    }
}

#[test]
fn outline_is_absent() {
    for property in ["outline", "outline-width", "outline-style", "outline-color"] {
        assert!(
            !property_is_supported(property),
            "`{property}` grew grammar support — port the outline rows"
        );
    }
}

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

#[test]
fn border_radius_expansion() {
    let rows: &[(&str, [&str; 4])] = &[
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
    assert!(!parses("border-radius", "1px / calc(2px +2px)"));
    assert!(parses("border-radius", "0/0"), "unitless zero is a length");
    assert!(parses("border-radius", "0 /     0"));
    assert!(
        !parses("border-radius", "100/0"),
        "bare non-zero numbers are not lengths"
    );
}

#[test]
fn box_shadow_grammar() {
    let full = computed("box-shadow: 1px 2px 3px 4px red", "box-shadow");
    for fragment in ["1px", "2px", "3px", "4px", "rgb(255, 0, 0)"] {
        assert!(full.contains(fragment), "`{fragment}` in: {full}");
    }
    let inset = computed("box-shadow: 1px 2px 3px inset rgb(255, 0, 0)", "box-shadow");
    assert!(inset.contains("inset"), "inset kept: {inset}");
    let leading = computed("box-shadow: inset 1px 2px 3px red", "box-shadow");
    assert!(leading.contains("inset"), "leading inset kept: {leading}");
    assert_eq!(
        computed(
            "box-shadow: 1px 2px 3px inset rgb(255, 0, 0), 1px 2px 3px inset red",
            "box-shadow",
        )
        .matches("inset")
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

#[test]
fn text_stroke_grammar() {
    let mut doc = Doc::with_css("view { color: rgb(9, 9, 9) }");
    let el = doc.el(doc.root, "view");

    doc.set_inline(el, "text-stroke: 1px yellow");
    doc.flush();
    assert_eq!(doc.value(el, "text-stroke-width"), "1px");
    assert_eq!(doc.value(el, "text-stroke-color"), "rgb(255, 255, 0)");

    doc.set_inline(el, "         yellow         1px       ");
    doc.set_inline(el, "text-stroke:         yellow         1px       ");
    doc.flush();
    assert_eq!(doc.value(el, "text-stroke-width"), "1px");
    assert_eq!(doc.value(el, "text-stroke-color"), "rgb(255, 255, 0)");

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

    assert!(
        !parses("text-stroke", "none"),
        "no `none` value in the text-stroke grammar"
    );
    assert!(
        !property_is_supported("-webkit-text-stroke"),
        "the prefixed spelling is not an author-facing Lynx property"
    );
}

#[test]
fn text_decoration_grammar() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");

    doc.set_inline(el, "text-decoration: none");
    doc.flush();
    assert_eq!(doc.value(el, "text-decoration-line"), "none");

    doc.set_inline(el, "text-decoration: underline line-through");
    doc.flush();
    assert_eq!(
        doc.value(el, "text-decoration-line"),
        "underline line-through"
    );
    assert_eq!(
        computed(
            "text-decoration: line-through underline",
            "text-decoration-line"
        ),
        "underline line-through"
    );
    assert!(parses("text-decoration-line", "underline line-through"));

    assert!(!parses("text-decoration-line", "underline overline"));
    assert!(!parses("text-decoration-line", "underline underline"));
    assert!(!parses("text-decoration-line", "none underline"));

    doc.set_inline(el, "text-decoration: yellow dashed underline");
    doc.flush();
    assert_eq!(doc.value(el, "text-decoration-line"), "underline");
    assert_eq!(doc.value(el, "text-decoration-style"), "dashed");
    assert_eq!(doc.value(el, "text-decoration-color"), "rgb(255, 255, 0)");

    assert!(!parses("text-decoration", "none 2px"));
    assert!(!parses("text-decoration", "underline 1px 2px"));
}

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
