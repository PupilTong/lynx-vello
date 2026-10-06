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

- stylo is the cascade engine, layered `future runtime adapter → dom →
  vendor/stylo` (fork with the `lynx` feature; Lynx-only properties and
  `rpx`/`ppx`/`sp` units are first-class grammar in the fork, no side-channel
  tricks — and since 2026-09-21 the fork's `lynx` length surface also admits
  the W3C `cqw`/`cqh` container units, see the containment scope note below
  for what every unit resolves against). The runtime-adapter layer is not
  currently implemented.
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

4. **Element text content (user-directed, 2026-09-12).**
   String `content` items and untyped, unnamespaced `attr()` (including string
   fallback) replace the rendered children of a `display: -lynx-text` paragraph
   or a nested text/contents scope. This is an explicitly supported text
   extension, not a claim of browser-compatible string `content` on ordinary
   elements. It supersedes the earlier text-only `::before` implementation;
   both `::before` and `::after` rendering remain deferred.

   Stylo owns parsing, matching and cascade on the element's primary style.
   The fork exposes `content`; its display grammar is unchanged and excludes
   `inline` and `block`. No global pseudo UA rule, before-style snapshot or
   separate pseudo paint origin is needed. Generated runs wear their element's
   font, color, decorations and whitespace policy and share the enclosing
   paragraph's wrapping/truncation.

   UA CSS uses `text[text] { content: attr(text); }` and
   `raw-text { content: attr(text); }`. A `text` without the attribute retains
   its child content; an attribute present with an empty value replaces it
   with empty content. Replacement suppresses all descendant rendering,
   including atomic and out-of-flow boxes, but leaves DOM children, selectors
   and `textContent` unchanged. `raw-text` needs no custom-element reflection
   or synthetic DOM text child. Attribute changes invalidate the owning
   paragraph even when no selector mentions the attribute.

   `normal` and `none` retain ordinary children. Unsupported lists also retain
   children as a whole: no partial supported prefix is rendered. Counters,
   quotes, images, namespaced/typed attributes and generated boxes remain
   deferred. The supported path is limited to text paragraphs and their
   nested text/contents scopes; it does not replace arbitrary flex/grid boxes.

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
   `dom`): flushes now drive stylo's own `driver::traverse_dom`;
   every piece of element state stylo mutates through `&self` became
   atomic; the one non-atomic slot (`ElementData`) is single-owner under
   stylo's one-worker-per-element traversal discipline; and mutations
   record pre-mutation snapshots that feed stylo's invalidation sets. See
   [style-architecture.md](style-architecture.md) for the resulting
   lifecycle and thread-safety invariants.

8. **Style→layout handoff: direct `Arc<ComputedValues>` reads + change
   flags.** hughie / render consume stylo's computed values in place;
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

11. **Animations: through the cascade, driven off the main thread.**
    Revised 2026-08-20 to follow browsers, superseding the earlier ruling
    that `transform`/`opacity`/`filter` should animate render-side *without
    touching the cascade*. No engine does that. An animated value is a
    cascade value: CSS Animations 1 §2 says an animation adds a specified
    value to the cascade at the animations level, and CSS Cascade 5 §6.1
    puts transition declarations above important author rules and animation
    declarations between important author and normal author. Blink samples
    effects into an `EffectStack` that the next style resolve folds into the
    element's `ComputedStyle`; WebKit resolves an animated `RenderStyle`;
    Gecko cascades at dedicated Animations/Transitions origins — which *are*
    stylo's `CascadeOrigin::Animations` and `CascadeOrigin::Transitions`
    (`vendor/stylo/style/rule_tree/level.rs`), so this engine inherits
    Firefox's mechanism rather than approximating it. A value outside the
    cascade would also be unobservable to `getComputedStyle` and unable to
    lose to `!important`, both of which the specs require.

    What browsers actually move off the main thread for
    `transform`/`opacity`/`filter` is the per-frame interpolation and
    rasterization, and what they throttle is the per-frame *restyle* — never
    the cascade. So: every animated property cascades, and the per-frame work
    runs on the document's owner thread (the Lynx main thread, on the
    painter's frame post with no JavaScript involved), as a stylo animation-only
    traversal over just the animating elements, with no selector matching and
    no layout for properties that cannot move a box. **Throttling follows
    composite export.** A browser can skip the per-frame restyle because a
    compositor is interpolating instead. Here an element whose
    `opacity`/`transform` animation set the commit exports as a composite
    curve is the same: the painter samples a clone of the set with stylo's own
    sampling code, as Firefox runs stylo on its compositor — the values the
    next cascade produces whenever both sides iterate from the same animation
    state — and composes the retained frame, and a window painter does not
    tick the main thread for it until the curve ends (an offscreen `tick`
    ticks it every call). Everything else — a property the export does not
    carry, an `!important` override, a start the driver has not anchored, a
    transition — still restyles through the animation-only traversal and
    rebuilds the retained scene every frame; what that frame saves is the
    cascade over the elements that are *not* animating, and the layout pass
    (`docs/tracking/css-animation.md` records what exports).

    *Structural side effects are per-animation constants.* A transform/filter
    also creates a containing block for positioned descendants and a stacking
    context, and those must be visible to layout for the whole animation
    rather than flickering with the interpolated value. Resolution (matching
    browser behavior and `will-change` semantics): an element with a
    **running** animation or transition of `transform`/`filter`/`opacity`
    establishes its containing block / stacking context **for the entire
    duration**, even across `none` keyframes — flipped once at start and once
    at end, so layout never needs per-frame animation state. Implemented for
    `opacity` and `transform` as two node bits the animation driver keeps
    (`animates_opacity`, `animates_transform`): an animation counts while
    pending (its delay included), running or paused, or finished with a
    `forwards`/`both` fill, a transition while pending or running, and a
    scroll-driven animation only while its timeline is active (26). Either bit
    makes a stacking context, the opacity bit a composited group, and the
    transform bit a containing block for absolute and fixed descendants. Not
    yet implemented for `filter`: an animation from `filter: none` has no
    stacking context or containing block while the value reads `none`.

12. **The animation staleness seam is back for exported curves — a known
    gap.** For an animation that ticks on the main thread the cascade output
    *is* the animated value, so a style query, a transition starting *from* an
    animating value, and invalidation all read one truth. An exported curve
    (11) is a render-private value: the painter samples it per frame — with
    the code the cascade runs, so the two agree whenever both read the same
    instant from the same animation state — and nothing on the main thread
    advances the timeline for it, so its cascade value holds at the last tick
    or commit until something ticks or commits again. `getComputedStyle` on an
    element fading by an exported curve therefore reads the opacity of the
    last main-thread reading while the screen shows the sampled one. A curve
    on a scroll timeline (26) is stale the same way against the offset: main
    adopts the painter's scroll at the mailbox marker — `scroll_offset` and
    layout reads follow it — but leaves an element whose committed curve
    samples that scroll container to the painter, so its cascade value holds
    at the last commit's offset until the next commit's resolution re-samples
    it. (Transitions export all the same: every job syncs main's clock to the
    painter's first, so a restyle that retargets or reverses one reads the
    instant the painter showed.) Browsers say what closing it takes: keep the
    cascade authoritative and re-sample on demand rather than let the two
    diverge. Not built.

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
    the future runtime-adapter level. The styling core contains no pageConfig
    branches.

15. **Built-in component defaults = a UA-origin stylesheet.** One
    user-agent-origin sheet in the stylist (mirroring web-elements' host
    styles), parameterized per §14. Correct cascade-origin semantics for
    free — author styles override naturally. No hardcoded per-element style
    seeding.

    *Importance constraint*: in the browser, web-elements' defaults are
    **author**-origin CSS, and several carry `!important`. Ours are UA
    origin, where an important declaration would **outrank author
    `!important`** (cascade origin order inverts for important
    declarations) — and inline style with it — so the generated UA sheet
    stays `!important`-free. Component behaviors web-elements enforces via
    `!important` (e.g. `scroll-view { display: flex !important }`) are
    instead owned by the native layout engine's element policy, not fought
    out in the cascade.

    *The exceptions* are all the paragraph's. The test distinguishing them
    from the `scroll-view` case above is **whether the fact is a default or a
    structural invariant**. A scroller's display mode is a default — web-core
    itself gates it on page config, and the question is only *which* layout
    mode the box gets. So the exception is spent on facts the cascade is
    merely *reporting*, never on defaults it is *choosing* — and the set is
    pinned by `the_ua_sheet_is_important_free_apart_from_the_text_block`,
    which fails on any new important declaration until it earns the same
    argument.

    - `display: -lynx-text` on `text`, on `inline-text`, and on a `text`'s own
      `inline-truncation` child (user decision, 2026-09-03). A text's
      inline-ness is not a mode at all: Lynx establishes it at tree-mutation
      time (`TextElement::OnNodeAdded` calls `ConvertToInlineElement`, which
      renames the tag and marks the node inline), the paragraph builder never
      reads `display`, and no author CSS can undo any of it. A cascade value a
      page could override would therefore describe an engine we do not have.
    - `padding: 0` on an `image` that is inline content of a paragraph
      (2026-09-17). The same argument reached from the other end: in web-core
      the authored host element generates no box at all —
      `x-text > x-image { display: contents !important }`, with the
      `inline-text`, `inline-truncation` and `lynx-wrapper` variants of it
      (`x-text.css:69-82`) — and the box on the line is a shadow
      `::part(img)` assembled from an inherited property list `padding` is not
      on (`:120-135`). No author CSS there can make an inline image's padding
      matter, and web-core's own erasure is itself `!important`. Here the
      authored element *is* the box, so a normal UA declaration would lose to
      the author's `padding` and the engine would advance the line by an edge
      web-core does not have (native is split; see `docs/tracking/deviations.md`). `margin` is deliberately untouched — the shadow
      part inherits it — and so is a `view` inside a `text`, which web-core
      leaves a real `inline-flex` box with its padding intact.
    - The `viewpager` row (2026-09-28): `flex-direction: row`,
      `linear-direction: row` and `flex-wrap: nowrap` on `viewpager` /
      `x-viewpager-ng`, and `position: relative` on `viewpager-item` /
      `x-viewpager-item-ng`. In both references a pager's pages form one row
      whatever the author writes on the pager: web-core lays them out in a
      shadow `#content` box whose `flex-direction: row` no author rule on the
      host reaches (`htmlTemplates.ts:379-405`), and native places the pages
      itself. Authors do write a main axis there — the web-core e2e cards style
      the pager `display: flex; flex-direction: column` or `display: linear` —
      and still get a row. Here the authored pager is the box that lays the
      pages out, so a normal declaration would let those cards stack their
      pages vertically, an engine neither reference has. The three are every
      property this grammar lets an author move a flex or linear main axis or
      wrap its line with (`linear-orientation` does not parse). The page's
      `position` is web-core's own `!important` (`x-viewpager-ng.css:63`): a
      page positioned absolutely would leave the row. The pager's `display`
      stays a default, as a scroller's does.
    - The `x-swiper-item` main-axis size (2026-10-06), six rules: `width:
      100%`, `height: 100%` under `vertical`, and the `carousel` (80%) and
      `flat-coverflow`/`coverflow` (60%) sizes on each axis. web-core pins
      every one of them with its own `!important`
      (`x-swiper.css:102-118,181-191,215-226,275-289`), and the swiper's
      snap positions are built from them, so a normal declaration would let
      an author rule resize an item off its page. The swiper's main axis
      needs no pin: it lives on the shadow `#content` box, which no author
      rule reaches, exactly as in web-core (`htmlTemplates.ts:225-275`). An
      item's `position` is not pinned (web-core does not pin it).
      `crates/bobcat-core/src/main/tree/swiper.rs` carries the argument.
    - The `<x-refresh-view>` header, footer and content (2026-10-06), three
      rules: `position: relative` on the first `x-refresh-header` (unless
      `enable-refresh="false"`) and on the first `x-refresh-footer` (unless
      `enable-loadmore="false"`), and `overscroll-behavior-y: auto` on every
      other child of the view. The two positions are web-core's own
      `!important` (`x-refresh-view.css:31,50`), and web-core's demo card
      `basic-element-x-refresh-view-demo` writes `position: absolute` on its
      header: the header and the footer are the column's ends, and the snap
      positions that hold them are built from where they sit, so a header an
      author rule takes out of the flow could never be pulled into view. The
      third is this engine's: the pull is the scroll chain from the content
      child into the shadow `#container`, and a content child whose y
      boundary contains its chain — an author's `overscroll-behavior`, or the
      `contain` a `scroll-coordinator` declares on itself — would stop every
      pull before it. A `scroll-view`'s `bounces` attribute reaches no style,
      so the rule is not there to beat a UA default. The view's other
      defaults — the host's flex column, box model, the header's and the
      footer's display (the container tags' `defaultDisplayLinear` policy)
      and `flex-shrink: 0`, the footer's `margin-top: auto`, both ends'
      `scroll-snap-align` and its view-timeline animation — stay normal
      declarations. `crates/bobcat-core/src/main/tree/refresh_view.rs`
      carries the argument.
    - The `<scroll-coordinator>` structure (2026-09-29), six rules under
      both spellings (`scroll-coordinator*` and web-core's
      `x-foldview-*-ng`): the coordinator's `overflow-y: scroll` and its
      `flex-direction: column` / `linear-direction: column`, the header's and
      the slot's `position: absolute`, the toolbar's `position: sticky`, and
      `overflow-y: hidden` under `enable-scroll="false"` /
      `scroll-enable="false"`. In every reference these boxes are placed by
      something no author rule reaches: web-core pins the scroll axis with its
      own `!important` (`x-foldview-ng.css:8`) and places the slot from a
      `ResizeObserver` (`XFoldviewNg.ts:27-38`), native lays all of them out
      in CoordinatorLayout / `UIScrollView` code. Here the authored boxes
      are the layout — the slot's `anchor-size()` resolves only in an
      absolutely positioned box (§28), the header must be one so it adds
      nothing to the in-flow height, the toolbar must be sticky to stay over
      the fold, and in a row the toolbar would stop spanning the coordinator
      — so a normal declaration would let an author rule take the
      coordinator apart. The `enable-scroll="false"` line is important only
      because it must beat the pinned `scroll`. The coordinator's `display`,
      sizes, `contain`, `overscroll-behavior` and the slot's `top`/`height`
      stay defaults; `crates/bobcat-core/src/main/tree/scroll_coordinator.rs`
      carries the full argument.

16. **cssId scoping is a runtime-adapter concern.** The feature exists for
    pageConfig `enableRemoveCSSScope = false` (that is the exact
    `.web.bundle` key; this doc previously shortened it to
    "removeCSSScope"); `dom` stays scope-unaware. Mechanism: the
    runtime adapter synthesizes `:where([l-css-id="N"])` guards onto
    selectors at ingest — string-parity with web-core's decoder output,
    trivially differential-testable, zero specificity perturbation. (With
    `enableRemoveCSSScope = true` the compiler emits css id `0`, guard
    synthesis is skipped, and styles are global.) Partitioned per-cssId
    rule sets remain a possible *measured* optimization, not the design.

17. **Legacy `css_og` (`enableCSSSelector=false`) bundles: out of scope for
    v1.** Reconfirms the earlier deferral. The decoder may still parse the
    class→declarations side table; the styling system ignores it. Legacy
    ReactLynx2-era bundles are explicitly unsupported until demand appears —
    and if ever supported, note their semantics are *not* plain CSS
    specificity (per-class application order), so "convert to `.class`
    rules" would be an approximation.

## D-bis. `StyleInfo` ingestion, as landed

*(Recorded when ingestion landed, after the 2026-07-11 session. Refines §B.5
and §D.16 with what the wire format actually permits.)*

20. **The parse-skipping floor is one selector-list parse per rule and one
    value parse per declaration.** Everything above that is skipped: no
    stylesheet text is produced, no sheet is tokenized, no at-rule or
    declaration-block parsing runs, and rules, keyframes and font-face rules
    are built as stylo structures directly
    (`StylesheetContents::from_rules`, already seeded in the fork). The floor
    is not a shortcut — it is forced twice over. The wire format does not
    decompose attribute selectors (`Attribute` values carry `[type=submit]`
    whole) or functional pseudo-classes (`nth-child(4n+1)`,
    `not(:has(figcaption))`), so a component-level selector path would still
    have to parse those as text; and stylo constructs specified values only
    through its value parsers, with shorthand expansion (`margin`, `border`,
    `flex`, `animation`, … are all on the wire) reachable no other way.
    Routing values through `parse_one_declaration_into` also keeps stylo's
    own property gating on. Declaration *values* are never re-serialized:
    the wire's value tokens are a lossless partition of the authored text,
    so concatenating them reproduces it byte for byte — except that a trailing
    `!important` must first be split back out at token level, because the wire's
    `is_important` flag is never set and the marker travels inside the value
    (see [web-binary-template.md](web-binary-template.md)).

21. **Fragments flatten in reverse-topological order; per-component scoping
    is not implemented.** All `css_id` fragments mount as one author sheet,
    imported fragments before importing ones, ties broken by ascending id.
    That order is what web-core's own TypeScript decoder used
    (`processStyleInfo.js` reverses its Kahn sort) and what the C++ engine's
    `ImportOtherFragment` means; shipped web-core lost it to `FnvHashMap`
    iteration order, so it is deterministic here and undefined there. For the
    common bundle — `enableRemoveCSSScope = true`, css id `0` only — the
    result is semantically identical to web-core. For an
    `enableRemoveCSSScope = false` bundle, the `:where([l-css-id="N"])` guards
    of §D.16 are absent, so component-scoped rules apply globally and two
    components styling the same class name collide; the CLI warns, naming the
    fragment ids, rather than rendering nothing or failing silently. Unlike
    web-core, an import cycle or a duplicated import edge does not drop a
    fragment.

22. **web-core's browser-shape selector rewrites are deliberately absent**, as
    is its `:not([l-e-name])` entry guard. The rewrites compensate for a
    browser DOM this engine does not have: Lynx tag names are this document's
    real element names (no `view` → `x-view` map), and the `page` element is
    the document element, so `:root` matches natively. Dropping the entry
    guard lowers every author rule by a uniform (0,1,0), which changes no
    ordering outcome. One divergence is a correctness *fix* rather than a
    simplification: web-core emits attribute selectors double-bracketed
    (`[[type=submit]]`), which no browser matches, so attribute rules match
    here where they silently do not there.

23. **`enableCSSSelector = false` is ignored rather than honored.** The flag
    is threaded from the bundle to the runtime and now, for the first time,
    reaches a layer that could act on it. Rules mount normally; the legacy
    `css_og` class→declarations side table stays out of scope per §D.17.

## E. The performance bar

18. **Match or beat native C++ Lynx.** The bar for "high performance" is the
    C++ engine's style resolver on equivalent scenarios — matching it is
    good enough, beating it is the goal. Harness details (CodSpeed/divan
    scenario benches, fixture selection, comparable instrumentation of the
    C++ engine) are implementation follow-ups, but the yardstick itself is
    fixed.

## F. Deliberate extensions beyond the subset

19. **CSS containment (`contain` / `content-visibility`): enabled as a
    user-directed extension beyond Lynx parity.** *(Recorded 2026-07-14, after
    the 2026-07-11 session.)* Native Lynx has **no** containment property —
    there is no `CssPropertyId` variant, so the `.web.bundle` wire format
    cannot carry it and no real bundle emits it. It therefore does not fit the
    wire-format subset framing of §A; it is a deliberate, user-directed W3C
    extension, arriving only via **inline styles** (`set_inline_style` or the
    CSSOM-like `set_inline_style_property`) and
    any future ingest path. In the vendored stylo fork, `contain` itself was
    already seeded in the lynx grammar (`lynx_properties.txt`); fork PR #9
    (squash-merged into the `lynx` branch) completed the css-contain-2 family
    by enabling `content-visibility` and `contain-intrinsic-size` (+ physical
    longhands) under the `lynx` feature — pref-gated for stock servo — with
    fork-side parse/compute/damage coverage. Ingestion applies no property allowlist, so the fork build is
    the only gate. On top of that grammar, this repo adds the consuming
    machinery: `dom`'s internal `effective_containment` fold and Stylo damage
    harvest, and `hughie`'s
    size/layout containment, skipped contents, and relayout-boundary
    invalidation. Motivation: `<list>` virtualization (see
    [tracking/components.md](tracking/components.md)) — off-screen / recycled
    rows are the archetypal `content-visibility` + intrinsic-size case.

    **v1 scope (css-contain-2, plus css-contain-3's `container-type`):**
    - **The `@container` rule is out of scope; `container-type` is not.** The rule stays
      gecko-only in the fork, so `container-name` cascades and nothing matches on it. But
      `container-type` is exposed, because `cqw`/`cqh` are a *standard* implementation rather
      than viewport aliases, and css-contain-3 §2.1 makes a size query container a contained
      box: `inline-size` applies layout, style and inline-size containment, `size` applies
      layout, style and size containment. `hughie::style::containment::effective_containment`
      folds it in beside `content-visibility`.
    - **Single-axis `inline-size` containment is real in layout** *(2026-09-21 user ruling,
      for standard `container-type` support; it replaces the v1 rule that layout ignored the
      keyword).* A box with `contain: inline-size` — or with `container-type: inline-size`,
      which implies it — takes its **width** as if it had no contents
      (`contain-intrinsic-width`, or zero), while its **height** still comes from them, laid
      out into that width. Size containment is per axis throughout: `contain: size`,
      `container-type: size` and a skipping box cover both physical axes exactly as before,
      and no keyword contains the block axis alone (the engine is horizontal-writing-mode
      only). It is still **never a relayout boundary** — that stays whole-box `SIZE | LAYOUT`,
      because a box whose block size answers to its contents cannot stop an internal change
      from resizing it and reflowing its ancestors. The last-remembered-size recording rule
      became per axis with it: an `inline-size`-contained box records the height its contents
      produced and leaves the width it last measured alone.
      - **The `cqw`/`cqh` units parse and resolve the standard way** *(2026-09-21)*. They are
        1% of the **nearest ancestor size query container's content box**: an ancestor with
        `container-type: size` supplies both axes, one with `container-type: inline-size`
        supplies the inline axis only (this engine is horizontal-writing-mode only, so that is
        the width), an axis no container supplies falls back to the **small viewport**, and so
        does everything on a page with no query container at all. That last case is also the
        one native Lynx has no token for; an author targeting the web can write these because
        the browser supplies them there, and the `lynx` grammar used to reject them, which
        dropped the whole declaration.

        The container's size comes from the **last committed layout**, because it is a cascade
        input only layout can produce. `dom` records every size query container's content box
        (`size − padding − border`, unrounded CSS px) at the committing layout run that
        produced it — the same moment, and the same call, that records the last remembered
        size, since both are that run's content box — into the slot-keyed last-committed-box
        table on the tree arenas (`crates/dom/src/layout/committed_box.rs`), and answers
        Stylo's `TElement::query_container_size` from it. A container that has never been laid out, or that has just stopped being one,
        answers `None` and its descendants fall back to the viewport.

        **The interleave is the primary path** *(2026-09-21)*. A `container-type: size` box is
        contained in *both* axes, so its content box is a function of its own style and its
        layout input — not of its contents — and the layout host computes it at the **entry** of
        that box's own committing run, before one child style has been read
        (`committed_box::container_estimate`, which is `compute_skipped_contents_size`: the same
        formula every algorithm takes for a contained axis). When it disagrees with what is
        published, the run records the new size and lays **no contents out**; `run_layout` then
        publishes it, runs the same targeted mark + flush described below, and relays that one
        subtree — inside the same layout run, before anything is rounded. The subtree is
        therefore laid out once, at the size it keeps, and a text reader is shaped once, at the
        font size the container gives it. Nesting settles one level per iteration of that
        settling loop, capped the same way, except that its last iteration *closes* the
        interleave rather than capping it, so every deferred subtree is always laid out.

        Two shapes are deliberately left out, because for them the estimate is not the
        algorithm's answer: **`container-type: inline-size`**, whose block axis really does
        answer to its contents (there is nothing to publish before them), and **`display:
        -lynx-text`**, whose block algorithm implements no size containment at all (a recorded
        deviation of the text block, not of this feature). Both fall to the loop below, as does
        a prediction that ever disagreed with its algorithm — which is a `debug_assert` and not
        a correctness question, because the record after the algorithm is what is published.

        **The post-layout recascade loop** is the fallback for those, and is `Document::layout`,
        which is Gecko's `UpdateContainerQueryStyles` in this engine's shape: a pass whose
        recorded sizes moved marks, under each moved container, the elements that actually
        resolved a container unit, and lays out again. The search starts at the container's flat children — a container's
        own `cqw` answers to *its* nearest container, an ancestor — and the elements it marks are
        the ones whose primary style carries `ComputedValueFlags::USES_CONTAINER_UNITS`, which
        Stylo sets on every cascade that resolved a `cqw`/`cqh`, including one that fell back to
        the viewport because the container had no size yet. **Inherited effects are Stylo's**: a
        marked `font-size: 10cqw` element whose inherited values move propagates `RECASCADE_SELF`
        to its children through the child cascade requirement, so a grandchild's `2em` follows
        without being marked here. A nested container does not stop the search, because an
        `inline-size` container supplies no block axis and a `cqh` under it still answers to the
        outer one; the extra element that marks re-cascades to the value it already had. Each
        mark is `RestyleHint::RECASCADE_SELF` — cascade again, do not match again — which is
        sound *because* it is made inside the loop — or inside the run, for the interleave:
        the very next flush reads it, in the same `layout()` call, with no animation tick
        reachable in between.
        `remove_animation_hints` deletes `RECASCADE_SELF` outright, so that spelling is only
        safe for a mark nothing can outlive. The loop is capped at `CONTAINER_PASSES = 4`; a
        container's supplied axes are contained, so its size cannot answer to its own contents
        and the loop converges in the nesting depth of containers that move together. **The cap
        iteration is the exception**: its marks are laid out by nobody here and wait for
        whatever flush comes next, which an animation tick can precede, so that iteration falls
        back to `Document::mark_subtree_recascade`'s tick-surviving `RESTYLE_SELF |
        RECASCADE_DESCENDANTS` on each moved container — the coarse whole-subtree mark, which
        also recascades the container itself. Either way the last layout stands and the document
        is one commit behind at worst.

        Three gates keep this free for everyone else. The changed list is empty unless a
        container's content box actually moved, the document only recascades at all once some
        style it cascaded carried `USES_CONTAINER_UNITS` — a page with query containers and no
        `cqw`/`cqh` has nothing to re-resolve — and a moved container with no user under it
        marks nothing and owes no pass. The interleave shares the middle gate exactly: a run
        defers nothing at all until that flag is set, so a page with query containers and no
        container units lays out precisely as it did before. A page with neither feature pays
        one enum test per committing box, one bit test per committing box, and one `is_empty`
        test per layout.

        **The `@container` at-rule is still out**: it is gecko-only in the fork, so nothing
        matches a size or style query. `container-name` parses and cascades and nothing
        consumes it. The `container` shorthand is **name-first**
        ([csswg-drafts#7180](https://github.com/w3c/csswg-drafts/issues/7180), pinned by a fork
        test): `container: foo / size` is a named size container, while `container: size` names
        the container "size" and makes it no query container at all. The name is not optional
        in the fork's grammar (`container: / size` is rejected), so `container-type` is the
        only way to write a type without a name. `cqi`/`cqb`/`cqmin`/`cqmax` stay unparsed.
        One comparison with web-core: there `cqw` is 1% of the `lynx-view` width and `cqh`
        follows the browser window unless the host sets `transform-vh`, while here both follow
        the standard container lookup and fall back to the view.
      - **The engine's length units, stated once**, none of which takes an embedder-supplied
        base: `vw`/`vh` are the viewport width/height divided by 100; `rpx` is the screen width
        divided by 750 (a fixed divisor), and the only screen this engine has is the view, so
        `N rpx == N/7.5 vw` (implemented in the fork's `rpx_to_computed_value`); `cqw`/`cqh` are
        the query container's width/height divided by 100, which with no query container means
        `vw`/`vh` as above. `px`, `em`/`rem` and `%` are the standard ones.
    - **`content-visibility: auto` relevance is implemented** *(2026-09-20; it was deferred to a
      host-pushed "always relevant" signal until then)*. `auto` still computes its always-on
      `layout | paint | style` containment, and on top of that `dom` determines *relevance to the
      user* (css-contain-2 §4.1) inside the commit that draws the frame: after the paint order is
      built and before the walk, each `auto` element **the build reached** — i.e. every one not
      under a skipped or `display: none` ancestor, whether or not it emitted a paint item — is
      relevant iff its own border box, under its world transform and clip chain, reaches the
      region that frame's culling admits. The build records the box itself rather than an item
      index, because relevance is geometric: a `visibility: hidden` `auto` element paints nothing
      yet still decides whether its (possibly `visibility: visible`) contents lay out. The bit is layout-side per-element state in a slot-keyed side table on
      `dom`'s tree arenas — never a Stylo `ElementState`, never a restyle trigger — and
      `StyleView` folds it into `CoreStyle::skips_contents`, which `effective_containment` turns
      into the `SIZE` bit a skipped box gains. An element no rendering update has reached yet is
      *undetermined* and skips, matching the spec's "determined in the next rendering update".
      - **The margin is the painter's encode window** (`ScrollSlot::encode_window`,
        `ENCODE_WINDOW_SCROLLPORTS = 1.0`), which is exactly the region the walk's culling
        admits and the compositor may scroll to without a new commit; the main thread, adopting
        the painter's posted offsets, recommits to re-center it once an offset has used half the
        headroom toward an edge (`ScrollSlot::recenter_due`). The relevance test is the
        walker's own `CullPlan::admits_border_box`, so "relevant wherever its contents could
        paint" holds by construction rather than by agreement. Anything undecidable — a singular
        transform, a non-finite bound, an item on a chain a sampled animation delta moves —
        counts as relevant.
      - **Reveal is same-commit.** A flip invalidates layout at the flipped nodes through the
        ordinary relayout machinery and the frame is rebuilt inside the same `render()`, under
        the same commit id; a later pass only determines elements this commit has not determined
        yet (the nested `auto` boxes that only now got an item). At most 4 passes, and no frame
        is ever published with a flip pending. The bounded rule has one accepted consequence: if
        a revealed element's `contain-intrinsic-size` estimate was wrong, the boxes the resulting
        reflow moved into or out of the window are re-determined by the *next* commit, not this
        one — the same one-update lag browsers have.
      - **Three of the spec's relevance conditions are N/A here**: this engine has no focus
        model, no selection, and no view transitions. The fourth is honoured: an element in the
        top layer (§29 — a `<dialog>` opened with `showModal()`; there is no fullscreen) is
        relevant whatever its geometry.
      - **`contentvisibilityautostatechange` (§4.4) is fired, Rust-side and engine-internal**
        *(2026-09-21, user ruling)*. The commit that determines relevance queues every element
        whose *skipping* changed — which is why the spec's "first observation" case needs no rule
        of its own: an undetermined box already skips, so the first determination of an on-screen
        box is a change and that of an off-screen box is not. The event is then delivered by
        `dom` itself, over `Document::event_steps`' path, to the **engine's own components** — a
        defined `dom::CustomElement`, which is what the built-in `<list>` will be — through
        `CustomElement::handle_event`. It is **never dispatched to JavaScript**: no realm is
        entered, a card's `addEventListener` for the same name never runs, and there is no Lynx
        event name, string-handler, worklet or `global-bindEvent` form of it. `bobcat-core`'s
        only part is *when*: after the commit it posts **one entry of its own** for the batch,
        which is the spec's "dispatched by posting a task at the time when the state change
        occurs". `bubbles` is `true` where the spec is silent (Chromium bubbles; WebKit and Gecko
        do not — w3c/csswg-drafts#11310 is open), `composed` and `cancelable` are false. See
        [tracking/dom-events.md](tracking/dom-events.md).
      - `content-visibility: hidden` is unchanged and fully implemented (skip contents +
        intrinsic size + strict-like containment).
    - **Animations and transitions in skipped contents are frozen** *(2026-09-20)*, for `hidden`
      and for a non-relevant `auto` box alike. css-contain-2 §4: "While an element is skipped, CSS
      transitions and animations on the element do not update: [...] Existing animations do not
      advance in their timeline. Running animations on the element do not end." and "When an
      element stops being skipped, animations and transitions are sampled and then resume
      advancing on their timelines as normal from that point." An element "is skipped" when it is
      part of some ancestor's skipped *contents* — "the flat tree descendants of the element" — so
      the box that skips is **not** itself skipped and its own animations run as normal.
      `dom`'s animation driver (`crates/dom/src/style/animation.rs`) asks
      `crate::layout::skips_contents` up the flat tree, once per element that owns animation
      state, and freezes by carrying every one of that element's start times forward by the
      interval the tick advanced over — the arithmetic that already anchors a newly created
      animation — so `now - started_at`, the progress, does not move and nothing is promoted,
      iterated, ended or re-cascaded. Frozen sets are excluded from
      `Document::has_active_animations` and from the committed frame's `animations_active` /
      `needs_main_ticks`, so a page whose only animations are frozen leaves
      `Painter::owes_frame` / `is_animating` false and the host stops ticking; the reveal — a
      style change, or a relevance flip inside `Document::render` — makes it active again in the
      same commit.
      - **Resume lands on the first tick after the reveal**, not on the reveal itself. The reveal
        is noticed between two ticks and the engine has no reading for the instant it happened;
        the interval containing it may be an arbitrarily long stretch the host never ticked at
        all, precisely because a frozen page owes no frames. Carrying that whole interval is the
        only rule that survives it, at the cost of at most one tick interval of freeze.
      - **One deviation from the spec**, and it is the first bullet of the same list: *"New
        animations are not created even if newly-applied style would start one."* Skipping
        contents does not skip **style** in this engine, so Stylo's `process_animations` creates
        the animation or transition the new style names whatever box is above it. What the driver
        can do — and does — is freeze it at its own start, so it contributes its start value while
        skipped and plays **from zero** when the subtree reveals. The observable difference from a
        browser is the `@keyframes` start value filling (with `animation-fill-mode: backwards` or
        `both`) during the skip, and an element's `getAnimations()`-equivalent state if one is ever
        exposed. Same for the spec's "it must not start any transitions": a transition is created
        and frozen rather than not created.
    - **The css-sizing-4 last remembered size is implemented** *(2026-09-20)*. `contain-intrinsic-*`
      takes `auto? [ none | <length> ]` per axis, and the `auto` keyword means: "if the element has
      a last remembered size and is currently skipping its contents, its explicit intrinsic inner
      size in the corresponding axis is the last remembered size in that axis"
      ([css-sizing-4 §5.2](https://drafts.csswg.org/css-sizing-4/#intrinsic-size-override)). The
      three parts are all `dom`'s (`crates/dom/src/layout/committed_box.rs`); `hughie` is unchanged,
      because it already reads `AutoLength(l)` as `l` and `AutoNone` as no explicit size and the
      substituted answer arrives as a plain `Length`.
      - **The recording moment is the commit's own layout run.** css-sizing-4 §5.2.1 records "at
        the time that ResizeObserver events are determined and delivered"; this engine has no
        ResizeObserver, so the host records inside `LayoutTree::compute_layout`, after an
        algorithm's *committing* run and only on a cache miss (a cache hit reproduces the size
        already recorded under that same input). It is the same box and the same numbers an
        observer would have been handed, one step earlier than a browser delivers them: a browser
        observes after the layout that produced the size, so a same-frame change that starts the
        box skipping still uses the previous frame's value, while here the commit that laid the
        box out is the one that remembers it. Both answer the spec's question — what was this box's
        inner size the last time it was rendered — with the last rendered size.
      - **What is recorded** is "the current inner dimensions of its principal box": the content
        box (`size − padding − border`) of that run's own unrounded output, in CSS px, per axis
        whose effective value carries `auto`, and only while the element does **not** have size
        containment — which excludes every skipping box, and is what lets a remembered size
        survive for as long as the box goes on skipping. An axis whose keyword is gone is cleared
        in the same write, which is the spec's "remove its last remembered size".
      - **The store is a second slot-keyed side table on `TreeArenas`**, beside the relevance
        table and for the same structural reason: a `StyleView` is built from the tree arenas
        alone. It is not in `LayoutSlot` and not in `NodeLayoutState`, it resets on free (the
        remembered size belongs to the element, so a recycled key must remember nothing), and it
        allocates nothing for a page that never uses the `auto` keyword. It is the *same* table
        that holds the css-contain-3 query container size — one entry per element, two fields,
        one `record` call per committing run — so writing it from a pass that holds the arenas
        shared goes through that table's staged `RefCell`, published once per layout pass under
        the exclusive borrow. A box never reads back a size recorded in the pass it is reading
        in: a skipping box is size-contained in both axes, so the only write its own run can
        make is the removal, and a box without the keyword never performs the lookup.
      - **`content-visibility: auto` implies the keyword**
        ([csswg-drafts#8407](https://github.com/w3c/csswg-drafts/issues/8407)): `Length(l)` behaves
        as `AutoLength(l)` and `None` as `AutoNone`. The fork carries the mapping
        (`ContainIntrinsicSize::add_auto_if_needed`) but applies it in
        `StyleAdjuster::adjust_for_contain_intrinsic_size`, which is `#[cfg(feature = "gecko")]`
        and never runs in this build, so `dom` applies the fold on the computed value on the way
        into layout rather than at cascade time. The observable difference from a cascade-time
        adjustment is confined to serialization: `getComputedStyle` still reports the authored
        value.
    - **Paint containment: layout + visual order.** `contain: paint`'s IFC / containing-block
      effects are computed and exposed by layout; since 2026-07-23 its stacking context and
      paint/hit-area clipping are implemented by `dom`'s `visual` module (paint order + hit
      testing). Actual pixel output still awaits the render crate.
    - **Style containment is N/A.** `contain: style` parses and feeds the `content` / `strict`
      composite math, but the engine has no counters or quotes; string/attribute generated content
      needs no containment boundary, so it has no semantic effect.

    The layout-side semantics (size-substitution, layout-containment baseline
    suppression + host CB contract, skipped contents, the relayout-boundary
    theorem) live in [layout-architecture.md](layout-architecture.md); the
    tracking rows are in [tracking/css-layout.md](tracking/css-layout.md).

    **The `<list>` UA rules, the first consumer of all of it** *(2026-09-21;
    the four decisions below were ruled by the user the same day)*. The
    motivating case is now written down, in
    `crates/bobcat-core/src/main/tree/list.rs`, translated from web-elements'
    `x-list.css`. Four decisions in it are this
    engine's rather than the reference's:
    - **A `list` is `container-type: size`**, as `x-list.css:9` is. That is
      what gives `contain-intrinsic-size: none auto
      attr(estimated-main-axis-size-px px, 100cqh)` a container to resolve
      against. The horizontal rule uses `100cqw` on the width axis. Missing
      estimates fall back in CSS; no custom property or Rust estimate adapter
      is needed. Estimates require nonnegative numbers, with no special
      handling of negative values.
      The consequence is the one a browser has too: a size query container is
      a contained box, so a list's own size never answers to its cells and a
      list with no declared size is a zero-height scroller.
    - **Per-attribute UA `display` rules select the layout mode.**
      `list[list-type="flow"] { display: grid }` and
      `list[list-type="waterfall"] { display: grid-lanes }` are plain
      declarations, so an author `display` on the list overrides them — the
      sheet may not use `!important` (§D.15). This resolves the second of the
      two decisions left open by the grid-lanes path assessment in
      [tracking/deviations.md](tracking/deviations.md); a list with no
      `list-type` still takes its display from `defaultDisplayLinear` like
      every other container, so the 2026-08-21 "one rule, one switch"
      decision is untouched for the case it was made about.
    - **The span count defaults to `1`, not web-core's `0`**, because
      `repeat(0, 1fr)` is an invalid track list and would drop the
      declaration outright. The UA reads `span-count` with `column-count` as
      its fallback through `attr(... number)`. A non-inherited `<integer>`
      registration defaults to one, and `repeat(max(1, var(...)), …)` handles
      nonpositive counts. Attribute writes need no custom-element callback.
    - **`wrapper` is exempt from the non-cell suppression.**
      `list > *:not(list-item):not(wrapper) { display: none }` names the tag
      explicitly, because this engine's `wrapper { display: contents }` is in
      the same origin and would lose on specificity, and ReactLynx routinely
      wraps a list's children.

    `sticky-top="true"` selects `position: sticky`, a nonnegative `top` from
    the list's `attr(sticky-offset px, 0px)`, and `z-index: 1`.
    Still absent: horizontal sticky insets, `sticky-bottom` and scroll-snap rules.

    **Numeric attribute grammar (2026-09-23 user ruling).** Text limits and
    lane counts require integers; offsets and size estimates require bare
    numbers in pixels. Units, trailing text, CSS expressions and fractional
    counts are invalid. Previous `parseFloat` prefix acceptance and fractional
    maxlength truncation were bugs, not compatibility requirements. Numeric
    `attr()` plus registered syntax supplies validation; there are no text or
    list attribute-reflection components. `tail-color-convert` is enabled only
    by the exact value `"true"` through a UA attribute selector.

24. **CSS Grid Level 3 grid lanes (`display: grid-lanes` +
    `flow-tolerance`): enabled as a user-directed extension beyond Lynx
    parity.** *(Recorded 2026-09-18.)* Native Lynx's `display` has no such
    value — `lynx/tools/css_generator/css_defines/24-display.json` lists only
    `none/flex/grid/linear/relative/block/auto` — and no `flow-tolerance`
    property exists in the generated property set, so the `.web.bundle` wire
    format can carry neither and no real bundle emits them. Like §19 this
    falls outside the wire-format subset framing of §A: it is a deliberate,
    user-directed W3C extension, arriving through inline styles, an author
    sheet, or the UA sheet. In the vendored stylo fork (read
    `git -C vendor/stylo show HEAD` for the patch) the `lynx` feature adds
    `DisplayInside::GridLanes` — block-level, and an item container, so a
    grid-lanes box blockifies its children and flags its `display: contents`
    children exactly as `grid` does (`Display::is_item_container`) — and the
    `flow-tolerance` longhand
    (`normal | <length-percentage [0,∞]> | infinite`, initial `normal`, not
    inherited, percentages against the container's grid-axis content-box
    size), which also had to be seeded in `lynx_properties.txt` because
    `lynx_only` alone does not enable a property. Both keywords survive to
    computed-value time on purpose: layout, not the cascade, resolves them.
    On top of that grammar this repo adds the algorithm
    (`crates/hughie/src/compute/grid/lanes.rs`, the `GridLanesStyle` trait)
    and its host dispatch (`DisplayMode::GridLanes` in
    `crates/dom/src/layout/style.rs` and `layout/host.rs`). Motivation: the
    Lynx `<list list-type="waterfall">` component — the feasibility
    assessment for that path is recorded in
    [tracking/deviations.md](tracking/deviations.md).

    **v1 scope:**
    - **No `inline-grid-lanes`**, which css-grid-3 does define. The fork's
      lynx grammar has no inline-level container value at all —
      `inline-flex` and `inline-grid` are gated out the same way
      (`vendor/stylo/style/values/specified/box.rs`, `DisplayKeyword::parse`)
      — so the inline-level partner is left out alongside them.
    - **No orientation property and no `grid-auto-flow: normal`.**
      css-grid-3 §2.3 still marks the orientation property "TBD", so only the
      initial-value rule exists: the block axis carries the tracks exactly
      when `grid-template-columns` is `none` and `grid-template-rows` is not,
      and `grid-auto-flow`'s `row`/`column` are ignored.
    - **`flow-tolerance: normal` resolves to `1em` in layout, `infinite` to
      an unbounded threshold.** The draft's computed-value line names only a
      computed `<length-percentage>`, so resolving the two keywords against
      the element's own computed font size is this implementation's choice,
      recorded as one. It is why `GridLanesStyle` carries a `font_size`
      accessor — no other length in layout is font-relative.
    - **Not implemented:** `dense` backfilling (§4.4 step 4), §6.4
      stacking-axis self-alignment, baseline alignment and baseline sharing
      in either axis, §3.1.1's intrinsic `repeat(auto-fill, auto)`, §3.4.2's
      virtual-item grouping, subgrid, and fragmentation.

    The pipeline and what it reuses from Grid live in
    [layout-architecture.md](layout-architecture.md); the tracking rows are
    in [tracking/css-layout.md](tracking/css-layout.md).

25. **Scroll chaining and snapping (user-directed, 2026-09-22):
    `overscroll-behavior`, `scroll-capture`, and css-scroll-snap-1 without
    its events.** Lynx has none of these properties — its nested-scroll
    protocol is attribute-wired (`scroll-forward-mode`, `enable-nested-scroll`;
    see [tracking/dom-events.md](tracking/dom-events.md)) and web-core sets
    nothing, relying on the browser's default chaining. This engine drives its
    own chain, so the standard surface is what an author gets:
    - **`overscroll-behavior: auto | contain | none`** (css-overscroll-1) per
      axis, enabled as a W3C extension beyond Lynx's index the way `contain`
      was. `contain` and `none` fence everything above the container on that
      axis; `none` equals `contain` here because there is no default
      rubber-band or boundary effect to suppress. It applies to every scroll
      container, `overflow: hidden` ones included, so a hidden wrapper can
      fence a chain it cannot itself consume.
    - **`overscroll-behavior: contain-bounce`** (user-directed 2026-09-22) is
      the engine's own fourth value, declared under the fork's `lynx` feature
      only: `contain`'s fence plus the boundary effect. A drag past the edge
      stretches the container on a rubber band, a fling overshoots it, and
      it springs back. The curves and constants are lynx-ui's `useBounce`
      hook, so a page that bounces through the property and one that bounces
      through the hook's transforms move alike: rubber band
      `d(x) = (1 − 1/(x·0.55/L + 1))·L` for `x` px of finger travel past the
      edge and `L` the scrollport extent (never reaching `L`), overshoot
      decay `0.99` per ms, bounce back `x(t) = (C₁ + 15·C₁·t)·e^(−15t)`
      (critically damped, no initial velocity). The stretch is the painter's
      alone — a programmatic scroll clamps, the document never stretches,
      and a wheel tick on a stretched container first lands it on its edge.
      All of it lives in the painter (`crates/bobcat-core/src/paint/motion.rs`
      for the curves, `paint/inertia.rs` for when they start and stop); the
      document only publishes the `bounce` axes on each scroll slot.
    - **`overscroll-behavior: circular`** (user-directed 2026-10-07) is the
      engine's own fifth value, declared under the fork's `lynx` feature only
      (`OverscrollBehavior::Circular`): the axis has no boundary. Its
      scrolling area repeats with a period of the whole scrolling area on
      that axis (`max_offset` plus the scrollport), so scrolling past the end
      continues from the start and back. Chaining is fenced as for `contain`:
      a circular axis absorbs every delta, so nothing reaches the container
      above. It is the painter's alone. The painter's live offset stands on
      the circle: a drag, fling or wheel step never meets a wall or a
      stretch, snap positions repeat every period (a flick forward from the
      last page lands on the first), and the offset is normalized into the
      period wherever it is composed, hit-tested or sampled, and at each
      rebase. While the scrollport straddles the seam the committed frame
      draws the container's content a second time, one period back, right
      after the container's own content in paint order and inside the same
      clip, so whatever paints over the container later still paints over
      the copy; hit testing tries an item at its place and then at its copy
      in the same front-to-back walk. The document treats the value as
      `contain` and keeps clamping every offset it writes to `0..=max`; what
      the painter posts is its offset modulo the period, clamped, so through
      the seam main sees `max_offset` (a scroll timeline reads its end until
      the wrap completes), as it sees the edge of a `contain-bounce` stretch.
      On a circular axis the committed encode window is the whole scrolling
      area and never re-centres, so a `content-visibility: auto` box inside
      it is always relevant on that axis. An axis whose content does not
      overflow wraps nothing. A programmatic scroll takes its target as the
      document clamped it and moves there the direct way, never round the
      seam. Known approximations of the seam copy: a sticky or anchored box
      and an exported animation inside it sample the unshifted offset, so
      they are positioned as in the first copy; a backdrop texture inside it
      is the one baked for the original; nested circular containers that
      straddle at once each draw only their own copy, not the
      wrapped-inside-wrapped combination; and the copy's offset is snapped
      to device pixels on its own, so with a non-integer period × device
      pixel ratio the two copies may meet one device pixel apart.
      `x-swiper[circular]` is wired to it
      (`crates/bobcat-core/src/main/tree/swiper.rs`).
    - **Inertia** (user-directed 2026-09-22, the same change): a scrolling
      drag's release carries its velocity over the last 100ms of the finger
      (Android's `VelocityTracker` horizon), and the fling decays
      geometrically at `0.998` per ms — UIKit's normal deceleration rate,
      the one number not from lynx-ui, which leaves the in-range fling to
      the native scroller. Each frame the fling's distance since the last is
      one more chain walk from the slot the drag latched, so it hands off to
      the container above like a drag does, stops dead at a wall, and
      overshoots a `contain-bounce` edge. On a snapping axis the fling is
      aimed at the position its whole travel would settle on, so it lands
      there rather than snapping after it stops. A drag's first step on a
      moving container stops it and takes over. Rest is one physical pixel.
      Nothing outside the painter changed for this: the input router and
      the document's scroll API are as they were, and no event is
      involved. A drag's release on a snapping axis glides to its snap
      position on the bounce back's critically damped spring, keeping the
      release velocity when the position is within one scrollport (an aimed
      fling otherwise), and a script-facing scroll names its behavior — a
      smooth one is a glide from rest, an instant one a jump — through
      `Document::scroll_to_with` (CSSOM-View `scrollTo({behavior})`, which is
      what `<viewpager>`'s `selectTab` calls). There is still no
      `scroll-behavior` property.
    - **`scroll-capture-x` / `scroll-capture-y`: `auto | nearest [ forward
      | backward ]?`**, with the `scroll-capture` shorthand setting both, are
      lynx-vello's own properties, with no W3C or Lynx counterpart (the fork
      declares them `lynx_only`). They are physical axes only, with no
      logical pair: the chain walk moves physical axes, and each axis is
      ordered by its own longhand, so a container can hand its vertical
      gestures to its ancestor while its horizontal ones nest inner first
      (what `<scroll-coordinator>` sets: `scroll-capture-y: nearest forward`
      inside its slot). `nearest` on a scroll container hands a
      gesture that starts in it to the nearest scroll container above it
      first; the container itself moves only once that ancestor cannot. A
      direction narrows that to one sign of delta: `nearest forward` defers
      only a delta that increases the offset on the axis being walked,
      `nearest backward` only one that decreases it, and the other direction
      keeps the inner-first order; `nearest` alone is both (user ruling
      2026-09-29, the per-direction spelling `<scroll-coordinator>` needs:
      forward folds the header first, backward unfolds it last). The
      direction is read per axis and per step — an axis the step does not
      move keeps the inner-first order, and each frame of a fling is a step
      of its own, so a forward fling folds the header and then carries on
      into the content. It reorders the chain and nothing else: reach is
      decided first (so `nearest` beside `contain` stays inside, whatever the
      direction), the walk continues outward past the ancestor, and it nests
      outward-first.
    - **Scroll snapping**: `scroll-snap-type`, `scroll-snap-align`,
      `scroll-snap-stop`, `scroll-padding` and `scroll-margin`, ported to servo
      in the fork behind the experimental pref like the other gecko-only
      Lynx-relevant properties, physical sides only. The engine's choices,
      each recorded in `crates/dom/src/scroll/snap.rs`: the proximity range is
      a third of the scrollport (Blink's ratio); a wheel tick is a
      direction-based operation (`mandatory` steps to the next position ahead
      of it, `proximity` keeps its natural end unless the next position is
      within range — never a position behind it, so small ticks still make
      progress); a drag settles on release; a snapping container is
      re-snapped at rest on every commit, which is also the initial snap;
      `block`/`inline` are `y`/`x`; axes are chosen independently. A drag's
      release glides to its snap position (the inertia bullet above); a wheel
      tick and the at-rest snap after a commit are instantaneous. **Out by request**: the
      `scrollsnapchange`/`scrollsnapchanging` events of css-scroll-snap-2.
      **Not implemented**: §7's same-element preference across axes, and
      snap areas escaping from inside a nested scroll container.
    - **`scroll-initial-target: none | nearest`** (css-scroll-snap-2 §3.1),
      declared under the `lynx` feature only since gecko has no
      declaration. The build records the `nearest` elements per scroll slot;
      the document scrolls each container to its first-in-tree-order target
      as `scrollIntoView` with `block: start`, `inline: nearest` and rebuilds
      the frame in the same commit. The scroll is an instant scroll request,
      so the painter shows it even over an offset of its own. Each new target
      is honoured once (first layout, or a later arrival); the "user no
      longer interested" escape is not modelled, and an unchanged target
      never re-scrolls.

    The one chain walk (`drive_chain`) and the snap rules are shared by the
    document and by `bobcat-core`'s painter over the frame's scroll-slot
    table, which now carries each slot's chaining policy and snap positions.

26. **Scroll-driven animations (user-directed): scroll-animations-1
    on the main thread and the painter, Blink where the specs are silent.** Native Lynx has no
    scroll timelines; the surface and every choice are in
    [tracking/css-animation.md](tracking/css-animation.md#scroll-driven-animations).
    Two rules belong here because they are about the cascade:
    - **The stale-timelines pass.** A timeline's ranges are layout outputs
      (the source's `max_offset`, the view subject's position) that feed the
      cascade, like a query container's size (19). So `Document::layout`
      resolves every progress-driven animation's timeline after each layout
      pass, writes its sample into stylo's `Animation::timeline_sample`,
      places its range keyframes (`Animation::set_timeline_ranges`), and
      re-cascades the elements whose sample, binding or keyframes changed
      before the containers' own marks are made (an animation-only traversal
      would strip their `RECASCADE_SELF`); a re-cascade that relayouts is one
      more pass of the same bounded loop. This is scroll-animations-1 §5.1's
      extra style and layout pass: the commit that creates an animation
      already shows it at the offset it found, never its base value first.
      What that final pass changes is re-sampled at the next commit, as §5.1
      allows. Between commits an adopted scroll re-samples only the
      animations its container drives that the committed frame does not
      sample itself (`Document::advance_scroll_timelines`) — one
      animation-only restyle, no layout unless an animated property moves a
      box. An exported one is the painter's between commits, and stale here
      (12).
    - **An animation on an inactive timeline is not current.** It is idle
      (Blink), so it has no effect whatever its fill and none of 11's side
      effects: no stacking context, group or containing block. A binding is
      resolved in the same `layout()` that creates the animation; until then
      it counts as active, so an animation on an active timeline — the usual
      case — never flips its bits.

27. **`if()` and the tree-counting functions (user-directed, 2026-09-28):
    css-values-5 §8.3 and §10, from the fork's `lynx` feature.** Neither is
    in Lynx: `lynx/core/renderer/css` names no `if()`, `sibling-index()` or
    `sibling-count()`, and `lynx-stack/packages/web-platform` authors none.
    The first user is the `<viewpager>` initial-page rule: a registered
    `<integer>` custom property set from `attr(select-index type(<integer>),
    …)` on the pager, and on each page
    `scroll-initial-target: if(style(--… : calc(sibling-index() - 1)): nearest;
    else: none)`. With the `lynx` feature off, the fork is upstream: `if` is
    not an arbitrary substitution function and the tree-counting functions
    stay behind `layout.css.tree-counting-functions.enabled` (pinned by
    `vendor/stylo/style/tests/lynx_feature_off_parity.rs`).
    - **Implemented.** `if()` is an arbitrary substitution function in any
      property, custom or not, shorthands included. At parse time a
      declaration is checked against the argument grammar only: `;`-separated
      branches, an optional trailing `;`, a condition that is not empty and
      holds no top-level `:`, `,`, `{}` block or `!`, and an optional value
      that holds no top-level `!`; anything else drops the declaration. A
      value may hold top-level commas and `{}` blocks. At computed-value time
      the branches are taken in order ("replace an if() function"): the
      condition's substitution functions are substituted, the result is
      parsed as `else | <boolean-expr[ <if-test> ]>`, evaluated, and only the
      first true branch's value is substituted. No true branch is the empty
      token stream, which a custom property keeps as its value and any other
      property, or a registered custom property whose syntax rejects it,
      treats as invalid at computed-value time. `not` / `and` / `or` /
      parentheses follow Appendix B: `<general-enclosed>` is unknown, and an
      unknown top-level result is false. The tests reuse the fork's parsers
      and evaluators: `style()` is the `@container style()` grammar including
      its range form (`style(--x > 3)`, `style(1 < --x <= 5)`), evaluated
      against the element's own custom properties; `media()` is a
      `<media-feature>` or `<media-condition>` against the device;
      `supports()` is `@supports` against this engine's `lynx` grammar.
      `var()`, `attr()`, `env()` and `if()` nest in either half of a branch,
      and `if()` nests in `var()` fallbacks (but see the typed `attr()` gap
      below). `sibling-index()` and `sibling-count()` parse wherever a
      number, integer or dimension is calculated with element context —
      declarations, registered custom property values, `style()` values and
      ranges — and not in `media()`, which has none. Under `lynx` both mark
      the style uncacheable in stylo's rule cache, which is keyed by rule
      node and not by parent; upstream marks only `sibling-index()`.
    - **attr()-taint (§8.7.2).** An `if()` result is attr()-tainted when the
      text of any condition substituted up to and including the chosen one
      was, or when any value a `style()` test read or substituted while those
      conditions were evaluated was (the queried custom properties, the
      parent's value for `inherit`, a feature or range value). A branch not
      reached taints nothing. A tainted result cannot be used as a URL.
    - **The cycle rule.** A custom property's `if()` is ordered by the
      custom-property dependency walk the way `var()` is: branch by branch,
      the condition's own references, the custom properties its `style()`
      tests read, and the font-relative units its text spells resolve first;
      then the condition is evaluated, and only the chosen value's references
      and font-relative units are walked. A branch that is never reached adds
      no dependency, font-relative units included: with a registered
      `<length>` `--l`, `--x: 0; --l: if(style(--x: 1): 2em; else: 10px);
      font-size: var(--l)` is `10px`, not a cycle. (The upstream `var()`
      path still counts the units of an unused fallback, and is unchanged.)
      A `style()` test that reaches the property being substituted puts it
      in a cycle, which makes it invalid at computed-value time (§8.3's
      "evaluates to false" plus the cyclic substitution context it marks).
    - **Choices where the text is silent or the tests disagree.**
      (a) A condition whose substitution fails (it holds a substitution
      function that substitutes to the guaranteed-invalid value) is parsed
      from its original text, with every substitution function in it left in
      place; the successful substitutions in the same condition are
      discarded too. A `style()` feature value holding one substitutes again
      when the feature is evaluated, fails, and makes the feature false; any
      other test holding one does not parse and is `<general-enclosed>`, so
      unknown. So `if(var(--missing) or style(--x: 1): a; else: b)`,
      `if(media(width > var(--missing)) or style(--x: 1): …)` and
      `if(supports(display: var(--missing)) or style(--x: 1): …)` are `a`,
      `if(not var(--missing): a; else: b)` is `b`, and with
      `--c: style(--x: 1)`, `if(var(--c) or media(width > var(--missing)):
      a; else: b)` is `b` although `var(--c)` alone would be true. This is
      what wpt `css/css-values/if-cycle.html` expects
      (`style(not (--x: var(--y)))` with `--y` cyclic is true) and what Blink
      does; the specification does not say how the guaranteed-invalid value
      parses. (b) A `{}` wrapper (§3.1.1) is not implemented on either half:
      a condition with a top-level `{}` block, wrapper or not, is rejected at
      parse time, and a value keeps its braces (`if(else: {a})` is `{a}`).
      A value may hold top-level commas, as in Blink's `ConsumeIf`
      (`third_party/blink/renderer/core/css/parser/css_variable_parser.cc`),
      where §3.1.1 would exclude them outside a wrapper. (c) `style()`
      compares values without their attr()-taint (§8.7.2's taint restricts
      use, it is not part of the value; wpt `if-conditionals.html` "Equality
      of attr-tainted if()"). (d) `unset` computes with respect to the
      element, and a feature without a value on a registered property is
      true only when it differs from the initial value (css-conditional-5
      §6.2); **Blink** makes `style(--r)` on a registered property at its
      initial value true. (e) No true branch is the empty token stream, as
      §8.3 says, so `--x: 0; --p: if(style(--x: 1): a); color: var(--p,
      green)` substitutes nothing into `color`, which is then invalid at
      computed-value time and inherits; **Blink** makes the `if()` invalid
      at computed-value time there, so the fallback `green` applies.
      `@container style()` keeps the fork's upstream behaviour on (c) and
      (d).
    - **Not implemented.** The spread syntax `...var()` (Appendix A);
      `style()` on standard properties (`style(color: red)` is
      `<general-enclosed>`, so unknown); the `{}` wrapper (b above); the
      `color` media feature, which the servo device does not have. `if()` is
      not valid outside property values (descriptors, `@media` preludes).
      Under `lynx`, `if` is one of the substitution functions a
      `<style-range>` value may name, so `@container style(if(…) > 3)` would
      parse as a container condition; no stylesheet reaches it, because the
      `@container` rule is gecko-only in `stylesheets/rule_parser.rs` and
      container style queries stay behind `layout.css.style-queries.enabled`
      (false) — pinned by `lynx_if_function.rs`.
    - **Invalidation.** `dom` adds no code for it. A queried custom property
      of the element's own changes with its own declarations; an inherited
      one changes the parent's inherited custom properties, and stylo
      recascades the children. A test that reads the parent's value of a
      registered property that does not inherit (`style(--l: inherit)`)
      flags the element `INHERITS_RESET_STYLE` (a fork change, the flag an
      explicit `inherit` of a reset property sets), so a change to the
      parent's non-inherited properties recascades it. An attribute read
      through `attr()`, in a condition or a chosen value, lands in the
      element's `attribute_references`, which
      `note_generated_attribute_change`
      (`crates/dom/src/style/invalidation.rs`) turns into a recascade of
      that element. A tree-counting function flags the parent
      `MAY_HAVE_TREE_COUNTING_FUNCTION`; inserting or removing a child then
      recascades its element children (`note_child_list_change`). A device
      change recascades the whole tree (`change_device`,
      `crates/dom/src/style/engine.rs`), which re-evaluates `media()`. Each
      is paid at the mutation; nothing runs per frame or per commit.
    - **Known gap (fork, pre-existing).** A typed `attr()` is substituted
      from the element's attribute map, which the cascade fills from the
      typed `attr()` references at the top level of a non-custom property's
      value and of its `if()` branches, and, for a custom property, from the
      references the dependency walk reaches — which follows a `var()` or
      `attr()` fallback only when the primary is guaranteed-invalid and
      counts a present attribute as valid whether or not it parses. So a
      typed `attr()` inside a fallback of a non-custom property is never
      substituted and takes its own fallback, or makes the declaration
      invalid: `width: var(--m, attr(data-w type(<length>)))`,
      `width: var(--m, if(else: attr(data-w type(<length>))))` and
      `width: attr(data-nope type(<length>), attr(data-w type(<length>)))`
      are all `auto` with `data-w="9px"`. In a custom property the same
      happens when the primary is a present attribute that does not parse:
      `attr(select-index type(<integer>), attr(initial-select-index
      type(<integer>), -1))` with `select-index="two"` and
      `initial-select-index="3"` is `-1`, not `3`
      (`crates/dom/tests/if_function.rs`, the ignored
      `recipe_a_non_integer_first_attribute_falls_through_to_the_second`).

28. **css-anchor-position-1 (user-directed, 2026-09-29 and 2026-09-30):
    the Editor's Draft module, current spellings only, minus the list under
    "Out" below.** Not in Lynx: `lynx/core/renderer/css` has no anchor
    property or function, and web-core reaches the same geometry with a
    `ResizeObserver`. The first user is the `<scroll-coordinator>` slot,
    which reads its header's and toolbar's heights
    (`top: anchor-size(--h height, 0px)`,
    `height: calc(100% - anchor-size(--t height, 0px))`).
    - **Surface.** The fork's `lynx` feature parses `anchor-name`,
      `anchor-scope`, `position-anchor` (`normal | none | auto |
      <anchor-name> | match-parent`, initial `normal`), `position-area`,
      `anchor()` in the inset properties (the `inset` shorthand included)
      and `anchor-size()` in `width`/`height`, `min-*`/`max-*`, the insets
      and the margins — on their own or inside a math function; an
      `anchor()` fallback may hold further anchor functions, an
      `anchor-size()` fallback no further `anchor-size()` (see "Gaps") —
      `anchor-center` in `justify-self`/`align-self`/`place-self`
      (not in the `*-items` properties), `position-try-fallbacks`,
      `position-try-order`, the `position-try` shorthand, `@position-try`
      and `position-visibility` (`always | [anchor-valid || anchor-visible
      || no-overflow]`, initial `anchor-visible`; by the user's ruling the
      legacy `anchors-valid`/`anchors-visible` are rejected as parse
      errors, and WPT's `position-visibility-*` parsing cases, which use
      them, are ported in the current spelling). The computed value keeps every anchor function;
      only layout resolves them. `@position-try` admits exactly the ED's
      properties that exist on the Lynx surface — the physical insets,
      `inset-inline-*`, `inset-inline`, `inset`, the physical and
      inline-axis margins, `width`/`height` and their min/max,
      `justify-self`, `align-self`, `place-self`, `position-anchor`,
      `position-area` — and drops custom properties, every other property
      and an `!important` declaration (that declaration only). The
      block-axis logical longhands (`inset-block-*`, `margin-block-*`,
      `block-size` and friends) are not Lynx properties at all, so they are
      dropped there as they are in a style rule. With the feature off the
      fork is upstream (`vendor/stylo/style/tests/lynx_feature_off_parity.rs`);
      the grammar is pinned by `vendor/stylo/style/tests/lynx_anchor_positioning.rs`.
      The Lynx alignment grammar has no `normal`, `self-start`/`self-end`,
      `left`/`right`, `last baseline`, `safe` or `unsafe` for
      `justify-self`/`align-self`; `auto` stands for `normal`.
    - **Who resolves what.** `hughie`'s absolute pass owns every rule that
      turns anchor geometry into a box — `anchor()` and `anchor-size()`
      (`compute/anchor.rs`), the `position-area` grid and its default
      alignment (`compute/anchor_area.rs`), `anchor-center`, the
      self-alignment of absolutely positioned boxes (`compute/mod.rs`) and
      the §6.5 fallback loop (`compute/anchor_fallback.rs`). The `dom` host
      (`crates/dom/src/layout/anchors.rs`) owns what is a tree walk or a
      cascade: the target anchor element, the default anchor, the anchor's
      rectangle with remembered scroll offsets, the scrollable containing
      block, the cascaded position options, the last successful option and
      the settle loop. The frame's space tree (`crates/dom/src/visual/anchored.rs`)
      owns the default scroll shift and `position-visibility`, sampled at
      compose and hit-test time; the main thread's scroll adoption asks the
      document whether a shift requires a new fallback determination. The
      seam is `docs/layout-architecture.md`; the storage and the settle loop
      `docs/dom-architecture.md` ("Anchor positioning"); the compose side and
      the wake `docs/runtime-architecture.md` ("Anchored boxes compose").
    - **Target anchor element (§2.3).** Candidates come from a name index
      the style harvests maintain (entries only for elements declaring
      `anchor-name`, `anchor-scope` or `position-try-fallbacks`), so a
      lookup costs the elements declaring the name — and under
      `anchor-scope` only those whose nearest scope for the name is none or
      one of the query box's scoping ancestors (the definers are partitioned
      by nearest scope once per change of the name's registry generation),
      so N list items each scoping one shared name cost O(N) candidates per
      layout, not O(N²). A candidate qualifies
      when its name *loosely* matches the reference (declared in the
      reference's tree or a shadow-including ancestor tree — the tree read
      back from the cascade level the fork records, as for timeline names),
      it generates a principal box (`display: contents` and `none` name
      nothing) and holds a committed box this pass that is not hidden,
      `anchor-scope` lets it through both ways (its own nearest scope for
      the name must contain the query box; the query box's nearest scope
      must contain it; scopes match strictly, `all` included, and follow the
      flat tree), and it is acceptable: it, or recursively the element
      generating its containing block, shares the query box's original
      containing block and is in flow or an absolutely positioned box
      earlier in flat tree order. The nearest qualifying ancestor wins, else
      the last in flat tree order. **Approximated:** the top layer exists
      (§29) but this clause's top-layer condition is not implemented (every
      box is treated as in the same layer); the initial containing block and the
      viewport are one containing block, so a `fixed` box can anchor to
      anything in flow (Blink's behaviour); "tree order" is the flat tree's;
      the skipped-contents clause is subsumed by the committed-box check (an
      element in skipped contents has no box, and a positioned box in the
      same skipped contents is not laid out either). `hughie` asks only from
      a containing block's absolute pass, where everything in flow under it
      and every earlier box it is the containing block of is already
      committed: a box escaping a static wrapper (an `absolute` box under a
      non-positioned parent, a `fixed` box under a transformed ancestor) is
      laid out by that pass too, in flat tree order with the block's own
      out-of-flow children, as css-position-3 orders it; only a box whose
      containing block is the initial one is placed after the run, which is
      where the initial containing block's own out-of-flow phase falls.
    - **Default anchor (§2.4).** `<anchor-name>` → the target anchor
      element; `match-parent` → the flat-tree parent's default anchor when
      the parent is absolutely positioned (`position-anchor` applies to
      nothing else) and that anchor is acceptable for the box; `normal`,
      `none` and `auto` → none (no host language here defines an implicit
      anchor element, and `normal` is `auto` only with a `position-area`).
      `match-parent` reads the parent's default anchor under the position
      option the parent was laid out with (its outcome), since
      `position-anchor` is an accepted `@position-try` property. Each
      question reaches the host once per option per absolute layout of the
      box — the engine keeps the answers for that layout, never across a
      pass, because the same pass can measure an escaping box earlier for
      its static position, before its anchors are placed. A box whose only
      anchor-positioning property is a `position-anchor` naming an element
      is still on the anchored path (`CoreStyle::names_position_anchor`),
      so it gets its scrollable containing block.
    - **Geometry and the anchor functions (§3.2, §5.1.1).** The host answers
      a target's unrounded border box of the current pass in the
      padding-box coordinates of the element generating the query box's
      containing block (the viewport's for none), unscrolled, relative
      offsets included, transforms ignored, translated by the box's
      remembered scroll offsets ("Scroll compensation", below); `hughie`
      rebases it onto what it lays the box out against (a grid area, the
      scrollable containing block, a `position-area` region).
      `anchor-size()`'s keyword picks the anchor's axis: `width`/`height`
      physically, `block`/`self-block` vertically and
      `inline`/`self-inline` horizontally (no `writing-mode`), an omitted
      keyword the property's own axis. `anchor()`: physical side keywords
      only in insets on their own axis; `inside`/`outside` by the inset's
      side; `start`/`end` by the containing block's `direction`,
      `self-start`/`self-end` by the box's; percentages and `center` between
      start and end in the containing block's direction; the inset is the
      length that puts the inset-modified containing block's edge on that
      anchor edge. An omitted name is the default anchor.
    - **Unresolvable → fallback → initial value.** Both functions resolve
      only on an absolutely positioned box with a target (for `anchor()`,
      also only in an inset on its keyword's axis); every other box — in
      flow, relatively or stickily positioned — takes the fallback. Without
      one the declaration is invalid at computed-value time, approximated
      by the property's initial value at layout time: `auto` for sizes and
      insets, `none` for max sizes, `0` for margins. Inside a math function
      one unresolvable function without a fallback makes the whole value
      initial. The computed value never changes; only layout sees the
      substitution.
    - **Caching.** An anchored box stays cacheable. Its own run reads its
      base style, where the functions take the unresolvable path and an
      option's values do not exist; the absolute pass hands it, on every
      axis where the values it is laid out with differ from its own style's
      (both axes under an `aspect-ratio`), the used border-box size as a
      known dimension, and applies insets and margins itself. So its layout
      input carries everything an anchor or an option contributes, and a
      moved anchor is a cache miss, not a stale hit. An axis sized from
      content with an anchored limit or margin (`width: auto;
      max-width: anchor-size(--a width)`) is measured with the box's size
      styles ignored, in the space its own run would get — the one its
      intrinsic sizing keyword asks for, else the inset-modified containing
      block, a height at the used width — and then clamped; its committed
      input then claims no content independence on that axis. Trial
      options are measured; only the chosen one is committed. A box with
      options claims no content independence (its choice depends on its own
      size). A box that uses none of the module pays one style predicate,
      and asks the host to count its options only when its
      `position-try-fallbacks` lists something; the anchored path is a cold,
      out-of-line function.
    - **Order of the absolute pass, per option.**
      1. *Options (§6.1, §6.5.2).* The host hands each option over already
         cascaded (index 0 = the box's own style); the engine reads only
         the accepted properties from it and the default anchor through the
         host.
      2. *Containing block.* With a default anchor, css-position-4's
         scrollable containing block (the generator's in-flow scrollable
         overflow, never smaller than the padding box; see below) replaces
         the generator's padding box; a grid area is not replaced. §3.1.1's pre-modification containing block is that
         (or the grid area); css-position-3 §2.1.1's original containing
         block — used only as css-align-3 §4.4.1.2's overflow limit — is the
         scrollable containing block when there is one, else the
         generator's padding box, even for a grid item.
      3. *`position-area` (§3.1).* With a default anchor: the 3×3 grid from
         the pre-modification containing block and the anchor (lines 1 and
         4 extend to an anchor outside it, so tracks can be empty), keywords
         through the fork's `to_physical` with `horizontal-tb` writing modes,
         and the region becomes the containing block, percentages included.
         `auto` insets and margins become 0. `normal` self-alignment takes
         §4.1's value (`center`, `anchor-center`, or toward the anchor),
         except that a single `auto` inset on an axis aligns *unsafely*
         toward the other one. Without a default anchor the property does
         nothing, but still counts as referencing it (`anchor-valid`).
      4. *Anchor functions*, as above, in the coordinates of the containing
         block after step 3.
      5. *Self-alignment* (below), including `anchor-center` (§4.2): with a
         default anchor, `auto` insets and margins on that axis become 0,
         the auto size is fit-content in the inset-modified containing
         block, and the margin box is centered on the anchor's center, then
         shifted by §4.4.1.2 (so it stays inside the inset-modified
         containing block when it fits). Without one it is `center`, with
         no effect on insets.
      6. *Fallback (§6.5).* The current option is the host's last
         successful one, else the base style. It is committed; if its margin
         box fits its inset-modified containing block (a layout unit of
         slack, 1/64 px) and that block was not negative-size, nothing is
         determined. Otherwise the options are tried in list order — stably
         sorted by their inset-modified containing block size, `auto`
         insets as 0 and margins out, for `position-try-order`
         (`most-height`/`most-block-size` vertical, the others horizontal) —
         skipping the current one, each as a measurement; the first that
         fits is committed. None fits → the current option's commit stands.
         `hughie` never records the last successful option; the host
         does (below).
      7. *Outcome.* For every committed anchor-positioned box: the chosen
         option, whether it still overflows (`no-overflow`), whether it
         references the default anchor (`position-area`, `anchor-center`,
         an unnamed function) and resolved one (`anchor-valid`), §3.3's
         per-axis compensation (default anchor present and `anchor-center`
         on that axis, or any `position-area`, or a resolved `anchor()` in a
         used inset on that axis whose target has the default anchor's
         nearest scroll container with that axis scrollable — the host
         answers that last part), and the inset-modified containing block
         and margin box in the generator's padding-box coordinates. Boxes
         using none of the module report nothing.
    - **Self-alignment of absolutely positioned boxes (css-position-3 §4,
      css-align-3 §6.1.2/§6.2.2) — new for every absolutely positioned box,
      anchored or not.** `normal` (and `auto`) and `stretch` keep the
      previous behaviour (stretch-fit auto size with both insets non-`auto`,
      placed at the start edge the box's own `direction` picks). Every other
      value makes an `auto` size fit-content in the inset-modified
      containing block and aligns the margin box in it — `start`/`end` by
      the containing block's direction, `self-*` by the box's, `left`/`right`
      physically, `baseline`/`last baseline` as their `self-start`/`self-end`
      fallbacks (`self-*`, `left`/`right` and `last baseline` exist only
      inside the engine; the Lynx grammar has none of them). It applies only with both insets and both margins
      non-`auto`: one `auto` inset places the box by the other, `auto`
      margins win. Overflow: `unsafe` (only §4.1's single-`auto` rule
      produces it) honours the alignment; everything else follows §4.4.1.2
      (stay inside the inset-modified containing block if it fits, else
      cover it and stay inside the bounding box of it and the original
      containing block, else start-align there). A negative inset-modified
      containing block is brought to zero at its weaker edge (css-position-3
      §3.5.2: the `auto` one, else the end one). **Flexbox and grid
      containers, every box laid out by a containing block that is not its
      parent (whatever that block's algorithm — Lynx has no such boxes, so
      starlight has no rule for them), and the boxes `dom` places itself
      (initial-containing-block boxes, `<text>` blocks) follow
      css-position-3; Lynx `linear` and `relative` containers follow
      starlight and ignore authored `justify-self`/`align-self` on their own
      absolutely positioned children** —
      starlight places them by their insets alone
      (`lynx/core/renderer/starlight/layout/position_layout_utils.cc`,
      `CalcStartOffset`: the start inset wins, then the end inset, then the
      static position), and those algorithms are not extended.
      `anchor-center` and `position-area`'s defaults, which are opt-in,
      still apply there.
    - **Position options (§6).** Cascaded at the style harvest, once per
      restyle of an element whose `position-try-fallbacks` is not `none`,
      with the fork's `Stylist::resolve_position_try` (the Position Fallback
      Origin over the element's rules — above author normal declarations,
      inline ones included, below `!important` ones — then the try tactic;
      an entry naming no rule is dropped, §6.1; the list has no length
      limit), and re-cascaded when a stylist flush reports a referenced
      `@position-try` name changed. Stored on the tree arenas beside the
      base style. **Last successful option (§6.5.1.1):** recorded once per
      rendering update (`Document::render`, after layout, before paint)
      from the last layout's outcome; `Document::layout` alone never
      records, so a second layout without a render starts from the base
      style again (WPT `position-try-fallbacks-no-fit-after-fit.html`).
      **Fallback-sensitive changes (§6.5.1):** a change noted at the flush
      harvest — `position`, a `position-try` longhand, an accepted
      `@position-try` property of the base style or of any option, a
      referenced rule, generating no box — makes the next rendering update
      forget the option and lay the box out again before it records;
      setting `position-try-fallbacks` to the value it already has is no
      change. The base style compared is §6.5.1's "computed base style …
      ignoring any declarations originating from the Transitions or
      Animations cascade origins": the element's rules cascaded again
      without the Animations, Transitions and SMIL-override levels
      (`RuleTree::remove_animation_rules`, only when it has such rules),
      recomputed at flushes; an animation tick re-cascades the options and
      keeps the base, so it never makes a change. A change of containing
      block association — an ancestor starting or stopping to generate the
      box's containing block — is one too. **Readback:** computed-value readback (`getComputedStyle`,
      `__GetComputedStyleByKey`) reports the chosen option's values for the
      accepted properties; every other property, and layout and paint
      themselves, read the base style (plus the option's geometry, which
      `hughie` reads from the option). Descendants inherit from the base
      style, not the chosen option (see "Gaps").
    - **Scroll compensation (§3.3).** Layout is unscrolled, so a scroll
      between an anchor and its box's containing block relays nothing out.
      The host keeps each anchored box's **remembered scroll offsets** as
      *displacements*: over each anchor's scroll containers up to, not
      including, the box's containing block, minus their scroll offsets,
      plus the sticky shifts of the anchor and its sticky ancestors on that
      chain. They are recorded after the run that reached a **recalculation
      point** — the box's first report (it began generating boxes; a box
      that stops generating one loses its state) and a fallback
      determination that switched options — and an anchor first referenced
      between two points is recorded on first reference, so no layout
      answer ever follows live scrolling. An option being *tried*, and a box
      with no record yet, read the current (stored, clamped) offsets —
      §6.5.2's hypothetical recalculation point — without sticky shifts,
      which need a finished run; the settle loop re-reads when the recording
      then differs. The **default scroll shift** is applied at compose: a
      box that compensates on some axis, or whose `position-visibility` can
      hide it, gets an *anchored node* in the frame's space tree, the
      outermost of its own ("as if affected by a transform (before any other
      transforms)"), whose translation is `L · mask(snap(live) −
      snap(remembered))` with `live = Σ own(sticky) − Σ offset(scroller) +
      fixed` over the default anchor's scroll-adjustment ancestors below
      the containing block, at the offsets the compose or hit test uses —
      the same chain and sign as the remembered displacement, so the shift
      is "the difference between the remembered scroll offset … and what
      its current remembered scroll offset would be". Both terms are
      snapped to the device pixel grid the same way, on the painter and in
      the main thread's fit test, so a box whose scrollers have not moved
      since its recalculation point has a shift of exactly zero at any
      fractional offset. **§6.5 on scroll:** the main thread, at the
      scroll mailbox marker and before every render
      (`Document::redetermine_scrolled_fallbacks`), recomputes the shift at
      the adopted offsets for each box with position options and, on a fit
      → overflow flip, flags it: its next layout reads every option's
      anchors at the current offsets (the current option included, so the
      loop runs from it) and remembers them — every re-determination
      refreshes the remembered offsets, including one that keeps a
      still-fitting current option. The fit test moves the margin box, and
      the inset-modified containing block's *carried* edges, by the shift;
      `hughie` reports which those are (`AnchorOutcome::carried_edges`):
      a non-`auto` inset's edge (an `anchor()` edge moves with the anchor;
      a length or `anchor-size()` edge leaves that side unconstrained, as
      Blink's `CalculateNonOverflowingRangeInOneAxis` does) and an `auto`
      inset's `position-area` line that is the default anchor's own edge.
      Only an `auto` inset's containing-block edge stays, and constrains.
    - **`position-visibility` (§6.6).** A paint-time flag, not a computed
      value: a hidden box's anchored node is the zero map, so the box and
      its containing-block descendants draw, hit and bake nothing, and its
      computed `visibility` is untouched (Blink's model, not the spec's
      `visibility: force-hidden`). A descendant escaping the box's
      containing-block chain (a `position: fixed` box whose containing
      block is outside it) composes through the slot's *visibility node*,
      opened on the fixed-containing-block context the box hands its
      descendants: the same zero map while the box is hidden, the identity
      otherwise, so it hides with the box without following its shift. The
      predicates are
      evaluated per composed frame on the painter and per hit test on the
      main thread, from the frame alone: `anchor-valid` from the outcome;
      `anchor-visible` — the default anchor's `visibility` is not
      `visible`, or its border box (ink overflow is approximated by it)
      mapped through its live space is fully clipped (zero intersection
      area; for a zero-area anchor, no contact) by *one* of the clips between
      it and the box's containing block — the frame's clip nodes, i.e.
      `overflow` and paint containment, each mapped through its live space
      and bounded by its axis-aligned box (radii and `clip-path` are not
      tested); a box anchored to a hidden anchored box, or to anything that
      box's node carries, finds its anchor's space degenerate and hides too;
      `no-overflow` — the shifted fit test above; at a zero shift it is the
      layout's own answer.
    - **Settle loop and relevance.** Every containing block lays its
      out-of-flow boxes out after its in-flow content in tree order, the
      escaping ones included, so a run reads only anchors it has already
      placed. What it misses is an anchor that moved while the reader's
      containing block was served from the cache (an in-place relayout deep
      in a sibling, a `contain: strict` boundary). After every run the host
      re-checks each read the boxes' last committing passes made — only the
      target's rectangle when the name's registry generation is unchanged
      and the target still has a box, the whole lookup otherwise — lays the
      boxes whose answers moved out again, and repeats until every read is
      stable. Within one `layout()` each anchor-positioned box earns a
      re-run at most once (WebKit's bound on its layout-dependency loop, the
      `invalidatedAnchorPositioned` set): a reader's relayout root is never
      deeper than its anchor's, so each stale reader costs one run, and the
      runs are bounded by the number of distinct stale boxes, with no
      constant. §2.3's dependencies are acyclic, but a box that reads two
      anchors whose moves surface in different runs (the ends of two chains
      with different numbers of cache-served hops) goes stale twice; the
      repeat is invalidated and earns no run. The last run's reads are
      checked too; a repeat found there leaves the document dirty, so the
      next `layout()` or `render()` runs again even when nothing else
      changed — one commit late — and no read is ever left unverified. **§2.5:**
      a `content-visibility: auto` element about to skip stays relevant
      when it holds, looked up as if it did not skip, a target anchor of a
      shown anchored box whose containing block is outside it.
    - **Scrollable containing block (css-position-4).** The containing
      block generator's scrollable overflow from its *in-flow* content —
      "ignoring absolutely positioned descendants" — measured from its
      padding-box origin and never smaller than its padding box. Its
      algorithm records it between its in-flow commit and its absolute pass
      (a side table holding scroll containers only), so the box laid out
      against it reads the current run's area, and no out-of-flow box — the
      anchored box itself included — can grow it. Otherwise it is whatever
      the generator's algorithm makes its scrollable overflow: a flexbox's
      ends at its content's far edge and counts a relatively positioned
      child's offset, a grid's ends past the end padding. WPT
      `scrollable-containing-block-size.html` wants the end padding and no
      relative offsets everywhere; the flexbox rows are recorded there as
      differences (one ignored `GAP` test).
    - **Out, with the reason.** Withdrawn spellings (`inset-area`,
      `inset-area()`, `position-try-options`, `@position-fallback`/`@try`,
      `anchor(implicit)`, `anchor-center` on `*-items`) do not parse — the
      ED is the target. Popovers and implicit anchor elements: no host
      language here has them, so `auto` never finds an anchor; the top
      layer and `<dialog>` exist (§29) but name no implicit anchor. Pseudo-elements (not generated in this engine),
      including `match-parent`'s originating-element branch. Writing modes:
      only `direction` exists, and every logical keyword maps through
      `horizontal-tb`. Fragmentation and multicol (no such boxes).
      Transforms in the anchor box geometry (§2's bounding-box rule): the
      layout box is the anchor box. CSSOM (§8), and `IntersectionObserver`/
      `ResizeObserver` as APIs (the recording moments they time are internal
      here). CSS `zoom`, `ident()` (not in the fork), container queries.
    - **Approximated or not done (engine side).** The containing block's
      direction for a box whose containing block is the initial one, and
      for a `<text>` block's own out-of-flow children, is the box's own
      `direction`; a box hoisted to a grid container is placed against its
      padding box rather than the grid area its placement names, and
      hoisted boxes add nothing to their containing block's scrollable
      overflow (both as before this module); §4.4.1.2's extension of the overflow limit rect to a
      scroll container's scrollable area (and to infinity for scrollers) is
      not modelled — the limit is the bounding box of the inset-modified and
      original containing blocks; `normal` still stretches a replaced box
      (css-position-3 §4.1 wants fit-content); the static position ignores
      self-alignment (§3.5.1's `self-end`/`center` rules — both `auto`
      insets still use the start-aligned static position); §3.5.2's
      *resolved* weaker inset for `getComputedStyle` is not produced; the
      content-sized anchored-axis measurement ignores the *other* axis's
      min/max sizes when that axis is also content-sized, and Flexbox's,
      Grid's and Relative's static position (used only when both insets on
      an axis are `auto`) is computed from the box's unresolved values.
    - **Deviations on the compose side.** Every re-determination on scroll
      refreshes the remembered offsets, also when it keeps the current
      option — still fitting at the current offsets, or no option fitting
      (the spec keeps them) — observable only for `anchor()` references to
      non-default anchors in another scroll context; a box that overflowed
      in every option is re-determined on scroll only after it fits again
      and leaves again (Blink re-checks each option's range); a box that
      flips between two options on adjacent boundaries is bounded by one
      re-determination per adopted offset (§6.5.1.1's last successful
      option is recorded, but Blink's anti-flicker rule — skip every option
      that also overflowed at the remembered offset — is not implemented);
      a length inset leaves its side of the scrolled fit test
      unconstrained (Blink's rule, not the spec's fixed inset-modified
      containing block); with
      more than one scroller between anchor and containing block the
      snapped shift may differ from the anchor's pixels by one device
      pixel; the shift sums the intervening scrollers' offsets in the
      containing block's layout space, so transforms on those scrollers are
      ignored (Blink maps through them; an open spec issue); a box anchored
      to an anchored box does not follow that box's own default scroll
      shift (spec-literal: only scroll containers adjust; Blink adds the
      chained translation); `no-overflow` applies only to boxes `hughie`
      reports (anchor-positioned or with options); an anchor with no box
      item of its own (an inline span painted by its paragraph) is never
      found clipped. `bounding_client_rect` (the `boundingClientRect` UI
      method) adds the default scroll shift of the box and of each
      anchored box on its containing-block chain, as
      `getBoundingClientRect` does, but not a `position-visibility` hide
      (in Blink the hidden box reports its rect too).
    - **Gaps (ignored `GAP` tests).** Anchor functions inherit as
      functions, not as the length they resolved to, so a child's `inherit`
      of an anchored inset or size is unresolvable there (WPT
      `anchor-inherited.html`); descendants inherit from the base style,
      not the chosen position option (`inherit-height-from-fallback.html`);
      the fork does not parse an anchor function inside an `anchor-size()`
      fallback (`anchor-query-fallback.html`'s last two cases); the flexbox
      scrolling area above.
    - **Conflicts, the ED followed.** §2.3 now prefers an acceptable
      *ancestor* over the last candidate in tree order; WPT
      `anchor-name-001.html` and `anchor-position-003.html` predate that
      and expect the last one where the query box sits inside an `--a1`
      anchor. `anchor-name-in-shadow.html`'s second case (a shadow tree's
      reference not matching its host's document-tree name) contradicts the
      ED's loosely matched names and `anchor-name-shadow-higher-tree.html`.
      `position-try-order-include-base.html` passes only if a base style
      that fits is re-sorted away; the ED determines fallback only on
      overflow. **Unverified in browsers:** WPT
      `grid-position-area-basic.html`'s reference places its box (as wide
      as the word "Anchored") at an empty `position-area` column 20px from
      the grid container's padding edge, unshifted, overflowing that edge;
      the engine follows §4.4.1.2's text and shifts it back until it ends
      at the padding edge (the mock-host test uses a 30px box).
    - **Tests.** `crates/hughie/tests/anchor_positioning.rs` and
      `anchor_size.rs` (every rule above against a mock host, WPT numbers
      where a file is named); `crates/dom/tests/anchor_positioning.rs` (the
      WPT ports through a real document, each naming its file and
      adaptation, and the skipped groups in its module doc) and
      `anchor_size.rs` (the `anchor-size()` ports, the coordinator geometry
      and its invalidation); the compose-side units in
      `crates/dom/src/visual/anchored.rs`; one GPU pixel test
      (`gpu_pixels.rs`,
      `an_anchored_box_follows_its_scrolled_anchor_and_hides_with_it`); the
      painter's end-to-end tests in
      `crates/bobcat-core/src/paint/event_loop_tests.rs`; the grammar in the
      fork's `lynx_anchor_positioning.rs`. **WPT groups ported:**
      `anchor-name-*`, `anchor-scope-*`, `anchor-position-*` (basics,
      borders, dynamic, sibling-index, principal box, circular),
      `anchor-center-*` (horizontal-tb), `anchor-function-*`,
      `anchor-query-fallback`, `anchor-invalid-fallback`,
      `position-anchor-*`, `position-area-*` (physical and logical in
      horizontal-tb), `scrollable-containing-block-*`, `position-try-*`,
      `try-tactic-*`, `last-successful-*`, `at-position-try-*`,
      `mixed-dependency-chain`, `anchored-c-v-hidden`, the parse and
      computed-value files (in the fork), and a few reftests as geometry.
      **Skipped:** top layer, popover, dialog and `::backdrop` (the top
      layer exists since §29, but not its anchor clauses);
      pseudo-elements; writing modes; multicol, inline fragmentation,
      tables and fieldsets; `transform-*`; `zoom`, print and iframes; CSSOM,
      Typed OM, `getComputedStyle` insets and IDL; animations, transitions
      and interpolation; container queries; `ident()`;
      `CSS.registerProperty`; stylesheet removal; `-crash` files;
      `anchor-scroll-*`, `position-visibility-*` and the other
      scroll-compensation files, whose numbers include the default scroll
      shift (covered instead by the painter and compose tests above);
      root-element and initial-containing-block sizing
      (`position-area-fixed`, `position-area-overflow-icb-*`); and the
      remaining reftests.

29. **The top layer, `<dialog>` and `::backdrop` (architect-decided,
    2026-10-07): css-position-4 §3 and HTML's `<dialog>`.** Bucket 1: web-core
    runs in a browser, so a card that writes `<dialog>` gets the browser's
    `HTMLDialogElement`, and web-elements' `x-overlay-ng` is itself a
    `<dialog>` opened with `showModal()` (`htmlTemplates.ts:136-172`). Native
    Lynx has neither a dialog nor a top layer — its `<overlay>` is a
    zero-sized box whose content is reparented into a platform window. The
    layer is generic in `dom` (`crates/dom/src/tree/top_layer.rs`,
    `docs/dom-architecture.md` "Top layer and `::backdrop`"); `<dialog>` is
    its first user (`crates/bobcat-core/src/main/tree/dialog.rs`).
    `<x-overlay-ng>` itself is a follow-up on this foundation.
    - **Implemented.** A document top layer: an ordered set with a per-entry
      "blocks the document" flag (HTML's modal dialog). A top-layer element is
      a stacking context of its own, painted after everything in the root
      stacking context in layer order, with the viewport as its containing
      block whatever its ancestors' transforms, filters, containment, clips,
      opacity, `z-index` or scroll offsets, a static position of zero, and no
      contribution to its parent's flow, intrinsic size or scrollable
      overflow. `::backdrop` is a real box for every rendered entry: painted
      immediately below its element, inheriting from it, styled by every
      origin's `::backdrop` rules through Stylo's lazy pseudo-element cascade,
      and suppressed by `display: none` or `content: none`. While a modal
      dialog blocks the document, nothing painted before its `::backdrop` is
      hit-testable (HTML's inert subtrees), and a hit on the backdrop reports
      the dialog. `:modal` and `:open` match. A top-layer element is relevant
      to the user (css-contain-2 §4). `<dialog>`: HTML's UA rules, the `open`
      attribute, `show()`, `showModal()`, `close()` and `requestClose()` as UI
      methods (`invoke` / `__InvokeUIMethod`), and non-bubbling `close` and
      `cancel` events with a `{}` detail, delivered from an entry of their own.
    - **UA rules, adapted.** HTML's `dialog` rules, with: the display
      `defaultDisplayLinear` picks for containers (`linear`, else the fork's
      initial `flex`) for HTML's `block`, which no box here lowers to;
      `dialog[open="false"]` closed beside `dialog:not([open])`, because
      `__SetAttribute` stringifies `false` and web-core removes such an
      attribute; `white`/`black` for `Canvas`/`CanvasText`, because the
      fork's `lynx` build parses no system colour (they are the values both
      take in the light scheme, the only one this engine has);
      `overflow: scroll` for `dialog:modal`'s `overflow: auto` (out of this
      engine) and `top: 0; bottom: 0` for its `inset-block: 0` (disabled in
      the fork); and `::backdrop { position: fixed; inset: 0; display: flex }`,
      the display because the pseudo-element has no other source of one `dom`
      lowers. No rule is `!important`.
    - **Membership stands in for §3.1's fixups.** The fork compiles
      `-servo-top-layer` and `StyleAdjuster::adjust_for_top_layer`, but the
      `lynx` build keeps the longhand out of its property-name table, so no
      sheet can declare it and the fixups never run. `dom` reads membership
      instead: a top-layer element and its backdrop lower to `fixed` against
      the viewport whatever their computed `position`, and a top-layer
      element ends its descendants' containing-block walks. Consequences: the
      computed `position` of a top-layer element is the author's (a browser
      computes a non-`absolute`/`fixed` value to `absolute`; where the box
      renders is the same), and a `display: contents` top-layer element is
      not blockified and so renders nothing. The UA rules write
      `position: fixed` on `dialog:modal` and `::backdrop` explicitly. Fork
      follow-up: expose `-servo-top-layer` to UA sheets.
    - **State.** "Open" is the `open` attribute, present and not `"false"`;
      the attribute callback is the one path that flips `:open`, and its
      removal (or `"false"`) also leaves the top layer and clears `:modal`,
      without a `close` event, as HTML's attribute steps fire none. "Modal" is
      membership with the blocking flag; a modal dialog removed from the
      document leaves the layer (HTML's removing steps) and comes back open
      but not modal.
    - **Invalid states.** `show()` on a modal dialog, `showModal()` on an
      open non-modal or a disconnected dialog: HTML throws
      `InvalidStateError`; `invoke` answers 4 `PARAM_INVALID`, which is what
      web-core reports for any method that throws
      (`createInvokeUIMethod.ts:12-44`). Native has a distinct
      `7 INVALID_STATE_ERROR` (`lynx_get_ui_result.h:53-61`); web-core is
      followed (`docs/tracking/deviations.md`).
    - **Gap.** HTML centres a modal dialog by shrink-to-fit sizing
      (`width: fit-content; height: fit-content; margin: auto` between
      zero insets). hughie sizes `fit-content` on an absolutely positioned
      box with both insets set as `auto` (stretch), so a dialog with no
      author size fills the viewport up to its `max-width`/`max-height`
      until that is fixed (an ignored `GAP` test in `dialog.rs` and in
      `crates/dom/tests/layout.rs`).
    - **Out, with the reason.**
      - The `overlay` property, transitions on it, and the pending top-layer
        removals: the fork has no `overlay`, so nothing could observe a
        delayed removal; leaving the layer is immediate.
      - A top-layer element inside skipped contents (`content-visibility:
        hidden`, or a non-relevant `auto`) stays hidden with them, where
        css-position-4 renders it: the hoisting, skipping and rounding paths
        all hide it today. A `display: none` ancestor correctly hides it.
      - Close requests (Escape, the back gesture), `closedby`, light dismiss
        and the close watcher: no keyboard input, and `closedby` is not
        parsed. `requestClose()` is the close watcher's `cancel` then `close`
        without cancelability — this engine's event model has no
        `preventDefault` — so it always closes.
      - `beforetoggle`/`toggle`, the focusing steps and the previously
        focused element, `autofocus`, `returnValue` (no reader exists in Lynx
        JS; `close(returnValue)` and `requestClose(returnValue)` drop it),
        popovers and fullscreen.
      - Animations and transitions on `::backdrop` itself: the lazy cascade
        carries no animation declarations.
      - css-anchor-position-1's top-layer clauses (§28 "Approximated").

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
- Generated-content boxes and `::before`/`::after` (§A.4) — fixture/app demand.
