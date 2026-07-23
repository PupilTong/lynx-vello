//! Selector matching, parsing, and serialization — ported from
//! `lynx/core/renderer/css/ng/matcher/selector_matcher_test.cc`,
//! `ng/selector/css_selector_parser_test.cc`, and
//! `ng/selector/lynx_css_selector_test.cc`.

mod common;

use common::{Doc, url_data};
use cssparser::ToCss;
use stylo::selector_parser::SelectorParser;
use w3c_dom::ElementState;

fn roundtrip(selector: &str) -> Option<String> {
    let list = SelectorParser::parse_author_origin_no_namespace(selector, &url_data()).ok()?;
    Some(list.to_css_string())
}

#[test]
fn compound_with_focus_state_matches() {
    let mut doc = Doc::new();
    let parent = doc.el(doc.root, "div");
    let target = doc.el(parent, "div.a");
    doc.dom.add_element_state(target, ElementState::FOCUS);
    assert!(doc.matches(target, "div .a:focus"));
    assert!(!doc.matches(target, "div .a:hover"));
}

#[test]
fn match_status_table() {
    let mut doc = Doc::new();
    let kids = doc.els(
        doc.root,
        &[
            "text",
            "view#main.foo[flatten=true][color=red green blue][lang=zh-CN]",
            "view",
            "view",
        ],
    );
    let target = kids[1];
    doc.dom.add_element_state(doc.root, ElementState::FOCUS);
    doc.dom.add_element_state(target, ElementState::FOCUS);

    let rows: &[(&str, bool)] = &[
        ("*", true),
        ("view", true),
        (":root view", true),
        (".foo", true),
        (".bar", false),
        ("#main", true),
        ("#test", false),
        (":focus", true),
        (":active", false),
        (":active:hover", false),
        (":focus *", true),
        (":focus > *", true),
        ("text + .foo", true),
        ("view + .foo", false),
        ("text ~ .foo", true),
        ("view ~ .foo", false),
        (":not(text)", true),
        (":not(view)", false),
        (":not(:active, :hover)", true),
        (":not(:active, :focus)", false),
    ];
    for &(selector, expected) in rows {
        assert_eq!(
            doc.matches(target, selector),
            expected,
            "selector `{selector}`"
        );
    }
}

#[test]
fn attribute_selectors_match_per_spec() {
    let mut doc = Doc::new();
    let target = doc.el(
        doc.root,
        "view#main.foo[flatten=true][color=red green blue][lang=zh-CN]",
    );
    let rows: &[(&str, bool)] = &[
        ("[flatten]", true),
        ("[missing]", false),
        ("[flatten=true]", true),
        ("[flatten=false]", false),
        ("[flatten='true']", true),
        ("[color~=green]", true),
        ("[color~=gree]", false),
        ("[lang|=zh]", true),
        ("[lang|=en]", false),
        ("[color^=red]", true),
        ("[color$=blue]", true),
        ("[color*='d green b']", true),
        ("[color*='blue red']", false),
        ("[flatten=TRUE i]", true),
        ("[flatten=TRUE s]", false),
        ("view[flatten=true][color^=red]", true),
    ];
    for &(selector, expected) in rows {
        assert_eq!(
            doc.matches(target, selector),
            expected,
            "selector `{selector}`"
        );
    }
}

#[test]
fn structural_pseudo_classes_match_per_spec() {
    let mut doc = Doc::new();
    let kids = doc.els(
        doc.root,
        &["text.a", "view.b", "text.c", "view.d", "view.e"],
    );
    let rows: &[(&str, usize, bool)] = &[
        (":first-child", 0, true),
        (":first-child", 1, false),
        (":last-child", 4, true),
        (":last-child", 3, false),
        (":nth-child(1)", 0, true),
        (":nth-child(2n+1)", 2, true),
        (":nth-child(2n+1)", 3, false),
        (":nth-child(odd)", 4, true),
        (":nth-child(even)", 1, true),
        (":nth-last-child(1)", 4, true),
        (":nth-last-child(2)", 3, true),
        (":first-of-type", 0, true),
        (":first-of-type", 1, true),
        (":first-of-type", 2, false),
        (":last-of-type", 2, true),
        (":nth-of-type(2)", 2, true),
        (":only-child", 0, false),
        (":is(.a, .d)", 3, true),
        (":is(.a, .d)", 1, false),
        (":where(.b)", 1, true),
        (":empty", 0, true),
    ];
    for &(selector, index, expected) in rows {
        assert_eq!(
            doc.matches(kids[index], selector),
            expected,
            "selector `{selector}` on child {index}"
        );
    }
    let solo_parent = doc.el(doc.root, "view.solo-parent");
    let solo = doc.el(solo_parent, "view");
    assert!(doc.matches(solo, ":only-child"));
}

#[test]
fn pseudo_element_selectors_parse_and_do_not_match_origin_elements() {
    let mut doc = Doc::new();
    let parent = doc.el(doc.root, "div");
    let target = doc.el(parent, "div.a");
    for selector in ["div .a::placeholder", "div::selection"] {
        assert!(
            Doc::selector_parses(selector),
            "`{selector}` must stay parseable"
        );
        assert!(
            !doc.matches(target, selector),
            "`{selector}` must not match the origin element in normal mode"
        );
    }
}

#[test]
fn valid_selectors_parse_and_serialize_idempotently() {
    let selectors = [
        "div",
        ":active",
        ".\\[item\\]",
        "div.class",
        "div:active",
        "#list:focus",
        "div:hover",
        "input::placeholder",
        "*::placeholder",
        "::selection",
        "#text::selection",
        "#main",
        ".a.b.c",
        "#main div",
        "#main div .a",
        "div .a.b",
        "view text",
        "div *",
        "div text *",
        "view[flatten=\"false\"]",
        "a[title]",
        "*",
        "div .a:hover",
    ];
    for selector in selectors {
        let Some(first) = roundtrip(selector) else {
            panic!("`{selector}` must parse");
        };
        let second = roundtrip(&first)
            .unwrap_or_else(|| panic!("serialized `{first}` must reparse (from `{selector}`)"));
        assert_eq!(first, second, "serialization of `{selector}` is idempotent");
    }
}

#[test]
fn escaped_class_names_match_literal_values() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.add_class(el, "[item]");
    assert!(doc.matches(el, ".\\[item\\]"));
    assert!(!doc.matches(el, ".item"));
}

#[test]
#[allow(clippy::too_many_lines)]
fn valid_an_plus_b_forms() {
    let rows: &[(&str, i64, i64)] = &[
        ("odd", 2, 1),
        ("OdD", 2, 1),
        ("even", 2, 0),
        ("EveN", 2, 0),
        ("0", 0, 0),
        ("8", 0, 8),
        ("+12", 0, 12),
        ("-14", 0, -14),
        ("0n", 0, 0),
        ("16N", 16, 0),
        ("-19n", -19, 0),
        ("+23n", 23, 0),
        ("n", 1, 0),
        ("N", 1, 0),
        ("+n", 1, 0),
        ("-n", -1, 0),
        ("-N", -1, 0),
        ("6n-3", 6, -3),
        ("-26N-33", -26, -33),
        ("n-18", 1, -18),
        ("+N-5", 1, -5),
        ("-n-7", -1, -7),
        ("0n+0", 0, 0),
        ("10n+5", 10, 5),
        ("10N +5", 10, 5),
        ("10n -5", 10, -5),
        ("N+6", 1, 6),
        ("n +6", 1, 6),
        ("+n -7", 1, -7),
        ("-N -8", -1, -8),
        ("-n+9", -1, 9),
        ("33N- 22", 33, -22),
        ("+n- 25", 1, -25),
        ("N- 46", 1, -46),
        ("n- 0", 1, 0),
        ("-N- 951", -1, -951),
        ("-n- 951", -1, -951),
        ("29N + 77", 29, 77),
        ("29n - 77", 29, -77),
        ("+n + 61", 1, 61),
        ("+N - 63", 1, -63),
        ("+n/**/- 48", 1, -48),
        ("-n + 81", -1, 81),
        ("-N - 88", -1, -88),
        ("3091970736n + 1", i64::from(i32::MAX), 1),
        ("-3091970736n + 1", i64::from(i32::MIN), 1),
        ("N- 3091970736", 1, -i64::from(i32::MAX)),
        ("N+ 3091970736", 1, i64::from(i32::MAX)),
    ];
    for &(input, a, b) in rows {
        let parsed = roundtrip(&format!(":nth-child({input})"))
            .unwrap_or_else(|| panic!("`:nth-child({input})` must parse"));
        let canonical = roundtrip(&format!(":nth-child({a}n{b:+})"))
            .unwrap_or_else(|| panic!("canonical ({a}, {b}) must parse"));
        assert_eq!(parsed, canonical, "`{input}` must mean ({a}n{b:+})");
    }
}

#[test]
fn invalid_an_plus_b_forms() {
    let rows = [
        "+ n", "3m+4", "12n--34", "12n- -34", "12n- +34", "23n-+43", "10n 5", "10n + +5",
        "10n + -5",
    ];
    for input in rows {
        assert!(
            roundtrip(&format!(":nth-child({input})")).is_none(),
            "`:nth-child({input})` must be rejected"
        );
    }
    assert!(
        roundtrip(":nth-child( odd )").is_some(),
        "whitespace inside the functional argument is trimmed per spec"
    );
}

#[test]
fn pseudo_elements_rejected_inside_functional_pseudo_classes() {
    let rows = [
        ":not(::before)",
        ":not(::content)",
        ":host(::before)",
        ":host(::content)",
        ":host-context(::before)",
        ":host-context(::content)",
        ":-webkit-any(::after, ::before)",
        ":-webkit-any(::content, span)",
    ];
    for selector in rows {
        assert!(
            !Doc::selector_parses(selector),
            "`{selector}` must be rejected"
        );
    }
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view.x");
    for selector in [":is(::placeholder)", ":where(::selection)"] {
        assert!(
            Doc::selector_parses(selector),
            "`{selector}` parses (forgiving list)"
        );
        assert!(
            !doc.matches(el, selector),
            "`{selector}` matches nothing (all arguments dropped)"
        );
    }
}

#[test]
fn simple_selectors_after_pseudo_elements() {
    let rejected = [
        "::before#id",
        ".class::content::before",
        "::shadow.class",
        "::-webkit-volume-slider.class",
        "::before:not(.a)",
        "::shadow:not(::after)",
        "div ::before.a",
        "::slotted(div)::slotted(span)",
        "::slotted(*)::first-letter",
    ];
    for selector in rejected {
        assert!(
            !Doc::selector_parses(selector),
            "`{selector}` must be rejected"
        );
    }
    assert!(
        Doc::selector_parses("::selection:hover") == Doc::selector_parses("::placeholder:hover"),
        "user-action pseudo-class support after pseudo-elements is uniform"
    );
}

#[test]
fn pseudo_element_must_be_rightmost() {
    for selector in ["::before *", "::selection *", "::placeholder view"] {
        assert!(
            !Doc::selector_parses(selector),
            "`{selector}` must be rejected"
        );
    }
}

#[test]
fn malformed_namespace_pipes_rejected() {
    for selector in ["div | .c", "| div", " | div"] {
        assert!(
            !Doc::selector_parses(selector),
            "`{selector}` must be rejected"
        );
    }
}

#[test]
fn universal_serializes_as_star() {
    assert_eq!(roundtrip("*").as_deref(), Some("*"));
}

#[test]
fn universal_attribute_names_rejected() {
    for selector in ["[*]", "[*|*]"] {
        assert!(
            !Doc::selector_parses(selector),
            "`{selector}` must be rejected"
        );
    }
}

#[test]
fn unicode_escapes_decode_and_preserve_case() {
    let mut doc = Doc::new();
    let el = doc.el(
        doc.root,
        "\u{212a}bd.\u{212a}l-ass#\u{212a}l-ass[\u{212a}l-ass=x]",
    );
    assert!(doc.matches(el, "\\212a bd"), "escaped tag name");
    assert!(doc.matches(el, ".\\212al-ass"), "escaped class");
    assert!(doc.matches(el, "#\\212al-ass"), "escaped id");
    assert!(doc.matches(el, "[\\212al-ass]"), "escaped attribute name");
    assert!(!doc.matches(el, "kbd"), "no ASCII lowercasing of U+212A");
}
