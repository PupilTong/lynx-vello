//! Background / mask / clip-path value grammar — ported from
//! `lynx/core/renderer/css/parser/background_*_handler_unittest.cc`,
//! `mask_composite_handler_unittest.cc`, and `clip_path_handler_unittest.cc`.

mod common;

use common::{Doc, parses, specified};

fn equivalent(property: &str, a: &str, b: &str) {
    let left = specified(property, a).unwrap_or_else(|| panic!("`{property}: {a}` must parse"));
    let right = specified(property, b).unwrap_or_else(|| panic!("`{property}: {b}` must parse"));
    assert_eq!(left, right, "`{a}` and `{b}` must mean the same");
}

fn computed(declaration: &str, property: &str) -> String {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, declaration);
    doc.flush();
    doc.value(el, property)
}

#[test]
fn background_origin_grammar() {
    for keyword in ["content-box", "padding-box", "border-box"] {
        assert_eq!(
            computed(
                &format!("background-origin: {keyword}"),
                "background-origin"
            ),
            keyword
        );
    }
    assert_eq!(
        computed(
            "background-origin: border-box, padding-box, content-box",
            "background-origin"
        ),
        "border-box, padding-box, content-box",
        "layer order preserved"
    );
    for invalid in ["fill-box", "margin-box", "stroke-box", "view-box"] {
        assert!(
            !parses("background-origin", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn background_clip_grammar() {
    for keyword in ["content-box", "padding-box", "border-box"] {
        assert_eq!(
            computed(&format!("background-clip: {keyword}"), "background-clip"),
            keyword
        );
    }
    assert_eq!(
        computed(
            "background-clip: border-box, padding-box, content-box",
            "background-clip"
        ),
        "border-box, padding-box, content-box"
    );
    assert!(!parses("background-clip", "text"));
}

#[test]
#[ignore = "engine-gap: background-clip:text not implemented in the servo build (Lynx clips text backgrounds)"]
fn background_clip_text() {
    assert_eq!(computed("background-clip: text", "background-clip"), "text");
    assert_eq!(
        computed(
            "background-clip: border-box, padding-box, content-box, text",
            "background-clip"
        ),
        "border-box, padding-box, content-box, text"
    );
}

#[test]
fn background_image_urls() {
    let list = "url('https://yyy/i/bg_flower.gif'), \
                url('https://tttt/files/7693/catfront.png'),\
                url('https://xxxx/ee/lynx-home/static/img/zh-logo-color.7c750dd6.png')";
    let value = computed(&format!("background-image: {list}"), "background-image");
    assert_eq!(
        value.matches("url(").count(),
        3,
        "three url layers: {value}"
    );
    assert!(value.contains("bg_flower.gif") && value.contains("catfront.png"));

    let single = computed(
        "background-image: url(\"data:image/png;base64,\")",
        "background-image",
    );
    assert_eq!(
        single.matches("url(").count(),
        1,
        "data URI preserved: {single}"
    );
    assert!(single.contains("data:image/png;base64,"));
}

#[test]
fn background_image_linear_gradients() {
    let bare_number = computed(
        "background-image: linear-gradient(to left, red, blue 30%, green 0.9)",
        "background-image",
    );
    assert!(
        bare_number.contains("90%"),
        "a bare-number stop position is a fraction: `0.9` computes to 90%: {bare_number}"
    );
    let corrected = computed(
        "background-image: linear-gradient(to left, red, blue 30%, green 90%)",
        "background-image",
    );
    assert!(corrected.contains("to left"), "direction kept: {corrected}");
    assert!(corrected.contains("30%") && corrected.contains("90%"));

    let defaulted = computed(
        "background-image: linear-gradient(rgba(0, 0, 255, 0.5), rgba(255, 255, 0, 0.5))",
        "background-image",
    );
    assert!(
        defaulted.starts_with("linear-gradient(rgba"),
        "default to-bottom direction is omitted in serialization: {defaulted}"
    );

    let literal = computed(
        "background-image: linear-gradient(to left, red -10%, blue 10%, green)",
        "background-image",
    );
    assert!(
        literal.contains("-10%") && literal.contains("10%"),
        "literal stops preserved without pre-mixing: {literal}"
    );
    assert!(
        parses(
            "background-image",
            "linear-gradient(to left, red, blue, green 90%, blue, black 150%)"
        ),
        "over-100% stops are valid and stay literal"
    );
}

#[test]
fn background_image_gradient_angle_units() {
    let rows: &[(&str, &str)] = &[
        ("linear-gradient(90DeG, green, green)", "90deg"),
        ("linear-gradient(0.25tUrN, green, green)", "90deg"),
        ("linear-gradient(1.57rAd, green, green)", "89.95"),
    ];
    for &(input, expected_fragment) in rows {
        let value = computed(&format!("background-image: {input}"), "background-image");
        assert!(
            value.contains(expected_fragment),
            "`{input}` computes an angle containing `{expected_fragment}`: {value}"
        );
    }
    for invalid in [
        "linear-gradient(100gRaD, red, red)",
        "linear-gradient(90degree, red, red)",
        "linear-gradient(100gradian, red, red)",
        "linear-gradient(1.57radian, red, red)",
        "linear-gradient(0.25turns, red, red)",
    ] {
        assert!(
            !parses("background-image", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn background_image_radial_gradients() {
    for valid in [
        "radial-gradient(ellipse at top, red, transparent)",
        "radial-gradient(ellipse 10px 5px at top, red, transparent)",
        "radial-gradient(circle 10px at top, red, transparent)",
        "radial-gradient(farthest-corner at center, red, transparent)",
        "radial-gradient(farthest-side at center, red, transparent)",
        "radial-gradient(closest-corner at center, red, transparent)",
        "radial-gradient(closest-side at center, red, transparent)",
        "radial-gradient(ellipse at top, red, transparent), radial-gradient(ellipse at right, blue, transparent)",
    ] {
        assert!(parses("background-image", valid), "`{valid}` must parse");
    }
    equivalent(
        "background-image",
        "radial-gradient(10px at top, red, transparent)",
        "radial-gradient(circle 10px at top, red, transparent)",
    );
    equivalent(
        "background-image",
        "radial-gradient(10px 5px at top, red, transparent)",
        "radial-gradient(ellipse 10px 5px at top, red, transparent)",
    );
    for invalid in [
        "radial-gradient(ellipse farthest-corner 10px at top, red, transparent)",
        "radial-gradient(circle 10px 5px at top, red, transparent)",
        "radial-gradient(ellipse 10px at top, red, transparent)",
        "radial-gradient(ellipse ellipse, red, transparent)",
        "radial-gradient(farthest-corner at center)",
    ] {
        assert!(
            !parses("background-image", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn background_image_conic_gradients() {
    for valid in [
        "conic-gradient(red, blue)",
        "conic-gradient(from 30deg, red, blue)",
        "conic-gradient(from 50deg at top right, red 0%, blue 90%)",
    ] {
        assert!(parses("background-image", valid), "`{valid}` must parse");
    }
    let angled = computed(
        "background-image: conic-gradient(from 50deg at top right, red 0%, blue 90%)",
        "background-image",
    );
    assert!(
        angled.contains("50deg") && angled.contains("90%"),
        "from-angle and stop positions kept: {angled}"
    );
    for invalid in [
        "conic-gradient(90deg, red, red)",
        "conic-gradient(from 90deg at red, red)",
        "conic-gradient(at, red, red)",
        "conic-gradient(at 10px red, red)",
    ] {
        assert!(
            !parses("background-image", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn background_position_grammar() {
    assert_eq!(
        computed(
            "background-position: bottom, right bottom",
            "background-position-y"
        ),
        "100%, 100%",
        "two layers, both bottom"
    );
    assert_eq!(
        computed(
            "background-position: bottom, right bottom",
            "background-position-x"
        ),
        "50%, 100%"
    );
    for (a, b) in [
        ("top left", "left top"),
        ("top right", "right top"),
        ("bottom right", "right bottom"),
        ("bottom left", "left bottom"),
        ("bottom center", "center bottom"),
        ("top center", "center top"),
        ("left center", "center left"),
        ("right center", "center right"),
    ] {
        equivalent("background-position", a, b);
    }
    for valid in [
        "calc(100% - 20px) 40px",
        "calc(20px + 50%) calc(30px * 2)",
        "calc(20px + (20px * 2)) 40px",
        "50px 40px",
        "50px 40%",
    ] {
        assert!(parses("background-position", valid), "`{valid}` must parse");
    }
    assert!(!parses(
        "background-position",
        "calc(20px + (20px * 2)) calc(50% + (10px * 2 * 50%))"
    ));
    for invalid in [
        "left hello",
        "top 10%",
        "top top",
        "right left",
        "10% left",
        "50px right",
    ] {
        assert!(
            !parses("background-position", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn background_repeat_grammar() {
    let rows: &[(&str, &str)] = &[
        ("repeat", "repeat"),
        ("no-repeat", "no-repeat"),
        ("repeat-x", "repeat-x"),
        ("repeat-y", "repeat-y"),
        ("round", "round"),
        ("space", "space"),
        ("repeat no-repeat", "repeat-x"),
        ("no-repeat repeat", "repeat-y"),
        ("round round", "round"),
        ("space space", "space"),
        ("repeat space", "repeat space"),
        ("round no-repeat", "round no-repeat"),
        ("repeat repeat, repeat", "repeat, repeat"),
        ("repeat-x, repeat-y, repeat", "repeat-x, repeat-y, repeat"),
    ];
    for &(input, expected) in rows {
        assert_eq!(
            computed(&format!("background-repeat: {input}"), "background-repeat"),
            expected,
            "`{input}`"
        );
    }
    for invalid in [
        "repeat-y repeat-x",
        "repeat-x no-repeat",
        "repeat-y round",
        "repeat space round",
        "repeat repeat-x",
    ] {
        assert!(
            !parses("background-repeat", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn background_shorthand_expansion() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");

    doc.set_inline(el, "background: url('https://yyy/i/bg_flower.gif')");
    doc.flush();
    assert!(doc.value(el, "background-image").contains("bg_flower.gif"));
    assert_eq!(doc.value(el, "background-color"), "rgba(0, 0, 0, 0)");
    assert_eq!(doc.value(el, "background-position-x"), "0%");
    assert_eq!(doc.value(el, "background-size"), "auto");
    assert_eq!(doc.value(el, "background-repeat"), "repeat");
    assert_eq!(doc.value(el, "background-origin"), "padding-box");
    assert_eq!(
        doc.value(el, "background-clip"),
        "border-box",
        "omitted clip resets to the W3C initial (border-box), not padding-box"
    );

    doc.set_inline(
        el,
        "background: url('https://a/1.gif') left 5% / 15% 60% repeat-x, \
         url('https://a/2.png') red",
    );
    doc.flush();
    assert_eq!(doc.value(el, "background-color"), "rgb(255, 0, 0)");
    assert_eq!(doc.value(el, "background-image").matches("url(").count(), 2);
    assert_eq!(doc.value(el, "background-position-x"), "0%, 0%");
    assert_eq!(doc.value(el, "background-position-y"), "5%, 0%");
    assert_eq!(doc.value(el, "background-size"), "15% 60%, auto");
    assert_eq!(doc.value(el, "background-repeat"), "repeat-x, repeat");

    doc.set_inline(
        el,
        "background: content-box center / contain no-repeat url(\"https://a/logo.svg\"), \
         content-box #eee border-box 35% url(\"https://a/1.png\")",
    );
    doc.flush();
    assert_eq!(doc.value(el, "background-color"), "rgb(238, 238, 238)");
    assert_eq!(
        doc.value(el, "background-origin"),
        "content-box, content-box"
    );
    assert_eq!(doc.value(el, "background-clip"), "content-box, border-box");
    assert_eq!(doc.value(el, "background-size"), "contain, auto");
    assert_eq!(doc.value(el, "background-repeat"), "no-repeat, repeat");
    assert_eq!(doc.value(el, "background-position-x"), "50%, 35%");
    assert_eq!(doc.value(el, "background-position-y"), "50%, 50%");
}

#[test]
fn background_shorthand_none_color_and_rejects() {
    assert_eq!(computed("background: NONE", "background-image"), "none");
    assert_eq!(
        computed("background: NONE", "background-color"),
        "rgba(0, 0, 0, 0)"
    );
    assert_eq!(
        computed("background: red", "background-color"),
        "rgb(255, 0, 0)"
    );
    assert_eq!(computed("background: red", "background-image"), "none");

    for invalid in [
        "url('https://a/b.png') 100% 100% / cover top no-repeat';",
        "hello",
        "hello red",
        "red red",
        "url('https://a/b.png') left hello",
        "red url('https://a/b.png'), radial-gradient(#FF0000, #00FF00)",
        "url('https://a/b.png') url('https://a/c.png')",
        "url('https://a/b.png') 100% 100% 100%",
        "url('https://a/b.png') 100% 100% top",
        "url('https://a/b.png') repeat-x repeat-y",
        "url('https://a/b.png') repeat-x repeat",
    ] {
        assert!(
            !parses("background", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn background_size_grammar() {
    let rows: &[(&str, &str)] = &[
        ("auto", "auto"),
        ("auto auto", "auto"),
        ("cover", "cover"),
        ("contain", "contain"),
        ("50px", "50px auto"),
        ("50px auto", "50px auto"),
        ("50px 40px", "50px 40px"),
        ("50px 40%", "50px 40%"),
        ("1px", "1px auto"),
        ("2% 3%", "2% 3%"),
        ("auto 4%", "auto 4%"),
        ("50px, 30px 40%", "50px auto, 30px 40%"),
        ("1px 2px, 3px 4px", "1px 2px, 3px 4px"),
    ];
    for &(input, expected) in rows {
        assert_eq!(
            computed(&format!("background-size: {input}"), "background-size"),
            expected,
            "`{input}`"
        );
    }
    for invalid in ["1px 2px 3px", "wrap"] {
        assert!(
            !parses("background-size", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn mask_composite_grammar() {
    assert_eq!(
        computed(
            "mask-composite: add, subtract, intersect, exclude",
            "mask-composite"
        ),
        "add, subtract, intersect, exclude"
    );
    for invalid in ["", "none", "xor", "add subtract", "add,"] {
        assert!(
            !parses("mask-composite", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn clip_path_basic_shapes() {
    for valid in [
        "circle(40px at 30px bottom)",
        "circle(30%)",
        "path(\"M 0 0 L 100 100 L 30 30 Z\")",
        "ellipse(20px 50% at bottom right)",
        "ellipse(20px 50px at left top)",
        "ellipse(20px 50px at 35% 20%)",
    ] {
        assert!(parses("clip-path", valid), "`{valid}` must parse");
    }
    assert!(
        !parses("width", "50ppx") && parses("width", "50rpx"),
        "ppx grew unit support — port the C++ ppx rows verbatim"
    );
    equivalent(
        "clip-path",
        "circle(30% at center)",
        "circle(30% at center center)",
    );
    assert_eq!(
        specified("clip-path", "circle(30%)").as_deref(),
        Some("circle(30%)")
    );
    for invalid in ["circle(ppp)", "path(100)", "ellipse(20px at left center)"] {
        assert!(
            !parses("clip-path", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn super_ellipse_is_absent() {
    assert!(
        !parses("clip-path", "super-ellipse(40px 30px 2 2 at 30px bottom)"),
        "super-ellipse grew grammar support — port the Lynx-faithful rows"
    );
}
