//! `@supports` parsing and evaluation — ported from
//! `lynx/core/renderer/css/ng/supports/supports_evaluator_test.cc` and
//! `ng/parser/supports_condition_parser_test.cc`.
//!
//! Scope: `enableCSSSelector = true` / `enableRemoveCSSScope = true`. Native
//! Lynx's `@supports` machinery is shipped-but-dead (zero production call
//! sites; every function-form unconditionally false — deviations.md). Here
//! `@supports` is evaluated for real via stylo. Everything is asserted end to
//! end: a probe rule guarded by the condition either applies or not, and
//! "parses" is distinguished from "evaluates false" by also probing the
//! negated condition (an unparseable condition drops the whole rule, so BOTH
//! probes fail; a false-evaluating one flips under `not`).

mod common;

use common::{Doc, rgb};

/// Does `@supports <condition> { .probe { … } }` apply?
fn supports(condition: &str) -> bool {
    let mut doc = Doc::with_css(&format!(
        "@supports {condition} {{ .probe {{ color: rgb(1, 2, 3) }} }}"
    ));
    let probe = doc.el(doc.root, "view.probe");
    doc.flush();
    doc.color(probe) == rgb(1, 2, 3)
}

/// Whether the condition parses at all (see module docs for the trick).
fn condition_parses(condition: &str) -> bool {
    supports(condition) || supports(&format!("not ({condition})"))
}

// C++: supports_condition_parser_test.cc::parse_empty_and_whitespace.
// Top-level grammar rejection is only observable as "the rule never
// applies" (the not-wrapping probe would itself change the input into a
// parseable <general-enclosed>).
#[test]
fn empty_condition_rejected() {
    assert!(!supports(""));
    assert!(!supports("   \t  "));
}

// C++: supports_condition_parser_test.cc::parse_invalid_grammar — all four
// are W3C-standard rejections.
#[test]
fn invalid_grammar_rejected() {
    // Same observability caveat as `empty_condition_rejected`: rejection is
    // asserted as "the rule never applies" — note every condition here
    // contains a supported sub-condition that would apply if the top-level
    // grammar were forgiving.
    for condition in [
        "(display: flex) garbage",
        "not display: flex",
        "not not (aspect-ratio: 1)",
        "(width: 1px) and (height: 2px) or (display: flex)",
    ] {
        assert!(!supports(condition), "`{condition}` must fail to parse");
    }
}

// C++: supports_evaluator_test.cc::declaration_support_eval +
// supports_condition_parser_test.cc::parse_declaration.
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

// C++: supports_evaluator_test.cc::unsupported_nodes_eval — W3C-corrected:
// Lynx returns false for every `selector()`; per CSS Conditional-4 (and
// stylo) `selector()` truly tests selector support.
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
    // An unsupported selector is a supported *parse* with a false result.
    assert!(!supports("selector(::unknown-pseudo)"));
    assert!(condition_parses("selector(::unknown-pseudo)"));
}

// C++: supports_condition_parser_test.cc::parse_declaration_falls_to_
// general_enclosed + parse_general_enclosed — the general-enclosed
// productions parse fine and evaluate false (stylo `FutureSyntax`).
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
    // …which makes their negation true.
    assert!(supports("not (unknown stuff)"));
}

// C++: supports_condition_parser_test.cc::parse_boolean_operators /
// parse_nested_conditions + supports_evaluator_test.cc::
// boolean_combinators_eval. Lynx's binary left-associative node shape is
// internal (stylo keeps flat vectors); the semantics are what ports.
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

// C++: supports_condition_parser_test.cc::parse_font_tech_and_font_format.
// Parse-level: the CSS Fonts-4 tokens are recognized as typed conditions.
// Eval-level: the servo build's capability probes are stubs today, so the
// truthfulness follows the engine's actual (absent) font-capability
// reporting — asserted only as "parses, evaluates deterministically".
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
    // An unknown format keyword is rejected at parse in stylo (typed
    // keyword), unlike Lynx's raw-string capture; the observable result of
    // `@supports font-format(bogus)` is identical (rule never applies).
    assert!(!supports("font-format(bogus-format)"));
}

// C++: supports_condition_parser_test.cc::parse_at_rule_function /
// parse_unknown_function — unknown function forms are general-enclosed:
// parse-ok, evaluate false.
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

// C++: supports_evaluator_test.cc::engine_version_range_eval +
// supports_condition_parser_test.cc::parse_engine_version_* — Lynx-only
// `-x-engine-version()` @supports extension. Intentionally NOT implemented:
// native Lynx's @supports evaluator has zero production call sites, so no
// shipped content can observe the version predicate; here the function form
// is general-enclosed (parses, evaluates false), which also matches what
// production native Lynx effectively does.
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

// Skipped (skip-internal): SupportsEvaluatorTest.null_node_eval — C++
// nullptr guard. SupportsConditionParserTest serialize/Lepus round-trips —
// native serialization plumbing; stylo's to_css covers serialization at the
// stylesheet level elsewhere.
