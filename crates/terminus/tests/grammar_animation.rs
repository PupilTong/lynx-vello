//! Animation/transition property grammar — ported from
//! `lynx/core/renderer/css/parser/animation_*_handler_unittest.cc`,
//! `transition_shorthand_handler_unittest.cc`,
//! `timing_function_handler_unittest.cc`, and `time_handler_unittest.cc`.

mod common;

use common::{Doc, parses, specified};

fn equivalent(property: &str, a: &str, b: &str) {
    let left = specified(property, a).unwrap_or_else(|| panic!("`{property}: {a}` must parse"));
    let right = specified(property, b).unwrap_or_else(|| panic!("`{property}: {b}` must parse"));
    assert_eq!(left, right, "`{a}` and `{b}` must mean the same");
}

#[test]
fn animation_direction_grammar() {
    for keyword in ["normal", "reverse", "alternate", "alternate-reverse"] {
        assert_eq!(
            specified("animation-direction", keyword).as_deref(),
            Some(keyword)
        );
    }
    assert_eq!(
        specified("animation-direction", "normal, reverse").as_deref(),
        Some("normal, reverse"),
        "comma lists keep one entry per animation"
    );
    for invalid in ["invalid,", "2", "2s"] {
        assert!(
            !parses("animation-direction", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn animation_fill_mode_grammar() {
    for keyword in ["none", "forwards", "backwards", "both"] {
        assert_eq!(
            specified("animation-fill-mode", keyword).as_deref(),
            Some(keyword)
        );
    }
    assert_eq!(
        specified("animation-fill-mode", "forwards, backwards").as_deref(),
        Some("forwards, backwards")
    );
    for invalid in ["invalid,", "2", "2s"] {
        assert!(
            !parses("animation-fill-mode", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn animation_iteration_count_grammar() {
    assert_eq!(
        specified("animation-iteration-count", "1000").as_deref(),
        Some("1000")
    );
    assert_eq!(
        specified("animation-iteration-count", "1").as_deref(),
        Some("1")
    );
    assert_eq!(
        specified("animation-iteration-count", "infinite").as_deref(),
        Some("infinite"),
        "keyword, not a numeric sentinel"
    );
    assert_eq!(
        specified("animation-iteration-count", "1000, infinite").as_deref(),
        Some("1000, infinite")
    );
    for invalid in ["invalid,", "2s", "-1"] {
        assert!(
            !parses("animation-iteration-count", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn transition_property_grammar() {
    assert_eq!(
        specified("transition-property", "width").as_deref(),
        Some("width")
    );
    assert_eq!(
        specified("transition-property", "all").as_deref(),
        Some("all")
    );
    assert_eq!(
        specified("transition-property", "hello").as_deref(),
        Some("hello"),
        "unknown idents are retained custom-idents"
    );
    let list = "opacity, scaleX, scaleY, scaleXY, width, height, \
                background-color, color, visibility, left, top, right, \
                bottom, transform, all, max-width, max-height, min-width, \
                min-height";
    assert_eq!(
        specified("transition-property", list).as_deref(),
        Some(list)
    );
    assert!(
        !parses("transition-property", "none, opacity"),
        "`none` is only valid standalone"
    );
    for invalid in ["invalid,", "2", "2s"] {
        assert!(
            !parses("transition-property", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn animation_shorthand_expansion() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");

    doc.set_inline(el, "animation: rotate 10s ease 1s 10 forwards");
    doc.flush();
    assert_eq!(doc.value(el, "animation-name"), "rotate");
    assert_eq!(doc.value(el, "animation-duration"), "10s");
    assert_eq!(doc.value(el, "animation-timing-function"), "ease");
    assert_eq!(doc.value(el, "animation-delay"), "1s");
    assert_eq!(doc.value(el, "animation-iteration-count"), "10");
    assert_eq!(doc.value(el, "animation-fill-mode"), "forwards");

    doc.set_inline(el, "animation: 10");
    doc.flush();
    assert_eq!(doc.value(el, "animation-name"), "none");
    assert_eq!(doc.value(el, "animation-iteration-count"), "10");
    assert_eq!(doc.value(el, "animation-duration"), "0s");
    assert_eq!(doc.value(el, "animation-delay"), "0s");
    assert_eq!(
        doc.value(el, "animation-timing-function"),
        "ease",
        "omitted timing function is the `ease` initial, not linear"
    );

    doc.set_inline(el, "animation: 10s 10 test");
    doc.flush();
    assert_eq!(doc.value(el, "animation-name"), "test");
    assert_eq!(doc.value(el, "animation-duration"), "10s");
    assert_eq!(doc.value(el, "animation-iteration-count"), "10");
    assert_eq!(doc.value(el, "animation-timing-function"), "ease");

    doc.set_inline(el, "animation: 10s ease 1s forwards 10 item1-ani-frames");
    doc.flush();
    assert_eq!(doc.value(el, "animation-name"), "item1-ani-frames");
    assert_eq!(doc.value(el, "animation-delay"), "1s");
    assert_eq!(doc.value(el, "animation-fill-mode"), "forwards");

    doc.set_inline(
        el,
        "animation: 10s ease 1ms forwards infinite test paused reverse",
    );
    doc.flush();
    assert_eq!(doc.value(el, "animation-name"), "test");
    assert_eq!(doc.value(el, "animation-duration"), "10s");
    assert_eq!(
        doc.value(el, "animation-delay"),
        "0.001s",
        "computed times serialize in seconds"
    );
    assert_eq!(doc.value(el, "animation-timing-function"), "ease");
    assert_eq!(doc.value(el, "animation-fill-mode"), "forwards");
    assert_eq!(doc.value(el, "animation-iteration-count"), "infinite");
    assert_eq!(doc.value(el, "animation-play-state"), "paused");
    assert_eq!(doc.value(el, "animation-direction"), "reverse");
}

#[test]
fn animation_shorthand_defaults_and_rejects() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, "animation: test");
    doc.flush();
    assert_eq!(doc.value(el, "animation-name"), "test");
    assert_eq!(doc.value(el, "animation-duration"), "0s");
    assert_eq!(doc.value(el, "animation-delay"), "0s");
    assert_eq!(doc.value(el, "animation-iteration-count"), "1");
    assert_eq!(doc.value(el, "animation-fill-mode"), "none");
    assert_eq!(doc.value(el, "animation-play-state"), "running");
    assert_eq!(doc.value(el, "animation-direction"), "normal");
    assert_eq!(doc.value(el, "animation-timing-function"), "ease");

    for invalid in ["test test", "12s 12s 10ms", "test, "] {
        assert!(
            !parses("animation", invalid),
            "`{invalid}` must be rejected"
        );
    }
    doc.set_inline(el, "animation: ease ease");
    doc.flush();
    assert_eq!(doc.value(el, "animation-timing-function"), "ease");
    assert_eq!(doc.value(el, "animation-name"), "ease");
}

#[test]
fn transition_shorthand_expansion() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");

    doc.set_inline(el, "transition: width 2s ease-in 1ms");
    doc.flush();
    assert_eq!(doc.value(el, "transition-property"), "width");
    assert_eq!(doc.value(el, "transition-duration"), "2s");
    assert_eq!(doc.value(el, "transition-delay"), "0.001s");
    assert_eq!(doc.value(el, "transition-timing-function"), "ease-in");

    let rows: &[(&str, &str, &str, &str, &str)] = &[
        ("width 2s ease", "width", "2s", "0s", "ease"),
        ("width 2s", "width", "2s", "0s", "ease"),
        ("width ease-out", "width", "0s", "0s", "ease-out"),
        ("width", "width", "0s", "0s", "ease"),
        ("hello", "hello", "0s", "0s", "ease"),
        ("none 2s ease-in 1ms", "none", "2s", "0.001s", "ease-in"),
    ];
    for &(input, property, duration, delay, timing) in rows {
        doc.set_inline(el, &format!("transition: {input}"));
        doc.flush();
        assert_eq!(doc.value(el, "transition-property"), property, "`{input}`");
        assert_eq!(doc.value(el, "transition-duration"), duration, "`{input}`");
        assert_eq!(doc.value(el, "transition-delay"), delay, "`{input}`");
        assert_eq!(
            doc.value(el, "transition-timing-function"),
            timing,
            "`{input}`"
        );
    }

    doc.set_inline(el, "transition: width 2s ease-in 1ms, height 10s");
    doc.flush();
    assert_eq!(doc.value(el, "transition-property"), "width, height");
    assert_eq!(doc.value(el, "transition-duration"), "2s, 10s");
    assert_eq!(doc.value(el, "transition-delay"), "0.001s, 0s");
    assert_eq!(doc.value(el, "transition-timing-function"), "ease-in, ease");
}

#[test]
fn transition_shorthand_rejects() {
    {
        let mut doc = Doc::new();
        let el = doc.el(doc.root, "view");
        doc.set_inline(el, "transition: none -2s ease-in 1ms");
        doc.flush();
        assert_eq!(doc.value(el, "transition-delay"), "-2s");
        assert_eq!(doc.value(el, "transition-duration"), "0.001s");
        assert_eq!(doc.value(el, "transition-timing-function"), "ease-in");
    }
    assert!(
        parses("transition", "width 2s -1ms"),
        "negative delay is valid"
    );
    assert!(!parses("transition-duration", "-2s"));

    for invalid in [
        "width 2s ease-in 1ms, ",
        "width 2s ease-in 1ms 1ms",
        "none 1s, none",
        "none, hello",
    ] {
        assert!(
            !parses("transition", invalid),
            "`{invalid}` must be rejected"
        );
    }
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, "transition: hello 1s, world 2s");
    doc.flush();
    assert_eq!(doc.value(el, "transition-property"), "hello, world");
}

#[test]
fn timing_function_keywords() {
    for keyword in ["linear", "ease", "ease-in", "ease-out", "ease-in-out"] {
        assert_eq!(
            specified("animation-timing-function", keyword).as_deref(),
            Some(keyword)
        );
    }
    assert_ne!(
        specified("animation-timing-function", "ease"),
        specified("animation-timing-function", "ease-in-out"),
        "ease is cubic-bezier(0.25, 0.1, 0.25, 1), not ease-in-out"
    );
    assert!(
        !parses("animation-timing-function", "ease-in-ease-out"),
        "Lynx's aliased spelling is not a CSS keyword"
    );
}

#[test]
fn timing_function_functions() {
    assert_eq!(
        specified("transition-timing-function", "cubic-bezier(1, 0.5, 0.5, 1)").as_deref(),
        Some("cubic-bezier(1, 0.5, 0.5, 1)")
    );
    assert_eq!(
        specified("transition-timing-function", "step-start").as_deref(),
        Some("steps(1, start)")
    );
    assert_eq!(
        specified("transition-timing-function", "step-end").as_deref(),
        Some("steps(1)")
    );
    equivalent(
        "transition-timing-function",
        "steps(1)",
        "steps(1, jump-end)",
    );
    equivalent("transition-timing-function", "steps(1)", "steps(1, end)");
    for position in ["jump-start", "jump-end", "jump-none", "jump-both"] {
        assert!(
            parses(
                "transition-timing-function",
                &format!("steps(2, {position})")
            ),
            "steps(2, {position}) must parse"
        );
    }
    assert!(!parses("transition-timing-function", "steps(1, jump-none)"));
    assert_eq!(
        specified(
            "transition-timing-function",
            "steps(1, jump-start), step-end"
        )
        .as_deref(),
        Some("steps(1, jump-start), steps(1)")
    );
    for invalid in [
        "",
        "hello",
        "ease, ",
        "cubic-bezier(1, 0.5)",
        "steps(1,)",
        "steps(1, hello)",
    ] {
        assert!(
            !parses("transition-timing-function", invalid),
            "`{invalid}` must be rejected"
        );
    }
}

#[test]
fn square_bezier_lynx_rows() {
    assert_eq!(
        specified("transition-timing-function", "square-bezier(1, 0.5)").as_deref(),
        Some("square-bezier(1, 0.5)")
    );
    equivalent(
        "transition-timing-function",
        "square-bezier(1, 0.5)",
        "square-bezier( 1 ,0.5 )",
    );
    assert_eq!(
        specified(
            "transition-timing-function",
            "square-bezier(1, 0.5), ease-in"
        )
        .as_deref(),
        Some("square-bezier(1, 0.5), ease-in")
    );
    for invalid in [
        "square-bezier(1)",
        "square-bezier(1, 0.5, 0)",
        "square-bezier(1,)",
        "square-bezier(a, b)",
        "square-bezier",
    ] {
        assert!(
            !parses("transition-timing-function", invalid),
            "`{invalid}` must be rejected (Lynx takes exactly two numbers)"
        );
    }
}

#[test]
fn time_values() {
    assert_eq!(specified("animation-duration", "2s").as_deref(), Some("2s"));
    assert_eq!(
        specified("animation-duration", "2ms").as_deref(),
        Some("2ms")
    );
    equivalent("animation-duration", "2000ms, 1s, 10s", "2000ms,1s,   10s ");
    equivalent("animation-duration", "010ms", "10ms");
    for invalid in ["200", "0", "abc", "7 ms", "-2ms"] {
        assert!(
            !parses("animation-duration", invalid),
            "`{invalid}` must be rejected for duration"
        );
    }
    assert_eq!(
        specified("animation-delay", "-2ms").as_deref(),
        Some("-2ms"),
        "delay accepts negative times"
    );
}
