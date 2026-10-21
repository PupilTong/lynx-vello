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
    Drag, Fill, Fresh, Hold, KeyDown, KeyUp, Logged, LoggedStart, LoggedWith, MouseDown, MouseDrag,
    MouseOver, MouseUp,
    Press, QuickCapture, Release, Reload, Select, SendGlobalEvent, Tap, UpdateData,
    UpdateGlobalProps, Wait, Wheel,
};

/// The two `change` events `basic-element-x-viewpager-ng-bindchange-select-tab`
/// logs, one per `selectTab`, up to their `timestamp`.
const SELECT_TAB_LAST: &str = concat!(
    r#"{"type":"change","eventPhase":2,"#,
    r#""target":{"dataset":{"testid":"bindchange"},"id":null,"uid":7},"#,
    r#""currentTarget":{"dataset":{"testid":"bindchange"},"id":null,"uid":7},"#,
    r#""detail":{"index":5,"isDragged":false}"#,
);
const SELECT_TAB_FIRST: &str = concat!(
    r#"{"type":"change","eventPhase":2,"#,
    r#""target":{"dataset":{"testid":"bindchange"},"id":null,"uid":7},"#,
    r#""currentTarget":{"dataset":{"testid":"bindchange"},"id":null,"uid":7},"#,
    r#""detail":{"index":0,"isDragged":false}"#,
);

/// What the two `getValue` cards log: the value, and the selection the field kept
/// when the tap on `#target` took its focus.
const GOT_VALUE: &str =
    r#"{"value":"hello","selectionStart":2,"selectionEnd":5,"isComposing":false,"selectionBegin":2}"#;

/// The start of each event `basic-element-x-textarea-bindinput` logs, up to its
/// `timestamp`.
const TEXTAREA_FOCUS: &str = concat!(
    r#"{"type":"focus","eventPhase":2,"#,
    r#""target":{"dataset":{"testid":"textarea"},"id":null,"uid":4},"#,
    r#""currentTarget":{"dataset":{"testid":"textarea"},"id":null,"uid":4},"#,
    r#""detail":{"value":""},"#,
);
const TEXTAREA_BLUR: &str = concat!(
    r#"{"type":"blur","eventPhase":2,"#,
    r#""target":{"dataset":{"testid":"textarea"},"id":null,"uid":4},"#,
    r#""currentTarget":{"dataset":{"testid":"textarea"},"id":null,"uid":4},"#,
    r#""detail":{"value":""},"#,
);
const TEXTAREA_INPUT: &str = concat!(
    r#"{"type":"input","eventPhase":2,"#,
    r#""target":{"dataset":{"testid":"textarea"},"id":null,"uid":4},"#,
    r#""currentTarget":{"dataset":{"testid":"textarea"},"id":null,"uid":4},"#,
    r#""detail":{"value":"value","textLength":5,"cursor":5,"isComposing":false,"#,
    r#""selectionStart":5,"selectionEnd":5},"#,
);

driven! {
    // Upstream's `api-shared-context`: `api-setSharedData` is booted first in the same
    // group (`COMPANIONS`), then `#target` of this second view is clicked.
    "api-getSharedData" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    // Upstream waits for the two lines the card logs, one per thread; by the 2026-10-20
    // ruling the main thread's throws instead (native), so only the background
    // thread's is awaited.
    "api-global-disallowed-vars" => [
        "logged": [Logged("background thread: undefined, undefined")],
    ],
    // Upstream listens for the two console lines the card writes, one from each thread,
    // each naming the module-level `globalThis.foo`.
    "api-globalThis" => [
        "logged": [Logged("mtsFoo 123"), Logged("btsFoo 123")],
    ],
    // `#tap-area`, the 100x100 box inset 50px. Upstream also offsets the whole view by
    // 200px in its host page, which a view with no embedding page has no word for.
    "api-bindtap-lynx-view-relative" => [
        "tapped": [Tap(100.0, 100.0)],
    ],
    // Upstream scrolls `#x`, the 300x500 scroll-view at the origin, to offset 200 and then
    // 400 by script; here the wheel is turned that far over it, 200px each time.
    "api-exposure-area" => [
        "scrolled-200": [Wheel((150.0, 250.0), (0.0, 200.0))],
        "scrolled-400": [Wheel((150.0, 250.0), (0.0, 200.0))],
    ],
    // Upstream scrolls `#x` (the horizontal scroll-view, y 266..500 on screen) to offset
    // 600 and back to 0, then `#y` (the vertical one, the whole 500px box) to 50 and back
    // to 0, by script; here the wheel is turned that far, over `#x` for the first two and
    // over the colour text inside `#y` for the last two. `#x` scrolls 400px at most, so
    // upstream's 600 stops at 400 too.
    "api-exposure-basic" => [
        "right-yellow": [Wheel((196.5, 390.0), (600.0, 0.0))],
        "white-back": [Wheel((196.5, 390.0), (-600.0, 0.0))],
        "red-down": [Wheel((196.5, 100.0), (0.0, 50.0))],
        "white-down-back": [Wheel((196.5, 100.0), (0.0, -50.0))],
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
    // Upstream sets `#y`, the full-viewport scroll-view, to scrollTop 200, 800 and 1000 by
    // script; here the wheel is turned by the differences over it.
    "api-exposure-dynamic-screen-margin" => [
        "orange-half-do-trigger": [Wheel((196.5, 360.0), (0.0, 200.0))],
        "green-half-not-trigger": [Wheel((196.5, 360.0), (0.0, 600.0))],
        "green-half-do-trigger": [Wheel((196.5, 360.0), (0.0, 200.0))],
    ],
    // Upstream sets `#y`, the full-viewport scroll-view, to scrollTop 200, 800 and 1000 by
    // script; here the wheel is turned by the differences over it.
    "api-exposure-dynamic-ui-margin" => [
        "orange-half-do-trigger": [Wheel((196.5, 360.0), (0.0, 200.0))],
        "green-half-not-trigger": [Wheel((196.5, 360.0), (0.0, 600.0))],
        "green-half-do-trigger": [Wheel((196.5, 360.0), (0.0, 200.0))],
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
    // Upstream scrolls the list to offset 500 by script, here the wheel is turned
    // that far over it, and upstream then asserts that the card logged a `scroll` and a
    // `scrollend` event. A logged event ends with its `timestamp`, so each is waited
    // for by the start of its line.
    "basic-element-list-basic" => [
        "scrolled": [
            Wheel((196.5, 250.0), (0.0, 500.0)),
            LoggedStart("{\"type\":\"scroll\","),
            LoggedStart("{\"type\":\"scrollend\","),
        ],
    ],
    // The same scroll over cells sized by their content.
    "basic-element-list-basic-size" => [
        "scrolled": [
            Wheel((196.5, 250.0), (0.0, 500.0)),
            LoggedStart("{\"type\":\"scroll\","),
            LoggedStart("{\"type\":\"scrollend\","),
        ],
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
    // Upstream's CDP swipe from (100, 300) up 100px over the 300x600 scroll-view, then
    // the `scroll` event the card logs and keeps.
    "basic-element-scroll-view-event-scroll" => [
        "swiped": [
            Drag((100.0, 300.0), (100.0, 200.0)),
            Wait(100),
            LoggedStart("{\"type\":\"scroll\","),
        ],
    ],
    // Upstream sets the scroll-view's `scrollTop` to 200 by script; here the wheel is
    // turned that far over it. Then the `scrollend` event the card logs and keeps.
    "basic-element-scroll-view-event-scrollend" => [
        "scrolled": [
            Wheel((150.0, 300.0), (0.0, 200.0)),
            LoggedStart("{\"type\":\"scrollend\","),
        ],
    ],
    // Upstream's two swipes: from (100, 300) up 200px, scrolling down, then from
    // (100, 100) down 200px, back to the top; it reads the logged `scrolltoupper`.
    "basic-element-scroll-view-event-scrolltoupper" => [
        "swiped-to-upper": [
            Drag((100.0, 300.0), (100.0, 100.0)),
            Wait(200),
            Drag((100.0, 100.0), (100.0, 300.0)),
            Wait(600),
            LoggedStart("{\"type\":\"scrolltoupper\","),
        ],
    ],
    // Upstream's two CDP swipes: from (100, 500) up 450px, 100 ms apart.
    "basic-element-scroll-view-event-scrolltolower" => [
        "swiped-to-lower": [
            Drag((100.0, 500.0), (100.0, 50.0)),
            Wait(100),
            Drag((100.0, 500.0), (100.0, 50.0)),
            LoggedStart("{\"type\":\"scrolltolower\","),
            Wait(100),
        ],
    ],
    // Upstream selects `inline`'s characters 1 to 4 of `#inline-target` with
    // `setBaseAndExtent`, forward and then backward. Here the mouse drags across the
    // same characters (16px text: `before` ends at 41.9, `n` starts at 48.9 and the
    // second `i` ends at 64.9, on the first line), each way.
    "basic-element-text-bindselectionchange" => [
        "forward": [MouseDrag((48.9, 9.0), (64.9, 9.0))],
        "backward": [MouseDrag((64.9, 9.0), (48.9, 9.0))],
    ],
    // Upstream opens the card four times and `selectText`s one of the four texts each
    // time. Here the mouse drags from the left half of that text's first glyph, so
    // from its start, to the end of its last line, on a fresh card each time; the two with `text-selection` false select
    // nothing.
    "basic-element-text-text-selection" => [
        "true-boolean": [MouseDrag((0.5, 9.0), (96.0, 40.0))],
        "true-string": [Fresh, MouseDrag((101.0, 9.0), (181.0, 57.0))],
        "false-string": [Fresh, MouseDrag((195.5, 9.0), (276.0, 57.0))],
        "false-boolean": [Fresh, MouseDrag((291.5, 9.0), (390.0, 40.0))],
    ],
    // `#tap`, clicked once.
    "basic-element-x-foldview-ng-method-setFoldExpanded" => [
        "folded-by-method": [Tap(113.2, 450.0)],
    ],
    // `#tap`, clicked once.
    "basic-element-x-foldview-ng-method-setFoldExpanded-overflow" => [
        "folded-to-limit": [Tap(141.5, 450.0)],
    ],
    // `input.press('Enter')`, then `input.fill('foobar')`. The card's `<input>` is
    // `<x-input>` here (vendored rewrite, see the fixtures README).
    "basic-element-input-bindinput" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar")],
    ],
    // The field already has focus (`focus`); a click outside it, on nothing focusable,
    // takes it away. Upstream has no spec for this card.
    "basic-element-x-input-bindblur" => [
        "blurred": [Tap(200.0, 400.0)],
    ],
    // `input.press('Enter')`: the field is clicked, which is how a press reaches it,
    // then Enter.
    "basic-element-x-input-bindconfirm" => [
        "confirmed": [Tap(150.0, 20.0), Press("Enter", "Enter", false)],
    ],
    // `input.click()`.
    "basic-element-x-input-bindfocus" => [
        "focused": [Tap(150.0, 20.0)],
    ],
    // `input.press('Enter')`, then `input.fill('foobar')`.
    "basic-element-x-input-bindinput" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar")],
    ],
    // The inner `<input>` focused, then `setSelectionRange(2, 5)`.
    "basic-element-x-input-bindselection" => [
        "selected": [Tap(150.0, 20.0), Select(2, 5)],
    ],
    // `.blur`, the red box over the focused field, clicked.
    "basic-element-x-input-blur" => [
        "blurred": [Tap(50.0, 50.0)],
    ],
    // `.focus`, the red box over the field, clicked.
    "basic-element-x-input-focus" => [
        "focused": [Tap(50.0, 50.0)],
    ],
    // The inner `<input>` focused, `setSelectionRange(2, 5)`, then `#target`, the red
    // box over it, clicked. Upstream clicks with a script `click()`, which keeps the
    // field focused; a tap takes focus, and the field answers the selection it kept.
    "basic-element-x-input-getValue" => [
        "got": [Tap(150.0, 120.0), Select(2, 5), Tap(50.0, 50.0), Logged(GOT_VALUE)],
    ],
    // `input.press('Enter')`, then `input.fill('foobar!@#)')`.
    "basic-element-x-input-input-filter" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar!@#)")],
    ],
    // `input.press('Enter')`, then `input.fill('foobar')`.
    "basic-element-x-input-ng-bindinput" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar")],
    ],
    // `.click`, the red box, clicked; then `.clear`, the blue box under it, whose
    // handler invokes the field's `setValue('')`. Upstream has no spec for this card.
    "basic-element-x-input-setValue" => [
        "clicked": [Tap(50.0, 50.0)],
        "cleared": [Tap(50.0, 150.0)],
    ],
    // `.click`, the red box, clicked. Upstream has no spec for this card.
    "basic-element-x-input-value" => [
        "clicked": [Tap(50.0, 50.0)],
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
    // again 2 s after letting go; then the same upward by 200px.
    "basic-element-x-refresh-view-demo" => [
        "pull-down": [Hold((200.0, 500.0), (200.0, 600.0))],
        "pull-down-loaded": [Release(200.0, 600.0), Wait(2000)],
        "pull-up": [Hold((200.0, 600.0), (200.0, 400.0))],
        "pull-up-loaded": [Release(200.0, 400.0), Wait(2000)],
    ],
    // Upstream waits out one 5 s interval with `autoplay` off, clicks the 10x10 button
    // at the top right to turn it on, waits two intervals, and clicks it off again.
    "basic-element-x-swiper-autoplay" => [
        "idle-one-interval": [Wait(5600)],
        "autoplayed-to-1": [Tap(388.0, 5.0), Wait(5600)],
        "autoplayed-to-2": [Wait(5600)],
        "autoplay-toggled-off": [Tap(388.0, 5.0), Wait(5500)],
    ],
    // The `next` button under the three swipers (y 450..460), which writes the
    // `programming` swiper's `current`; a wait for two autoplay turns; then a swipe
    // left on the first, `manual`, swiper. Upstream asserts at the end on the logged
    // events. Its swipe is a fast touch, where a fling's reach depends on the
    // wall-clock spacing of the moves; here the finger drags 300px, well past half
    // the 393px page, and stays still before it lifts, so the swiper snaps to item 1
    // with no fling and the frame is the same every run.
    "basic-element-x-swiper-bindchange" => [
        "paged": [
            Tap(5.0, 455.0),
            Wait(6600),
            Hold((350.0, 50.0), (50.0, 50.0)),
            Wait(300),
            Release(50.0, 50.0),
            Wait(1000),
            LoggedWith(&[r#""type":"change""#, r#""testid":"programming""#, r#""detail":{"current":1,"#]),
            LoggedWith(&[r#""type":"change""#, r#""testid":"autoplay""#, r#""detail":{"current":2,"#]),
            LoggedWith(&[r#""type":"change""#, r#""testid":"autoplay""#, r#""detail":{"current":3,"#]),
            LoggedWith(&[r#""type":"change""#, r#""testid":"manual""#, r#""detail":{"current":1,"#]),
        ],
    ],
    // The same sequence; upstream reads the logged `scrollend` events.
    "basic-element-x-swiper-bindscrollend" => [
        "paged": [
            Tap(5.0, 455.0),
            Wait(6600),
            Hold((350.0, 50.0), (50.0, 50.0)),
            Wait(300),
            Release(50.0, 50.0),
            Wait(1000),
            LoggedWith(&[r#""type":"scrollend""#, r#""testid":"programming""#, r#""detail":{"current":1}"#]),
            LoggedWith(&[r#""type":"scrollend""#, r#""testid":"autoplay""#, r#""detail":{"current":2}"#]),
            LoggedWith(&[r#""type":"scrollend""#, r#""testid":"autoplay""#, r#""detail":{"current":3}"#]),
            LoggedWith(&[r#""type":"scrollend""#, r#""testid":"manual""#]),
        ],
    ],
    // The same sequence; upstream reads the logged `scrollstart` events: the
    // `programming` and `autoplay` swipers' not dragged, the `manual` one's dragged.
    "basic-element-x-swiper-bindscrollstart" => [
        "paged": [
            Tap(5.0, 455.0),
            Wait(6600),
            Hold((350.0, 50.0), (50.0, 50.0)),
            Wait(300),
            Release(50.0, 50.0),
            Wait(1000),
            LoggedWith(&[r#""type":"scrollstart""#, r#""testid":"programming""#, r#""detail":{"current":0,"isDragged":false}"#]),
            LoggedWith(&[r#""type":"scrollstart""#, r#""testid":"autoplay""#, r#""detail":{"current":0,"isDragged":false}"#]),
            LoggedWith(&[r#""type":"scrollstart""#, r#""testid":"autoplay""#, r#""detail":{"current":1,"isDragged":false}"#]),
            LoggedWith(&[r#""type":"scrollstart""#, r#""testid":"autoplay""#, r#""detail":{"current":2,"isDragged":false}"#]),
            LoggedWith(&[r#""type":"scrollstart""#, r#""testid":"manual""#, r#""detail":{"current":0,"isDragged":true}"#]),
        ],
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
    // Upstream waits 5.6 s for one 5 s autoplay turn, then clicks the two 10x10 buttons
    // under the swiper, `interval` 1000 (y 150..160) and `interval` 0 (y 160..170),
    // waiting 1.6 s and 0.6 s after them.
    // Once `interval` is 1000 the swiper turns every second, each turn a half-second
    // glide, so the screen is quiet for half a second at a time. Upstream's screenshot
    // is 1.6 s after the click: here 1.1 s of wait and the 0.5 s settle every frame
    // has, and a short stability check, so the capture falls inside that quiet spell.
    "basic-element-x-swiper-interval" => [
        // Upstream's 5.6 s count from the card's load, so the frame starts the card
        // again: counted from the first screen, whose capture takes a varying time,
        // the wait could span one turn or two.
        "second-item": [Fresh, Wait(5100)],
        "third-item": [Tap(5.0, 155.0), Wait(1100), QuickCapture],
        "last-item": [Tap(5.0, 165.0), Wait(600)],
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
    // The first `.block`, the 200px sky-blue box, clicked twice.
    "basic-element-x-textarea-disabled" => [
        "disabled": [Tap(100.0, 100.0)],
        "enabled": [Tap(100.0, 100.0)],
    ],
    // `textarea.click()`, `textarea.blur()` (a click outside it), then
    // `textarea.fill('value')`. Upstream listens for the three events the card logs:
    // `focus` and `blur` with an empty value, `input` with `value` and the caret after it.
    "basic-element-x-textarea-bindinput" => [
        "focused": [Tap(150.0, 15.0), LoggedStart(TEXTAREA_FOCUS)],
        "blurred": [Tap(200.0, 400.0), LoggedStart(TEXTAREA_BLUR)],
        "filled": [Tap(150.0, 15.0), Fill("value"), LoggedStart(TEXTAREA_INPUT)],
    ],
    // The inner `<textarea>` focused, then `setSelectionRange(2, 5)`.
    "basic-element-x-textarea-bindselection" => [
        "selected": [Tap(150.0, 20.0), Select(2, 5)],
    ],
    // The inner `<textarea>` focused, `setSelectionRange(2, 5)`, then `#target`, the
    // red box over it, clicked, as for `basic-element-x-input-getValue`.
    "basic-element-x-textarea-getValue" => [
        "got": [Tap(150.0, 120.0), Select(2, 5), Tap(50.0, 50.0), Logged(GOT_VALUE)],
    ],
    // `textarea.press('Enter')`, then `textarea.fill('foobar!@#)')`.
    "basic-element-x-textarea-input-filter" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar!@#)")],
    ],
    // The six 5px `.block`s, `setValue` to `setValueAndLength`, each clicked once.
    "basic-element-x-textarea-maxlength" => [
        "dynamic": [
            Tap(100.0, 2.5),
            Tap(100.0, 8.5),
            Tap(100.0, 14.5),
            Tap(100.0, 20.5),
            Tap(100.0, 26.5),
            Tap(100.0, 32.5),
        ],
    ],
    // `setValue`, the 50px `.block`, clicked.
    "basic-element-x-textarea-maxlines" => [
        "updated": [Tap(100.0, 25.0)],
    ],
    // `setValue`, the 50px `.block`, clicked.
    "basic-element-x-textarea-min-height-max-height" => [
        "updated": [Tap(100.0, 25.0)],
    ],
    // The `.block`, the 200px sky-blue box, clicked.
    "basic-element-x-textarea-placeholder" => [
        "updated": [Tap(100.0, 100.0)],
    ],
    // The `.block`, the 200px sky-blue box, clicked.
    "basic-element-x-textarea-placeholder-style" => [
        "updated": [Tap(100.0, 100.0)],
    ],
    // `allow-horizontal-gesture={false}`: upstream swipes from (100, 50) right 200px and
    // expects the pager not to move.
    "basic-element-x-viewpager-ng-allow-horizontal-gesture" => [
        "swiped": [Drag((100.0, 50.0), (300.0, 50.0)), Wait(1000)],
    ],
    // Upstream swipes from (300, 50) left 250px and reads the last logged `change`:
    // index 1, with `isDragged` present.
    "basic-element-x-viewpager-ng-bindchange" => [
        "swiped": [
            Drag((300.0, 50.0), (50.0, 50.0)),
            Wait(1000),
            Logged("{\"index\":1,\"isDragged\":false}"),
        ],
    ],
    // The control texts `last` (y 16..32), then `first` (y 0..16).
    "basic-element-x-viewpager-ng-bindchange-select-tab" => [
        "last-selected": [Tap(196.5, 24.0), LoggedStart(SELECT_TAB_LAST)],
        "first-selected": [Tap(196.5, 8.0), LoggedStart(SELECT_TAB_FIRST)],
    ],
    // Upstream swipes from (300, 50) left 250px and expects at least one logged
    // `offsetchange`.
    "basic-element-x-viewpager-ng-bindoffsetchange" => [
        "swiped": [
            Drag((300.0, 50.0), (50.0, 50.0)),
            Wait(1000),
            LoggedStart("{\"offset\":"),
        ],
    ],
    // Upstream's two swipes over the pager (x 39..353, y 80..580): 200px to the left from
    // (300, 200), then 200px to the right from (50, 200).
    "basic-element-x-viewpager-ng-exposure" => [
        "exposure-1": [Drag((300.0, 200.0), (100.0, 200.0))],
        "exposure-2": [Drag((50.0, 200.0), (250.0, 200.0))],
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
    // `#tap-me`. Upstream reads `scrollTop` three seconds into an auto-scroll at 120 px/s;
    // a screen that is still scrolling cannot be pinned, so the frame is taken after the
    // run has reached the end of the 2200px list and stopped there (`autoStop`), which
    // takes about 16 s.
    "basic-main-query-selector" => [
        "auto-scrolled-to-end": [Tap(196.5, 55.0), Wait(17000)],
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
    // `#target`, clicked once. Upstream reads `scrollTop` two seconds into an auto-scroll
    // at 120 px/s; the five items scroll 230px, so the run has reached the end and
    // stopped before the frame settles.
    "basic-ref-main-invoke-ui-method" => [
        "auto-scrolled": [Tap(196.5, 55.0)],
    ],
    // `#target` is tapped, then the host reloads.
    "basic-reload" => [
        "tapped": [Tap(50.0, 50.0)],
        "reloaded": [Reload],
    ],
    // `#target`, clicked twice.
    // Upstream hovers `#target`, the red 100x100 box at the origin, then presses and
    // releases the mouse button there.
    "basic-bindmouse" => [
        "pressed-and-released": [MouseOver(50.0, 50.0), MouseDown(50.0, 50.0), MouseUp(50.0, 50.0)],
    ],
    // Upstream presses `a`; the view's own `bindKeydown` must not hear a key nothing
    // has focused.
    "basic-bindkeydown-out-of-view-noop" => [
        "pressed-a": [Press("a", "KeyA", false)],
    ],
    // Upstream presses `a`.
    "basic-global-bindkeydown" => [
        "pressed-a": [Press("a", "KeyA", false)],
    ],
    // Upstream presses `a` and reads `code`.
    "basic-global-bindkeydown-code" => [
        "pressed-a": [Press("a", "KeyA", false)],
    ],
    // Upstream presses `Enter` and reads `key`.
    "basic-global-bindkeydown-key" => [
        "pressed-enter": [Press("Enter", "Enter", false)],
    ],
    // Upstream presses `Shift+a`: here the `a` key with Shift held, which is what the
    // card reads (`shiftKey`).
    "basic-global-bindkeydown-shift" => [
        "pressed-shift-a": [Press("A", "KeyA", true)],
    ],
    // Upstream puts `a` down, checks nothing changed yet, and lets it go.
    "basic-global-bindkeyup" => [
        "a-down": [KeyDown("a", "KeyA")],
        "a-up": [KeyUp("a", "KeyA")],
    ],
    // Upstream's `dragAndHold` from (300, 50) left 250px, the finger kept down, then the
    // screenshot; here the finger is lifted for a last frame as well.
    "basic-element-x-swiper-mode-carry" => [
        "held": [Hold((300.0, 50.0), (50.0, 50.0))],
        "released": [Release(50.0, 50.0)],
    ],
    "basic-style-remove" => [
        "inline-removed": [Tap(50.0, 50.0)],
        "inline-restored": [Tap(50.0, 50.0)],
    ],
    // `#target`, clicked twice.
    "basic-style-remove-one-property" => [
        "background-removed": [Tap(50.0, 50.0)],
        "background-restored": [Tap(50.0, 50.0)],
    ],
    // `#update`, the last row (the text `Update`, y 177..193): upstream clicks it and
    // asserts what `#solid-text` computes afterwards.
    "config-css-inheritance-default" => [
        "updated": [Tap(20.0, 185.0)],
    ],
    "config-css-inheritance-false" => [
        "updated": [Tap(20.0, 185.0)],
    ],
    "config-css-inheritance-true" => [
        "updated": [Tap(20.0, 185.0)],
    ],
}
