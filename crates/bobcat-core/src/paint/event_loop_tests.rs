use std::sync::Arc;
use std::time::{Duration, Instant};

use dom::Point2D;
use dom::input::{InputEvent, PointerKind, PointerPhase};

use crate::test_support::{
    IntersectionWatcher, TestEngine, TestViewSpec, WatcherLog, take_watched,
};

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
/// table, consumption arbitrated against published bounds, the offset
/// posted to the main thread and adopted there. The drag travels 30px:
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
    // travel minus the 8px drag slop moved the scroller 22px, and the
    // document followed: the marker queued ahead of the probe.
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
        offset,
        "the document adopts the posted offset"
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
/// settles onto the nearest snap position after its release: 130px of
/// travel less the 8px slop leaves it at 122, nearer the second card's start
/// (200) than the first's (0). Let go without velocity, it glides there over
/// the frames that follow rather than jumping on the release.
#[test]
fn a_released_drag_settles_onto_the_nearest_snap_position() {
    let mut engine = booted(&snapping_page(
        "scroll-snap-type:y mandatory",
        "scroll-snap-align:start",
        200,
        0,
    ));
    touch_at(&mut engine, 0.0, PointerPhase::Down, 150.0);
    touch_at(&mut engine, 0.05, PointerPhase::Move, 20.0);
    assert_intent(&engine, 3, 122.0);
    touch_at(&mut engine, 0.4, PointerPhase::Up, 20.0);
    assert_intent(&engine, 3, 122.0);
    assert!(engine.is_animating(), "the settle owes frames");
    frame_at(&mut engine, 0.45);
    let moving = intent_y(&engine, 3);
    assert!(moving > 122.0 && moving < 200.0, "on its way, got {moving}");
    frame_at(&mut engine, 1.5);
    assert!(
        (intent_y(&engine, 3) - 200.0).abs() < f32::EPSILON,
        "exactly on the position, got {}",
        intent_y(&engine, 3)
    );
    assert!(!engine.is_animating());
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

/// `scroll-initial-target: nearest` on the third card sets the scroller's
/// initial position in the document itself, inside the boot's own commit,
/// so the published frame — and the painter's at-rest rule — start from it.
#[test]
fn an_initial_scroll_target_positions_the_container_in_the_boot_commit() {
    let page = snapping_page("", "", 200, 0).replace(
        "__SetInlineStyles(card, 'flex-shrink:0;width:200px;height:200px;');",
        "__SetInlineStyles(card, 'flex-shrink:0;width:200px;height:200px;'
              + (i === 2 ? 'scroll-initial-target:nearest' : ''));",
    );
    let mut engine = booted(&page);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(0.0, 400.0),
        "the document rests on the target"
    );
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::zero(),
    ));
    assert_eq!(
        engine.painter.scroll_intents.offset_for(node_id(3)),
        None,
        "nothing for the painter to override"
    );
}

/// Pins the painter's clock and feeds one touch at `y`.
fn touch_at(engine: &mut TestEngine, at: f64, phase: PointerPhase, y: f32) {
    engine.painter.clock.pin(at);
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, y),
        1,
        PointerKind::Touch,
        phase,
    ));
}

/// Runs the painter's gesture clock at `at`, the way a display frame does.
fn frame_at(engine: &mut TestEngine, at: f64) {
    engine.painter.clock.pin(at);
    engine.painter.service_gesture_clock(at);
}

fn intent_y(engine: &TestEngine, node: u64) -> f32 {
    engine
        .painter
        .scroll_intents
        .offset_for(node_id(node))
        .unwrap_or_else(|| panic!("node {node} has a scroll intent"))
        .y
}

/// A flick: 60px of upward finger travel over 20ms, released at speed —
/// 52px of drag after the slop, then a release velocity of 3 px/ms.
fn flick(engine: &mut TestEngine, at: f64) {
    touch_at(engine, at, PointerPhase::Down, 150.0);
    touch_at(engine, at + 0.01, PointerPhase::Move, 120.0);
    touch_at(engine, at + 0.02, PointerPhase::Move, 90.0);
    touch_at(engine, at + 0.02, PointerPhase::Up, 90.0);
}

/// A flick keeps the scroller moving after the finger lifts: the release
/// velocity decays per frame along `motion`'s curve, the frames are owed
/// while it does, and it comes to rest where the curve is spent — the
/// drag's 52px plus a 3 px/ms fling's whole travel, `−v₀/ln 0.998` ≈
/// 1498px, clamped here by the 800px maximum.
#[test]
fn a_flick_keeps_scrolling_after_its_release_and_comes_to_rest() {
    let mut engine = booted(&snapping_page("", "", 200, 0));
    flick(&mut engine, 0.0);
    assert!((intent_y(&engine, 3) - 52.0).abs() < 0.5);
    assert!(engine.is_animating(), "a fling owes frames");

    frame_at(&mut engine, 0.1);
    let early = intent_y(&engine, 3);
    // 80ms at 3 px/ms decaying at 0.998: 3·(0.998^80 − 1)/ln 0.998 ≈ 222.
    assert!(
        (early - (52.0 + 222.0)).abs() < 3.0,
        "the first frame moved by the curve, got {early}"
    );
    frame_at(&mut engine, 0.3);
    let later = intent_y(&engine, 3);
    assert!(later > early, "still moving");
    for at in [1.0, 2.0, 4.0, 8.0] {
        frame_at(&mut engine, at);
    }
    assert!(
        (intent_y(&engine, 3) - 800.0).abs() < 0.5,
        "spent against the end, got {}",
        intent_y(&engine, 3)
    );
    assert!(!engine.is_animating(), "at rest, no frame owed");
}

/// The same flick on a slower start — a 1 px/ms release — travels
/// `−1/ln 0.998` ≈ 500px and stops short of the end, and a `mandatory`
/// scroller aims the fling at the snap position that travel settles on:
/// 52 + 500 = 552 is nearest the fourth card's start at 600, so the fling
/// lands there exactly rather than snapping after it stops.
#[test]
fn a_fling_on_a_snapping_scroller_lands_on_a_snap_position() {
    let slow_flick = |engine: &mut TestEngine| {
        touch_at(engine, 0.0, PointerPhase::Down, 150.0);
        touch_at(engine, 0.02, PointerPhase::Move, 130.0);
        touch_at(engine, 0.04, PointerPhase::Move, 110.0);
        touch_at(engine, 0.06, PointerPhase::Move, 90.0);
        touch_at(engine, 0.06, PointerPhase::Up, 90.0);
    };
    let mut engine = booted(&snapping_page("", "", 200, 0));
    slow_flick(&mut engine);
    for at in [0.5, 1.0, 2.0, 4.0, 8.0] {
        frame_at(&mut engine, at);
    }
    let unsnapped = intent_y(&engine, 3);
    assert!(
        (unsnapped - 551.5).abs() < 2.0,
        "the curve's whole travel, got {unsnapped}"
    );

    let mut engine = booted(&snapping_page(
        "scroll-snap-type:y mandatory",
        "scroll-snap-align:start",
        200,
        0,
    ));
    slow_flick(&mut engine);
    frame_at(&mut engine, 0.5);
    let mid = intent_y(&engine, 3);
    assert!(mid > 100.0 && mid < 600.0, "on its way, got {mid}");
    for at in [1.0, 2.0, 4.0, 8.0] {
        frame_at(&mut engine, at);
    }
    assert!(
        (intent_y(&engine, 3) - 600.0).abs() < 0.5,
        "aimed at the position, got {}",
        intent_y(&engine, 3)
    );
    assert!(!engine.is_animating());
}

/// `overscroll-behavior: contain-bounce`: a drag past the start edge
/// stretches the scroller by the rubber band — 92px of finger travel past
/// the edge (100 less the slop) on a 200px scrollport stretches
/// `(1 − 1/(92·0.55/200 + 1))·200` ≈ 40.4px — and a release from rest
/// springs it back to the edge on the critically damped curve.
#[test]
fn a_drag_past_a_bouncing_edge_stretches_and_springs_back() {
    let mut engine = booted(&snapping_page(
        "overscroll-behavior:contain-bounce",
        "",
        200,
        0,
    ));
    touch_at(&mut engine, 0.0, PointerPhase::Down, 50.0);
    touch_at(&mut engine, 0.05, PointerPhase::Move, 150.0);
    let stretched = intent_y(&engine, 3);
    assert!((stretched - -40.4).abs() < 0.2, "got {stretched}");
    assert!(!engine.is_animating(), "under the finger nothing animates");
    // Held still, then let go: no velocity, so the bounce back starts at
    // once from the stretch.
    touch_at(&mut engine, 0.4, PointerPhase::Up, 150.0);
    assert!(engine.is_animating(), "the bounce back owes frames");
    frame_at(&mut engine, 0.41);
    let first = intent_y(&engine, 3);
    assert!(
        first < 0.0 && first > stretched,
        "barely moved yet, got {first}"
    );
    frame_at(&mut engine, 0.6);
    let later = intent_y(&engine, 3);
    assert!(later > first && later < 0.0, "on its way home, got {later}");
    frame_at(&mut engine, 1.5);
    assert!(
        intent_y(&engine, 3).abs() < f32::EPSILON,
        "home on the edge, got {}",
        intent_y(&engine, 3)
    );
    assert!(!engine.is_animating());
}

/// A fling into a bouncing end edge overshoots past it — raw, at the
/// overshoot decay — and springs back to rest on the edge.
#[test]
fn a_fling_into_a_bouncing_edge_overshoots_and_springs_back() {
    let mut engine = booted(&snapping_page(
        "overscroll-behavior:contain-bounce",
        "",
        200,
        0,
    ));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 700.0),
    ));
    assert_intent(&engine, 3, 700.0);
    flick(&mut engine, 0.0);
    let mut furthest = intent_y(&engine, 3);
    for at in [0.05, 0.1, 0.15, 0.2, 0.25, 0.3] {
        frame_at(&mut engine, at);
        furthest = furthest.max(intent_y(&engine, 3));
    }
    assert!(
        furthest > 800.0 && furthest <= 1000.0,
        "past the end and inside a scrollport of it, got {furthest}"
    );
    for at in [1.0, 2.0, 3.0] {
        frame_at(&mut engine, at);
    }
    assert!(
        (intent_y(&engine, 3) - 800.0).abs() < f32::EPSILON,
        "back on the edge, got {}",
        intent_y(&engine, 3)
    );
    assert!(!engine.is_animating());
}

/// Without `contain-bounce` the same fling stops dead at the edge: the
/// boundary is a wall, and nothing chains past a lone scroller.
#[test]
fn a_fling_into_a_plain_edge_stops_there() {
    let mut engine = booted(&snapping_page("", "", 200, 0));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 700.0),
    ));
    flick(&mut engine, 0.0);
    for at in [0.05, 0.1, 0.2] {
        frame_at(&mut engine, at);
        assert!(intent_y(&engine, 3) <= 800.0);
    }
    assert!((intent_y(&engine, 3) - 800.0).abs() < 0.5);
    assert!(!engine.is_animating(), "spent against the wall");
}

/// A [`snapping_page`] whose scroller is `overscroll-behavior: circular`:
/// five 200px cards in a 200px scrollport, so `max_offset` 800 and a
/// period of 1000 — the last card's end meets the first card's start.
const CIRCULAR_PERIOD: f32 = 1000.0;

/// Where the painter shows `node` on a circular y axis, normalized into
/// the period: its raw live offset may stand anywhere until the next
/// rebase brings it back.
fn wrapped_y(engine: &mut TestEngine, node: u64) -> f32 {
    live_offset(engine, node).y.rem_euclid(CIRCULAR_PERIOD)
}

/// Runs display frames from `at` until nothing animates, and answers the
/// clock reading it came to rest at.
fn frames_until_still(engine: &mut TestEngine, mut at: f64) -> f64 {
    let deadline = at + 10.0;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < deadline, "never came to rest");
        frame_at(engine, at);
    }
    at
}

/// `overscroll-behavior: circular` with `mandatory` snapping: a gentle
/// flick forward from the last card (800) pages on to the first card's
/// next copy, 1000, rather than stopping at the end. The flick is 10px of
/// finger travel per 20ms after the 8px slop — 812 at the release, a
/// release velocity of 0.5 px/ms whose whole fling travels
/// `−0.5/ln 0.998` ≈ 250px — so its predicted end, ≈ 1062, is nearest
/// the periodic position 1000, 188px away: inside one scrollport, so a
/// glide. Once posted, the document adopts 1000 modulo the period: 0.
#[test]
fn a_flick_forward_from_the_last_page_wraps_to_the_first() {
    let mut engine = booted(&snapping_page(
        "overscroll-behavior:circular; scroll-snap-type:y mandatory",
        "scroll-snap-align:start",
        200,
        0,
    ));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 800.0),
    ));
    assert_intent(&engine, 3, 800.0);
    touch_at(&mut engine, 0.0, PointerPhase::Down, 150.0);
    touch_at(&mut engine, 0.02, PointerPhase::Move, 140.0);
    touch_at(&mut engine, 0.04, PointerPhase::Move, 130.0);
    touch_at(&mut engine, 0.04, PointerPhase::Up, 130.0);
    assert!(
        (wrapped_y(&mut engine, 3) - 812.0).abs() < 0.5,
        "released at 812, got {}",
        wrapped_y(&mut engine, 3)
    );
    assert!(engine.is_animating(), "the glide owes frames");
    let at = frames_until_still(&mut engine, 0.04);
    let landed = live_offset(&mut engine, 3).y;
    assert!(
        landed.rem_euclid(CIRCULAR_PERIOD).abs() < f32::EPSILON,
        "on the first card's next copy, got {landed}"
    );
    engine.painter.publish_scroll(at, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::zero(),
        "the document adopts the first card"
    );
}

/// The other way round: a drag back from the first card (finger moving
/// down) shows the last card's end above the seam, and the document —
/// which never wraps — holds `max_offset` meanwhile (the user's ruling,
/// as for a `contain-bounce` stretch). 150px of travel less the 8px slop
/// leaves the live offset at −142, 908 in the period; let go without
/// velocity, it is nearer the last card's previous copy (−200, 58px off)
/// than the first card (142px off), and glides there: 800 in the period,
/// which the document adopts at rest.
#[test]
fn a_drag_back_from_the_first_page_shows_the_last_and_posts_the_edge() {
    let mut engine = booted(&snapping_page(
        "overscroll-behavior:circular; scroll-snap-type:y mandatory",
        "scroll-snap-align:start",
        200,
        0,
    ));
    touch_at(&mut engine, 0.0, PointerPhase::Down, 30.0);
    touch_at(&mut engine, 0.05, PointerPhase::Move, 180.0);
    assert!(
        (wrapped_y(&mut engine, 3) - 858.0).abs() < 0.5,
        "−142 in the period, got {}",
        live_offset(&mut engine, 3).y
    );
    // The probe queues behind the marker the drag step sent.
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(0.0, 800.0),
        "through the seam the document holds the edge"
    );
    touch_at(&mut engine, 0.4, PointerPhase::Up, 180.0);
    let at = frames_until_still(&mut engine, 0.4);
    assert!(
        (wrapped_y(&mut engine, 3) - 800.0).abs() < f32::EPSILON,
        "on the last card, got {}",
        live_offset(&mut engine, 3).y
    );
    engine.painter.publish_scroll(at, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(0.0, 800.0),
        "the document adopts the last card"
    );
}

/// `overscroll-behavior: circular` on the inner of two nested scrollers
/// (inner `max_offset` 900, so a period of 1000): a wheel of 1500 is all
/// the inner's — it wraps to 500 — and nothing chains out to the outer.
#[test]
fn a_circular_axis_never_chains_out() {
    let mut engine = booted(&nested_scrollers_page("overscroll-behavior:circular"));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 50.0),
        dom::Vector2D::new(0.0, 1500.0),
    ));
    assert!(
        (wrapped_y(&mut engine, 4) - 500.0).abs() < 0.5,
        "the inner wrapped, got {}",
        live_offset(&mut engine, 4).y
    );
    assert_eq!(
        engine.painter.scroll_intents.offset_for(node_id(3)),
        None,
        "nothing chained out to the outer"
    );
    assert_eq!(
        scroll_offset_of(&mut engine, 4),
        dom::Vector2D::new(0.0, 500.0),
        "the document adopts the wrapped offset"
    );
}

/// A fling on a circular axis without snapping meets no wall: wheeled to
/// 700, the [`flick`] adds its 52px of drag and a 3 px/ms fling's whole
/// travel, `−3/ln 0.998` ≈ 1498px, so the scroller runs through the end
/// and on round the circle to ≈ 2250 unwrapped — 250 in the period —
/// ending by its decay alone. The document adopts the wrapped offset.
#[test]
fn a_fling_on_a_plain_circular_axis_runs_past_the_end() {
    let mut engine = booted(&snapping_page("overscroll-behavior:circular", "", 200, 0));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 700.0),
    ));
    assert_intent(&engine, 3, 700.0);
    flick(&mut engine, 0.0);
    // Summed modulo the period frame by frame, so a rebase that brings
    // the live offset back into the period does not lose the travel.
    let mut previous = wrapped_y(&mut engine, 3);
    let mut travelled = previous;
    let mut at = 0.02;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < 10.0, "the fling never came to rest");
        frame_at(&mut engine, at);
        let now = wrapped_y(&mut engine, 3);
        travelled += (now - previous).rem_euclid(CIRCULAR_PERIOD);
        previous = now;
    }
    assert!(
        (travelled - 2250.5).abs() < 3.0,
        "through the end and round the circle, got {travelled}"
    );
    let rest = wrapped_y(&mut engine, 3);
    engine.painter.publish_scroll(at, None);
    let adopted = scroll_offset_of(&mut engine, 3);
    assert!(
        adopted.x == 0.0 && (adopted.y - rest).abs() < 0.5,
        "the document adopts {rest}, got {adopted:?}"
    );
}

/// A drag landing on a flinging scroller takes it over: a finger's down
/// alone stops nothing (the router decides nothing before the slop), the
/// drag's first step stops the fling where it finds the container, and a
/// quiet release settles there. Its tap was suppressed by the consumed
/// scroll as any drag's is, which the fence tap pins.
#[test]
fn a_drag_on_a_flinging_scroller_takes_it_over() {
    let mut engine = booted(SCROLLING_GESTURE_PAGE);
    flick(&mut engine, 0.0);
    frame_at(&mut engine, 0.1);
    let moving = intent_y(&engine, 3);
    assert!(moving > 52.0 && engine.is_animating());

    touch_at(&mut engine, 0.15, PointerPhase::Down, 100.0);
    frame_at(&mut engine, 0.2);
    assert!(engine.is_animating(), "a down alone decides nothing");
    let found = intent_y(&engine, 3);
    assert!(found > moving);
    // 10px of travel: the 8px slop, then a 2px step that takes over.
    touch_at(&mut engine, 0.25, PointerPhase::Move, 90.0);
    assert!(!engine.is_animating(), "the drag stopped the fling");
    let held = intent_y(&engine, 3);
    assert!(
        (held - (found + 2.0)).abs() < 0.5,
        "got {held} after {found}"
    );
    frame_at(&mut engine, 0.5);
    assert!((intent_y(&engine, 3) - held).abs() < f32::EPSILON);
    touch_at(&mut engine, 0.6, PointerPhase::Up, 90.0);
    frame_at(&mut engine, 1.0);
    assert!((intent_y(&engine, 3) - held).abs() < f32::EPSILON);
    assert!(!engine.is_animating());

    engine.painter.clock.pin(2.0);
    engine.dispatch_input(touch(1, PointerPhase::Down, 150.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 150.0));
    wait_for_log(&mut engine, "tap:150");
}

/// A 200px row pager (node 3) of five 200px pages (nodes 4–8) snapping
/// `mandatory` on x, each page carrying `page_css`: a `<viewpager>`'s shape.
fn pager_page(page_css: &str) -> String {
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const pager = __CreateView(0);
          __AppendElement(page, pager);
          globalThis.held = [page, pager];
          __SetInlineStyles(pager, 'display:flex;flex-direction:row;overflow:scroll;width:200px;height:200px;scroll-snap-type:x mandatory');
          for (let i = 0; i < 5; i++) {{
            const item = __CreateView(0);
            __AppendElement(pager, item);
            held.push(item);
            __SetInlineStyles(item, 'flex-shrink:0;width:200px;height:200px;scroll-snap-align:start;{page_css}');
          }}
          __FlushElementTree();
        }};
        "
    )
}

/// Pins the painter's clock and feeds one touch at `x`, halfway down.
fn touch_x_at(engine: &mut TestEngine, at: f64, phase: PointerPhase, x: f32) {
    engine.painter.clock.pin(at);
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(x, 100.0),
        1,
        PointerKind::Touch,
        phase,
    ));
}

/// Where the painter shows `node`: its live offset, else the committed one.
fn live_offset(engine: &mut TestEngine, node: u64) -> dom::Vector2D<f32> {
    let node = node_id(node);
    engine
        .painter
        .scroll_intents
        .offset_for(node)
        .unwrap_or_else(|| {
            let frame = engine
                .painter
                .published_frame()
                .expect("a frame is adopted");
            let index = frame.slot_of(node).expect("the node is a scroll container");
            frame.scroll_slots()[index as usize].offset
        })
}

/// Adopts whatever the view has published and rebases onto it at `at`,
/// the way a display frame does before it composes.
fn adopt_at(engine: &mut TestEngine, at: f64) {
    frame_at(engine, at);
    let frame = engine
        .painter
        .published_frame()
        .expect("a frame is adopted");
    engine.painter.scroll_intents.rebase(&frame, at);
}

/// A horizontal flick on the pager: 60px of leftward travel over 20ms —
/// 52px of drag after the slop, then a release velocity of 3 px/ms.
fn flick_x(engine: &mut TestEngine, at: f64) {
    touch_x_at(engine, at, PointerPhase::Down, 150.0);
    touch_x_at(engine, at + 0.01, PointerPhase::Move, 120.0);
    touch_x_at(engine, at + 0.02, PointerPhase::Move, 90.0);
    touch_x_at(engine, at + 0.02, PointerPhase::Up, 90.0);
}

/// A release without velocity on a pager glides to the nearest page over
/// several frames — never past it — lands on it exactly and stops asking
/// for frames; the document follows.
#[test]
fn a_quiet_release_on_a_pager_glides_onto_the_nearest_page() {
    let mut engine = booted(&pager_page(""));
    touch_x_at(&mut engine, 0.0, PointerPhase::Down, 150.0);
    touch_x_at(&mut engine, 0.05, PointerPhase::Move, 30.0);
    let released = live_offset(&mut engine, 3).x;
    assert!((released - 112.0).abs() < 0.5, "got {released}");
    touch_x_at(&mut engine, 0.4, PointerPhase::Up, 30.0);
    assert!(engine.is_animating(), "the glide owes frames");
    let mut previous = released;
    let mut moving_frames = 0;
    let mut at = 0.4;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < 1.5, "the glide never landed");
        frame_at(&mut engine, at);
        let now = live_offset(&mut engine, 3).x;
        assert!(
            now >= previous && now <= 200.0,
            "monotone onto the page, got {now} after {previous}"
        );
        if now < 200.0 {
            moving_frames += 1;
        }
        previous = now;
    }
    assert!(moving_frames >= 5, "a glide, not a jump: {moving_frames}");
    assert!((live_offset(&mut engine, 3).x - 200.0).abs() < f32::EPSILON);
    engine.painter.publish_scroll(at, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(200.0, 0.0),
        "the document adopts where it landed"
    );
}

/// A flick on a pager whose pages are `scroll-snap-stop: always` lands on
/// the next page — the stop its whole fling would have passed — well within
/// 0.7s, keeping the release velocity, and never passes it.
#[test]
fn a_flick_on_a_pager_lands_on_the_next_page_without_passing_it() {
    let mut engine = booted(&pager_page("scroll-snap-stop:always"));
    flick_x(&mut engine, 0.0);
    let released = live_offset(&mut engine, 3).x;
    assert!((released - 52.0).abs() < 0.5, "got {released}");
    assert!(engine.is_animating());
    frame_at(&mut engine, 0.02 + 1.0 / 60.0);
    let first = live_offset(&mut engine, 3).x;
    assert!(
        first - released > 20.0,
        "the release velocity carries on at once, got {first}"
    );
    let mut at = 0.02;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < 0.7, "not on the page within 0.7s");
        frame_at(&mut engine, at);
        let now = live_offset(&mut engine, 3).x;
        assert!(now <= 200.0, "passed the next page: {now}");
    }
    assert!((live_offset(&mut engine, 3).x - 200.0).abs() < f32::EPSILON);
}

/// A snap position farther than one scrollport is still reached by the
/// aimed fling: without `always` stops the same flick's whole travel
/// settles on the last page, 748px on.
#[test]
fn a_far_snap_position_is_still_reached_by_a_fling() {
    let mut engine = booted(&pager_page(""));
    flick_x(&mut engine, 0.0);
    let frame = engine
        .painter
        .published_frame()
        .expect("a frame is adopted");
    assert!(
        engine
            .painter
            .scroll_intents
            .is_flinging(&frame, node_id(3)),
        "a fling carries it"
    );
    assert!(!engine.painter.scroll_intents.is_gliding(node_id(3)));
    frame_at(&mut engine, 0.3);
    let mid = live_offset(&mut engine, 3).x;
    assert!(mid > 200.0 && mid < 800.0, "on its way, got {mid}");
    for at in [1.0, 2.0, 4.0, 8.0] {
        frame_at(&mut engine, at);
    }
    assert!(
        (live_offset(&mut engine, 3).x - 800.0).abs() < 0.5,
        "aimed at the last page, got {}",
        live_offset(&mut engine, 3).x
    );
    assert!(!engine.is_animating());
}

/// A drag's first step on a gliding pager stops the glide where it finds
/// the pager, and the pager moves with the finger from there.
#[test]
fn a_drag_interrupts_a_glide() {
    let mut engine = booted(&pager_page(""));
    touch_x_at(&mut engine, 0.0, PointerPhase::Down, 150.0);
    touch_x_at(&mut engine, 0.05, PointerPhase::Move, 30.0);
    touch_x_at(&mut engine, 0.4, PointerPhase::Up, 30.0);
    frame_at(&mut engine, 0.45);
    let found = live_offset(&mut engine, 3).x;
    assert!(found > 112.0 && found < 200.0, "mid-glide, got {found}");

    touch_x_at(&mut engine, 0.5, PointerPhase::Down, 100.0);
    // 10px of travel: the 8px slop, then a 2px step that takes over.
    touch_x_at(&mut engine, 0.55, PointerPhase::Move, 90.0);
    assert!(!engine.is_animating(), "the drag stopped the glide");
    let held = live_offset(&mut engine, 3).x;
    assert!(
        (held - (found + 2.0)).abs() < 0.5,
        "got {held} after {found}"
    );
    frame_at(&mut engine, 0.8);
    assert!((live_offset(&mut engine, 3).x - held).abs() < f32::EPSILON);
}

/// A smooth programmatic scroll leaves the document where it was and the
/// painter glides the pager to the target from rest; the offsets it posts
/// name the request, so main adopts them and the request ends.
#[test]
fn a_smooth_request_glides_to_its_target() {
    let mut engine = booted(&pager_page(""));
    let target = engine
        .probe_document(|document| {
            document.scroll_to_with(
                node_id(3),
                dom::Vector2D::new(400.0, 0.0),
                dom::scroll::ScrollBehavior::Smooth,
            )
        })
        .expect("the view's task answers probes");
    assert_eq!(target, dom::Vector2D::new(400.0, 0.0));
    // A later entry, so the request's own commit has been published too.
    assert_eq!(scroll_offset_of(&mut engine, 3), dom::Vector2D::zero());

    adopt_at(&mut engine, 1.0);
    assert!(engine.is_animating(), "the request started a glide");
    assert!(engine.painter.scroll_intents.is_gliding(node_id(3)));
    let mut previous = 0.0;
    let mut at = 1.0;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < 2.0, "the glide never landed");
        frame_at(&mut engine, at);
        let now = live_offset(&mut engine, 3).x;
        assert!(now >= previous && now <= 400.0, "got {now}");
        previous = now;
    }
    assert!((live_offset(&mut engine, 3).x - 400.0).abs() < f32::EPSILON);
    engine.painter.publish_scroll(at, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(400.0, 0.0)
    );
    assert_eq!(
        engine
            .probe_document(|document| document.pending_scroll_request(node_id(3)))
            .expect("the view's task answers probes"),
        None,
        "acknowledged"
    );
}

/// An instant programmatic scroll lands in one frame even while a fling is
/// carrying the container: the fling stops, and a post the painter made
/// before it saw the request does not undo the document's move.
#[test]
fn an_instant_request_lands_at_once_over_a_fling() {
    let mut engine = booted(SCROLLING_GESTURE_PAGE);
    flick(&mut engine, 0.0);
    frame_at(&mut engine, 0.1);
    assert!(engine.is_animating(), "flinging");
    let flung = live_offset(&mut engine, 3).y;
    assert!(flung > 52.0);

    engine
        .probe_document(|document| {
            document.scroll_to_with(
                node_id(3),
                dom::Vector2D::new(0.0, 100.0),
                dom::scroll::ScrollBehavior::Instant,
            )
        })
        .expect("the view's task answers probes");
    // The fling's last step, posted before the painter adopted the frame
    // carrying the request.
    engine.painter.publish_scroll(0.1, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(0.0, 100.0),
        "the stale post was dropped"
    );

    frame_at(&mut engine, 0.12);
    assert!(!engine.is_animating(), "the request stopped the fling");
    assert!((live_offset(&mut engine, 3).y - 100.0).abs() < f32::EPSILON);
    frame_at(&mut engine, 0.5);
    assert!((live_offset(&mut engine, 3).y - 100.0).abs() < f32::EPSILON);
    engine.painter.publish_scroll(0.5, None);
    assert_eq!(
        engine
            .probe_document(|document| document.pending_scroll_request(node_id(3)))
            .expect("the view's task answers probes"),
        None,
        "the painter's post acknowledged it"
    );
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(0.0, 100.0)
    );
}

/// A wheel tick on a container a smooth request is gliding ends the glide:
/// the wheel's own landing position, snapped as a wheel tick snaps, is
/// where the container rests, and no later frame of the glide overwrites
/// it. The post that follows names the request, so main adopts it.
#[test]
fn a_wheel_tick_ends_a_glide_where_the_wheel_puts_it() {
    let mut engine = booted(&viewpager_page(""));
    engine
        .probe_document(|document| {
            document.scroll_to_with(
                node_id(3),
                dom::Vector2D::new(800.0, 0.0),
                dom::scroll::ScrollBehavior::Smooth,
            )
        })
        .expect("the view's task answers probes");
    // A later entry, so the request's own commit has been published too.
    assert_eq!(scroll_offset_of(&mut engine, 3), dom::Vector2D::zero());
    adopt_at(&mut engine, 1.0);
    frame_at(&mut engine, 1.05);
    let mid = live_offset(&mut engine, 3).x;
    assert!(mid > 0.0 && mid < 600.0, "mid-glide, got {mid}");
    assert!(engine.painter.scroll_intents.is_gliding(node_id(3)));

    engine.painter.clock.pin(1.06);
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(30.0, 0.0),
    ));
    let landed = live_offset(&mut engine, 3).x;
    assert!(
        landed > mid && landed % 200.0 == 0.0,
        "the wheel stepped to the next page ahead of {mid}, got {landed}"
    );
    assert!(!engine.painter.scroll_intents.is_gliding(node_id(3)));
    assert!(!engine.is_animating(), "the glide ended");
    for at in [1.1, 1.3, 2.0] {
        frame_at(&mut engine, at);
        assert!(
            (live_offset(&mut engine, 3).x - landed).abs() < f32::EPSILON,
            "the wheel's position stands at {at}"
        );
    }
    engine.painter.publish_scroll(2.0, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(landed, 0.0)
    );
    assert_eq!(
        engine
            .probe_document(|document| document.pending_scroll_request(node_id(3)))
            .expect("the view's task answers probes"),
        None,
        "the post named the request"
    );
}

// --- `<viewpager>` built from its own tags -----------------------------------

/// A 200px `viewpager` (node 3) of five `viewpager-item` pages (nodes 4–8),
/// styled by the UA sheet alone apart from the pager's size, with `setup`
/// run before the first flush. A tap on the pager calls
/// `selectTab({index: 2, smooth: false})` and logs the answer's code in the
/// pager's `log` attribute.
fn viewpager_page(setup: &str) -> String {
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const pager = __CreateElement('viewpager', 0);
          __AppendElement(page, pager);
          globalThis.held = [page, pager];
          __SetInlineStyles(pager, 'width:200px;height:200px');
          for (let i = 0; i < 5; i++) {{
            const item = __CreateElement('viewpager-item', 0);
            __AppendElement(pager, item);
            held.push(item);
          }}
          __AddEventListener(pager, 'tap', () => {{
            __InvokeUIMethod(pager, 'selectTab', {{index: 2, smooth: false}}, result => {{
              __SetAttribute(pager, 'log', 'selectTab:' + result.code);
            }});
          }}, {{}});
          {setup}
          __FlushElementTree();
        }};
        "
    )
}

/// Runs display frames until nothing animates, and answers where the
/// pager came to rest.
fn frames_until_rest(engine: &mut TestEngine, mut at: f64) -> f32 {
    let deadline = at + 3.0;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < deadline, "never came to rest");
        frame_at(engine, at);
    }
    live_offset(engine, 3).x
}

/// A quiet release under half a page glides back to the page; one past half
/// glides on to the next.
#[test]
fn a_viewpager_drag_returns_under_half_a_page_and_turns_past_it() {
    for (to, dragged, rest) in [(100.0, 42.0, 0.0), (30.0, 112.0, 200.0)] {
        let mut engine = booted(&viewpager_page(""));
        touch_x_at(&mut engine, 0.0, PointerPhase::Down, 150.0);
        touch_x_at(&mut engine, 0.05, PointerPhase::Move, to);
        let released = live_offset(&mut engine, 3).x;
        assert!((released - dragged).abs() < 0.5, "got {released}");
        touch_x_at(&mut engine, 0.4, PointerPhase::Up, to);
        assert!(engine.is_animating(), "a glide, not a jump");
        let landed = frames_until_rest(&mut engine, 0.4);
        assert!(
            (landed - rest).abs() < f32::EPSILON,
            "dragged to {dragged}, came to rest at {landed}"
        );
    }
}

/// `select-index="2"`: the first frame the painter adopts is on page 2 and
/// rests there. The user then swipes to page 3, main adopts it without a
/// commit, and the card changes the attribute to 0: the commit that follows
/// carries the new initial target as an instant request, so the painter
/// shows page 0 rather than keeping the offset it held for the swipe.
#[test]
fn a_selected_page_is_the_first_frame_and_a_new_index_overrides_a_swipe() {
    let mut engine = booted(&viewpager_page(
        "__SetAttribute(pager, 'select-index', '2');",
    ));
    adopt_at(&mut engine, 0.0);
    assert!((live_offset(&mut engine, 3).x - 400.0).abs() < f32::EPSILON);
    assert!(!engine.is_animating(), "at rest on the page");
    engine.painter.publish_scroll(0.0, None);
    assert_eq!(
        engine
            .probe_document(|document| document.pending_scroll_request(node_id(3)))
            .expect("the view's task answers probes"),
        None,
        "the painter's post acknowledged the first commit's request"
    );

    touch_x_at(&mut engine, 1.0, PointerPhase::Down, 190.0);
    touch_x_at(&mut engine, 1.05, PointerPhase::Move, 30.0);
    touch_x_at(&mut engine, 1.4, PointerPhase::Up, 30.0);
    assert!((frames_until_rest(&mut engine, 1.4) - 600.0).abs() < f32::EPSILON);
    engine.painter.publish_scroll(3.0, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(600.0, 0.0),
        "main adopted the swipe"
    );

    engine
        .probe_document(|document| document.set_attribute(node_id(3), "select-index", "0"))
        .expect("the view's task answers probes");
    // A later entry, so the attribute's own commit has been published too.
    assert_eq!(scroll_offset_of(&mut engine, 3), dom::Vector2D::zero());
    adopt_at(&mut engine, 3.1);
    assert!(
        live_offset(&mut engine, 3).x.abs() < f32::EPSILON,
        "the painter shows the new page, got {}",
        live_offset(&mut engine, 3).x
    );
    frame_at(&mut engine, 3.5);
    assert!(live_offset(&mut engine, 3).x.abs() < f32::EPSILON);
    assert!(!engine.is_animating());
}

/// A flick moves exactly one page: the UA sheet's `scroll-snap-stop:
/// always` on every page stops the fling the flick would carry past it.
#[test]
fn a_viewpager_flick_turns_exactly_one_page() {
    let mut engine = booted(&viewpager_page(""));
    flick_x(&mut engine, 0.0);
    let mut at = 0.02;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < 1.0, "never came to rest");
        frame_at(&mut engine, at);
        assert!(
            live_offset(&mut engine, 3).x <= 200.0,
            "passed the next page"
        );
    }
    assert!((live_offset(&mut engine, 3).x - 200.0).abs() < f32::EPSILON);
}

/// `enable-scroll="false"`: a drag moves nothing, and `selectTab` still turns
/// the pager — in the document at once, on the painter at its next frame.
#[test]
fn a_viewpager_that_cannot_scroll_ignores_drags_but_not_select_tab() {
    let mut engine = booted(&viewpager_page(
        "__SetAttribute(pager, 'enable-scroll', 'false');",
    ));
    touch_x_at(&mut engine, 0.0, PointerPhase::Down, 190.0);
    touch_x_at(&mut engine, 0.05, PointerPhase::Move, 30.0);
    touch_x_at(&mut engine, 0.4, PointerPhase::Up, 30.0);
    assert!(!engine.is_animating());
    assert!(live_offset(&mut engine, 3).x.abs() < f32::EPSILON);

    engine.painter.clock.pin(1.0);
    engine.dispatch_input(touch(1, PointerPhase::Down, 100.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 100.0));
    wait_for_log(&mut engine, "selectTab:0");
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(400.0, 0.0),
        "an instant turn moves the document at once"
    );
    adopt_at(&mut engine, 1.1);
    assert!((live_offset(&mut engine, 3).x - 400.0).abs() < f32::EPSILON);
    assert!(!engine.is_animating());
}

/// `bounces`: a drag past the first page and one past the last stretch the
/// pager, and each springs back to its edge. Without it, neither edge moves.
#[test]
fn a_bouncing_viewpager_stretches_at_both_ends_and_springs_back() {
    for bounces in [true, false] {
        let setup = if bounces {
            "__SetAttribute(pager, 'bounces', 'true');"
        } else {
            ""
        };
        let mut engine = booted(&viewpager_page(setup));
        for (edge, from, to, start) in [(0.0, 50.0, 150.0, 0.0), (800.0, 150.0, 50.0, 2.0)] {
            if edge > 0.0 {
                engine
                    .probe_document(|document| {
                        document.scroll_to_with(
                            node_id(3),
                            dom::Vector2D::new(800.0, 0.0),
                            dom::scroll::ScrollBehavior::Instant,
                        )
                    })
                    .expect("the view's task answers probes");
                // A later entry, so the request's own commit is published.
                assert_eq!(
                    scroll_offset_of(&mut engine, 3),
                    dom::Vector2D::new(800.0, 0.0)
                );
                adopt_at(&mut engine, start - 0.1);
                assert!((live_offset(&mut engine, 3).x - edge).abs() < f32::EPSILON);
            }
            touch_x_at(&mut engine, start, PointerPhase::Down, from);
            touch_x_at(&mut engine, start + 0.05, PointerPhase::Move, to);
            let stretch = live_offset(&mut engine, 3).x - edge;
            if bounces {
                // 92px past the edge on a 200px scrollport: the rubber band's
                // `(1 − 1/(92·0.55/200 + 1))·200` ≈ 40.4px.
                assert!(
                    (stretch.abs() - 40.4).abs() < 0.2,
                    "stretched past {edge}: {stretch}"
                );
                assert_eq!(stretch < 0.0, edge == 0.0, "outward at {edge}");
            } else {
                assert!(stretch.abs() < f32::EPSILON, "{edge} is a wall: {stretch}");
            }
            touch_x_at(&mut engine, start + 0.4, PointerPhase::Up, to);
            let rest = frames_until_rest(&mut engine, start + 0.4);
            assert!(
                (rest - edge).abs() < f32::EPSILON,
                "back on {edge}, got {rest} (bounces={bounces})"
            );
        }
    }
}

/// A vertical `scroll-view` filling the first page scrolls vertically under
/// a vertical drag and leaves the pager alone; a horizontal drag starting on
/// it passes through to the pager, which turns.
#[test]
fn a_vertical_scroller_inside_a_page_scrolls_and_hands_horizontal_drags_to_the_pager() {
    let mut engine = booted(&viewpager_page(
        "const inner = __CreateScrollView(0);
         __AppendElement(held[2], inner);
         __SetInlineStyles(inner, 'width:100%;height:100%');
         const tall = __CreateView(0);
         __AppendElement(inner, tall);
         __SetInlineStyles(tall, 'flex-shrink:0;width:200px;height:1000px');
         held.push(inner, tall);",
    ));
    touch_at(&mut engine, 0.0, PointerPhase::Down, 150.0);
    touch_at(&mut engine, 0.05, PointerPhase::Move, 50.0);
    touch_at(&mut engine, 0.4, PointerPhase::Up, 50.0);
    frames_until_rest(&mut engine, 0.4);
    let inner = live_offset(&mut engine, 9);
    assert!(
        (inner.y - 92.0).abs() < 0.5,
        "the scroll-view scrolled: {inner:?}"
    );
    assert!(inner.x.abs() < f32::EPSILON);
    assert!(
        live_offset(&mut engine, 3).x.abs() < f32::EPSILON,
        "the pager stayed"
    );

    touch_x_at(&mut engine, 1.0, PointerPhase::Down, 190.0);
    touch_x_at(&mut engine, 1.05, PointerPhase::Move, 30.0);
    touch_x_at(&mut engine, 1.4, PointerPhase::Up, 30.0);
    let rest = frames_until_rest(&mut engine, 1.4);
    assert!(
        (rest - 200.0).abs() < f32::EPSILON,
        "the pager turned: {rest}"
    );
    assert!(
        (live_offset(&mut engine, 9).y - inner.y).abs() < f32::EPSILON,
        "the scroll-view kept its offset"
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
/// inside the encode window is composed from the painting side's intents;
/// the document adopts the posted offset, but inside the window and short
/// of half its headroom nothing recommits — and hit testing follows the
/// intent offsets, not the committed ones, so a tap lands on what the
/// screen shows.
#[test]
fn a_windowed_scroll_recommits_nothing_and_hits_route_at_the_intent_offsets() {
    let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
    let frame = engine.published_frame().expect("boot published a frame");
    let boot_commit = frame.commit_id();
    drop(frame);

    // 30px is inside half the encode-window headroom (the 200px
    // scrollport), so no recentering commit is due either.
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 30.0),
    ));
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(0.0, 30.0),
        "the document adopts the posted offset"
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

/// An adopted offset past half the encode-window headroom is
/// `recenter_due` on the committed slot, so main recommits: the frame
/// re-centers its window on the scrolled offset, all without any script
/// involvement.
#[test]
fn main_recenters_when_the_adopted_offset_is_recenter_due() {
    let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
    let boot_commit = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    // max_offset is 200 (400px of rows in a 200px scrollport), so the
    // window tops out at 200 and 150 is past half its headroom.
    let boot = engine.published_frame().expect("boot published a frame");
    let boot_slot = boot.scroll_slots()[boot.slot_of(node_id(3)).expect("a slot") as usize];
    assert!(boot_slot.recenter_due(dom::Vector2D::new(0.0, 150.0)));
    assert!(boot.covers_scroll_offset(node_id(3), dom::Vector2D::new(0.0, 150.0)));
    drop(boot);
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
            "the recentering commit never published"
        );
        std::thread::yield_now();
    };
    let scroller = node_id(3);
    let slot = frame.scroll_slots()[frame.slot_of(scroller).expect("a slot") as usize];
    assert!(
        (slot.offset.y - 150.0).abs() < 0.5,
        "the commit publishes the scrolled offset, got {:?}",
        slot.offset
    );
    assert!(
        !slot.recenter_due(dom::Vector2D::new(0.0, 150.0)),
        "and its window is centered on it"
    );
}

/// A layout read on the main thread sees the user's scroll once the marker
/// is serviced: `boundingClientRect` subtracts the adopted offset.
#[test]
fn a_bounding_client_rect_follows_an_adopted_scroll() {
    let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 30.0),
    ));
    let rect = engine
        .probe_document(|tree| tree.bounding_client_rect(node_id(5)))
        .expect("the view's task answers probes")
        .expect("the second row is laid out");
    assert!(
        (rect.origin.y - 170.0).abs() < 0.5,
        "the second row sits 30px higher, got {rect:?}"
    );
}

/// Polls `log` until it holds `count` entries, then answers them. The
/// delivery is an entry of its own, queued behind the one that updated, so
/// one probe round trip is not enough to have seen it.
fn wait_for_watched(engine: &mut TestEngine, log: &WatcherLog, count: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    loop {
        seen.extend(take_watched(log));
        if seen.len() >= count {
            // A later entry: the delivery's own epilogue has run by the time
            // this answers, so whatever it committed is published.
            engine
                .probe_document(|_| ())
                .expect("the view's task answers probes");
            seen.extend(take_watched(log));
            return seen;
        }
        assert!(
            Instant::now() < deadline,
            "expected {count} entries, saw {seen:?}"
        );
        std::thread::yield_now();
    }
}

/// An engine component's intersection observer, end to end through the real
/// painter: a wheel scroll inside the encode window is composed on the
/// painting side, its offset posted and adopted on the main thread, the
/// update run against it by that entry's epilogue, and the entries delivered
/// to the component by the entry after — with no commit anywhere along the
/// way, because a windowed scroll recommits nothing.
///
/// Rooted at the 200px scroller, the first row starts wholly visible and the
/// second sits exactly at the scrollport's bottom edge: intersecting, at
/// ratio 0, because intersection is edge-inclusive. The 0.1 threshold is
/// what makes the second row's arrival a crossing — under `[0, 1]` alone,
/// ratio 0 on the edge and ratio 0.15 thirty pixels later share a band.
#[test]
fn a_windowed_scroll_delivers_intersections_to_an_engine_component() {
    let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
    let log: WatcherLog = Arc::default();
    let watcher = IntersectionWatcher::new(&log);
    let observer = engine
        .probe_document(move |document| {
            document.define("x-watcher", Box::new(watcher));
            let root = document.document_element().id();
            let element = document.create_element("x-watcher", ());
            document.set_inline_style(element, "position:absolute;width:0;height:0");
            document.append_child(root, element);
            let (scroller, first, second) = (node_id(3), node_id(4), node_id(5));
            document.set_id_attribute(first, Some("first"));
            document.set_id_attribute(second, Some("second"));
            let observer = document.create_intersection_observer(
                dom::IntersectionObserverOwner::Element(element),
                Some(scroller),
                dom::RootMargin::ZERO,
                vec![0.0, 0.1, 1.0],
            );
            document.observe_intersection(observer, first);
            document.observe_intersection(observer, second);
            observer.get()
        })
        .expect("the view's task answers probes");
    assert_eq!(
        wait_for_watched(&mut engine, &log, 2),
        [
            format!("{observer}:first:true:1"),
            format!("{observer}:second:true:0"),
        ],
        "the first update reports both rows, the edge-adjacent one as intersecting",
    );
    let committed = engine
        .published_frame()
        .expect("the probe's commit is published")
        .commit_id();

    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 30.0),
    ));
    assert_eq!(
        wait_for_watched(&mut engine, &log, 2),
        [
            format!("{observer}:first:true:0.85"),
            format!("{observer}:second:true:0.15"),
        ],
        "the row that left ratio 1 and the row that crossed 0.1, in observe order",
    );
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        dom::Vector2D::new(0.0, 30.0),
        "the document adopted the posted offset",
    );
    assert_eq!(
        engine
            .published_frame()
            .expect("still published")
            .commit_id(),
        committed,
        "a windowed scroll commits nothing, and the delivery wrote nothing",
    );
}

/// A 200px scroller (node 3) whose first row (node 4) recolors along a
/// `scroll()` timeline of 1000px: `color` never exports, so only the main
/// thread's cascade moves it.
fn booted_scroll_driven() -> TestEngine {
    TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const scroller = __CreateView(0);
          const row = __CreateView(0);
          const filler = __CreateView(0);
          __AppendElement(page, scroller);
          __AppendElement(scroller, row);
          __AppendElement(scroller, filler);
          globalThis.held = [page, scroller, row, filler];
          __SetInlineStyles(scroller, 'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px');
          __SetInlineStyles(row, 'flex-shrink:0;width:200px;height:20px');
          __SetInlineStyles(filler, 'flex-shrink:0;width:200px;height:1180px');
          __SetClasses(row, 'driven');
          __FlushElementTree();
        };
        ",
    )
    .with_style_sheet(
        ".driven { animation: recolor linear both; animation-timeline: scroll(); }
         @keyframes recolor { from { color: rgb(0, 0, 0); } to { color: rgb(0, 0, 250); } }",
    )
    .boot()
}

/// The row's blue channel as the document computes it, in `[0, 1]`.
fn blue_of(engine: &mut TestEngine, node: u64) -> f32 {
    engine
        .probe_document(move |tree| {
            tree.get(node_id(node))
                .and_then(dom::Node::computed_style)
                .map(|style| style.get_color().solid_color().components.2)
        })
        .flatten()
        .expect("the row is styled")
}

/// A scroll-driven animation the painter does not sample follows the user's
/// scroll on the main thread: the offset the painter posts is adopted at the
/// marker and re-samples the row, and the commit that follows carries its
/// color. Nothing ticks: the frame asks for no frame posts, and the painter
/// is idle at rest.
#[test]
fn a_scroll_driven_color_follows_the_users_scroll() {
    let mut engine = booted_scroll_driven();
    let boot = engine.published_frame().expect("boot published a frame");
    assert!(!boot.animations_active() && !boot.needs_main_ticks());
    assert!(!boot.has_exported_curves(), "color never exports");
    drop(boot);
    assert!(
        blue_of(&mut engine, 4).abs() < 1e-6,
        "the boot commit samples offset 0"
    );

    drag(&mut engine, 150.0, 50.0);
    // Whatever the release flung comes to rest first.
    let mut at = 1.0;
    while engine.is_animating() {
        assert!(at < 10.0, "the fling never came to rest");
        at += 0.016;
        frame_at(&mut engine, at);
    }
    let dragged = scroll_offset_of(&mut engine, 3).y;
    assert!(dragged > 50.0, "the drag scrolled, got {dragged}");
    let expected = dragged / 1000.0 * 250.0 / 255.0;
    assert!(
        (blue_of(&mut engine, 4) - expected).abs() < 1e-3,
        "the color follows the adopted offset {dragged}"
    );

    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 100.0),
    ));
    let offset = scroll_offset_of(&mut engine, 3).y;
    assert!(
        (blue_of(&mut engine, 4) - offset / 1000.0 * 250.0 / 255.0).abs() < 1e-3,
        "and every further scroll, at {offset}"
    );
    let frame = engine.published_frame().expect("still published");
    assert!(!frame.animations_active() && !frame.needs_main_ticks());
    assert!(
        engine.painter.begin_frame(at + 1.0, false).is_none(),
        "no frame post crosses for a scroll-driven animation"
    );
    assert!(!engine.is_animating(), "the painter is idle at rest");
}

/// A tap finds a header a list below it drives by a named scroll timeline
/// where the list's scroll carries it: hit testing samples the exported
/// curve at the painter's offsets, with no commit in between.
#[test]
fn a_tap_finds_a_scroll_driven_header_where_the_scroll_carries_it() {
    let mut engine = TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const header = __CreateView(0);
          const list = __CreateView(0);
          const filler = __CreateView(0);
          __AppendElement(page, header);
          __AppendElement(page, list);
          __AppendElement(list, filler);
          globalThis.held = [page, header, list, filler];
          __SetClasses(header, 'header');
          __SetClasses(list, 'list');
          __SetClasses(filler, 'filler');
          __AddEventListener(header, 'tap', (event) => {
            __SetAttribute(header, 'log', 'tap:' + event.detail.x);
          }, {});
          __FlushElementTree();
        };
        ",
    )
    .with_style_sheet(
        ".header { position: absolute; left: 0px; top: 0px; width: 40px; height: 40px;
                   animation: slide linear both; animation-timeline: --list; }
         .list { position: absolute; left: 0px; top: 100px; width: 200px; height: 200px;
                 display: flex; flex-direction: column; overflow: scroll;
                 scroll-timeline: --list; }
         .filler { flex-shrink: 0; width: 200px; height: 400px; }
         @keyframes slide { from { transform: translateX(0px); }
                            to { transform: translateX(200px); } }",
    )
    .boot();
    let boot = engine.published_frame().expect("boot committed a frame");
    assert!(boot.has_exported_curves() && !boot.has_live_curves());
    // 100px of travel less the 8px slop: the header slides 92px right.
    touch_at(&mut engine, 0.0, PointerPhase::Down, 250.0);
    touch_at(&mut engine, 0.05, PointerPhase::Move, 150.0);
    touch_at(&mut engine, 0.5, PointerPhase::Up, 150.0);
    assert!((intent_y(&engine, 4) - 92.0).abs() < 0.5);
    assert!(
        (scroll_offset_of(&mut engine, 4).y - 92.0).abs() < 0.5,
        "main adopted the scroll"
    );
    assert_eq!(
        engine
            .published_frame()
            .expect("still published")
            .commit_id(),
        boot.commit_id(),
        "and no commit carried the header"
    );
    engine.dispatch_input(touch(1, PointerPhase::Down, 110.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 110.0));
    wait_for_log(&mut engine, "tap:110");
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

/// Posts one frame request and waits for the commit it implies to publish.
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
        "no frame post crosses while the curve covers the animation"
    );
}

/// The tick that anchors a delayed animation changes no style, yet it
/// settles the animation's start, so it commits: the frame it publishes
/// carries the curve and frees the main thread for the whole delay.
#[test]
fn the_tick_anchoring_a_delayed_animation_commits_its_curve() {
    let mut engine = booted_animated(
        "view { width: 100px; height: 100px; background-color: red;
                    animation: fade 1s linear 5s infinite both; }
             @keyframes fade { from { opacity: 1; } to { opacity: 0; } }",
    );
    assert!(
        !engine
            .published_frame()
            .expect("the boot flush published")
            .has_exported_curves(),
        "an unanchored delay does not export"
    );

    synchronized_tick(&mut engine, 0.1);
    let frame = engine.published_frame().expect("the anchoring committed");
    assert!(frame.has_exported_curves(), "the anchored delay exports");
    assert!(!frame.needs_main_ticks());
}

/// A transparent tap target sliding by an exported curve draws nothing, so
/// the compose program names no curve; a tap still finds it where the curve
/// carries it at the tap's clock reading, not where it was committed.
#[test]
fn a_tap_finds_an_inkless_mover_where_its_curve_carries_it() {
    let mut engine = TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateView(0);
          __AppendElement(page, view);
          globalThis.held = [page, view];
          __SetClasses(view, 'hotspot');
          __AddEventListener(view, 'tap', (event) => {
            __SetAttribute(view, 'log', 'tap:' + event.detail.x);
          }, {});
          __FlushElementTree();
        };
        ",
    )
    .with_style_sheet(
        ".hotspot { position: absolute; left: 0px; top: 0px; width: 100px; height: 100px;
                    animation: slide 1s linear infinite; }
         @keyframes slide { from { transform: translateX(0px); }
                            to { transform: translateX(250px); } }",
    )
    .boot();
    synchronized_tick(&mut engine, 0.1);
    let frame = engine.published_frame().expect("the promotion committed");
    assert!(frame.has_exported_curves(), "the slide exported");
    assert!(!frame.has_live_curves(), "and draws nothing");
    // Committed at x ≤ 25; at 0.9 the slide has carried it to x ≥ 200,
    // whether its first tick was at 0 or at 0.1.
    engine.painter.clock.pin(0.9);
    engine.dispatch_input(touch(1, PointerPhase::Down, 260.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 260.0));
    wait_for_log(&mut engine, "tap:260");
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

/// One windowed-painter frame: adopt whatever the view published, then post a
/// frame request only if that frame asks for one — the protocol
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

/// A windowed painter sends no frame post while nothing is moving, so an
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
    // frame post has crossed since boot and the document's timeline still
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

/// A 400px containing block (node 3) holding a scroller (node 4) whose
/// content (node 5) carries the anchor (node 6) at `anchor_css`'s place, and,
/// outside the scroller, the anchored box (node 7) styled `anchored_css`.
fn booted_anchored(sheet: &str) -> TestEngine {
    TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const cb = __CreateView(0);
          const scroller = __CreateView(0);
          const content = __CreateView(0);
          const anchor = __CreateView(0);
          const anchored = __CreateView(0);
          __AppendElement(page, cb);
          __AppendElement(cb, scroller);
          __AppendElement(scroller, content);
          __AppendElement(content, anchor);
          __AppendElement(cb, anchored);
          globalThis.held = [page, cb, scroller, content, anchor, anchored];
          __SetClasses(cb, 'cb');
          __SetClasses(scroller, 'scroller');
          __SetClasses(content, 'content');
          __SetClasses(anchor, 'anchor');
          __SetClasses(anchored, 'anchored');
          __FlushElementTree();
        };
        ",
    )
    .with_style_sheet(sheet)
    .boot()
}

/// The page [`booted_anchored`] builds with a 200×100 scroller over 500px
/// square content, the anchor 40×30 at (50, 50), and `anchored` added to the
/// anchored box's 10px square.
fn anchor_sheet(anchored: &str) -> String {
    format!(
        ".cb {{ display: flex; flex-direction: column; position: relative;
               width: 400px; height: 400px; }}
         .scroller {{ display: flex; flex-direction: column; flex-shrink: 0;
                     overflow: scroll; width: 200px; height: 100px; }}
         .content {{ display: flex; flex-direction: column; flex-shrink: 0;
                    width: 500px; height: 500px; }}
         .anchor {{ flex-shrink: 0; anchor-name: --a; width: 40px; height: 30px;
                   margin-top: 50px; margin-left: 50px; }}
         .anchored {{ position: absolute; position-anchor: --a;
                     width: 10px; height: 10px; {anchored} }}"
    )
}

/// What the painter's hit test finds at `(x, y)`, at its own offsets.
fn painter_hit(engine: &mut TestEngine, x: f32, y: f32) -> Option<dom::NodeId> {
    let frame = engine.published_frame()?;
    let intents = &engine.painter.scroll_intents;
    frame
        .hit(
            Point2D::new(x, y),
            &|slot| intents.offset_for(slot.node),
            None,
        )
        .map(|target| target.node)
}

/// What the document's own hit test finds at `(x, y)`, at its adopted
/// offsets.
fn main_hit(engine: &mut TestEngine, x: f32, y: f32) -> Option<dom::NodeId> {
    engine
        .probe_document(move |tree| {
            tree.elements_from_point(Point2D::new(x, y))
                .first()
                .copied()
        })
        .flatten()
}

fn wheel(engine: &mut TestEngine, dx: f32, dy: f32) {
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 50.0),
        dom::Vector2D::new(dx, dy),
    ));
}

/// css-anchor-position-1 §3.3 from the engine's side (WPT `anchor-scroll-001`
/// adapted): a box placed below its anchor by `position-area` follows the
/// anchor on both axes while the scroller between them moves — composed by
/// the painter at its intents, hit where it is drawn on both threads, and
/// never committed.
#[test]
fn an_anchored_box_follows_its_anchor_through_a_scroll_on_both_axes() {
    let mut engine = booted_anchored(&anchor_sheet("position-area: bottom center;"));
    let anchored = node_id(7);
    // Centered under the anchor: (65, 80), 10×10.
    assert_eq!(painter_hit(&mut engine, 70.0, 85.0), Some(anchored));
    let boot = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    wheel(&mut engine, 20.0, 30.0);
    assert_eq!(
        scroll_offset_of(&mut engine, 4),
        dom::Vector2D::new(20.0, 30.0),
        "main adopted the scroll"
    );
    assert_eq!(painter_hit(&mut engine, 50.0, 55.0), Some(anchored));
    assert_ne!(painter_hit(&mut engine, 70.0, 85.0), Some(anchored));
    assert_eq!(main_hit(&mut engine, 50.0, 55.0), Some(anchored));
    assert_eq!(
        engine
            .published_frame()
            .expect("still published")
            .commit_id(),
        boot,
        "the painter composed the shift: no commit"
    );
}

/// §3.3's compensation is per axis: a box whose only anchor reference is a
/// vertical `anchor()` follows the anchor up and stays where it is across.
#[test]
fn an_anchored_box_follows_only_on_the_axes_it_compensates() {
    let mut engine = booted_anchored(&anchor_sheet("top: anchor(bottom); left: 150px;"));
    let anchored = node_id(7);
    assert_eq!(painter_hit(&mut engine, 155.0, 85.0), Some(anchored));
    wheel(&mut engine, 20.0, 30.0);
    assert_eq!(
        scroll_offset_of(&mut engine, 4),
        dom::Vector2D::new(20.0, 30.0)
    );
    assert_eq!(painter_hit(&mut engine, 155.0, 55.0), Some(anchored));
    assert_ne!(
        painter_hit(&mut engine, 135.0, 55.0),
        Some(anchored),
        "the horizontal scroll does not carry it"
    );
}

/// §6.6 `position-visibility: anchor-visible`, the initial value, flips
/// while scrolling: the box hides once its anchor is scrolled wholly out of
/// the scroller's clip and shows again when it comes back — each frame, on
/// the painter. (The 70px step is past half the 100px scrollport's encode
/// headroom, so a recentering commit follows it; the flips do not wait on
/// one.)
#[test]
fn an_anchored_box_hides_while_its_anchor_is_scrolled_away() {
    let mut engine = booted_anchored(&anchor_sheet("position-area: bottom center;"));
    let anchored = node_id(7);
    // The anchor at y −20..10 still shows 10px: the box at y 10..20 shows.
    wheel(&mut engine, 0.0, 70.0);
    assert_eq!(painter_hit(&mut engine, 70.0, 15.0), Some(anchored));
    // At −35..−5 it is gone, and so is the box at −5..5.
    wheel(&mut engine, 0.0, 15.0);
    assert!((scroll_offset_of(&mut engine, 4).y - 85.0).abs() < 0.5);
    assert_ne!(painter_hit(&mut engine, 70.0, 2.0), Some(anchored));
    assert_ne!(main_hit(&mut engine, 70.0, 2.0), Some(anchored));
    wheel(&mut engine, 0.0, -85.0);
    assert_eq!(painter_hit(&mut engine, 70.0, 85.0), Some(anchored));
}

/// §6.6 `no-overflow` flips while scrolling: the box shifted past its
/// containing block's top edge hides.
#[test]
fn a_no_overflow_box_hides_once_the_shift_pushes_it_out() {
    let mut engine = booted_anchored(&anchor_sheet(
        "bottom: anchor(top); left: anchor(left); height: 40px;
         position-visibility: no-overflow;",
    ));
    let anchored = node_id(7);
    // Above the anchor: y 10..50.
    assert_eq!(painter_hit(&mut engine, 55.0, 20.0), Some(anchored));
    wheel(&mut engine, 0.0, 5.0);
    assert_eq!(painter_hit(&mut engine, 55.0, 10.0), Some(anchored));
    wheel(&mut engine, 0.0, 6.0);
    assert!((scroll_offset_of(&mut engine, 4).y - 11.0).abs() < 0.5);
    assert_ne!(painter_hit(&mut engine, 55.0, 10.0), Some(anchored));
}

/// Waits for a commit newer than `after` to publish, and answers its id.
fn next_commit(engine: &mut TestEngine, after: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let commit = engine
            .published_frame()
            .expect("still published")
            .commit_id();
        if commit > after {
            return commit;
        }
        assert!(Instant::now() < deadline, "no commit followed the scroll");
        std::thread::yield_now();
    }
}

/// §6.5 on scroll (WPT `anchor-scroll-position-try-001` adapted): a box
/// above its anchor, scrolled up past its containing block's top edge,
/// switches to its `flip-block` fallback below the anchor in the commit that
/// answers the adopted offset; scrolled back until the fallback overflows
/// the bottom edge, it switches back. The painter's post is the whole wake:
/// main re-runs the determination where it adopts the offsets.
#[test]
fn a_scroll_that_pushes_an_anchored_box_out_switches_its_fallback_and_back() {
    let mut engine = booted_anchored(
        ".cb { display: flex; flex-direction: column; position: relative;
               width: 400px; height: 400px; }
         .scroller { display: flex; flex-direction: column; flex-shrink: 0;
                     overflow: scroll; width: 400px; height: 400px; }
         .content { display: flex; flex-direction: column; flex-shrink: 0;
                    width: 400px; height: 1000px; }
         .anchor { flex-shrink: 0; anchor-name: --a; width: 40px; height: 30px;
                   margin-top: 250px; }
         .anchored { position: absolute; position-anchor: --a; position-area: top;
                     position-try-fallbacks: flip-block; width: 10px; height: 150px; }",
    );
    let anchored = node_id(7);
    // Above the anchor, y 100..250, centered on it at x 15..25.
    assert_eq!(painter_hit(&mut engine, 20.0, 200.0), Some(anchored));
    let boot = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    // 100px up still fits: 0..150, no commit.
    wheel(&mut engine, 0.0, 100.0);
    assert!((scroll_offset_of(&mut engine, 4).y - 100.0).abs() < 0.5);
    assert_eq!(painter_hit(&mut engine, 20.0, 100.0), Some(anchored));
    assert_eq!(
        engine.published_frame().expect("published").commit_id(),
        boot
    );

    // 200px up would put it at −100..50: below the anchor at 50..80
    // instead, 80..230.
    wheel(&mut engine, 0.0, 100.0);
    let switched = next_commit(&mut engine, boot);
    assert_eq!(painter_hit(&mut engine, 20.0, 150.0), Some(anchored));
    assert_eq!(main_hit(&mut engine, 20.0, 150.0), Some(anchored));

    // Back to the top: below the anchor at 250..280 it would overflow
    // (280..430), and above it fits again.
    wheel(&mut engine, 0.0, -200.0);
    next_commit(&mut engine, switched);
    assert_eq!(painter_hit(&mut engine, 20.0, 200.0), Some(anchored));
    assert_ne!(painter_hit(&mut engine, 20.0, 300.0), Some(anchored));
}

/// A `<scroll-coordinator>`'s shape in plain CSS: a 400px column scroller
/// (node 3) carrying `outer_css`, holding a 200px header block (node 4) and
/// a 400px-tall inner column scroller (node 5) of ten 100px items with
/// `scroll-capture-y: nearest forward`. The outer's range is 200 (header plus
/// inner, 600, less its 400px scrollport), the inner's 600.
fn coordinator_page(outer_css: &str) -> String {
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const outer = __CreateView(0);
          const header = __CreateView(0);
          const inner = __CreateView(0);
          __AppendElement(page, outer);
          __AppendElement(outer, header);
          __AppendElement(outer, inner);
          globalThis.held = [page, outer, header, inner];
          __SetInlineStyles(outer, 'display:flex;flex-direction:column;overflow:scroll;width:200px;height:400px;{outer_css}');
          __SetInlineStyles(header, 'flex-shrink:0;width:200px;height:200px');
          __SetInlineStyles(inner, 'display:flex;flex-direction:column;flex-shrink:0;overflow:scroll;width:200px;height:400px;scroll-capture-y:nearest forward');
          for (let i = 0; i < 10; i++) {{
            const item = __CreateView(0);
            __AppendElement(inner, item);
            held.push(item);
            __SetInlineStyles(item, 'flex-shrink:0;width:200px;height:100px');
          }}
          __FlushElementTree();
        }};
        "
    )
}

/// A drag at `at` from `from_y` to `to_y` in one move, released half a
/// second later so it carries no velocity: a scroll of
/// `from_y − to_y` less the 8px slop, and no fling after it.
fn quiet_drag(engine: &mut TestEngine, at: f64, from_y: f32, to_y: f32) {
    touch_at(engine, at, PointerPhase::Down, from_y);
    touch_at(engine, at + 0.01, PointerPhase::Move, to_y);
    touch_at(engine, at + 0.5, PointerPhase::Up, to_y);
}

/// The coordinator's and the inner scroller's vertical offsets.
fn coordinator_offsets(engine: &mut TestEngine) -> (f32, f32) {
    (live_offset(engine, 3).y, live_offset(engine, 5).y)
}

/// A forward drag that starts in the inner scroller folds the coordinator
/// first: 100px of scroll all go to the coordinator, the inner stays.
#[test]
fn a_forward_drag_in_the_inner_scroller_folds_the_coordinator_first() {
    let mut engine = booted(&coordinator_page(""));
    // The inner scroller spans 200..600 in the coordinator; 350 is in it.
    quiet_drag(&mut engine, 0.0, 350.0, 242.0);
    assert_eq!(coordinator_offsets(&mut engine), (100.0, 0.0));
}

/// The fold, then the inner content, then the other way round: forward 300
/// folds the coordinator to its 200 maximum and gives the inner the rest;
/// backward 50 goes to the inner first; backward 300 empties the inner's 50
/// and unfolds the coordinator by the 250 left — all 200 of it, the last
/// 50 going nowhere.
#[test]
fn forward_folds_before_the_inner_scroller_and_backward_unfolds_after_it() {
    let mut engine = booted(&coordinator_page(""));
    quiet_drag(&mut engine, 0.0, 390.0, 82.0);
    assert_eq!(coordinator_offsets(&mut engine), (200.0, 100.0));

    // Folded, the inner scroller spans the whole 0..400 scrollport.
    quiet_drag(&mut engine, 1.0, 100.0, 158.0);
    assert_eq!(coordinator_offsets(&mut engine), (200.0, 50.0));

    quiet_drag(&mut engine, 2.0, 50.0, 358.0);
    assert_eq!(coordinator_offsets(&mut engine), (0.0, 0.0));
}

/// A forward fling from the inner scroller is captured on every frame: it
/// folds the coordinator, and once that is at its maximum the rest of the
/// fling carries on into the inner content until it comes to rest there.
#[test]
fn a_forward_fling_folds_the_coordinator_then_scrolls_the_inner_content() {
    let mut engine = booted(&coordinator_page(""));
    // The flick of `flick`, inside the inner scroller: 52px of drag after
    // the slop, then a release at 3 px/ms.
    touch_at(&mut engine, 0.0, PointerPhase::Down, 350.0);
    touch_at(&mut engine, 0.01, PointerPhase::Move, 320.0);
    touch_at(&mut engine, 0.02, PointerPhase::Move, 290.0);
    touch_at(&mut engine, 0.02, PointerPhase::Up, 290.0);
    assert_eq!(coordinator_offsets(&mut engine), (52.0, 0.0));
    assert!(engine.is_animating(), "a fling owes frames");

    // 80ms of fling is about 222px: 148 fold the coordinator, the rest
    // scrolls the inner content.
    frame_at(&mut engine, 0.1);
    let (outer, inner) = coordinator_offsets(&mut engine);
    assert!((outer - 200.0).abs() < f32::EPSILON, "folded, got {outer}");
    assert!(
        inner > 50.0,
        "the fling went on into the inner, got {inner}"
    );
    assert!(engine.is_animating(), "and is still going");

    let mut at = 0.1;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < 10.0, "never came to rest");
        frame_at(&mut engine, at);
    }
    // A 3 px/ms fling travels about 1498px: past both ranges.
    assert_eq!(coordinator_offsets(&mut engine), (200.0, 600.0));
}

/// A wheel over the inner scroller folds the coordinator before it scrolls
/// the inner content.
#[test]
fn a_forward_wheel_over_the_inner_scroller_folds_the_coordinator_first() {
    let mut engine = booted(&coordinator_page(""));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 350.0),
        dom::Vector2D::new(0.0, 150.0),
    ));
    assert_eq!(coordinator_offsets(&mut engine), (150.0, 0.0));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 350.0),
        dom::Vector2D::new(0.0, 100.0),
    ));
    assert_eq!(coordinator_offsets(&mut engine), (200.0, 50.0));
}

/// A drag that starts on the header latches the coordinator itself, whose
/// chain the inner scroller is not on: it scrolls as any scroller does, and
/// the inner never moves.
#[test]
fn a_drag_on_the_coordinators_own_content_scrolls_it_as_before() {
    let mut engine = booted(&coordinator_page(""));
    quiet_drag(&mut engine, 0.0, 180.0, 22.0);
    assert_eq!(coordinator_offsets(&mut engine), (150.0, 0.0));
    quiet_drag(&mut engine, 1.0, 30.0, -178.0);
    assert_eq!(
        coordinator_offsets(&mut engine),
        (200.0, 0.0),
        "the coordinator pins at its maximum and the rest goes nowhere"
    );
}

/// `overflow-y: hidden` on the coordinator (its `enable-scroll="false"`):
/// not user-scrollable, so the walk admits it nothing and the inner
/// scroller scrolls from the first pixel.
#[test]
fn a_coordinator_that_is_not_user_scrollable_never_folds() {
    let mut engine = booted(&coordinator_page("overflow-y:hidden"));
    quiet_drag(&mut engine, 0.0, 350.0, 242.0);
    assert_eq!(coordinator_offsets(&mut engine), (0.0, 100.0));
}

/// An `<x-refresh-view>` 200px x 400px: a 50px header, a `scroll-view` of
/// the view's full height holding 1000px of content, a 50px footer. Its
/// shadow `#container` scrolls 740px (two 120px placeholders, the header,
/// the content, the footer) in a 400px port and opens on the content at
/// 170. Answers the engine and `#container`'s handle bits.
fn booted_refresh_view() -> (TestEngine, u64) {
    let mut engine = booted(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const view = __CreateElement('x-refresh-view', 0);
          const header = __CreateElement('x-refresh-header', 0);
          const inner = __CreateScrollView(0);
          const tall = __CreateView(0);
          const footer = __CreateElement('x-refresh-footer', 0);
          __AppendElement(page, view);
          __AppendElement(view, header);
          __AppendElement(view, inner);
          __AppendElement(inner, tall);
          __AppendElement(view, footer);
          globalThis.held = [page, view, header, inner, tall, footer];
          __SetInlineStyles(view, 'width:200px;height:400px');
          __SetInlineStyles(header, 'height:50px;position:absolute');
          __SetInlineStyles(inner, 'width:100%;height:100%');
          __SetInlineStyles(tall, 'flex-shrink:0;width:200px;height:1000px');
          __SetInlineStyles(footer, 'height:50px');
          __FlushElementTree();
        };
        ",
    );
    let container = engine
        .probe_document(|document| {
            let page = document.document_element().id();
            let view = document
                .query_selector_all(page, "x-refresh-view")
                .expect("a valid selector")[0];
            let shadow = document.shadow_root(view)?;
            Some(document.get(shadow)?.child_ids()[0].to_bits())
        })
        .flatten()
        .expect("the refresh view has a shadow `#container`");
    (engine, container)
}

/// The pull on the inner scroller at its top chains into the shadow
/// `#container`, which the drag then holds: a commit landing mid-drag — the
/// one that publishes the header's new snap alignment — does not settle it.
/// Released with the header fully shown (`#container` at 70, the header at
/// 120..170), it glides to the header's start; released with the header 60%
/// shown, back to the content at 170.
#[test]
fn a_pull_on_a_refresh_view_holds_its_container_then_settles_on_release() {
    for (to_y, pulled, shown, rest) in [(308.0, 70.0, true, 120.0), (238.0, 140.0, false, 170.0)] {
        let (mut engine, container) = booted_refresh_view();
        let commit = engine.published_frame().expect("booted").commit_id();
        assert!((live_offset(&mut engine, container).y - 170.0).abs() < f32::EPSILON);

        touch_at(&mut engine, 0.0, PointerPhase::Down, 200.0);
        touch_at(&mut engine, 0.01, PointerPhase::Move, to_y);
        assert!(
            (live_offset(&mut engine, container).y - pulled).abs() < 0.5,
            "the pull reached `#container`: {:?}",
            live_offset(&mut engine, container)
        );
        // Main adopts the pulled offset and re-samples the header; when its
        // alignment changed, it commits the new snap positions.
        if shown {
            next_commit(&mut engine, commit);
        }
        adopt_at(&mut engine, 0.2);
        assert!(
            (live_offset(&mut engine, container).y - pulled).abs() < 0.5,
            "a held container is not settled by a commit"
        );

        touch_at(&mut engine, 0.6, PointerPhase::Up, to_y);
        let mut at = 0.6;
        while engine.is_animating() {
            at += 1.0 / 60.0;
            assert!(at < 5.0, "never came to rest");
            frame_at(&mut engine, at);
        }
        let landed = live_offset(&mut engine, container).y;
        assert!(
            (landed - rest).abs() < f32::EPSILON,
            "pulled to {pulled}, landed on {landed}, not {rest}"
        );
    }
}
