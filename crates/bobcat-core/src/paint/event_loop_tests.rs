use std::time::{Duration, Instant};

use dom::Point2D;
use dom::input::{InputEvent, PointerKind, PointerPhase};

use crate::test_support::{TestEngine, TestViewSpec};

/// The handle a packed id names, the way script spells one.
fn node_id(bits: u64) -> dom::NodeId {
    dom::NodeId::from_bits(bits).expect("a well-formed packed handle")
}

/// Boots a script and waits for it to finish, leaving the view's task parked
/// on its channels with the boot's frame published.
///
/// This suite reads the document and the decisions, never pixels, so the view
/// it builds has nowhere to draw.
fn booted(source: &str) -> TestEngine {
    TestViewSpec::new(source).boot()
}

/// One attribute of one node, read on the view's own thread through a probe.
fn attribute_of(engine: &mut TestEngine, node: u64, name: &'static str) -> Option<String> {
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
fn wait_for_log(engine: &mut TestEngine, expected: &str) {
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

/// The page a `timestamp`/`params` test reads: one `tap` listener that
/// logs what every dispatched event carries beside its detail.
const STAMPED_GESTURE_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          __AppendElement(page, view);
          globalThis.held = [page, view];
          __SetInlineStyles(view, 'width:200px;height:200px');
          __AddEventListener(view, 'tap', (event) => {
            __SetAttribute(
              view,
              'log',
              typeof event.timestamp + ':' + event.timestamp
                + ':' + typeof event.params
                + ':' + JSON.stringify(event.params),
            );
          }, {});
          __FlushElementTree();
        };
        ";

/// Every dispatched event carries a `timestamp` and a `params`. The clock
/// is pinned before the release, so the value the listener reads is that
/// arrival reading in milliseconds — this engine's time origin is the
/// view's timeline, as web-core's is the document's.
#[test]
fn a_delivered_event_carries_its_timestamp_and_params() {
    let mut engine = booted(STAMPED_GESTURE_PAGE);
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.painter.clock.pin(0.25);
    engine.dispatch_input(touch(1, PointerPhase::Up, 12.0));
    wait_for_log(&mut engine, "number:250:object:{}");
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
    engine.painter.clock.pin(0.6);
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
    engine.painter.clock.pin(0.6);
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
    engine.painter.clock.pin(0.6);
    engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Down, 30.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 30.0));
    wait_for_log(&mut engine, "longpress:10,tap:30");
}

/// The touch suite's page: the same 200x200 view, with listeners that log
/// what only a touch event carries — the detail point and the three lists.
const TOUCH_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          __AppendElement(page, view);
          globalThis.held = [page, view];
          globalThis.entries = [];
          __SetInlineStyles(view, 'width:200px;height:200px');
          const note = (event) => {
            entries.push([
              event.type,
              event.detail.x,
              event.touches.length,
              event.targetTouches.length,
              event.changedTouches.length,
              event.changedTouches[0].identifier,
            ].join(':'));
            __SetAttribute(view, 'log', entries.join());
          };
          __AddEventListener(view, 'touchmove', note, {});
          __AddEventListener(view, 'touchend', note, {});
          __FlushElementTree();
        };
        ";

/// One finger's move and release reach a listener with their lists decoded:
/// the move while the finger is down reports it in all three, and the
/// release reports it in `changedTouches` alone, with the detail point
/// falling back to the finger that lifted.
#[test]
fn a_touch_sequence_delivers_its_lists_to_the_realm() {
    let mut engine = booted(TOUCH_PAGE);
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Move, 20.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 22.0));
    wait_for_log(&mut engine, "touchmove:20:1:1:1:1,touchend:22:0:0:1:1");
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

fn scroll_offset_of(engine: &mut TestEngine, node: u64) -> dom::Vector2D<f32> {
    engine
        .probe_document(move |tree| tree.scroll_offset(node_id(node)))
        .expect("the view's task answers probes")
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
        .painter
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

/// An outer column scroller (node 3, max offset 800) whose first child is
/// an inner scroller (node 4, 200×100, max offset 900) carrying
/// `inner_css`; the outer's own filler sits below the inner.
fn nested_scrollers_page(inner_css: &str) -> String {
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const outer = __CreateView(0);
          const inner = __CreateView(0);
          const filler = __CreateView(0);
          const spacer = __CreateView(0);
          __AppendElement(page, outer);
          __AppendElement(outer, inner);
          __AppendElement(inner, filler);
          __AppendElement(outer, spacer);
          globalThis.held = [page, outer, inner, filler, spacer];
          __SetInlineStyles(outer, 'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px');
          __SetInlineStyles(inner, 'display:flex;flex-shrink:0;overflow:scroll;width:200px;height:100px;{inner_css}');
          __SetInlineStyles(filler, 'flex-shrink:0;width:200px;height:1000px');
          __SetInlineStyles(spacer, 'flex-shrink:0;width:200px;height:900px');
          __FlushElementTree();
        }};
        "
    )
}

/// A 200px column scroller (node 3) of five `card_height`px cards carrying
/// `container_css` and `card_css`, behind a `lead`px spacer that snaps to
/// nothing, for the snapping tests.
fn snapping_page(container_css: &str, card_css: &str, card_height: u32, lead: u32) -> String {
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const scroller = __CreateView(0);
          __AppendElement(page, scroller);
          globalThis.held = [page, scroller];
          __SetInlineStyles(scroller, 'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px;{container_css}');
          if ({lead} > 0) {{
            const spacer = __CreateView(0);
            __AppendElement(scroller, spacer);
            held.push(spacer);
            __SetInlineStyles(spacer, 'flex-shrink:0;width:200px;height:{lead}px');
          }}
          for (let i = 0; i < 5; i++) {{
            const card = __CreateView(0);
            __AppendElement(scroller, card);
            held.push(card);
            __SetInlineStyles(card, 'flex-shrink:0;width:200px;height:{card_height}px;{card_css}');
          }}
          __FlushElementTree();
        }};
        "
    )
}

fn drag(engine: &mut TestEngine, from_y: f32, to_y: f32) {
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, from_y),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, to_y),
        1,
        PointerKind::Touch,
        PointerPhase::Move,
    ));
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, to_y),
        1,
        PointerKind::Touch,
        PointerPhase::Up,
    ));
}

fn assert_intent(engine: &TestEngine, node: u64, expected: f32) {
    let offset = engine
        .painter
        .scroll_intents
        .offset_for(node_id(node))
        .unwrap_or_else(|| panic!("node {node} has a scroll intent"));
    assert!(
        (offset.y - expected).abs() < 0.5,
        "node {node} should sit at {expected}, got {offset:?}"
    );
}

/// `scroll-capture: nearest` on the inner scroller: the drag that starts in
/// it moves the scroller above it, and the inner one stays — the router
/// latched the inner, the published chain policy reordered the walk.
#[test]
fn a_capturing_container_hands_its_drag_to_the_scroller_above() {
    let mut engine = booted(&nested_scrollers_page("scroll-capture:nearest"));
    drag(&mut engine, 50.0, 20.0);
    assert_intent(&engine, 3, 22.0);
    assert_eq!(
        engine.painter.scroll_intents.offset_for(node_id(4)),
        None,
        "the inner scroller was not moved"
    );
}

/// `overscroll-behavior: contain` on the inner scroller: a wheel far past
/// its end pins it there and hands nothing to the scroller above.
#[test]
fn overscroll_contain_keeps_a_wheel_inside_its_container() {
    let mut engine = booted(&nested_scrollers_page("overscroll-behavior:contain"));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 50.0),
        dom::Vector2D::new(0.0, 5000.0),
    ));
    assert_intent(&engine, 4, 900.0);
    assert_eq!(
        engine.painter.scroll_intents.offset_for(node_id(3)),
        None,
        "the remainder was fenced off"
    );
}

/// A drag on a `mandatory` snapping scroller is raw while it lasts and
/// settles onto the nearest snap position at its release: 130px of travel
/// less the 8px slop leaves it at 122, nearer the second card's start (200)
/// than the first's (0).
#[test]
fn a_released_drag_settles_onto_the_nearest_snap_position() {
    let mut engine = booted(&snapping_page(
        "scroll-snap-type:y mandatory",
        "scroll-snap-align:start",
        200,
        0,
    ));
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, 150.0),
        1,
        PointerKind::Touch,
        PointerPhase::Down,
    ));
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, 20.0),
        1,
        PointerKind::Touch,
        PointerPhase::Move,
    ));
    assert_intent(&engine, 3, 122.0);
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, 20.0),
        1,
        PointerKind::Touch,
        PointerPhase::Up,
    ));
    assert_intent(&engine, 3, 200.0);
}

/// A wheel tick on a `mandatory` snapping scroller lands on the next snap
/// position in its direction, however small the tick.
#[test]
fn a_wheel_tick_moves_to_the_next_snap_position() {
    let mut engine = booted(&snapping_page(
        "scroll-snap-type:y mandatory",
        "scroll-snap-align:start",
        200,
        0,
    ));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 30.0),
    ));
    assert_intent(&engine, 3, 200.0);
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, -30.0),
    ));
    assert_intent(&engine, 3, 0.0);
}

/// A `mandatory` scroller whose initial offset is no snap position (a 50px
/// spacer ahead of the first card puts the first position at 50) is settled
/// as soon as its first commit is adopted, with no gesture at all: the
/// zero-delta wheel here decides nothing, it only lets the painter adopt.
#[test]
fn a_snapping_container_rests_on_a_position_after_its_first_commit() {
    let mut engine = booted(&snapping_page(
        "scroll-snap-type:y mandatory",
        "scroll-snap-align:start",
        200,
        50,
    ));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::zero(),
    ));
    assert_intent(&engine, 3, 50.0);
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
        .painter
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
        engine.painter.gesture.needs_frame(),
        "the down arms a deadline, which is what keeps frames coming"
    );

    engine.painter.clock.pin(0.6);
    let now = engine.painter.clock.now_seconds();
    engine.painter.service_gesture_clock(now);
    wait_for_log(&mut engine, "longpress:10");
    assert!(
        !engine.painter.gesture.needs_frame(),
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
    // The probe round-tripped the main thread, so the epilogue of that entry
    // has already run its commit-if-dirty — and found nothing.
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
        engine.painter.scroll_intents.offset_for(scroller),
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
///
/// The sheet is an author stylesheet like any other now: the host answers it
/// before the entry, which is the order every view mounts one in.
fn booted_animated(animation_css: &str) -> TestEngine {
    TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          __AppendElement(page, view);
          globalThis.held = [page, view];
          __FlushElementTree();
        };
        ",
    )
    .with_style_sheet(animation_css)
    .boot()
}

/// Sends one `BeginFrame` and waits for the commit it implies to publish.
fn synchronized_tick(engine: &mut TestEngine, now: f64) {
    let seq = engine
        .painter
        .begin_frame(now, true)
        .expect("a tick crosses");
    assert!(
        engine
            .painter
            .wait_begin_frame(seq, Duration::from_secs(30)),
        "the view's task services the tick"
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
        engine.painter.begin_frame(0.5, false).is_none(),
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
        engine.painter.begin_frame(0.1, false).is_none(),
        "inside the curve's domain nothing crosses"
    );

    let seq = engine
        .painter
        .begin_frame(0.3, false)
        .expect("the passed boundary sends the finish tick");
    assert!(
        engine
            .painter
            .wait_begin_frame(seq, Duration::from_secs(30))
    );
    let finished = engine.published_frame().expect("the finish committed");
    assert!(
        !finished.animations_active(),
        "the finish restyle retires the timeline"
    );
    assert!(!finished.has_live_curves());
}

/// One windowed-painter frame: adopt whatever the view published, then send a
/// `BeginFrame` only if that frame asks for one — the protocol
/// [`crate::Painter::pump`] runs, rather than the unconditional tick an
/// offscreen painter takes.
fn windowed_frame(engine: &mut TestEngine, now: f64) {
    // The adoption has to come first: `begin_frame` decides from the frame
    // this painter holds, not from the one the view has published.
    engine.painter.published_frame();
    if let Some(seq) = engine.painter.begin_frame(now, false) {
        assert!(
            engine
                .painter
                .wait_begin_frame(seq, Duration::from_secs(30)),
            "the view's task services the tick"
        );
    }
}

fn is_animating(engine: &mut TestEngine) -> bool {
    engine
        .probe_document(|tree| tree.has_active_animations())
        .expect("the view is live")
}

/// A windowed painter sends no `BeginFrame` while nothing is moving, so an
/// idle page leaves the timeline wherever the last frame left it — here, at
/// boot. The animation a tap starts ten seconds later must still run its whole
/// duration from the frame that follows the tap, rather than being created ten
/// seconds in the past and finishing on its first frame.
#[test]
fn an_animation_started_after_idle_time_runs_from_the_next_frame() {
    let mut engine = TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          __AppendElement(page, view);
          globalThis.held = [page, view];
          __SetInlineStyles(view, 'width:200px;height:200px');
          __AddEventListener(view, 'tap', () => {
            __SetClasses(view, 'moving');
            __SetAttribute(view, 'log', 'tap');
          }, {});
          __FlushElementTree();
        };
        ",
    )
    .with_style_sheet(
        "@keyframes slide { from { transform: translateX(0px); }
                            to { transform: translateX(100px); } }
         .moving { animation: slide 4s linear forwards; }",
    )
    .boot();

    // Ten idle seconds. Nothing on the page asked for a frame, so no
    // `BeginFrame` has crossed since boot and the document's timeline still
    // reads zero.
    let tapped_at = 10.0;
    engine.painter.clock.pin(tapped_at);
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 12.0));
    wait_for_log(&mut engine, "tap");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !engine
        .painter
        .published_frame()
        .is_some_and(|frame| frame.animations_active())
    {
        assert!(
            Instant::now() < deadline,
            "the tap's commit never armed the animation"
        );
        std::thread::yield_now();
    }

    windowed_frame(&mut engine, tapped_at + 0.016);
    assert!(
        is_animating(&mut engine),
        "the frame after the tap is the animation's first, not its last"
    );
    windowed_frame(&mut engine, tapped_at + 3.9);
    assert!(
        is_animating(&mut engine),
        "a 4s animation is still running 3.9s after that frame"
    );
    windowed_frame(&mut engine, tapped_at + 4.1);
    assert!(
        !is_animating(&mut engine),
        "and it is over 4.1s after it, having played the whole way"
    );
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

/// A timer still ahead is the engine's own to wait out. Nothing here drives
/// it — no pump, no command, no deadline handed to the host — and its
/// callback's commit is already published by the first turn that looks.
#[test]
fn a_timer_still_ahead_fires_on_the_engines_own_clock() {
    let mut engine = booted(
        r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              setTimeout(() => __SetAttribute(view, 'ticked', 'yes'), 200);
              __FlushElementTree();
            };
            ",
    );
    let booted_commit = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();
    assert_eq!(
        attribute_of(&mut engine, 3, "ticked"),
        None,
        "a command must not stand in for the deadline"
    );

    // `published_frame` reads what has been published and sends nothing, so
    // nothing in this loop can be what ran the callback.
    let bound = Instant::now() + Duration::from_secs(10);
    loop {
        let commit = engine
            .published_frame()
            .expect("a frame stays published")
            .commit_id();
        if commit != booted_commit {
            break;
        }
        assert!(
            Instant::now() < bound,
            "the timer never came due on the engine's own clock"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        attribute_of(&mut engine, 3, "ticked").as_deref(),
        Some("yes"),
        "and the entry the timer ran in committed its mutation"
    );
}

/// A deadline already behind the engine needs no wait at all: the entry that
/// armed it is the entry whose epilogue fires it, and the commit that
/// epilogue publishes is already the callback's.
#[test]
fn a_timer_that_is_already_due_runs_without_a_nudge_from_the_painter() {
    let mut engine = booted(
        r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              setTimeout(() => {
                __SetAttribute(view, 'ticked', 'now');
                __FlushElementTree();
              }, 0);
              __FlushElementTree();
            };
            ",
    );
    // Read without sending anything: boot is over, so this is the frame the
    // engine published on its own.
    let booted_commit = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    // One probe, and one only. A probe is a command, and a command is exactly
    // the nudge this test says a due timer does not need — but it is applied
    // before the epilogue that runs timers, so an engine that waited for a
    // host turn would answer this first one with nothing.
    assert_eq!(
        attribute_of(&mut engine, 3, "ticked").as_deref(),
        Some("now"),
        "a zero-delay callback must not wait on the host"
    );
    // And nothing was published after boot's frame, so the commit the
    // callback's own flush produced is that very frame: the timer ran inside
    // boot's own epilogue rather than in an entry of its own.
    assert_eq!(
        engine
            .published_frame()
            .expect("a frame stays published")
            .commit_id(),
        booted_commit,
        "the epilogue that ran the callback is the one that committed it"
    );
}

/// A timer that throws has an event listener's standing, not a script
/// failure's: it is reported, the repeat stays armed, and the loop goes on.
#[test]
fn a_throwing_interval_is_reported_and_keeps_its_place_in_the_schedule() {
    let mut engine = booted(
        r"
            globalThis.ticks = 0;
            globalThis.renderPage = function () {
              __CreatePage('card', 0);
              setInterval(() => {
                ticks += 1;
                throw new Error('a timer may fail');
              }, 1);
            };
            ",
    );

    let mut reported = 0usize;
    let deadline = Instant::now() + Duration::from_secs(5);
    while reported < 2 {
        reported += engine
            .pump()
            .into_iter()
            .filter(|event| {
                matches!(event, crate::EngineEvent::TimerFailed(error)
                        if error.message.contains("a timer may fail"))
            })
            .count();
        assert!(
            Instant::now() < deadline,
            "a thrown timer callback must neither be swallowed nor disarm its interval"
        );
        std::thread::yield_now();
    }
}
