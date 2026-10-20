//! `@supports` parsing and evaluation — ported from
//! `lynx/core/renderer/css/ng/supports/supports_evaluator_test.cc` and
//! `ng/parser/supports_condition_parser_test.cc`.

mod common;

use common::{Doc, rgb};

fn supports(condition: &str) -> bool {
    let mut doc = Doc::with_css(&format!(
        "@supports {condition} {{ .probe {{ color: rgb(1, 2, 3) }} }}"
    ));
    let probe = doc.el(doc.root, "view.probe");
    doc.flush();
    doc.color(probe) == rgb(1, 2, 3)
}

fn condition_parses(condition: &str) -> bool {
    supports(condition) || supports(&format!("not ({condition})"))
}

#[test]
fn empty_condition_rejected() {
    assert!(!supports(""));
    assert!(!supports("   \t  "));
}

#[test]
fn invalid_grammar_rejected() {
    for condition in [
        "(display: flex) garbage",
        "not display: flex",
        "not not (aspect-ratio: 1)",
        "(width: 1px) and (height: 2px) or (display: flex)",
    ] {
        assert!(!supports(condition), "`{condition}` must fail to parse");
    }
}

#[test]
fn declaration_conditions() {
    let rows: &[(&str, bool)] = &[
        ("(display: flex)", true),
        ("(color: red !important)", true),
        ("(display: unknown)", false),
        ("(unknown-property: flex)", false),
        ("(background: linear-gradient(red, blue))", true),
        ("(--my-var: 42px)", true),
    ];
    for &(condition, expected) in rows {
        assert_eq!(supports(condition), expected, "condition `{condition}`");
        assert!(condition_parses(condition), "`{condition}` parses");
    }
}

#[test]
fn selector_function_evaluates_support() {
    for condition in [
        "selector(.foo)",
        "selector(div > p)",
        "selector(:is(.a, .b))",
        "selector(:nth-child(2n+1))",
    ] {
        assert!(supports(condition), "condition `{condition}`");
    }
    assert!(!supports("selector(::unknown-pseudo)"));
    assert!(condition_parses("selector(::unknown-pseudo)"));
}

#[test]
fn general_enclosed_parses_and_evaluates_false() {
    for condition in [
        "(display flex)",
        "(display:)",
        "()",
        "(1 + 1)",
        "(unknown stuff)",
    ] {
        assert!(!supports(condition), "`{condition}` evaluates false");
        assert!(
            condition_parses(condition),
            "`{condition}` still parses (general-enclosed)"
        );
    }
    assert!(supports("not (unknown stuff)"));
}

#[test]
fn boolean_combinators() {
    let rows: &[(&str, bool)] = &[
        ("not (display: 9999)", true),
        ("not (display: flex)", false),
        ("(display: flex) and (color: red)", true),
        ("(display: flex) and (color: nonsense)", false),
        ("(width: 1px) and (height: 1px) and (color: red)", true),
        ("(display: nonsense) or (display: flex)", true),
        ("(display: nonsense) or (color: nonsense)", false),
        ("(width: bad) or (height: bad) or (color: red)", true),
        ("((display: flex) or (display: nonsense))", true),
        ("not ((display: flex) and (color: red))", false),
        ("(((display: flex)))", true),
        ("(display: flex) and (unknown stuff)", false),
        ("(display: flex) and selector(.a)", true),
    ];
    for &(condition, expected) in rows {
        assert_eq!(supports(condition), expected, "condition `{condition}`");
    }
}

#[test]
fn font_tech_and_font_format_parse() {
    for condition in [
        "font-tech(color-svg)",
        "font-tech(variations)",
        "font-format(woff2)",
        "font-format(opentype)",
    ] {
        assert!(condition_parses(condition), "`{condition}` parses");
    }
    assert!(!supports("font-format(bogus-format)"));
}

#[test]
fn unknown_functions_are_general_enclosed() {
    for condition in [
        "at-rule(layer)",
        "at-rule(container; (width > 0px))",
        "future-feature(something)",
        "unknown-fn()",
    ] {
        assert!(!supports(condition), "`{condition}` evaluates false");
        assert!(condition_parses(condition), "`{condition}` parses");
    }
}

#[test]
fn engine_version_extension_is_inert() {
    for condition in [
        "-x-engine-version(3.0, 5.0)",
        "-x-engine-version(3.0, *)",
        "-x-engine-version(0.0)",
    ] {
        assert!(!supports(condition), "`{condition}` evaluates false");
        assert!(
            condition_parses(condition),
            "`{condition}` parses as general-enclosed"
        );
        assert!(supports(&format!("not ({condition})")));
    }
}
