//! Custom-property (`var()`) behavior ported from the `LynxJS` C++ engine:

mod common;

use common::{Doc, rgb};
use w3c_dom::NodeId;

fn styled(inline: &str) -> (Doc, NodeId) {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, inline);
    doc.flush();
    (doc, el)
}

fn styled_under_colored_root(inline: &str) -> (Doc, NodeId) {
    let mut doc = Doc::new();
    doc.set_inline(doc.root, "color: rgb(1, 2, 3)");
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, inline);
    doc.flush();
    (doc, el)
}

#[test]
fn simple_variable_substitution() {
    let (doc, el) = styled("--color: red; color: var(--color)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
}

#[test]
fn non_variable_value() {
    let (doc, el) = styled("color: red");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
}

#[test]
fn multiple_variables_in_one_string() {
    let (doc, el) =
        styled("--color: red; --size: 16px; color: var(--color); font-size: var(--size)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
    assert_eq!(doc.value(el, "font-size"), "16px");
}

#[test]
fn resolve_css_variables_typed_parse() {
    let (doc, el) = styled("--test: 20px; top: var(--test)");
    assert_eq!(doc.value(el, "top"), "20px");
}

#[test]
fn nested_variable_references() {
    let (doc, el) = styled("--primary: red; --secondary: var(--primary); color: var(--secondary)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
}

#[test]
fn deep_nested_variable_references() {
    let (doc, el) =
        styled("--a: red; --b: var(--a); --c: var(--b); --d: var(--c); color: var(--d)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
}

#[test]
fn variable_with_fallback() {
    let (doc, el) = styled("color: var(--primary, blue)");
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}

#[test]
fn non_cycle_fallback_behavior() {
    let (doc, el) = styled("--valid: blue; --out: var(--undefined, fallback)");
    assert_eq!(doc.value(el, "--out"), "fallback");
}

#[test]
fn substitution_resolved_with_fallback_resolved() {
    let (doc, el) = styled("--inner: green; color: var(--missing, var(--inner))");
    assert_eq!(doc.color(el), rgb(0, 128, 0));
}

#[test]
fn substitution_consume_property1() {
    let (doc, el) =
        styled("--a: var(--b, red); --b: var(--c, yellow); --c: blue; color: var(--d, var(--a))");
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}

#[test]
fn substitution_nested_variable() {
    let (doc, el) = styled(
        "--a: var(--b, red); --b: var(--c, yellow); --c: blue; \
         color: var(--d, var(--invalid-name, var(--invalid-name2, var(--a))))",
    );
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}

#[test]
fn cycle_detection_two_var() {
    let (doc, el) = styled_under_colored_root("--a: var(--b); --b: var(--a); color: var(--a)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

#[test]
fn multi_variable_cycle_detection() {
    let (doc, el) =
        styled_under_colored_root("--a: var(--b); --b: var(--c); --c: var(--a); color: var(--a)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

#[test]
fn self_referencing_variable() {
    let (doc, el) = styled_under_colored_root("--self: var(--self); color: var(--self)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

#[test]
fn cross_reference_cycle_detection() {
    let (doc, el) = styled_under_colored_root(
        "--z: var(--x) var(--y); --x: var(--z); --y: var(--x); color: var(--x)",
    );
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

#[test]
fn empty_variable_map() {
    let (doc, el) = styled_under_colored_root("color: var(--undefined)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

#[test]
fn resolve_css_variables_null_props() {
    let (doc, el) = styled("top: var(--missing)");
    assert_eq!(doc.value(el, "top"), "auto");
}

#[test]
fn complex_cycle_with_fallback() {
    let (doc, el) =
        styled_under_colored_root("--cyclic: var(--cyclic, fallback); color: var(--cyclic)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

#[test]
fn multiple_variables_with_cycle() {
    let (doc, el) = styled(
        "--valid: blue; --cycle1: var(--cycle2); --cycle2: var(--cycle1); \
         color: var(--valid); border: var(--cycle1)",
    );
    assert_eq!(doc.color(el), rgb(0, 0, 255));
    assert_eq!(doc.value(el, "border-top-style"), "none");
}

#[test]
fn mixed_cycle_and_valid_variables() {
    let (doc, el) = styled(
        "--a: var(--cyclic); --cyclic: var(--a); --valid: green; \
         color: var(--valid); --carry: var(--cyclic, fallback)",
    );
    assert_eq!(doc.color(el), rgb(0, 128, 0));
    assert_eq!(doc.value(el, "--carry"), "fallback");
}

#[test]
fn mixed_cycle_and_valid_variables2() {
    let (doc, el) = styled(
        "--cyclic: var(--cyclic-b, red); --cyclic-a: var(--cyclic-b, yellow); \
         --cyclic-b: var(--cyclic, pink); color: var(--cyclic-a, blue)",
    );
    assert_eq!(doc.color(el), rgb(255, 255, 0));
}

#[test]
fn cycle_with_fallback_correct_behavior() {
    let (doc, el) = styled_under_colored_root(
        "--a: var(--b, fallback-b); --b: var(--c, fallback-c); --c: var(--a, fallback-a); \
         --valid: blue; color: var(--a); --out: var(--nonexistent, fallback-value)",
    );
    assert_eq!(doc.value(el, "--a"), "");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
    assert_eq!(doc.value(el, "--out"), "fallback-value");
}

#[test]
fn substitute_all() {
    let (doc, el) = styled(
        "--a: var(--b, red); --b: var(--c, yellow); --c: blue; \
         --d: var(--d, var(--invalid-name, var(--invalid-name2, var(--a))))",
    );
    assert_eq!(doc.value(el, "--a"), "blue");
    assert_eq!(doc.value(el, "--b"), "blue");
    assert_eq!(doc.value(el, "--c"), "blue");
    assert_eq!(
        doc.value(el, "--d"),
        "",
        "self-cycle-with-fallback is guaranteed-invalid (W3C), not resolved to blue"
    );
}

#[test]
fn substitution_resolved_simple() {
    let (doc, el) =
        styled("--color: red; --size: 16px; color: var(--color); font-size: var(--size)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
    assert_eq!(doc.value(el, "font-size"), "16px");
}

#[test]
fn substitution_resolved_fallback() {
    let (doc, el) = styled("color: var(--primary, blue)");
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}
