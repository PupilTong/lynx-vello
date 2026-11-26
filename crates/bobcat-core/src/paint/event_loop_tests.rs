use std::sync::Arc;
use std::time::{Duration, Instant};

use terminus::link::Point2D;
use terminus::link::input::{InputEvent, PointerKind, PointerPhase};
use terminus::link::painter::ScrollIntentsTestUtils as _;

use crate::test_support::{
    IntersectionWatcher, TestEngine, TestViewSpec, WatcherLog, take_watched,
};

fn node_id(bits: u64) -> terminus::link::NodeId {
    terminus::link::NodeId::from_bits(bits).expect("a well-formed packed handle")
}

fn booted(source: &str) -> TestEngine {
    TestViewSpec::new(source).boot()
}

fn attribute_of(engine: &mut TestEngine, node: u64, name: &'static str) -> Option<String> {
    engine
        .probe_document(move |tree| {
            tree.get(node_id(node))
                .and_then(|live| live.attribute(name).map(std::borrow::Cow::into_owned))
        })
        .flatten()
}

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

#[test]
fn a_quick_release_delivers_tap_to_the_realm() {
    let mut engine = booted(&gesture_page(false));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 12.0));
    wait_for_log(&mut engine, "tap:12");
}

const CLICK_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const row = __CreateView(0);
          const left = __CreateView(0);
          const right = __CreateView(0);
          __AppendElement(page, row);
          __AppendElement(row, left);
          __AppendElement(row, right);
          globalThis.held = [page, row, left, right];
          globalThis.entries = [];
          __SetInlineStyles(row, 'display:flex;flex-direction:row;width:200px;height:100px');
          __SetInlineStyles(left, 'width:100px;height:100px');
          __SetInlineStyles(right, 'width:100px;height:100px');
          const note = (event) => {
            entries.push(event.type + '@' + event.target.uid);
            __SetAttribute(row, 'log', entries.join());
          };
          for (const name of ['click', 'dblclick', 'tap']) {
            __AddEventListener(page, name, note, {});
          }
          __FlushElementTree();
        };
        ";

fn touch_xy(id: u32, phase: PointerPhase, x: f32, y: f32) -> InputEvent {
    InputEvent::pointer(Point2D::new(x, y), id, PointerKind::Touch, phase)
}

#[test]
fn a_click_reaches_the_common_ancestor_and_two_quick_ones_a_dblclick() {
    let mut engine = booted(CLICK_PAGE);
    engine.painter.clock_mut().pin(0.0);
    engine.dispatch_input(touch_xy(1, PointerPhase::Down, 50.0, 50.0));
    engine.dispatch_input(touch_xy(1, PointerPhase::Up, 150.0, 50.0));
    wait_for_log(&mut engine, "click@3");

    engine.painter.clock_mut().pin(1.0);
    engine.dispatch_input(touch_xy(1, PointerPhase::Down, 50.0, 50.0));
    engine.dispatch_input(touch_xy(1, PointerPhase::Up, 50.0, 50.0));
    engine.painter.clock_mut().pin(1.2);
    engine.dispatch_input(touch_xy(1, PointerPhase::Down, 50.0, 50.0));
    engine.dispatch_input(touch_xy(1, PointerPhase::Up, 50.0, 50.0));
    wait_for_log(
        &mut engine,
        "click@3,click@4,tap@4,click@4,dblclick@4,tap@4",
    );
}

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

#[test]
fn a_delivered_event_carries_its_timestamp_and_params() {
    let mut engine = booted(STAMPED_GESTURE_PAGE);
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.painter.clock_mut().pin(0.25);
    engine.dispatch_input(touch(1, PointerPhase::Up, 12.0));
    wait_for_log(&mut engine, "number:250:object:{}");
}

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

#[test]
fn a_held_pointer_delivers_longpress_and_suppresses_the_tap() {
    let mut engine = booted(&gesture_page(true));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.painter.clock_mut().pin(0.6);
    engine.dispatch_input(touch(1, PointerPhase::Move, 10.0));
    wait_for_log(&mut engine, "longpress:10");

    engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Down, 30.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 30.0));
    wait_for_log(&mut engine, "longpress:10,tap:30");
}

#[test]
fn a_long_hold_without_longpress_listener_still_taps() {
    let mut engine = booted(&gesture_page(false));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.painter.clock_mut().pin(0.6);
    engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
    wait_for_log(&mut engine, "tap:10");
}

#[test]
fn a_release_after_the_deadline_delivers_longpress_before_the_release() {
    let mut engine = booted(&gesture_page(true));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.painter.clock_mut().pin(0.6);
    engine.dispatch_input(touch(1, PointerPhase::Up, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Down, 30.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 30.0));
    wait_for_log(&mut engine, "longpress:10,tap:30");
}

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

#[test]
fn a_touch_sequence_delivers_its_lists_to_the_realm() {
    let mut engine = booted(TOUCH_PAGE);
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    engine.dispatch_input(touch(1, PointerPhase::Move, 20.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 22.0));
    wait_for_log(&mut engine, "touchmove:20:1:1:1:1,touchend:22:0:0:1:1");
}

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

fn scroll_offset_of(engine: &mut TestEngine, node: u64) -> terminus::link::Vector2D<f32> {
    engine
        .probe_document(move |tree| tree.scroll_offset(node_id(node)))
        .expect("the view's task answers probes")
}

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

    let offset = engine
        .painter
        .scroll_intents()
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

const SCROLL_EVENTS_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const scroller = __CreateView(0);
          __AppendElement(page, scroller);
          globalThis.held = [page, scroller];
          globalThis.entries = [];
          __SetInlineStyles(scroller, 'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px');
          for (let i = 0; i < 5; i++) {
            const card = __CreateView(0);
            __AppendElement(scroller, card);
            held.push(card);
            __SetInlineStyles(card, 'flex-shrink:0;width:200px;height:200px');
          }
          const note = (event) => {
            entries.push(event.type);
            __SetAttribute(scroller, 'log', entries.join());
          };
          __AddEventListener(scroller, 'scroll', note, {});
          __AddEventListener(scroller, 'scrollend', note, {});
          __AddEventListener(scroller, 'tap', () => {
            if (globalThis.scrolled) return;
            globalThis.scrolled = true;
            __InvokeUIMethod(scroller, 'scrollTo', { top: 100 }, () => {});
          }, {});
          __FlushElementTree();
        };
        ";

fn drive_until_log(engine: &mut TestEngine, at: f64, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        adopt_at(engine, at);
        engine.painter.publish_scroll(at, None);
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

#[test]
fn instant_drag_and_wheel_scrolls_fire_scroll_then_one_scrollend() {
    let mut engine = booted(SCROLL_EVENTS_PAGE);
    touch_at(&mut engine, 0.0, PointerPhase::Down, 100.0);
    touch_at(&mut engine, 0.05, PointerPhase::Up, 100.0);
    drive_until_log(&mut engine, 0.05, "scroll,scrollend");
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::new(0.0, 100.0)
    );

    touch_at(&mut engine, 1.0, PointerPhase::Down, 150.0);
    touch_at(&mut engine, 1.5, PointerPhase::Move, 120.0);
    touch_at(&mut engine, 2.0, PointerPhase::Move, 120.0);
    touch_at(&mut engine, 2.0, PointerPhase::Up, 120.0);
    drive_until_log(&mut engine, 2.0, "scroll,scrollend,scroll,scrollend");
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::new(0.0, 122.0)
    );

    for (at, log) in [
        (3.0, "scroll,scrollend,scroll,scrollend,scroll"),
        (3.03, "scroll,scrollend,scroll,scrollend,scroll,scroll"),
    ] {
        engine.painter.clock_mut().pin(at);
        engine.dispatch_input(InputEvent::wheel(
            Point2D::new(100.0, 100.0),
            terminus::link::Vector2D::new(0.0, 20.0),
        ));
        assert!(engine.is_animating(), "the quiet interval owes frames");
        drive_until_log(&mut engine, at, log);
    }
    drive_until_log(
        &mut engine,
        3.2,
        "scroll,scrollend,scroll,scrollend,scroll,scroll,scrollend",
    );
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::new(0.0, 162.0)
    );
}

#[test]
fn a_key_reaches_a_page_listener_with_its_keyboard_members() {
    let mut engine = booted(
        r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const view = __CreateView(0);
              __AppendElement(page, view);
              globalThis.held = [page, view];
              __AddEventListener(page, 'keydown', (event) => {
                __SetAttribute(view, 'log',
                  [event.type, event.key, event.code, event.shiftKey].join());
              }, {});
              __FlushElementTree();
            };
            ",
    );
    let shift = terminus::link::input::Modifiers {
        shift: true,
        ..terminus::link::input::Modifiers::default()
    };
    engine.dispatch_input(InputEvent::key(
        terminus::link::input::KeyPhase::Down,
        "A",
        "KeyA",
        false,
        shift,
    ));
    wait_for_log(&mut engine, "keydown,A,KeyA,true");
}

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

fn assert_shown_at(engine: &mut TestEngine, node: u64, expected: f32) {
    let offset = live_offset(engine, node);
    assert!(
        (offset.y - expected).abs() < 0.5,
        "node {node} should sit at {expected}, got {offset:?}"
    );
}

#[test]
fn a_capturing_container_hands_its_drag_to_the_scroller_above() {
    let mut engine = booted(&nested_scrollers_page("scroll-capture:nearest"));
    drag(&mut engine, 50.0, 20.0);
    assert_shown_at(&mut engine, 3, 22.0);
    assert_eq!(
        engine.painter.scroll_intents().offset_for(node_id(4)),
        None,
        "the inner scroller was not moved"
    );
}

#[test]
fn overscroll_contain_keeps_a_wheel_inside_its_container() {
    let mut engine = booted(&nested_scrollers_page("overscroll-behavior:contain"));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 50.0),
        terminus::link::Vector2D::new(0.0, 5000.0),
    ));
    assert_shown_at(&mut engine, 4, 900.0);
    assert_eq!(
        engine.painter.scroll_intents().offset_for(node_id(3)),
        None,
        "the remainder was fenced off"
    );
}

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
    assert_shown_at(&mut engine, 3, 122.0);
    touch_at(&mut engine, 0.4, PointerPhase::Up, 20.0);
    assert_shown_at(&mut engine, 3, 122.0);
    assert!(engine.is_animating(), "the settle owes frames");
    frame_at(&mut engine, 0.45);
    let moving = live_offset(&mut engine, 3).y;
    assert!(moving > 122.0 && moving < 200.0, "on its way, got {moving}");
    frame_at(&mut engine, 1.5);
    assert!(
        (live_offset(&mut engine, 3).y - 200.0).abs() < f32::EPSILON,
        "exactly on the position, got {}",
        live_offset(&mut engine, 3).y
    );
    assert!(!engine.is_animating());
}

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
        terminus::link::Vector2D::new(0.0, 30.0),
    ));
    assert_shown_at(&mut engine, 3, 200.0);
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, -30.0),
    ));
    assert_shown_at(&mut engine, 3, 0.0);
}

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
        terminus::link::Vector2D::zero(),
    ));
    assert_shown_at(&mut engine, 3, 50.0);
}

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
        terminus::link::Vector2D::new(0.0, 400.0),
        "the document rests on the target"
    );
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::zero(),
    ));
    assert_eq!(
        engine.painter.scroll_intents().offset_for(node_id(3)),
        None,
        "nothing for the painter to override"
    );
}

fn touch_at(engine: &mut TestEngine, at: f64, phase: PointerPhase, y: f32) {
    engine.painter.clock_mut().pin(at);
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(100.0, y),
        1,
        PointerKind::Touch,
        phase,
    ));
}

fn frame_at(engine: &mut TestEngine, at: f64) {
    engine.painter.clock_mut().pin(at);
    engine.painter.service_gesture_clock(at);
}

fn flick(engine: &mut TestEngine, at: f64) {
    touch_at(engine, at, PointerPhase::Down, 150.0);
    touch_at(engine, at + 0.01, PointerPhase::Move, 120.0);
    touch_at(engine, at + 0.02, PointerPhase::Move, 90.0);
    touch_at(engine, at + 0.02, PointerPhase::Up, 90.0);
}

#[test]
fn a_flick_keeps_scrolling_after_its_release_and_comes_to_rest() {
    let mut engine = booted(&snapping_page("", "", 200, 0));
    flick(&mut engine, 0.0);
    assert!((live_offset(&mut engine, 3).y - 52.0).abs() < 0.5);
    assert!(engine.is_animating(), "a fling owes frames");

    frame_at(&mut engine, 0.1);
    let early = live_offset(&mut engine, 3).y;
    assert!(
        (early - (52.0 + 222.0)).abs() < 3.0,
        "the first frame moved by the curve, got {early}"
    );
    frame_at(&mut engine, 0.3);
    let later = live_offset(&mut engine, 3).y;
    assert!(later > early, "still moving");
    for at in [1.0, 2.0, 4.0, 8.0] {
        frame_at(&mut engine, at);
    }
    assert!(
        (live_offset(&mut engine, 3).y - 800.0).abs() < 0.5,
        "spent against the end, got {}",
        live_offset(&mut engine, 3).y
    );
    assert!(!engine.is_animating(), "at rest, no frame owed");
}

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
    let unsnapped = live_offset(&mut engine, 3).y;
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
    let mid = live_offset(&mut engine, 3).y;
    assert!(mid > 100.0 && mid < 600.0, "on its way, got {mid}");
    for at in [1.0, 2.0, 4.0, 8.0] {
        frame_at(&mut engine, at);
    }
    assert!(
        (live_offset(&mut engine, 3).y - 600.0).abs() < 0.5,
        "aimed at the position, got {}",
        live_offset(&mut engine, 3).y
    );
    assert!(!engine.is_animating());
}

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
    let stretched = live_offset(&mut engine, 3).y;
    assert!((stretched - -40.4).abs() < 0.2, "got {stretched}");
    assert!(!engine.is_animating(), "under the finger nothing animates");
    touch_at(&mut engine, 0.4, PointerPhase::Up, 150.0);
    assert!(engine.is_animating(), "the bounce back owes frames");
    frame_at(&mut engine, 0.41);
    let first = live_offset(&mut engine, 3).y;
    assert!(
        first < 0.0 && first > stretched,
        "barely moved yet, got {first}"
    );
    frame_at(&mut engine, 0.6);
    let later = live_offset(&mut engine, 3).y;
    assert!(later > first && later < 0.0, "on its way home, got {later}");
    frame_at(&mut engine, 1.5);
    assert!(
        live_offset(&mut engine, 3).y.abs() < f32::EPSILON,
        "home on the edge, got {}",
        live_offset(&mut engine, 3).y
    );
    assert!(!engine.is_animating());
}

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
        terminus::link::Vector2D::new(0.0, 700.0),
    ));
    assert_shown_at(&mut engine, 3, 700.0);
    flick(&mut engine, 0.0);
    let mut furthest = live_offset(&mut engine, 3).y;
    for at in [0.05, 0.1, 0.15, 0.2, 0.25, 0.3] {
        frame_at(&mut engine, at);
        furthest = furthest.max(live_offset(&mut engine, 3).y);
    }
    assert!(
        furthest > 800.0 && furthest <= 1000.0,
        "past the end and inside a scrollport of it, got {furthest}"
    );
    for at in [1.0, 2.0, 3.0] {
        frame_at(&mut engine, at);
    }
    assert!(
        (live_offset(&mut engine, 3).y - 800.0).abs() < f32::EPSILON,
        "back on the edge, got {}",
        live_offset(&mut engine, 3).y
    );
    assert!(!engine.is_animating());
}

#[test]
fn a_fling_into_a_plain_edge_stops_there() {
    let mut engine = booted(&snapping_page("", "", 200, 0));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, 700.0),
    ));
    flick(&mut engine, 0.0);
    for at in [0.05, 0.1, 0.2] {
        frame_at(&mut engine, at);
        assert!(live_offset(&mut engine, 3).y <= 800.0);
    }
    assert!((live_offset(&mut engine, 3).y - 800.0).abs() < 0.5);
    assert!(!engine.is_animating(), "spent against the wall");
}

const CIRCULAR_PERIOD: f32 = 1000.0;

fn wrapped_y(engine: &mut TestEngine, node: u64) -> f32 {
    live_offset(engine, node).y.rem_euclid(CIRCULAR_PERIOD)
}

fn frames_until_still(engine: &mut TestEngine, mut at: f64) -> f64 {
    let deadline = at + 10.0;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < deadline, "never came to rest");
        frame_at(engine, at);
    }
    at
}

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
        terminus::link::Vector2D::new(0.0, 800.0),
    ));
    assert_shown_at(&mut engine, 3, 800.0);
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
        terminus::link::Vector2D::zero(),
        "the document adopts the first card"
    );
}

#[test]
fn a_drag_back_from_the_first_page_shows_the_last_and_posts_it_across_the_seam() {
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
    let held = scroll_offset_of(&mut engine, 3);
    assert!(
        held.x == 0.0 && (held.y - 858.0).abs() < 0.5,
        "through the seam the document holds the offset the screen shows, got {held:?}"
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
        terminus::link::Vector2D::new(0.0, 800.0),
        "the document adopts the last card"
    );
}

#[test]
fn a_circular_axis_never_chains_out() {
    let mut engine = booted(&nested_scrollers_page("overscroll-behavior:circular"));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 50.0),
        terminus::link::Vector2D::new(0.0, 1500.0),
    ));
    assert!(
        (wrapped_y(&mut engine, 4) - 500.0).abs() < 0.5,
        "the inner wrapped, got {}",
        live_offset(&mut engine, 4).y
    );
    assert_eq!(
        engine.painter.scroll_intents().offset_for(node_id(3)),
        None,
        "nothing chained out to the outer"
    );
    assert_eq!(
        scroll_offset_of(&mut engine, 4),
        terminus::link::Vector2D::new(0.0, 500.0),
        "the document adopts the wrapped offset"
    );
}

#[test]
fn a_fling_on_a_plain_circular_axis_runs_past_the_end() {
    let mut engine = booted(&snapping_page("overscroll-behavior:circular", "", 200, 0));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, 700.0),
    ));
    assert_shown_at(&mut engine, 3, 700.0);
    flick(&mut engine, 0.0);
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

#[test]
fn a_drag_on_a_flinging_scroller_takes_it_over() {
    let mut engine = booted(SCROLLING_GESTURE_PAGE);
    flick(&mut engine, 0.0);
    frame_at(&mut engine, 0.1);
    let moving = live_offset(&mut engine, 3).y;
    assert!(moving > 52.0 && engine.is_animating());

    touch_at(&mut engine, 0.15, PointerPhase::Down, 100.0);
    frame_at(&mut engine, 0.2);
    assert!(engine.is_animating(), "a down alone decides nothing");
    let found = live_offset(&mut engine, 3).y;
    assert!(found > moving);
    touch_at(&mut engine, 0.25, PointerPhase::Move, 90.0);
    assert!(!engine.is_animating(), "the drag stopped the fling");
    let held = live_offset(&mut engine, 3).y;
    assert!(
        (held - (found + 2.0)).abs() < 0.5,
        "got {held} after {found}"
    );
    frame_at(&mut engine, 0.5);
    assert!((live_offset(&mut engine, 3).y - held).abs() < f32::EPSILON);
    touch_at(&mut engine, 0.6, PointerPhase::Up, 90.0);
    frame_at(&mut engine, 1.0);
    assert!((live_offset(&mut engine, 3).y - held).abs() < f32::EPSILON);
    assert!(!engine.is_animating());

    engine.painter.clock_mut().pin(2.0);
    engine.dispatch_input(touch(1, PointerPhase::Down, 150.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 150.0));
    wait_for_log(&mut engine, "tap:150");
}

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

fn touch_x_at(engine: &mut TestEngine, at: f64, phase: PointerPhase, x: f32) {
    engine.painter.clock_mut().pin(at);
    engine.dispatch_input(InputEvent::pointer(
        Point2D::new(x, 100.0),
        1,
        PointerKind::Touch,
        phase,
    ));
}

fn live_offset(engine: &mut TestEngine, node: u64) -> terminus::link::Vector2D<f32> {
    let node = node_id(node);
    engine
        .painter
        .scroll_intents()
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

fn adopt_at(engine: &mut TestEngine, at: f64) {
    frame_at(engine, at);
    let frame = engine
        .painter
        .published_frame()
        .expect("a frame is adopted");
    engine.painter.scroll_intents_mut().rebase(&frame, at);
}

fn flick_x(engine: &mut TestEngine, at: f64) {
    touch_x_at(engine, at, PointerPhase::Down, 150.0);
    touch_x_at(engine, at + 0.01, PointerPhase::Move, 120.0);
    touch_x_at(engine, at + 0.02, PointerPhase::Move, 90.0);
    touch_x_at(engine, at + 0.02, PointerPhase::Up, 90.0);
}

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
        terminus::link::Vector2D::new(200.0, 0.0),
        "the document adopts where it landed"
    );
}

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
            .scroll_intents()
            .is_flinging(&frame, node_id(3)),
        "a fling carries it"
    );
    assert!(!engine.painter.scroll_intents().is_gliding(node_id(3)));
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

#[test]
fn a_smooth_request_glides_to_its_target() {
    let mut engine = booted(&pager_page(""));
    let target = engine
        .probe_document(|document| {
            document.scroll_to_with(
                node_id(3),
                terminus::link::Vector2D::new(400.0, 0.0),
                terminus::link::scroll::ScrollBehavior::Smooth,
            )
        })
        .expect("the view's task answers probes");
    assert_eq!(target, terminus::link::Vector2D::new(400.0, 0.0));
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::zero()
    );

    adopt_at(&mut engine, 1.0);
    assert!(engine.is_animating(), "the request started a glide");
    assert!(engine.painter.scroll_intents().is_gliding(node_id(3)));
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
        terminus::link::Vector2D::new(400.0, 0.0)
    );
    assert_eq!(
        engine
            .probe_document(|document| document.pending_scroll_request(node_id(3)))
            .expect("the view's task answers probes"),
        None,
        "acknowledged"
    );
}

#[test]
fn an_auto_scroll_runs_on_the_painter_and_its_posts_reach_the_document() {
    let mut engine = booted(SCROLLING_GESTURE_PAGE);
    assert!(!engine.is_animating());
    engine
        .probe_document(|document| document.auto_scroll(node_id(3), 120.0, false, true))
        .expect("the view's task answers probes");
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::zero()
    );

    adopt_at(&mut engine, 1.0);
    assert!(engine.is_animating(), "the auto-scroll owes frames");
    assert!(engine.painter.scroll_intents().has_auto_scroll(node_id(3)));
    let mut at = 1.0;
    for _ in 0..31 {
        at += 1.0 / 60.0;
        frame_at(&mut engine, at);
    }
    let live = live_offset(&mut engine, 3);
    assert!((live.y - 60.0).abs() < 0.01, "got {live:?}");
    assert!(engine.is_animating(), "still on its way");
    engine.painter.publish_scroll(at, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        live,
        "the document follows"
    );
    assert_eq!(
        engine
            .probe_document(|document| document.pending_auto_scroll(node_id(3)))
            .expect("the view's task answers probes"),
        None,
        "the painter's post acknowledged the change"
    );

    engine
        .probe_document(|document| document.auto_scroll(node_id(3), 0.0, false, true))
        .expect("the view's task answers probes");
    assert_eq!(scroll_offset_of(&mut engine, 3), live);
    adopt_at(&mut engine, at);
    assert!(!engine.is_animating(), "stopped, nothing is owed");
    frame_at(&mut engine, at + 1.0);
    assert_eq!(live_offset(&mut engine, 3), live, "where it stood");
}

#[test]
fn an_autoplaying_swiper_turns_by_itself() {
    let mut engine = booted(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const swiper = __CreateElement('x-swiper', 0);
          __SetInlineStyles(swiper, 'width:200px;height:100px');
          __SetAttribute(swiper, 'interval', '30');
          __SetAttribute(swiper, 'autoplay', true);
          __AppendElement(page, swiper);
          for (let i = 0; i < 3; i++) {
            __AppendElement(swiper, __CreateElement('x-swiper-item', 0));
          }
          globalThis.held = [page, swiper];
          __FlushElementTree();
        };
        ",
    );
    let content = engine
        .probe_document(|document| {
            let page = document.document_element().id();
            let swiper = document.get(page).expect("the page").child_ids()[0];
            crate::main::tree::swiper::content(document, swiper)
        })
        .expect("the view's task answers probes")
        .expect("the swiper's `#content`");
    assert_eq!(
        engine
            .probe_document(move |document| document.scroll_offset(content))
            .expect("the view's task answers probes"),
        terminus::link::Vector2D::zero(),
        "on its first item"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut at = 1.0;
    loop {
        at += 1.0 / 60.0;
        adopt_at(&mut engine, at);
        engine.painter.publish_scroll(at, None);
        let offset = engine
            .probe_document(move |document| document.scroll_offset(content))
            .expect("the view's task answers probes");
        if offset.x >= 200.0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the swiper never turned, at {offset:?}"
        );
        std::thread::yield_now();
    }
}

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
                terminus::link::Vector2D::new(0.0, 100.0),
                terminus::link::scroll::ScrollBehavior::Instant,
            )
        })
        .expect("the view's task answers probes");
    engine.painter.publish_scroll(0.1, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::new(0.0, 100.0),
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
        terminus::link::Vector2D::new(0.0, 100.0)
    );
}

#[test]
fn a_wheel_tick_ends_a_glide_where_the_wheel_puts_it() {
    let mut engine = booted(&viewpager_page(""));
    engine
        .probe_document(|document| {
            document.scroll_to_with(
                node_id(3),
                terminus::link::Vector2D::new(800.0, 0.0),
                terminus::link::scroll::ScrollBehavior::Smooth,
            )
        })
        .expect("the view's task answers probes");
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::zero()
    );
    adopt_at(&mut engine, 1.0);
    frame_at(&mut engine, 1.05);
    let mid = live_offset(&mut engine, 3).x;
    assert!(mid > 0.0 && mid < 600.0, "mid-glide, got {mid}");
    assert!(engine.painter.scroll_intents().is_gliding(node_id(3)));

    engine.painter.clock_mut().pin(1.06);
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(30.0, 0.0),
    ));
    let landed = live_offset(&mut engine, 3).x;
    assert!(
        landed > mid && landed % 200.0 == 0.0,
        "the wheel stepped to the next page ahead of {mid}, got {landed}"
    );
    assert!(!engine.painter.scroll_intents().is_gliding(node_id(3)));
    assert!(
        engine.painter.scroll_intents().is_wheeling(node_id(3)),
        "the glide ended, and the wheel gesture's quiet interval runs"
    );
    for at in [1.1, 1.3, 2.0] {
        frame_at(&mut engine, at);
        assert!(
            (live_offset(&mut engine, 3).x - landed).abs() < f32::EPSILON,
            "the wheel's position stands at {at}"
        );
    }
    assert!(!engine.is_animating(), "the wheel gesture has rested");
    engine.painter.publish_scroll(2.0, None);
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::new(landed, 0.0)
    );
    assert_eq!(
        engine
            .probe_document(|document| document.pending_scroll_request(node_id(3)))
            .expect("the view's task answers probes"),
        None,
        "the post named the request"
    );
}

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

fn frames_until_rest(engine: &mut TestEngine, mut at: f64) -> f32 {
    let deadline = at + 3.0;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < deadline, "never came to rest");
        frame_at(engine, at);
    }
    live_offset(engine, 3).x
}

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
        terminus::link::Vector2D::new(600.0, 0.0),
        "main adopted the swipe"
    );

    engine
        .probe_document(|document| document.set_attribute(node_id(3), "select-index", "0"))
        .expect("the view's task answers probes");
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::zero()
    );
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

    engine.painter.clock_mut().pin(1.0);
    engine.dispatch_input(touch(1, PointerPhase::Down, 100.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 100.0));
    wait_for_log(&mut engine, "selectTab:0");
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::new(400.0, 0.0),
        "an instant turn moves the document at once"
    );
    adopt_at(&mut engine, 1.1);
    assert!((live_offset(&mut engine, 3).x - 400.0).abs() < f32::EPSILON);
    assert!(!engine.is_animating());
}

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
                            terminus::link::Vector2D::new(800.0, 0.0),
                            terminus::link::scroll::ScrollBehavior::Instant,
                        )
                    })
                    .expect("the view's task answers probes");
                assert_eq!(
                    scroll_offset_of(&mut engine, 3),
                    terminus::link::Vector2D::new(800.0, 0.0)
                );
                adopt_at(&mut engine, start - 0.1);
                assert!((live_offset(&mut engine, 3).x - edge).abs() < f32::EPSILON);
            }
            touch_x_at(&mut engine, start, PointerPhase::Down, from);
            touch_x_at(&mut engine, start + 0.05, PointerPhase::Move, to);
            let stretch = live_offset(&mut engine, 3).x - edge;
            if bounces {
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

fn swiper_page(setup: &str) -> String {
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const swiper = __CreateElement('x-swiper', 0);
          __AppendElement(page, swiper);
          globalThis.held = [page, swiper];
          __SetInlineStyles(swiper, 'width:200px;height:200px');
          for (let i = 0; i < 3; i++) {{
            const item = __CreateElement('x-swiper-item', 0);
            __AppendElement(swiper, item);
            held.push(item);
          }}
          {setup}
          __FlushElementTree();
        }};
        "
    )
}

fn swiper_scroller(engine: &mut TestEngine) -> u64 {
    let frame = engine
        .painter
        .published_frame()
        .expect("a frame is adopted");
    frame
        .scroll_slots()
        .iter()
        .find(|slot| slot.snap.x.is_some())
        .expect("the swiper's scroller snaps on x")
        .node
        .to_bits()
}

#[test]
fn a_swiper_resized_at_rest_stays_on_its_item() {
    for (circular, to, rested, adopted, resized) in [
        (false, 30.0, 200.0, 200.0, 300.0),
        (true, 270.0, -200.0, 400.0, 600.0),
    ] {
        let mut engine = booted(&swiper_page(if circular {
            "__SetAttribute(swiper, 'circular', '');"
        } else {
            ""
        }));
        let content = swiper_scroller(&mut engine);
        touch_x_at(&mut engine, 0.0, PointerPhase::Down, 150.0);
        touch_x_at(&mut engine, 0.05, PointerPhase::Move, to);
        touch_x_at(&mut engine, 0.4, PointerPhase::Up, to);
        let mut at = 0.4;
        while engine.painter.scroll_intents().is_animating() {
            at += 1.0 / 60.0;
            assert!(at < 3.0, "never came to rest (circular={circular})");
            frame_at(&mut engine, at);
        }
        let live = live_offset(&mut engine, content).x;
        assert!(
            (live - rested).abs() < f32::EPSILON,
            "the swipe rests on the item, got {live} (circular={circular})"
        );
        engine.painter.publish_scroll(at, None);
        assert_eq!(
            scroll_offset_of(&mut engine, content),
            terminus::link::Vector2D::new(adopted, 0.0),
            "main adopted the swipe (circular={circular})"
        );

        engine
            .probe_document(|document| {
                document.set_inline_style(node_id(3), "width:300px;height:200px");
            })
            .expect("the view's task answers probes");
        assert_eq!(
            scroll_offset_of(&mut engine, content),
            terminus::link::Vector2D::new(resized, 0.0),
            "the document follows the item (circular={circular})"
        );
        adopt_at(&mut engine, at + 0.1);
        let live = live_offset(&mut engine, content).x;
        assert!(
            (live - resized).abs() < f32::EPSILON,
            "the painter shows the item, got {live} (circular={circular})"
        );
        assert!(
            !engine.painter.scroll_intents().is_animating(),
            "a jump, not a glide (circular={circular})"
        );
        engine.painter.publish_scroll(at + 0.1, None);
        assert_eq!(
            scroll_offset_of(&mut engine, content),
            terminus::link::Vector2D::new(resized, 0.0),
            "and main keeps it (circular={circular})"
        );
    }
}

#[test]
fn a_wheel_scrolls_and_reaches_a_wheel_listener() {
    let page = SCROLLING_GESTURE_PAGE.replace(
        "__AddEventListener(view, 'tap', note, {});",
        "__AddEventListener(view, 'wheel', (event) => {
               // `WheelEvent`'s delta, on the event itself; the Lynx
               // `detail` of a wheel is native's `{}`.
               entries.push(event.type + ':' + event.deltaY + ':' +
                 JSON.stringify(event.detail));
               __SetAttribute(view, 'log', entries.join());
             }, {});",
    );
    let mut engine = booted(&page);
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, 30.0),
    ));
    wait_for_log(&mut engine, "wheel:30:{}");
    let offset = engine
        .painter
        .scroll_intents()
        .offset_for(node_id(3))
        .expect("the wheel scrolled the view");
    assert!(
        (offset.y - 30.0).abs() < 0.5,
        "the wheel scrolled the view, got {offset:?}"
    );
}

#[test]
fn a_stationary_hold_longpresses_on_the_frame_clock() {
    let mut engine = booted(&gesture_page(true));
    engine.dispatch_input(touch(1, PointerPhase::Down, 10.0));
    assert!(
        engine.painter.gesture().needs_frame(),
        "the down arms a deadline, which is what keeps frames coming"
    );

    engine.painter.clock_mut().pin(0.6);
    let now = engine.painter.now_seconds();
    engine.painter.service_gesture_clock(now);
    wait_for_log(&mut engine, "longpress:10");
    assert!(
        !engine.painter.gesture().needs_frame(),
        "a resolved deadline stops asking for frames"
    );
}

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

#[test]
fn a_windowed_scroll_recommits_nothing_and_hits_route_at_the_intent_offsets() {
    let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
    let frame = engine.published_frame().expect("boot published a frame");
    let boot_commit = frame.commit_id();
    drop(frame);

    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, 30.0),
    ));
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::new(0.0, 30.0),
        "the document adopts the posted offset"
    );
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
        engine.painter.scroll_intents().offset_for(scroller),
        Some(terminus::link::Vector2D::new(0.0, 30.0)),
        "the intent carries the offset composition draws at"
    );

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

#[test]
fn main_recenters_when_the_adopted_offset_is_recenter_due() {
    let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
    let boot_commit = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    let boot = engine.published_frame().expect("boot published a frame");
    let boot_slot = boot.scroll_slots()[boot.slot_of(node_id(3)).expect("a slot") as usize];
    assert!(boot_slot.recenter_due(terminus::link::Vector2D::new(0.0, 150.0)));
    assert!(boot.covers_scroll_offset(node_id(3), terminus::link::Vector2D::new(0.0, 150.0)));
    drop(boot);
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, 150.0),
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
        !slot.recenter_due(terminus::link::Vector2D::new(0.0, 150.0)),
        "and its window is centered on it"
    );
}

#[test]
fn a_bounding_client_rect_follows_an_adopted_scroll() {
    let mut engine = booted(TWO_ROW_SCROLLER_PAGE);
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, 30.0),
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

fn wait_for_watched(engine: &mut TestEngine, log: &WatcherLog, count: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    loop {
        seen.extend(take_watched(log));
        if seen.len() >= count {
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
                Box::new(terminus::link::ElementHandler(element)),
                terminus::link::IntersectionObserverInit {
                    root: Some(scroller),
                    thresholds: vec![0.0, 0.1, 1.0],
                    ..terminus::link::IntersectionObserverInit::default()
                },
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
        terminus::link::Vector2D::new(0.0, 30.0),
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
        terminus::link::Vector2D::new(0.0, 30.0),
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

const EXPOSURE_ROWS_PAGE: &str = r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const scroller = __CreateView(0);
          __AppendElement(page, scroller);
          __SetInlineStyles(scroller,
            'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px');
          const rows = [];
          for (let index = 0; index < 8; index++) {
            const row = __CreateView(0);
            __SetInlineStyles(row, 'flex-shrink:0;width:200px;height:60px');
            __AppendElement(scroller, row);
            __SetAttribute(row, 'exposure-id', 'row' + index);
            rows.push(row);
          }
          globalThis.held = [page, scroller, rows];
          __FlushElementTree();
        };
        ";

const EXPOSURE_LISTENER: &str = r"
        import { lynx, console } from 'bobcat:bts-runtime';
        const emitter = lynx.getJSModule('GlobalEventEmitter');
        for (const name of ['exposure', 'disexposure']) {
          emitter.addListener(name, (...args) => {
            const [list] = args;
            const shaped = args.length === 1 && list.every((record) =>
              record.type === name &&
              record.target.uid === record['unique-id'] &&
              record.currentTarget.uid === record['unique-id'] &&
              record.exposureID === record['exposure-id'] &&
              record.detail['unique-id'] === 0 &&
              typeof record.timestamp === 'number');
            const ids = list.map((record) => record['exposure-id']).join(',');
            console.log(`${name}:${ids}${shaped ? '' : ':malformed'}`);
          });
        }
        ";

fn wait_for_console(engine: &mut TestEngine, heard: &mut Vec<String>, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while heard.len() < count {
        for event in engine.pump() {
            match event {
                crate::EngineEvent::ConsoleMessage { message, .. } => heard.push(message),
                event @ (crate::EngineEvent::StartupFailed(_)
                | crate::EngineEvent::Panicked(_)
                | crate::EngineEvent::ScriptRunError(_)
                | crate::EngineEvent::ListenerFailed(_)
                | crate::EngineEvent::WorkerThrew { .. }
                | crate::EngineEvent::WorkerEnded { .. }) => {
                    panic!("a realm failed: {event:?}");
                }
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "expected {count} console messages, heard {heard:?}"
        );
        std::thread::yield_now();
    }
}

#[test]
fn a_windowed_scroll_delivers_exposure_records_to_the_bts() {
    let mut engine = TestViewSpec::new(EXPOSURE_ROWS_PAGE)
        .with_background_entry(EXPOSURE_LISTENER)
        .create(Arc::new(crate::view::NoWakeup));
    let mut heard = Vec::new();
    wait_for_console(&mut engine, &mut heard, 1);
    assert_eq!(
        heard,
        ["exposure:row0,row1,row2,row3"],
        "boot's update exposed every row the scrollport shows, the one it shows in part included",
    );
    engine
        .probe_document(|_| ())
        .expect("the view's task answers probes");
    let committed = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, 70.0),
    ));
    wait_for_console(&mut engine, &mut heard, 3);
    assert_eq!(
        heard[1..],
        ["exposure:row4", "disexposure:row0"],
        "the row that entered, then the row that left, each list once",
    );
    assert_eq!(
        scroll_offset_of(&mut engine, 3),
        terminus::link::Vector2D::new(0.0, 70.0),
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

fn blue_of(engine: &mut TestEngine, node: u64) -> f32 {
    engine
        .probe_document(move |tree| {
            tree.get(node_id(node))
                .and_then(terminus::link::Node::computed_style)
                .map(|style| style.get_color().solid_color().components.2)
        })
        .flatten()
        .expect("the row is styled")
}

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
        terminus::link::Vector2D::new(0.0, 100.0),
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
    frame_at(&mut engine, at + 1.0);
    assert!(!engine.is_animating(), "the painter is idle at rest");
}

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
    touch_at(&mut engine, 0.0, PointerPhase::Down, 250.0);
    touch_at(&mut engine, 0.05, PointerPhase::Move, 150.0);
    touch_at(&mut engine, 0.5, PointerPhase::Up, 150.0);
    assert!((live_offset(&mut engine, 4).y - 92.0).abs() < 0.5);
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

fn row_heights(engine: &mut TestEngine) -> Vec<f32> {
    engine
        .probe_document(|document| {
            let page = document.document_element().id();
            document
                .query_selector_all(page, ".row")
                .expect("a valid selector")
                .into_iter()
                .map(|row| {
                    document
                        .rounded_layout(row)
                        .expect("a live row")
                        .size
                        .height
                })
                .collect()
        })
        .expect("the view's task answers probes")
}

#[test]
fn a_reveal_owed_at_the_pass_cap_continues_on_the_next_ticks() {
    let mut engine = TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const list = __CreateView(0);
          __SetClasses(list, 'list');
          __AppendElement(page, list);
          globalThis.held = [page, list];
          for (let i = 0; i < 40; i++) {
            const row = __CreateView(0);
            const cell = __CreateView(0);
            __SetClasses(row, 'row');
            __SetClasses(cell, 'cell');
            __AppendElement(row, cell);
            __AppendElement(list, row);
            held.push(row, cell);
          }
          __FlushElementTree();
        };
        ",
    )
    .with_style_sheet(
        ".list { display: flex; flex-direction: column; overflow: scroll;
                 width: 200px; height: 100px; }
         .row { display: flex; flex-direction: column; flex-shrink: 0;
                content-visibility: auto; contain-intrinsic-size: none auto 100px; }
         .cell { height: 20px; }",
    )
    .boot();
    let boot = engine
        .painter
        .published_frame()
        .expect("the boot flush published");
    assert!(
        boot.needs_main_ticks(),
        "the boot commit stopped at the pass cap with a reveal owed"
    );

    let mut now = 0.0;
    while engine
        .painter
        .published_frame()
        .expect("still published")
        .needs_main_ticks()
    {
        now += 0.016;
        assert!(now < 0.5, "the reveals settle within a few ticks");
        let seq = engine
            .painter
            .begin_frame(now, false)
            .expect("a frame owing a reveal asks for the tick");
        assert!(
            engine
                .painter
                .wait_begin_frame(seq, Duration::from_secs(30)),
            "the view's task services the tick"
        );
    }
    assert!(
        engine.painter.begin_frame(now + 0.016, false).is_none(),
        "a settled frame asks for no more ticks"
    );
    let heights = row_heights(&mut engine);
    assert!(
        heights[..=10]
            .iter()
            .all(|&height| (height - 20.0).abs() < 0.5),
        "every row inside the window is revealed: {heights:?}"
    );
    assert!(
        heights[11..]
            .iter()
            .all(|&height| (height - 100.0).abs() < 0.5),
        "and no row past it: {heights:?}"
    );
}

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
    engine.painter.clock_mut().pin(0.9);
    engine.dispatch_input(touch(1, PointerPhase::Down, 260.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 260.0));
    wait_for_log(&mut engine, "tap:260");
}

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

fn windowed_frame(engine: &mut TestEngine, now: f64) {
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

    let tapped_at = 10.0;
    engine.painter.clock_mut().pin(tapped_at);
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
    let booted_commit = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    assert_eq!(
        attribute_of(&mut engine, 3, "ticked").as_deref(),
        Some("now"),
        "a zero-delay callback must not wait on the host"
    );
    assert_eq!(
        engine
            .published_frame()
            .expect("a frame stays published")
            .commit_id(),
        booted_commit,
        "the epilogue that ran the callback is the one that committed it"
    );
}

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

fn painter_hit(engine: &mut TestEngine, x: f32, y: f32) -> Option<terminus::link::NodeId> {
    let frame = engine.published_frame()?;
    let intents = engine.painter.scroll_intents();
    frame
        .hit(
            Point2D::new(x, y),
            &|slot| intents.offset_for(slot.node),
            None,
        )
        .map(|target| target.node)
}

fn main_hit(engine: &mut TestEngine, x: f32, y: f32) -> Option<terminus::link::NodeId> {
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
        terminus::link::Vector2D::new(dx, dy),
    ));
}

#[test]
fn an_anchored_box_follows_its_anchor_through_a_scroll_on_both_axes() {
    let mut engine = booted_anchored(&anchor_sheet("position-area: bottom center;"));
    let anchored = node_id(7);
    assert_eq!(painter_hit(&mut engine, 70.0, 85.0), Some(anchored));
    let boot = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    wheel(&mut engine, 20.0, 30.0);
    assert_eq!(
        scroll_offset_of(&mut engine, 4),
        terminus::link::Vector2D::new(20.0, 30.0),
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

#[test]
fn an_anchored_box_follows_only_on_the_axes_it_compensates() {
    let mut engine = booted_anchored(&anchor_sheet("top: anchor(bottom); left: 150px;"));
    let anchored = node_id(7);
    assert_eq!(painter_hit(&mut engine, 155.0, 85.0), Some(anchored));
    wheel(&mut engine, 20.0, 30.0);
    assert_eq!(
        scroll_offset_of(&mut engine, 4),
        terminus::link::Vector2D::new(20.0, 30.0)
    );
    assert_eq!(painter_hit(&mut engine, 155.0, 55.0), Some(anchored));
    assert_ne!(
        painter_hit(&mut engine, 135.0, 55.0),
        Some(anchored),
        "the horizontal scroll does not carry it"
    );
}

#[test]
fn an_anchored_box_hides_while_its_anchor_is_scrolled_away() {
    let mut engine = booted_anchored(&anchor_sheet("position-area: bottom center;"));
    let anchored = node_id(7);
    wheel(&mut engine, 0.0, 70.0);
    assert_eq!(painter_hit(&mut engine, 70.0, 15.0), Some(anchored));
    wheel(&mut engine, 0.0, 15.0);
    assert!((scroll_offset_of(&mut engine, 4).y - 85.0).abs() < 0.5);
    assert_ne!(painter_hit(&mut engine, 70.0, 2.0), Some(anchored));
    assert_ne!(main_hit(&mut engine, 70.0, 2.0), Some(anchored));
    wheel(&mut engine, 0.0, -85.0);
    assert_eq!(painter_hit(&mut engine, 70.0, 85.0), Some(anchored));
}

#[test]
fn a_no_overflow_box_hides_once_the_shift_pushes_it_out() {
    let mut engine = booted_anchored(&anchor_sheet(
        "bottom: anchor(top); left: anchor(left); height: 40px;
         position-visibility: no-overflow;",
    ));
    let anchored = node_id(7);
    assert_eq!(painter_hit(&mut engine, 55.0, 20.0), Some(anchored));
    wheel(&mut engine, 0.0, 5.0);
    assert_eq!(painter_hit(&mut engine, 55.0, 10.0), Some(anchored));
    wheel(&mut engine, 0.0, 6.0);
    assert!((scroll_offset_of(&mut engine, 4).y - 11.0).abs() < 0.5);
    assert_ne!(painter_hit(&mut engine, 55.0, 10.0), Some(anchored));
}

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
    assert_eq!(painter_hit(&mut engine, 20.0, 200.0), Some(anchored));
    let boot = engine
        .published_frame()
        .expect("boot published a frame")
        .commit_id();

    wheel(&mut engine, 0.0, 100.0);
    assert!((scroll_offset_of(&mut engine, 4).y - 100.0).abs() < 0.5);
    assert_eq!(painter_hit(&mut engine, 20.0, 100.0), Some(anchored));
    assert_eq!(
        engine.published_frame().expect("published").commit_id(),
        boot
    );

    wheel(&mut engine, 0.0, 100.0);
    let switched = next_commit(&mut engine, boot);
    assert_eq!(painter_hit(&mut engine, 20.0, 150.0), Some(anchored));
    assert_eq!(main_hit(&mut engine, 20.0, 150.0), Some(anchored));

    wheel(&mut engine, 0.0, -200.0);
    next_commit(&mut engine, switched);
    assert_eq!(painter_hit(&mut engine, 20.0, 200.0), Some(anchored));
    assert_ne!(painter_hit(&mut engine, 20.0, 300.0), Some(anchored));
}

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

fn quiet_drag(engine: &mut TestEngine, at: f64, from_y: f32, to_y: f32) {
    touch_at(engine, at, PointerPhase::Down, from_y);
    touch_at(engine, at + 0.01, PointerPhase::Move, to_y);
    touch_at(engine, at + 0.5, PointerPhase::Up, to_y);
}

fn coordinator_offsets(engine: &mut TestEngine) -> (f32, f32) {
    (live_offset(engine, 3).y, live_offset(engine, 5).y)
}

#[test]
fn a_forward_drag_in_the_inner_scroller_folds_the_coordinator_first() {
    let mut engine = booted(&coordinator_page(""));
    quiet_drag(&mut engine, 0.0, 350.0, 242.0);
    assert_eq!(coordinator_offsets(&mut engine), (100.0, 0.0));
}

#[test]
fn forward_folds_before_the_inner_scroller_and_backward_unfolds_after_it() {
    let mut engine = booted(&coordinator_page(""));
    quiet_drag(&mut engine, 0.0, 390.0, 82.0);
    assert_eq!(coordinator_offsets(&mut engine), (200.0, 100.0));

    quiet_drag(&mut engine, 1.0, 100.0, 158.0);
    assert_eq!(coordinator_offsets(&mut engine), (200.0, 50.0));

    quiet_drag(&mut engine, 2.0, 50.0, 358.0);
    assert_eq!(coordinator_offsets(&mut engine), (0.0, 0.0));
}

#[test]
fn a_forward_fling_folds_the_coordinator_then_scrolls_the_inner_content() {
    let mut engine = booted(&coordinator_page(""));
    touch_at(&mut engine, 0.0, PointerPhase::Down, 350.0);
    touch_at(&mut engine, 0.01, PointerPhase::Move, 320.0);
    touch_at(&mut engine, 0.02, PointerPhase::Move, 290.0);
    touch_at(&mut engine, 0.02, PointerPhase::Up, 290.0);
    assert_eq!(coordinator_offsets(&mut engine), (52.0, 0.0));
    assert!(engine.is_animating(), "a fling owes frames");

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
    assert_eq!(coordinator_offsets(&mut engine), (200.0, 600.0));
}

#[test]
fn a_forward_wheel_over_the_inner_scroller_folds_the_coordinator_first() {
    let mut engine = booted(&coordinator_page(""));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 350.0),
        terminus::link::Vector2D::new(0.0, 150.0),
    ));
    assert_eq!(coordinator_offsets(&mut engine), (150.0, 0.0));
    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(100.0, 350.0),
        terminus::link::Vector2D::new(0.0, 100.0),
    ));
    assert_eq!(coordinator_offsets(&mut engine), (200.0, 50.0));
}

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

#[test]
fn a_coordinator_that_is_not_user_scrollable_never_folds() {
    let mut engine = booted(&coordinator_page("overflow-y:hidden"));
    quiet_drag(&mut engine, 0.0, 350.0, 242.0);
    assert_eq!(coordinator_offsets(&mut engine), (0.0, 100.0));
}

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

fn booted_refresh_view_with_methods() -> (TestEngine, u64) {
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
          __SetInlineStyles(header, 'height:50px');
          __SetInlineStyles(inner, 'width:100%;height:100%');
          __SetInlineStyles(tall, 'flex-shrink:0;width:200px;height:1000px');
          __SetInlineStyles(footer, 'height:50px');
          const methods = ['autoStartRefresh', 'finishRefresh'];
          __AddEventListener(view, 'tap', () => {
            const method = methods.shift();
            __InvokeUIMethod(view, method, {}, result => {
              __SetAttribute(view, 'log', method + ':' + result.code);
              __FlushElementTree();
            });
          }, {});
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
    adopt_at(&mut engine, 0.0);
    (engine, container)
}

fn tap_refresh_view(engine: &mut TestEngine, at: f64, log: &str) {
    let before = engine.published_frame().expect("booted").commit_id();
    engine.painter.clock_mut().pin(at);
    engine.dispatch_input(touch(1, PointerPhase::Down, 100.0));
    engine.dispatch_input(touch(1, PointerPhase::Up, 100.0));
    wait_for_log(engine, log);
    next_commit(engine, before);
    adopt_at(engine, at);
}

fn refresh_frames_until_rest(engine: &mut TestEngine, container: u64, mut at: f64) -> (f32, f64) {
    let deadline = at + 3.0;
    while engine.is_animating() {
        at += 1.0 / 60.0;
        assert!(at < deadline, "never came to rest");
        frame_at(engine, at);
    }
    (live_offset(engine, container).y, at)
}

#[test]
fn auto_start_refresh_rests_the_refresh_view_on_its_header() {
    let (mut engine, container) = booted_refresh_view_with_methods();
    assert!((live_offset(&mut engine, container).y - 170.0).abs() < f32::EPSILON);

    tap_refresh_view(&mut engine, 1.0, "autoStartRefresh:0");
    assert_eq!(
        scroll_offset_of(&mut engine, container),
        terminus::link::Vector2D::new(0.0, 120.0),
        "the document moves at once"
    );
    let (rest, _) = refresh_frames_until_rest(&mut engine, container, 1.0);
    assert!(
        (rest - 120.0).abs() < f32::EPSILON,
        "rests on the header's start, not {rest}"
    );
}

#[test]
fn finish_refresh_glides_the_refresh_view_back_to_its_content() {
    let (mut engine, container) = booted_refresh_view_with_methods();
    tap_refresh_view(&mut engine, 1.0, "autoStartRefresh:0");
    let (rest, at) = refresh_frames_until_rest(&mut engine, container, 1.0);
    assert!((rest - 120.0).abs() < f32::EPSILON, "on the header: {rest}");

    tap_refresh_view(&mut engine, at + 0.5, "finishRefresh:0");
    assert!(engine.is_animating(), "a glide, not a jump");
    let (rest, _) = refresh_frames_until_rest(&mut engine, container, at + 0.5);
    assert!(
        (rest - 170.0).abs() < f32::EPSILON,
        "rests on `#content`, not {rest}"
    );
}
