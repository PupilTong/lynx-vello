// What the suite does to a card after its first screen, and the frames that
// reaches. Each script restates what upstream's Playwright spec drives
// (`lynx-stack/packages/web-platform/web-core-e2e/tests/reactlynx.spec.ts`), in
// the terms an embedder has: an input event at a viewport point, a call on the
// view. A point stands for the element upstream's locator names, and the
// comment says which.
//
// A script is not a verdict. Whether a frame is right is said in
// `verified.rs` or `pending.rs`, and only a verified card's frames have
// goldens: `<case>.png` for the first screen, `<case>.<label>.png` after it.

use Step::{
    Drag, Fresh, Logged, Reload, SendGlobalEvent, Tap, UpdateData, UpdateGlobalProps, Wait, Wheel,
};

driven! {
    // `#tap-area`, the 100x100 box inset 50px. Upstream also offsets the whole view by
    // 200px in its host page, which a view with no embedding page has no word for.
    "api-bindtap-lynx-view-relative" => [
        "tapped": [Tap(100.0, 100.0)],
    ],
    // The four numbered texts of the horizontal scroll-view, left to right. The fourth
    // is tapped on its visible part: its centre, (420, 19), is past the viewport, and
    // upstream's click scrolls it into view first.
    "api-exposure-change-exposure-id" => [
        "index-1": [Tap(60.0, 19.0)],
        "index-2": [Tap(180.0, 19.0)],
        "index-3": [Tap(300.0, 19.0)],
        "index-4": [Tap(382.0, 19.0)],
    ],
    // `#button`, clicked once.
    "api-exposure-stop-events-has-complex-dataset" => [
        "disexposed": [Tap(5.0, 5.0), Logged("pass:dataset2")],
    ],
    // `#button`, clicked once.
    "api-exposure-stop-events-has-dataset" => [
        "disexposed": [Tap(5.0, 5.0), Logged("pass:dataset1"), Logged("pass:dataset2")],
    ],
    // The three buttons, in the order upstream clicks them: `stopExposure()`, then the
    // third (`sendEvent: false`), then the second (`sendEvent: true`).
    "api-exposure-stop-exposure" => [
        "stopped": [Tap(196.5, 452.5)],
        "stopped-without-event": [Tap(196.5, 592.5)],
        "stopped-with-event": [Tap(196.5, 522.5)],
    ],
    // `#update-frame-data`, clicked once.
    "api-frame-data-update" => [
        "data-updated": [Tap(196.5, 128.0)],
    ],
    // `#target`, clicked once.
    "api-invoke-fail" => [
        "tapped": [Tap(5.0, 45.0)],
    ],
    // `#target`, clicked once.
    "api-invoke-success" => [
        "tapped": [Tap(5.0, 45.0)],
    ],
    // Nothing is done to the card: upstream waits 3 s for a module answer that
    // comes 2.5 s late.
    "api-nativemodules-call-delay" => [
        "answered": [Wait(3000)],
    ],
    // `#target`. Upstream's assertions are on the host's `error` event; the frame is
    // what the card shows once it has reported.
    "api-report-error" => [
        "error-reported": [Tap(50.0, 50.0)],
    ],
    // `#start`, then `#stop`: once `loop` is on screen it pushes both buttons down a
    // line, which puts `#stop` where `#start` was.
    "api-requestAnimationFrame" => [
        "looping": [Tap(196.5, 24.0)],
        "stopped": [Tap(196.5, 24.0)],
    ],
    // The host sends `event-test` with the one argument the listener waits for.
    "api-sendGlobalEvent" => [
        "sent": [SendGlobalEvent("event-test", r#"["change"]"#)],
    ],
    // The host merges `{mockData: 'updatedData'}`.
    "api-updateData" => [
        "updated": [UpdateData(r#"{"mockData":"updatedData"}"#)],
    ],
    // The same host call; the card's default data processor rewrites it.
    "api-updateData-processData" => [
        "updated": [UpdateData(r#"{"mockData":"updatedData"}"#)],
    ],
    // The host sets `backgroundColor` to blue; upstream labels the frame `blue`.
    "api-updateGlobalProps" => [
        "blue": [UpdateGlobalProps(r#"{"backgroundColor":"blue"}"#)],
    ],
    // `#target`, the 100x100 box at the origin, is clicked twice.
    "basic-bindtap" => [
        "tapped": [Tap(50.0, 50.0)],
        "tapped-again": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "basic-bindtap-detail" => [
        "tapped": [Tap(50.0, 50.0)],
        "tapped-again": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked once. Upstream also reads the `data-mts-clicked` attribute
    // the main-thread handler writes, which no frame shows; the line that handler
    // logs beside it is the evidence it ran.
    "basic-bindtap-simultaneous" => [
        "tapped": [Tap(50.0, 50.0), Logged("MTS Clicked")],
    ],
    // The wheel is turned 100px over `#target`, the 160x160 box at the origin.
    "basic-bindwheel-view" => [
        "wheeled": [Wheel((80.0, 80.0), (0.0, 100.0))],
    ],
    // `#img`, clicked once.
    "basic-element-image-support-tap-event" => [
        "tapped": [Tap(100.0, 100.0)],
    ],
    // Upstream scrolls the list to offset 500 by script; here the wheel is turned
    // that far over it.
    "basic-element-list-basic" => [
        "scrolled": [Wheel((196.5, 250.0), (0.0, 500.0))],
    ],
    // The same scroll over cells sized by their content.
    "basic-element-list-basic-size" => [
        "scrolled": [Wheel((196.5, 250.0), (0.0, 500.0))],
    ],
    // The wheel is turned 100px over `#target`, the 240x240 list at the origin.
    "basic-element-list-bindwheel" => [
        "wheeled": [Wheel((120.0, 120.0), (0.0, 100.0))],
    ],
    // Upstream scrolls the list to offset 5000, past its end, and reads the last
    // cell's computed height before and after. Here the wheel is turned that far
    // over the list, twice: the first turn stops where the extent ended while the
    // last cell was still its 100px estimate, the second reaches the end once the
    // cell has its real 200px.
    "basic-element-list-estimated-main-axis-size-px" => [
        "reached": [Wheel((196.5, 250.0), (0.0, 5000.0))],
        "measured": [Wheel((196.5, 250.0), (0.0, 5000.0))],
    ],
    // `#target`, clicked twice.
    "basic-element-list-remove-action" => [
        "item-4-swapped-in": [Tap(50.0, 50.0)],
        "items-2-3-restored": [Tap(50.0, 50.0)],
    ],
    // `#scrollToPosition`, clicked once.
    "basic-element-list-scroll-to-position" => [
        "scrolled-to-position": [Tap(196.5, 508.0)],
    ],
    // `#target`, clicked four times.
    "basic-element-text-set-native-props-with-setData" => [
        "native-text": [Tap(0.5, 18.5)],
        "value-hello": [Tap(0.5, 18.5)],
        "native-text-2nd": [Tap(0.5, 18.5)],
        "value-world": [Tap(0.5, 18.5)],
    ],
    // `#tap`, clicked once.
    "basic-element-x-foldview-ng-method-setFoldExpanded" => [
        "folded-by-method": [Tap(113.2, 450.0)],
    ],
    // `#tap`, clicked once.
    "basic-element-x-foldview-ng-method-setFoldExpanded-overflow" => [
        "folded-to-limit": [Tap(141.5, 450.0)],
    ],
    // Two upstream tests. `-could-show-all`: the four show buttons, clicked at
    // x 300, to the right of every overlay's square, with the spec's own 100 ms
    // between them; then the close-all button the same way. `-event-correct`: each
    // overlay shown, its square clicked, and closed again, the count going 1, 3, 6, 10.
    "basic-element-x-overlay-ng-counter-test2" => [
        "all-shown": [Tap(300.0, 400.0), Wait(100), Tap(300.0, 450.0), Wait(100), Tap(300.0, 500.0), Wait(100), Tap(300.0, 550.0)],
        "all-closed": [Tap(300.0, 350.0)],
        "count-1": [Fresh, Tap(300.0, 380.0), Wait(50), Tap(50.0, 300.0), Wait(50), Tap(300.0, 380.0)],
        "count-3": [Tap(300.0, 450.0), Wait(50), Tap(50.0, 300.0), Wait(50), Tap(300.0, 450.0)],
        "count-6": [Tap(300.0, 500.0), Wait(50), Tap(50.0, 300.0), Wait(50), Tap(300.0, 500.0)],
        "count-10": [Tap(300.0, 550.0), Wait(50), Tap(50.0, 270.0), Wait(50), Tap(300.0, 550.0)],
    ],
    // Upstream clicks page points: (10, 10), on the blue 50x50 box whose `bindtap`
    // shows the overlay, then (200, 50), on the overlay's wrapper above its red
    // panel, whose `bindtap` slides the panel away and hides the overlay 250 ms on.
    "basic-element-x-overlay-ng-demo" => [
        "overlay-shown": [Tap(10.0, 10.0)],
        "overlay-hidden": [Tap(200.0, 50.0)],
    ],
    // Four upstream tests open this card, each from a fresh page, one per button
    // (`#toggleModal1`..`4`, stacked at x 21..372 from y 182, 54.5px each).
    // 1: the button shows a pass-through overlay; a click at (63, 200), on its red
    //    panel, must change nothing; one at (300, 200), beside the panel, must reach
    //    the button under the overlay and close it.
    // 2: an overlay created by a conditional, shown and closed.
    // 3: an overlay closed from inside, by the `catchtap` of its own blue view.
    // 4: an overlay over a backdrop view of its own.
    "basic-element-x-overlay-ng-playground-test" => [
        "overlay-1-shown": [Tap(196.5, 209.25)],
        "panel-tapped": [Tap(63.0, 200.0)],
        "overlay-1-closed": [Tap(300.0, 200.0)],
        "overlay-2-shown": [Fresh, Tap(196.5, 263.75)],
        "overlay-2-closed": [Tap(20.0, 700.0)],
        "overlay-3-shown": [Fresh, Tap(196.5, 318.25)],
        "overlay-3-closed": [Tap(100.0, 400.0)],
        "overlay-4-shown": [Fresh, Tap(0.0, 0.0), Tap(196.5, 372.75)],
        "overlay-4-kept": [Tap(100.0, 330.0)],
        "overlay-4-closed": [Tap(50.0, 50.0)],
    ],
    // Upstream drags the content down 100px and screenshots with the finger held, then
    // again 2 s after letting go; then the same upward by 200px. A drag here lifts at
    // its end, so only the two frames after a release are taken.
    "basic-element-x-refresh-view-demo" => [
        "refreshed": [Drag((200.0, 500.0), (200.0, 600.0)), Wait(2000)],
        "loaded-more": [Drag((200.0, 600.0), (200.0, 400.0)), Wait(2000)],
    ],
    // Upstream waits out one 5 s interval with `autoplay` off, clicks the 10x10 button
    // at the top right to turn it on, waits two intervals, and clicks it off again.
    "basic-element-x-swiper-autoplay" => [
        "idle-one-interval": [Wait(5600)],
        "autoplayed-to-1": [Tap(388.0, 5.0), Wait(5600)],
        "autoplayed-to-2": [Wait(5600)],
        "autoplay-toggled-off": [Tap(388.0, 5.0), Wait(5500)],
    ],
    // The `next` button under the three swipers (y 450..460), a wait for two autoplay
    // turns, then a 100px swipe left on the first swiper. Upstream asserts once, at
    // the end, on the `change` events the card logged.
    "basic-element-x-swiper-bindchange" => [
        "paged": [Tap(5.0, 455.0), Wait(6600), Drag((300.0, 50.0), (200.0, 50.0))],
    ],
    // The same sequence with a 200px swipe; upstream reads the logged `scrollend` events.
    "basic-element-x-swiper-bindscrollend" => [
        "paged": [Tap(5.0, 455.0), Wait(6600), Drag((300.0, 50.0), (100.0, 50.0))],
    ],
    // The same sequence; upstream reads the logged `scrollstart` events.
    "basic-element-x-swiper-bindscrollstart" => [
        "paged": [Tap(5.0, 455.0), Wait(6600), Drag((300.0, 50.0), (100.0, 50.0))],
    ],
    // The transparent 10x10 `next` button under the swiper, at (0, 150), clicked four
    // times: `current` goes 1, 2, 3 and back to 0.
    "basic-element-x-swiper-circular-carousel" => [
        "current-1": [Tap(5.0, 155.0)],
        "current-2": [Tap(5.0, 155.0)],
        "current-3": [Tap(5.0, 155.0)],
        "wrapped-to-0": [Tap(5.0, 155.0)],
    ],
    // The transparent 10x10 `next` button under the swiper, at (0, 150), clicked four
    // times: `current` goes 1, 2, 3 and back to 0.
    "basic-element-x-swiper-circular-carry" => [
        "current-1": [Tap(5.0, 155.0)],
        "current-2": [Tap(5.0, 155.0)],
        "current-3": [Tap(5.0, 155.0)],
        "wrapped-to-0": [Tap(5.0, 155.0)],
    ],
    // The transparent 10x10 `next` button under the swiper, at (0, 150), clicked four
    // times: `current` goes 1, 2, 3 and back to 0.
    "basic-element-x-swiper-circular-coverflow" => [
        "current-1": [Tap(5.0, 155.0)],
        "current-2": [Tap(5.0, 155.0)],
        "current-3": [Tap(5.0, 155.0)],
        "wrapped-to-0": [Tap(5.0, 155.0)],
    ],
    // The `next` button, here `position: fixed` at the viewport origin, clicked four
    // times: `current` goes 1, 2, 3 and back to 0.
    "basic-element-x-swiper-circular-flat-coverflow" => [
        "current-1": [Tap(5.0, 5.0)],
        "current-2": [Tap(5.0, 5.0)],
        "current-3": [Tap(5.0, 5.0)],
        "wrapped-to-0": [Tap(5.0, 5.0)],
    ],
    // The transparent 10x10 `next` button under the swiper, at (0, 150), clicked four
    // times: `current` goes 1, 2, 3 and back to 0.
    "basic-element-x-swiper-circular-normal" => [
        "current-1": [Tap(5.0, 155.0)],
        "current-2": [Tap(5.0, 155.0)],
        "current-3": [Tap(5.0, 155.0)],
        "wrapped-to-0": [Tap(5.0, 155.0)],
    ],
    // The right-hand swiper (x 196.5..393), whose `bindtap` adds one to its `current`,
    // clicked four times; the last value, 4, names no item of four.
    "basic-element-x-swiper-current" => [
        "current-1": [Tap(294.75, 75.0)],
        "current-2": [Tap(294.75, 75.0)],
        "current-3": [Tap(294.75, 75.0)],
        "current-4-out-of-range": [Tap(294.75, 75.0)],
    ],
    // The swiper (x 0..383), whose `bindtap` adds one to `current`; before the third
    // click the 10x10 button at the top right sets `duration` to 100.
    "basic-element-x-swiper-duration" => [
        "current-1": [Tap(191.5, 75.0)],
        "current-2": [Tap(191.5, 75.0)],
        "current-3-duration-100": [Tap(388.0, 5.0), Tap(191.5, 75.0)],
        "current-4-out-of-range": [Tap(191.5, 75.0)],
    ],
    // Upstream waits for one 5 s autoplay turn, then clicks the two 10x10 buttons under
    // the swiper: `interval` 1000 (y 150..160), then `interval` 0 (y 160..170).
    "basic-element-x-swiper-interval" => [
        "second-item": [Wait(5100)],
        "third-item": [Tap(5.0, 155.0), Wait(1100)],
        "last-item": [Tap(5.0, 165.0)],
    ],
    // The right-hand swiper, `#swiper-1`, whose `bindtap` invokes `scrollTo` with the
    // next index, clicked twice.
    "basic-element-x-swiper-method-scroll-to" => [
        "scrolled-to-1": [Tap(294.75, 75.0)],
        "scrolled-to-2": [Tap(294.75, 75.0)],
    ],
    // The swiper, whose `bindtap` sets `current` to its last item.
    "basic-element-x-swiper-mode-carousel" => [
        "last-item": [Tap(196.5, 75.0)],
    ],
    // The swiper, whose `bindtap` sets `current` to its last item.
    "basic-element-x-swiper-mode-coverflow" => [
        "last-item": [Tap(196.5, 75.0)],
    ],
    // The swiper, whose `bindtap` sets `current` to its last item.
    "basic-element-x-swiper-mode-flat-coverflow" => [
        "last-item": [Tap(196.5, 75.0)],
    ],
    // The swiper, whose `bindtap` sets `current` to its last item.
    "basic-element-x-swiper-mode-normal" => [
        "last-item": [Tap(196.5, 75.0)],
    ],
    // The `next` button, `position: fixed` at the viewport origin: all five swipers go
    // to `current` 1.
    "basic-element-x-swiper-page-margin" => [
        "current-1": [Tap(5.0, 5.0)],
    ],
    // The right-hand swiper, clicked twice; then the left-hand one, which unmounts
    // the right-hand one, and again (it is now the full width), which mounts a new
    // one. Before each of its last two screenshots upstream also calls the left
    // swiper's own `scrollTo` from the page, to put Firefox, which keeps the old
    // offset across the width change, where Chromium and WebKit already are.
    "basic-element-x-swiper-swiper-dynamic" => [
        "index-1": [Tap(294.75, 75.0)],
        "index-2": [Tap(294.75, 75.0)],
        "unmounted": [Tap(98.25, 75.0)],
        "remounted": [Tap(196.5, 75.0)],
    ],
    // The `next` button, `position: fixed` at the viewport origin: all five swipers go
    // to `current` 1.
    "basic-element-x-swiper-vertical" => [
        "current-1": [Tap(5.0, 5.0)],
    ],
    // The control texts `last` (y 16..32), then `first` (y 0..16).
    "basic-element-x-viewpager-ng-bindchange-select-tab" => [
        "last-selected": [Tap(196.5, 24.0)],
        "first-selected": [Tap(196.5, 8.0)],
    ],
    // The centre of the pager, `#target`, clicked once.
    "basic-element-x-viewpager-ng-method-selecttab" => [
        "tab-1-selected": [Tap(196.5, 75.0)],
    ],
    // `#target`, clicked once.
    "basic-event-bubble-dataset" => [
        "tapped": [Tap(25.0, 25.0)],
    ],
    // `#target`, clicked once.
    "basic-event-dataset" => [
        "tapped": [Tap(25.0, 25.0)],
    ],
    // `#target`, clicked twice.
    "basic-event-target-id" => [
        "tapped": [Tap(50.0, 50.0)],
        "tapped-again": [Tap(50.0, 50.0)],
    ],
    // Upstream opens this card under two test names, each from a fresh page: one
    // clicks `#target1` (the 400x400 box, at a point clear of its child), the other
    // `#target2`, the 50x50 child at the origin.
    "basic-event-trigger" => [
        "target1-tapped": [Tap(200.0, 200.0)],
        "target2-tapped": [Fresh, Tap(25.0, 25.0)],
    ],
    // `#target`, clicked twice.
    "basic-global-bind" => [
        "tapped": [Tap(50.0, 50.0)],
        "tapped-again": [Tap(50.0, 50.0)],
    ],
    // `#target2`, the second 100x100 box of the lazy row, clicked twice.
    "basic-lazy-component" => [
        "tapped": [Tap(150.0, 50.0)],
        "tapped-again": [Tap(150.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "basic-lazy-component-css-selector-false-exchange-class" => [
        "class-exchanged": [Tap(50.0, 50.0)],
        "class-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "basic-lazy-component-css-selector-false-inline-css-change-same-time" => [
        "inline-and-class-added": [Tap(50.0, 50.0)],
        "inline-and-class-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked four times.
    "basic-lazy-component-css-selector-false-inline-remove-css-remove-inline" => [
        "inline-red": [Tap(50.0, 50.0)],
        "green-class-removed": [Tap(50.0, 50.0)],
        "inline-removed": [Tap(50.0, 50.0)],
        "all-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked once.
    "basic-lazy-component-css-selector-false-remove-all" => [
        "classes-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "basic-lazy-component-css-selector-false-remove-css-and-reuse-css" => [
        "green-class-added": [Tap(50.0, 50.0)],
        "green-class-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "basic-lazy-component-css-selector-false-remove-css-and-style-collapsed" => [
        "inline-yellow": [Tap(50.0, 50.0)],
        "inline-green": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "basic-lazy-component-css-selector-false-remove-inline-style-and-reuse-css" => [
        "inline-red": [Tap(50.0, 50.0)],
        "inline-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked once.
    "basic-lazy-component-mts-bindtap" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    // `#target2` of the first lazy row, then of the second, 100px below.
    "basic-lazy-component-multi" => [
        "first-tapped": [Tap(150.0, 50.0)],
        "second-tapped": [Tap(150.0, 150.0)],
    ],
    // `#target2` of the first lazy row, then of the second, 100px below.
    "basic-lazy-component-multi-import" => [
        "first-tapped": [Tap(150.0, 50.0)],
        "second-tapped": [Tap(150.0, 150.0)],
    ],
    // `#target2`, the second 100x100 box of the lazy row, clicked twice.
    "basic-lazy-component-relative-path" => [
        "tapped": [Tap(150.0, 50.0)],
        "tapped-again": [Tap(150.0, 50.0)],
    ],
    // `#target` (the red box under the first lazy row) mounts a second row below
    // itself; each row's `#target2` is at x 100..200.
    "basic-lazy-component-when-need-with-itself" => [
        "second-shown": [Tap(50.0, 150.0)],
        "first-tapped": [Tap(150.0, 50.0)],
        "shown-again": [Tap(50.0, 150.0)],
        "second-tapped": [Tap(150.0, 250.0)],
    ],
    // `#target` asks for the lazy row, which mounts under it; then that row's
    // `#target2`, twice.
    "basic-lazy-component-when-needed" => [
        "loaded": [Tap(50.0, 50.0)],
        "tapped": [Tap(150.0, 150.0)],
        "tapped-again": [Tap(150.0, 150.0)],
    ],
    // `#target` is tapped, then `#reload`, the second 100x100 box, whose handler
    // calls `lynx.reload()`.
    "basic-lynx-reload" => [
        "tapped": [Tap(50.0, 50.0)],
        "reloaded": [Tap(50.0, 150.0)],
    ],
    // `#tap-me`. Upstream reads `scrollTop` three seconds into an auto-scroll.
    "basic-main-query-selector" => [
        "auto-scrolling": [Tap(196.5, 55.0)],
    ],
    // `#target`, clicked once.
    "basic-mts-bindtap" => [
        "tapped": [Tap(50.0, 50.0), Logged("hello world")],
    ],
    // `#target`, clicked once.
    "basic-mts-bindtap-change-element-background" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    // A finger swipes 10px to the right from (20, 20), inside `#target`.
    "basic-mts-bindtouchstart" => [
        "swiped": [Drag((20.0, 20.0), (30.0, 20.0))],
    ],
    // `#target`, clicked once.
    "basic-mts-mainthread-ref" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked once.
    "basic-mts-run-on-background" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked once.
    "basic-mts-systeminfo" => [
        "tapped": [Tap(50.0, 50.0), Logged("hello world")],
    ],
    // `#target`, clicked once.
    "basic-page-event" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked once.
    "basic-ref-main-invoke-ui-method" => [
        "auto-scrolled": [Tap(196.5, 55.0)],
    ],
    // `#target` is tapped, then the host reloads.
    "basic-reload" => [
        "tapped": [Tap(50.0, 50.0)],
        "reloaded": [Reload],
    ],
    // `#target`, clicked twice.
    "basic-style-remove" => [
        "inline-removed": [Tap(50.0, 50.0)],
        "inline-restored": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "basic-style-remove-one-property" => [
        "background-removed": [Tap(50.0, 50.0)],
        "background-restored": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "config-css-selector-false-exchange-class" => [
        "class-exchanged": [Tap(50.0, 50.0)],
        "class-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "config-css-selector-false-inline-css-change-same-time" => [
        "inline-and-class-added": [Tap(50.0, 50.0)],
        "inline-and-class-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked four times.
    "config-css-selector-false-inline-remove-css-remove-inline" => [
        "inline-added": [Tap(50.0, 50.0)],
        "class-removed": [Tap(50.0, 50.0)],
        "inline-removed": [Tap(50.0, 50.0)],
        "all-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked once.
    "config-css-selector-false-remove-all" => [
        "classes-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "config-css-selector-false-remove-css-and-reuse-css" => [
        "class-added": [Tap(50.0, 50.0)],
        "class-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "config-css-selector-false-remove-css-and-style-collapsed" => [
        "inline-yellow-class-green": [Tap(50.0, 50.0)],
        "inline-green-class-removed": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "config-css-selector-false-remove-inline-style-and-reuse-css" => [
        "inline-added": [Tap(50.0, 50.0)],
        "inline-removed": [Tap(50.0, 50.0)],
    ],
}
