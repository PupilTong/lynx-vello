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
    "basic-element-x-audio-tt-play" =>
        "upstream's own test body is empty (its skip is commented out with a FIXME), so nothing is asserted. There is no `x-audio-tt` component and no audio output here, and the card's tap handler reads an undeclared `e`, so it would throw before any `invoke`",
    "basic-element-x-webview-bindmessage" =>
        "the text must read `hello from iframe` once the `<x-webview html=…>` document posts a message; it reads `waiting`: there is no `x-webview` component and nothing in `crates/` hosts an HTML document",
}
