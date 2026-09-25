# CSS animation, transition & JS animate() API

> Research: multi-agent sweep over `lynx/` and `lynx-stack/` (see [AGENTS.md](../../AGENTS.md) for the reference-repo shorthand and the W3C-first standards policy). Supersedes the earlier stub.

### CSS Animations, Transitions & JS Animation API

Scope covered: `animation-*`/`transition-*` CSS longhands and shorthands, `@keyframes` parsing, timing-function keywords/curves, animation/transition events, and the JS-facing `NativeElement.animate()` imperative API. Source of truth is `lynx/core/renderer/css/parser/*`, `lynx/core/renderer/css/css_style_utils.cc`, `lynx/core/style/timing_function_data.{h,cc}`, `lynx/gfx/animation/*` (curve math and event names), `lynx/core/animation/*` (CSSKeyframeManager / CSSTransitionManager, the runtime drivers), and `lynx/core/renderer/dom/element.cc` + `lynx/core/runtime/js/bindings/java_script_element.cc` (the JS API surface). I could not find any spring-physics timing function anywhere in this pipeline — `lynx/clay/gfx/animation/bounce_animator.h` implements spring/bounce physics, but `clay/` is a **wholly separate** Flutter-engine-based rendering runtime (see `lynx/clay/README.md`) with zero references from `core/renderer/css` or `core/animation`; it is not reachable from `.web.bundle`/ReactLynx and is out of scope. The one genuinely non-standard timing keyword is `square-bezier(x, y)`, a Lynx-only single-control-point curve.

#### Animation & transition properties

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `animation` (shorthand) | `[name, duration, delay, timing, count, direction, fill-mode, play-state]`, comma-separated for multiple animations | Core | Yes | | lynx/core/renderer/css/parser/animation_shorthand_handler.cc |
| `animation-name` | keyframes reference, ident | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc (`AnimationNameValue`) |
| `animation-duration` | time value | Core | Yes | | lynx/core/renderer/css/parser/animation_shorthand_handler.cc |
| `animation-delay` | time value, negative allowed | Core | Yes | | lynx/core/renderer/css/parser/animation_shorthand_handler.cc |
| `animation-timing-function` | keyword/cubic-bezier()/steps() | Core | Partial | see timing-function table below | lynx/core/renderer/css/parser/timing_function_handler.cc |
| `animation-iteration-count` | number or `infinite` | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:4060-4087 |
| `animation-direction` | `normal\|reverse\|alternate\|alternate-reverse` | Core | Yes | | lynx/core/renderer/starlight/style/css_type.h:332-337 |
| `animation-fill-mode` | `none\|forwards\|backwards\|both` | Core | Yes | | lynx/core/renderer/starlight/style/css_type.h:339-344 |
| `animation-play-state` | `running\|paused` | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:4089-4095 |
| `@keyframes` selectors | `from`/`to`/`N%`, clamped to [0,1] | Core | Yes | | lynx/core/renderer/css/css_keyframes_token.h:64-81 |
| `transition` (shorthand) | `[property, duration, delay, timing]`, comma-separated | Core | Partial | see `transition-property` row | lynx/core/renderer/css/parser/transition_shorthand_handler.cc |
| `transition-property` | Lynx uses a closed enum (`AnimationPropertyType`), not free-form idents | Core | No | Spec allows any CSS property name (incl. custom idents/`all`, undefined ones are just no-ops); Lynx's parser instead resolves each token against a **fixed** internal property enum (`ALL_ANIMATABLE_PROPERTY_ID` in `css_property.h:47-65`) plus Lynx-only pseudo-properties `scaleX`/`scaleY`/`scaleXY` (single-axis transform animation) that are not real CSS properties at all. Rewrite should accept standard longhand names + `all`/`none`, and drop scaleX/Y/XY as first-class transition targets (model them as `transform` axis animations instead, or keep as a compat shim). | lynx-stack/packages/repl/src/generated/lynx-types-map.json (transitionProperty union); lynx/core/renderer/css/parser/css_string_parser.cc (CSSTransitionLayer/`Transition()`) |
| `transition-duration` / `transition-delay` / `transition-timing-function` | one entry per `transition-property` slot, cycles if fewer entries (per spec) | Core | Yes | | lynx/core/renderer/css/parser/transition_shorthand_handler.cc |
| `animationstart`/`animationend`/`animationcancel`/`animationiteration` | keyframe animation lifecycle events | Core | Partial | Event names match spec (`AnimationEvent`) exactly, but the `detail` payload is a custom dict `{new_animator, animation_type, animation_name}` — missing standard `AnimationEvent` fields `animationName`(dup, ok)/`elapsedTime`/`pseudoElement`. Rewrite should populate `elapsedTime` at minimum since ReactLynx app code may read it. | lynx/core/animation/constants.h:13,15,17,19-20; lynx/core/animation/animation.cc:131-150,384-404 |
| `transitionstart`/`transitionend`/`transitioncancel` | CSS transition lifecycle events | Core | Partial | Same custom-dict issue as above; standard `TransitionEvent` also has `propertyName` (which CSS property transitioned) — not present in the dict at all. Rewrite should add `propertyName` and `elapsedTime`. | lynx/core/animation/constants.h:14,16,18; lynx/core/animation/animation.cc:131-150,384-399 |
| Animatable property set | opacity, transform, background-color, color, width/height/min/max, left/top/right/bottom, margin/padding/border-width/border-color (all 4 sides), flex-grow, flex-basis, filter, transform-origin, offset-distance, background-position, visibility | Core | Yes (all are standard animatable CSS properties) | | lynx/core/renderer/css/css_property.h:47-104 (`FOREACH_NEW_ANIMATOR_PROPERTY`, `ALL_ANIMATABLE_PROPERTY_ID`) |
| `enter-transition-name` / `exit-transition-name` / `pause-transition-name` / `resume-transition-name` | Lynx-specific `<list>`-item transition-name hooks (no CSS/DOM standard equivalent — closest analog is the still-experimental View Transitions API `view-transition-name`) | Extended | No | Not a W3C concept; scope this to the `<list>` component only if/when lynx-vello implements `<list>` recycling; otherwise skip. | lynx/core/renderer/css/parser/animation_shorthand_handler.cc:76-79; lynx/core/renderer/css/computed_css_style.cc:2586-2611,4213-4239 |
| `layout-animation-create/update/delete-duration/delay/timing-function/property` | Lynx-specific `<list>` item insert/update/delete animation properties; `property` value restricted to `opacity\|scaleX\|scaleY\|scaleXY` | Rare | No (no W3C equivalent) | Native `<list>`-only feature (Android/iOS list adapter diffing); out of scope unless lynx-vello implements a native recycler-backed `<list>`. | lynx/core/renderer/ui_component/list/list_container_animation_manager.h; lynx-stack/packages/repl/src/generated/lynx-types-map.json (layoutAnimationCreateProperty union) |
| `implicit-animation` | Parsed but explicitly a no-op placeholder — deprecated | Rare | N/A (dead property) | Skip entirely; upstream already treats it as inert. | lynx/core/renderer/css/computed_css_style.cc:1787-1793 |
| `x-animation-color-interpolation` | Vendor-prefixed: `auto\|sRGB\|linearRGB` — controls color space used when interpolating color animations | Extended | No (non-standard property name/prefix) | CSS Color 4 has an analogous concept via `color-interpolation-method` inside `@keyframes`/gradients (`in oklab`, `in srgb`, etc.) but no such vendor property exists standardly. If matching Lynx behavior, add an internal (non-CSS-exposed) linear-vs-sRGB interpolation toggle for color animation curves; do not expose as a real CSS property in the standards-compliant path. | lynx/core/renderer/css/parser/enum_handler.cc:494-509,772-774,843 |
| `animation-composition`, `animation-timeline` (scroll-driven), `linear()` easing | CSS Animations Level 2 / Scroll-driven Animations features | — | Not implemented in Lynx | Confirmed absent from the entire `core/renderer/css` + `core/animation` tree (grepped, zero hits). Not a "deviation" per se — just an unimplemented-in-Lynx feature; lynx-vello should treat these as net-new asks, not compatibility work. lynx-vello implements scroll-driven animations as a W3C extension (see *Scroll-driven animations* below); `animation-composition` and `linear()` stay absent. | (absence confirmed via grep across lynx/core/renderer/css, lynx/core/animation) |

#### Timing-function keywords / curves

| Item | Description | Tier | W3C-compliant? | Deviation & what we should do instead | Source refs |
|---|---|---|---|---|---|
| `linear` | identity curve `t -> t` | Core | Yes | | lynx/gfx/animation/timing_function.cc:150-161 |
| `ease-in` | `cubic-bezier(0.42, 0, 1.0, 1.0)` | Core | Yes | | lynx/gfx/animation/timing_function.cc:16-34 |
| `ease-out` | `cubic-bezier(0.0, 0.0, 0.58, 1.0)` | Core | Yes | | lynx/gfx/animation/timing_function.cc:16-34 |
| `ease-in-out` | `cubic-bezier(0.42, 0, 0.58, 1.0)` | Core | Yes | | lynx/gfx/animation/timing_function.cc:16-34 |
| `ease` | Mapped to the **same** `TimingFunctionType::kEaseInEaseOut` as `ease-in-out`, i.e. resolves to `cubic-bezier(0.42, 0, 0.58, 1.0)` | Core | **No** | Spec `ease` = `cubic-bezier(0.25, 0.1, 0.25, 1.0)`, a materially different (gentler start, no pause point) curve from `ease-in-out`. Confirmed deliberate, not an oversight: `timing_function_handler_unittest.cc:24` asserts `{"ease", kEaseInEaseOut}` as the expected mapping, and the correct `EaseType::EASE` preset (`0.25,0.1,0.25,1.0`) exists in `timing_function.cc:19-21` but is unreachable — no `TimingFunctionType::kEase` enum value exists to select it. **lynx-vello should implement the W3C-correct distinct `ease` curve** (`cubic-bezier(0.25,0.1,0.25,1.0)`), not replicate this Lynx quirk. | lynx/core/renderer/css/parser/css_string_parser.cc:2739-2742 (`TokenToTimingFunctionType`); lynx/gfx/animation/timing_function.cc:16-34,162-191; lynx/core/renderer/css/parser/timing_function_handler_unittest.cc:23-25 |
| `ease-in-ease-out` | Lynx-only alias, identical to `ease-in-out`/`ease` | Rare | No (non-standard keyword) | Extra alias, harmless; only implement if fixture bundles actually use it. | lynx/core/renderer/css/css_keywords.tmpl:316 |
| `cubic-bezier(x1, y1, x2, y2)` | arbitrary 2-point cubic bezier, x-coords implicitly clamped by solver | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:1916-1933; lynx/gfx/animation/cubic_bezier.cc |
| `steps(n, position)` | `position` ∈ `start\|end\|jump-start\|jump-end\|jump-both\|jump-none` | Core | Yes (matches CSS Easing Level 2 `<step-position>`) | | lynx/core/renderer/css/parser/css_string_parser.cc:1934-1962 |
| `step-start` / `step-end` | shorthand for `steps(1, start)` / `steps(1, end)` | Core | Yes | | lynx/core/renderer/css/parser/css_string_parser.cc:1878-1887 |
| `square-bezier(x, y)` | **Lynx-only, non-standard.** Single-control-point curve; internally rewritten to a standard cubic-bezier via `(x·⅔, y·⅔, 1+(x−1)·⅔, 1+(y−1)·⅔)` before evaluation — i.e. it's really just sugar for a symmetric cubic-bezier, not a distinct math model. | Extended | No (not a CSS keyword/function at all) | Not W3C. Two options: (a) skip — parse/reject at the CSS layer and require authors to use plain `cubic-bezier()`; or (b) support for bundle compat by expanding `square-bezier(x,y)` to the equivalent `cubic-bezier(...)` at parse time (same math as Lynx) so existing `.web.bundle` styles render identically. Prefer (b) for ReactLynx-authored-content compatibility since it's a pure algebraic rewrite with no behavioral surprise beyond the keyword name. | lynx/core/renderer/css/css_keywords.tmpl:321; lynx/core/renderer/css/parser/css_string_parser.cc:1904-1915; lynx/gfx/animation/timing_function.cc:42-48 (`CreateSquareBezier`); lynx/core/renderer/css/css_style_utils.cc:1111-1112 |
| Spring/physics-based easing | **Not present** in the CSS/ReactLynx animation pipeline | — | N/A | `lynx/clay/gfx/animation/bounce_animator.h` (spring/bounce physics, comment cites `SpringTimingParameters.swift`) exists only in `clay/`, a separate Flutter-based rendering runtime unrelated to `.web.bundle`/`core/renderer/css` (verified zero cross-references). Do not port; not reachable from the format lynx-vello targets. | lynx/clay/README.md:1-19; lynx/clay/gfx/animation/bounce_animator.h; grep confirms no `core/` reference to `clay/gfx/animation` |

#### JS-driven animation API surface (`element.animate()`)

Lynx exposes a single low-level **imperative** animation entry point on native element handles, `NativeElement.animate(operation, name, keyframesObj, optionsObj)`, implemented as a JSI host function (`lynx/core/runtime/js/bindings/java_script_element.cc:16-106`) and routed down through `TemplateAssembler::ElementAnimate`/`ElementAnimateV2` (`lynx/core/renderer/template_assembler.cc:1783-1817`) to `Element::Animate`/`Element::AnimateV2` (`lynx/core/renderer/dom/element.cc:1153-1369`). This is **not** the standard DOM `Element.animate()` (Web Animations API) signature — there is no `KeyframeEffect`/`Animation` object returned to JS, no promise (`animation.finished`), and no `Animation` handle with `.playState`/`.currentTime` getters. It is a fire-and-forget imperative call keyed by an animation name string.

- **Operations** (`AnimationOperation` enum, `lynx/core/runtime/js/bindings/js_app.h:62`): `START=0`, `PLAY=1`, `PAUSE=2`, `CANCEL=3`, `FINISH=4`.
  - `START`: requires exactly 4 args — `(0, jsName: string, keyframes: object, options: object)`. Internally synthesizes (or reuses, if `options.name` is set) a `@keyframes`-equivalent token from the `keyframes` object via `starlight::CSSStyleUtils::UpdateCSSKeyframes`, then applies the `options` table as `animation-*` longhands.
  - `PLAY`/`PAUSE`: set `animation-play-state` to `running`/`paused` respectively (2 args: `(op, name)`).
  - `CANCEL`: resets all `animation-*` longhands to initial values (full reset list at `element.cc:1274-1279`).
  - `FINISH`: only updates internal imperative-animation bookkeeping (no direct style reset observed) — jumps the animation to its end state via the normal animation-manager fill-mode logic.
- **Options object keys** (mapped JS→CSS via `CSSProperty::GetTimingOptionsPropertyID`, `lynx/core/renderer/css/css_property.h:38-45`): `duration`, `delay`, `iterations` (accepts `Infinity` → serialized as `"infinite"`), `fill`, `easing`, `direction`, `play-state`. These names closely mirror the WAAPI `KeyframeAnimationOptions` dictionary (`duration`/`delay`/`iterations`/`easing`/`direction`) plus two Lynx-only additions (`fill` is actually WAAPI-standard too; `play-state` is **not** a WAAPI constructor option — WAAPI controls play state via `.play()`/`.pause()` methods on the returned `Animation`, not an options key).
- **Two API generations**: `Source::kAnimate` (legacy `NativeElement.animate`) vs `Source::kAnimateV2` (`lynx/core/renderer/dom/imperative_animation_state.h:27-30`); `AnimateV2` is gated behind `enable_new_animator()` and is the path integrated with `ImperativeAnimationState` bookkeeping for the "new styling pipeline" (fill-mode persistence cleanup, keyframe ownership tracking). Could not confirm from `core/` alone whether a higher-level, promise-returning JS wrapper exists in the Lepus/JS runtime layer or only in app-level JS libraries (`lynx-stack/packages/react/**` has no references to this native `animate` binding at all — ReactLynx's declarative `style={{transition: ...}}`/`@keyframes` path does not go through this imperative API).
- **Events fired**: same `animationstart/end/cancel/iteration` names as declarative CSS animations (both paths funnel through the same `animation::Animation`/`CSSKeyframeManager` runtime, since `Element::Animate` ultimately just writes `animation-*` styles) — see events table above for the payload-shape caveat.
- **Recommendation for lynx-vello**: implement the real W3C `Element.animate()` (Web Animations API) surface — returning a proper `Animation` object with `.play()/.pause()/.cancel()/.finish()/.reverse()`, `.finished` promise, `.playState`, `.currentTime`, and standard `animationstart/end/cancel/iteration`+`finish` events — since that's the ReactLynx-compatibility-relevant behavior surface JS app code would exercise, while still accepting Lynx's `(operation, name, keyframes, options)` wire format from decoded bundles/native bridge calls if any existing ReactLynx internals rely on it (need to check `lynx-stack/packages/react/runtime` animation helpers specifically, not yet found in this pass).

**W3C divergence flags requiring lynx-vello attention:**
1. `ease` keyword resolves to the `ease-in-out` curve instead of its own spec-defined curve — implement the correct distinct curve.
2. `transition-property` is a closed enum (plus non-standard `scaleX`/`scaleY`/`scaleXY` pseudo-properties) rather than accepting arbitrary CSS property idents/`all` per spec — implement standard open-ended property-name matching.
3. `animationend`/`transitionend` (etc.) event detail objects omit spec-mandated fields (`elapsedTime`, `propertyName` for transitions, `pseudoElement`) — populate these for ReactLynx event-handler compatibility.
4. The native `element.animate()` binding is not the Web Animations API — no returned `Animation` handle/promise/`playState`. For "ReactLynx compatibility" purposes (matching *behavior* JS code observes), prefer implementing genuine WAAPI semantics rather than the raw Lynx wire protocol's fire-and-forget shape.
5. `square-bezier()` and `x-animation-color-interpolation` are non-standard; both are cleanly reducible to standard mechanisms (cubic-bezier expansion; internal color-space toggle) if bundle compatibility is required, otherwise safe to drop.

---

## As landed

The driver exists as of the `@keyframes` timeline change. What that means
concretely, so the tables above are read as "the target" and this section as
"the state":

- **Where the state lives.** Stylo's own `DocumentAnimationSet`
  (`vendor/stylo/style/servo/animation.rs`) is the animation state, held by the
  `Document` and handed to every `SharedStyleContext`. Keyframe resolution,
  interpolation, timing functions, direction, fill mode, and the
  `Animations`/`Transitions` cascade origins are all Stylo's; the engine
  contributes the timeline. Animations start and cancel inside the ordinary
  style flush, through `MatchMethods::process_animations`.
- **Where the frame work happens.** `Document::advance_animations(now)` runs on
  the document's owner thread — the Lynx main thread once the script starts —
  once per burst holding the painter's frame post, which carries the
  presenting side's clock reading (posts coalesce in the view's scroll
  mailbox: the latest `now`, the greatest `seq`); no JavaScript is involved.
  For an animation the commit exported as a composite curve (below), a window
  painter posts no per-frame request at all: the compositor samples the curve
  itself, and the main thread hears about the animation again only when a
  finite curve runs out (a frame post per frame from its end until the finish
  restyle's commit is adopted) or something else commits. An offscreen
  `Painter::tick` posts a frame on every call whatever the frame reports, so
  a headless host still ticks the main thread each frame. Every job on the
  main thread first runs `Document::sync_animation_clock` at the painter's
  latest clock: the timeline and the states move (promotion at an anchored
  start, iteration, end), nothing is re-cascaded or committed unless
  something ends, and the next tick re-cascades what moved — so a restyle
  inside the job computes from the current instant. It is a Stylo animation-only traversal, which
  does no selector matching and reads no snapshots, over just the animating
  elements and whatever inherits from them.
  A property that cannot move a box never reaches layout, because the damage
  harvest only invalidates layout for damage that says relayout.
- **When an animation starts.** The flush arms it; the first frame after that
  flush starts it. The painting side stops posting frames for an idle page and
  for an animation an exported curve already covers, and a host that does not
  sync the clock at every job (Bobcat does) hands Stylo a time that can be
  arbitrarily stale — a tap after ten idle seconds would otherwise create an
  animation ten seconds in the past and finish it on its first frame. A job's
  clock sync carries a fresh pending start with the clock rather than
  anchoring it. Web Animations resolves a pending animation's start time at
  the first frame after it was created, so the driver shifts every `Pending`
  animation and transition it has not anchored yet forward by the interval that
  tick advanced the timeline over, once, keeping delays (positive and negative)
  intact.
- **When an animation is frozen** (css-contain-2 §4, landed 2026-09-20). An
  element in a *skipped subtree* — one with a proper flat-tree ancestor whose
  contents `content-visibility: hidden` or a non-relevant `content-visibility:
  auto` skips — does not advance: "Existing animations do not advance in their
  timeline. Running animations on the element do not end." The driver asks
  `crate::layout::skips_contents` up the flat tree once per element that owns
  animation state (a page with none asks nothing) and freezes by applying the
  anchoring arithmetic above to a *running* animation: every start time is
  carried forward by the tick's interval, so progress stands still and nothing
  is promoted, iterated, ended or re-cascaded. The skipping element itself is
  **not** frozen — the spec skips its contents, not the element. Frozen sets
  are excluded from `has_active_animations`, from the frame's
  `animations_active`/`needs_main_ticks`, and therefore from `owes_frame`, so a
  page whose only animations are frozen stops being ticked at all; the reveal
  (a style change, or a relevance flip inside `render`) makes it active again
  in the same commit, and the animation resumes at the first tick after it. The
  one deviation — an animation or transition that *starts* while skipped is
  created and frozen at its start rather than not created, because style is not
  skipped — is recorded in `style-assumptions.md` §19.
- **Measured cost** (Apple silicon, `cargo bench -p dom --bench animation`, a
  120-card page; medians). An idle page pays **6.9 ns** per frame — one bool
  and one epoch compare, so a document that never animates never pays for the
  driver. One `transform` frame costs **254 µs** with a single animating
  element and **642 µs** with all 120; the animation half of that is 5.2 µs and
  380 µs, so the tick is O(animating) at roughly 3.2 µs per element. A `width`
  animation, which does reach layout, costs 24 µs more at one element and 69 µs
  more at 120 — that is the reflow the paint-only path avoids. These `frame_*`
  and `tick_*` arms time the main-thread path. `frame_card_text_*` and
  `frame_shimmer_rows` time the frame the painter runs instead — a main-thread
  tick only when the committed frame asks for one, then the composition — over
  cards holding a `<text>` and over a list of 200 shimmering rows.
- **Composite curve export** (`dom::style::curve_export` +
  `dom::visual::curves`) closes §11's gap: at commit, an element whose
  animation set moves only `opacity`/`transform` is exported whole as an
  `AnimationSlot` curve on the committed frame — a clone of every non-canceled
  stylo `Animation` in the set's order. The painter samples the clones with
  stylo's own code: `Animation::progress_at` and `sample_at`, the two halves
  the fork splits `get_property_declaration_at_time` into (the main thread's
  cascade runs the same sampling half on its own iteration state, which the
  driver's `iterate_to` moves by the arithmetic `progress_at` repeats on a
  copy). Inside the curve's domain the sampled `AnimationValue`s are bit-equal
  to the ones the main thread's cascade commits at the same instant whenever
  both sides iterate from the same animation state; start times the main
  thread accumulates over several ticks can differ from the sampler's single
  step in the last bit for durations that are not binary fractions. Servo's
  deviations come along: later in the set wins, and `maybe_start_animations`'
  `return;` after updating an existing animation (so `animation-name: a`
  restyled to `a, b` never starts `b`). A sampled transform folds by the
  builder's own f32 fold (`visual::transform::ContextMatrix`) onto the
  parent's committed world, so the element's world matches a commit's bit for
  bit relative to that parent world; composed geometry agrees to f32 rounding.
  This exports several animations, pending (anchored, with or without a
  backwards fill), paused and held-fill animations, pending (anchored) and
  running transitions — sampled with `Transition::calculate_value`, inserted
  after the animations so a transition wins its property as the `Transitions`
  origin does, ending the domain at `start_time + duration`; a `transform`
  transition's reach runs from its `from` to its `to` over its timing
  function's eased range (the fork's `PropertyAnimation::{from, to,
  timing_function}`), so it culls and exports inside a composited group like
  a keyframes curve — `steps()`,
  `square-bezier`, `%`/`em`/`rem`/`rpx`/`vw`/`calc()`/`var()` values (already
  px in stylo's computed keyframes, `%` resolved against the border box as the
  commit resolves it), mismatched lists, backfilled `from`/`to` and every Lynx
  transform function. There is no second interpolation. The export is exact
  whenever, per exported property, either nothing contributed at the commit
  instant or every instant inside the domain keeps a contribution: an
  animation starts contributing only when a pending one starts, before which
  it contributed nothing, and stops only at `expires_at`, the hand-back. One
  stylo limitation is shared rather than fixed: a mismatched remainder holding
  a `%` length interpolates as `InterpolateMatrix`, which stylo's matrix
  conversion reads as the identity, on the main thread and in the curve alike.
- **What still refuses** — and keeps the element on per-frame main-thread
  ticks, a refusal never being wrong; a refusal allocates no slot, so every
  slot carries a live curve: an animation or a transition on a property other
  than `opacity`/`transform`; author `!important` on a property only
  animations drive (`get_properties_overriding_animations`: it outranks the
  animations origin, so the main thread holds it still; the transitions
  origin outranks it in turn); a pending animation or transition the driver
  has not anchored yet (its next tick moves its start; anchoring commits a
  frame, so the export follows one tick later — a transition a listener
  starts is anchored by the frame the listener's pass posted, since that
  frame runs after the burst's events); an element frozen when the last tick
  ended (css-contain-2 §4: the next tick carries its start times, and commits
  so the export follows it); a keyframe or transition value some
  interpolation takes out of the plane — a perspective term in its matrix, or
  any 3D function under a parent `perspective` — or a committed world that is
  not 2D invertible; a property contributed at the commit that some offset of
  a scroll timeline would leave with no contribution (the base-value rule under
  "Scroll-driven animations"); pseudo-element sets; and the structural
  refusals below.
- **Where the two sides differ.** Past the curve's domain: it ends at its
  first animation's end (`expires_at`: the first instant a finite animation,
  iterated as the sampler iterates it, has ended), after which an animation's
  contribution can be replaced by the base value. The compositor then holds
  the domain's last instant until the commit of the finish restyle is adopted
  — at least one frame — and the frame asks for main-thread ticks from that
  instant on. And at the tick a filling animation finishes beside a running
  one on the same property: that tick's cascade still lets the later-listed
  running one win, then the driver puts the held one back last in the set, so
  the curve cloned for that commit — and every later commit — shows the held
  value while the commit's own bake shows the running one; the delta carries
  the bake to the held value from the first composed frame. The cascade value
  of an exported animation is stale between main-thread readings
  (`docs/style-assumptions.md` §12).
- **Structure refuses only a curve with a `transform` track, and only where
  its motion cannot be re-expressed or bounded** — and a refusal takes the
  whole curve, its opacity values included, to main-thread ticks: an element
  whose extent, `max(size, content_size)`, exceeds
  `MAX_MOVING_EXTENT_VIEWPORTS` (three viewport areas; Firefox caps composited
  transforms the same way); an enclosing composited group that neither a
  still clip nor the viewport bounds the element in, which only a scale range
  through 0 on the group's side produces (`visual::space::movers_bounded`);
  and an enclosing composited group around a curve without a reach (below).
  Individual transforms, a motion path and the transform origin are factors
  the fold holds still between commits, so they refuse nothing.
  Clips (a `<text>`'s UA `overflow: clip` included), scroll containers (every
  `overflow: hidden` card included), sticky boxes and enclosing groups, inside
  or around the animated subtree, do not refuse it. The committed frame
  records one compose space tree (`visual::space`) of scroll, sticky and
  animation nodes in containing-block order; the curve's delta
  `planar(W(t))·planar(W_c)⁻¹` is its animation node's map, so it applies at
  that node's place on every path through it — fragments, layer pushes,
  clips, image draws, filter bakes and hit tests alike — and composition
  retargets the element's group alpha.
- **Moving content is culled through the curve's reach.** Each exported
  transform track carries, per op, the range its parameters take over the
  whole curve domain (the cubic-bezier control-point hull covers overshoot).
  The viewport pulls back through those ranges, so one encode serves every
  instant: 200 list rows each running a shimmer encode only the rows that can
  meet the list's window. Clips and scroll encode windows inside the moving
  subtree bound as usual, and `content-visibility: auto` relevance follows.
  A scale range reaching 0 bounds nothing; there the extent cap is what bounds
  the encode. The ranges are read off stylo's computed keyframes where stylo
  interpolates op by op: every keyframe's list holds the same translate, scale
  and planar rotate primitives in the same order, a shorter list — `none`
  included — padded with identities as stylo pads it; a segment eases by its
  lower keyframe's timing function running forward and by its upper keyframe's
  running reversed, as stylo eases it; a transition's one segment runs from its
  `from` to its `to`, eased by its own timing function. A scroll timeline
  picks progress in `[0, 1]` as the clock does, so the reach — which reads no
  time — bounds a scroll-driven curve exactly as it bounds the same keyframes on
  the document timeline. It eases in the direction the binding samples, which
  stylo's `return;` deviation can leave apart from the cloned animation's; a
  held progress-driven sample takes both directions. A list holding an op the reach does not
  model (matrix, skew, 3D rotation) or a mismatched remainder stylo decomposes
  has no reach: its pullback admits everything, the extent cap bounds its
  encode, and it does not export inside a composited group. That is the one
  culling coarsening the stylo-sampled curves brought. A composited group
  whose content moves inside it takes its rect from that content carried
  through the same ranges, cut to its clips and to the viewport pulled back
  into the group's space.
- **Per-frame work is bounded by what the program draws.** Composition
  samples only the curves and sticky boxes the compose program references;
  the earliest curve end is a commit-time value. `has_live_curves` is true
  when the program references a curve that reads the document timeline — the
  compositor then recomposes each frame — so a frame whose curves are all
  culled, or all on scroll timelines, composes once per scroll.
  `has_exported_curves` is true when any curve exported: input hit tests then
  sample at the input's instant, since a curve moves its element's hit area
  even where nothing of it is drawn. A `filter: blur()` group whose content
  carries a curve, or whose element's transform curve moves it across an
  ancestor's clip, bakes at the frame's instant; its element's own
  opacity-only curve does not make it, and each filter entry re-bakes on its
  own readings — a scroll among them whenever such an entry samples curves and
  the program composes one on a scroll timeline, whose source need not ride
  the entry's paths.
- **Side effects follow the animation, not its export** (web-animations-1:
  an in-effect `opacity`/`transform` animation acts as `will-change` naming
  it). The driver keeps two node bits, `animates_opacity` and
  `animates_transform`, recomputed in `sync_animation_state`: an animation
  counts while pending (its delay included), running or paused, or finished
  with a `forwards`/`both` fill; a transition while pending or running.
  Either bit makes the element a stacking context; the opacity bit also
  forces its composited group and makes it a Backdrop Root at every reading;
  the transform bit makes it the containing block of its absolute and fixed
  descendants whatever its committed value, and relayouts them only when the
  bit flips, at the animation's start and end. Paint order and containment
  therefore do not change when an animation is refused, delayed, or handed
  between the compositor and the main thread. Native Lynx always gives a
  fixed box the page root; web-core leaves these side effects to the
  browser, and this follows web-core.
- **The scene rebuild was the dominant term.** Before the export, every
  frame carried a constant ~248 µs of scene rebuild: at one animating
  element that is 98% of the frame. An exported animation now rebuilds
  nothing per frame — composition replays the committed fragments with a
  new sampled delta/alpha — and the `frame_*` numbers above apply only to
  the fallback path.
- **Wiring the driver costs non-animating documents nothing**: `initial_commit`,
  `media_viewport_flip`, `var_chain_cascade`, and `noop_commit` in the `css`
  bench all sit within noise of their pre-driver medians. Reaching that required
  guarding every animation-map read behind a per-node bit — Stylo asks
  `has_animations` for every element on every match-and-cascade and twice more
  per candidate through the style-sharing cache, and an unguarded read of the
  one document-wide lock from every Rayon worker cost 3-10x on those benches.
- **Cascade participation follows browsers** (`docs/style-assumptions.md` §11,
  revised 2026-08-20). Every animated property goes through the cascade, at
  Stylo's `Animations`/`Transitions` origins, including `transform`, `opacity`,
  and `filter`. That is what the specs require — CSS Animations 1 §2 adds the
  animated value *to the cascade*, CSS Cascade 5 §6.1 places it between
  important author and normal author declarations — and what all three engines
  do; what they move off the main thread is per-frame interpolation and
  rasterization, and what they throttle is the per-frame restyle, never the
  cascade. For an animation that ticks on the main thread the cascade output
  *is* the animated value, so `Node::computed_style` is correct mid-animation.
  An exported curve is a render-private value: the main thread's cascade value
  for it holds at its last tick or commit until something ticks or commits
  again, so §12's query-time staleness seam is back for exported animations —
  a known gap, not built.
- **Two Stylo behaviors the driver has to correct**, both from the same area and
  both fixed without patching the fork. `process_animations_for_style` retains
  only unfinished animations, which would drop an
  `animation-fill-mode: forwards` value at the next restyle of that element, so
  the driver keeps finished-but-filling animations and puts them back after each
  traversal. And `ElementAnimationSet::update_animations_for_new_style` cancels
  an animation the new style no longer names *without marking the set dirty* —
  unlike the two sibling cancel paths beside it — so `process_animations` never
  replaces the element's `Animations` origin and the element keeps the value the
  animation had when it was cancelled, running or filling, until something else
  happens to restyle it. The driver detects cancelled animations after each
  traversal and re-cascades those elements itself.
- **Known gaps.** No animation events yet
  (`animationstart`/`animationend`/`animationiteration`/`animationcancel`) —
  the driver knows the state transitions, nothing relays them to the realm.
  Transitions are wired (`transition_rule`, `has_css_transitions`) and their
  timeline start is covered by a test, but nothing else about them is.
  `element.animate()` has no producer and is out of scope.

## Scroll-driven animations

scroll-animations-1 over the css-animations-2 `animation-timeline`: bound and
sampled on the main thread, and sampled by the painter from the live scroll
offset where the animation exports. Native Lynx has no scroll timelines, so
there is nothing to reconcile with it; the choices below follow Blink where
the specifications are silent or disagree.

- **Properties.** `animation-timeline: scroll() | view() | <dashed-ident> |
  none | auto`, `scroll-timeline-name/-axis` (+ `scroll-timeline`),
  `view-timeline-name/-axis/-inset` (+ `view-timeline`), `timeline-scope`,
  `animation-range-start/-end` (+ `animation-range`) and
  `animation-duration: auto` parse and cascade under `lynx` (the stylo fork's
  `lynx: expose the scroll-animations-1 timeline properties`). `animation`
  resets `animation-timeline` and the ranges, as Firefox and Chrome ship it.
- **Model.** An animation whose `animation-timeline` is not `auto` is
  *progress-driven* (the fork's `Animation::is_progress_driven`): stylo
  cascades `Animation::timeline_sample`, the simple iteration progress plus
  the iteration's direction, and never iterates, ends or ticks it by the
  clock. `dom::style::timeline` writes that sample: `ProgressTiming`
  normalizes the animation's range, delay and duration into scroll-offset px
  once per commit, and `iteration_progress` applies web-animations-1 §4.5-4.8
  (web-animations-2 §2.4.4's timeline-boundary rule included) to an offset.
  The phase boundaries are absolute offsets and the active interval's end is
  the range's end itself, so a range ending at the scroll limit reaches its
  last keyframe exactly however its start delay rounds (Blink snaps within a
  tolerance instead).
- **Timeline boundary.** An active interval ending at the source's scroll
  limit — the timeline's maximum time — stays active there without a
  forwards fill; one ending anywhere else is in its after phase at its end.
  This is web-animations-2 as amended by csswg-drafts#13819 (issue #12134)
  and Blink's `Animation::UpdateBoundaryAlignment`, which compare the range
  end against the scroll limits. For `view()` that is not the cover range's
  100%: `animation-range: entry` on a last child ends at the limit and holds;
  `cover` ending short of it does not. The superseded wording, which checked
  the timeline's own 0% and 100%, is what SPEC-3 first encoded.
  An animation is bound per commit after layout (`Document::resolve_timelines`,
  inside `Document::layout`'s pass loop) and re-sampled between commits when
  its scroll container moves (`Document::advance_scroll_timelines`, which the
  runtime calls when it adopts the painter's scroll at the mailbox marker).
  No clock frame is ever asked for; a scroll-driven animation leaves the
  timeline idle.
- **Composite path.** A running scroll-driven animation on `opacity` or
  `transform` exports with its element's curve (the set walk above), carrying
  its binding: the source, the axis and the `ProgressTiming`. After the
  build's walk each binding is bound to its source's scroll slot — the
  element's own slot for `scroll(self)` and a later-painted source's are
  allocated after its curve — and a source the frame has no slot for holds the
  committed sample, as a paused animation and an inactive timeline do. The
  compositor samples the curve from the offset it composes that slot at,
  clamped to `[0, max]` and unsnapped, through `iteration_progress`, then
  stylo's `sample_at`: the sampled values are bit-equal to the ones the main
  thread's cascade commits after scrolling to the same offset. A scroll-driven
  curve has no `expires_at`, asks for no recompose per frame
  (`has_live_curves` counts only curves that read the clock; a scroll
  recomposes by itself) and no main-thread tick; a drag, a fling or a bounce
  moves it with no commit. The main thread leaves an element whose committed
  curve samples a slot to the painter when it adopts a scroll: its cascade
  value is stale between commits, as any exported curve's is
  (`docs/style-assumptions.md` §12), and every commit's resolution re-samples
  it.
- **Base-value rule.** A scroll timeline's offsets move both ways, so there is
  no hand-back: the export refuses when, for a property some entry
  contributes at the commit's instant and offsets, no entry keeps a
  contribution over the whole domain — a document-timeline entry contributing
  now, a transition, a held sample, or a scroll-driven entry with no before
  phase without a backwards fill and no after phase without a forwards fill
  anywhere in the scroll range `[0, max]`. A property no entry contributes at
  the commit is always representable: the committed value is then the base
  value, which a sample without the property reproduces. So a `view()`
  animation with fill `none` committed while its subject is outside its range
  exports, and one committed inside it refuses; `scroll()` over its whole
  range contributes at every offset, the limit included, fill or not. The
  specification's examples use `both`, which is representable at every
  offset.
- **Stale timelines** (scroll-animations-1 §5.1). The flush that creates an
  animation cascades it with no effect; the same `layout()` binds it, writes
  its sample and re-cascades it before the commit, and a change that relayouts
  takes one more pass (bounded by `CONTAINER_PASSES`). A change that pass
  causes is re-sampled at the next commit.
- **Sources.** `scroll(nearest)` binds the nearest scroll container on the
  containing-block chain, `hidden` axes included (every scroll container here
  scrolls both axes; Blink skips only `visible`/`clip` axes); with none, and
  for `scroll(root)`, the document element stands in for the viewport and is
  inactive unless it is a scroll container. `scroll(self)` binds the element.
  `view()` binds the nearest scroll container with no root fallback. A
  timeline whose source cannot scroll along its axis, whose view subject has
  no box (`display: none`/`contents`, or skipped contents, css-contain-2 §4),
  or whose cover range is empty is inactive.
- **View geometry.** The subject's border box in the source's unscrolled
  padding-box coordinates, transforms ignored, against the scrollport inset by
  `view-timeline-inset` (`auto` = the source's `scroll-padding`, `%` of the
  scrollport). A `position: sticky` box between the subject and its source
  makes each edge alignment hold over an interval of offsets; the sticky chain
  is solved exactly (its offset is piecewise linear in the scroll offset) and
  each named range takes the earliest or latest end scroll-animations-1 §3.1
  names. Blink handles only the first sticky container.
- **Named timelines** (§4.2): over the flat tree, the nearest inclusive
  ancestor defining the name (later list entries win, a scroll timeline beats
  a view timeline), else the last definer in flat tree order under the
  nearest `timeline-scope` boundary or the document element, skipping those
  a nested `timeline-scope` limits. Names are tree-scoped (css-scoping): a
  reference matches a definition from the same tree. The tree a cascaded name
  belongs to is read back from stylo's shadow cascade order the way its rule
  collector writes it: `::slotted` rules name the tree of the slot they came
  through along the assigned-slot chain, `:host` rules the element's own
  shadow tree, and `::part` rules the `n`-th enclosing tree that has any
  `::part` rule. The definers come from a side table the flush's restyles
  keep, so a lookup walks candidates, never the page; ordering them walks
  each candidate's flat-tree path once and scans a node's children only
  where the survivors branch.
- **Blink choices.**
  - A time duration and delay convert *proportionally* into the range
    (`AnimationEffect::NormalizedTiming`), rather than css-animations-2's
    "treated as `auto`"; the two agree unless a delay is non-zero. `auto`
    fills the range with no delay. An infinite count, and a duration and
    delay totalling nothing or less (`1s -1s`), leave a zero active duration
    at the range's start, as `NormalizedTiming` does (SPEC-3 first gave the
    latter the `auto` rule): past the start only a forwards fill shows, and
    it shows the last keyframe.
  - A range name on a scroll timeline names the whole timeline.
  - Resuming from `animation-play-state: paused` re-aligns to the scroll
    position; a paused animation holds its last sample, and one created
    paused holds the sample of that moment, not the base value.
  - Progress reads the offset clamped to `[0, max]`: a `contain-bounce`
    stretch is not a scroll offset.
  - An animation on an inactive timeline is idle: no effect whatever its
    fill, and not *current*, so it sets none of the `animates` side effects,
    and an exported curve counts it animating nothing.
- **Deviations and limits.**
  - Stylo's `return;` after updating an existing same-name animation is kept
    (the servo deviation is mirrored), and it now also freezes the timeline
    kind and the timeline of every later same-name animation:
    `Animation::timeline` can lag `animation-timeline` for those, and the
    binding reads `Animation::timeline`.
  - `view-timeline-inset` is not animatable (the fork keeps Firefox's
    `animation_type = "none"`; scroll-animations-1 animates it).
  - Named-range keyframe selectors (`entry 0% { }`) stay pref-gated in the
    fork and are ignored — a ruled follow-up.
  - Pseudo-element animations on a progress timeline are not bound and have
    no effect.
  - An element in skipped contents (css-contain-2 §4) holds its sample.
  - No animation events and no JS `ScrollTimeline`/`ViewTimeline` API: the
    engine dispatches no animation events at all.
  - An animation the composite path refuses — another property, a
    representability refusal, or any of the export's other refusals —
    re-cascades on the main thread when its scroll container moves: one
    animation-only restyle and one commit per adopted scroll while a source
    moves, one frame behind the painter's scroll, and nothing at rest.
  - The painter clamps a scroll-driven offset to the timing's `limit`, the
    source's scroll range as the commit resolved it, rather than to the
    slot's `max_offset`: the two are the same value, and reading the timing's
    keeps the timeline-boundary comparison exact.

---

## Also see

Scope note: this feeds both the style engine (parsing/interpolation) and the render engine (frame scheduling/compositing) — see `.claude/agents/lynx-css-engine.md` and `.claude/agents/lynx-render-engine.md`.
