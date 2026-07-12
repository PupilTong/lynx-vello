//! CSS invalidation-subsystem cases ported from the `LynxJS` C++ engine.
//!
//! Ports (attempted) from:
//! - `core/renderer/css/ng/invalidation/invalidation_set_test.cc`
//! - `core/renderer/css/ng/invalidation/rule_invalidation_set_test.cc`
//!
//! Scope: `enableCSSSelector = true` (NG selector path) and
//! `enableRemoveCSSScope = true` (global styles) only — see
//! `crates/stylo-dom/tests/common/mod.rs` and `docs/style-assumptions.md`.
//!
//! Expectation policy: port each inventory case's `ours_expected` — W3C-correct
//! for real spec features, Lynx-faithful for Lynx-only extensions; never weaken
//! an expectation to make a test pass, `#[ignore = "engine-gap: …"]` a correct
//! expectation the engine can't yet meet (see `docs/tracking/deviations.md`).
//!
//! ## Why this file contains no active tests
//!
//! Every case in both C++ files exercises the *internal* data structures of the
//! invalidation subsystem, not observable CSS behavior. `invalidation_set_test.cc`
//! covers the `InvalidationSet::Backing<T>` single-slot-to-hashset growable
//! container, the `DescendantInvalidationSet::InvalidatesElement` class-membership
//! predicate, the whole-subtree-invalid flag that collapses per-feature backings,
//! and the shared self-invalidation singleton. `rule_invalidation_set_test.cc`
//! covers the selector → invalidation-map builder: which feature a selector
//! contributes to the self / descendant / sibling buckets, universal-subject
//! whole-subtree collapse, singleton reuse and copy-on-write promotion, `:not()`
//! argument handling, and cross-sheet `Merge`.
//!
//! stylo owns the equivalent machinery in `invalidation::element::invalidation_map`
//! and builds it automatically from parsed selectors. None of it is reachable
//! through this crate's public surface or the `Doc` harness (computed values,
//! selector matching, specificity, media evaluation), so there is nothing to
//! assert: every inventory case carries a `skip-internal` disposition and is
//! recorded in the footer block below rather than ported. The behavioral *intent*
//! of several cases (e.g. for `.a div`, toggling `.a` dirties descendant `<div>`)
//! is already exercised end-to-end by stylo's restyle traversal, on which the
//! value-based ports in the sibling suites (and
//! `harness_smoke.rs::mutation_helpers_restyle_incrementally`) rely.

mod common;

// Skipped (disposition): every case in both source files is an internal
// invalidation-subsystem unit test with no observable CSS surface (see the module
// doc comment above). stylo builds and exercises the equivalent invalidation
// machinery itself, so none are ported. The 17 skip-internal cases follow, one
// per line (`#[rustfmt::skip]` keeps them unwrapped).
#[rustfmt::skip]
const _SKIPPED_DISPOSITIONS: () = {
    // --- invalidation_set_test.cc ---
    // Skipped (disposition): InvalidationSetTest.Backing_* (10 tests) — internal Backing<T> single-slot/hashset container mechanics (create/add/dedup/independence/clear/empty/iterate/accessors); stylo owns its own invalidation-set storage.
    // Skipped (disposition): InvalidationSetTest.ClassInvalidatesElement — internal InvalidationSet membership predicate (class-list intersection); stylo's restyle path performs the equivalent match.
    // Skipped (disposition): InvalidationSetTest.SubtreeInvalid_AddBefore / SubtreeInvalid_AddAfter / SubtreeInvalid_Combine_1 — whole-subtree flag empties and blocks per-feature backings; stylo uses RESTYLE_DESCENDANTS with no exposed collapse-on-flag mechanic.
    // Skipped (disposition): InvalidationSetTest.SelfInvalidationSet_Combine — shared self-invalidation singleton identity/idempotent-combine and InvalidatesSelf propagation; stylo has an equivalent internal fast path.
    // Skipped (disposition): InvalidationSetTest.AttributeInvalidatesElement / SubtreeInvalid_Combine_2 — commented out (not compiled) in the C++ source; no active expectation to port.
    // --- rule_invalidation_set_test.cc ---
    // Skipped (disposition): RuleInvalidationSetTest.interleavedDescendantSibling1 — subject class `.p` yields a self-invalidation entry; internal selector→invalidation-map builder.
    // Skipped (disposition): RuleInvalidationSetTest.id — `#a #b`, id `a` change invalidates descendant id `b`; internal map builder.
    // Skipped (disposition): RuleInvalidationSetTest.pseudoClass — subject `:focus` yields self-invalidation; stylo tracks dynamic-state dependence internally.
    // Skipped (disposition): RuleInvalidationSetTest.tagName — `:focus e`, focus change invalidates descendant tag `e`; internal map builder.
    // Skipped (disposition): RuleInvalidationSetTest.Whole — `.a *`, universal subject forces whole-subtree descendant invalidation; internal canonicalization.
    // Skipped (disposition): RuleInvalidationSetTest.SelfInvalidationSet — self-only class/id/pseudo features reuse the shared self-invalidation singleton; internal memory optimization.
    // Skipped (disposition): RuleInvalidationSetTest.ReplaceSelfInvalidationSet — copy-on-write promotion off the singleton once a descendant feature (`.a div`) appears; internal storage mechanic.
    // Skipped (disposition): RuleInvalidationSetTest.Not — features inside `:not()` bucketing for `.b:not(.a) .c`; internal builder (stylo tracks `:not()` arguments per spec regardless).
    // Skipped (disposition): RuleInvalidationSetTest.EnsureMutableInvalidationSet2 — merging `.a div` + bare `div` yields descendant tag `div` for class `a`; internal mutable-set growth.
    // Skipped (disposition): RuleInvalidationSetTest.EnsureMutableInvalidationSet3 — sibling `~ *` stays out of the descendant whole-subtree bucket for `.a ~ *` + `.a div`; internal descendant/sibling bucketing.
    // Skipped (disposition): RuleInvalidationSetTest.Merge — RuleInvalidationSet::Merge cross-sheet aggregation then clear; stylo merges per-sheet invalidation maps during Stylist rebuild.
    // Skipped (disposition): RuleInvalidationSetTest.IgnoreSibling — sibling-positioned features (`.a ~ div`, `.a ~ div .b`, `.a ~ div *`) produce no descendant invalidation; internal bucketing.
    // Total: 17 inventory cases, all skip-internal; 0 ported.
};
