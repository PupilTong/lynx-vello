// Cards that were read against their own source and upstream's test, and whose
// rendering is wrong, or cannot be judged from a frame, in the way the reason
// names. No golden belongs to any of them; see `web_core_e2e.rs`.
//
// A reason starting `awaiting a ruling` is a card where native Lynx and
// web-core disagree and nobody has said which this engine follows.
//
// Moving one into `verified.rs` is the whole job this suite exists for: read
// what the case tests, decide whether this engine answers it correctly, and
// only then accept the picture.

pending! {
    "api-SystemInfo" =>
        "awaiting a ruling: the box turns green only if `SystemInfo.platform === 'web'`. web-core hard-codes `'web'`, native reports its build platform, and this engine reports `\"headless\"` (`packages/bobcat-element/src/main-thread-runtime.ts`, `background-thread-runtime.ts`); `docs/tracking/js-runtime.md` leaves the string open. `pixelRatio` is already a number, so the string alone decides the card",
    "api-bindlauoutchange" =>
        "`bindLayoutChange` must deliver the box's rect and turn `#target` green; no `layoutchange` event exists here (`docs/tracking/web-text-test-replication.md` section E), so it stays pink",
    "api-bindlayoutchange-lynx-view-relative" =>
        "the same `layoutchange` card with the host element offset upstream; no `layoutchange` event exists here (`docs/tracking/web-text-test-replication.md` section E), so `#target` stays pink",
    "api-devtool-event" =>
        "the card shows `ready` once it has a listener on `lynx.getDevtool()`, then the host's `sendDevtoolEvent` payload; it shows `waiting` because `lynx.getDevtool` does not exist (the background thread throws `TypeError: not a function`), and a view has no devtool-event call for a host to make",
    "api-dispose" =>
        "the first screen (a 100x100 pink box) is right, but the assertion is on what happens after the host releases the view: the ReactLynx unmount cleanup logs `fin` and exactly one worker terminates. Dropping the `LynxView` ends the only channel a console line could arrive on, so neither is observable from the host",
    "api-error" =>
        "the card throws on both threads, so a blank screen is the only painting there can be; what upstream asserts is the error report's sourcemap offset and stack, which no capture can settle. web-core also hides the host element, where this engine deliberately keeps the view (#330)",
    "api-error-bts" =>
        "a BTS-only throw, same shape: upstream asserts `fileName` (`app-service.js`), where this engine names the realm instead",
    "api-error-mts" =>
        "an MTS-only throw: blank is consistent and the view surviving to `ScriptFinished` is #330 working; upstream asserts the report's `fileName` (`lepus.js`), which this engine does not carry",
    "api-frame-auto-height" =>
        "a `<frame auto-height>` must take its height from the nested card. `<frame>` exists since #409 and renders its nested card's main-thread script into its shadow root (see `api-frame-src`), but the minimal frame has no `auto-height`, by the ruling recorded with #409 (MTS only; no `load` event, no `data`, no `global-props`, no `auto-*`)",
    "api-frame-auto-width" =>
        "upstream asserts only attributes of web-core's host element for a `<frame auto-width>`, which no capture carries. `<frame>` exists since #409 and renders its nested card's main-thread script into its shadow root (see `api-frame-src`), but the minimal frame has no `auto-width`, by the ruling recorded with #409 (MTS only; no `load` event, no `data`, no `global-props`, no `auto-*`)",
    "api-frame-bindload" =>
        "`bindload` on a `<frame>` must deliver `statusCode: 0`, `success` and the bundle URL, printed in three texts. `<frame>` exists since #409 and renders its nested card's main-thread script into its shadow root (see `api-frame-src`), but the minimal frame has no `load` event, by the ruling recorded with #409 (MTS only; no `load` event, no `data`, no `global-props`, no `auto-*`)",
    "api-frame-data" =>
        "a `<frame data=...>` must hand its `data` to the nested card, which prints `data:from-data`. `<frame>` exists since #409 and renders its nested card's main-thread script into its shadow root (see `api-frame-src`), but the minimal frame has no `data`, by the ruling recorded with #409 (MTS only; no `load` event, no `data`, no `global-props`, no `auto-*`), so it prints `data:`",
    "api-frame-data-update" =>
        "a tap changes a `<frame>`'s `data` and the nested card must go from `data:before` to `data:after`. `<frame>` exists since #409 and renders its nested card's main-thread script into its shadow root (see `api-frame-src`), but the minimal frame has no `data`, by the ruling recorded with #409 (MTS only; no `load` event, no `data`, no `global-props`, no `auto-*`)",
    "api-frame-element-map" =>
        "upstream asserts that web-core realizes `<frame>` as a `LYNX-VIEW` DOM element, a fact about web-core's tag map that no capture carries. The frame itself renders its nested card here since #409, as `api-frame-src` shows",
    "api-frame-global-props" =>
        "a `<frame global-props=...>` must reach the nested card's `lynx.__globalProps`, which prints `global:from-global-props`. `<frame>` exists since #409 and renders its nested card's main-thread script into its shadow root (see `api-frame-src`), but the minimal frame has no `global-props`, by the ruling recorded with #409 (MTS only; no `load` event, no `data`, no `global-props`, no `auto-*`)",
    "api-get-path-info" =>
        "awaiting a ruling: the card reads `res.path` in the `path()` callback and expects `id: undefined` for an element with none. web-core answers `{path: […]}` with `id: undefined`; native answers the node array itself with `id: \"\"`, and this engine follows native (`__BobcatQueryNodes`, `nodeFields` in `packages/bobcat-element/src/element-papi.ts`), so `res.path` is undefined and `#result` stays pink",
    "api-global-disallowed-vars" =>
        "DEFECT: `MTS_CHUNK_PREAMBLE` (crates/bobcat-core/src/esm.rs) declares none of the disallowed globals the BTS preamble shadows, so MTS code naming `navigator`/`postMessage` raises a ReferenceError where web-core's lepus wrapper gives `undefined`. The card paints only a sizeless view, so the blank screen hides it",
    "api-invoke-fail" =>
        "the subject works: a tap invokes the unknown method `seekTo` and the `fail` callback's `code === 3` turns `#result` green (no kind and not the base set answers `seekTo`: `crates/bobcat-core/src/main/tree/base_methods.rs`). The frame still cannot be pinned, because the element above it is an `<x-input>`, which is not an element here (editable text is out of scope: user, 2026-09-14) and so takes no border-box default: it is drawn 42px high where its styles give 40px, and both boxes sit 2px low",
    "api-invoke-success" =>
        "`SelectorQuery.invoke`'s success callback is the subject, but the method it calls is `<x-input>`'s `focus`, which answers code 3 (`crates/bobcat-core/src/main/tree/base_methods.rs`: `focus`/`blur` are not built, `dom` has no focus model), and `<x-input>` itself is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so `#result` stays pink after the tap; the invoke contract is covered by `api-invoke-fail`'s unknown-method path",
    "api-lynx-performance" =>
        "the box turns green once `lynx.performance.addTimingListener` has seen `__lynx_timing_actual_fmp` and a flagged update; `lynx.performance` is undefined on the background thread, so it stays pink. Upstream also reads the keys of the host's `timing` event, which the view does not report",
    "api-report-error" =>
        "a tap calls `lynx.reportError('foo')`, and upstream asserts the host's `error` event: `detail.error.message === 'Error: foo'`, a stack, `sourceMap.offset`, and that its shell then hides the view. The report arrives (`ScriptReported`, message `foo`) and the red box stays; the fields upstream reads are not on it, and hiding the view is what #330 deliberately does not do — both await a ruling",
    "api-set-release" =>
        "`_SetSourceMapRelease` is a deliberate no-op here and `ScriptReported` carries no release, so the report upstream asserts cannot match; the blank screen is consistent but settles nothing",
    "api-set-release-bts" =>
        "DEFECT: the BTS `app` behind `lynxCoreInject.tt` has no `setSourceMapRelease`, so the card hits a TypeError before its own error",
    "api-updateData-callback" =>
        "two things stand in the way: the suite never calls `update_data`, and this engine's host API has no completion callback at all — `LynxView::update_data(data, processor_name)` reports no completion, where web-core takes a third callback argument. Upstream asserts only the console line it produces",
    "basic-element-input-bindinput" =>
        "JSX `<input>` is the same element as `<x-input>`, and editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card types into the box and asserts `bindinput`'s detail carries `value`, `textLength`, `selectionStart` and `selectionEnd` consistently",
    "basic-element-list-estimated-main-axis-size-px-waterfall" =>
        "the card exists to show that a waterfall list whose cells carry only `estimated-main-axis-size-px` still reports `scrolltolower` after `autoScroll` is invoked from `useEffect` (upstream's removed test waited for that console line). `autoScroll` is built on lists since #408, but the list threshold event `scrolltolower` is not (#407 built only `scroll` and `scrollend`), so nothing is logged",
    "basic-element-list-horizontal-estimated-main-axis-size-px" =>
        "the card exists to show that a horizontal list whose cells carry only `estimated-main-axis-size-px` still reports `scrolltolower` after `autoScroll` is invoked from `useEffect` (upstream's removed test waited for that console line). `autoScroll` is built on lists since #408, but the list threshold event `scrolltolower` is not (#407 built only `scroll` and `scrollend`), so nothing is logged",
    "basic-element-list-horizontal-estimated-main-axis-size-px-waterfall" =>
        "the card exists to show that a horizontal waterfall list whose cells carry only `estimated-main-axis-size-px` still reports `scrolltolower` after `autoScroll` is invoked from `useEffect` (upstream's removed test waited for that console line). `autoScroll` is built on lists since #408, but the list threshold event `scrolltolower` is not (#407 built only `scroll` and `scrollend`), so nothing is logged",
    "basic-element-scroll-view-event-scrolltolower" =>
        "the script mirrors upstream's two swipes from (100, 500) up 450px, and the frame is right, scrolled to the end. What upstream asserts is a `scrolltolower` event logged once. #407 built the W3C `scroll` and `scrollend` events; the Lynx threshold events `scrolltolower` and `scrolltoupper` are not built, so nothing is logged",
    "basic-element-scroll-view-event-scrolltoupper" =>
        "the upper-threshold event, same reason as `basic-element-scroll-view-event-scrolltolower`: `scrolltoupper` is not built",
    "basic-element-scroll-view-scroll-to-index" =>
        "`initial-scroll-to-index={1}` must land the scrollport on the second child at boot; the attribute does not exist here, so we start at the first. Statically wrong, and a missing attribute rather than an interaction",
    "basic-element-text-bindlayout" =>
        "the layout event is produced but never dispatched, so the result row stays empty",
    "basic-element-text-bindselectionchange" =>
        "selection events are not dispatched; the paragraph itself is right",
    "basic-element-text-set-native-props-text" =>
        "the setNativeProps push is not retargeted onto the leading raw-text (recorded GAP)",
    "basic-element-text-set-native-props-text-do-not-change-inline-text" =>
        "same un-retargeted push (recorded GAP)",
    "basic-element-text-set-native-props-with-maxlength" =>
        "same un-retargeted push; the clamp itself is right but the paragraph box is two lines tall for one painted line",
    "basic-element-text-set-native-props-with-setData" =>
        "same un-retargeted push; the pre-tap frame is right",
    "basic-element-text-text-selection" =>
        "what a selection drag highlights is an interaction no first screen carries; selection is out of scope",
    "basic-element-x-audio-tt-play" =>
        "upstream's own test body is empty (its skip is commented out with a FIXME), so nothing is asserted. There is no `x-audio-tt` component and no audio output here, and the card's tap handler reads an undeclared `e`, so it would throw before any `invoke`",
    "basic-element-x-input-bindblur" =>
        "`<x-input>` paints a geometrically right box with no value, and editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts `bindblur` reports the value when focus leaves",
    "basic-element-x-input-bindconfirm" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card starts the input focused and asserts the keyboard's confirm key fires `bindconfirm` with the current value",
    "basic-element-x-input-bindfocus" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts `bindfocus` reports the value of a `type=\"password\"` input when it gains focus",
    "basic-element-x-input-bindinput" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card types and asserts `bindinput`'s detail keeps `value.length == textLength` with the caret positions alongside",
    "basic-element-x-input-bindselection" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts `bindselection` reports `selectionStart`/`selectionEnd` as the selection moves",
    "basic-element-x-input-blur" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the red square and the input's box are placed right, the value is absent, and there is no focus machinery for the tap to drive",
    "basic-element-x-input-focus" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card taps a square to run `SelectorQuery.invoke({method:'focus'})` on the input and asserts `bindfocus` then fires",
    "basic-element-x-input-getValue" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card invokes the input's `getValue` method, whose `{value, selectionBegin, selectionEnd}` reply only an editable input can produce",
    "basic-element-x-input-input-filter" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: a geometrically right, empty box where `input-filter` would strip punctuation as the user types",
    "basic-element-x-input-ng-bindinput" =>
        "`<x-input-ng>` is the same element under its other tag name, and editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts the same `bindinput` detail contract",
    "basic-element-x-input-placeholder" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts the `placeholder` text paints with `placeholder-color`, `-font-weight` and `-font-size` applied, none of which exists without an input",
    "basic-element-x-input-placeholder-pseudo-element" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card styles the placeholder through a `::placeholder` rule instead of the attributes, which needs the same missing element",
    "basic-element-x-input-setValue" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card taps to grow the bound value, then invokes `setValue` with an empty string and asserts the box clears",
    "basic-element-x-input-type" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts each `type` (default, `text`, `number`, `digit`, `password`) renders its value the way that type demands",
    "basic-element-x-input-value" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card taps to append to the bound `value` and asserts the input repaints with it",
    "basic-element-x-refresh-view-demo" =>
        "the first screen is right, and a 100px pull does bring the red header in. `finishRefresh` and `finishLoadMore` now exist (`crates/bobcat-core/src/main/tree/refresh_view.rs`), but the card calls them, and adds its item rows, only from its `startrefresh` and `startloadmore` handlers, and no refresh-view event is dispatched (same file: every event is not implemented), so after each release no item rows appear and the header stays snapped in where upstream shows two rows, then four, with the header and footer put back. Upstream's two mid-pull screenshots are taken with the finger still down, which the suite has no step for",
    "basic-element-x-swiper-bindchange" =>
        "upstream asserts the `change` events three swipers log across a manual swipe, autoplay and a `current` change; the card logs nothing: no swiper event exists (`crates/bobcat-core/src/main/tree/swiper.rs`; #408 built autoplay and left the swiper's events out of its scope). The paged frame's manual swiper position depends on the fling velocity of the Drag step, which depends on wall-clock time between moves, so that frame is not a stable picture either",
    "basic-element-x-swiper-bindscrollend" =>
        "upstream asserts the `scrollend` events three swipers log; the card logs nothing: no swiper event exists (`crates/bobcat-core/src/main/tree/swiper.rs`; #408 built autoplay and left the swiper's events out of its scope)",
    "basic-element-x-swiper-bindscrollstart" =>
        "upstream asserts the `scrollstart` events three swipers log, with `isDragged` and `current`; the card logs nothing: no swiper event exists (`crates/bobcat-core/src/main/tree/swiper.rs`; #408 built autoplay and left the swiper's events out of its scope)",
    "basic-element-x-swiper-circular-coverflow" =>
        "`current` 1 and 2 are right: the current item flat and centred, a turned neighbour on each side (#395). The first screen and `current` 3, where the scrollport straddles the seam, are wrong: on the first screen the red current item is drawn with the end keyframe (x 193..314, 0.514 wide, skewed) instead of flat at 79..314, the yellow neighbour's inner edge is at x 59 instead of 78, and the green neighbour is turned out of the scrollport; `current` 3 mirrors it. Two causes in `dom`. A view progress timeline's offset is clamped to `0..=max_offset` (`crates/dom/src/visual/curves.rs` `ScrollTimeline::progress`; `crates/dom/src/style/timeline.rs` `Binding::sample`), and on a circular axis the admitted offset runs to the period (943.2 here, `max_offset` 550.2), so past 550.2 every item is sampled as if the offset were 550.2. And the seam copy is drawn with the animation samples of the first copy (`crates/dom/src/paint/compose.rs` `replay_seams` moves only the scroll offset back one period), the approximation `docs/style-assumptions.md` 25 records, so an item drawn across the seam does not get the progress of the place it is drawn at. Before #395 the same defect drew the red item at the end keyframe's `scale(0.8)`",
    "basic-element-x-swiper-interval" =>
        "the first two frames are web-core's: green after one 5 s turn, and blue 1.6 s after `interval` is set to 1000 (the script takes that frame with a short stability check, because the swiper is quiet for only half a second between turns). The last is not: after `interval` 0, web-core's 4 ms timer moves the swiper to its last item, yellow, within 0.6 s, and here it stays on blue (still blue 4.8 s later). web-core keeps a logical `currentIndex` and adds one per tick; `turn` (`crates/bobcat-core/src/main/tree/swiper.rs`) takes the current item from the scroll offset, and with a turn due every 4 ms a new turn comes before the swiper has moved from blue (inferred from the code, not traced)",
    "basic-element-x-textarea-bindinput" =>
        "`<x-textarea>` is out of scope for the same reason, and editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts `bindinput`/`bindfocus`/`bindblur` each deliver an object detail",
    "basic-element-x-textarea-bindselection" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts `bindselection` reports the caret range inside a textarea",
    "basic-element-x-textarea-color" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts `color: red` reaches the textarea's own value text, which never paints",
    "basic-element-x-textarea-disabled" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts an unset, a static and a toggled `disabled` textarea each paint their disabled state",
    "basic-element-x-textarea-focus" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts an unset, a static and a toggled `focus` attribute each move focus into the right textarea",
    "basic-element-x-textarea-getValue" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card invokes the textarea's `getValue` method, which only an editable textarea can answer",
    "basic-element-x-textarea-input-filter" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: an empty 300x42 box where the initial value belongs",
    "basic-element-x-textarea-maxlength" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts the five orderings of setting `value` and `maxlength` all truncate to the same result",
    "basic-element-x-textarea-maxlines" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts `maxlines=3` clamps a four-line value, and re-clamps when the value changes",
    "basic-element-x-textarea-min-height-max-height" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts the textarea grows with its content between `min-height: 100px` and `max-height: 200px`",
    "basic-element-x-textarea-placeholder" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts a static and a later-set `placeholder` both paint",
    "basic-element-x-textarea-placeholder-font-size" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts the precedence between `placeholder-font-size` and the element's own `font-size` across four textareas",
    "basic-element-x-textarea-placeholder-style" =>
        "editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card asserts the placeholder's colour, size and weight all repaint when their bound values change",
    "basic-element-x-viewpager-ng-allow-horizontal-gesture" =>
        "the first screen is right; `allow-horizontal-gesture={false}` only shows under the swipe it refuses",
    "basic-element-x-viewpager-ng-bindchange" =>
        "the first screen is right; the pager's `change` event is deliberately not implemented and the card is swipe-driven",
    "basic-element-x-viewpager-ng-bindchange-select-tab" =>
        "every frame is right: tapping `select last tab` runs `selectTab({index: 5})` and the pager shows index-5, tapping `select first tab` brings back index-0. What upstream asserts is the `change` event: one per `selectTab`, with `detail.index` 5 and then 0; the card logs none because the pager dispatches no event (`crates/bobcat-core/src/main/tree/viewpager.rs`: the component has the `selectTab` method alone)",
    "basic-element-x-viewpager-ng-bindoffsetchange" =>
        "the first screen is right; `offsetchange` is deliberately not implemented and the card is swipe-driven",
    "basic-element-x-webview-bindmessage" =>
        "the text must read `hello from iframe` once the `<x-webview html=…>` document posts a message; it reads `waiting`: there is no `x-webview` component and nothing in `crates/` hosts an HTML document",
    "basic-event-trigger" =>
        "a `capture-bindtap` and a `bindtap` on one element must both run for one tap, capture first, with the first handler's `setState` visible to the second: the text must read `[capture tap][bind tap]`. It reads `[bind tap]`: the second handler's `setText(text + …)` still sees the empty `text`, so the capture handler's update is overwritten. Both handlers reach the background thread without a render between them (inferred from the picture, not traced)",
    "basic-mts-systeminfo" =>
        "awaiting the same ruling as `api-SystemInfo`: the main-thread tap handler logs `hello world` only if `SystemInfo.platform === 'web'`, and this engine reports `\"headless\"`, so the line never comes",
    "basic-ppx-unit" =>
        "the Lynx-only `ppx` unit is not in the fork's length grammar (`vendor/stylo/style/tests/lynx_lengths.rs::rejects_invalid_or_removed_units`), so both declarations are dropped and nothing paints. Admitting it needs a ruling first: native resolves `ppx` by dividing by the device pixel ratio (`lynx/core/renderer/css/css_style_utils.cc`), web-core defines `--ppx-unit: 1cqw` (`web-core/css/index.css`), and upstream's assertion — a 10x10 box after the host narrows the view to 50px — holds only under web-core's. That resize is also a step the suite does not have",
}
