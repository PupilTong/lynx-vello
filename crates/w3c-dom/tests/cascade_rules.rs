//! Ports of `core/renderer/css/ng/style/rule_set_unittest.cc` (`LynxJS` C++
//! engine) for the `w3c-dom` crate.
//!
//! Scope: `enableCSSSelector = true` (NG selector path) and
//! `enableRemoveCSSScope = true` (global styles) only.
//!
//! Expectation policy (see `docs/style-assumptions.md` and
//! `docs/tracking/deviations.md`): port each inventory case's `ours_expected`
//! per assertion — W3C-correct behavior for real spec features even where the
//! C++ deviates, Lynx-faithful behavior for Lynx-only extensions. Never weaken
//! an expectation to make a test pass.
//!
//! This file ports **zero** runnable tests: every case in
//! `rule_set_unittest.cc` exercises a C++ engine *internal* with no observable
//! CSS surface, so there is no computed value or selector-match result a
//! `w3c-dom` integration test could assert. Each is accounted for in the
//! skip footer below. The equivalent stylo internals (`SelectorMap` bucketing,
//! `CascadeData` sibling-rule bookkeeping, `@supports` parse-time evaluation)
//! are validated indirectly by the matching/cascade behavior tests in the
//! sibling files, exactly as the C++ suite's own rationale prescribes.
//!
//! Load-bearing caveat carried forward from
//! `RuleSetTest.FindBestRuleSetAndAdd_*`: Lynx's NG matcher *buckets* `[attr]`
//! selectors but never *matches* them at runtime, whereas stylo matches
//! attribute selectors per spec. Any integration test derived from these rule
//! strings must therefore expect attribute selectors to actually match — that
//! divergence is asserted where it is observable, in the selector-matching
//! tests, not here.

// No `mod common;`: with no runnable cases this file uses none of the shared
// harness. The skip footer is the entire deliverable.
//
// Each skipped C++ case below is its own blank-line-separated block so the
// formatter (`wrap_comments = true`) reflows them independently and each stays
// greppable via its leading `// Skipped (disposition):` marker.

// Skipped (disposition): RuleSetTest.FindBestRuleSetAndAdd_* (10 tests, incl.
// PlaceholderPseudo) — skip-internal: asserts which lookup bucket
// (id/class/attr/tag/pseudo) a parsed rule lands in; stylo's
// SelectorMap::find_bucket owns bucketing (same ID>Class>Attribute>LocalName
// priority), an internal index optimization rather than observable CSS —
// correctness is validated by matching results, not bucket membership.

// Skipped (disposition): RuleSetTest.HasAdjacentSiblingRules_* (7 tests) —
// skip-internal: asserts a `+`-combinator-presence flag used to schedule
// sibling invalidation (Blink RuleFeatureSet-derived); stylo carries the
// equivalent sibling-rule bookkeeping on CascadeData, not a user-observable
// CSS behavior.

// Skipped (disposition):
// RuleSetTest.SupportsConditionWithoutEvaluatorDoesNotPoisonCache /
// SupportsConditionResultIsCached — skip-internal: asserts ConditionRule
// memoization of a Lynx-only @supports engine-version-range predicate (a
// shipped-but-dead subsystem per deviations.md §@supports); stylo evaluates
// @supports at parse time via its own supports_condition module, with no
// per-rule result cache to reproduce.
