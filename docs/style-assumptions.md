# Styling-system design assumptions

> User-confirmed in interactive Q&A sessions, 2026-07-11 through 2026-07-16. This doc records
> the **decisions** that bound the high-performance styling-system
> implementation; it decides *what and why*, not *how*. Architecture/ownership
> rules live in [style-architecture.md](style-architecture.md); the behavior
> inventory lives in [docs/tracking/](tracking/README.md); the standards
> policy (W3C-correct vs Lynx-faithful classification) lives in
> [AGENTS.md](../AGENTS.md).

The one-line framing: **the styling system is a subset of the W3C standard —
the author surface is seeded by Lynx's official property index and closed over
complete shorthand/longhand families, while the semantics are Stylo's.**
Everything below refines that sentence.

## Settled before this session (not re-decided)

- `stylo-dom::Document` is the sole workspace owner of CSS computation and
  uses `vendor/stylo` as its engine (fork with the `lynx` feature; the selected
  Lynx-only properties and `rpx` unit are first-class grammar in the fork,
  with no side-channel tricks). `lynx-widget` may call the document but is not
  part of the CSS-computation layer.
- Compat target is **web-core / `.web.bundle`** behavior, not native
  `.lynx.bundle`.
- W3C-correct semantics for real spec features; faithful cloning for
  Lynx-only extensions ([AGENTS.md](../AGENTS.md) standards policy).
- Custom properties (`--x`/`var()`) ride stylo's native spec-compliant
  support. Lynx `-x-*` properties, `ppx`/`sp`, and the three deferred gravity
  properties are not implemented ([deviations.md](tracking/deviations.md)).

## A. Scope — what "the subset" means

1. **The official Lynx property index is the author-surface seed.** The
   source-of-truth list is
   `vendor/stylo/style/properties/lynx_properties.txt`, synchronized from the
   official Lynx property index. From that seed, code generation computes the
   complete shorthand/longhand closure: if either a shorthand or one of its
   longhands is supported, the shorthand and *all* of its component longhands
   are authorable. Supported shorthands use their complete Stylo/W3C grammar;
   there are no Lynx-specific partial-shorthand parsers.

2. **The subset is compiled, not filtered at runtime.** With
   `feature = "lynx"`, unsupported names are absent from the generated
   property-name table and unsupported enum/type/parser arms are absent from
   the generated Rust. There is no runtime feature flag and no post-parse
   filter-out layer. Without the feature, upstream Servo behavior is retained
   and tested for parity. The deliberate exclusions are all Lynx `-x-*`
   properties, the `ppx` and `sp` units, and `linear-cross-gravity`,
   `linear-gravity`, and `linear-layout-gravity`. See
   [stylo-lynx-css-subset.md](stylo-lynx-css-subset.md) for the complete
   property/value behavior table.

   `direction` follows W3C exactly: only `ltr | rtl` exists. Lynx's internal
   `normal` state and its `lynx-rtl` compatibility value are not part of this
   engine.

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
   inert.

   *Policy reconciliation*: this does not trip [AGENTS.md](../AGENTS.md)'s
   bucket-1 rule ("implement W3C-correct behavior for spec features Lynx
   supports") because **native Lynx does not support this feature at all** —
   no `content` property exists anywhere in its property table; only the web
   target renders it, as a side effect of browser passthrough. The omission
   is a scoped exception to the *web-core compat target*, recorded as a
   decision in [deviations.md](tracking/deviations.md). **Milestone to
   revisit**: when the render engine grows generated-content box support, or
   the first real fixture/app depends on it — whichever comes first.

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

   *§6/§7 required a DOM redesign, and it shipped together with the styling
   system* (not as a retrofit onto the earlier single-threaded-flush
   `stylo-dom`): flushes now drive stylo's own `driver::traverse_dom`;
   every piece of element state stylo mutates through `&self` became
   atomic; the one non-atomic slot (`ElementData`) is single-owner under
   stylo's one-worker-per-element traversal discipline; and mutations
   record pre-mutation snapshots that feed stylo's invalidation sets. See
   [style-architecture.md](style-architecture.md) for the resulting
   lifecycle and thread-safety invariants.

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

    *Structural side effects are not render-private.* A transform/filter
    also creates a containing block for positioned descendants and a
    stacking context — and those must be visible to layout even while the
    interpolated value lives render-side. Resolution (matching browser
    behavior and `will-change` semantics): an element with a **running**
    animation or transition of `transform`/`filter`/`opacity` establishes
    its containing block / stacking context **for the entire duration**,
    even across `none` keyframes. That makes the structural bits a
    per-animation constant flipped at start/end through the normal
    style-side path — layout never needs per-frame animation state, only
    the render side interpolates.

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

14. **pageConfig becomes Document-owned UA styles, not widget engine logic.**
    A host can pass `defaultDisplayLinear` / `defaultOverflowVisible` inputs
    through the PAPI layer, but `stylo-dom::Document` is the sole CSS
    computation owner and owns the UA-origin stylesheet semantics. Forwarding
    configuration through `lynx-widget` does not make it a style adapter and
    must not lead to CSS behavior being implemented there.

15. **Built-in component defaults = a Document-owned UA-origin stylesheet.**
    One user-agent-origin sheet in the `Document`'s stylist (mirroring
    web-elements' host styles), parameterized per §14. Correct cascade-origin
    semantics for free — author styles override naturally. No hardcoded
    per-widget style seeding and no CSS calculation in `lynx-widget`.

    *Importance constraint*: in the browser, web-elements' defaults are
    **author**-origin CSS, and several carry `!important`. Ours are UA
    origin, where an important declaration would **outrank author
    `!important`** (cascade origin order inverts for important
    declarations) — so the generated UA sheet must stay `!important`-free.
    Component behaviors web-elements enforces via `!important` (e.g.
    `scroll-view { display: flex !important }`) are instead owned by the
    native layout engine's element policy, not fought out in the cascade.

    Stylo's initial values remain the upstream W3C values. Lynx defaults such
    as the inherited 14px root font, `justify-items: stretch`, zero gaps,
    `position: relative`, `overflow: hidden`, and the configurable default
    display are ordinary declarations in this UA sheet; the Stylo fork must
    not carry `lynx_initial` overrides.

16. **cssId scoping is a stylesheet-ingestion concern owned by
    `stylo-dom`.** The feature exists for
    pageConfig `enableRemoveCSSScope = false` (that is the exact
    `.web.bundle` key; this doc previously shortened it to "removeCSSScope").
    The host/PAPI layer passes the css id and flag as input; it does not rewrite
    selector semantics. The `Document` ingestion path synthesizes
    `:where([l-css-id="N"])` guards onto selectors — string-parity with
    web-core's decoder output, trivially differential-testable, zero
    specificity perturbation. (With `enableRemoveCSSScope = true` the compiler
    emits css id `0`, guard synthesis is skipped, and styles are global.)
    Partitioned per-cssId rule sets remain a possible *measured* optimization,
    not the design.

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
