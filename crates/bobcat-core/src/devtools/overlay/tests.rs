//! The `Overlay` domain over a real view: the highlight in captured pixels,
//! inspect mode's taking and routing of input, and what detach and
//! `Overlay.disable` undo.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use terminus::link::Point2D;
use terminus::link::input::{InputEvent, PointerKind, PointerPhase};

use crate::devtools::test_support::{Client, booted};
use crate::test_support::{TestEngine, TestViewSpec};
use crate::view::Screenshot;

const MARGIN: (u16, u16) = (5, 50);
const BORDER: (u16, u16) = (15, 50);
const PADDING: (u16, u16) = (25, 50);
const CONTENT: (u16, u16) = (50, 50);
const B_CONTENT: (u16, u16) = (150, 50);

const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

fn script(listen: bool) -> String {
    let listeners = if listen {
        "for (const name of ['tap', 'pointerdown']) {
           __AddEventListener(a, name, () => {
             counts[name] += 1;
             __SetAttribute(a, name, String(counts[name]));
           }, {});
         }"
    } else {
        ""
    };
    format!(
        "
globalThis.renderPage = function () {{
  const page = __CreatePage('card', 0);
  const stage = __CreateView(0);
  __SetInlineStyles(stage, 'position:absolute;left:0px;top:0px;width:200px;height:100px;background-color:rgb(255,255,255)');
  __AppendElement(page, stage);
  const box = (id, left) => {{
    const view = __CreateView(0);
    __SetID(view, id);
    __SetInlineStyles(view, 'position:absolute;top:0px;left:' + left + 'px;box-sizing:content-box;width:40px;height:40px;padding:10px;border:10px solid rgb(0,0,0);margin:10px;background-color:rgb(255,255,255)');
    __AppendElement(stage, view);
    return view;
  }};
  const a = box('a', 0);
  const b = box('b', 100);
  globalThis.counts = {{ tap: 0, pointerdown: 0 }};
  {listeners}
  globalThis.held = [page, stage, a, b];
  __FlushElementTree();
}};
"
    )
}

fn page(listen: bool) -> TestEngine {
    booted(TestViewSpec::new(&script(listen)).offscreen(200.0, 100.0))
}

fn config() -> Value {
    json!({
        "contentColor": {"r": 255, "g": 0, "b": 0, "a": 0.5},
        "paddingColor": {"r": 0, "g": 255, "b": 0},
        "borderColor": {"r": 0, "g": 0, "b": 255, "a": 0.5},
        "marginColor": {"r": 255, "g": 255, "b": 0, "a": 0.5},
        "showInfo": true,
    })
}

fn node(engine: &mut TestEngine, selector: &'static str) -> u64 {
    engine
        .probe_document(move |document| {
            let page = document.document_element().id();
            document
                .query_selector_all(page, selector)
                .expect("a valid selector")[0]
                .to_bits()
        })
        .expect("the view answers probes")
}

fn attribute(engine: &mut TestEngine, name: &'static str) -> Option<String> {
    engine
        .probe_document(move |document| {
            let page = document.document_element().id();
            let a = document
                .query_selector_all(page, "#a")
                .expect("a valid selector")[0];
            document
                .get(a)
                .and_then(|node| node.attribute(name))
                .map(std::borrow::Cow::into_owned)
        })
        .expect("the view answers probes")
}

fn wait_for_attribute(engine: &mut TestEngine, name: &'static str, value: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while attribute(engine, name).as_deref() != Some(value) {
        assert!(Instant::now() < deadline, "`{name}` never became {value}");
        let _ = engine.pump();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn routed(engine: &TestEngine, name: &str) -> bool {
    engine.view.frames().borrow().listeners.contains(name)
}

fn wait_until_unrouted(engine: &mut TestEngine, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while routed(engine, name) {
        assert!(Instant::now() < deadline, "`{name}` stayed routed");
        let _ = engine.pump();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn after_the_next_commit(engine: &mut TestEngine) {
    for _ in 0..2 {
        let _ = engine.probe_document(|_| ());
    }
}

fn shot(engine: &mut TestEngine) -> Screenshot {
    after_the_next_commit(engine);
    engine.capture().expect("the page captures")
}

fn at(shot: &Screenshot, (x, y): (u16, u16)) -> [u8; 3] {
    let width = usize::try_from(shot.size.width).expect("the frame is addressable");
    let index = (usize::from(y) * width + usize::from(x)) * 4;
    [
        shot.pixels[index],
        shot.pixels[index + 1],
        shot.pixels[index + 2],
    ]
}

fn assert_near(shot: &Screenshot, point: (u16, u16), expected: [u8; 3]) {
    let actual = at(shot, point);
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(&actual, expected)| actual.abs_diff(expected) <= 2),
        "at {point:?}: {actual:?}, expected {expected:?}"
    );
}

fn assert_plain(shot: &Screenshot) {
    assert_near(shot, MARGIN, WHITE);
    assert_near(shot, BORDER, BLACK);
    assert_near(shot, PADDING, WHITE);
    assert_near(shot, CONTENT, WHITE);
    assert_near(shot, B_CONTENT, WHITE);
}

fn assert_highlighted_a(shot: &Screenshot) {
    assert_near(shot, CONTENT, [255, 128, 128]);
    assert_near(shot, PADDING, [0, 255, 0]);
    assert_near(shot, BORDER, [0, 0, 128]);
    assert_near(shot, MARGIN, [255, 255, 128]);
}

fn pointer(engine: &mut TestEngine, (x, y): (u16, u16), device: PointerKind, phase: PointerPhase) {
    let position = Point2D::new(f32::from(x), f32::from(y));
    engine.dispatch_input(InputEvent::pointer(position, 1, device, phase));
}

fn tap(engine: &mut TestEngine, point: (u16, u16)) {
    pointer(engine, point, PointerKind::Touch, PointerPhase::Down);
    pointer(engine, point, PointerKind::Touch, PointerPhase::Up);
}

fn inspect(client: &Client, engine: &mut TestEngine, mode: &str) {
    client.result(
        engine,
        None,
        "Overlay.setInspectMode",
        json!({"mode": mode, "highlightConfig": config()}),
    );
}

#[test]
fn highlight_node_blends_each_area_and_hide_highlight_restores_them() {
    let mut engine = page(false);
    let client = Client::attach(&engine);
    assert_plain(&shot(&mut engine));
    let a = node(&mut engine, "#a");

    let answer = client.result(
        &mut engine,
        None,
        "Overlay.highlightNode",
        json!({"highlightConfig": config(), "nodeId": a}),
    );
    assert_eq!(answer, json!({}));
    let highlighted = shot(&mut engine);
    assert_highlighted_a(&highlighted);
    assert_near(&highlighted, B_CONTENT, WHITE);

    client.result(&mut engine, None, "Overlay.hideHighlight", json!({}));
    assert_plain(&shot(&mut engine));
}

#[test]
fn a_second_highlight_moves_to_the_other_node() {
    let mut engine = page(false);
    let client = Client::attach(&engine);
    let (a, b) = (node(&mut engine, "#a"), node(&mut engine, "#b"));
    client.result(
        &mut engine,
        None,
        "Overlay.highlightNode",
        json!({"highlightConfig": config(), "nodeId": a}),
    );
    assert_highlighted_a(&shot(&mut engine));

    client.result(
        &mut engine,
        None,
        "Overlay.highlightNode",
        json!({"highlightConfig": config(), "backendNodeId": b}),
    );
    let moved = shot(&mut engine);
    assert_near(&moved, CONTENT, WHITE);
    assert_near(&moved, BORDER, BLACK);
    assert_near(&moved, B_CONTENT, [255, 128, 128]);
}

#[test]
fn a_stale_or_missing_id_is_an_error() {
    let mut engine = page(false);
    let client = Client::attach(&engine);
    let stale = engine
        .probe_document(|document| {
            let node = document.create_element("view", ());
            document.drop_element(node);
            node.to_bits()
        })
        .expect("the view answers probes");
    for id in [json!(stale), json!(0), json!(u64::MAX >> 8)] {
        let error = client.error(
            &mut engine,
            None,
            "Overlay.highlightNode",
            json!({"highlightConfig": config(), "nodeId": id}),
        );
        assert_eq!(error["code"], -32000, "{id}");
        assert_eq!(error["message"], "Could not find node with given id");
    }
    let error = client.error(
        &mut engine,
        None,
        "Overlay.highlightNode",
        json!({"highlightConfig": config(), "selector": "#a"}),
    );
    assert_eq!(error["code"], -32602);
    let error = client.error(&mut engine, None, "Overlay.highlightQuad", json!({}));
    assert_eq!(error["code"], -32601);
}

#[test]
fn a_highlight_inside_a_scroller_follows_a_painter_side_scroll() {
    let script = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const scroller = __CreateView(0);
  __SetInlineStyles(scroller, 'position:absolute;left:0px;top:0px;width:100px;height:100px;display:flex;flex-direction:column;overflow:scroll;background-color:rgb(255,255,255)');
  const inner = __CreateView(0);
  __SetID(inner, 'inner');
  __SetInlineStyles(inner, 'flex-shrink:0;width:40px;height:40px;margin-top:30px;background-color:rgb(255,255,255)');
  const filler = __CreateView(0);
  __SetInlineStyles(filler, 'flex-shrink:0;width:40px;height:300px');
  __AppendElement(page, scroller);
  __AppendElement(scroller, inner);
  __AppendElement(scroller, filler);
  globalThis.held = [page, scroller, inner, filler];
  __FlushElementTree();
};
";
    let mut engine = booted(TestViewSpec::new(script).offscreen(100.0, 100.0));
    let client = Client::attach(&engine);
    let inner = node(&mut engine, "#inner");
    client.result(
        &mut engine,
        None,
        "Overlay.highlightNode",
        json!({"highlightConfig": {"contentColor": {"r": 255, "g": 0, "b": 0}}, "nodeId": inner}),
    );
    let before = shot(&mut engine);
    assert_near(&before, (20, 35), [255, 0, 0]);
    assert_near(&before, (20, 15), WHITE);
    let commit = engine
        .painter
        .published_frame()
        .expect("a frame is held")
        .commit_id();

    engine.dispatch_input(InputEvent::wheel(
        Point2D::new(50.0, 50.0),
        terminus::link::Vector2D::new(0.0, 20.0),
    ));
    let scrolled = engine.capture().expect("the page captures");
    assert_eq!(
        engine
            .painter
            .published_frame()
            .expect("a frame is held")
            .commit_id(),
        commit,
        "the painter drew the scroll from the frame it held"
    );
    assert_near(&scrolled, (20, 15), [255, 0, 0]);
    assert_near(&scrolled, (20, 65), WHITE);
}

#[test]
fn inspect_mode_takes_input_highlights_and_requests_the_node() {
    let mut engine = page(true);
    let client = Client::attach(&engine);
    let a = node(&mut engine, "#a");
    assert!(routed(&engine, "pointerdown") && !routed(&engine, "pointerup"));

    inspect(&client, &mut engine, "searchForNode");
    for name in ["pointerdown", "pointermove", "pointerup", "tap"] {
        assert!(routed(&engine, name), "{name}");
    }
    tap(&mut engine, CONTENT);
    let requested = client.event(&mut engine, None, "Overlay.inspectNodeRequested");
    assert_eq!(requested, json!({"backendNodeId": a}));
    assert_highlighted_a(&shot(&mut engine));
    assert_eq!(attribute(&mut engine, "tap"), None);
    assert_eq!(attribute(&mut engine, "pointerdown"), None);

    pointer(
        &mut engine,
        B_CONTENT,
        PointerKind::Mouse,
        PointerPhase::Move,
    );
    let hovered = shot(&mut engine);
    assert_near(&hovered, B_CONTENT, [255, 128, 128]);
    assert_near(&hovered, CONTENT, WHITE);

    inspect(&client, &mut engine, "none");
    assert_plain(&shot(&mut engine));
    assert!(routed(&engine, "pointerdown") && routed(&engine, "tap"));
    assert!(!routed(&engine, "pointermove") && !routed(&engine, "pointerup"));
    tap(&mut engine, CONTENT);
    wait_for_attribute(&mut engine, "tap", "1");
    assert_eq!(attribute(&mut engine, "pointerdown").as_deref(), Some("1"));
}

#[test]
fn inspect_mode_routes_events_for_a_page_with_no_listener() {
    let mut engine = page(false);
    let client = Client::attach(&engine);
    let a = node(&mut engine, "#a");
    assert!(!routed(&engine, "pointermove") && !routed(&engine, "pointerup"));

    inspect(&client, &mut engine, "searchForUAShadowDOM");
    tap(&mut engine, CONTENT);
    let requested = client.event(&mut engine, None, "Overlay.inspectNodeRequested");
    assert_eq!(requested, json!({"backendNodeId": a}));
    assert_highlighted_a(&shot(&mut engine));

    inspect(&client, &mut engine, "none");
    assert!(!routed(&engine, "pointermove") && !routed(&engine, "pointerup"));
}

#[test]
fn an_unsupported_inspect_mode_is_invalid_params() {
    let mut engine = page(false);
    let client = Client::attach(&engine);
    let error = client.error(
        &mut engine,
        None,
        "Overlay.setInspectMode",
        json!({"mode": "captureAreaScreenshot"}),
    );
    assert_eq!(error["code"], -32602);
    assert!(!routed(&engine, "pointermove"));
}

fn highlighted_and_inspecting(engine: &mut TestEngine, client: &Client) {
    client.result(engine, None, "Overlay.enable", json!({}));
    inspect(client, engine, "searchForNode");
    tap(engine, CONTENT);
    client.event(engine, None, "Overlay.inspectNodeRequested");
    assert_highlighted_a(&shot(engine));
}

fn assert_released(engine: &mut TestEngine) {
    wait_until_unrouted(engine, "pointerup");
    assert_plain(&shot(engine));
    assert!(!routed(engine, "pointermove"));
    assert!(routed(engine, "pointerdown") && routed(engine, "tap"));
    tap(engine, CONTENT);
    wait_for_attribute(engine, "tap", "1");
    assert_eq!(attribute(engine, "pointerdown").as_deref(), Some("1"));
}

#[test]
fn detach_removes_the_highlight_and_ends_inspect_mode() {
    let mut engine = page(true);
    let client = Client::attach(&engine);
    highlighted_and_inspecting(&mut engine, &client);
    drop(client);
    assert_released(&mut engine);
}

#[test]
fn overlay_disable_removes_the_highlight_and_ends_inspect_mode() {
    let mut engine = page(true);
    let client = Client::attach(&engine);
    highlighted_and_inspecting(&mut engine, &client);
    client.result(&mut engine, None, "Overlay.disable", json!({}));
    assert_released(&mut engine);
}

#[test]
fn with_nothing_attached_input_is_delivered() {
    let mut engine = page(true);
    tap(&mut engine, CONTENT);
    wait_for_attribute(&mut engine, "tap", "1");
    assert_eq!(attribute(&mut engine, "pointerdown").as_deref(), Some("1"));
}
