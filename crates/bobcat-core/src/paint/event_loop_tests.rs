use std::sync::Arc;
use std::time::{Duration, Instant};

use dom::Point2D;
use dom::input::{InputEvent, PointerKind, PointerPhase};

use super::Painter;

/// The handle a packed id names, the way script spells one.
fn node_id(bits: u64) -> dom::NodeId {
    dom::NodeId::from_bits(bits).expect("a well-formed packed handle")
}

/// Boots a script and waits for it to finish, leaving the main thread
/// parked on its command channel with the boot's frame published.
fn booted(source: &str) -> Painter {
    let document = crate::main::tree::new_document(
        crate::main::tree::Viewport::new(393.0, 727.0),
        crate::main::tree::PageConfig::default(),
    );
    let mut engine = Painter::start(
        document,
        crate::main::tree::Viewport::new(393.0, 727.0),
        super::frame_size(393.0, 727.0, 1.0).expect("the test viewport is valid"),
        Arc::new(super::NoWakeup),
        super::EntryModule {
            source: source.to_owned(),
            url: "app:///main.js".to_owned(),
        },
    )
    .expect("the test view starts");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if engine
            .pump()
            .into_iter()
            .any(|event| matches!(event, crate::EngineEvent::ScriptFinished))
        {
            assert!(
                engine.published_frame().is_some(),
                "boot's flush publishes before ScriptFinished is pumped"
            );
            return engine;
        }
        assert!(Instant::now() < deadline, "the entry module did not finish");
        std::thread::yield_now();
    }
}

/// One attribute of one node, read on the main thread through a probe.
fn attribute_of(engine: &mut Painter, node: u64, name: &'static str) -> Option<String> {
    engine
        .probe_document(move |tree| {
            tree.get(node_id(node))
                .and_then(|live| live.attribute(name).map(str::to_owned))
        })
        .flatten()
}

/// The whole loop: input arrives on this thread, is routed against the
/// published frame and decided here, and delivered to a listener on the
/// thread that owns the realm and the document.
#[test]
fn a_host_input_event_reaches_a_listener_in_the_realm() {
    let mut engine = booted(
        r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              __SetInlineStyles(view, 'width:200px;height:200px');
              __AddEventListener(view, 'pointerdown', (event) => {
                // Observable from the painting side, and proof delivery ran
                // where the document is.
                __SetAttribute(view, 'seen', event.type + ':' + event.detail.x);
              }, {});
              __FlushElementTree();
            };
            ",
    );

    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(10.0, 10.0),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));

    // Delivery is asynchronous by construction: this thread queued a
    // command and moved on.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let seen = attribute_of(&mut engine, 3, "seen");
        if seen.as_deref() == Some("pointerdown:10") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the listener never ran, attribute was {seen:?}"
        );
        std::thread::yield_now();
    }
}

/// A listener that throws must not take the loop with it: the failure is
/// reported rather than swallowed, and the next event still delivers.
#[test]
fn a_throwing_listener_keeps_the_loop_alive_and_is_reported() {
    let mut engine = booted(
        r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              globalThis.count = 0;
              __SetInlineStyles(view, 'width:200px;height:200px');
              __AddEventListener(view, 'pointerdown', () => {
                count += 1;
                __SetAttribute(view, 'seen', String(count));
                throw new Error('a listener may fail');
              }, {});
              __FlushElementTree();
            };
            ",
    );

    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(10.0, 10.0),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));

    let mut reported = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        reported |= engine.pump().into_iter().any(|event| {
            matches!(event, crate::EngineEvent::ListenerFailed(error)
                    if error.message.contains("a listener may fail"))
        });
        let seen = attribute_of(&mut engine, 3, "seen");
        if seen.as_deref() == Some("1") && reported {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "listener ran: {seen:?}, failure reported: {reported}"
        );
        std::thread::yield_now();
    }

    // And the loop still works: a second event routes and is delivered.
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(10.0, 10.0),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if attribute_of(&mut engine, 3, "seen").as_deref() == Some("2") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a thrown listener must not wedge delivery"
        );
        std::thread::yield_now();
    }
}

/// A node with no registration must not cost a trip into the realm, and a
/// script that registered nothing must not keep the loop from working.
#[test]
fn an_event_with_no_listener_changes_nothing() {
    let mut engine = booted(
        r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              __SetInlineStyles(view, 'width:200px;height:200px');
              __FlushElementTree();
            };
            ",
    );

    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(10.0, 10.0),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));

    std::thread::sleep(Duration::from_millis(50));
    assert!(attribute_of(&mut engine, 3, "seen").is_none());
}

/// The gesture suite's page: one 200x200 view whose listeners append
/// `type:x` to a `log` attribute. The placeholder line opts a variant
/// into a `longpress` registration.
const GESTURE_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          __AppendElement(page, view);
          globalThis.held = [page, view];
          globalThis.entries = [];
          __SetInlineStyles(view, 'width:200px;height:200px');
          const note = (event) => {
            entries.push(event.type + ':' + event.detail.x);
            __SetAttribute(view, 'log', entries.join());
          };
          __AddEventListener(view, 'tap', note, {});
          //LONGPRESS
          __FlushElementTree();
        };
        ";

fn gesture_page(with_longpress: bool) -> String {
    if with_longpress {
        GESTURE_PAGE.replace(
            "//LONGPRESS",
            "__AddEventListener(view, 'longpress', note, {});",
        )
    } else {
        GESTURE_PAGE.to_owned()
    }
}

fn touch(id: u32, phase: PointerPhase, x: f32) -> InputEvent {
    InputEvent::pointer(Point2D::new(x, 10.0), id, PointerKind::Touch, phase)
}

/// Polls until the view's `log` attribute equals `expected` — equality,
/// not containment, so an event that should have been suppressed fails
/// the wait by showing up in the actual value. The deadline is generous
/// because the whole suite's realm boots share the machine with this
/// spin.
fn wait_for_log(engine: &mut Painter, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let log = attribute_of(engine, 3, "log");
        if log.as_deref() == Some(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected log {expected:?}, last saw {log:?}"
        );
        std::thread::yield_now();
    }
}

/// A press released within the slop synthesizes `tap` at the release
/// point, delivered through the same path as the raw pointer events.
#[test]
fn a_quick_release_delivers_tap_to_the_realm() {
    let mut engine = booted(&gesture_page(false));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 12.0));
    wait_for_log(&mut engine, "tap:12");
}

/// Travel beyond the 50px tap slop disqualifies the sequence; the later
/// fence tap proves the suppressed one was never sent, because the
/// command channel is ordered.
#[test]
fn travel_beyond_the_tap_slop_suppresses_the_tap() {
    let mut engine = booted(&gesture_page(false));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Move, 100.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 100.0));
    engine.dispatch_input(touch(1, PointerPhase::Down, 150.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 150.0));
    wait_for_log(&mut engine, "tap:150");
}

/// Holding past the deadline delivers `longpress` on the engine's own
/// timeline, and the sequence's release is then not a tap — Lynx's
/// `long_press_consumed` rule. The fence tap pins the suppression.
#[test]
fn a_held_pointer_delivers_longpress_and_suppresses_the_tap() {
    let mut engine = booted(&gesture_page(true));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.clock.pin(0.6);
    engine.dispatch_input(touch(1, PointerPhase::Move, 10.0));
    wait_for_log(&mut engine, "longpress:10");

    engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Down, 30.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 30.0));
    wait_for_log(&mut engine, "longpress:10,tap:30");
}

/// With no `longpress` listener anywhere, the deadline lapses silently
/// and a slow release is still a tap — the listener-presence gate read
/// through the shared name table.
#[test]
fn a_long_hold_without_longpress_listener_still_taps() {
    let mut engine = booted(&gesture_page(false));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.clock.pin(0.6);
    engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
    wait_for_log(&mut engine, "tap:10");
}

/// Input processed after the deadline resolves the deadline first: the
/// decision order is the delivery order on the ordered channel, so
/// `longpress` precedes the release that follows it.
#[test]
fn a_release_after_the_deadline_delivers_longpress_before_the_release() {
    let mut engine = booted(&gesture_page(true));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.clock.pin(0.6);
    engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Down, 30.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 30.0));
    wait_for_log(&mut engine, "longpress:10,tap:30");
}

/// A scrollable page: the 200x200 view scrolls a 1000px-tall child, and
/// its `tap` listener logs `type:x` exactly as the gesture page does.
const SCROLLING_GESTURE_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          const filler = __CreateView(0);
          __AppendElement(page, view);
          __AppendElement(view, filler);
          globalThis.held = [page, view, filler];
          globalThis.entries = [];
          __SetInlineStyles(view, 'display:flex;overflow:scroll;width:200px;height:200px');
          __SetInlineStyles(filler, 'flex-shrink:0;width:200px;height:1000px');
          const note = (event) => {
            entries.push(event.type + ':' + event.detail.x);
            __SetAttribute(view, 'log', entries.join());
          };
          __AddEventListener(view, 'tap', note, {});
          __FlushElementTree();
        };
        ";

fn scroll_offset_of(engine: &mut Painter, node: u64) -> dom::Vector2D<f32> {
    engine
        .probe_document(move |tree| tree.scroll_offset(node_id(node)))
        .expect("the main thread answers probes")
}

/// A drag the user-agent scroll consumed is the claim that suppresses
/// `tap` — end to end: recognition against the published scroll-slot
/// table, consumption arbitrated against published bounds, the scroll
/// applied authoritatively on the main thread. The drag travels 30px:
/// past the 8px drag slop so it scrolls, inside the 50px tap slop so the
/// claim is the only suppressor. The fence tap at another x pins that
/// the suppressed one never crossed the channel.
#[test]
fn a_scroll_consuming_drag_suppresses_the_tap() {
    let mut engine = booted(SCROLLING_GESTURE_PAGE);
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, 100.0),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, 70.0),
        1,
        PointerKind::Touch,
        PointerPhase::Move,
    ));
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, 70.0),
        1,
        PointerKind::Touch,
        PointerPhase::Up,
    ));
    engine.dispatch_input(touch(1, PointerPhase::Down, 150.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 150.0));
    wait_for_log(&mut engine, "tap:150");

    // The router's scroll decision landed in the intents: 30px of
    // travel minus the 8px drag slop moved the scroller 22px. The
    // document never hears about a windowed scroll.
    let offset = engine
        .scroll_intents
        .offset_for(node_id(3))
        .expect("the drag scrolled the view");
    assert!(
        (offset.y - 22.0).abs() < 0.5,
        "the drag scrolled the view, got {offset:?}"
    );
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::zero(),
        "a windowed scroll leaves the document untouched"
    );
}

/// A wheel over scrollable content scrolls it (the router's decision,
/// landing in the intents) and dispatches `wheel` with its delta in
/// the detail — in that order.
#[test]
fn a_wheel_scrolls_and_reaches_a_wheel_listener() {
    let page = SCROLLING_GESTURE_PAGE.replace(
        "__AddEventListener(view, 'tap', note, {});",
        "__AddEventListener(view, 'wheel', (event) => {
               entries.push(event.type + ':' + event.detail.deltaY);
               __SetAttribute(view, 'log', entries.join());
             }, {});",
    );
    let mut engine = booted(&page);
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 30.0),
    ));
    wait_for_log(&mut engine, "wheel:30");
    let offset = engine
        .scroll_intents
        .offset_for(node_id(3))
        .expect("the wheel scrolled the view");
    assert!(
        (offset.y - 30.0).abs() < 0.5,
        "the wheel scrolled the view, got {offset:?}"
    );
}

/// A stationary hold produces no further input, so only the frame half
/// — `service_gesture_clock` plus the `needs_frame` continuation — can
/// resolve it. This drives that half exactly as `draw`/`tick`
/// do, without needing a GPU output.
#[test]
fn a_stationary_hold_longpresses_on_the_frame_clock() {
    let mut engine = booted(&gesture_page(true));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    assert!(
        engine.gesture.needs_frame(),
        "the down arms a deadline, which is what keeps frames coming"
    );

    engine.clock.pin(0.6);
    let now = engine.clock.now_seconds();
    engine.service_gesture_clock(now);
    wait_for_log(&mut engine, "longpress:10");
    assert!(
        !engine.gesture.needs_frame(),
        "a resolved deadline stops asking for frames"
    );
}

/// A 200x200 scroller over two 200px rows, each logging its own tap into
/// its own attribute — so a hit's row is observable from out here.
const TWO_ROW_SCROLLER_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          const first = __CreateView(0);
          const second = __CreateView(0);
          __AppendElement(page, view);
          __AppendElement(view, first);
          __AppendElement(view, second);
          globalThis.held = [page, view, first, second];
          __SetInlineStyles(view,
            'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px');
          for (const row of [first, second]) {
            __SetInlineStyles(row,
              'flex-shrink:0;width:200px;height:200px;background-color:#808080');
          }
          __AddEventListener(first, 'tap', () => __SetAttribute(first, 'tapped', 'yes'), {});
          __AddEventListener(second, 'tap', () => __SetAttribute(second, 'tapped', 'yes'), {});
          __FlushElementTree();
        };
        ";

/// The composed-scroll law, from the engine's side: a user scroll
/// inside the encode window lands in the painting side's intents and
/// nowhere else — no command crosses, the document's offsets stay put,
/// nothing recommits — and hit testing follows the intent offsets, not
/// the committed ones, so a tap lands on what the screen shows.
#[test]
fn a_windowed_scroll_recommits_nothing_and_hits_route_at_the_intent_offsets() {
    let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
    let frame = engine.published_frame().expect("boot published a frame");
    assert!(
        frame.composite_plan().is_some(),
        "a scroller frame layers: targets draw it from retained planes"
    );
    let boot_commit = frame.commit_id();
    drop(frame);

    // 30px is inside half the encode-window headroom (the 200px
    // scrollport), so no refill commit is due either.
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 30.0),
    ));
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::zero(),
        "a windowed scroll leaves the document untouched"
    );
    // The probe round-tripped the main thread, so its round's
    // commit-if-dirty has already run — and found nothing.
    assert_eq!(
        engine
            .published_frame()
            .expect("still published")
            .commit_id(),
        boot_commit,
        "a windowed scroll must not recommit"
    );
    let scroller = node_id(3);
    assert_eq!(
        engine.scroll_intents.offset_for(scroller),
        Some(dom::Vector2D::new(0.0, 30.0)),
        "the intent carries the offset composition draws at"
    );

    // Screen y=180 plus the 30px intent offset is content y=210: the
    // second row. Routed against the committed offsets it would be the
    // first.
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, 180.0),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, 180.0),
        1,
        PointerKind::Touch,
        PointerPhase::Up,
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let second = attribute_of(&mut engine, 5, "tapped");
        if second.as_deref() == Some("yes") {
            break;
        }
        assert!(
            attribute_of(&mut engine, 4, "tapped").is_none(),
            "the tap landed on the unscrolled row: hits ignored the intent offsets"
        );
        assert!(
            Instant::now() < deadline,
            "the tap never delivered, second row saw {second:?}"
        );
        std::thread::yield_now();
    }
    assert!(
        attribute_of(&mut engine, 4, "tapped").is_none(),
        "only the row under the scrolled point may see the tap"
    );
}

/// A scroll past half the encode-window headroom asks the main thread
/// for a refill: the next commit re-centers the windows and publishes
/// the scrolled offsets, all without any script involvement.
#[test]
fn a_scroll_past_half_the_encode_window_requests_a_refill_commit() {
    let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
    let boot_commit = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    // max_offset is 200 (400px of rows in a 200px scrollport), so the
    // window tops out at 200 and 150 is past half its headroom.
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 150.0),
    ));

    let deadline = Instant::now() + Duration::from_secs(5);
    let frame = loop {
        let frame = engine.published_frame().expect("still published");
        if frame.commit_id() > boot_commit {
            break frame;
        }
        assert!(
            Instant::now() < deadline,
            "the refill commit never published"
        );
        std::thread::yield_now();
    };
    let scroller = node_id(3);
    let slot = frame.slot_of(scroller).expect("the scroller has a slot");
    let published = frame.scroll_slots()[slot as usize].offset;
    assert!(
        (published.y - 150.0).abs() < 0.5,
        "the refill commit publishes the scrolled offset, got {published:?}"
    );
}

/// Boots a card whose one view runs `animation_css`, waiting for the
/// boot flush like [`booted`] does.
fn booted_animated(animation_css: &str) -> Painter {
    let mut document = crate::main::tree::new_document(
        crate::main::tree::Viewport::new(393.0, 727.0),
        crate::main::tree::PageConfig::default(),
    );
    crate::style::add_style_sheet_text(&mut document, animation_css);
    let mut engine = Painter::start(
        document,
        crate::main::tree::Viewport::new(393.0, 727.0),
        super::frame_size(393.0, 727.0, 1.0).expect("the test viewport is valid"),
        Arc::new(super::NoWakeup),
        super::EntryModule {
            source: r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  __AppendElement(page, view);
                  globalThis.held = [page, view];
                  __FlushElementTree();
                };
                "
            .to_owned(),
            url: "app:///animated.js".to_owned(),
        },
    )
    .expect("the test view starts");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !engine
        .pump()
        .into_iter()
        .any(|event| matches!(event, crate::EngineEvent::ScriptFinished))
    {
        assert!(Instant::now() < deadline, "the entry module did not finish");
        std::thread::yield_now();
    }
    engine
}

/// Sends one `BeginFrame` and waits for its round's commit to publish.
fn synchronized_tick(engine: &mut Painter, now: f64) {
    let seq = engine.begin_frame(now, true).expect("a tick crosses");
    assert!(
        engine.link.wait_begin_frame(seq, Duration::from_secs(5)),
        "the main thread services the tick"
    );
}

/// An exported curve animates on the painting side: after the tick
/// that promotes it to running, the committed frame carries the curve,
/// wants no per-frame main-thread ticks, and `begin_frame` sends
/// nothing.
#[test]
fn an_exported_curve_stops_asking_for_main_thread_ticks() {
    let mut engine = booted_animated(
        "view { width: 100px; height: 100px; background-color: red;
                    animation: fade 1s linear infinite; }
             @keyframes fade { from { opacity: 1; } to { opacity: 0; } }",
    );
    let boot = engine.published_frame().expect("the boot flush published");
    assert!(
        boot.needs_main_ticks(),
        "a pending animation still needs the promoting tick"
    );

    synchronized_tick(&mut engine, 0.1);
    let frame = engine.published_frame().expect("the promotion committed");
    assert!(frame.animations_active());
    assert!(frame.has_live_curves(), "the fade exported");
    assert!(
        !frame.needs_main_ticks(),
        "an exported curve frees the main thread"
    );
    assert!(
        engine.begin_frame(0.5, false).is_none(),
        "no BeginFrame crosses while the curve covers the animation"
    );
}

/// A finite curve's expiry is the one moment the main thread must hear
/// about: the boundary tick runs the finish restyle and the next frame
/// reports the timeline idle.
#[test]
fn a_finished_curve_hands_the_animation_back_to_the_main_thread() {
    let mut engine = booted_animated(
        "view { width: 100px; height: 100px; background-color: red;
                    animation: fade 0.2s linear; }
             @keyframes fade { from { opacity: 1; } to { opacity: 0; } }",
    );
    synchronized_tick(&mut engine, 0.05);
    let frame = engine.published_frame().expect("the promotion committed");
    assert!(frame.has_live_curves());
    assert!(
        engine.begin_frame(0.1, false).is_none(),
        "inside the curve's domain nothing crosses"
    );

    let seq = engine
        .begin_frame(0.3, false)
        .expect("the passed boundary sends the finish tick");
    assert!(engine.link.wait_begin_frame(seq, Duration::from_secs(5)));
    let finished = engine.published_frame().expect("the finish committed");
    assert!(
        !finished.animations_active(),
        "the finish restyle retires the timeline"
    );
    assert!(!finished.has_live_curves());
}

#[test]
fn independent_views_can_own_live_script_threads_in_one_process() {
    let source = r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              __AppendElement(page, __CreateView(0));
              __FlushElementTree();
            };
        ";

    let mut first = booted(source);
    let mut second = booted(source);

    for engine in [&mut first, &mut second] {
        let children = engine
            .probe_document(|tree| tree.document_element().child_ids().len())
            .expect("each live view retains its own document");
        assert_eq!(children, 1);
    }
}
