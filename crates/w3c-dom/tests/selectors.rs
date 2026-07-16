//! Selector matching, parsing, and serialization — ported from
//! `lynx/core/renderer/css/ng/matcher/selector_matcher_test.cc`,
//! `ng/selector/css_selector_parser_test.cc`, and
//! `ng/selector/lynx_css_selector_test.cc`.
//!
//! Scope: `enableCSSSelector = true` (NG path) / `enableRemoveCSSScope = true`
//! only. Expectation policy (docs/tracking/deviations.md): W3C-correct
//! behavior per stylo — selectors Lynx parses but never matches (attribute
//! selectors, `:nth-*`, `:is()`/`:where()`) MUST match per spec here; Lynx's
//! internal mechanisms (kUAShadow chains, Lepus encode/decode) are not
//! reproduced, only their observable behavior.

mod common;

use common::{Doc, url_data};
use cssparser::ToCss;
use stylo::selector_parser::SelectorParser;
use w3c_dom::ElementState;

/// Parse + serialize; `None` when the list fails to parse.
fn roundtrip(selector: &str) -> Option<String> {
    let list = SelectorParser::parse_author_origin_no_namespace(selector, &url_data()).ok()?;
    Some(list.to_css_string())
}

// C++: selector_matcher_test.cc::CSSMatcherTest.CheckSimple
#[test]
fn compound_with_focus_state_matches() {
    let mut doc = Doc::new();
    let parent = doc.el(doc.root, "div");
    let target = doc.el(parent, "div.a");
    doc.set_state(target, ElementState::FOCUS, true);
    assert!(doc.matches(target, "div .a:focus"));
    assert!(!doc.matches(target, "div .a:hover"));
}

// C++: selector_matcher_test.cc::MatchStatusTest.All (SelectorChecker suite,
// 20 parameterized rows over one fixed tree).
#[test]
fn match_status_table() {
    let mut doc = Doc::new();
    // page(:focus) > [ text, view#main.foo(:focus)[flatten="true"]
    //                  [color="red green blue"][lang="zh-CN"], view, view ]
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
    doc.set_state(doc.root, ElementState::FOCUS, true);
    doc.set_state(target, ElementState::FOCUS, true);

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
        // Compound short-circuit: :active fails, :hover never evaluated.
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

// W3C-corrected additive coverage on the same fixture: attribute selectors
// parse in Lynx's NG grammar but its matcher never matches any kAttribute*
// type (selector_matcher.cc falls through to `return false`). Per policy they
// must match per Selectors-4 here, including the `~=`/`|=` forms and
// case-insensitivity flags the legacy path lacked.
// C++: attribute grammar from css_selector_parser_test.cc:417-454 +
// attribute_selector_matching.cc (legacy path), W3C-corrected.
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

// W3C-corrected additive coverage: structural pseudo-classes are parsed by
// Lynx's NG grammar with correct specificity and dead-code-correct MatchNth
// arithmetic, but the matcher never calls them. Here they match per spec.
// C++: lynx_css_selector.cc MatchNth (dead code), W3C-corrected.
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
        (":first-of-type", 0, true),  // first text
        (":first-of-type", 1, true),  // first view
        (":first-of-type", 2, false), // second text
        (":last-of-type", 2, true),   // last text
        (":nth-of-type(2)", 2, true), // second text
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
    // :only-child on an actual only child.
    let solo_parent = doc.el(doc.root, "view.solo-parent");
    let solo = doc.el(solo_parent, "view");
    assert!(doc.matches(solo, ":only-child"));
}

// C++: selector_matcher_test.cc::CSSMatcherTest.CheckPseudoElement — Lynx
// matches `div .a::placeholder, div::selection` through synthetic UAShadow
// pseudo-element nodes. We do not reproduce the mechanism; the observable
// contract asserted here is that the pseudo-element selector list parses and
// never matches the *origin* element itself in normal matching mode.
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

// C++: css_selector_parser_test.cc::SelectorParseTest.Parse +
// lynx_css_selector_test.cc::LynxSelectorTest.Parse (Lepus round-trip dropped
// as internal; stylo's parse→serialize→reparse→serialize idempotence is the
// analog). Lynx serializes un-escaped/unquoted; stylo re-escapes and quotes
// per CSSOM — assert stylo's form, per policy.
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
        "div .a:hover", // CSSSelectorParserTest.SimpleSelector smoke row
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

// Behavioral half of the escaped-identifier rows: `.\[item\]` addresses the
// literal class name `[item]`.
// C++: css_selector_parser_test.cc (escaped-class table rows).
#[test]
fn escaped_class_names_match_literal_values() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.add_class(el, "[item]");
    assert!(doc.matches(el, ".\\[item\\]"));
    assert!(!doc.matches(el, ".item"));
}

// C++: css_selector_parser_test.cc::CSSSelectorParserTest.ValidANPlusB (48
// rows). No public ConsumeANPlusB analog: each form is parsed inside
// `:nth-child()` and compared — via canonical serialization — against the
// same selector built from the expected (a, b) pair. The last four rows are
// the i32 overflow-clamp rows.
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
        // i32 saturation (Lynx clamps to INT_MAX/INT_MIN; stylo must too).
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

// C++: css_selector_parser_test.cc::CSSSelectorParserTest.InvalidANPlusB.
// The C++ " odd" row is a raw-helper artifact: in a real `:nth-child()`
// context surrounding whitespace is trimmed, so it parses — asserted in the
// valid direction below.
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

// C++: css_selector_parser_test.cc::CSSSelectorParserTest.
// PseudoElementsInCompoundLists — pseudo-elements are invalid inside
// :not()/:host()/:-webkit-any() argument lists. (The :host/::content/
// :-webkit-any rows also reject here simply as unknown pseudos; the net
// assertion — rejection — is identical.)
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
    // W3C-corrected: `:is()`/`:where()` argument lists are FORGIVING per
    // Selectors-4 — a pseudo-element inside is dropped rather than rejecting
    // the whole selector, and the surviving (empty) list matches nothing.
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

// C++: css_selector_parser_test.cc::CSSSelectorParserTest.
// InvalidSimpleAfterPseudoElementInCompound — only the rows whose
// pseudo-elements this build recognizes are load-bearing; unknown-pseudo rows
// (::shadow, ::slotted, ::-webkit-*) reject for that reason instead.
// W3C-corrected split: Selectors-4 allows *user-action pseudo-classes* after
// a pseudo-element, so `::after:hover` is VALID per spec (Lynx rejected it
// blanket-fashion); id/class/:not()/combinators after a pseudo-element stay
// invalid.
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

// C++: css_selector_parser_test.cc::CSSSelectorParserTest.
// InvalidPseudoElementInNonRightmostCompound.
#[test]
fn pseudo_element_must_be_rightmost() {
    for selector in ["::before *", "::selection *", "::placeholder view"] {
        assert!(
            !Doc::selector_parses(selector),
            "`{selector}` must be rejected"
        );
    }
}

// C++: css_selector_parser_test.cc::CSSSelectorParserTest.UnexpectedPipe.
#[test]
fn malformed_namespace_pipes_rejected() {
    for selector in ["div | .c", "| div", " | div"] {
        assert!(
            !Doc::selector_parses(selector),
            "`{selector}` must be rejected"
        );
    }
}

// C++: css_selector_parser_test.cc::CSSSelectorParserTest.SerializedUniversal.
#[test]
fn universal_serializes_as_star() {
    assert_eq!(roundtrip("*").as_deref(), Some("*"));
}

// C++: css_selector_parser_test.cc::CSSSelectorParserTest.
// AttributeSelectorUniversalInvalid.
#[test]
fn universal_attribute_names_rejected() {
    for selector in ["[*]", "[*|*]"] {
        assert!(
            !Doc::selector_parses(selector),
            "`{selector}` must be rejected"
        );
    }
}

// C++: css_selector_parser_test.cc::CSSSelectorParserTest.ASCIILowerHTMLStrict
// — `\212a` decodes to U+212A (KELVIN SIGN), is not ASCII-lowercased, and the
// escape consumes its terminating space. Asserted behaviorally: the selector
// addresses an element whose tag/class/id/attribute name contains the literal
// codepoint.
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

// Skipped (skip-internal): CSSSelectorParserTest.ImplicitShadowCrossingCombinators
// — asserts Lynx's synthetic kUAShadow combinator + TagHistory chain, a
// native data structure with no stylo analog; the observable parse behavior
// is covered by `valid_selectors_parse_and_serialize_idempotently`.
//
// Skipped (skip-internal): LynxSelectorTest Lepus ToLepus()/FromLepus() CArray
// encode/decode — native serialization plumbing; stylo's parse→serialize
// idempotence above is the behavioral analog.
