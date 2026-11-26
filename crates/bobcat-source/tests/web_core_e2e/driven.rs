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

const GOT_VALUE: &str =
    r#"{"value":"hello","selectionStart":2,"selectionEnd":5,"isComposing":false,"selectionBegin":2}"#;

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
    "api-getSharedData" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    "api-global-disallowed-vars" => [
        "logged": [Logged("background thread: undefined, undefined")],
    ],
    "api-globalThis" => [
        "logged": [Logged("mtsFoo 123"), Logged("btsFoo 123")],
    ],
    "api-bindtap-lynx-view-relative" => [
        "tapped": [Tap(100.0, 100.0)],
    ],
    "api-exposure-area" => [
        "scrolled-200": [Wheel((150.0, 250.0), (0.0, 200.0))],
        "scrolled-400": [Wheel((150.0, 250.0), (0.0, 200.0))],
    ],
    "api-exposure-basic" => [
        "right-yellow": [Wheel((196.5, 390.0), (600.0, 0.0))],
        "white-back": [Wheel((196.5, 390.0), (-600.0, 0.0))],
        "red-down": [Wheel((196.5, 100.0), (0.0, 50.0))],
        "white-down-back": [Wheel((196.5, 100.0), (0.0, -50.0))],
    ],
    "api-exposure-change-exposure-id" => [
        "index-1": [Tap(60.0, 19.0)],
        "index-2": [Tap(180.0, 19.0)],
        "index-3": [Tap(300.0, 19.0)],
        "index-4": [Tap(382.0, 19.0)],
    ],
    "api-exposure-dynamic-screen-margin" => [
        "orange-half-do-trigger": [Wheel((196.5, 360.0), (0.0, 200.0))],
        "green-half-not-trigger": [Wheel((196.5, 360.0), (0.0, 600.0))],
        "green-half-do-trigger": [Wheel((196.5, 360.0), (0.0, 200.0))],
    ],
    "api-exposure-dynamic-ui-margin" => [
        "orange-half-do-trigger": [Wheel((196.5, 360.0), (0.0, 200.0))],
        "green-half-not-trigger": [Wheel((196.5, 360.0), (0.0, 600.0))],
        "green-half-do-trigger": [Wheel((196.5, 360.0), (0.0, 200.0))],
    ],
    "api-exposure-stop-events-has-complex-dataset" => [
        "disexposed": [Tap(5.0, 5.0), Logged("pass:dataset2")],
    ],
    "api-exposure-stop-events-has-dataset" => [
        "disexposed": [Tap(5.0, 5.0), Logged("pass:dataset1"), Logged("pass:dataset2")],
    ],
    "api-exposure-stop-exposure" => [
        "stopped": [Tap(196.5, 452.5)],
        "stopped-without-event": [Tap(196.5, 592.5)],
        "stopped-with-event": [Tap(196.5, 522.5)],
    ],
    "api-frame-data-update" => [
        "data-updated": [Tap(196.5, 128.0)],
    ],
    "api-invoke-fail" => [
        "tapped": [Tap(5.0, 45.0)],
    ],
    "api-invoke-success" => [
        "tapped": [Tap(5.0, 45.0)],
    ],
    "api-nativemodules-call-delay" => [
        "answered": [Wait(3000)],
    ],
    "api-requestAnimationFrame" => [
        "looping": [Tap(196.5, 24.0)],
        "stopped": [Tap(196.5, 24.0)],
    ],
    "api-sendGlobalEvent" => [
        "sent": [SendGlobalEvent("event-test", r#"["change"]"#)],
    ],
    "api-updateData" => [
        "updated": [UpdateData(r#"{"mockData":"updatedData"}"#)],
    ],
    "api-updateData-processData" => [
        "updated": [UpdateData(r#"{"mockData":"updatedData"}"#)],
    ],
    "api-updateGlobalProps" => [
        "blue": [UpdateGlobalProps(r#"{"backgroundColor":"blue"}"#)],
    ],
    "basic-bindtap" => [
        "tapped": [Tap(50.0, 50.0)],
        "tapped-again": [Tap(50.0, 50.0)],
    ],
    "basic-bindtap-detail" => [
        "tapped": [Tap(50.0, 50.0)],
        "tapped-again": [Tap(50.0, 50.0)],
    ],
    "basic-bindtap-simultaneous" => [
        "tapped": [Tap(50.0, 50.0), Logged("MTS Clicked")],
    ],
    "basic-bindwheel-view" => [
        "wheeled": [Wheel((80.0, 80.0), (0.0, 100.0))],
    ],
    "basic-element-image-support-tap-event" => [
        "tapped": [Tap(100.0, 100.0)],
    ],
    "basic-element-list-basic" => [
        "scrolled": [
            Wheel((196.5, 250.0), (0.0, 500.0)),
            LoggedStart("{\"type\":\"scroll\","),
            LoggedStart("{\"type\":\"scrollend\","),
        ],
    ],
    "basic-element-list-basic-size" => [
        "scrolled": [
            Wheel((196.5, 250.0), (0.0, 500.0)),
            LoggedStart("{\"type\":\"scroll\","),
            LoggedStart("{\"type\":\"scrollend\","),
        ],
    ],
    "basic-element-list-bindwheel" => [
        "wheeled": [Wheel((120.0, 120.0), (0.0, 100.0))],
    ],
    "basic-element-list-estimated-main-axis-size-px" => [
        "reached": [Wheel((196.5, 250.0), (0.0, 5000.0))],
        "measured": [Wheel((196.5, 250.0), (0.0, 5000.0))],
    ],
    "basic-element-list-remove-action" => [
        "item-4-swapped-in": [Tap(50.0, 50.0)],
        "items-2-3-restored": [Tap(50.0, 50.0)],
    ],
    "basic-element-list-scroll-to-position" => [
        "scrolled-to-position": [Tap(196.5, 508.0)],
    ],
    "basic-element-scroll-view-event-scroll" => [
        "swiped": [
            Hold((100.0, 300.0), (100.0, 200.0)),
            Wait(300),
            Release(100.0, 200.0),
            Wait(100),
            LoggedStart("{\"type\":\"scroll\","),
        ],
    ],
    "basic-element-scroll-view-event-scrollend" => [
        "scrolled": [
            Wheel((150.0, 300.0), (0.0, 200.0)),
            LoggedStart("{\"type\":\"scrollend\","),
        ],
    ],
    "basic-element-scroll-view-event-scrolltoupper" => [
        "swiped-to-upper": [
            Drag((100.0, 300.0), (100.0, 100.0)),
            Wait(200),
            Drag((100.0, 100.0), (100.0, 300.0)),
            Wait(600),
            LoggedStart("{\"type\":\"scrolltoupper\","),
        ],
    ],
    "basic-element-scroll-view-event-scrolltolower" => [
        "swiped-to-lower": [
            Drag((100.0, 500.0), (100.0, 50.0)),
            Wait(100),
            Drag((100.0, 500.0), (100.0, 50.0)),
            LoggedStart("{\"type\":\"scrolltolower\","),
            Wait(100),
        ],
    ],
    "basic-element-text-bindselectionchange" => [
        "forward": [MouseDrag((48.9, 9.0), (64.9, 9.0))],
        "backward": [MouseDrag((64.9, 9.0), (48.9, 9.0))],
    ],
    "basic-element-text-text-selection" => [
        "true-boolean": [MouseDrag((0.5, 9.0), (96.0, 40.0))],
        "true-string": [Fresh, MouseDrag((101.0, 9.0), (181.0, 57.0))],
        "false-string": [Fresh, MouseDrag((195.5, 9.0), (276.0, 57.0))],
        "false-boolean": [Fresh, MouseDrag((291.5, 9.0), (390.0, 40.0))],
    ],
    "basic-element-x-foldview-ng-method-setFoldExpanded" => [
        "folded-by-method": [Tap(113.2, 450.0)],
    ],
    "basic-element-x-foldview-ng-method-setFoldExpanded-overflow" => [
        "folded-to-limit": [Tap(141.5, 450.0)],
    ],
    "basic-element-input-bindinput" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar")],
    ],
    "basic-element-x-input-bindblur" => [
        "blurred": [Tap(200.0, 400.0)],
    ],
    "basic-element-x-input-bindconfirm" => [
        "confirmed": [Tap(150.0, 20.0), Press("Enter", "Enter", false)],
    ],
    "basic-element-x-input-bindfocus" => [
        "focused": [Tap(150.0, 20.0)],
    ],
    "basic-element-x-input-bindinput" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar")],
    ],
    "basic-element-x-input-bindselection" => [
        "selected": [Tap(150.0, 20.0), Select(2, 5)],
    ],
    "basic-element-x-input-blur" => [
        "blurred": [Tap(50.0, 50.0)],
    ],
    "basic-element-x-input-focus" => [
        "focused": [Tap(50.0, 50.0)],
    ],
    "basic-element-x-input-getValue" => [
        "got": [Tap(150.0, 120.0), Select(2, 5), Tap(50.0, 50.0), Logged(GOT_VALUE)],
    ],
    "basic-element-x-input-input-filter" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar!@#)")],
    ],
    "basic-element-x-input-ng-bindinput" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar")],
    ],
    "basic-element-x-input-setValue" => [
        "clicked": [Tap(50.0, 50.0)],
        "cleared": [Tap(50.0, 150.0)],
    ],
    "basic-element-x-input-value" => [
        "clicked": [Tap(50.0, 50.0)],
    ],
    "basic-element-x-overlay-ng-counter-test2" => [
        "all-shown": [Tap(300.0, 400.0), Wait(100), Tap(300.0, 450.0), Wait(100), Tap(300.0, 500.0), Wait(100), Tap(300.0, 550.0)],
        "all-closed": [Tap(300.0, 350.0)],
        "count-1": [Fresh, Tap(300.0, 380.0), Wait(50), Tap(50.0, 300.0), Wait(50), Tap(300.0, 380.0)],
        "count-3": [Tap(300.0, 450.0), Wait(50), Tap(50.0, 300.0), Wait(50), Tap(300.0, 450.0)],
        "count-6": [Tap(300.0, 500.0), Wait(50), Tap(50.0, 300.0), Wait(50), Tap(300.0, 500.0)],
        "count-10": [Tap(300.0, 550.0), Wait(50), Tap(50.0, 270.0), Wait(50), Tap(300.0, 550.0)],
    ],
    "basic-element-x-overlay-ng-demo" => [
        "overlay-shown": [Tap(10.0, 10.0)],
        "overlay-hidden": [Tap(200.0, 50.0)],
    ],
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
    "basic-element-x-refresh-view-demo" => [
        "pull-down": [Hold((200.0, 500.0), (200.0, 600.0))],
        "pull-down-loaded": [Release(200.0, 600.0), Wait(2000)],
        "pull-up": [Hold((200.0, 600.0), (200.0, 400.0))],
        "pull-up-loaded": [Release(200.0, 400.0), Wait(2000)],
    ],
    "basic-element-x-swiper-autoplay" => [
        "idle-one-interval": [Wait(5600)],
        "autoplayed-to-1": [Tap(388.0, 5.0), Wait(5600)],
        "autoplayed-to-2": [Wait(5600)],
        "autoplay-toggled-off": [Tap(388.0, 5.0), Wait(5500)],
    ],
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
    "basic-element-x-swiper-circular-carousel" => [
        "current-1": [Tap(5.0, 155.0)],
        "current-2": [Tap(5.0, 155.0)],
        "current-3": [Tap(5.0, 155.0)],
        "wrapped-to-0": [Tap(5.0, 155.0)],
    ],
    "basic-element-x-swiper-circular-carry" => [
        "current-1": [Tap(5.0, 155.0)],
        "current-2": [Tap(5.0, 155.0)],
        "current-3": [Tap(5.0, 155.0)],
        "wrapped-to-0": [Tap(5.0, 155.0)],
    ],
    "basic-element-x-swiper-circular-coverflow" => [
        "current-1": [Tap(5.0, 155.0)],
        "current-2": [Tap(5.0, 155.0)],
        "current-3": [Tap(5.0, 155.0)],
        "wrapped-to-0": [Tap(5.0, 155.0)],
    ],
    "basic-element-x-swiper-circular-flat-coverflow" => [
        "current-1": [Tap(5.0, 5.0)],
        "current-2": [Tap(5.0, 5.0)],
        "current-3": [Tap(5.0, 5.0)],
        "wrapped-to-0": [Tap(5.0, 5.0)],
    ],
    "basic-element-x-swiper-circular-normal" => [
        "current-1": [Tap(5.0, 155.0)],
        "current-2": [Tap(5.0, 155.0)],
        "current-3": [Tap(5.0, 155.0)],
        "wrapped-to-0": [Tap(5.0, 155.0)],
    ],
    "basic-element-x-swiper-current" => [
        "current-1": [Tap(294.75, 75.0)],
        "current-2": [Tap(294.75, 75.0)],
        "current-3": [Tap(294.75, 75.0)],
        "current-4-out-of-range": [Tap(294.75, 75.0)],
    ],
    "basic-element-x-swiper-duration" => [
        "current-1": [Tap(191.5, 75.0)],
        "current-2": [Tap(191.5, 75.0)],
        "current-3-duration-100": [Tap(388.0, 5.0), Tap(191.5, 75.0)],
        "current-4-out-of-range": [Tap(191.5, 75.0)],
    ],
    "basic-element-x-swiper-interval" => [
        "second-item": [Fresh, Wait(5100)],
        "third-item": [Tap(5.0, 155.0), Wait(1100), QuickCapture],
        "last-item": [Tap(5.0, 165.0), Wait(600)],
    ],
    "basic-element-x-swiper-method-scroll-to" => [
        "scrolled-to-1": [Tap(294.75, 75.0)],
        "scrolled-to-2": [Tap(294.75, 75.0)],
    ],
    "basic-element-x-swiper-mode-carousel" => [
        "last-item": [Tap(196.5, 75.0)],
    ],
    "basic-element-x-swiper-mode-coverflow" => [
        "last-item": [Tap(196.5, 75.0)],
    ],
    "basic-element-x-swiper-mode-flat-coverflow" => [
        "last-item": [Tap(196.5, 75.0)],
    ],
    "basic-element-x-swiper-mode-normal" => [
        "last-item": [Tap(196.5, 75.0)],
    ],
    "basic-element-x-swiper-page-margin" => [
        "current-1": [Tap(5.0, 5.0)],
    ],
    "basic-element-x-swiper-swiper-dynamic" => [
        "index-1": [Tap(294.75, 75.0)],
        "index-2": [Tap(294.75, 75.0)],
        "unmounted": [Tap(98.25, 75.0)],
        "remounted": [Tap(196.5, 75.0)],
    ],
    "basic-element-x-swiper-vertical" => [
        "current-1": [Tap(5.0, 5.0)],
    ],
    "basic-element-x-textarea-disabled" => [
        "disabled": [Tap(100.0, 100.0)],
        "enabled": [Tap(100.0, 100.0)],
    ],
    "basic-element-x-textarea-bindinput" => [
        "focused": [Tap(150.0, 15.0), LoggedStart(TEXTAREA_FOCUS)],
        "blurred": [Tap(200.0, 400.0), LoggedStart(TEXTAREA_BLUR)],
        "filled": [Tap(150.0, 15.0), Fill("value"), LoggedStart(TEXTAREA_INPUT)],
    ],
    "basic-element-x-textarea-bindselection" => [
        "selected": [Tap(150.0, 20.0), Select(2, 5)],
    ],
    "basic-element-x-textarea-getValue" => [
        "got": [Tap(150.0, 120.0), Select(2, 5), Tap(50.0, 50.0), Logged(GOT_VALUE)],
    ],
    "basic-element-x-textarea-input-filter" => [
        "filled": [Tap(150.0, 20.0), Press("Enter", "Enter", false), Fill("foobar!@#)")],
    ],
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
    "basic-element-x-textarea-maxlines" => [
        "updated": [Tap(100.0, 25.0)],
    ],
    "basic-element-x-textarea-min-height-max-height" => [
        "updated": [Tap(100.0, 25.0)],
    ],
    "basic-element-x-textarea-placeholder" => [
        "updated": [Tap(100.0, 100.0)],
    ],
    "basic-element-x-textarea-placeholder-style" => [
        "updated": [Tap(100.0, 100.0)],
    ],
    "basic-element-x-viewpager-ng-allow-horizontal-gesture" => [
        "swiped": [Drag((100.0, 50.0), (300.0, 50.0)), Wait(1000)],
    ],
    "basic-element-x-viewpager-ng-bindchange" => [
        "swiped": [
            Drag((300.0, 50.0), (50.0, 50.0)),
            Wait(1000),
            Logged("{\"index\":1,\"isDragged\":false}"),
        ],
    ],
    "basic-element-x-viewpager-ng-bindchange-select-tab" => [
        "last-selected": [Tap(196.5, 24.0), LoggedStart(SELECT_TAB_LAST)],
        "first-selected": [Tap(196.5, 8.0), LoggedStart(SELECT_TAB_FIRST)],
    ],
    "basic-element-x-viewpager-ng-bindoffsetchange" => [
        "swiped": [
            Drag((300.0, 50.0), (50.0, 50.0)),
            Wait(1000),
            LoggedStart("{\"offset\":"),
        ],
    ],
    "basic-element-x-viewpager-ng-exposure" => [
        "exposure-1": [Drag((300.0, 200.0), (100.0, 200.0))],
        "exposure-2": [Drag((50.0, 200.0), (250.0, 200.0))],
    ],
    "basic-element-x-viewpager-ng-method-selecttab" => [
        "tab-1-selected": [Tap(196.5, 75.0)],
    ],
    "basic-event-bubble-dataset" => [
        "tapped": [Tap(25.0, 25.0)],
    ],
    "basic-event-dataset" => [
        "tapped": [Tap(25.0, 25.0)],
    ],
    "basic-event-target-id" => [
        "tapped": [Tap(50.0, 50.0)],
        "tapped-again": [Tap(50.0, 50.0)],
    ],
    "basic-event-trigger" => [
        "target1-tapped": [Tap(200.0, 200.0)],
        "target2-tapped": [Fresh, Tap(25.0, 25.0)],
    ],
    "basic-global-bind" => [
        "tapped": [Tap(50.0, 50.0)],
        "tapped-again": [Tap(50.0, 50.0)],
    ],
    "basic-lazy-component" => [
        "tapped": [Tap(150.0, 50.0)],
        "tapped-again": [Tap(150.0, 50.0)],
    ],
    "basic-lazy-component-mts-bindtap" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    "basic-lazy-component-multi" => [
        "first-tapped": [Tap(150.0, 50.0)],
        "second-tapped": [Tap(150.0, 150.0)],
    ],
    "basic-lazy-component-multi-import" => [
        "first-tapped": [Tap(150.0, 50.0)],
        "second-tapped": [Tap(150.0, 150.0)],
    ],
    "basic-lazy-component-relative-path" => [
        "tapped": [Tap(150.0, 50.0)],
        "tapped-again": [Tap(150.0, 50.0)],
    ],
    "basic-lazy-component-when-need-with-itself" => [
        "second-shown": [Tap(50.0, 150.0)],
        "first-tapped": [Tap(150.0, 50.0)],
        "shown-again": [Tap(50.0, 150.0)],
        "second-tapped": [Tap(150.0, 250.0)],
    ],
    "basic-lazy-component-when-needed" => [
        "loaded": [Tap(50.0, 50.0)],
        "tapped": [Tap(150.0, 150.0)],
        "tapped-again": [Tap(150.0, 150.0)],
    ],
    "basic-lynx-reload" => [
        "tapped": [Tap(50.0, 50.0)],
        "reloaded": [Tap(50.0, 150.0)],
    ],
    "basic-main-query-selector" => [
        "auto-scrolled-to-end": [Tap(196.5, 55.0), Wait(17000)],
    ],
    "basic-mts-bindtap" => [
        "tapped": [Tap(50.0, 50.0), Logged("hello world")],
    ],
    "basic-mts-bindtap-change-element-background" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    "basic-mts-bindtouchstart" => [
        "swiped": [Drag((20.0, 20.0), (30.0, 20.0))],
    ],
    "basic-mts-mainthread-ref" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    "basic-mts-run-on-background" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    "basic-mts-systeminfo" => [
        "tapped": [Tap(50.0, 50.0), Logged("hello world")],
    ],
    "basic-page-event" => [
        "tapped": [Tap(50.0, 50.0)],
    ],
    "basic-ref-main-invoke-ui-method" => [
        "auto-scrolled": [Tap(196.5, 55.0)],
    ],
    "basic-reload" => [
        "tapped": [Tap(50.0, 50.0)],
        "reloaded": [Reload],
    ],
    "basic-bindmouse" => [
        "pressed-and-released": [MouseOver(50.0, 50.0), MouseDown(50.0, 50.0), MouseUp(50.0, 50.0)],
    ],
    "basic-bindkeydown-out-of-view-noop" => [
        "pressed-a": [Press("a", "KeyA", false)],
    ],
    "basic-global-bindkeydown" => [
        "pressed-a": [Press("a", "KeyA", false)],
    ],
    "basic-global-bindkeydown-code" => [
        "pressed-a": [Press("a", "KeyA", false)],
    ],
    "basic-global-bindkeydown-key" => [
        "pressed-enter": [Press("Enter", "Enter", false)],
    ],
    "basic-global-bindkeydown-shift" => [
        "pressed-shift-a": [Press("A", "KeyA", true)],
    ],
    "basic-global-bindkeyup" => [
        "a-down": [KeyDown("a", "KeyA")],
        "a-up": [KeyUp("a", "KeyA")],
    ],
    "basic-element-x-swiper-mode-carry" => [
        "held": [Hold((300.0, 50.0), (50.0, 50.0))],
        "released": [Release(50.0, 50.0)],
    ],
    "basic-style-remove" => [
        "inline-removed": [Tap(50.0, 50.0)],
        "inline-restored": [Tap(50.0, 50.0)],
    ],
    "basic-style-remove-one-property" => [
        "background-removed": [Tap(50.0, 50.0)],
        "background-restored": [Tap(50.0, 50.0)],
    ],
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
