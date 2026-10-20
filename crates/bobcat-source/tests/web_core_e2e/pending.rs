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
    "api-frame-bindload" =>
        "`bindload` on a `<frame>` must deliver `statusCode: 0`, `success` and the bundle URL, printed in three texts. A frame has no `load` event (the ruling recorded with #409, unchanged by #425), so the texts keep `-1` and two empty strings",
    "api-global-disallowed-vars" =>
        "native-vs-web-core conflict for the user: main-thread code reading `navigator` and `postMessage` throws ReferenceError here (`MTS_CHUNK_PREAMBLE`, crates/bobcat-core/src/esm.rs, shadows none of them), as on native, where nothing declares them on the main thread and PrimJS throws for an undeclared global. web-core wraps main-thread code with `const navigator=void 0,postMessage=void 0; let window=void 0` (`decode.worker.ts`), because its main thread runs in an iframe that has the browser's own, and upstream expects `main thread: undefined, undefined`. The card paints only a sizeless view, so the screen settles nothing",
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
    "basic-element-text-bindselectionchange" =>
        "upstream selects `#inline-target`'s text with `setBaseAndExtent` and expects `bindselectionchange` to report `7-10-forward` on the `<text>` and `1-4-forward` on the inline text, then the same backward. #467 built the document's `selectionchange` and the Lynx `<text>` event derived from it, but a selection exists only inside a `contenteditable` host there: selecting read-only text is its follow-up, so this plain `<text>` can hold no selection and nothing is reported",
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
    "basic-element-x-refresh-view-demo" =>
        "the first screen is right, and a 100px pull does bring the red header in. `finishRefresh` and `finishLoadMore` now exist (`crates/bobcat-core/src/main/tree/refresh_view.rs`), but the card calls them, and adds its item rows, only from its `startrefresh` and `startloadmore` handlers, and no refresh-view event is dispatched (same file: every event is not implemented), so after each release no item rows appear and the header stays snapped in where upstream shows two rows, then four, with the header and footer put back. Upstream's two mid-pull screenshots are taken with the finger still down, which the suite has no step for",
    "basic-element-x-swiper-bindchange" =>
        "upstream asserts the `change` events three swipers log across a manual swipe, autoplay and a `current` change; the card logs nothing: no swiper event exists (`crates/bobcat-core/src/main/tree/swiper.rs`; #408 built autoplay and left the swiper's events out of its scope). The paged frame's manual swiper position depends on the fling velocity of the Drag step, which depends on wall-clock time between moves, so that frame is not a stable picture either",
    "basic-element-x-swiper-bindscrollend" =>
        "upstream asserts the `scrollend` events three swipers log; the card logs nothing: no swiper event exists (`crates/bobcat-core/src/main/tree/swiper.rs`; #408 built autoplay and left the swiper's events out of its scope)",
    "basic-element-x-swiper-bindscrollstart" =>
        "upstream asserts the `scrollstart` events three swipers log, with `isDragged` and `current`; the card logs nothing: no swiper event exists (`crates/bobcat-core/src/main/tree/swiper.rs`; #408 built autoplay and left the swiper's events out of its scope)",
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
        "native-vs-web-core conflict for the user: the card files `capture-bindtap` and `bindtap` on one element, and expects both to run, capture first: `[capture tap][bind tap]`. Here handlers are keyed by event name alone (`fileEvent` in packages/bobcat-element/src/element-papi.ts, native's `insert_or_assign` in `AttributeHolder::SetStaticEvent`), so the `bindtap` filed second replaces the capture handler and the text reads `[bind tap]`, as native's default event path does. web-core keys them by name and type (`element_data.rs`), so both survive and run, each in its own background task with a re-render between them",
}
