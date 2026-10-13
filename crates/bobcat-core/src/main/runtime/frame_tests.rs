//! `<frame src>` end to end, over a real view: the page realm's
//! `__CreateFrame` and `__SetAttribute(frame, "src", …)`, `bobcat:frame`'s
//! fetch and `ShadowRealm`, and `bobcat:frame-boot` binding the child and
//! running the nested card's `root` in it.
//!
//! The host stands in for a container installer: it answers the plain fetch
//! of the card's URL, and serves `<url>/root.js` (the card's root as an MTS
//! entry, `MTS_CHUNK_PREAMBLE` and then the body) and `<url>/style-info.css`
//! (its page CSS), which is what `bobcat-source`'s `BundleInstaller`
//! registers for a fetched full card.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dom::stylo::color::AbsoluteColor;

use crate::main::tree::LynxDocument;
use crate::test_support::{TestEngine, TestViewSpec};
use crate::view::{EngineEvent, NoWakeup};

const CARD: &str = "app:///child.web.bundle";

/// The nested card's `root`, as a compiled card's root defines its
/// `renderPage`: a global the boot calls with the processed init data. It
/// throws — which the test sees as a diagnostic — unless its `__Card__` is
/// `src` as the frame wrote it and its `SystemInfo` is the host's.
fn root(src: &str) -> String {
    format!(
        "if (__Card__ !== '{src}') throw Error('__Card__ is ' + __Card__); \
        if (SystemInfo.pixelWidth !== 393) throw Error('SystemInfo ' + JSON.stringify(SystemInfo)); \
        globalThis.renderPage = () => {{ const p = __CreatePage('0', 0); \
        const t = __CreateText(0); __AppendElement(t, __CreateRawText('nested')); \
        __AppendElement(p, t); }};"
    )
}

/// The host page: a text of its own, then a 100×50 frame showing `src`.
/// `updatePage` clears the frame's `src`, so a test reaches the page realm
/// through `LynxView::update_data`.
fn host_entry(src: &str) -> String {
    format!(
        r"
        const page = __CreatePage('0', 0);
        const own = __CreateText(0);
        __AppendElement(own, __CreateRawText('host'));
        __AppendElement(page, own);
        const frame = __CreateFrame(0);
        __SetAttribute(frame, 'style', 'width:100px;height:50px');
        __SetAttribute(frame, 'src', '{src}');
        __AppendElement(page, frame);
        __FlushElementTree();
        globalThis.updatePage = () => {{
            __SetAttribute(frame, 'src', null);
            __FlushElementTree();
        }};
    "
    )
}

/// A host serving the card at [`CARD`], with its root and its page CSS,
/// for a frame whose `src` is `src`.
fn serving_the_card(src: &str) -> TestViewSpec {
    serving_a_root(src, &root(src))
}

/// The same, with `root` as the card's root body.
fn serving_a_root(src: &str, root: &str) -> TestViewSpec {
    TestViewSpec::new(&host_entry(src))
        .with_fetchable(CARD)
        .with_module(&format!("{CARD}/root.js"), &super::card_entry(root))
        .serving_style_sheet(&format!("{CARD}/style-info.css"), "text { color: red }")
        .with_style_sheet_probe()
}

/// What the frame shows, read off the document.
struct FrameState {
    frame_size: (f32, f32),
    /// The nested page's id and its element children's ids.
    nested: Vec<dom::NodeId>,
    nested_color: Option<AbsoluteColor>,
    host_color: Option<AbsoluteColor>,
}

fn frame_state(tree: &mut LynxDocument) -> Option<FrameState> {
    let page = tree.document_element();
    let host_text = page
        .children()
        .find(|child| child.tag_name() == Some("text"))?
        .id();
    let frame = page
        .children()
        .find(|child| child.tag_name() == Some("frame"))?
        .id();
    let color = |tree: &LynxDocument, id| {
        tree.get(id)
            .and_then(dom::Node::computed_style)
            .map(|style| style.get_color().solid_color())
    };
    let host_color = color(tree, host_text);
    let frame_box = tree.bounding_client_rect(frame)?;
    let shadow = tree.shadow_root(frame)?;
    let mut nested = Vec::new();
    let mut nested_color = None;
    if let Some(nested_page) = tree
        .get(shadow)
        .and_then(|shadow| shadow.children().find(|child| child.is_element()))
    {
        nested.push(nested_page.id());
        for child in nested_page.children().filter(|child| child.is_element()) {
            nested.push(child.id());
            if child.tag_name() == Some("text") {
                nested_color = color(tree, child.id());
            }
        }
    }
    Some(FrameState {
        frame_size: (frame_box.size.width, frame_box.size.height),
        nested,
        nested_color,
        host_color,
    })
}

/// Pumps until `done` holds of the frame, collecting every event; any
/// event that ends the view fails the test.
fn pump_until(
    engine: &mut TestEngine,
    events: &mut Vec<EngineEvent>,
    what: &str,
    done: impl Fn(&FrameState) -> bool,
) -> FrameState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in engine.pump() {
            assert!(!event.is_fatal(), "the view ended: {event:?}");
            events.push(event);
        }
        // Briefly: the document's thread may be parked on a source the next
        // turn of this one answers.
        if let Some(state) = engine
            .view
            .probe_document_within(Duration::from_millis(20), frame_state)
            .flatten()
            && done(&state)
        {
            return state;
        }
        assert!(Instant::now() < deadline, "{what}: {events:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Every diagnostic among `events`, as its message.
fn reported(events: &[EngineEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ScriptReported { message, .. } => Some(message.as_str()),
            _ => None,
        })
        .collect()
}

/// A frame's `src` boots the card in a `ShadowRealm` bound to it: the
/// card's tree is the nested page in the frame's shadow root, inside the
/// frame's own 100×50 box, and its page CSS styles that tree alone. Clearing
/// `src` frees the nested tree.
#[test]
fn a_frames_src_boots_the_card_inside_it() {
    a_frame_boots(CARD);
}

/// A relative `src` is the fetcher's to resolve, against the view's base:
/// the frame hands it over as written, and its section URLs are written
/// from that same string, so they resolve to what the fetch registered.
#[test]
fn a_relative_frame_src_resolves_against_the_view_base() {
    a_frame_boots("child.web.bundle");
}

fn a_frame_boots(src: &str) {
    let mut engine = serving_the_card(src).create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    let state = pump_until(
        &mut engine,
        &mut events,
        "the nested card rendered",
        |state| state.nested.len() == 2 && state.nested_color.is_some(),
    );
    assert_eq!(state.frame_size, (100.0, 50.0));
    let red = AbsoluteColor::srgb_legacy(255, 0, 0, 1.0);
    assert_eq!(
        state.nested_color,
        Some(red),
        "the card's page CSS styles its text"
    );
    assert_eq!(
        state.host_color,
        Some(AbsoluteColor::BLACK),
        "and not the host's"
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
    assert!(
        engine.view.published_frame().is_some(),
        "the view published a frame"
    );

    let nested = state.nested;
    engine
        .view
        .update_data("{}".to_owned(), String::new())
        .expect("the view takes an update");
    pump_until(
        &mut engine,
        &mut events,
        "the nested card was released",
        |state| state.nested.is_empty(),
    );
    let freed = engine
        .probe_document(move |tree| nested.iter().all(|id| tree.get(*id).is_none()))
        .expect("the document answers");
    assert!(freed, "clearing src freed every nested node");
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A `src` the fetcher cannot fetch is one diagnostic naming it, and the
/// view goes on: boot finishes and the frame stays empty.
#[test]
fn a_frame_src_that_does_not_fetch_is_one_diagnostic() {
    let missing = "app:///missing.web.bundle";
    let mut engine = serving_the_card(missing).create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let finished = |events: &[EngineEvent]| {
        events
            .iter()
            .any(|event| matches!(event, EngineEvent::ScriptFinished))
    };
    while !finished(&events) || reported(&events).is_empty() {
        for event in engine.pump() {
            assert!(!event.is_fatal(), "the view ended: {event:?}");
            events.push(event);
        }
        assert!(Instant::now() < deadline, "no diagnostic: {events:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
    // A few more turns, for a second report that should not come.
    for _ in 0..20 {
        events.extend(engine.pump());
        std::thread::sleep(Duration::from_millis(2));
    }
    let messages = reported(&events);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains(&format!("frame {missing}")),
        "{messages:?}"
    );
    let state = engine
        .probe_document(frame_state)
        .flatten()
        .expect("the host page rendered");
    assert!(state.nested.is_empty(), "the frame stays empty");
    assert!(events.iter().all(|event| !event.is_fatal()));
}

/// The card's root is app code: one that throws is one diagnostic, reported
/// in the frame's realm, and boot goes on to render what it defined, as the
/// page's boot renders a page whose entry threw.
#[test]
fn a_throwing_root_is_reported_once_and_still_renders() {
    let mut engine = serving_a_root(
        CARD,
        &format!("{} throw Error('the root threw');", root(CARD)),
    )
    .create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    pump_until(
        &mut engine,
        &mut events,
        "the nested card rendered",
        |state| state.nested.len() == 2,
    );
    for _ in 0..20 {
        events.extend(engine.pump());
        std::thread::sleep(Duration::from_millis(2));
    }
    let messages = reported(&events);
    assert_eq!(messages.len(), 1, "{events:?}");
    assert!(messages[0].contains("the root threw"), "{messages:?}");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, EngineEvent::ScriptRunError(_))),
        "the throw is not reported a second time: {events:?}"
    );
}
