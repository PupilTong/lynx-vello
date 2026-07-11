# Styling-system design assumptions

> User-confirmed in an interactive Q&A session, 2026-07-11. This doc records
> the **decisions** that bound the high-performance styling-system
> implementation; it decides *what and why*, not *how*. Architecture/ownership
> rules live in [style-architecture.md](style-architecture.md); the behavior
> inventory lives in [docs/tracking/](tracking/README.md); the standards
> policy (W3C-correct vs Lynx-faithful classification) lives in
> [AGENTS.md](../AGENTS.md).

The one-line framing: **the styling system is a subset of the W3C standard —
the subset is defined by what the `.web.bundle` wire format can carry, and
the semantics are stylo's.** Everything below refines that sentence.

## Settled before this session (not re-decided)

- stylo is the cascade engine, layered `lynx-widget → stylo-dom → vendor/stylo`
  (fork with the `lynx` feature; Lynx-only properties and `rpx`/`ppx`/`sp`
  units are first-class grammar in the fork, no side-channel tricks).
- Compat target is **web-core / `.web.bundle`** behavior, not native
  `.lynx.bundle`.
- W3C-correct semantics for real spec features; faithful cloning for
  Lynx-only extensions ([AGENTS.md](../AGENTS.md) standards policy).
- Custom properties (`--x`/`var()`) ride stylo's native spec-compliant
  support; deprecated Lynx properties are dropped, not implemented
  ([deviations.md](tracking/deviations.md)).

## A. Scope — what "the subset" means

1. **Wire-format-driven surface.** The supported CSS surface is whatever a
   `.web.bundle` can encode: `RuleType::{Declaration, FontFace, KeyFrames}`
   rules, inline styles, and the web-core property table — modulo the
   existing deprecated-property exclusion. No hand-curated property
   allowlist is maintained. Web-core parity by construction: on the web
   target the effective surface is "encoder output × full browser CSS", and
   stylo *is* a browser engine.

2. **The subset is a scope boundary, not a runtime gate.** Full stylo runs;
   standard CSS that "sneaks through" the wire format (e.g. `writing-mode`
   as a raw declaration string) behaves however stylo behaves. No
   validation/strip layer, no pruning of the stylo fork's property set. This
   matches web-core, where the browser also accepted everything.

3. **Selector matching = full stylo matching.** Everything stylo parses,
   matches per spec — including `:is()`, `:where()`, `:has()`, the
   `:nth-*` family, and attribute selectors that native Lynx parses but
   never matches. We deliberately do **not** replicate the native NG
   matcher's gaps ([css-selectors-cascade.md](tracking/css-selectors-cascade.md));
   the web target matched everything via the browser, and `:where()` is
   load-bearing for scoping (§D.16).

4. **`::before`/`::after` + `content`: omitted in v1.** Native-Lynx
   fidelity wins over web-target parity here — an intentional, recorded
   divergence from web-core (where browser passthrough renders them).
   Selectors parse; no pseudo-element boxes are generated and `content` is
   inert. Revisit when real fixtures/apps demand it.

## B. Performance architecture

5. **Ingestion = direct construction.** Decoded rkyv StyleInfo is lowered
   straight into stylo `Stylesheet`/`PropertyDeclarationBlock` structures:
   one selector-list parse per rule, per-property value parses from the
   decoded `(property-id, value-string)` pairs. No re-serialization to a
   CSS text blob and no full-sheet re-tokenization (web-core's approach) —
   this is the startup-latency-critical path.

6. **Parallel styling from day 1.** stylo's rayon work-stealing traversal
   (Firefox-style) is enabled from the start, not retrofitted. Sequential
   fallback thresholds for small trees are a tuning detail, not a design
   phase.

7. **Incremental restyle via stylo invalidation sets from day 1.** Class /
   attribute / state flips restyle only the elements whose rules could be
   affected. No coarse mark-subtree-dirty MVP phase; the invalidation
   machinery is the headline performance feature, and native Lynx has the
   same concept (`RuleInvalidationSet`).

8. **Style→layout handoff: direct `Arc<ComputedValues>` reads + change
   flags.** neutron-star / render consume stylo's computed values in place;
   dirty/invalidation flags (stylo change hints) decide what re-runs.
   Flattening into per-node PODs is *not* assumed — it happens only if
   profiling later proves the pointer-chasing costs.

## C. Runtime semantics

9. **Inheritance: full W3C, always on.** Reconfirms the earlier decision in
   [deviations.md](tracking/deviations.md): stylo's standard inheritance
   plus web-elements-parity UA resets (`x-text { color: initial }`, …).
   Native Lynx's `enableCSSInheritance` gate is never emulated.

10. **`@media`: wire stylo's evaluation now, ahead of the wire format.**
    The `.web.bundle` format cannot express `@media` today (`RuleType` has
    no condition-rule variant), so no bundle can exercise it yet — but the
    engine supports it from the start via stylo, with the C++ engine's NG
    evaluator model as the behavioral reference. Extending the wire format
    is separate future work. `Device`/media machinery also serves viewport
    units and the `rpx` basis regardless.

11. **Animations: hybrid from day 1.** `transform`/`opacity`/`filter`
    animate render-side without touching the cascade (compositor-style);
    layout-affecting properties animate style-side through stylo's
    Animations/Transitions cascade levels (per-frame computed-value
    interpolation, correct cascade interactions). Two clocks by design.

12. **Animation staleness seam: query-time sync.** The render side owns the
    in-flight value for render-driven properties; when something needs the
    current value (style queries, a transition starting *from* an animating
    value, cascade/invalidation logic), it is sampled back and overlaid on
    computed style on demand. One source of truth per property; the sync
    surface is small and explicit.

13. **Dynamic pseudo-classes deferred past v1.** `:hover`/`:active`/`:focus`
    simply don't match until the event system lands. The reserved
    architecture is event-pushed element state: the event/gesture layer sets
    stylo `ElementState` bits and invalidation does targeted restyles (both
    Lynx's and Firefox's model). Touch→hover mapping policy belongs to the
    event layer, not the style engine.

## D. Integration & configuration

14. **pageConfig becomes generated UA styles, not engine logic.** Flags like
    `defaultDisplayLinear` / `defaultOverflowVisible` are honored the way
    web-core honors them: they parameterize generated UA-sheet content at
    the widget/adapter level. The styling core contains no pageConfig
    branches.

15. **Built-in component defaults = a UA-origin stylesheet.** One
    user-agent-origin sheet in the stylist (mirroring web-elements' host
    styles), parameterized per §14. Correct cascade-origin semantics for
    free — author styles override naturally. No hardcoded per-widget style
    seeding.

16. **cssId scoping is a widget-layer concern.** The feature exists for
    `removeCSSScope = false`; `stylo-dom` stays scope-unaware. Mechanism:
    the widget layer synthesizes `:where([l-css-id="N"])` guards onto
    selectors at ingest — string-parity with web-core's decoder output,
    trivially differential-testable, zero specificity perturbation. (With
    `removeCSSScope = true`, guard synthesis is skipped and styles are
    global.) Partitioned per-cssId rule sets remain a possible *measured*
    optimization, not the design.

17. **Legacy `css_og` (`enableCSSSelector=false`) bundles: out of scope for
    v1.** Reconfirms the earlier deferral. The decoder may still parse the
    class→declarations side table; the styling system ignores it. Legacy
    ReactLynx2-era bundles are explicitly unsupported until demand appears —
    and if ever supported, note their semantics are *not* plain CSS
    specificity (per-class application order), so "convert to `.class`
    rules" would be an approximation.

## E. The performance bar

18. **Match or beat native C++ Lynx.** The bar for "high performance" is the
    C++ engine's style resolver on equivalent scenarios — matching it is
    good enough, beating it is the goal. Harness details (CodSpeed/divan
    scenario benches, fixture selection, comparable instrumentation of the
    C++ engine) are implementation follow-ups, but the yardstick itself is
    fixed.

## Deliberately still open (known non-decisions)

- The v1 media-feature set `Device` exposes (viewport geometry, orientation,
  `prefers-color-scheme`, resolution, …) and any wire-format extension
  design for `@media`.
- Parallel-traversal tuning (small-tree sequential threshold).
- The exact API surface for query-time sync of render-driven animated values
  (§C.12) — defined together with the render/runtime layers.
- Which milestone re-enables dynamic pseudo-classes (§C.13).
- Whether to keep a CSS-text serialization path purely as a
  differential-testing oracle against web-core output.
- The revisit trigger for `::before`/`::after` (§A.4) — fixture/app demand.
