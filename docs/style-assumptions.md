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
    runs on the document's owner thread (the Lynx main thread, on a
    `BeginFrame` tick with no JavaScript involved), as a stylo animation-only
    traversal over just the animating elements, with no selector matching and
    no layout for properties that cannot move a box. **The open gap is layerization, and it is also why this
    engine does not throttle.** A browser can skip the per-frame restyle
    because a compositor is interpolating instead; here the animation-only
    traversal is the only thing that produces the animated value, and with no
    compose-time layers the whole retained scene is rebuilt every animated
    frame anyway. So what a frame saves today is the cascade over the elements
    that are *not* animating, and the layout pass — not the restyle a browser
    throttles, nor the rasterization a compositor skips. Both of those follow
    from layers, not from a second animation path.

    *Structural side effects are per-animation constants.* A transform/filter
    also creates a containing block for positioned descendants and a stacking
    context, and those must be visible to layout for the whole animation
    rather than flickering with the interpolated value. Resolution (matching
    browser behavior and `will-change` semantics): an element with a
    **running** animation or transition of `transform`/`filter`/`opacity`
    establishes its containing block / stacking context **for the entire
    duration**, even across `none` keyframes — flipped once at start and once
    at end, so layout never needs per-frame animation state.

12. **No animation staleness seam.** Superseded by 11: the cascade output *is*
    the animated value, so computed style is correct mid-animation and a style
    query, a transition starting *from* an animating value, and invalidation
    all read the same one truth with nothing to sample back. The earlier
    query-time overlay existed only to reconcile a render-private value with
    computed style, and there is no render-private value. If layerization later
    introduces one, this item returns with it — and browsers already say what
    it must do then: keep the cascade authoritative and re-sample on demand
    rather than let the two diverge.

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
        admits and the compositor may scroll to without a new commit. The relevance test is the
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
      - **Four of the spec's relevance conditions are N/A here**: this engine has no top layer
        (no `dialog`, no fullscreen), no focus model, no selection, and no view transitions.
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
      var(--estimated-main-axis-size-px, 100cqh)` a container to resolve
      against, so a cell with no supplied estimate is one scrollport tall.
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
      declaration outright. `span-count`/`column-count` reflect only positive
      integers; anything else clears the hint and leaves the default. The
      reflection is the `list` tag's own `CustomElement`, with the cell's
      `estimated-main-axis-size-px` on a second one for `list-item`: an
      attribute-to-CSS mapping lives in its own tag's component, never in a
      shared name-keyed dispatcher (user ruling, 2026-09-21).
    - **`wrapper` is exempt from the non-cell suppression.**
      `list > *:not(list-item):not(wrapper) { display: none }` names the tag
      explicitly, because this engine's `wrapper { display: contents }` is in
      the same origin and would lose on specificity, and ReactLynx routinely
      wraps a list's children.

    Still absent from the list UA sheet: `sticky-top`/`sticky-bottom`
    attribute rules and scroll-snap rules. CSS `position: sticky` itself is
    implemented (2026-09-22); the component attribute mapping is separate.

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
      axis; `none` equals `contain` here because there is no rubber-band or
      boundary effect to suppress. It applies to every scroll container,
      `overflow: hidden` ones included, so a hidden wrapper can fence a
      chain it cannot itself consume.
    - **`scroll-capture: auto | nearest`** is lynx-vello's own property, with
      no W3C or Lynx counterpart (the fork declares it `lynx_only`). `nearest`
      on a scroll container hands a gesture that starts in it to the nearest
      scroll container above it first; the container itself moves only once
      that ancestor cannot. It reorders the chain and nothing else: reach is
      decided first (so `nearest` beside `contain` stays inside), the walk
      continues outward past the ancestor, and it nests outward-first.
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
      `block`/`inline` are `y`/`x`; axes are chosen independently. Snaps are
      instantaneous (no `scroll-behavior`). **Out by request**: the
      `scrollsnapchange`/`scrollsnapchanging` events of css-scroll-snap-2.
      **Not implemented**: §7's same-element preference across axes, and
      snap areas escaping from inside a nested scroll container.
    - **`scroll-initial-target: none | nearest`** (css-scroll-snap-2 §3.1),
      declared under the `lynx` feature only since gecko has no
      declaration. The build records the `nearest` elements per scroll slot;
      the document scrolls each container to its first-in-tree-order target
      as `scrollIntoView` with `block: start`, `inline: nearest` and rebuilds
      the frame in the same commit. Each new target is honoured once (first
      layout, or a later arrival); the "user no longer interested" escape is
      not modelled, and an unchanged target never re-scrolls.

    The one chain walk (`drive_chain`) and the snap rules are shared by the
    document and by `bobcat-core`'s painter over the frame's scroll-slot
    table, which now carries each slot's chaining policy and snap positions.

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
