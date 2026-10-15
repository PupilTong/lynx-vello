//! `<frame src>` end to end, over a real view: the page realm's
//! `__CreateFrame` and `__SetAttribute(frame, "src", …)`, `bobcat:frame`'s
//! fetch and `ShadowRealm`, `bobcat:frame-boot` running the nested card's
//! `root` in the child under the frame's nested page, the frame's `data`
//! and `global-props`, which the card boots with and takes as updates, and
//! the frame's connection and disconnection, which the custom element
//! queues and `bobcat:frame` turns into a load and a disposal.
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

/// A second card the host serves, with no page CSS: what replaces [`CARD`]
/// when the host page writes a new `src`.
const SECOND: &str = "app:///second.web.bundle";

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
/// `updatePage` writes the data's `src` to the frame, `null` included, so a
/// test reaches the page realm through `LynxView::update_data`; a `data` or
/// a `globalProps` key instead writes its value, as the object itself, to the
/// frame's `data` or `global-props`; an `op` detaches the frame, attaches it
/// again, lets go of its handle, or logs what two UI methods answer for it.
fn host_entry(src: &str) -> String {
    host_page(src, true)
}

/// [`host_entry`], with the frame appended to the page or left detached.
fn host_page(src: &str, attached: bool) -> String {
    host_page_with(src, attached, "")
}

/// [`host_page`], running `before` — which names the frame `frame` — after
/// the frame is created and before its `src` is written.
fn host_page_with(src: &str, attached: bool, before: &str) -> String {
    format!(
        r"
        const page = __CreatePage('0', 0);
        const own = __CreateText(0);
        __AppendElement(own, __CreateRawText('host'));
        __AppendElement(page, own);
        let frame = __CreateFrame(0);
        __SetAttribute(frame, 'style', 'width:100px;height:50px');
        {before}
        __SetAttribute(frame, 'src', '{src}');
        if ({attached}) __AppendElement(page, frame);
        __FlushElementTree();
        globalThis.updatePage = (data) => {{
            if (data.op === 'detach') {{
                __RemoveElement(page, frame);
            }} else if (data.op === 'attach') {{
                __AppendElement(page, frame);
            }} else if (data.op === 'forget') {{
                frame = null;
                // Enough removals for the realm's collection, so nothing
                // waits on the collector's own schedule.
                for (let i = 0; i < 40; ++i) {{
                    const view = __CreateView(0);
                    __AppendElement(page, view);
                    __RemoveElement(page, view);
                }}
            }} else if (data.op === 'measure') {{
                const answers = {{}};
                for (const method of ['boundingClientRect', 'updateData']) {{
                    __InvokeUIMethod(frame, method, {{}}, result => {{
                        answers[method] = result;
                    }});
                }}
                const rect = answers.boundingClientRect;
                const refused = answers.updateData;
                console.log('measured ' + rect.code + ' ' +
                    [rect.data.left, rect.data.top, rect.data.width, rect.data.height].join(',') +
                    ' ' + refused.code + ' ' + String(refused.data));
            }} else if ('data' in data) {{
                __SetAttribute(frame, 'data', data.data);
            }} else if ('globalProps' in data) {{
                __SetAttribute(frame, 'global-props', data.globalProps);
            }} else {{
                __SetAttribute(frame, 'src', data.src);
            }}
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

/// The same, with `root` as the card's root body. [`SECOND`] is served
/// too, with its own root and no page CSS.
fn serving_a_root(src: &str, root: &str) -> TestViewSpec {
    TestViewSpec::new(&host_entry(src))
        .with_fetchable(CARD)
        .with_module(&format!("{CARD}/root.js"), &super::card_entry(root))
        .serving_style_sheet(&format!("{CARD}/style-info.css"), "text { color: red }")
        .with_fetchable(SECOND)
        .with_module(
            &format!("{SECOND}/root.js"),
            &super::card_entry(&self::root(SECOND)),
        )
        .with_style_sheet_probe()
}

/// Writes `src` (JSON: a string or `null`) to the host's frame through
/// `updatePage`.
fn write_src(engine: &mut TestEngine, src: &str) {
    engine
        .view
        .update_data(format!(r#"{{"src":{src}}}"#), String::new())
        .expect("the view takes an update");
}

/// Runs one of `updatePage`'s operations on the host's frame.
fn operate(engine: &mut TestEngine, op: &str) {
    engine
        .view
        .update_data(format!(r#"{{"op":"{op}"}}"#), String::new())
        .expect("the view takes an update");
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
/// frame's own 100×50 box, and its page CSS styles that tree alone. Removing
/// `src` keeps the card, as native and web-core do; a different `src`
/// replaces it, and the first card's nodes are freed.
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
    // A removal does nothing. `update_data` is answered in order, so once
    // a later update has run, so has this one.
    write_src(&mut engine, "null");
    write_src(&mut engine, &format!("{src:?}"));
    for _ in 0..20 {
        events.extend(engine.pump());
        std::thread::sleep(Duration::from_millis(2));
    }
    let kept = pump_until(&mut engine, &mut events, "the card is kept", |_| true);
    assert_eq!(kept.nested, nested, "removing src keeps the nested tree");
    assert_eq!(kept.nested_color, Some(red));

    write_src(&mut engine, &format!("{SECOND:?}"));
    // The nested page is the frame's for its life; what is under it is the
    // card's.
    let page = nested[0];
    let replaced = pump_until(
        &mut engine,
        &mut events,
        "the second card replaced the first",
        |state| state.nested.len() == 2 && !nested[1..].contains(&state.nested[1]),
    );
    assert_eq!(replaced.nested[0], page, "the nested page stays");
    assert_eq!(
        replaced.nested_color,
        Some(AbsoluteColor::BLACK),
        "the second card has no page CSS, and the first card's is gone"
    );
    let freed = engine
        .probe_document(move |tree| nested[1..].iter().all(|id| tree.get(*id).is_none()))
        .expect("the document answers");
    assert!(freed, "a new src freed every node of the first card");
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
    assert_eq!(state.nested.len(), 1, "the frame's nested page stays empty");
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

/// A card whose `root` also logs `tick` every few milliseconds, for as long
/// as its realm lives.
fn ticking_root() -> String {
    format!("{} setInterval(() => console.log('tick'), 5);", root(CARD))
}

/// How many `tick`s among `events`.
fn ticks(events: &[EngineEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(event, EngineEvent::ConsoleMessage { message, .. } if message == "tick")
        })
        .count()
}

/// Pumps for `duration`, answering every event.
fn pump_for(engine: &mut TestEngine, duration: Duration) -> Vec<EngineEvent> {
    let deadline = Instant::now() + duration;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        for event in engine.pump() {
            assert!(!event.is_fatal(), "the view ended: {event:?}");
            events.push(event);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    events
}

/// Pumps until the card's realm has logged a `tick`.
fn pump_until_ticking(engine: &mut TestEngine, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while ticks(&pump_for(engine, Duration::from_millis(10))) == 0 {
        assert!(Instant::now() < deadline, "{what}: no tick");
    }
}

/// Asserts that no `tick` comes any more: what the card's realm queued
/// before it was let go of is drained first, then nothing more arrives.
fn assert_no_more_ticks(engine: &mut TestEngine, what: &str) {
    pump_for(engine, Duration::from_millis(50));
    let later = pump_for(engine, Duration::from_millis(200));
    assert_eq!(ticks(&later), 0, "{what}: {later:?}");
}

/// The nested page of the frame `frame` and its element children, read
/// wherever the frame is.
fn nested_of(engine: &mut TestEngine, frame: dom::NodeId) -> Vec<dom::NodeId> {
    engine
        .probe_document(move |tree| {
            let page = crate::main::tree::nested_page(tree, frame).expect("a frame");
            let mut nested = vec![page];
            nested.extend(
                tree.get(page)
                    .into_iter()
                    .flat_map(|page| page.children().filter(|child| child.is_element()))
                    .map(dom::Node::id),
            );
            nested
        })
        .expect("the document answers")
}

/// Disconnecting a frame disposes of its card, as web-core's `LynxView`
/// does on `disconnectedCallback`: the nested page is emptied, its sheet
/// removed and the card's realm gone, so its timers stop. Connecting it
/// again loads the frame's `src` again, as `connectedCallback` renders it.
#[test]
fn a_removed_frame_disposes_of_its_card_and_reappending_renders_it_again() {
    let mut engine = serving_a_root(CARD, &ticking_root()).create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    pump_until(
        &mut engine,
        &mut events,
        "the nested card rendered",
        |state| state.nested.len() == 2 && state.nested_color.is_some(),
    );
    pump_until_ticking(&mut engine, "the card's realm runs");
    let frame = engine
        .probe_document(|tree| {
            tree.document_element()
                .children()
                .find(|child| child.tag_name() == Some("frame"))
                .map(dom::Node::id)
        })
        .flatten()
        .expect("the page holds the frame");
    let first = nested_of(&mut engine, frame);

    operate(&mut engine, "detach");
    let deadline = Instant::now() + Duration::from_secs(30);
    while nested_of(&mut engine, frame).len() > 1 {
        events.extend(pump_for(&mut engine, Duration::from_millis(5)));
        assert!(Instant::now() < deadline, "the nested page was not emptied");
    }
    assert_eq!(
        nested_of(&mut engine, frame),
        first[..1],
        "the nested page stays"
    );
    assert_no_more_ticks(&mut engine, "the disconnected frame's card is disposed of");

    operate(&mut engine, "attach");
    let again = pump_until(
        &mut engine,
        &mut events,
        "the card rendered again",
        |state| state.nested.len() == 2 && state.nested[1] != first[1],
    );
    assert_eq!(again.nested[0], first[0]);
    assert_eq!(
        again.nested_color,
        Some(AbsoluteColor::srgb_legacy(255, 0, 0, 1.0)),
        "its page CSS is mounted again"
    );
    pump_until_ticking(&mut engine, "a new realm runs the card");
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A frame that was given its `src` while detached loads and renders all
/// the same; one whose handle is then collected — it never connected, so
/// no disconnection disposes of it — disposes of its card through
/// `bobcat:frame`'s `FinalizationRegistry`, and no orphan realm is left
/// running on its timers.
#[test]
fn a_collected_frame_handle_disposes_of_its_card() {
    let mut engine = TestViewSpec::new(&host_page(CARD, false))
        .with_fetchable(CARD)
        .with_module(
            &format!("{CARD}/root.js"),
            &super::card_entry(&ticking_root()),
        )
        .serving_style_sheet(&format!("{CARD}/style-info.css"), "text { color: red }")
        .with_style_sheet_probe()
        .create(Arc::new(NoWakeup));
    pump_until_ticking(&mut engine, "the detached frame's card runs");
    operate(&mut engine, "forget");
    assert_no_more_ticks(&mut engine, "the collected frame's card is disposed of");
}

/// A card's `root` that shows its page data: one `text` reading
/// `<data.label>/<global props' message>`, which `renderPage` builds from
/// the processed init data. Its `updatePage` merges an update into
/// `lynx.__initData`, as a compiled card's own `updatePage` keeps its data,
/// and its `updateGlobalProps` reads the merged `lynx.__globalProps`; both
/// rewrite the same raw text in place. It logs `booted <src>` once per realm
/// it runs in.
fn data_root(src: &str) -> String {
    format!(
        "if (__Card__ !== '{src}') throw Error('__Card__ is ' + __Card__); \
        console.log('booted ' + __Card__); \
        let raw; \
        const label = (data) => data.label + '/' + lynx.__globalProps.message; \
        globalThis.renderPage = (data) => {{ const p = __CreatePage('0', 0); \
        const t = __CreateText(0); raw = __CreateRawText(label(data)); \
        __AppendElement(t, raw); __AppendElement(p, t); }}; \
        globalThis.updatePage = (data) => {{ \
        lynx.__initData = {{ ...lynx.__initData, ...data }}; \
        __SetAttribute(raw, 'text', label(lynx.__initData)); __FlushElementTree(); }}; \
        globalThis.updateGlobalProps = () => {{ \
        __SetAttribute(raw, 'text', label(lynx.__initData)); __FlushElementTree(); }};"
    )
}

/// A host running `entry` that serves [`CARD`] and [`SECOND`], each with a
/// [`data_root`] and no page CSS.
fn serving_data_cards(entry: &str) -> TestViewSpec {
    TestViewSpec::new(entry)
        .with_fetchable(CARD)
        .with_module(
            &format!("{CARD}/root.js"),
            &super::card_entry(&data_root(CARD)),
        )
        .with_fetchable(SECOND)
        .with_module(
            &format!("{SECOND}/root.js"),
            &super::card_entry(&data_root(SECOND)),
        )
        .with_style_sheet_probe()
}

/// Writes `value` (JSON, `null` for a removal) to the host's frame through
/// `updatePage`: `key` is `data` for the frame's `data`, `globalProps` for
/// its `global-props`.
fn write_frame(engine: &mut TestEngine, key: &str, value: &str) {
    engine
        .view
        .update_data(format!(r#"{{"{key}":{value}}}"#), String::new())
        .expect("the view takes an update");
}

/// The text the frame's card shows — the raw text of the nested page's
/// `text` — and that `text`'s id, read off the document.
fn nested_text(tree: &mut LynxDocument) -> Option<(dom::NodeId, String)> {
    let frame = tree
        .document_element()
        .children()
        .find(|child| child.tag_name() == Some("frame"))?
        .id();
    let page = crate::main::tree::nested_page(tree, frame)?;
    let text = tree
        .get(page)?
        .children()
        .find(|child| child.tag_name() == Some("text"))?;
    let raw = text
        .children()
        .find(|child| child.tag_name() == Some("raw-text"))?;
    Some((text.id(), raw.attribute("text")?.to_owned()))
}

/// Pumps until the frame's card shows `expected` — in a `text` other than
/// `replacing`, when given, so that a card booted in place of another one
/// showing the same text is told apart — collecting every event, and
/// answers the id of the `text` showing it.
fn pump_until_text(
    engine: &mut TestEngine,
    events: &mut Vec<EngineEvent>,
    expected: &str,
    replacing: Option<dom::NodeId>,
) -> dom::NodeId {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in engine.pump() {
            assert!(!event.is_fatal(), "the view ended: {event:?}");
            events.push(event);
        }
        if let Some((text, shown)) = engine
            .view
            .probe_document_within(Duration::from_millis(20), nested_text)
            .flatten()
            && shown == expected
            && replacing != Some(text)
        {
            // What the card logged before it rendered was sent before the
            // probe was answered: one more turn collects it.
            events.extend(engine.pump());
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "the card never showed {expected:?}: {events:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// How many realms booted `src` among `events`.
fn boots(events: &[EngineEvent], src: &str) -> usize {
    let booted = format!("booted {src}");
    events
        .iter()
        .filter(|event| {
            matches!(event, EngineEvent::ConsoleMessage { message, .. } if *message == booted)
        })
        .count()
}

/// A frame's `data` and `global-props`: what is written before `src` is
/// what the card boots with; a write after boot is the card's own
/// `updateData`/`updateGlobalProps`, re-rendered in place by the same realm;
/// a new `src` and a reconnection boot with the last plain objects written;
/// anything else is ignored without a diagnostic, and a removal leaves the
/// running card as it is and boots the next one with `{}`.
#[test]
fn a_frames_data_and_global_props_reach_its_card() {
    let before = "__SetAttribute(frame, 'data', {label: 'first'}); \
                  __SetAttribute(frame, 'global-props', {message: 'hello'});";
    let mut engine =
        serving_data_cards(&host_page_with(CARD, true, before)).create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    let text = pump_until_text(&mut engine, &mut events, "first/hello", None);
    assert_eq!(boots(&events, CARD), 1);

    write_frame(&mut engine, "data", r#"{"label":"second"}"#);
    assert_eq!(
        pump_until_text(&mut engine, &mut events, "second/hello", None),
        text,
        "an update re-renders the card in place"
    );
    write_frame(&mut engine, "globalProps", r#"{"message":"there"}"#);
    assert_eq!(
        pump_until_text(&mut engine, &mut events, "second/there", None),
        text
    );
    assert_eq!(boots(&events, CARD), 1, "and boots nothing");

    // None of these is a plain object, so none replaces what the next
    // boot reads.
    for ignored in ["3", "[1]", r#""nope""#, r#""[1]""#] {
        write_frame(&mut engine, "data", ignored);
        write_frame(&mut engine, "globalProps", ignored);
    }
    write_src(&mut engine, &format!("{SECOND:?}"));
    // The first card showed the same text: the second one is its own `text`.
    let second = pump_until_text(&mut engine, &mut events, "second/there", Some(text));
    assert_eq!(boots(&events, SECOND), 1);

    // A write while the frame is detached reaches the card it boots when
    // it is attached again.
    operate(&mut engine, "detach");
    write_frame(&mut engine, "data", r#"{"label":"third"}"#);
    operate(&mut engine, "attach");
    let third = pump_until_text(&mut engine, &mut events, "third/there", Some(second));
    assert_eq!(boots(&events, SECOND), 2);

    // A removal leaves the running card's data alone: the next update
    // still reads `third`.
    write_frame(&mut engine, "data", "null");
    write_frame(&mut engine, "globalProps", r#"{"message":"last"}"#);
    assert_eq!(
        pump_until_text(&mut engine, &mut events, "third/last", None),
        third
    );
    // And the next boot reads `{}`, which has no label.
    operate(&mut engine, "detach");
    operate(&mut engine, "attach");
    pump_until_text(&mut engine, &mut events, "undefined/last", Some(third));
    assert_eq!(boots(&events, SECOND), 3);
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A frame answers the base set's methods as any element does —
/// `boundingClientRect` with code 0 and its box — and has no method of its
/// own: `updateData` is code 3, the shared table's "no such method".
#[test]
fn a_frame_answers_the_base_methods_and_none_of_its_own() {
    let mut engine = serving_the_card(CARD).create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    pump_until(
        &mut engine,
        &mut events,
        "the nested card rendered",
        |state| state.nested.len() == 2,
    );
    operate(&mut engine, "measure");
    let deadline = Instant::now() + Duration::from_secs(30);
    let measured = loop {
        for event in engine.pump() {
            assert!(!event.is_fatal(), "the view ended: {event:?}");
            events.push(event);
        }
        if let Some(message) = events.iter().find_map(|event| match event {
            EngineEvent::ConsoleMessage { message, .. } if message.starts_with("measured ") => {
                Some(message.clone())
            }
            _ => None,
        }) {
            break message;
        }
        assert!(Instant::now() < deadline, "nothing measured: {events:?}");
        std::thread::sleep(Duration::from_millis(2));
    };
    let rect = engine
        .probe_document(|tree| {
            let frame = tree
                .document_element()
                .children()
                .find(|child| child.tag_name() == Some("frame"))?
                .id();
            tree.bounding_client_rect(frame)
        })
        .flatten()
        .expect("the frame has a box");
    assert_eq!((rect.size.width, rect.size.height), (100.0, 50.0));
    assert_eq!(
        measured,
        format!(
            "measured 0 {},{},{},{} 3 undefined",
            rect.origin.x, rect.origin.y, rect.size.width, rect.size.height
        )
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}
