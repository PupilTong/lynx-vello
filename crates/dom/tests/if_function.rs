//! css-values-5 §8.3 `if()` and §10 `sibling-index()` / `sibling-count()`,
//! computed through a real document.
//!
//! The grammar lives in the vendored Stylo fork (`lynx` feature). These tests
//! pin what reaches computed style: which branch a condition picks, what an
//! empty result does to the declaration, the cycle rule, substitution
//! functions nested in either half of a branch, and that a change to anything
//! a condition read re-cascades the element. The last part ports
//! web-platform-tests `css/css-values/if-*.html` cases.

mod common;

use std::fmt::Write as _;

use common::{Doc, rgb};
use dom::NodeId;
use euclid::default::Vector2D;

fn styled(inline: &str) -> (Doc, NodeId) {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, inline);
    doc.flush();
    (doc, el)
}

/// `inline` on a child of an element styled `parent`.
fn styled_under(parent: &str, inline: &str) -> (Doc, NodeId) {
    let mut doc = Doc::new();
    let outer = doc.el(doc.root, "view");
    doc.set_inline(outer, parent);
    let el = doc.el(outer, "view");
    doc.set_inline(el, inline);
    doc.flush();
    (doc, el)
}

fn custom(inline: &str, name: &str) -> String {
    let (doc, el) = styled(inline);
    doc.value(el, name)
}

// ---------------------------------------------------------------------------
// Branch selection, per test kind.

#[test]
fn style_test_picks_the_first_true_branch() {
    let value = |x: &str| {
        custom(
            &format!("--x: {x}; --p: if(style(--x: 1): one; style(--x: 2): two; else: other)"),
            "--p",
        )
    };
    assert_eq!(value("1"), "one");
    assert_eq!(value("2"), "two");
    assert_eq!(value("3"), "other");
}

#[test]
fn a_style_test_without_a_value_tests_for_a_value() {
    assert_eq!(
        custom("--x: 3; --p: if(style(--x): set; else: missing)", "--p"),
        "set"
    );
    assert_eq!(
        custom("--p: if(style(--x): set; else: missing)", "--p"),
        "missing"
    );
}

#[test]
fn no_true_branch_is_the_empty_token_stream() {
    // A custom property takes it as its value.
    assert_eq!(custom("--x: 3; --p: if(style(--x: 1): one)", "--p"), "");
    // Empty values, and branches after `else`, never matter.
    assert_eq!(
        custom("--x: 3; --p: if(style(--x: 3): ; else: other)", "--p"),
        ""
    );
    assert_eq!(custom("--x: 3; --p: if(style(--x: 3):)", "--p"), "");
    assert_eq!(
        custom(
            "--p: if(else: first; style(--x): second; else: third)",
            "--p"
        ),
        "first"
    );
}

#[test]
fn a_trailing_semicolon_is_allowed() {
    assert_eq!(custom("--x: 3; --p: if(style(--x: 3): yes;)", "--p"), "yes");
    assert_eq!(custom("--p: if(style(--x): yes; else: no;)", "--p"), "no");
}

#[test]
fn media_test_reads_the_device() {
    // The harness device is 800 × 600.
    assert_eq!(
        custom("--p: if(media(width >= 600px): wide; else: narrow)", "--p"),
        "wide"
    );
    assert_eq!(
        custom("--p: if(media(max-width: 1px): small; else: large)", "--p"),
        "large"
    );
    assert_eq!(
        custom(
            "--p: if(media((width > 700px) and (height < 700px)): yes; else: no)",
            "--p"
        ),
        "yes"
    );
    assert_eq!(
        custom("--p: if(media(orientation: landscape): l; else: p)", "--p"),
        "l"
    );
    // A media type is neither a feature nor a condition: unknown, so false.
    assert_eq!(custom("--p: if(media(screen): yes; else: no)", "--p"), "no");
}

#[test]
fn media_test_does_not_admit_tree_counting_functions() {
    assert_eq!(
        custom(
            "--p: if(media(width > calc(0px * sibling-index())): yes; else: no)",
            "--p"
        ),
        "no"
    );
}

#[test]
fn supports_test_reads_this_engines_grammar() {
    let value = |test: &str| custom(&format!("--p: if(supports({test}): yes; else: no)"), "--p");
    assert_eq!(value("display: flex"), "yes");
    assert_eq!(value("(display: flex)"), "yes");
    assert_eq!(value("display: invalid"), "no");
    // `display: table-cell` is outside the `lynx` grammar.
    assert_eq!(value("display: table-cell"), "no");
    assert_eq!(value("not (display: invalid)"), "yes");
    assert_eq!(value("(display: flex) and (width: 1px)"), "yes");
    assert_eq!(value("(display: flex) and (widthx: 1px)"), "no");
    assert_eq!(value("selector(view > text)"), "yes");
    // A bare name is neither a declaration nor a condition: unknown, so false.
    assert_eq!(value("display"), "no");
}

#[test]
fn if_applies_to_a_keyword_property() {
    let (doc, el) = styled("--mode: off; display: if(style(--mode: off): none; else: flex)");
    assert_eq!(doc.value(el, "display"), "none");
    let (doc, el) = styled("--mode: on; display: if(style(--mode: off): none; else: flex)");
    assert_eq!(doc.value(el, "display"), "flex");
}

#[test]
fn if_applies_to_a_length_property() {
    let (doc, el) = styled("--size: big; width: if(style(--size: big): 40px; else: 10px)");
    assert_eq!(doc.value(el, "width"), "40px");
    let (doc, el) =
        styled("--size: small; width: calc(if(style(--size: big): 40px; else: 10px) * 2)");
    assert_eq!(doc.value(el, "width"), "20px");
}

#[test]
fn if_applies_to_a_shorthand() {
    let (doc, el) = styled("--x: 1; padding: if(style(--x: 1): 1px 2px; else: 0)");
    assert_eq!(doc.value(el, "padding-top"), "1px");
    assert_eq!(doc.value(el, "padding-right"), "2px");
    assert_eq!(doc.value(el, "padding-bottom"), "1px");
    let (doc, el) = styled("--x: 2; padding: if(style(--x: 1): 1px 2px; else: 0)");
    assert_eq!(doc.value(el, "padding-right"), "0px");
}

#[test]
fn an_empty_result_is_invalid_at_computed_value_time() {
    // `color` inherits, so it takes the parent's value, not the earlier
    // declaration's.
    let (doc, el) = styled_under(
        "color: rgb(1, 2, 3)",
        "--x: 0; color: red; color: if(style(--x: 1): blue)",
    );
    assert_eq!(doc.color(el), rgb(1, 2, 3));
    // `width` does not, so it takes its initial value.
    let (doc, el) = styled("--x: 0; width: 7px; width: if(style(--x: 1): 3px)");
    assert_eq!(doc.value(el, "width"), "auto");
}

#[test]
fn an_empty_result_is_invalid_for_a_registered_property() {
    let mut doc = Doc::with_css(
        "@property --n { syntax: \"<integer>\"; inherits: false; initial-value: 5; }",
    );
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, "--n: if(style(--x: 1): 1)");
    doc.flush();
    assert_eq!(doc.value(el, "--n"), "5");
}

#[test]
fn a_value_that_breaks_the_argument_grammar_is_invalid_at_parse_time() {
    // The declaration is dropped while parsing, so the earlier one stays.
    for value in [
        "if()",
        "if(style(--x) green)",
        "if(style(--x): green;;)",
        "if(;)",
        "if(: green)",
        "if(style(--x): green!)",
        "if(!style(--x): green)",
        "if(style(--x): green; else)",
        // A condition excludes top-level commas and `{}` blocks.
        "if(style(--x: 1), else: red)",
        "if(style(--x: 1) {x}: red; else: blue)",
        "if({else}: red; else: blue)",
    ] {
        let (doc, el) = styled(&format!("--x: 1; color: rgb(4, 5, 6); color: {value}"));
        assert_eq!(doc.color(el), rgb(4, 5, 6), "`{value}` must not parse");
    }
}

#[test]
fn a_value_may_hold_commas_and_colons() {
    let (doc, el) = styled("--p: if(else: a, b: c)");
    assert_eq!(doc.value(el, "--p"), "a, b: c");
    let (doc, el) =
        styled("--x: 1; background-color: if(style(--x: 1): rgb(0, 128, 0); else: red)");
    assert_eq!(doc.style(el).get_background().background_color.clone(), {
        let (reference, el2) = styled("background-color: rgb(0, 128, 0)");
        reference
            .style(el2)
            .get_background()
            .background_color
            .clone()
    });
}

// ---------------------------------------------------------------------------
// Boolean logic (Appendix B).

#[test]
fn not_and_or_and_parentheses() {
    let value = |condition: &str| {
        custom(
            &format!("--a: 1; --b: 2; --p: if({condition}: yes; else: no)"),
            "--p",
        )
    };
    assert_eq!(value("not style(--a: 2)"), "yes");
    assert_eq!(value("style(--a: 1) and style(--b: 2)"), "yes");
    assert_eq!(value("style(--a: 1) and style(--b: 3)"), "no");
    assert_eq!(value("style(--a: 9) or style(--b: 2)"), "yes");
    assert_eq!(
        value("style(--a: 9) or (style(--b: 2) and (not style(--a: 9)))"),
        "yes"
    );
    // `not` only starts an expression: `and not` does not parse, so the
    // group is <general-enclosed>, unknown, and `false or unknown` is false.
    assert_eq!(
        value("style(--a: 9) or (style(--b: 2) and not style(--a: 9))"),
        "no"
    );
    assert_eq!(value("(style(--a: 1)) and (media(width > 1px))"), "yes");
    assert_eq!(value("style((--a: 1) and ((--b: 3) or (--b: 2)))"), "yes");
    assert_eq!(value("style(not (--a: 1))"), "no");
    // `and` and `or` do not mix without parentheses: the condition fails to
    // parse and the branch is skipped.
    assert_eq!(
        value("style(--a: 1) and style(--b: 2) or style(--a: 1)"),
        "no"
    );
}

#[test]
fn unknown_is_false_at_the_top_level_and_kleene_inside() {
    let value = |condition: &str| {
        custom(
            &format!("--a: 1; --p: if({condition}: yes; else: no)"),
            "--p",
        )
    };
    // `unknown(…)` and `(…)` are <general-enclosed>.
    assert_eq!(value("unknown(1)"), "no");
    assert_eq!(value("not unknown(1)"), "no");
    assert_eq!(value("unknown(1) or style(--a: 1)"), "yes");
    assert_eq!(value("unknown(1) and style(--a: 1)"), "no");
    assert_eq!(value("not (unknown(1) and style(--a: 2))"), "yes");
    // A style() whose contents are not a style query is <general-enclosed>
    // too, and so is one testing a non-custom property.
    assert_eq!(value("style(style(--a))"), "no");
    assert_eq!(value("not style(color: red)"), "no");
    // An ident is not a group at all: the branch does not parse.
    assert_eq!(value("style(--a: 1) and invalid"), "no");
}

// ---------------------------------------------------------------------------
// style() keywords, registered values and attr() taint.

#[test]
fn style_keywords_compare_against_the_cascade_defaults() {
    let mut doc = Doc::with_css(
        "@property --len { syntax: \"<length>\"; inherits: false; initial-value: 3px; }
         .outer { --inh: outer; --len: 30px; }",
    );
    let outer = doc.el(doc.root, "view.outer");
    let inner = doc.el(outer, "view");
    doc.set_inline(
        inner,
        "--p1: if(style(--inh: inherit): yes; else: no);
         --p2: if(style(--inh: unset): yes; else: no);
         --p3: if(style(--len: initial): yes; else: no);
         --p4: if(style(--len: inherit): yes; else: no);
         --p5: if(style(--len: unset): yes; else: no);
         --p6: if(style(--none: initial): yes; else: no);
         --p7: if(style(--inh: revert): yes; else: no)",
    );
    doc.flush();
    // `--inh` is inherited unregistered, so `unset` is `inherit`.
    assert_eq!(doc.value(inner, "--p1"), "yes");
    assert_eq!(doc.value(inner, "--p2"), "yes");
    // `--len` does not inherit: it is at its initial value, not the parent's.
    assert_eq!(doc.value(inner, "--p3"), "yes");
    assert_eq!(doc.value(inner, "--p4"), "no");
    assert_eq!(doc.value(inner, "--p5"), "yes");
    assert_eq!(doc.value(inner, "--p6"), "yes");
    // Cascade-dependent keywords are false.
    assert_eq!(doc.value(inner, "--p7"), "no");
}

#[test]
fn a_style_test_without_a_value_on_a_registered_property_compares_with_its_initial_value() {
    let mut doc = Doc::with_css(
        "@property --r { syntax: \"<integer>\"; inherits: false; initial-value: 0; }
         .t { --p: if(style(--r): changed; else: unchanged); }",
    );
    let unset = doc.el(doc.root, "view.t");
    let zero = doc.el(doc.root, "view.t");
    doc.set_inline(zero, "--r: 0");
    let one = doc.el(doc.root, "view.t");
    doc.set_inline(one, "--r: 1");
    doc.flush();
    assert_eq!(doc.value(unset, "--p"), "unchanged");
    assert_eq!(doc.value(zero, "--p"), "unchanged");
    assert_eq!(doc.value(one, "--p"), "changed");
}

#[test]
fn registered_values_compare_as_computed_values() {
    let mut doc = Doc::with_css(
        "@property --len { syntax: \"<length>\"; inherits: false; initial-value: 0px; }
         @property --c { syntax: \"<color>\"; inherits: false; initial-value: black; }",
    );
    let el = doc.el(doc.root, "view");
    doc.set_inline(
        el,
        "font-size: 10px; --len: 20px; --c: green;
         --p1: if(style(--len: 2em): yes; else: no);
         --p2: if(style(--len: calc(10px + 10px)): yes; else: no);
         --p3: if(style(--c: rgb(0, 128, 0)): yes; else: no);
         --p4: if(style(--len > 19px): yes; else: no)",
    );
    doc.flush();
    for name in ["--p1", "--p2", "--p3", "--p4"] {
        assert_eq!(doc.value(el, name), "yes", "{name}");
    }
}

#[test]
fn unregistered_values_compare_as_token_text() {
    assert_eq!(
        custom(
            "--x: 3; --p: if(style(--x: calc(1 + 2)): yes; else: no)",
            "--p"
        ),
        "no"
    );
}

#[test]
fn attr_taint_is_not_part_of_the_compared_value() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view[data-a=attr][data-n=2]");
    doc.set_inline(
        el,
        "--x: attr; --t: attr(data-a type(*));
         --p1: if(style(--x: attr(data-a type(*))): yes; else: no);
         --p2: if(style(--t: attr): yes; else: no)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "--p1"), "yes");
    assert_eq!(doc.value(el, "--p2"), "yes");
}

// ---------------------------------------------------------------------------
// The cycle rule and dependency ordering.

#[test]
fn a_query_on_the_property_being_substituted_is_a_cycle() {
    // The cycle makes `--p` invalid, so `var()` takes its fallback.
    for inline in [
        "--p: if(style(--p): a; else: b)",
        "--p: if(style(--p: 3): a; else: b)",
        "--p: if(style(not (--p)): a; else: b)",
        "--x: var(--p); --p: if(style(--x: 3): a; else: b)",
        "--x: 3; --p: if(style(--x: var(--p)): a; else: b)",
        "--x: 3; --p: if(style(--x: 3): var(--p); else: b)",
    ] {
        let (doc, el) = styled(&format!("{inline}; color: var(--p, green)"));
        assert_eq!(doc.color(el), rgb(0, 128, 0), "`{inline}` is a cycle");
    }
}

#[test]
fn a_branch_that_is_not_reached_creates_no_dependency() {
    // The second condition and the first value are never evaluated.
    assert_eq!(
        custom(
            "--x: 0; --p: if(style(--x: 0): one; style(--p): two)",
            "--p"
        ),
        "one"
    );
    assert_eq!(
        custom("--x: 3; --p: if(style(--x: 0): var(--p); else: two)", "--p"),
        "two"
    );
}

#[test]
fn a_queried_property_in_its_own_cycle_is_guaranteed_invalid() {
    assert_eq!(
        custom(
            "--y: var(--z); --z: var(--y); --p: if(style(--y): a; else: b)",
            "--p"
        ),
        "b"
    );
    assert_eq!(
        custom(
            "--x: var(--x); --y: 3; --p: if(style((--x) or (--y)): a; else: b)",
            "--p"
        ),
        "a"
    );
}

#[test]
fn a_queried_property_resolves_before_the_query() {
    // `--x` depends on `--y`, declared after it; `--p` must see the result.
    assert_eq!(
        custom(
            "--p: if(style(--x: 7): yes; else: no); --x: var(--y); --y: 7",
            "--p"
        ),
        "yes"
    );
    // A non-custom property reading an if() that reads a custom property.
    let (doc, el) =
        styled("width: if(style(--w: wide): 50px; else: 5px); --w: var(--v); --v: wide");
    assert_eq!(doc.value(el, "width"), "50px");
}

#[test]
fn a_font_relative_query_value_uses_this_elements_font_size() {
    let mut doc = Doc::with_css(
        "@property --len { syntax: \"<length>\"; inherits: false; initial-value: 0px; }",
    );
    let parent = doc.el(doc.root, "view");
    doc.set_inline(parent, "font-size: 10px");
    let el = doc.el(parent, "view");
    doc.set_inline(
        el,
        "font-size: 20px; --len: 40px; --p: if(style(--len: 2em): yes; else: no)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "--p"), "yes");
}

#[test]
fn a_font_relative_unit_in_a_branch_not_taken_is_no_dependency() {
    let css = "@property --l { syntax: \"<length>\"; inherits: false; initial-value: 1px; }";
    // The `2em` branch is not taken, so `--l` does not depend on the font
    // size and `font-size: var(--l)` is no cycle.
    let mut doc = Doc::with_css(css);
    let el = doc.el(doc.root, "view");
    doc.set_inline(
        el,
        "--x: 0; --l: if(style(--x: 1): 2em; else: 10px); font-size: var(--l)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "--l"), "10px");
    assert_eq!(doc.value(el, "font-size"), "10px");
    // Taken, the branch makes the cycle: both are invalid at computed-value
    // time, `--l` takes its initial value and `font-size` inherits.
    let mut doc = Doc::with_css(css);
    let parent = doc.el(doc.root, "view");
    doc.set_inline(parent, "font-size: 7px");
    let el = doc.el(parent, "view");
    doc.set_inline(
        el,
        "--x: 1; --l: if(style(--x: 1): 2em; else: 10px); font-size: var(--l)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "--l"), "1px");
    assert_eq!(doc.value(el, "font-size"), "7px");
    // A taken branch with a font-relative unit and no cycle computes against
    // this element's font size.
    let mut doc = Doc::with_css(css);
    let el = doc.el(doc.root, "view");
    doc.set_inline(
        el,
        "--x: 1; font-size: 10px; --l: if(style(--x: 1): 2em; else: 10px)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "--l"), "20px");
}

// ---------------------------------------------------------------------------
// Nesting.

#[test]
fn if_nests_in_var_fallbacks_and_in_itself() {
    let (doc, el) = styled("--x: 1; color: var(--missing, if(style(--x: 1): green; else: red))");
    assert_eq!(doc.color(el), rgb(0, 128, 0));
    assert_eq!(
        custom(
            "--x: 1; --y: 2; --p: if(style(--x: 1): if(style(--y: 2): both; else: x); else: none)",
            "--p"
        ),
        "both"
    );
    // An if() in a condition substitutes before the condition parses.
    assert_eq!(
        custom(
            "--x: 1; --p: if(if(style(--x: 1): else; else: style(--x: 9)): yes; else: no)",
            "--p"
        ),
        "yes"
    );
}

#[test]
fn substitution_functions_substitute_in_conditions_and_values() {
    // var() in a condition, including one that spells the whole test.
    assert_eq!(
        custom(
            "--x: 3; --y: 3; --p: if(style(--x: var(--y)): yes; else: no)",
            "--p"
        ),
        "yes"
    );
    assert_eq!(
        custom("--t: else; --p: if(var(--t): yes; else: no)", "--p"),
        "yes"
    );
    // var() in a value, substituted only when chosen.
    assert_eq!(
        custom("--v: chosen; --p: if(else: var(--v))", "--p"),
        "chosen"
    );
    // env() on both sides. The comparison is a registered one, because an
    // unregistered one compares token text and env() spells `0.0px`.
    let mut doc = Doc::with_css(
        "@property --x { syntax: \"<length>\"; inherits: false; initial-value: 1px; }",
    );
    let el = doc.el(doc.root, "view");
    doc.set_inline(
        el,
        "--x: 0px; --p: if(style(--x: env(safe-area-inset-top)): env(safe-area-inset-left); else: no)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "--p"), "0.0px");
    // attr() on both sides, typed and untyped.
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view[data-mode=dark][data-w=12]");
    doc.set_inline(
        el,
        "--p: if(style(--m: attr(data-mode type(<custom-ident>))): attr(data-w px); else: 0px);
         --m: dark; width: var(--p)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "width"), "12px");
}

#[test]
fn typed_attr_in_a_branch_of_a_non_custom_property() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view[data-w=9px]");
    doc.set_inline(
        el,
        "--x: 1; width: if(style(--x: 1): attr(data-w type(<length>)); else: 1px)",
    );
    doc.flush();
    assert_eq!(doc.value(el, "width"), "9px");
}

#[test]
fn a_guaranteed_invalid_value_in_a_style_feature_makes_it_false() {
    // `--y` is in a cycle: `(--x: var(--y))` is false, so its negation holds.
    assert_eq!(
        custom(
            "--x: 11; --y: var(--y); --p: if(style(not (--x: var(--y))): a; else: b)",
            "--p"
        ),
        "a"
    );
    assert_eq!(
        custom(
            "--p: if(style(--missing: var(--missing)): a; else: b)",
            "--p"
        ),
        "b"
    );
}

// ---------------------------------------------------------------------------
// Tree-counting functions.

const COUNTED: &str = "
@property --len { syntax: \"<length>\"; inherits: false; initial-value: 0px; }
@property --i { syntax: \"<integer>\"; inherits: false; initial-value: 0; }
.item {
  --len: calc(100px * sibling-index());
  --i: sibling-index();
  --index-is-two: if(style(sibling-index() = 2): yes; else: no);
  --even: if(style(mod(sibling-index(), 2) = 0): yes; else: no);
  --matches-len: if(style(--len = calc(100px * sibling-index())): yes; else: no);
  --matches-shifted: if(style(--len = calc(100px * (sibling-index() + 1))): yes; else: no);
  --count-is-three: if(style(sibling-count() = 3): yes; else: no);
  width: calc(1px * sibling-index());
}
";

fn counted() -> (Doc, Vec<NodeId>) {
    let mut doc = Doc::with_css(COUNTED);
    let list = doc.el(doc.root, "view");
    let items = doc.els(list, &["view.item", "view.item", "view.item"]);
    doc.flush();
    (doc, items)
}

fn column(doc: &Doc, items: &[NodeId], name: &str) -> Vec<String> {
    items.iter().map(|&item| doc.value(item, name)).collect()
}

#[test]
fn tree_counting_functions_in_style_ranges() {
    // wpt css/css-values/tree-counting/sibling-function-if-style-query.html
    let (doc, items) = counted();
    assert_eq!(column(&doc, &items, "--index-is-two"), ["no", "yes", "no"]);
    assert_eq!(column(&doc, &items, "--even"), ["no", "yes", "no"]);
    assert_eq!(column(&doc, &items, "--matches-len"), ["yes", "yes", "yes"]);
    assert_eq!(
        column(&doc, &items, "--matches-shifted"),
        ["no", "no", "no"]
    );
    assert_eq!(
        column(&doc, &items, "--count-is-three"),
        ["yes", "yes", "yes"]
    );
}

#[test]
fn tree_counting_functions_in_registered_values_and_lengths() {
    let (doc, items) = counted();
    assert_eq!(column(&doc, &items, "--i"), ["1", "2", "3"]);
    assert_eq!(column(&doc, &items, "--len"), ["100px", "200px", "300px"]);
    assert_eq!(column(&doc, &items, "width"), ["1px", "2px", "3px"]);
}

#[test]
fn tree_counting_values_are_per_parent() {
    // One list of one item and one of three, the same rule on every item: a
    // value must come from the item's own parent, in a property that does not
    // inherit (whose computed values stylo caches per rule) and in one that
    // does.
    for (rule, property, one, three) in [
        (
            "width: calc(1px * sibling-count())",
            "width",
            ["1px"],
            ["3px", "3px", "3px"],
        ),
        (
            "height: calc(1px * sibling-index())",
            "height",
            ["1px"],
            ["1px", "2px", "3px"],
        ),
        (
            "font-size: calc(10px * sibling-count())",
            "font-size",
            ["10px"],
            ["30px", "30px", "30px"],
        ),
        (
            "line-height: calc(10px * sibling-index())",
            "line-height",
            ["10px"],
            ["10px", "20px", "30px"],
        ),
    ] {
        let mut doc = Doc::with_css(&format!(".item {{ {rule}; }}"));
        let a = doc.el(doc.root, "view");
        let b = doc.el(doc.root, "view");
        let a_items = doc.els(a, &["view.item"]);
        let b_items = doc.els(b, &["view.item", "view.item", "view.item"]);
        doc.flush();
        assert_eq!(column(&doc, &a_items, property), one, "{rule}");
        assert_eq!(column(&doc, &b_items, property), three, "{rule}");
    }
}

#[test]
fn tree_counting_functions_follow_sibling_insertion_and_removal() {
    let (mut doc, items) = counted();
    let list = doc.dom.get(items[0]).unwrap().parent_id().unwrap();
    let first = doc.dom.create_element("view", ());
    doc.dom.add_class(first, "item");
    doc.dom.insert_before(list, first, Some(items[0]));
    doc.flush();
    assert_eq!(column(&doc, &items, "--i"), ["2", "3", "4"]);
    assert_eq!(column(&doc, &items, "--count-is-three"), ["no", "no", "no"]);
    doc.dom.remove_element(items[1]);
    doc.flush();
    assert_eq!(doc.value(items[2], "--i"), "3");
    assert_eq!(doc.value(items[2], "--count-is-three"), "yes");
}

#[test]
fn registered_integer_against_sibling_index() {
    let mut doc = Doc::with_css(
        "@property --pick { syntax: \"<integer>\"; inherits: true; initial-value: -1; }
         .list { --pick: 1; }
         .item { --chosen: if(style(--pick: calc(sibling-index() - 1)): yes; else: no); }",
    );
    let list = doc.el(doc.root, "view.list");
    let items = doc.els(list, &["view.item", "view.item", "view.item"]);
    doc.flush();
    assert_eq!(column(&doc, &items, "--chosen"), ["no", "yes", "no"]);
}

// ---------------------------------------------------------------------------
// The `select-index` recipe on a generic horizontal scroll container.

const PAGER: &str = "
@property --initial-index { syntax: \"<integer>\"; inherits: true; initial-value: -1; }
page { display: flex; width: 800px; height: 600px; }
.pager {
  display: flex; flex-direction: row; width: 100px; height: 100px; overflow: scroll;
  --initial-index:
    attr(select-index type(<integer>), attr(initial-select-index type(<integer>), -1));
}
.wrapper { display: flex; flex-direction: row; flex-shrink: 0; }
.item {
  display: flex; flex-shrink: 0; width: 100px; height: 100px;
  scroll-initial-target:
    if(style(--initial-index: calc(sibling-index() - 1)): nearest; else: none);
}
";

/// A 100 px wide pager with `count` 100 px pages, the pager spelled `pager`.
fn pager(pager: &str, count: usize) -> (Doc, NodeId, Vec<NodeId>) {
    let mut doc = Doc::with_css(PAGER);
    let container = doc.el(doc.root, pager);
    let items = (0..count).map(|_| doc.el(container, "view.item")).collect();
    (doc, container, items)
}

fn targets(doc: &Doc, items: &[NodeId]) -> Vec<bool> {
    items
        .iter()
        .map(|&item| doc.value(item, "scroll-initial-target") == "nearest")
        .collect()
}

#[test]
fn recipe_first_layout_lands_on_the_named_page() {
    let (mut doc, container, items) = pager("view.pager[select-index=2]", 4);
    doc.dom.commit();
    assert_eq!(targets(&doc, &items), [false, false, true, false]);
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(200.0, 0.0));
}

#[test]
fn recipe_without_the_attribute_has_no_target() {
    let (mut doc, container, items) = pager("view.pager", 4);
    doc.dom.commit();
    assert_eq!(targets(&doc, &items), [false; 4]);
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(0.0, 0.0));
}

#[test]
fn recipe_falls_back_to_the_second_attribute() {
    let (mut doc, container, items) = pager("view.pager[initial-select-index=1]", 4);
    doc.dom.commit();
    assert_eq!(targets(&doc, &items), [false, true, false, false]);
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(100.0, 0.0));
    // The first attribute wins when both parse.
    let (mut doc, container, _) = pager("view.pager[select-index=3][initial-select-index=1]", 4);
    doc.dom.commit();
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(300.0, 0.0));
}

#[test]
fn recipe_ignores_a_value_that_is_not_an_integer() {
    let (mut doc, container, items) = pager("view.pager[select-index=1.5]", 4);
    doc.dom.commit();
    assert_eq!(targets(&doc, &items), [false; 4]);
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(0.0, 0.0));
}

#[test]
#[ignore = "fork gap (pre-existing): the custom-property dependency walk counts a present \
            attribute as a valid attr() primary even when it does not parse, so it never \
            reaches the typed attr() in the fallback, which is then missing from the \
            attribute map and takes its own fallback: the result is -1 (style-assumptions §27)"]
fn recipe_a_non_integer_first_attribute_falls_through_to_the_second() {
    let (mut doc, container, _) = pager("view.pager[select-index=two][initial-select-index=3]", 4);
    doc.dom.commit();
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(300.0, 0.0));
}

#[test]
fn recipe_out_of_range_has_no_target() {
    for index in ["4", "10", "-1", "-5"] {
        let (mut doc, container, items) = pager(&format!("view.pager[select-index={index}]"), 4);
        doc.dom.commit();
        assert_eq!(targets(&doc, &items), [false; 4], "select-index={index}");
        assert_eq!(
            doc.dom.scroll_offset(container),
            Vector2D::new(0.0, 0.0),
            "select-index={index}"
        );
    }
}

#[test]
fn recipe_counts_pages_inside_one_wrapper() {
    let mut doc = Doc::with_css(PAGER);
    let container = doc.el(doc.root, "view.pager[select-index=1]");
    let wrapper = doc.el(container, "view.wrapper");
    let items = doc.els(wrapper, &["view.item", "view.item", "view.item"]);
    doc.dom.commit();
    assert_eq!(targets(&doc, &items), [false, true, false]);
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(100.0, 0.0));
}

#[test]
fn recipe_follows_an_attribute_change_after_layout() {
    let (mut doc, container, items) = pager("view.pager[select-index=1]", 4);
    doc.dom.commit();
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(100.0, 0.0));
    doc.set_attr(container, "select-index", "3");
    doc.dom.commit();
    assert_eq!(targets(&doc, &items), [false, false, false, true]);
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(300.0, 0.0));
}

#[test]
fn recipe_follows_sibling_insertion_and_removal_before_the_target() {
    let (mut doc, container, items) = pager("view.pager[select-index=2]", 4);
    doc.dom.commit();
    assert_eq!(targets(&doc, &items), [false, false, true, false]);
    // Scroll away, so that honouring a new target is visible.
    doc.dom.scroll_to(container, Vector2D::new(0.0, 0.0));

    let inserted = doc.dom.create_element("view", ());
    doc.dom.add_class(inserted, "item");
    doc.dom.insert_before(container, inserted, Some(items[0]));
    doc.dom.commit();
    // The page that is now third is the target.
    assert_eq!(targets(&doc, &items), [false, true, false, false]);
    assert_eq!(doc.value(inserted, "scroll-initial-target"), "none");
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(200.0, 0.0));

    doc.dom.scroll_to(container, Vector2D::new(0.0, 0.0));
    doc.dom.remove_element(items[0]);
    doc.dom.commit();
    // Back to four pages, the third being `items[2]` again.
    assert_eq!(targets(&doc, &items[1..]), [false, true, false]);
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(200.0, 0.0));
}

#[test]
fn recipe_removing_the_attribute_removes_the_target() {
    let (mut doc, container, items) = pager("view.pager[select-index=2]", 4);
    doc.dom.commit();
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(200.0, 0.0));
    doc.remove_attr(container, "select-index");
    doc.dom.commit();
    assert_eq!(targets(&doc, &items), [false; 4]);
    // Nothing scrolls the container back: a target only ever scrolls it to
    // the target.
    assert_eq!(doc.dom.scroll_offset(container), Vector2D::new(200.0, 0.0));
}

// ---------------------------------------------------------------------------
// Invalidation.

#[test]
fn an_inherited_queried_property_change_recascades() {
    // wpt css/css-values/if-invalidation.html, if-style-invalidation.html
    let mut doc =
        Doc::with_css(".t { --prop: if(style(--x: 3): true_value; else: false_value;); }");
    doc.set_inline(doc.root, "--x: 3");
    let el = doc.el(doc.root, "view.t");
    doc.flush();
    assert_eq!(doc.value(el, "--prop"), "true_value");
    let root = doc.root;
    doc.set_inline(root, "--x: 0");
    doc.flush();
    assert_eq!(doc.value(el, "--prop"), "false_value");
}

#[test]
fn an_own_queried_property_change_recascades() {
    let (mut doc, el) = styled("--x: 3; width: if(style(--x: 3): 30px; else: 10px)");
    assert_eq!(doc.value(el, "width"), "30px");
    doc.set_inline(el, "--x: 4; width: if(style(--x: 3): 30px; else: 10px)");
    doc.flush();
    assert_eq!(doc.value(el, "width"), "10px");
}

#[test]
fn an_attribute_read_in_a_branch_recascades() {
    let mut doc = Doc::with_css(
        ".t { width: if(style(--on: 1): attr(data-w type(<length>)); else: 1px); --on: 1; }",
    );
    let el = doc.el(doc.root, "view.t[data-w=5px]");
    doc.flush();
    assert_eq!(doc.value(el, "width"), "5px");
    doc.set_attr(el, "data-w", "8px");
    doc.flush();
    assert_eq!(doc.value(el, "width"), "8px");
}

#[test]
fn an_attribute_read_in_a_condition_recascades() {
    let mut doc = Doc::with_css(
        ".t { --m: dark; --p: if(style(--m: attr(data-m type(<custom-ident>))): on; else: off); }",
    );
    let el = doc.el(doc.root, "view.t[data-m=dark]");
    doc.flush();
    assert_eq!(doc.value(el, "--p"), "on");
    doc.set_attr(el, "data-m", "light");
    doc.flush();
    assert_eq!(doc.value(el, "--p"), "off");
}

#[test]
fn a_device_change_re_evaluates_media_tests() {
    // wpt css/css-values/if-media-invalidation.html, on the viewport height.
    let mut doc = Doc::with_css(
        ".t { --actual: if(media((height < 100px) or ((height >= 200px) and (height < 300px))): true_value; else: false_value;); }",
    );
    let el = doc.el(doc.root, "view.t");
    doc.dom.set_viewport(50.0, 50.0);
    doc.flush();
    assert_eq!(doc.value(el, "--actual"), "true_value");
    for (height, expected) in [
        (100.0, "false_value"),
        (200.0, "true_value"),
        (300.0, "false_value"),
    ] {
        doc.dom.set_viewport(50.0, height);
        doc.flush();
        assert_eq!(doc.value(el, "--actual"), expected, "height {height}");
    }
}

#[test]
fn inherit_of_a_non_inherited_registered_property_follows_the_parent() {
    const CSS: &str = "@property --l { syntax: \"<length>\"; inherits: false; initial-value: 1px; }
        .a { --l: 30px; }
        .b { --l: 40px; }";
    const CHILD: &str = "--l: 30px; --p: if(style(--l: inherit): yes; else: no);
        width: if(style(--l: inherit): 5px; else: 6px)";
    // The parent's own declaration changes.
    let mut doc = Doc::with_css(CSS);
    let parent = doc.el(doc.root, "view");
    doc.set_inline(parent, "--l: 30px");
    let el = doc.el(parent, "view");
    doc.set_inline(el, CHILD);
    doc.flush();
    assert_eq!(
        (doc.value(el, "--p"), doc.value(el, "width")),
        ("yes".into(), "5px".into())
    );
    doc.set_inline(parent, "--l: 40px");
    doc.flush();
    assert_eq!(
        (doc.value(el, "--p"), doc.value(el, "width")),
        ("no".into(), "6px".into())
    );
    // The parent matches another rule.
    let mut doc = Doc::with_css(CSS);
    let parent = doc.el(doc.root, "view.a");
    let el = doc.el(parent, "view");
    doc.set_inline(el, CHILD);
    doc.flush();
    assert_eq!(doc.value(el, "--p"), "yes");
    doc.remove_class(parent, "a");
    doc.add_class(parent, "b");
    doc.flush();
    assert_eq!(
        (doc.value(el, "--p"), doc.value(el, "width")),
        ("no".into(), "6px".into())
    );
}

#[test]
fn inherit_and_unset_of_inherited_properties_follow_the_parent() {
    let mut doc = Doc::with_css(
        "@property --li { syntax: \"<length>\"; inherits: true; initial-value: 1px; }",
    );
    let parent = doc.el(doc.root, "view");
    doc.set_inline(parent, "--li: 30px; --u: a");
    let el = doc.el(parent, "view");
    doc.set_inline(
        el,
        "--li: 30px; --u: a;
         --p1: if(style(--li: unset): yes; else: no);
         --p2: if(style(--u: inherit): yes; else: no)",
    );
    doc.flush();
    assert_eq!(
        (doc.value(el, "--p1"), doc.value(el, "--p2")),
        ("yes".into(), "yes".into())
    );
    doc.set_inline(parent, "--li: 40px; --u: b");
    doc.flush();
    assert_eq!(
        (doc.value(el, "--p1"), doc.value(el, "--p2")),
        ("no".into(), "no".into())
    );
}

// ---------------------------------------------------------------------------
// attr() taint through if() (css-values-5 §8.7.2).

/// The stylesheet of wpt `css/css-values/attr-security-if.html`, its `div`
/// rule on a class.
const ATTR_SECURITY_SHEET: &str = r#"
@property --some-string { syntax: "<string>"; inherits: false; initial-value: "empty"; }
.d { --condition-val: 3; --str: text; --true: true; --some-string: attr(data-foo); }
"#;

const URL: &str = "https://does-not-exist.test/404.png";
const URL2: &str = "https://does-not-exist-2.test/404.png";

fn with_data_foo(data_foo: &str, property: &str, value: &str) -> String {
    let mut doc = Doc::with_css(ATTR_SECURITY_SHEET);
    let el = doc.el(doc.root, "view.d");
    doc.set_attr(el, "data-foo", data_foo);
    doc.set_inline(el, &format!("{property}: {value}"));
    doc.flush();
    doc.value(el, property)
}

/// wpt `css/css-values/attr-security-if.html`, with `background-image`
/// holding the `url()` directly: `image-set()` is outside the `lynx`
/// grammar. Its second case (an `attr()` string as the image) has no
/// counterpart without `image-set()`, since a string is no `<image>`.
#[test]
fn wpt_attr_security_if() {
    let image = |url: &str| format!("url(\"{url}\")");
    // The custom property keeps the tainted text; only a URL is refused.
    assert_eq!(
        with_data_foo(URL, "--x", "if(style(--true): attr(data-foo);)"),
        format!("\"{URL}\"")
    );
    // A branch that is not taken does not taint.
    assert_eq!(
        with_data_foo(
            URL2,
            "background-image",
            &format!("if(style(--true): url({URL2}); else: attr(data-foo);)")
        ),
        image(URL2)
    );
    for (data_foo, value) in [
        // A value a style() test read is tainted.
        (URL, format!("if(style(--some-string): url({URL});)")),
        // A condition's own text is tainted.
        (
            "3",
            format!("if(style(--condition-val: attr(data-foo type(*))): url({URL});)"),
        ),
        // So is an earlier condition that was false.
        (
            "1",
            format!(
                "if(style(--condition-val: attr(data-foo type(*))): url({URL});
                    style(--true): url({URL}); else: url({URL});)"
            ),
        ),
        // And an if() inside a style() value.
        (
            "3",
            format!(
                "if(style(--condition-val: if(style(--true): attr(data-foo type(*));)): url({URL});)"
            ),
        ),
        (
            "3",
            format!("if(style(--condition-val >= attr(data-foo type(*))): url({URL});)"),
        ),
        (
            "3",
            format!("if(style(--condition-val < attr(data-foo type(*))): url({URL});)"),
        ),
        (
            "3",
            format!("if(style(--str < attr(data-foo type(*))): url({URL});)"),
        ),
        (
            "text",
            format!("if(style(--condition-val < attr(data-foo type(*))): url({URL});)"),
        ),
    ] {
        assert_eq!(
            with_data_foo(data_foo, "background-image", &value),
            "none",
            "{value} with data-foo={data_foo}"
        );
    }
    // A condition after the chosen one is not evaluated and does not taint.
    assert_eq!(
        with_data_foo(
            "attr(data-foo type(*))",
            "background-image",
            &format!(
                "if(style(--true): url({URL}); style(--condition-val): url({URL}); else: url({URL});)"
            )
        ),
        image(URL)
    );
    // The custom-property counterparts keep the URL as text.
    for value in [
        format!(
            "if(style(--condition-val: if(style(--true): attr(data-foo type(*));)): url({URL});)"
        ),
        format!("if(style(--condition-val >= attr(data-foo type(*))): url({URL});)"),
    ] {
        assert_eq!(
            with_data_foo("3", "--x", &value),
            format!("url({URL})"),
            "{value}"
        );
    }
}

#[test]
fn attr_taint_from_values_read_by_style_tests() {
    // `--t` holds an attr() value; testing it taints the chosen URL, whether
    // the test compares it or only asks whether it has a value.
    for condition in ["style(--t: \"x\")", "style(--t)", "not style(--t: \"y\")"] {
        let mut doc = Doc::new();
        let el = doc.el(doc.root, "view[data-foo=x]");
        doc.set_inline(
            el,
            &format!(
                "--t: attr(data-foo); background-image: if({condition}: url({URL}); else: none)"
            ),
        );
        doc.flush();
        assert_eq!(doc.value(el, "background-image"), "none", "{condition}");
    }
    // An untainted read leaves the URL alone.
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(
        el,
        &format!("--t: x; background-image: if(style(--t: x): url({URL}); else: none)"),
    );
    doc.flush();
    assert_eq!(doc.value(el, "background-image"), format!("url(\"{URL}\")"));
}

// ---------------------------------------------------------------------------
// web-platform-tests ports.

#[test]
fn wpt_if_initial_unregistered() {
    let (doc, el) = styled("color: if(style(--x: initial): green; else: red)");
    assert_eq!(doc.color(el), rgb(0, 128, 0));
}

#[test]
fn wpt_if_range_with_attr_crash() {
    // Pass if this does not panic.
    let (doc, el) = styled("color: if(style(attr(data-foo <number>) = 3): green; else: red;)");
    let _ = doc.color(el);
}

#[test]
fn wpt_if_function_revert_rule() {
    let mut doc = Doc::with_css(
        ".t1 { color: green; }
         .t1 { color: red; --x: 3; color: if(style(--x:1000):red; else:revert-rule); }
         .t2 { color: green; }
         .t2 { color: red; --x: 1000; color: if(style(--x:1000):revert-rule; else:red); }",
    );
    let t1 = doc.el(doc.root, "view.t1");
    let t2 = doc.el(doc.root, "view.t2");
    doc.flush();
    assert_eq!(doc.color(t1), rgb(0, 128, 0));
    assert_eq!(doc.color(t2), rgb(0, 128, 0));
}

/// The stylesheet of wpt `css/css-values/if-conditionals.html`, with its
/// `div` rule on a class.
const IF_CONDITIONALS_SHEET: &str = r#"
@property --string { syntax: "<string>"; inherits: true; initial-value: ""; }
@property --color { syntax: "<color>"; inherits: true; initial-value: blue; }
@property --length { syntax: "<length>"; inherits: false; initial-value: 3px; }
@property --length-inherited { syntax: "<length>"; inherits: true; initial-value: 3px; }
@property --percentage { syntax: "<percentage>"; inherits: true; initial-value: 30%; }
@property --number { syntax: "<number>"; inherits: true; initial-value: 3; }
@property --angle { syntax: "<angle>"; inherits: true; initial-value: 3deg; }
@property --time { syntax: "<time>"; inherits: true; initial-value: 3s; }
@property --resolution { syntax: "<resolution>"; inherits: true; initial-value: 3dpi; }
.d { font-size: 30px; }
.outer { --inherited: outer_value; --number: 30; --x: 11; --length: 30px; --length-inherited: 30px; }
.inner { --inherited: inner_value; }
"#;

/// What `--property` computes to when its `if()` declaration was dropped at
/// parse time: an earlier declaration of it that the test sets first.
const REJECTED: &str = "rejected-at-parse-time";

/// `(value, custom properties on the element, expected --property)`.
type ConditionalsCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

/// `(value, custom properties, expected --prop, data-foo)`.
type CycleCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
    Option<&'static str>,
);

/// Every case of wpt `css/css-values/if-conditionals.html` but eight: four
/// use the `color` media feature, which the servo device does not have, and
/// four expect `supports()` to accept `display: table-cell`, `list-item` or
/// `contents`, or a three-value `transform-origin`, all outside the `lynx`
/// grammar. An empty custom property value is the test's
/// `setProperty(name, '')`, which sets nothing. The test expects `""` both for
/// a value no branch of which matched and for one rejected at parse time; the
/// second is [`REJECTED`] here.
const IF_CONDITIONALS: &[ConditionalsCase] = &[
    (
        "if(style(--x: 3): true_value)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if( style( --x : 3 ) : true_value )",
        &[("--x", "3")],
        "true_value ",
    ),
    ("if(style(--x): true_value;)", &[("--x", "3")], "true_value"),
    (
        "if(  style(--x)  : true_value; )",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--x: 3): true_value;)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--x: 0): true_value;)",
        &[("--x", "0")],
        "true_value",
    ),
    ("if(style(--x: 0): ;)", &[("--x", "0")], ""),
    ("if(style(--x: 0): )", &[("--x", "0")], ""),
    (
        "if(style(--x: blue): true_value;)",
        &[("--x", "blue")],
        "true_value",
    ),
    (
        "if(style(--x: 3): true_value; else: false_value)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--non-existent: var(--non-existent)): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(--x: initial): true_value; else: false_value)",
        &[("--x", "")],
        "false_value",
    ),
    (
        "if(style(--x: initial): true_value; else: false_value)",
        &[("--x", "initial")],
        "true_value",
    ),
    (
        "if(style(--non-existent: initial): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(--x: initial): true_value; else: false_value)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(style(--inherited: inherit): true_value;\n          else: false_value)",
        &[("--inherited", "outer_value")],
        "true_value",
    ),
    (
        "if(style(--inherited: inherit): true_value;\n          else: false_value)",
        &[("--inherited", "inherit")],
        "true_value",
    ),
    (
        "if(style(--inherited: inherit): true_value;\n          else: false_value)",
        &[("--inherited", "unset")],
        "true_value",
    ),
    (
        "if(style(--inherited: inherit): true_value;\n          else: false_value)",
        &[("--inherited", "inner_value")],
        "false_value",
    ),
    (
        "if(style(--inherited: unset): true_value;\n          else: false_value)",
        &[("--inherited", "outer_value")],
        "true_value",
    ),
    (
        "if(style(--inherited: unset): true_value;\n          else: false_value)",
        &[("--inherited", "unset")],
        "true_value",
    ),
    (
        "if(style(--inherited: unset): true_value;\n          else: false_value)",
        &[("--inherited", "inner_value")],
        "false_value",
    ),
    (
        "if(style(--inherited: unset): true_value;\n          else: false_value)",
        &[("--inherited", "inherit")],
        "true_value",
    ),
    (
        "if(style(--x: 3): true_value; else:false_value)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--x: 3): true_value; else: false_value;)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--x: 0): true_value; else: false_value)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if( style( --x: 0 ) : true_value ;  else :  false_value)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(style(not (--unknown)): true_value;)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--x: 0): true_value;\n          else: )",
        &[("--x", "3")],
        "",
    ),
    (
        "if(style(--x: 3): ;\n          else: false_value)",
        &[("--x", "3")],
        "",
    ),
    (
        "if(style(--non-existent: 0): true_value;\n          else: false_value)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "calc(1 + 2)")],
        "false_value",
    ),
    (
        "if(style(--x: calc(1 + 2)): true_value;\n          else: false_value)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(style(--non-existent): true_value;\n          else: false_value;)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(style(style(--x)): true_value;\n          else: false_value;)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(style(var(--x)): true_value;\n          else: false_value;)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(style(--x: revert): true_value;\n          else: false_value)",
        &[("--x", "11")],
        "false_value",
    ),
    (
        "if(style(--x: revert-layer): true_value;\n          else: false_value)",
        &[("--x", "")],
        "false_value",
    ),
    (
        "if(style(--x):a)if(style(--x):b)",
        &[("--x", "3")],
        "a/**/b",
    ),
    (
        "if(style(--x!): true_value;\n          else: false_value)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(style(color: green): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(not style(--non-existent): true_value;\n          else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(not style(--x: 0): true_value;\n          else: false_value)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--x: 0) and style(--y: 3): true_value;\n          else: false_value)",
        &[("--x", "3"), ("--y", "3")],
        "false_value",
    ),
    (
        "if(style(--x: 3) and style(--y: 3): true_value;\n          else: false_value)",
        &[("--x", "3"), ("--y", "3")],
        "true_value",
    ),
    (
        "if(style(--x: 0) or style(--y: 3): true_value;\n          else: false_value)",
        &[("--x", "3"), ("--y", "3")],
        "true_value",
    ),
    (
        "if(style(--x: 0) or (style(--y: 3) and style(--z: 3)): true_value;\n          else: false_value)",
        &[("--x", "3"), ("--y", "3"), ("--z", "3")],
        "true_value",
    ),
    (
        "if(style(--non-existent): value1;\n          style(--x): value2;\n          else: value3;)",
        &[("--x", "3")],
        "value2",
    ),
    (
        "if(style(--x: 1): value1;\n          style(--x: 2): value2;\n          style(--x: 3): value3;)",
        &[("--x", "3")],
        "value3",
    ),
    (
        "if(style(--x: 1): value1;\n          style(--y: green): value2;\n          style(--z: 3px): value3;\n          else: value4;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "10px")],
        "value4",
    ),
    (
        "if(style(--x: 1): value1;\n          else: value2;\n          style(--y: green): value3;\n          style(--z: 3px): value4;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "10px")],
        "value2",
    ),
    (
        "if(style(--x: 1): value1;\n          else: value2;\n          style(--y: green): value3;\n          style(--z: 3px): value4;\n          else: value5;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "10px")],
        "value2",
    ),
    (
        "if(style(--x: 3): value1;\n          style(--y: green): value2;\n          style(--z: 3px): value3;\n          else: value4;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "10px")],
        "value1",
    ),
    (
        "if(style((--x: 3) and (not (--y: red) or (--z: 10px))): true_value;\n          else: false_value;)",
        &[("--x", "3"), ("--y", "green"), ("--z", "11px")],
        "false_value",
    ),
    (
        "if(style(--x: 3): value1;\n          style((--x: 3) and (not (--y: red) or (--z: 10px))): value2;\n          else: value3;)",
        &[("--x", "3"), ("--y", "green"), ("--z", "11px")],
        "value1",
    ),
    (
        "if(style((--x: 1) and (--y: green) and (--z: 3px)): true_value;\n          else: false_value;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "10px")],
        "false_value",
    ),
    (
        "if(style((--x: 3) and (--y: red) and (--z: 10px)): true_value;\n          else: false_value;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "10px")],
        "true_value",
    ),
    (
        "if(style((--x: 3) and ((not (--y: red)) or (--z: 10px))): true_value;\n          else: false_value;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "11px")],
        "false_value",
    ),
    (
        "if(style((--x: 3) and ((not (--y: red)) or (--z: 10px))): true_value;\n          else: false_value;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "10px")],
        "true_value",
    ),
    (
        "if(style((--x: 3) and ((not (--y: red)) or (--z: 10px))): true_value;\n          else: false_value;)",
        &[("--x", "3"), ("--y", "green"), ("--z", "11px")],
        "true_value",
    ),
    (
        "if(style((--x: 3) and (not (--y: red))): value1;\n          style(--z: 15px): value2;\n          else: value3;)",
        &[("--x", "3"), ("--y", "green"), ("--z", "11px")],
        "value1",
    ),
    (
        "if(style((--x: 3) and (not (--y: red))): value1;\n          style(--z: 15px): value2;\n          else: value3;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "15px")],
        "value2",
    ),
    (
        "if(style((--x: 3) and (not (--y: red))): value1;\n          style(--z: 15px): value2;\n          else: value3;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "11px")],
        "value3",
    ),
    (
        "if(style(--string: \"success\"): true_value;\n          else: false_value)",
        &[("--string", "\"success\"")],
        "true_value",
    ),
    (
        "if(style(--string: \"success\"): true_value;\n          else: false_value)",
        &[("--string", "\"fail\"")],
        "false_value",
    ),
    (
        "if(style(--number: 1): true_value;\n          else: false_value)",
        &[("--number", "1")],
        "true_value",
    ),
    (
        "if(style(--number: 3): true_value;\n          else: false_value)",
        &[("--number", "1")],
        "false_value",
    ),
    (
        "if(style(--number: calc(1 + 2)): true_value;\n          else: false_value)",
        &[("--number", "3")],
        "true_value",
    ),
    (
        "if(style(--number: 3): true_value;\n          else: false_value)",
        &[("--number", "calc(1 + 2)")],
        "true_value",
    ),
    (
        "if(style(--number: revert): true_value;\n          else: false_value)",
        &[("--number", "3")],
        "false_value",
    ),
    (
        "if(style(--number: revert-layer): true_value;\n          else: false_value)",
        &[("--number", "3")],
        "false_value",
    ),
    (
        "if(style(--length: 1px): true_value;\n          else: false_value)",
        &[("--length", "1px")],
        "true_value",
    ),
    (
        "if(style(--length: 3): true_value;\n          else: false_value)",
        &[("--length", "3px")],
        "false_value",
    ),
    (
        "if(style(--length: calc(1px + 2px)): true_value;\n          else: false_value)",
        &[("--length", "3px")],
        "true_value",
    ),
    (
        "if(style(--length: 3px): true_value;\n          else: false_value)",
        &[("--length", "calc(1px + 2px)")],
        "true_value",
    ),
    (
        "if(style(--length: 1em): true_value;\n          else: false_value)",
        &[("--length", "30px")],
        "true_value",
    ),
    (
        "if(style(--length: 30px): true_value;\n          else: false_value)",
        &[("--length", "1em")],
        "true_value",
    ),
    (
        "if(style(--length: 3): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(--length: initial): true_value;\n          else: false_value)",
        &[("--length", "3px")],
        "true_value",
    ),
    (
        "if(style(--length: initial): true_value;\n          else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(--length: inherit): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(--length: inherit): true_value;\n          else: false_value)",
        &[("--length", "30px")],
        "true_value",
    ),
    (
        "if(style(--length: unset): true_value;\n          else: false_value)",
        &[("--length", "3px")],
        "true_value",
    ),
    (
        "if(style(--length-inherited: inherit): true_value;\n          else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(--length-inherited: inherit): true_value;\n          else: false_value)",
        &[("--length-inherited", "30px")],
        "true_value",
    ),
    (
        "if(style(--length-inherited: inherit): true_value;\n          else: false_value)",
        &[("--length-inherited", "unset")],
        "true_value",
    ),
    (
        "if(style(--length-inherited: unset): true_value;\n          else: false_value)",
        &[("--length-inherited", "30px")],
        "true_value",
    ),
    (
        "if(style(--length-inherited: unset): true_value;\n          else: false_value)",
        &[("--length-inherited", "inherit")],
        "true_value",
    ),
    (
        "if(style(--percentage: 30%): true_value;\n          else: false_value)",
        &[("--percentage", "30%")],
        "true_value",
    ),
    (
        "if(style(--percentage: 90px): true_value;\n          else: false_value)",
        &[("--percentage", "30%")],
        "false_value",
    ),
    (
        "if(style(--percentage: 30px): true_value;\n          else: false_value)",
        &[("--percentage", "30%")],
        "false_value",
    ),
    (
        "if(style(--percentage: 90%): true_value;\n          else: false_value)",
        &[("--percentage", "3px")],
        "false_value",
    ),
    (
        "if(style(--color: green): true_value;\n          else: false_value)",
        &[("--color", "green")],
        "true_value",
    ),
    (
        "if(style(--color: rgb(0, 128, 0)): true_value;\n          else: false_value)",
        &[("--color", "green")],
        "true_value",
    ),
    (
        "if(style(--color: green): true_value;\n          else: false_value)",
        &[("--color", "rgb(0, 128, 0)")],
        "true_value",
    ),
    (
        "if(style(--color: #008000): true_value;\n          else: false_value)",
        &[("--color", "green")],
        "true_value",
    ),
    (
        "if(style(--color: green): true_value;\n          else: false_value)",
        &[("--color", "#008000")],
        "true_value",
    ),
    (
        "if(style(--color: green): true_value;\n          else: false_value)",
        &[("--color", "blue")],
        "false_value",
    ),
    (
        "if(style(--x: var(--x)): true_value;\n          else: false_value)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--non-existent: var(--non-existent)): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(--x: var(--y)): true_value;\n          else: false_value)",
        &[("--x", "3"), ("--y", "3")],
        "true_value",
    ),
    (
        "if(style(--x: var(--y)): true_value;\n          else: false_value)",
        &[("--x", "1"), ("--y", "3")],
        "false_value",
    ),
    (
        "if(style(--x: attr(data-foo type(<length>))): true_value;\n          else: false_value)",
        &[("--x", "30px")],
        "true_value",
    ),
    (
        "if(style(--x: attr(data-foo)): true_value;\n          else: false_value)",
        &[("--x", "\"30px\"")],
        "true_value",
    ),
    (
        "if(style(--length: attr(data-foo type(<length>))): true_value;\n          else: false_value)",
        &[("--length", "30px")],
        "true_value",
    ),
    (
        "if(style(--length: attr(data-foo type(<length>))): true_value;\n          else: false_value)",
        &[("--length", "30")],
        "false_value",
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "var(--y)"), ("--y", "3")],
        "true_value",
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "var(--y)"), ("--y", "1")],
        "false_value",
    ),
    (
        "if(style(--x: \"30px\"): true_value;\n          else: false_value)",
        &[("--x", "attr(data-foo)")],
        "true_value",
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "attr(data-foo)")],
        "false_value",
    ),
    (
        "if(style(--length: 30px): true_value;\n          else: false_value)",
        &[("--length", "attr(data-foo type(<length>))")],
        "true_value",
    ),
    (
        "if(style(--length: 30): true_value;\n          else: false_value)",
        &[("--length", "attr(data-foo type(<length>))")],
        "false_value",
    ),
    (
        "if(style(5 > 3): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(0 = 0): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(0 = 0px): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(0 = 0%): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(0 < 3px): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(5 > 3 !invalid): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(5 !invalid > 3): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(5 > 3 !): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(5.5 > 3): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(5.5 > 3.3): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(10em > 3px): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(1em > 1px): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(7px > 3px): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(3px > 3px): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(3turn > 3deg): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(3turn <= 3deg): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(3% >= 3%): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(3s > 3ms): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(3dppx > 96dpi): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(--length > initial): true_value;\n          else: false_value)",
        &[("--length", "5px")],
        "false_value",
    ),
    (
        "if(style(--x = initial): true_value; else: false_value)",
        &[("--x", "initial")],
        "false_value",
    ),
    (
        "if(style(--x <= 3): true_value; else: false_value)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--x >= --y): true_value; else: false_value)",
        &[("--x", "3"), ("--y", "3")],
        "true_value",
    ),
    (
        "if(style(--length > 3px): true_value; else: false_value)",
        &[("--length", "11px")],
        "true_value",
    ),
    (
        "if(style(--x > 3px): true_value; else: false_value)",
        &[("--x", "11px")],
        "true_value",
    ),
    (
        "if(style(--number >= 3): true_value; else: false_value)",
        &[("--number", "3")],
        "true_value",
    ),
    (
        "if(style(--x >= 3): true_value; else: false_value)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(--percentage > 3%): true_value; else: false_value)",
        &[("--percentage", "5%")],
        "true_value",
    ),
    (
        "if(style(--x > 3%): true_value; else: false_value)",
        &[("--x", "5%")],
        "true_value",
    ),
    (
        "if(style(--angle < 1turn): true_value; else: false_value)",
        &[("--angle", "1deg")],
        "true_value",
    ),
    (
        "if(style(--x < 1turn): true_value; else: false_value)",
        &[("--x", "1deg")],
        "true_value",
    ),
    (
        "if(style(--time <= 1000ms): true_value; else: false_value)",
        &[("--time", "1s")],
        "true_value",
    ),
    (
        "if(style(var(--time) <= 1000ms): true_value; else: false_value)",
        &[("--time", "1s")],
        "true_value",
    ),
    (
        "if(style(--x <= 1000ms): true_value; else: false_value)",
        &[("--x", "1s")],
        "true_value",
    ),
    (
        "if(style(3dppx > --resolution): true_value; else: false_value)",
        &[("--resolution", "96dpi")],
        "true_value",
    ),
    (
        "if(style(3dppx > --x): true_value; else: false_value)",
        &[("--x", "96dpi")],
        "true_value",
    ),
    (
        "if(style(--x + 1 >= --y): true_value; else: false_value)",
        &[("--x", "5"), ("--y", "3")],
        "false_value",
    ),
    (
        "if(style(--x >= --y + 1): true_value; else: false_value)",
        &[("--x", "5"), ("--y", "3")],
        "false_value",
    ),
    (
        "if(style(calc(--x + 1) >= --y): true_value; else: false_value)",
        &[("--x", "5"), ("--y", "3")],
        "false_value",
    ),
    (
        "if(style(--x >= calc(--y + 1)): true_value; else: false_value)",
        &[("--x", "5"), ("--y", "3")],
        "false_value",
    ),
    (
        "if(style(--x >= calc(3px + 3px)): true_value; else: false_value)",
        &[("--x", "7px")],
        "true_value",
    ),
    (
        "if(style(calc(var(--x) + 1) >= var(--y)): true_value; else: false_value)",
        &[("--x", "5"), ("--y", "3")],
        "true_value",
    ),
    (
        "if(style(var(--x) >= --x): true_value; else: false_value)",
        &[("--x", "3")],
        "true_value",
    ),
    (
        "if(style(3px > 3): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(3em > 3deg): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(1px >= 1%) or style(1px <= 1%): true_value; else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(style(--length > 3): true_value; else: false_value)",
        &[("--length", "11em")],
        "false_value",
    ),
    (
        "if(style(--number > 3px): true_value; else: false_value)",
        &[("--number", "11")],
        "false_value",
    ),
    (
        "if(style(--x >= 3): true_value; else: false_value)",
        &[("--x", "3px")],
        "false_value",
    ),
    (
        "if(style(--length >= 30px): true_value;\n                                    else: false_value)",
        &[("--length", "attr(data-foo type(<length>))")],
        "true_value",
    ),
    (
        "if(style(10px <= 10px < 11px): true_value; else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(style(3 < --x <= 5): true_value; else: false_value)",
        &[("--x", "5")],
        "true_value",
    ),
    (
        "if(style(--x >= --y > --z): true_value; else: false_value)",
        &[("--x", "3"), ("--y", "3"), ("--z", "3")],
        "false_value",
    ),
    (
        "if(style(--x >= --y > --z): true_value; else: false_value)",
        &[("--x", "3"), ("--y", "3"), ("--z", "1")],
        "true_value",
    ),
    (
        "if(media(max-width: 1px): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(media((max-width: 1px)): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(media(height <= 999999px): true_value;\n          else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(supports(display): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(supports(display: invalid): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(supports(selector(h2 > p)): true_value;\n          else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(supports((selector(h2 > p))): true_value;\n          else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if(supports((display: invalid) and (display: list-item) and (display: contents)): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if(supports((display: table-cell) and (invalid: list-item) and (display: contents)): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    (
        "if((media(min-width: 1px)) or (style(--x)): true_value;\n          else: false_value)",
        &[],
        "true_value",
    ),
    (
        "if((media(height <= 999999px)) and style(--non-existent): true_value;\n          else: false_value)",
        &[],
        "false_value",
    ),
    ("if()", &[("--x", "3")], REJECTED),
    ("if(style())", &[("--x", "3")], REJECTED),
    ("if(style(--x: 3) !)", &[("--x", "3")], REJECTED),
    (
        "if(style(--x: 3) true_value;\n          else: false_value)",
        &[("--x", "3")],
        REJECTED,
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value!)",
        &[("--x", "3")],
        REJECTED,
    ),
    (
        "if(!style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "3")],
        REJECTED,
    ),
    (
        "if(style(--x) and invalid: true_value;\n          else: false_value)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(invalid or style(--x): true_value;\n          else: false_value)",
        &[("--x", "3")],
        "false_value",
    ),
    (
        "if(style(not (--x: 5) or (--z: 10px)): true_value;\n          else: false_value;)",
        &[("--x", "3"), ("--y", "green"), ("--z", "11px")],
        "false_value",
    ),
    ("if(style(--x: 0): true_value;)", &[("--x", "3")], ""),
    (
        "if(style(--non-existent): true_value;)",
        &[("--x", "3")],
        "",
    ),
    (
        "if(style(--non-existent: 3): true_value;)",
        &[("--x", "3")],
        "",
    ),
    ("if(style(--invalid): value)", &[], ""),
    (
        "if(style(--invalid): var(--x))",
        &[("--x", "true_value")],
        "",
    ),
    (
        "if(style(--x: 1): value1;\n          style(--y: green): value2;\n          style(--z: 3px): value3;)",
        &[("--x", "3"), ("--y", "red"), ("--z", "10px")],
        "",
    ),
    (
        "if(style(--x: attr(data-attr type(*))): true_value; else: false_value)",
        &[("--x", "attr")],
        "true_value",
    ),
    (
        "if(style(--x: attr): true_value; else: false_value)",
        &[("--x", "attr(data-attr type(*))")],
        "true_value",
    ),
    (
        "if(var(--else): true_value; else: false_value)",
        &[("--else", "else")],
        "true_value",
    ),
];

#[test]
fn wpt_if_conditionals() {
    for (value, properties, expected) in IF_CONDITIONALS {
        let mut doc = Doc::with_css(IF_CONDITIONALS_SHEET);
        let outer = doc.el(doc.root, "view.d.outer");
        let inner = doc.el(outer, "view.d.inner[data-foo=30px][data-attr=attr]");
        let mut inline = String::new();
        for (name, value) in *properties {
            if !value.is_empty() {
                write!(inline, "{name}: {value}; ").unwrap();
            }
        }
        write!(inline, "--property: {REJECTED}; --property: {value}").unwrap();
        doc.set_inline(inner, &inline);
        doc.flush();
        assert_eq!(
            doc.value(inner, "--property"),
            *expected,
            "{value:?} with {properties:?}"
        );
    }
}

/// Every case of wpt `css/css-values/if-cycle.html`.
const IF_CYCLE: &[CycleCase] = &[
    (
        "if(style(--x: 3): var(--prop); else: value)",
        &[("--x", "3")],
        "",
        None,
    ),
    (
        "if(style(--x: 3): value; else: var(--prop))",
        &[("--x", "0")],
        "",
        None,
    ),
    (
        "if(style(--x: 3): attr(data-foo type(*)); else: value)",
        &[("--x", "3")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--x: 3): value; else: attr(data-foo type(*)))",
        &[("--x", "0")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--prop): var(--prop); else: value)",
        &[],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--x): var(--prop); else: value)",
        &[("--x", "var(--prop)")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--prop: 3): true_value;\n          else: false_value)",
        &[("--prop", "3")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "var(--prop)")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(not (--prop)): true_value;\n          else: false_value)",
        &[],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(not (--x: var(--y))): true_value;\n          else: false_value)",
        &[("--x", "11"), ("--y", "var(--prop)")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style((--prop) or (--y)): true_value;\n          else: false_value)",
        &[("--y", "3")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style((--prop) and (--y)): true_value;\n          else: false_value)",
        &[("--y", "3")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--x: var(--prop)): true_value;\n          else: false_value)",
        &[("--x", "3")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--x: var(--y)): true_value;\n          else: false_value)",
        &[("--x", "3"), ("--y", "var(--prop)")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "attr(data-foo type(*))")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "var(--y)"), ("--y", "attr(data-foo type(*))")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--x: attr(data-foo type(*))): true_value;\n          else: false_value)",
        &[("--x", "30px")],
        "",
        Some("var(--prop)"),
    ),
    (
        "if(style(--y): true_value;\n          else: false_value)",
        &[("--y", "var(--x)"), ("--x", "var(--y)")],
        "false_value",
        Some("var(--prop)"),
    ),
    (
        "if(style(not (--x: var(--y))): true_value;\n          else: false_value)",
        &[("--x", "11"), ("--y", "var(--y)")],
        "true_value",
        Some("var(--prop)"),
    ),
    (
        "if(style(not (--x: var(--y))): true_value;\n          else: false_value)",
        &[("--x", "11"), ("--y", "var(--y)")],
        "true_value",
        Some("var(--prop)"),
    ),
    (
        "if(style((--x) or (--y)): true_value;\n          else: false_value)",
        &[("--x", "var(--x)"), ("--y", "3")],
        "true_value",
        Some("var(--prop)"),
    ),
    (
        "if(style((--x) and (--y)): true_value;\n          else: false_value)",
        &[("--x", "var(--x)"), ("--y", "3")],
        "false_value",
        Some("var(--prop)"),
    ),
    (
        "if(style((not (--z)) or (--y)): true_value;\n          else: false_value)",
        &[("--z", "var(--x)"), ("--x", "var(--z)"), ("--y", "3")],
        "true_value",
        Some("var(--prop)"),
    ),
    (
        "if(style((not (--z)) and (--y)): true_value;\n          else: false_value)",
        &[("--z", "var(--x)"), ("--x", "var(--z)"), ("--y", "3")],
        "true_value",
        Some("var(--prop)"),
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "attr(data-foo type(*))")],
        "false_value",
        Some("var(--x)"),
    ),
    (
        "if(style(--x: 3): true_value;\n          else: false_value)",
        &[("--x", "var(--y)"), ("--y", "attr(data-foo type(*))")],
        "false_value",
        Some("var(--y)"),
    ),
    (
        "if(style(--x: var(--y)): true_value;\n          else: false_value)",
        &[("--x", "3"), ("--y", "var(--z)"), ("--z", "var(--y)")],
        "false_value",
        Some("var(--y)"),
    ),
    (
        "if(style(--x: var(--y)): true_value;\n          else: false_value)",
        &[("--x", "3"), ("--y", "var(--y)")],
        "false_value",
        Some("var(--y)"),
    ),
    (
        "if(style(--x: attr(data-foo type(*))): true_value;\n          else: false_value)",
        &[("--x", "30px")],
        "false_value",
        Some("attr(data-foo type(*))"),
    ),
    (
        "if(style(--x: attr(data-foo, var(--y))): true_value;\n          else: false_value)",
        &[("--x", "\"30px\""), ("--y", "var(--y)")],
        "true_value",
        Some("30px"),
    ),
    (
        "if(style(--x: 0): value1; style(--prop): value2)",
        &[("--x", "0")],
        "value1",
        Some("30px"),
    ),
    (
        "if(style(--x: 3): value1;\n          style(--y: 3): value2;\n          else: value3)",
        &[("--x", "3"), ("--y", "var(--prop)")],
        "value1",
        Some("30px"),
    ),
    (
        "if(style(--x: 0): value1; style(--y): value2)",
        &[("--x", "0"), ("--y", "var(--y)")],
        "value1",
        Some("30px"),
    ),
    (
        "if(style(--x: 3): value1;\n          style(--y: 3): value2;\n          else: value3)",
        &[("--x", "3"), ("--y", "var(--y)")],
        "value1",
        Some("30px"),
    ),
    (
        "if(style(--x: 0): var(--prop); else: value)",
        &[("--x", "3")],
        "value",
        Some("30px"),
    ),
    (
        "if(style(--x: 0): value; else: var(--prop))",
        &[("--x", "0")],
        "value",
        Some("30px"),
    ),
    (
        "if(style(--x: 3): value1;\n          style(--y: 3): var(--prop);\n          else: value3)",
        &[("--x", "3"), ("--y", "0")],
        "value1",
        Some("30px"),
    ),
    (
        "if(style(--x: 3): var(--prop);\n          style(--y: 3): var(--prop);\n          else: value3)",
        &[("--x", "0"), ("--y", "0")],
        "value3",
        Some("30px"),
    ),
    (
        "if(style(--x: 3): var(--x);\n          else: value)",
        &[("--x", "3")],
        "3",
        Some("30px"),
    ),
    (
        "if(style(--x: var(--y)): var(--y);\n          else: value)",
        &[("--x", "3"), ("--y", "3")],
        "3",
        Some("30px"),
    ),
    (
        "if(style(--x: var(--x)): true_value;\n          else: false_value)",
        &[("--x", "3")],
        "true_value",
        Some("30px"),
    ),
    (
        "if(style(--non-existent: var(--non-existent)): true_value;\n          else: false_value)",
        &[],
        "false_value",
        Some("30px"),
    ),
    (
        "if(style(--x: 3): true_value;\n          else: var(--x))",
        &[("--x", "1")],
        "1",
        Some("30px"),
    ),
    (
        "if(style(--x: var(--x)): value1;\n          style(--x: 3): value2;\n          else: value3;)",
        &[("--x", "3")],
        "value1",
        Some("30px"),
    ),
    (
        "if(style(--x: var(--y)): value1;\n          style(--z: 3): value2;\n          else: value3;)",
        &[("--x", "var(--y)"), ("--y", "var(--y)"), ("--z", "3")],
        "value2",
        Some("30px"),
    ),
    (
        "if(style(--z: var(--y)): value1;\n          style(--z: 3): value2;\n          else: value3;)",
        &[("--x", "var(--y)"), ("--y", "var(--y)"), ("--z", "3")],
        "value2",
        Some("30px"),
    ),
];

#[test]
fn wpt_if_cycle() {
    for (value, properties, expected, data_foo) in IF_CYCLE {
        let mut doc = Doc::new();
        let el = doc.el(doc.root, "view");
        if let Some(data_foo) = data_foo {
            doc.set_attr(el, "data-foo", data_foo);
        }
        let mut inline = String::new();
        for (name, value) in *properties {
            write!(inline, "{name}: {value}; ").unwrap();
        }
        write!(inline, "--prop: {value}").unwrap();
        doc.set_inline(el, &inline);
        doc.flush();
        assert_eq!(
            doc.value(el, "--prop"),
            *expected,
            "{value:?} with {properties:?}, data-foo {data_foo:?}"
        );
    }
}
