//! Custom-property (`var()`) behavior ported from the `LynxJS` C++ engine:
//!
//! - `core/renderer/css/css_value_unittest.cc` (`CSSValueSubstitutionTest.*` — the `var()`
//!   substitution engine)
//! - `core/renderer/css/css_variable_handler_unittest.cc`
//!   (`CSSVariableHandlerTest.ResolveCSSVariables*` — the fiber resolve path)
//!
//! Scope: `enableCSSSelector = true` (NG selector path) and
//! `enableRemoveCSSScope = true` (global styles) only — the same scope the
//! shared harness (`tests/common/mod.rs`) documents.
//!
//! Expectation policy (see `docs/style-assumptions.md` and
//! `docs/tracking/deviations.md`): `var()` is a real W3C feature, so every
//! assertion here ports the inventory's `ours_expected` (**W3C-correct**),
//! *not* the C++ `lynx_expected`. Two Lynx quirks are therefore corrected:
//!
//!  1. **Fallback whitespace** — Lynx keeps the literal space after the comma (`var(--x, blue)` →
//!     `" blue"`); stylo/W3C trims to `"blue"`. Every fallback assertion below expects the trimmed
//!     form.
//!  2. **Self-cycle-with-fallback** — Lynx walks the fallback chain and resolves `--d: var(--d, …)`
//!     to a value; W3C makes a self-referential custom property *guaranteed-invalid* regardless of
//!     the fallback (a fallback never breaks a cycle). See [`substitute_all`].
//!
//! The C++ tests operate on Lynx's internal `CSSValue`/substitution-string
//! model and often assert a concatenated substitution string across several
//! properties. stylo resolves single-phase and per-declaration, so each such
//! case is ported as independent declarations whose *computed values* are
//! asserted: a color-valued result via [`Doc::color`], a length via
//! [`Doc::value`], and a raw token-stream (arbitrary-ident) result by reading
//! the carrying **custom property's own computed value** (also via
//! [`Doc::value`], which serializes `PropertyDeclarationId::Custom`).
//!
//! Skipped C++ cases (internal plumbing or Lynx-only `{{…}}` syntax / coarse
//! fallback machinery) are accounted for in the footer block.

mod common;

use common::{Doc, rgb};
use stylo_dom::ElementId;

/// One `view` under the page root, carrying `inline`, flushed. Returns the
/// element so several computed values can be read from it.
fn styled(inline: &str) -> (Doc, ElementId) {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, inline);
    doc.flush();
    (doc, el)
}

/// One `view` under a page root whose `color` is a distinctive `rgb(1, 2, 3)`.
///
/// `color` is an inherited property, so an invalid-at-computed-value-time
/// `color` declaration on the child leaves it `unset` — i.e. inheriting the
/// root's `rgb(1, 2, 3)`. Asserting the child ends up exactly `rgb(1, 2, 3)`
/// proves a `var()` reference was dropped (IACVT) rather than resolving to
/// some stray value.
fn styled_under_colored_root(inline: &str) -> (Doc, ElementId) {
    let mut doc = Doc::new();
    doc.set_inline(doc.root, "color: rgb(1, 2, 3)");
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, inline);
    doc.flush();
    (doc, el)
}

// --- Basic substitution ---------------------------------------------------

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.SimpleVariableSubstitution
#[test]
fn simple_variable_substitution() {
    let (doc, el) = styled("--color: red; color: var(--color)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.NonVariableValue
#[test]
fn non_variable_value() {
    // A value with no var() passes through custom-property resolution intact.
    let (doc, el) = styled("color: red");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.MultipleVariablesInOneString
#[test]
fn multiple_variables_in_one_string() {
    // C++ asserted a concatenated "color: red; font-size: 16px"; ported as two
    // independent declarations, each computed value checked.
    let (doc, el) =
        styled("--color: red; --size: 16px; color: var(--color); font-size: var(--size)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
    assert_eq!(doc.value(el, "font-size"), "16px");
}

// C++: css_variable_handler_unittest.cc::CSSVariableHandlerTest.ResolveCSSVariables
#[test]
fn resolve_css_variables_typed_parse() {
    // After substitution the value is re-parsed through the property grammar
    // into a typed length, not left as a string: `top: var(--test)` with
    // --test=20px computes to the 20px length.
    let (doc, el) = styled("--test: 20px; top: var(--test)");
    assert_eq!(doc.value(el, "top"), "20px");
}

// --- Indirection chains ---------------------------------------------------

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.NestedVariableReferences
#[test]
fn nested_variable_references() {
    let (doc, el) = styled("--primary: red; --secondary: var(--primary); color: var(--secondary)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.DeepNestedVariableReferences
#[test]
fn deep_nested_variable_references() {
    let (doc, el) =
        styled("--a: red; --b: var(--a); --c: var(--b); --d: var(--c); color: var(--d)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
}

// --- Fallback (whitespace trimmed to W3C) ---------------------------------

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.VariableWithFallback
#[test]
fn variable_with_fallback() {
    // --primary undefined → the fallback is used. Lynx keeps a leading space
    // (" blue"); W3C trims. Asserting a color makes the whitespace irrelevant
    // to *this* property, so the sibling custom-property read below pins the
    // trim directly.
    let (doc, el) = styled("color: var(--primary, blue)");
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}

#[test]
fn variable_fallback_can_resolve_to_css_wide_keyword() {
    let mut doc = Doc::new();
    doc.set_inline(doc.root, "color: rgb(1, 2, 3)");
    let child = doc.el(doc.root, "view");
    doc.set_inline(child, "color: var(--missing, inherit)");
    doc.flush();

    assert_eq!(
        doc.color(child),
        rgb(1, 2, 3),
        "a substituted CSS-wide keyword must re-enter the standard cascade path"
    );
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.NonCycleFallbackBehavior
#[test]
fn non_cycle_fallback_behavior() {
    // A plain undefined var with an arbitrary-ident fallback. Read the carrying
    // custom property's computed value to observe the trimmed token stream
    // directly ("fallback", not Lynx's " fallback").
    let (doc, el) = styled("--valid: blue; --out: var(--undefined, fallback)");
    assert_eq!(doc.value(el, "--out"), "fallback");
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.SubstitutionResolvedWithFallbackResolved
#[test]
fn substitution_resolved_with_fallback_resolved() {
    // A var() *inside* a fallback is itself resolved: var(--missing, var(--inner)).
    let (doc, el) = styled("--inner: green; color: var(--missing, var(--inner))");
    assert_eq!(doc.color(el), rgb(0, 128, 0));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.SubstitutionConsumeProperty1
#[test]
fn substitution_consume_property1() {
    // Undefined --d falls back to var(--a), which resolves through the chain to
    // blue. (The C++ handle_func / consumed-var-map sub-assert is internal
    // dependency-tracking plumbing and is not ported.)
    let (doc, el) =
        styled("--a: var(--b, red); --b: var(--c, yellow); --c: blue; color: var(--d, var(--a))");
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.SubstitutionNestedVariable
#[test]
fn substitution_nested_variable() {
    // Deeply nested fallbacks resolve to blue. Lynx accretes one leading space
    // per fallback level ("   blue"); W3C trims to "blue" (asserted as a color).
    let (doc, el) = styled(
        "--a: var(--b, red); --b: var(--c, yellow); --c: blue; \
         color: var(--d, var(--invalid-name, var(--invalid-name2, var(--a))))",
    );
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}

// --- Cycles → invalid at computed-value time ------------------------------

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.CycleDetection
#[test]
fn cycle_detection_two_var() {
    // --a ↔ --b two-node cycle → both guaranteed-invalid; color: var(--a) with
    // no fallback → IACVT → color stays inherited (rgb(1, 2, 3)).
    let (doc, el) = styled_under_colored_root("--a: var(--b); --b: var(--a); color: var(--a)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.MultiVariableCycleDetection
#[test]
fn multi_variable_cycle_detection() {
    // --a → --b → --c → --a three-node cycle.
    let (doc, el) =
        styled_under_colored_root("--a: var(--b); --b: var(--c); --c: var(--a); color: var(--a)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.SelfReferencingVariable
#[test]
fn self_referencing_variable() {
    // --self: var(--self) is a one-node cycle → guaranteed-invalid.
    let (doc, el) = styled_under_colored_root("--self: var(--self); color: var(--self)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.CrossReferenceCycleDetection
#[test]
fn cross_reference_cycle_detection() {
    // A definition referencing two vars still participates in the cycle:
    // --z: var(--x) var(--y); --x: var(--z); --y: var(--x).
    let (doc, el) = styled_under_colored_root(
        "--z: var(--x) var(--y); --x: var(--z); --y: var(--x); color: var(--x)",
    );
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.EmptyVariableMap
#[test]
fn empty_variable_map() {
    // Undefined var, no fallback, nothing defined → IACVT → color inherited.
    let (doc, el) = styled_under_colored_root("color: var(--undefined)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

// C++: css_variable_handler_unittest.cc::CSSVariableHandlerTest.ResolveCSSVariablesNullProps
#[test]
fn resolve_css_variables_null_props() {
    // `top: var(--missing)` with nothing defined and no fallback → IACVT → the
    // declaration is dropped, so `top` stays at its initial `auto`.
    let (doc, el) = styled("top: var(--missing)");
    assert_eq!(doc.value(el, "top"), "auto");
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.ComplexCycleWithFallback
#[test]
fn complex_cycle_with_fallback() {
    // A fallback in the *definition* does not break the cycle:
    // --cyclic: var(--cyclic, fallback) is still guaranteed-invalid, and
    // color: var(--cyclic) with no query-level fallback → IACVT.
    let (doc, el) =
        styled_under_colored_root("--cyclic: var(--cyclic, fallback); color: var(--cyclic)");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
}

// --- Cycles mixed with valid vars -----------------------------------------

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.MultipleVariablesWithCycle
#[test]
fn multiple_variables_with_cycle() {
    // --valid resolves; a cyclic var with *no* fallback contributes nothing, so
    // the property it feeds stays initial. C++ used `border: var(--cycle1)`;
    // ported as-is (a border longhand asserts the "declaration dropped" state).
    let (doc, el) = styled(
        "--valid: blue; --cycle1: var(--cycle2); --cycle2: var(--cycle1); \
         color: var(--valid); border: var(--cycle1)",
    );
    assert_eq!(doc.color(el), rgb(0, 0, 255));
    assert_eq!(doc.value(el, "border-top-style"), "none");
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.MixedCycleAndValidVariables
#[test]
fn mixed_cycle_and_valid_variables() {
    // A cyclic var queried *with* a fallback DOES use the fallback (a cyclic ref
    // is guaranteed-invalid, and var(--cyclic, fallback) falls to the fallback).
    // C++ used `border: var(--cyclic, fallback)`, but "fallback" is not a valid
    // `border` value, so a real `border` would go IACVT and mask the very
    // behavior under test. Carry the fallback in a custom property instead — it
    // accepts an arbitrary token stream — to observe that the fallback was
    // honored (trimmed to "fallback", not Lynx's "  fallback").
    let (doc, el) = styled(
        "--a: var(--cyclic); --cyclic: var(--a); --valid: green; \
         color: var(--valid); --carry: var(--cyclic, fallback)",
    );
    assert_eq!(doc.color(el), rgb(0, 128, 0));
    assert_eq!(doc.value(el, "--carry"), "fallback");
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.MixedCycleAndValidVariables2
#[test]
fn mixed_cycle_and_valid_variables2() {
    // --cyclic ↔ --cyclic-b cycle (both guaranteed-invalid). --cyclic-a is
    // outside the cycle: it references the invalid --cyclic-b, so it uses its
    // own fallback "yellow"; then var(--cyclic-a, blue) → yellow.
    let (doc, el) = styled(
        "--cyclic: var(--cyclic-b, red); --cyclic-a: var(--cyclic-b, yellow); \
         --cyclic-b: var(--cyclic, pink); color: var(--cyclic-a, blue)",
    );
    assert_eq!(doc.color(el), rgb(255, 255, 0));
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.CycleWithFallbackCorrectBehavior
#[test]
fn cycle_with_fallback_correct_behavior() {
    // --a → --b → --c → --a, each with a *definition* fallback. Querying any
    // member with no query-level fallback → IACVT (definition fallbacks do not
    // rescue a cycle). A separate non-cyclic undefined var WITH a fallback uses
    // it (trimmed).
    let (doc, el) = styled_under_colored_root(
        "--a: var(--b, fallback-b); --b: var(--c, fallback-c); --c: var(--a, fallback-a); \
         --valid: blue; color: var(--a); --out: var(--nonexistent, fallback-value)",
    );
    // Cyclic member is guaranteed-invalid: reading it is empty, and the color
    // it feeds is IACVT → inherited.
    assert_eq!(doc.value(el, "--a"), "");
    assert_eq!(doc.color(el), rgb(1, 2, 3));
    // Non-cyclic undefined var with a fallback: trimmed fallback.
    assert_eq!(doc.value(el, "--out"), "fallback-value");
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.SubstituteAll
#[test]
fn substitute_all() {
    // --a/--b/--c form a non-cyclic chain resolving to blue. --d begins with a
    // self-reference `var(--d, …)`: W3C makes --d guaranteed-invalid regardless
    // of its fallback (THE key divergence — Lynx instead walks the fallback to
    // "blue"). Reading a guaranteed-invalid custom property yields empty.
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

// --- Resolved (fiber second-phase) path: same observable outcome ----------

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.SubstitutionResolvedSimple
#[test]
fn substitution_resolved_simple() {
    // Lynx's non-recursive "resolved" phase is observationally identical to
    // single-phase W3C resolution when the map is already plain values.
    let (doc, el) =
        styled("--color: red; --size: 16px; color: var(--color); font-size: var(--size)");
    assert_eq!(doc.color(el), rgb(255, 0, 0));
    assert_eq!(doc.value(el, "font-size"), "16px");
}

// C++: css_value_unittest.cc::CSSValueSubstitutionTest.SubstitutionResolvedFallback
#[test]
fn substitution_resolved_fallback() {
    // Same fallback-whitespace correction as `variable_with_fallback`, via the
    // resolved path.
    let (doc, el) = styled("color: var(--primary, blue)");
    assert_eq!(doc.color(el), rgb(0, 0, 255));
}

// Skipped (disposition):
// css_value_unittest.cc::CSSValueTest.DefaultConstruct/ConstructFromEnum/ConstructFromNumber —
// skip-internal: Lynx's CSSValue tagged-union + CSSValuePattern is native plumbing; stylo models
// values differently. css_value_unittest.cc::CSSValueSubstitutionTest.DepthLimit —
// skip-out-of-scope: Lynx's tunable max_depth (3/10) recursion guard is an implementation detail,
// not spec behavior; normal-depth resolution is covered by deep_nested_variable_references.
// css_value_unittest.cc::CSSValueToVarReferenceTest.SimpleVariableToVarReference/
// MultipleVariablesToVarReference/VariableWithFallbackMapToVarReference/
// NoVariableConversionForNonVariable/NoDoubleConversion/EmptyVariableName/ComplexVariableWithCalc/
// VariableWithSimpleFallback — skip-out-of-scope: legacy `{{--name}}` mustache syntax +
// ToVarReference() conversion are native-only (.lynx.bundle); the deviations map says "do not
// implement {{}} at all", and the var()-syntax semantics are already covered above.
// css_value_unittest.cc::CSSValueSubstitutionTest.SubstitutionResolvedNoRecursive — skip-internal:
// asserts an invariant of Lynx's two-phase pre-substituted map; a single-phase engine has no
// analogue. css_variable_handler_unittest.cc::CSSVariableHandlerTest.FormatStringWithRule0..Rule6 —
// skip-out-of-scope: legacy `{{--name}}` mustache substitution inside calc(); native-only, and
// var()-in-calc semantics are covered by the var() cases here. css_variable_handler_unittest.
// cc::CSSVariableHandlerTest.FormatStringWithRule7 — skip-out-of-scope: Lynx's coarse
// whole-declaration `default_props` fallback has no W3C analogue (spec requires per-var() fallback;
// undefined+no-fallback → IACVT). css_variable_handler_unittest.cc::CSSVariableHandlerTest.
// FormatStringWithRule8 — skip-out-of-scope: Lynx's build-time `default_value_map` fallback source
// is native-only; the "set var wins over its fallback" semantic is standard per-var() fallback.
// css_variable_handler_unittest.cc::CSSVariableHandlerTest.HasCSSVariableInAnyStyleMap —
// skip-internal: fast-path detection over Lynx StyleMap/CSSValue containers; stylo tracks
// var-dependence internally.
