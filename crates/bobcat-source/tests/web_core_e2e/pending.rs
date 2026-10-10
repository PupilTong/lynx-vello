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
    "api-SelectorQuery" =>
        "thirty `select(…).invoke('boundingClientRect')` probes, each a green or red row; rows `s24_dataset` (`[data-test]`) and `s25_dataset_value` (`[data-test=test_value]`) are red where the card's table, native and web-core have green: typed datasets are not reflected to DOM `data-*` attributes, which `__BobcatQueryNodes` (`packages/bobcat-element/src/element-papi.ts`) matches over (`docs/node-query-runtime.md`, Remaining boundaries)",
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
    "api-exposure-area" =>
        "upstream scrolls `#x` by script and expects the node with `exposure-area='50%'` to report exposure only once half of it is in view; nothing changes colour because this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-exposure-basic" =>
        "upstream scrolls two scroll-views by script and screenshots the background the `exposure`/`disexposure` global events set; the background never changes because this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-exposure-change-exposure-id" =>
        "tapping a numbered box changes the `exposure-id` it carries, and the lines must then read the current, exposed and dis-exposed index; `target index:` and `prev index:` stay empty because this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-exposure-custom-event-handler" =>
        "each of ten rows must turn green from its `binduiappear`; every row stays orange because this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-exposure-dynamic-screen-margin" =>
        "`exposure-screen-margin-bottom` must move the edge a node is tested against as `#y` is scrolled; nothing turns orange because this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-exposure-dynamic-ui-margin" =>
        "`exposure-ui-margin-top` in percent must grow or shrink the node's own box as `#y` is scrolled; nothing turns orange because this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-exposure-no-fake-disappear" =>
        "`#control` must turn green from `binduiappear` while `#target` below the fold receives neither event; `#control` stays red because this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-exposure-stop-events-has-complex-dataset" =>
        "`lynx.stopExposure()` must send a `disexposure` carrying the node's dataset, logged as `pass:dataset2`; `lynx.stopExposure` is not a function here (the background thread throws) and this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-exposure-stop-events-has-dataset" =>
        "`lynx.stopExposure()` must send a `disexposure` per exposed node with its dataset, logged as `pass:dataset1` and `pass:dataset2`; `lynx.stopExposure` is not a function here (the background thread throws) and this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-exposure-stop-exposure" =>
        "the orange area must read `disexposure`, `none`, `disexposure` after the three `lynx.stopExposure(…)` buttons; it reads `none` throughout: `lynx.stopExposure` is not a function here and this engine produces no exposure events (`uiappear`, `uidisappear`, `exposure`, `disexposure` occur nowhere in `crates/` or `packages/bobcat-element/src`)",
    "api-frame-auto-height" =>
        "a `<frame auto-height>` must take its height from the nested card. `<frame>` does not exist here: the compiled card calls `__CreateFrame`, which `packages/bobcat-element/src/element-papi.ts` lists as not implemented, so the main-thread render throws and the screen is blank",
    "api-frame-auto-width" =>
        "upstream asserts only attributes of web-core's host element for a `<frame auto-width>`, which no capture carries. `<frame>` does not exist here: the compiled card calls `__CreateFrame`, which `packages/bobcat-element/src/element-papi.ts` lists as not implemented, so the main-thread render throws and the screen is blank",
    "api-frame-bindload" =>
        "`bindload` on a `<frame>` must deliver `statusCode: 0`, `success` and the bundle URL, printed in three texts. `<frame>` does not exist here: the compiled card calls `__CreateFrame`, which `packages/bobcat-element/src/element-papi.ts` lists as not implemented, so the main-thread render throws and the screen is blank",
    "api-frame-data" =>
        "a `<frame data=…>` must hand its `data` to the nested card, which prints `data:from-data`. `<frame>` does not exist here: the compiled card calls `__CreateFrame`, which `packages/bobcat-element/src/element-papi.ts` lists as not implemented, so the main-thread render throws and the screen is blank",
    "api-frame-data-update" =>
        "a tap changes a `<frame>`'s `data` and the nested card must go from `data:before` to `data:after`. `<frame>` does not exist here: the compiled card calls `__CreateFrame`, which `packages/bobcat-element/src/element-papi.ts` lists as not implemented, so the main-thread render throws and the screen is blank",
    "api-frame-element-map" =>
        "upstream asserts that web-core realizes `<frame>` as a `LYNX-VIEW` DOM element, a fact about web-core's tag map that no capture carries. `<frame>` does not exist here: the compiled card calls `__CreateFrame`, which `packages/bobcat-element/src/element-papi.ts` lists as not implemented, so the main-thread render throws and the screen is blank",
    "api-frame-global-props" =>
        "a `<frame global-props=…>` must reach the nested card's `lynx.__globalProps`, which prints `global:from-global-props`. `<frame>` does not exist here: the compiled card calls `__CreateFrame`, which `packages/bobcat-element/src/element-papi.ts` lists as not implemented, so the main-thread render throws and the screen is blank",
    "api-frame-src" =>
        "a 300x120 `<frame src>` must load and render the nested card, whose first line reads `frame:ready`. `<frame>` does not exist here: the compiled card calls `__CreateFrame`, which `packages/bobcat-element/src/element-papi.ts` lists as not implemented, so the main-thread render throws and the screen is blank",
    "api-get-path-info" =>
        "awaiting a ruling: the card reads `res.path` in the `path()` callback and expects `id: undefined` for an element with none. web-core answers `{path: […]}` with `id: undefined`; native answers the node array itself with `id: \"\"`, and this engine follows native (`__BobcatQueryNodes`, `nodeFields` in `packages/bobcat-element/src/element-papi.ts`), so `res.path` is undefined and `#result` stays pink",
    "api-getSharedData" =>
        "no upstream test has this name: it is the second view of `api-shared-context`, where a tap must paint the colour the first view stored with `lynx.setSharedData`. `lynx.getSharedData` does not exist here (`docs/tracking/js-runtime.md`), and the suite boots no second view in the group",
    "api-global-disallowed-vars" =>
        "DEFECT: `MTS_CHUNK_PREAMBLE` (crates/bobcat-core/src/esm.rs) declares none of the disallowed globals the BTS preamble shadows, so MTS code naming `navigator`/`postMessage` raises a ReferenceError where web-core's lepus wrapper gives `undefined`. The card paints only a sizeless view, so the blank screen hides it",
    "api-globalThis" =>
        "a module-level `globalThis.foo` must be a bare identifier in both realms and inside a `runOnMainThread` worklet; the card paints only a sizeless view, so a capture says nothing about it",
    "api-invoke-fail" =>
        "the subject works: a tap invokes the unknown method `seekTo` and the `fail` callback's `code === 3` turns `#result` green. The frame still cannot be pinned, because the element above it is an `<x-input>`, which is not an element here (editable text is out of scope: user, 2026-09-14) and so takes no border-box default: it is drawn 42px high where its styles give 40px, and both boxes sit 2px low",
    "api-invoke-success" =>
        "`SelectorQuery.invoke`'s success callback is the subject, but the method it calls is `<x-input>`'s `focus`, and editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green; the invoke contract itself is covered by `api-invoke-fail`'s unknown-method path once a tap can be driven",
    "api-lynx-performance" =>
        "the box turns green once `lynx.performance.addTimingListener` has seen `__lynx_timing_actual_fmp` and a flagged update; `lynx.performance` is undefined on the background thread, so it stays pink. Upstream also reads the keys of the host's `timing` event, which the view does not report",
    "api-report-error" =>
        "a tap calls `lynx.reportError('foo')`, and upstream asserts the host's `error` event: `detail.error.message === 'Error: foo'`, a stack, `sourceMap.offset`, and that its shell then hides the view. The report arrives (`ScriptReported`, message `foo`) and the red box stays; the fields upstream reads are not on it, and hiding the view is what #330 deliberately does not do — both await a ruling",
    "api-set-release" =>
        "`_SetSourceMapRelease` is a deliberate no-op here and `ScriptReported` carries no release, so the report upstream asserts cannot match; the blank screen is consistent but settles nothing",
    "api-set-release-bts" =>
        "DEFECT: the BTS `app` behind `lynxCoreInject.tt` has no `setSourceMapRelease`, so the card hits a TypeError before its own error",
    "api-setSharedData" =>
        "the effect stores `green` with `lynx.setSharedData` and paints what `lynx.getSharedData` returns; neither exists here (`docs/tracking/js-runtime.md`), the effect throws on the background thread, and the box stays orange",
    "api-updateData-callback" =>
        "two things stand in the way: the suite never calls `update_data`, and this engine's host API has no completion callback at all — `LynxView::update_data(data, processor_name)` reports no completion, where web-core takes a third callback argument. Upstream asserts only the console line it produces",
    "basic-bindkeydown-out-of-view-noop" =>
        "a non-global `bindKeydown` must not fire for a key pressed outside the view, so the box must stay pink. It does, but for no reason the case is about: the engine has no key input: `InputKind` (`crates/dom/src/input/mod.rs`) is `Pointer | Wheel`, so nothing can deliver `keydown` or `keyup`",
    "basic-bindmouse" =>
        "`bindmousedown`, `bindmouseup` and `bindmousemove` must each fire with `button`, `buttons`, `x`, `y`, `pageX`, `clientX`…; the engine synthesizes pointer and touch events plus `tap` and `longpress` (`crates/bobcat-core/src/paint/gesture.rs`) and no `mouse*` event",
    "basic-element-input-bindinput" =>
        "JSX `<input>` is the same element as `<x-input>`, and editable text is ruled out of scope (user, 2026-09-14; `docs/tracking/web-text-test-replication.md`), so this will not go green: the card types into the box and asserts `bindinput`'s detail carries `value`, `textLength`, `selectionStart` and `selectionEnd` consistently",
    "basic-element-list-basic" =>
        "the first screen is right (five 100px cells), and a 500px wheel scroll brings cells 6 to 10; what upstream asserts after its scripted scroll is that `bindscroll` and `bindscrollend` both ran, and neither handler is ever called on a `<list>` here (the card logs nothing)",
    "basic-element-list-basic-size" =>
        "same card with cells sized by their content (110px each, right on the first screen); the `scroll` and `scrollend` events upstream asserts after scrolling are never delivered on a `<list>` here",
    "basic-element-list-estimated-main-axis-size-px-waterfall" =>
        "The card exists to show that a waterfall list whose cells carry only `estimated-main-axis-size-px` still reports `scrolltolower` after `autoScroll` is invoked from `useEffect` (upstream's removed test waited for that console line). This engine has neither: list threshold events are not implemented (`crates/bobcat-core/src/main/tree/list.rs` head comment) and `autoScroll` is not a UI method the host has (`invokeUIMethod`, `packages/bobcat-element/src/element-papi.ts`, answers code 3), so nothing is logged — and the evidence is a console line, which no capture carries.",
    "basic-element-list-horizontal-estimated-main-axis-size-px" =>
        "The card exists to show that a horizontal list whose cells carry only `estimated-main-axis-size-px` still reports `scrolltolower` after `autoScroll` is invoked from `useEffect` (upstream's removed test waited for that console line). This engine has neither: list threshold events are not implemented (`crates/bobcat-core/src/main/tree/list.rs` head comment) and `autoScroll` is not a UI method the host has (`invokeUIMethod`, `packages/bobcat-element/src/element-papi.ts`, answers code 3), so nothing is logged — and the evidence is a console line, which no capture carries.",
    "basic-element-list-horizontal-estimated-main-axis-size-px-waterfall" =>
        "The card exists to show that a horizontal waterfall list whose cells carry only `estimated-main-axis-size-px` still reports `scrolltolower` after `autoScroll` is invoked from `useEffect` (upstream's removed test waited for that console line; the same commit fixed the lower threshold of a horizontal waterfall). This engine has neither: list threshold events are not implemented (`crates/bobcat-core/src/main/tree/list.rs` head comment) and `autoScroll` is not a UI method the host has (`invokeUIMethod`, `packages/bobcat-element/src/element-papi.ts`, answers code 3), so nothing is logged — and the evidence is a console line, which no capture carries.",
    "basic-element-list-scroll-to-position" =>
        "the first screen is right (five 100px cells in a 500px list); `scrollToPosition` is a UI method that does not exist and the card drives it from a tap",
    "basic-element-scroll-view-event-scroll" =>
        "the pre-swipe frame is right; `scroll-view` emits no `scroll` event and a swipe cannot be settled by a capture",
    "basic-element-scroll-view-event-scrollend" =>
        "same card with `bindscrollend`: the event does not exist and the gesture cannot be settled",
    "basic-element-scroll-view-event-scrolltolower" =>
        "the bottom-threshold event: the threshold observer behind `scrolltolower` is not built, and the frame cannot swipe",
    "basic-element-scroll-view-event-scrolltoupper" =>
        "the upper-threshold event, same reason",
    "basic-element-scroll-view-scroll-to-index" =>
        "`initial-scroll-to-index={1}` must land the scrollport on the second child at boot; the attribute does not exist here, so we start at the first. Statically wrong, and a missing attribute rather than an interaction",
    "basic-element-svg-background-image" =>
        "upstream's screenshot is a drawing from the `content` string over the element's own tiled `background-image`; the tiles paint and the drawing does not: the Lynx `<svg src|content|bindload>` component is not provided yet. The decision against it (revision 3) is withdrawn and the component is in implementation (`docs/svg-lynx-component-design.md`; #375, the first of its four changes, is in); until the rest lands `svg` is still the standard element, drawn from its DOM children, and a card that passes its markup in `content` draws nothing and fires no `load`",
    "basic-element-svg-bindload" =>
        "`#result` must turn green from the `<svg>`'s `bindload`, and it has no size for a capture to show: the Lynx `<svg src|content|bindload>` component is not provided yet. The decision against it (revision 3) is withdrawn and the component is in implementation (`docs/svg-lynx-component-design.md`; #375, the first of its four changes, is in); until the rest lands `svg` is still the standard element, drawn from its DOM children, and a card that passes its markup in `content` draws nothing and fires no `load`",
    "basic-element-svg-hex-color" =>
        "upstream's screenshot is a plus sign from an `<svg content=…>` whose colour is written with a `#`; the frame is blank: the Lynx `<svg src|content|bindload>` component is not provided yet. The decision against it (revision 3) is withdrawn and the component is in implementation (`docs/svg-lynx-component-design.md`; #375, the first of its four changes, is in); until the rest lands `svg` is still the standard element, drawn from its DOM children, and a card that passes its markup in `content` draws nothing and fires no `load`",
    "basic-element-svg-utf8" =>
        "upstream's screenshot is a drawing from an `<svg content=…>` string with non-ASCII characters in it; the frame is blank: the Lynx `<svg src|content|bindload>` component is not provided yet. The decision against it (revision 3) is withdrawn and the component is in implementation (`docs/svg-lynx-component-design.md`; #375, the first of its four changes, is in); until the rest lands `svg` is still the standard element, drawn from its DOM children, and a card that passes its markup in `content` draws nothing and fires no `load`",
    "basic-element-svg-with-css" =>
        "upstream's screenshot is a drawing in an `<svg content=…>` sized by a class; the frame is blank: the Lynx `<svg src|content|bindload>` component is not provided yet. The decision against it (revision 3) is withdrawn and the component is in implementation (`docs/svg-lynx-component-design.md`; #375, the first of its four changes, is in); until the rest lands `svg` is still the standard element, drawn from its DOM children, and a card that passes its markup in `content` draws nothing and fires no `load`",
    "basic-element-svg-with-position" =>
        "upstream's screenshot is a red square in an absolutely positioned, padded, translucent `<svg content=…>`; the frame is blank: the Lynx `<svg src|content|bindload>` component is not provided yet. The decision against it (revision 3) is withdrawn and the component is in implementation (`docs/svg-lynx-component-design.md`; #375, the first of its four changes, is in); until the rest lands `svg` is still the standard element, drawn from its DOM children, and a card that passes its markup in `content` draws nothing and fires no `load`",
    "basic-element-text-bindlayout" =>
        "the layout event is produced but never dispatched, so the result row stays empty",
    "basic-element-text-bindselectionchange" =>
        "selection events are not dispatched; the paragraph itself is right",
    "basic-element-text-maxline-with-setData" =>
        "the setState carrying the clamped string lands at 1000 ms, past this suite's 500 ms settle",
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
    "basic-element-x-blur-view-blur-radius" =>
        "DEFECT: the backdrop bake clears to transparent black and never carries the canvas base colour, and the filtered texture is drawn over the still-painted unfiltered backdrop, so wherever the baked alpha is below one the sharp backdrop shows through — a visible logo silhouette inside the blurred box. The blur itself is spec-exact (sigma 25 matches Chromium to 1-2 levels over opaque areas). Not one of the recorded backdrop-filter narrowings",
    "basic-element-x-foldview-ng-method-setFoldExpanded" =>
        "the unfolded first screen matches run for run; `setFoldExpanded` is deliberately not implemented and the card is a tap + method",
    "basic-element-x-foldview-ng-method-setFoldExpanded-overflow" =>
        "same unfolded screen; the method is not implemented and a capped fold offset is invisible while unfolded",
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
    "basic-element-x-overlay-ng-counter-test2" =>
        "DEFECT in incremental layout, which #372's overlay structure exposes: the first screen is right — no hidden overlay shows and no host background tints the page — but a shown overlay never appears. Each show button flips its label and no square is painted or hit, so the count stays 0 where upstream reads 1, 3, 6, 10. The card's `display: flex` on the host gives it a box, which the UA rule makes `contain: strict`, a relayout boundary, so showing the overlay parks the host as a relayout root of its own. The same tap changes the button's label, which parks `.test-card` as a second root, an ancestor of the first. The rounding tail (`position_and_round_parked_boundaries`, `crates/dom/src/layout/host.rs`) skips a root that has a parked ancestor and leaves it to that ancestor's walk, but the walk enters only boxes written since the last rounding, and nothing marks the boxes between the two roots: here the `display: contents` wrapper around the overlay. So the tail never reaches the host, and the shadow dialog, a top-layer box only the tail places (`pre_position`), keeps its hidden mark and a zero box. Marking the boxes between the two roots where the inner one is skipped makes all four squares show and the count reach 10 (tried, not committed). A `LynxDocument` test reproduces it: a sized view holding a `wrapper` holding an overlay with an author `display`, shown in the same flush in which a sibling inside that view changes its height",
    "basic-element-x-refresh-view-demo" =>
        "the first screen is right, and a 100px pull does bring the red header in. What upstream asserts after each release needs `startrefresh`/`startloadmore` to fire and `finishRefresh`/`finishLoadMore` to put the header and footer back: no refresh-view event or UI method exists (`crates/bobcat-core/src/main/tree/refresh_view.rs`), so no item rows appear and the header stays snapped in. Upstream's two mid-pull screenshots are taken with the finger still down, which the suite has no step for",
    "basic-element-x-swiper-autoplay" =>
        "the first frame is right (with `autoplay` off nothing turns in 5.6 s); the next three must show the second, third and fourth item as autoplay turns the swiper, and it stays on the first: `autoplay`, `interval` and `smooth-scroll` are not implemented (deferred by the user; `crates/bobcat-core/src/main/tree/swiper.rs`). The last frame is also a native/web-core difference: web-core turns once more after `autoplay` is switched off, native stops",
    "basic-element-x-swiper-bindchange" =>
        "upstream asserts the `change` events three swipers log across a manual swipe, autoplay and a `current` change; the card logs nothing: no swiper event exists (`swiper.rs`: every event and every UI method is not implemented), and `autoplay`, `interval` and `smooth-scroll` are not implemented (deferred by the user; `crates/bobcat-core/src/main/tree/swiper.rs`)",
    "basic-element-x-swiper-bindscrollend" =>
        "upstream asserts the `scrollend` events three swipers log; the card logs nothing: no swiper event exists (`swiper.rs`: every event and every UI method is not implemented)",
    "basic-element-x-swiper-bindscrollstart" =>
        "upstream asserts the `scrollstart` events three swipers log, with `isDragged` and `current`; the card logs nothing: no swiper event exists (`swiper.rs`: every event and every UI method is not implemented)",
    "basic-element-x-swiper-circular-carousel" =>
        "four of the five frames are right; on the last item web-core keeps it at the swiper's start and shows the first one in the 20% to its right, and here the last item rests against the swiper's right edge with the third one in the 20% to its left. `circular` wraps the axis and the mode's 20% margins are off under it now (#368), so the items pack edge to edge; but an end item's snap position lies outside the scroll range and the offset stops at the range's end instead of going on through the seam: the last item's start is at 943px of a 1258px period and the offset rests at 865, the items' extent less the swiper's width",
    "basic-element-x-swiper-circular-coverflow" =>
        "the first screen is already different: web-core centres the first item with the last one to its left, and here the first item rests against the swiper's left edge with only the second beside it; the last item rests against the right edge the same way. `circular` wraps the axis and the mode's 20% margins are off under it now (#368), so the items pack edge to edge; but an end item's snap position lies outside the scroll range and the offset stops at the range's end instead of going on through the seam. `coverflow` is also flat here, which is itself awaiting confirmation",
    "basic-element-x-swiper-circular-flat-coverflow" =>
        "the two middle frames are right now: the current item centred with a neighbour in the 20% on each side. The first screen and the last item are not: web-core centres the first item with the last one to its left, and the last item with the first one to its right; here the first item rests against the swiper's left edge and the last against its right, each with one neighbour only. `circular` wraps the axis and the mode's 20% margins are off under it now (#368), so the items pack edge to edge; but an end item's snap position lies outside the scroll range and the offset stops at the range's end instead of going on through the seam: centring the first item needs an offset of -79px and it rests at 0; centring the last needs 629 and it rests at 550",
    "basic-element-x-swiper-interval" =>
        "autoplay at `interval` 5000, then 1000, then 0 must reach the second, third and last item; the swiper stays on the first: `autoplay`, `interval` and `smooth-scroll` are not implemented (deferred by the user; `crates/bobcat-core/src/main/tree/swiper.rs`)",
    "basic-element-x-swiper-method-scroll-to" =>
        "a tap invokes the swiper's `scrollTo` UI method with the next index; the right-hand swiper must go to its second and third item and stays on the first: no swiper UI method exists (`swiper.rs`)",
    "basic-element-x-swiper-mode-carry" =>
        "the first screen is right (one item at full scale, no dots), but what upstream screenshots is the swiper with a 250px drag still held, both items part-scaled. The suite has no step that leaves the finger down, and the picture would show the scale running linearly where web-core's keyframes ease, which is part of the flat-coverflow decision awaiting confirmation",
    "basic-element-x-swiper-mode-coverflow" =>
        "awaiting confirmation: `coverflow` is flat here — each neighbour is scaled, where web-core rotates it in 3D (and native rotates without scaling). `docs/tracking/deviations.md` records that as the architect's decision, for performance, to be confirmed by the user, and a golden would settle it. Everything else in both frames is right: 60% items, centred, the 20% margins, the dots",
    "basic-element-x-swiper-page-margin" =>
        "five swipers, one per mode, turned from `current` 0 to 1; four are right in both frames and the fourth is `coverflow`. awaiting confirmation: `coverflow` is flat here — each neighbour is scaled, where web-core rotates it in 3D (and native rotates without scaling). `docs/tracking/deviations.md` records that as the architect's decision, for performance, to be confirmed by the user, and a golden would settle it. `page-margin` itself changes nothing here, as in web-core; native draws it as a gap between pages",
    "basic-element-x-swiper-swiper-dynamic" =>
        "the first four frames are right: `current` and `indicator-color` changing together, and the right-hand swiper unmounted so the left one takes the full width. When a new swiper is mounted and the left one is halved again, it must stay on its second item and lands on its third: its scroll offset is kept across the width change, where css-scroll-snap-1 has a snapped container re-snap to the same item (Chromium and WebKit do; upstream calls the element's own `scrollTo` from the page to make Firefox agree, a call no embedder here has)",
    "basic-element-x-swiper-vertical" =>
        "five `vertical` swipers, one per mode, turned from `current` 0 to 1; four are right in both frames and the fourth is `coverflow`. awaiting confirmation: `coverflow` is flat here — each neighbour is scaled, where web-core rotates it in 3D (and native rotates without scaling). `docs/tracking/deviations.md` records that as the architect's decision, for performance, to be confirmed by the user, and a golden would settle it",
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
        "the first screen is right; tap + `selectTab` + `change` are all outside a first screen, and `change` is not implemented",
    "basic-element-x-viewpager-ng-bindoffsetchange" =>
        "the first screen is right; `offsetchange` is deliberately not implemented and the card is swipe-driven",
    "basic-element-x-viewpager-ng-exposure" =>
        "geometry is right, but exposure tracking does not exist, so the five texts never repaint as pages come into view",
    "basic-element-x-webview-bindmessage" =>
        "the text must read `hello from iframe` once the `<x-webview html=…>` document posts a message; it reads `waiting`: there is no `x-webview` component and nothing in `crates/` hosts an HTML document",
    "basic-event-trigger" =>
        "a `capture-bindtap` and a `bindtap` on one element must both run for one tap, capture first, with the first handler's `setState` visible to the second: the text must read `[capture tap][bind tap]`. It reads `[bind tap]`: the second handler's `setText(text + …)` still sees the empty `text`, so the capture handler's update is overwritten. Both handlers reach the background thread without a render between them (inferred from the picture, not traced)",
    "basic-flex-nested-linear-setting" =>
        "the case asserts computed style only and neither box has a background, so a first screen shows nothing either way",
    "basic-global-bindkeydown" =>
        "`global-bindKeydown` must turn the box green on a key press; the engine has no key input: `InputKind` (`crates/dom/src/input/mod.rs`) is `Pointer | Wheel`, so nothing can deliver `keydown` or `keyup`",
    "basic-global-bindkeydown-code" =>
        "`global-bindKeydown` must show the pressed key's `code`; the engine has no key input: `InputKind` (`crates/dom/src/input/mod.rs`) is `Pointer | Wheel`, so nothing can deliver `keydown` or `keyup`",
    "basic-global-bindkeydown-key" =>
        "`global-bindKeydown` must show the pressed key's `key`; the engine has no key input: `InputKind` (`crates/dom/src/input/mod.rs`) is `Pointer | Wheel`, so nothing can deliver `keydown` or `keyup`",
    "basic-global-bindkeydown-shift" =>
        "`global-bindKeydown` must see `shiftKey` for Shift+a; the engine has no key input: `InputKind` (`crates/dom/src/input/mod.rs`) is `Pointer | Wheel`, so nothing can deliver `keydown` or `keyup`",
    "basic-global-bindkeyup" =>
        "`global-bindKeyup` must turn the box green on a key release; the engine has no key input: `InputKind` (`crates/dom/src/input/mod.rs`) is `Pointer | Wheel`, so nothing can deliver `keydown` or `keyup`",
    "basic-lazy-component-css" =>
        "the card's `.container` must stay red while the lazy component's own `.container` is orange; both boxes are red. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies. Once it does, per-entry scoping still stands in the way: every sheet is mounted document-wide (`__SetCSSId` is a sink)",
    "basic-lazy-component-css-blank" =>
        "the card's `.container { background-color: red }` must not colour the lazy component's `.container`, whose own rule only sizes it; both boxes are red. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies, and the card's rule reaches the lazy view because every sheet is mounted document-wide (`__SetCSSId` is a sink)",
    "basic-lazy-component-css-multi" =>
        "three `.container` boxes must paint three colours, each from its own entry's sheet; the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies, and the screen never stops changing, so no frame can be taken at all",
    "basic-lazy-component-css-selector-false-exchange-class" =>
        "`#target` must be green, yellow, then unpainted as its classes are exchanged and removed; it is never painted. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies",
    "basic-lazy-component-css-selector-false-inline-css-change-same-time" =>
        "`#target` must be yellow, red, yellow; it is unpainted, red, unpainted: only the inline colour shows. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies",
    "basic-lazy-component-css-selector-false-inline-remove-css-remove-inline" =>
        "`#target` must be green, red, red, yellow, then unpainted; it is unpainted wherever a class should colour it. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies",
    "basic-lazy-component-css-selector-false-multi-level-selector" =>
        "the lazy component's `.parent .background` must paint `#target` pink; the frame is blank. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies",
    "basic-lazy-component-css-selector-false-remove-all" =>
        "`#target` must be green until its classes are removed; it is never painted. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies",
    "basic-lazy-component-css-selector-false-remove-css-and-reuse-css" =>
        "`#target` must be yellow, green, yellow as a class is added and removed; it is never painted. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies",
    "basic-lazy-component-css-selector-false-remove-css-and-style-collapsed" =>
        "the three frames look right (green, yellow, green) only because the inline colour decides each of them; the class rules the case sets against it never apply: the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies. A golden now would be of a card that tests nothing",
    "basic-lazy-component-css-selector-false-remove-inline-style-and-reuse-css" =>
        "`#target` must be green, red, green; it is unpainted, red, unpainted. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies",
    "basic-lazy-component-css-selector-false-type-selector" =>
        "no upstream test opens it (the test of that position opens the `config-` twin); by its source the lazy component's `view { … background-color: yellow }` must paint a 100x100 yellow box, and the frame is blank. the lazy container loads, but its stylesheet does not: `__AdoptStyleSheet('<container>/index.css')` throws, because `lazy_bundle_sources` (`crates/bobcat-source/src/lazy_bundle.rs`) registers a named sheet only for a custom section with `encoding: \"CSS\"`, and a `.web.bundle` container carries its rules in `StyleInfo`. No class rule of the lazy component applies",
    "basic-lazy-component-mts-bindtap" =>
        "the lazy component mounts (pink box), but a tap on its `main-thread:bindtap` box does not turn it green. The log shows the container's stylesheet failing to load (`__AdoptStyleSheet('<chunk>/index.css')`: no such sheet is registered for a container without a `CSS` section, `crates/bobcat-source/src/lazy_bundle.rs`) inside `rLynxPrepareLazyBundleMTS`; whether that throw is what keeps the lazy bundle's main-thread handler from being registered is not traced",
    "basic-main-query-selector" =>
        "a main-thread tap handler calls `autoScroll` on the scroll-view it finds with `lynx.querySelector`, and upstream reads `scrollTop > 100` three seconds later; `autoScroll` is not a UI method here (`invokeUIMethod` answers code 3, reported as `ListenerFailed`), so the list does not move",
    "basic-mts-systeminfo" =>
        "awaiting the same ruling as `api-SystemInfo`: the main-thread tap handler logs `hello world` only if `SystemInfo.platform === 'web'`, and this engine reports `\"headless\"`, so the line never comes",
    "basic-ppx-unit" =>
        "the Lynx-only `ppx` unit is not in the fork's length grammar (`vendor/stylo/style/tests/lynx_lengths.rs::rejects_invalid_or_removed_units`), so both declarations are dropped and nothing paints. Admitting it needs a ruling first: native resolves `ppx` by dividing by the device pixel ratio (`lynx/core/renderer/css/css_style_utils.cc`), web-core defines `--ppx-unit: 1cqw` (`web-core/css/index.css`), and upstream's assertion — a 10x10 box after the host narrows the view to 50px — holds only under web-core's. That resize is also a step the suite does not have",
    "basic-ref-main-invoke-ui-method" =>
        "a main-thread tap handler calls `autoScroll` through a `main-thread:ref` on a scroll-view, and upstream reads `scrollTop > 100` two seconds later; `autoScroll` is not a UI method here (`invokeUIMethod` answers code 3, reported as `ListenerFailed`), so the list does not move",
    "config-css-inheritance-default" =>
        "awaiting the same re-confirmation as `config-css-inheritance-false`: the build leaves `enableCSSInheritance` unset, which compiles to `false`",
    "config-css-inheritance-false" =>
        "awaiting re-confirmation of a ruling: with the flag off this engine paints web-core's picture (texts take the view's typography and reset only `color`), where native inherits nothing. That is the recorded decision (`docs/tracking/deviations.md`, CSS inheritance, 2026-07), but its premise — web-core ignores the flag entirely — stopped being true with lynx-stack #3907, so a golden would settle the off case without the user having looked again. Upstream also taps `#update` and asserts `#solid-text` stays black",
    "config-css-inheritance-true" =>
        "DEFECT: `enableCSSInheritance: true` has no effect: the three inherited texts and the two gradient texts are black where upstream asserts the view's red and its `linear-gradient(red, blue)`. The UA rule `text { color: initial }` (`crates/bobcat-core/src/main/tree/text.rs`) is unconditional and `PageConfig` has no inheritance switch. That was a decision (`docs/tracking/deviations.md`, CSS inheritance) resting on web-core ignoring the flag; since lynx-stack #3907 web-core honours it, as native does. Upstream also taps `#update` and asserts the new size and gradient reach `#solid-text`",
    "config-css-remove-scope-false" =>
        "`#index` must be red and `#sub` green, each from its own file's `.basic`; `#sub` covers `#index` exactly, so a frame shows only `#sub`, which is green with or without scoping, and the hidden half is the wrong one: fragments are mounted globally (`crates/bobcat-source/src/lower_style.rs`), so `#index` computes green. Same cause as `config-css-remove-scope-false-with-descendant-combinator`",
    "config-css-remove-scope-false-with-descendant-combinator" =>
        "the flag this card is built to test has no effect here: `index.css` and `sub.css` each define `.a .b` with a different colour, and per-component css-id scoping is the only thing that could tell the two views apart. `crates/bobcat-source/src/lower_style.rs::to_preparsed_style_sheet` flattens every `css_id` fragment into one rule list and synthesizes no `:where([l-css-id=\"N\"])` guard, so both rules match both views and `sub.css`'s later `orange` overrides `index.css`'s `green` on the index-owned child too — the collision `CompatibilityWarning::ComponentScopedCss` already predicts. The two stacked 100x100 boxes are placed right; only the upper one's colour is wrong",
    "config-css-selector-false-exchange-class" =>
        "`class='background-yellow background-green'` must be green, the exchanged order yellow, no class unpainted; it is yellow, yellow, unpainted. The card is built with `enableCSSSelector: false`, where the class written last in the `class` attribute wins; this engine ignores the flag by decision (`docs/style-assumptions.md` D.17, D-bis.23) and resolves the two equal-specificity rules by source order, where `.background-yellow` comes second",
    "config-css-selector-false-inline-remove-css-remove-inline" =>
        "the first screen must be green and is yellow; the later frames (red, red, yellow, unpainted) are right. The card is built with `enableCSSSelector: false`, where the class written last in the `class` attribute wins; this engine ignores the flag by decision (`docs/style-assumptions.md` D.17, D-bis.23) and resolves the two equal-specificity rules by source order, where `.background-yellow` comes second",
    "config-css-selector-false-remove-all" =>
        "the first screen must be green and is yellow; removing the classes leaves it unpainted, which is right. The card is built with `enableCSSSelector: false`, where the class written last in the `class` attribute wins; this engine ignores the flag by decision (`docs/style-assumptions.md` D.17, D-bis.23) and resolves the two equal-specificity rules by source order, where `.background-yellow` comes second",
    "config-css-selector-false-remove-css-and-reuse-css" =>
        "adding `background-green` after `background-yellow` must turn the box green; it stays yellow in all three frames. The card is built with `enableCSSSelector: false`, where the class written last in the `class` attribute wins; this engine ignores the flag by decision (`docs/style-assumptions.md` D.17, D-bis.23) and resolves the two equal-specificity rules by source order, where `.background-yellow` comes second",
    "config-css-selector-false-remove-inline-style-and-reuse-css" =>
        "must be green, red, green; it is yellow, red, yellow. The card is built with `enableCSSSelector: false`, where the class written last in the `class` attribute wins; this engine ignores the flag by decision (`docs/style-assumptions.md` D.17, D-bis.23) and resolves the two equal-specificity rules by source order, where `.background-yellow` comes second",
    "config-mixed-01" =>
        "`class='background-yellow background-green'` must be green and is yellow. The card is built with `enableCSSSelector: false`, where the class written last in the `class` attribute wins; this engine ignores the flag by decision (`docs/style-assumptions.md` D.17, D-bis.23) and resolves the two equal-specificity rules by source order, where `.background-yellow` comes second",
}
