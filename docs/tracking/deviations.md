# Known Lynx ↔ W3C behavior deviations

> Research: multi-agent sweep over `lynx/` and `lynx-stack/` (see [AGENTS.md](../../AGENTS.md)
> for the reference-repo shorthand and the W3C-first standards policy).

The per-domain research pass found **283 individual rows** marked `No`/`Partial`
in the `W3C-compliant?` column across `css-layout.md`, `css-visual.md`,
`css-text.md`, `css-animation.md`, `dom-events.md`, `js-runtime.md`,
`components.md`, `reactlynx.md`, `css-selectors-cascade.md`, `css-at-rules.md`,
`media-resources.md`, and `accessibility.md` — that's the honest total, and
this file does **not** reproduce all of it (that would just be a copy of
those tables). It curates the ~35 rows with the broadest architectural
impact — the ones that shape a whole subsystem's design rather than a single
property's value grammar. **For the complete list, read the `W3C-compliant?`
column in each file directly** before assuming something isn't covered here.

Most rows are plain "Lynx has a non-standard extra API/property with no CSS
equivalent" (port as-is, no policy decision needed) — those are *not*
curated here. What's below is specifically: (a) cases where Lynx's behavior
actively conflicts with a W3C algorithm/API Lynx *also* claims to implement,
and (b) cases where matching ReactLynx behavior requires an explicit,
consequential choice about whether to follow the spec or the quirk.

## Layout & stacking (see [css-layout.md](css-layout.md))

- **`z-index`/stacking context** — Lynx reparents any element with
  `z-index != 0` once to the nearest stacking-context ancestor and sorts
  siblings by raw integer value; it does not implement the real recursive,
  per-stacking-context CSS algorithm. **Decision: implement the real CSS
  algorithm** (reuse stylo/Servo's existing stacking-context logic) —
  apps relying on Lynx's actual buggy z-index behavior may render
  differently, and that's intentional.
- **`overflow`/`overflow-x`/`overflow-y` default** — Lynx defaults to
  `hidden`; CSS defaults to `visible`. **Decision: match Lynx's default**,
  not CSS's — this is a values/defaults divergence, not an algorithm one,
  and ReactLynx apps assume clipping-by-default. The *keyword set* went the
  other way (2026-07-29): Lynx's native grammar is `visible | hidden`, but
  the fork now ships `scroll` and `clip` too, because the web bundle this
  stack consumes uses both — `web-elements`' `scroll-view.css` authors
  `overflow-y: scroll` and `overflow-x: clip` — so the trimmed set made a
  scrollable box inexpressible. Only `scroll` responds to gestures; `hidden`
  stays a clip that scrolls only programmatically, which is what keeps the
  `hidden`-on-every-element UA default from making the whole page
  drag-scrollable, and `clip` is not a scroll container at all.
- **`position: sticky` does not stick (2026-07-30)** — it parses in the
  fork's grammar and Lynx itself implements scroll-container-relative
  stickiness, but lynx-vello's paint build treats a sticky box as normal flow,
  so it scrolls away with its container. **Not a decision — an unimplemented
  gap**, recorded because interactive scrolling made it observable: before that
  landed, sticky and `relative` were indistinguishable. What is missing is the
  css-position-3 §6.3 offset clamp against the scrollport; the box already
  sits in the right flow and scrolls with the right ancestor.
- **`overflow: auto` omitted** — CSS has it; this engine does not.
  **Decision (user, 2026-07-29): leave it out.** Nothing here paints
  scrollbars, so `auto` ("scrollbars only when needed") would behave exactly
  like `scroll` everywhere except one place: css-overflow-3 §3 pairs a
  `visible` axis against a scrollable one by computing the `visible` one to
  `auto`. Without `auto`, stylo's `to_scrollable()` pairs it into `hidden`
  instead. **Consequence to know:** given `overflow-x: visible; overflow-y:
  scroll`, a horizontal overflow that a browser would let the user drag is
  clipped here instead. The vertical scrolling authors actually wanted is
  unaffected, and no axis becomes spuriously draggable.
- **`box-sizing` default** — Lynx defaults to `border-box`; CSS defaults to
  `content-box`. **Decision: match Lynx's default**, same reasoning as
  `overflow` above.
- **`display`** — carries only Lynx's internal child-layout-mode switch
  (flex/linear/grid/relative/none), not CSS's external inline/block
  dichotomy. Model our own `display` computation to expose both: the
  Lynx-compatible internal mode apps' CSS actually selects, plus (opt-in)
  real external display type if we ever need it.
- **`display: linear` / `display: relative` and their `linear-*`/`relative-*`
  properties** — genuine Lynx-only layout algorithms (Android
  `LinearLayout`/`RelativeLayout`-derived) with no CSS equivalent at all;
  not a spec violation, just extensions to implement faithfully as their
  own algorithms (not flex polyfills, unlike how `web-core` does it).
- **Relative logical/physical precedence (temporary implementation
  deviation)** — Starlight remaps logical relative-property IDs to a physical
  slot during style application, so a logical and physical declaration that
  target the same side are last-applied-wins. The stylo fork currently models
  the four logical variants as independent longhands, which discards that
  cross-property source order before `hughie` lowers them. The current
  bridge consequently uses physical-wins-unless-sentinel. Direction comes
  from the item itself and the LTR/RTL mapping is otherwise exact. Exact
  bucket-2 parity requires changing the fork declarations into a true
  `logical = true` group; until then this tie-break is an explicitly recorded
  limitation, not a desired semantic.
- **`position: fixed` containing block** — in every mode Lynx supports, a
  fixed element's containing block is unconditionally the single page-root
  element (reached via render-tree reparenting in the legacy path, or a
  dedicated root pointer + root-only measurement pass in the newer
  `enable-fixed-new`/`enable-unify-fixed-behavior` paths), with **no
  exception for ancestors with `transform`/`filter`/`perspective`/
  `will-change`/`contain`** (confirmed absent — no `transform` reference
  exists anywhere in `core/renderer/starlight/layout/`, and Lynx has no
  `contain` property at all) and no component-boundary-scoped containing
  block either. **Decision: implement the real CSS algorithm** — viewport
  by default, re-anchored to the nearest qualifying ancestor when one
  exists — not Lynx's unconditional escape-to-root. (Scroll-offset
  exclusion, on the other hand, is achieved structurally — the fixed
  element's native view is simply never mounted inside any scrollable
  ancestor — and that part already matches the *observable* CSS behavior of
  staying put while scrolling, so no decision is needed there.)

## CSS visual/paint & animation (see [css-visual.md](css-visual.md), [css-animation.md](css-animation.md))

- **`background` shorthand, multi-layer** — Lynx has an acknowledged code
  comment ("different from the web"): if a background layer has no image,
  Lynx silently skips updating that layer's other sub-properties instead of
  treating each layer independently per spec. **Decision: implement full
  per-layer independence** per spec rather than the acknowledged bug.
- **`filter`** — standard CSS accepts a space-separated chain of any number
  of filter functions (`blur(2px) grayscale(50%)`); Lynx's parser hard-fails
  after the first function. Implement the real chained grammar.
- **`background-clip: border-area`** — a genuine Lynx-only value with no CSS
  equivalent (distinct from `border-box`); needs its own behavioral
  spec-mining rather than mapping to any standard box.
- **`ease` timing-function keyword** — Lynx maps `ease` to the *same* curve
  as `ease-in-out` (`cubic-bezier(0.42,0,0.58,1.0)`); confirmed deliberate
  (asserted in a unit test), not an oversight, and the spec-correct curve
  (`cubic-bezier(0.25,0.1,0.25,1.0)`) exists in the codebase but is
  unreachable. **Decision: implement the real, distinct `ease` curve.**
- **`transition-property`** — Lynx resolves each token against a closed
  internal property enum (plus non-standard `scaleX`/`scaleY`/`scaleXY`
  pseudo-properties) instead of accepting arbitrary CSS property
  idents/`all` per spec. Implement standard open-ended property-name
  matching.

## Text layout (see [css-text.md](css-text.md))

- **`text-align: justify` does not parse, and the declaration is dropped
  without a trace** — the vendored fork excludes `Justify` from
  `TextAlignKeyword` under its `lynx` feature
  (`vendor/stylo/style/values/specified/text.rs`, `#[cfg(not(feature =
  "lynx"))]`), so the value is invalid at parse time and CSS drops the whole
  declaration. Both worlds it is measured against accept it: native Lynx's own
  parser maps the keyword to `TextAlignType::kJustify`
  (`lynx/core/renderer/css/parser/enum_handler.cc`), and the web target hands
  `text-align` straight to the browser, which really does justify. The failure
  mode is quiet and it is *not* a fallback to `start`: an invalid declaration
  leaves whatever `text-align` already cascaded or inherited in place, so a
  `text` inside a `text-align: center` subtree stays centered instead of
  becoming justified — no console message, no computed-value difference to
  notice. Recorded, not fixed. Nothing below the parser is missing: Parley
  already implements `Alignment::Justify` (inter-cluster expansion on every
  line but the last), and `hughie`'s `TextAlign` → `Alignment` map is a `match`
  with one arm to add. The whole cost is a fork patch un-gating the keyword,
  which is a change to the vendored submodule and its own review, so it is
  filed here rather than smuggled into an engine-side change.
- **Parley 0.11 paragraph base direction** — the public Parley builders and
  `Layout` expose no base-direction override. Internally, text analysis always
  invokes bidi resolution with an automatic/first-strong base level, even
  though the lower-level `parlance` crate defines a `BaseDirection` value.
  Consequently, the `hughie` text adapter can resolve physical
  `text-align: start`/`end` from `CoreStyle::direction()`, but cannot yet force
  UAX #9 paragraph ordering for neutral or opposite-strong text inside an
  explicitly LTR/RTL container. Injecting bidi controls would corrupt source
  byte ranges, so the current implementation retains Parley's automatic bidi
  shaping and records this as an integration limitation. Revisit when Parley
  exposes its base-level input (or if a narrowly-scoped upstream patch is
  adopted).

## Event model & gestures (see [dom-events.md](dom-events.md))

- **`bind`/`catch`/`capture-bind`/`capture-catch`/`global-bindEvent`** — phase
  and stop-propagation behavior are baked into which attribute name authored
  a handler, not a runtime `{capture}` flag on a shared listener API as in
  DOM, and there's no way to register more than one handler with independent
  behavior at the same node/phase. **Decision: implement real DOM Level 3
  dispatch** (single capture+bubble walk, explicit `capture: bool` per
  listener) and translate `catch*`/`capture-catch` to "register + implicit
  `stopPropagation()`" — this reproduces Lynx's exact observable behavior
  while still being real `addEventListener` underneath.
- **No `preventDefault()`/`cancelable`** — Lynx's touch/tap/longpress events
  cannot be canceled by app code; built-in behavior suppression (e.g. scroll
  panning) goes through a separate gesture-arbitration API instead.
  **Decision (2026-08-17, superseding an earlier one): do not expose
  `preventDefault()` at all.** It was implemented on 2026-08-16 and removed
  the next day. Nothing on the dispatch path can produce a cancel — the
  Element PAPI cannot, because the worklet runtime has neither the method nor
  a bit for it — so the earlier "expose it for web-platform-code
  compatibility" decision bought a boundary with no producer. Recognizers stay
  keyed off the gesture arbitration API, which is also what will drive the
  one surviving suppression seam, `InputEvent::default_prevented`.
- **`tap` suppressed by `longpress`** — Lynx hardcodes "tap doesn't fire if
  longpress was consumed earlier in the same touch sequence," a global rule
  with no DOM equivalent. **Decision: derive this from gesture-recognizer
  arbitration** (a `waitFor` relationship between built-in `Tap`/`LongPress`
  recognizers) instead of a hardcoded cross-event rule, so apps that don't
  use the gesture API don't inherit an invisible suppression.
- **`tap`/`longpress` synthesis semantics (user ruling 2026-08-21,
  implemented in `crates/bobcat-core/src/gesture.rs`)** — `tap` fires at
  release with the native movement-cancel rule; `longpress` fires at a press
  held past 500ms (the native default on every platform). The recorded
  deviations inside that ruling:
  - **Slop shape is radial, 50px.** Lynx's Android implementation compares
    per-axis and its own source comments name the radial comparison as the
    intended behavior (iOS ships radial at 45pt); we implement the intent,
    radially, at the Android/Harmony default of 50. The page-config knobs
    (`tapSlop`, `longPressDuration`) are not wired yet — every view uses the
    defaults.
  - **The long-press "consumed" gate is name-level, not per-chain.** Lynx
    suppresses `tap` when the `longpress` walk found a handler anywhere in
    the response chain; this engine's presenting side only knows which names
    have listeners somewhere in the document (its own lock-free replica of
    the main thread's index), so a `longpress` listener
    on an unrelated element also suppresses a sequence's `tap`. Per-chain
    precision arrives with a presenting-side per-node index.
  - **The gate is one routing pass stale, and loses rather than replays
    (2026-08-31).** The replica is resynced at the top of each routing or
    gesture pass and then left alone, so a registration the main thread has
    not yet published — or published after this pass drained — is invisible
    until the next pass: an event of a freshly registered name is filtered
    out rather than queued, and a `longpress` registration racing its own
    deadline may land on either side of it. Accepted in exchange for a
    presenting side that takes no lock on the pointer path.
  - **Single-finger only, all pointer kinds.** A second concurrent pointer
    cancels synthesis for the whole overlap (Lynx's single-finger `tap`
    gate, with `enableMultiTouch` unimplemented); mouse and pen sequences
    synthesize like touch, because this engine's embedders feed all three
    through one seam and the web target's `tap` is the browser `click`,
    which mice produce.
  - **The web target's `tap` is the browser `click` renamed** (no slop rule,
    no `longpress` at all, no gesture API — `web-core` limitations, not Lynx
    semantics). This engine implements the native semantics; matching
    web-core's degradations was rejected.
  - **`click` is not synthesized** (Android and the fragment path emit it
    beside `tap` with re-targeting at release); no consumer yet.
- **`pointer-events: none` hit-test fall-through** — Lynx falls through to
  the *next sibling* under the point; W3C says the element (and normally its
  subtree) becomes fully transparent to hit-testing, continuing the search
  as if it weren't in the tree at all. These can produce different hit
  targets in overlapping-sibling layouts — needs an explicit implementation
  choice.
- **Nested-scroll coordination** (`scroll-forward-mode`/`scroll-backward-mode`,
  capture-phase scroll interception, fling handoff ordering) has no W3C
  equivalent; the nearest partial analog is `overscroll-behavior`, which only
  controls *whether* scroll chains past a boundary, not consumption order or
  rubber-band restore semantics. Web-core today just relies on uncontrolled
  browser default scroll-chaining — native Lynx's actual coordinated model is
  the real spec to match.

## CSS selectors, cascade & at-rules (see [css-selectors-cascade.md](css-selectors-cascade.md), [css-at-rules.md](css-at-rules.md))

- **Deprecated CSS properties/values are dropped, not implemented** — the
  lynxjs.org API index is the authority (user decision, 2026-07): the style
  engine implements only non-deprecated properties and values. Concretely:
  `linear-orientation`, `linear-gravity`, `linear-layout-gravity`,
  `linear-cross-gravity` (modern API = standard
  `justify-content`/`align-items`/`align-self`), `grid-column-span`/
  `grid-row-span`, and `linear-direction`'s legacy
  `vertical`/`horizontal(-reverse)` value spellings do not parse. Note that
  web-core today still *accepts* these from old bundles — bundles authored
  against deprecated APIs will lose those declarations here. Intentional.
- **CSS inheritance** — native Lynx gates *all* property inheritance behind
  `enableCSSInheritance` (default **off**, allowlist when on;
  [css-text.md](css-text.md) recommends replicating that gate). But the
  compat target `web-core` **ignores that flag entirely** (zero references
  in `lynx-stack/packages/web-platform/`): it runs on the browser's
  always-on W3C inheritance and reproduces the visible Lynx behavior with
  targeted UA-sheet resets (`x-text { color: initial }` +
  `x-text > x-text { color: inherit }` in `web-elements`).
  **Decision (user-confirmed, 2026-07): web-core parity** — lynx-vello uses
  stylo's standard always-on inheritance plus the replicated UA resets. The
  native-Lynx gate is documented as a config-gated fallback design
  (doctored parent `ComputedValues`, allowlist mask) but not implemented.
  Custom properties inherit unconditionally in both worlds.
  **Landed 2026-08-21** in `bobcat_core::tree`'s UA sheet, which had carried
  the decision unimplemented until then — until it landed an ancestor `view`'s
  `color` really did reach the text under it. The port is `color: initial` on
  `text`, `color: inherit` on `text > text` and `text > wrapper > text`, and
  the `text > *` child suppression the same block relies on. Two consequences
  worth knowing: the reset stops a Lynx **text gradient** at the text root
  exactly as it stops a solid color (the fork's `color` holds either, and
  `inherit` carries the gradient into a nested run intact); and a `view` or
  `image` that opts back out of the suppression becomes a plain flex box,
  where web-elements can hand it `inline-flex`/`contents` inside a real inline
  formatting context — the same no-IFC substitution already recorded for a
  nested `text`.
  A tag this engine has no rule for (anything but `wrapper`, `view`, `image`,
  `text`, `raw-text`) generates no box inside a `text`, which is the literal
  `x-text > * { display: none }` behavior and not a narrowing of it.

- **`:is()`, `:where()`, `:has()`, `:nth-child()` family, `:first-child`/`:last-child`/`:only-child`/`:empty`**
  — all are *parsed* by Lynx's selector grammar but have **no matcher case at
  runtime** (always false). This is a bigger gap than a values mismatch: the
  grammar exists, the semantics don't. `:where()` in particular is load-bearing
  — it's the exact mechanism `web-core` uses for CSS-module scoping — so
  lynx-vello needs it working natively regardless of ReactLynx author usage.
  **Decision (user-confirmed, 2026-07-11): full stylo matching** — everything
  that parses, matches per spec; the native matcher's gaps are not
  replicated. See [docs/style-assumptions.md](../style-assumptions.md) §A.3.
- **`::before`/`::after` + `content`** — entirely unimplemented in the native
  engine (no box generation, no `content` property at all); only works on the
  web target via passthrough to the real browser. lynx-vello must explicitly
  decide whether to add real support (W3C-correct) since there's no native
  Lynx behavior to fall back to.
  **Decision (user-confirmed, 2026-07-11): omit in v1** — native-Lynx
  fidelity; selectors parse but generate no boxes and `content` stays inert.
  This is an intentional divergence from the web target; revisit on fixture
  demand. See [docs/style-assumptions.md](../style-assumptions.md) §A.4.
- **`@media` queries** — the C++ engine has a complete, recently-added,
  spec-modeled evaluator, but it is **wired only into the native `.lynx.bundle`
  pipeline**. The `.web.bundle` wire format lynx-vello actually decodes has
  **no `@media` representation at all** (confirmed: upstream `RuleType` and
  our decoder's `RuleKind` each have exactly three corresponding variants in
  `lynx-stack`'s encoder and our decoder). Any `@media` block in ReactLynx-for-web
  source is dropped today. This is the single biggest at-rule gap: decide
  whether to (a) match today's web-bundle behavior (media queries never
  apply) or (b) extend the format, using the C++ engine's evaluator model as
  the behavioral reference either way.
  **Decision (user-confirmed, 2026-07-11): (b) — wire stylo's `@media`
  evaluation now**, ahead of the wire format, with the C++ NG evaluator as
  the behavioral reference; the format extension itself stays future work.
  See [docs/style-assumptions.md](../style-assumptions.md) §C.10.
- **`@supports`** — parser and evaluator exist in the C++ engine but have
  **zero production call sites** (confirmed by grep); `selector()`,
  `font-tech()`, `font-format()`, and `at-rule()` all unconditionally return
  false. Not a quirk so much as a shipped-but-dead subsystem — lynx-vello
  should decide whether to actually wire it up rather than copy the "always
  false" behavior.
- **`@font-face` nested in `@media`/`@supports`/`@layer`** — the native
  engine parses then silently discards these; the web-bundle build tool
  drops them even earlier, with no trace. Neither is spec-compliant CSS
  Cascading & Fonts behavior; pick the real spec as the target since there's
  no working native behavior worth preserving here.
- **A top-level `@font-face` is parsed, mounted, and then completely inert**
  — this one is ours, not Lynx's. The path runs end to end: the decoder
  surfaces `PreparsedRule::FontFace`, `bobcat_core::style::lower` turns it
  into a rule through `Document::build_font_face_rule`, `dom` parses the
  descriptor block with stylo's own `parse_font_face_block`, and
  `append_rules` mounts the result in the stylist. Nothing then reads it.
  Fonts reach Parley by exactly one route — the embedder listing `FontBlob`s
  in the `ViewSources::fonts` it builds a view from, which `hughie`'s
  `TextContext` passes to Parley with no alias, so the families come from
  inside the font file —
  and family matching never consults a font-face rule. So `src` is never
  fetched, a `font-family` descriptor never names anything, and `font-display`,
  `unicode-range`, and the descriptor-level weight/style/stretch ranges have no
  effect at all. A sheet declaring a webfont costs a parse and a rule slot and
  changes no glyph; text renders in whatever the embedder registered, or the
  system fallback. Recorded, not fixed: making it real needs a font *loader* —
  a fetch of the `src` list with format/tech negotiation, a load state machine
  behind `font-display`, and a Parley registration keyed by the descriptor
  family rather than the file's own — which is a resource-pipeline feature,
  not a style-engine one. Until then, treat `@font-face` in a bundle as
  something that will silently do nothing.

## Components (see [components.md](components.md))

- **Almost every built-in component exposes a bespoke imperative JS method
  surface** (`invoke()`-based RPC: `scrollTo`, `getScrollInfo`,
  `setInputFilter`, `startAnimate`, etc.) instead of standard DOM
  properties/methods — this is a systemic pattern, not a per-component
  quirk; see the full table in `components.md` and `js-runtime.md` before
  assuming any one component's API is a one-off.
- **`loadLazyBundle`** intentionally resolves synchronously on the fast path,
  which **violates the Promise/microtask spec** (`.then` must be async) to
  make first-screen render synchronous. This is the reverse of the usual
  policy: matching ReactLynx behavior here means *replicating* a spec
  violation on purpose. Needs an explicit decision, not a default policy
  application.
- **Form constraint-validation** (`checkValidity()`, `required`, `min`/`max`/`step`,
  `ValidityState`) is **absent entirely**, not just non-standard — no
  ReactLynx app depends on it existing. Document as intentionally
  unimplemented rather than a gap to fill.
- **`x-list` virtualization** is CSS `content-visibility: auto` +
  `contentvisibilityautostatechange` on the web target (not true cell
  recycling); native Lynx has three independently-implemented list backends
  with overlapping method names. No single "the" behavior to copy — treat
  native Lynx's virtualized-position-map model as the reference since that's
  what `list.scrollToPosition`/`getVisibleCells` actually need.
- **`scroll-view` and `list` take their display mode from
  `defaultDisplayLinear` here; web-core gives neither of them that**, for two
  different reasons. `scroll-view` is pinned linear:
  `scroll-view { --lynx-display: linear !important }` in `scroll-view.css`,
  which the page-config sheet's `[lynx-default-display-linear=false] *` cannot
  outrank, so on the current container-query path a page built with the flag
  off still gets a linear scroller in a browser. `x-list` is the opposite — it
  is in `linear.css`'s common block but in neither the list that enables the
  `--lynx-display-toggle` nor the one that swaps
  `flex-direction`/`flex-wrap`/`justify-content` by mode, and `x-list.css`
  writes `display: flex; flex-direction: column` outright, so a list is a flex
  container in a browser whichever way the flag is set. **Decision (user,
  2026-08-21): the UA sheet treats a scroller's display like a view's** — one
  rule, one switch, and no per-tag exception. That is deliberate, and the
  alternative is not a different UA rule: `!important` is exactly what
  [docs/style-assumptions.md](../style-assumptions.md) §D.15 forbids in this
  sheet (web-elements' defaults are author origin, ours are user-agent origin,
  and an important UA declaration would outrank author `!important` instead of
  losing to it), so §D.15 hands forced modes to the layout engine's element
  policy — which is the piece not yet built, and what this row records. It
  shows only with `defaultDisplayLinear` off for `scroll-view` and only with it
  on for `list`, and with the main axis pinned the same way in both modes what
  is left to differ is how a child is sized —
  `linear-weight`/`linear-weight-sum` against
  `flex-grow`/`flex-shrink`/`flex-basis` — and that a linear container never
  wraps.
- **A `scroll-view`/`list` is a scroll container and an axis, and nothing
  else** — the UA sheet gives both tags the container block (border box,
  configured display), the scrolling axis with its clipped cross axis, the
  matching main axis in both display modes, and `enable-scroll="false"`. The
  rest of what `scroll-view.css` and `x-list.css` carry is absent: fading
  edges, scrollbar visibility (`enable-scrollbar`/`scroll-bar-enable`), scroll
  snapping (`item-snap`/`paging-enabled`), `sticky-top`/`sticky-bottom`,
  the threshold-observer parts behind `scrolltoupper`/`scrolltolower`,
  `list-type="flow"`/`"waterfall"`, and `list-item` itself — which has no rule
  at all, so a cell is an ordinary flex box rather than the
  `content-visibility`-virtualized, `contain: layout paint` one web-elements
  builds. Several of those are shadow-part or virtualization machinery that a
  UA sheet cannot express alone; they belong with the component work, not with
  the tag defaults.

## JS runtime & APIs (see [js-runtime.md](js-runtime.md), [accessibility.md](accessibility.md))

- **`lynx.createSelectorQuery()`/`NodesRef`** — modeled on WeChat Mini
  Program's batched `SelectorQuery`, not DOM `querySelector`: async,
  explicit `.exec()`, callback-based `invoke()`. This shape (async/batched
  query, not live synchronous DOM references) recurs across most of the
  JS-facing element API — treat it as the systemic pattern, not a one-off.
- **`requestAnimationFrame`/timers** — signature-compatible with the W3C
  APIs, but callback timing is tied to Lynx's own frame/vsync pump (paused in
  background, no guaranteed cadence). Implement by driving our own frame pump
  rather than assuming browser rAF semantics transfer directly.
- **`lynx.SystemInfo` / `SystemInfo.pixelWidth`/`pixelHeight`** — no direct
  W3C equivalent (closest: `devicePixelRatio`/`screen.width`); Lynx exposes
  it as a static global snapshot rather than a live queryable API, and the
  Android implementation is documented as a "shared, polluted, process-global"
  value not maintained per-view — worth a cleaner per-view accessor in
  lynx-vello rather than copying that specific bug.
- **Accessibility**: Lynx has **no implicit ARIA-like semantic
  roles/focusability** (nothing is focusable/announced unless explicitly
  opted in via `accessibility-element`), and `accessibility-traits` is a flat
  non-standard vocabulary mixing ARIA roles and states in one list, not the
  ARIA role/state separation. The legacy Android virtual a11y tree is also
  reconstructed from flattened hit-testing rather than DOM reading order.
  These are the systemic AT-model differences to design around, not
  individual prop quirks.

*(Full per-row detail, including ~250 more narrowly-scoped items — mostly
Lynx-only extension properties/APIs with no W3C equivalent to conflict with —
lives in the `W3C-compliant?` column of each linked file.)*
