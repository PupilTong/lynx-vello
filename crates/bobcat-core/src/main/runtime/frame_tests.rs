//! `<frame src>` end to end, over a real view: the page realm's
//! `__CreateFrame` and `__SetAttribute(frame, "src", …)`, `bobcat:frame`'s
//! fetch and `ShadowRealm`, `bobcat:frame-boot` running the nested card's
//! `root` in the child under the frame's nested page and connecting the
//! card's own background thread, a `Worker` the child creates, the frame's
//! `data` and `global-props`, which the card boots with and takes as updates
//! on both threads, and the frame's connection and disconnection, which the
//! custom element queues and `bobcat:frame` turns into a load and a
//! disposal that waits for the card's background thread. Then input events
//! inside a frame, which the kernel's walk delivers to each listening realm
//! once, at the path's top: the page realm first, then each card's realm,
//! each walking the steps it holds handles for. Then exposure and the nested
//! lifecycle: each record is an event at its element and each connection an
//! event at its frame, which the listener table hands the realm holding the
//! handle, so each card's BTS hears its own elements, a card's
//! `stopExposure` stops its own, a card let go of sends its last
//! `disexposure` list before its BTS is disposed of and its page reset, and
//! a frame a card created is that card's to load and let go of.
//!
//! The host stands in for a container installer: it answers the plain fetch
//! of the card's URL, and serves `<url>/root.js` (the card's root as an MTS
//! entry, `MTS_CHUNK_PREAMBLE` and then the body), `<url>/style-info.css`
//! (its page CSS) and `<url>#background-thread` (its BTS boot, here a
//! stand-in that logs what the BTS is handed), which is what
//! `bobcat-source`'s `BundleInstaller` registers for a fetched full card.

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
/// `card`, the frame's `src` resolved against the view's base, and its
/// `SystemInfo` is the host's.
fn root(card: &str) -> String {
    format!(
        "if (__Card__ !== '{card}') throw Error('__Card__ is ' + __Card__); \
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

/// The BTS boot a host serves at `<card>#background-thread`, which a
/// frame's card connects its background thread to: a stand-in for
/// `bobcat-source`'s, which loads the card's `/app-service.js`, that logs
/// what the BTS was started with and every update and app hook it is
/// handed, each line as `bts <card> …`, and then runs `extra`.
///
/// `data` lines also read the group's shared data under `from`, which a
/// page's BTS may have set.
fn bts_boot(card: &str, extra: &str) -> String {
    format!(
        "import {{ lynx, NativeModules }} from 'bobcat:bts-runtime';
        const app = lynx.getApp();
        const log = line => console.log('bts {card} ' + line);
        log('booted ' + lynx.__initData.label + '/' + lynx.__globalProps.message);
        app.updateCardData = data => log('data ' + data.label + ' shared ' + lynx.getSharedData('from'));
        app.updateGlobalProps = props => log('props ' + props.message);
        app.callDestroyLifetimeFun = () => log('destroyed');
        {extra}"
    )
}

/// `spec` serving `card`'s BTS boot, [`bts_boot`] with `extra`.
fn with_bts(spec: TestViewSpec, card: &str, extra: &str) -> TestViewSpec {
    spec.with_module(&format!("{card}#background-thread"), &bts_boot(card, extra))
}

/// A host serving the card at [`CARD`], with its root, its page CSS and its
/// BTS boot, for a frame whose `src` is `src`.
fn serving_the_card(src: &str) -> TestViewSpec {
    serving_a_root(src, &root(CARD))
}

/// The same, with `root` as the card's root body. [`SECOND`] is served
/// too, with its own root, its BTS boot and no page CSS.
fn serving_a_root(src: &str, root: &str) -> TestViewSpec {
    let spec = TestViewSpec::new(&host_entry(src))
        .with_fetchable(CARD)
        .with_module(&format!("{CARD}/root.js"), &super::card_entry(root))
        .serving_style_sheet(&format!("{CARD}/style-info.css"), "text { color: red }")
        .with_fetchable(SECOND)
        .with_module(
            &format!("{SECOND}/root.js"),
            &super::card_entry(&self::root(SECOND)),
        )
        .with_style_sheet_probe();
    with_bts(with_bts(spec, CARD, ""), SECOND, "")
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

/// Every diagnostic among `events`, as its message: the realms' own reports,
/// a listener's throw in any realm, and every worker's throw or end, so a
/// frame's card's BTS that failed is one too.
fn reported(events: &[EngineEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ScriptReported { message, .. } => Some(message.as_str()),
            EngineEvent::ListenerFailed(error) => Some(&*error.message),
            EngineEvent::WorkerThrew { error, .. } | EngineEvent::WorkerEnded { error, .. } => {
                Some(&*error.message)
            }
            _ => None,
        })
        .collect()
}

/// Whether some console message among `events` is exactly `message`.
fn logged(events: &[EngineEvent], message: &str) -> bool {
    events.iter().any(
        |event| matches!(event, EngineEvent::ConsoleMessage { message: seen, .. } if seen == message),
    )
}

/// How many console messages among `events` are exactly `message`.
fn logged_times(events: &[EngineEvent], message: &str) -> usize {
    events
        .iter()
        .filter(
            |event| matches!(event, EngineEvent::ConsoleMessage { message: seen, .. } if seen == message),
        )
        .count()
}

/// Pumps until a console message among `events` is exactly `message`,
/// collecting every event.
fn pump_until_logged(engine: &mut TestEngine, events: &mut Vec<EngineEvent>, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !logged(events, message) {
        for event in engine.pump() {
            assert!(!event.is_fatal(), "the view ended: {event:?}");
            events.push(event);
        }
        assert!(
            Instant::now() < deadline,
            "nothing logged {message:?}: {events:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
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

/// A relative `src` is resolved against the view's base once, by
/// `resolveUrl`, the way the fetcher resolves it: the card's `__Card__`, its
/// section URLs and its BTS entry are all that absolute URL, so they name
/// what the fetch registered, while the frame keeps `src` as written for
/// its unchanged-`src` check.
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
    // Its background thread, at the card's resolved URL.
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {CARD} booted undefined/undefined"),
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
    // The nested page is the card's: the second card renders under a page
    // of its own.
    let replaced = pump_until(
        &mut engine,
        &mut events,
        "the second card replaced the first",
        |state| state.nested.len() == 2 && !nested.contains(&state.nested[1]),
    );
    assert_ne!(replaced.nested[0], nested[0], "a fresh nested page");
    assert_eq!(
        replaced.nested_color,
        Some(AbsoluteColor::BLACK),
        "the second card has no page CSS, and the first card's is gone"
    );
    let freed = engine
        .probe_document(move |tree| nested.iter().all(|id| tree.get(*id).is_none()))
        .expect("the document answers");
    assert!(
        freed,
        "a new src freed every node of the first card, its page included"
    );
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
/// before it was let go of, and what it ran while its BTS acknowledged its
/// disposal, are drained first, then nothing more arrives.
fn assert_no_more_ticks(engine: &mut TestEngine, what: &str) {
    pump_for(engine, Duration::from_millis(150));
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
/// does on `disconnectedCallback`: the card's nested page is freed and an
/// empty one put in its place, its sheet removed and the card's realm gone,
/// so its timers stop. Connecting it again loads the frame's `src` again,
/// into that page, as `connectedCallback` renders it.
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
    let emptied = nested_of(&mut engine, frame);
    assert_ne!(
        emptied[0], first[0],
        "an empty page of its own for the next card"
    );
    let gone = first.clone();
    assert!(
        engine
            .probe_document(move |tree| gone.iter().all(|id| tree.get(*id).is_none()))
            .expect("the document answers"),
        "the card's page went with it"
    );
    assert_no_more_ticks(&mut engine, "the disconnected frame's card is disposed of");

    operate(&mut engine, "attach");
    let again = pump_until(
        &mut engine,
        &mut events,
        "the card rendered again",
        |state| state.nested.len() == 2 && state.nested[1] != first[1],
    );
    assert_eq!(
        again.nested[0], emptied[0],
        "rendered into the page the reset built"
    );
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
    let spec = TestViewSpec::new(&host_page(CARD, false))
        .with_fetchable(CARD)
        .with_module(
            &format!("{CARD}/root.js"),
            &super::card_entry(&ticking_root()),
        )
        .serving_style_sheet(&format!("{CARD}/style-info.css"), "text { color: red }")
        .with_style_sheet_probe();
    let mut engine = with_bts(spec, CARD, "").create(Arc::new(NoWakeup));
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
/// [`data_root`], a [`bts_boot`] and no page CSS.
fn serving_data_cards(entry: &str) -> TestViewSpec {
    let spec = TestViewSpec::new(entry)
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
        .with_style_sheet_probe();
    with_bts(with_bts(spec, CARD, ""), SECOND, "")
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

/// A frame's card has a background thread of its own, as in native and in
/// web-core: a `Worker` at `bobcat:bts` its child realm creates and connects
/// to the card's BTS boot. It starts with the frame's `data` and
/// `global-props`; a later write reaches it as `updateCardData` and
/// `updateGlobalProps` — what re-renders a compiled card once its first
/// screen is out — after the card's main thread took it; and it shares the
/// group's `sharedData` with the page's BTS, as native's nested view joins
/// its parent's group.
#[test]
fn a_frames_card_runs_a_background_thread_that_takes_its_data() {
    let before = "__SetAttribute(frame, 'data', {label: 'first'}); \
                  __SetAttribute(frame, 'global-props', {message: 'hello'});";
    let mut engine = serving_data_cards(&host_page_with(CARD, true, before))
        .with_background_entry(
            "import { lynx } from 'bobcat:bts-runtime'; \
             lynx.setSharedData('from', 'page'); console.log('page bts set');",
        )
        .create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    let text = pump_until_text(&mut engine, &mut events, "first/hello", None);
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {CARD} booted first/hello"),
    );
    pump_until_logged(&mut engine, &mut events, "page bts set");

    write_frame(&mut engine, "data", r#"{"label":"second"}"#);
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {CARD} data second shared page"),
    );
    assert_eq!(
        pump_until_text(&mut engine, &mut events, "second/hello", None),
        text,
        "the card's main thread took it too"
    );
    write_frame(&mut engine, "globalProps", r#"{"message":"there"}"#);
    pump_until_logged(&mut engine, &mut events, &format!("bts {CARD} props there"));
    pump_until_text(&mut engine, &mut events, "second/there", None);
    assert_eq!(
        logged_times(&events, &format!("bts {CARD} booted first/hello")),
        1,
        "one background thread, which the updates reach"
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// The card's background thread gets the view's native modules, as native's
/// nested view inherits its parent's: `bobcat:frame` hands the page realm's
/// module table to the card, which posts it in its BTS's `initialize`, and
/// the answer comes back to the frame's BTS through the inbox the view
/// registered for the worker the child created.
#[test]
fn a_frames_card_background_thread_calls_the_views_native_modules() {
    let spec = TestViewSpec::new(&host_entry(CARD))
        .with_fetchable(CARD)
        .with_module(&format!("{CARD}/root.js"), &super::card_entry(&root(CARD)))
        .with_style_sheet_probe()
        .with_echo_module("Echo");
    let mut engine = with_bts(
        spec,
        CARD,
        "NativeModules.Echo.echo('from-frame', (...args) => log('echoed ' + JSON.stringify(args)));",
    )
    .create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!(r#"bts {CARD} echoed ["from-frame",null]"#),
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// Letting go of a frame's card disposes of its background thread before its
/// realm goes, as both references do: the card's BTS runs its app cleanup
/// (`callDestroyLifetimeFun`, `ReactLynx`'s `destroyCard`) and acknowledges,
/// the child terminates it, and only then closes. The child is kept the
/// while: here the BTS's cleanup holds the worker thread for a while, and
/// the card's main thread goes on ticking after the cleanup began. Then
/// both threads stop, and a reconnection boots the card with a new BTS.
#[test]
fn a_frames_card_background_thread_is_disposed_before_its_realm_goes() {
    let spec = TestViewSpec::new(&host_entry(CARD))
        .with_fetchable(CARD)
        .with_module(
            &format!("{CARD}/root.js"),
            &super::card_entry(&ticking_root()),
        )
        .with_style_sheet_probe();
    let bts_tick = format!("bts {CARD} tick");
    let mut engine = with_bts(
        spec,
        CARD,
        "setInterval(() => log('tick'), 5); \
         app.callDestroyLifetimeFun = () => { \
             log('destroyed'); \
             const until = Date.now() + 200; while (Date.now() < until); \
         };",
    )
    .create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    pump_until_logged(&mut engine, &mut events, &bts_tick);
    pump_until_ticking(&mut engine, "the card's main thread runs");

    operate(&mut engine, "detach");
    let destroyed = format!("bts {CARD} destroyed");
    pump_until_logged(&mut engine, &mut events, &destroyed);
    let after = pump_for(&mut engine, Duration::from_millis(400));
    assert!(
        ticks(&after) > 0,
        "the card's realm lived on while its BTS disposed: {after:?}"
    );
    assert_no_more_ticks(
        &mut engine,
        "the card's realm closed after its BTS answered",
    );
    let quiet = pump_for(&mut engine, Duration::from_millis(100));
    assert_eq!(
        logged_times(&quiet, &bts_tick),
        0,
        "the card's BTS was terminated: {quiet:?}"
    );
    events.extend(after);

    operate(&mut engine, "attach");
    let booted = format!("bts {CARD} booted undefined/undefined");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut again = Vec::new();
    while logged_times(&again, &booted) == 0 || ticks(&again) == 0 {
        again.extend(pump_for(&mut engine, Duration::from_millis(5)));
        assert!(Instant::now() < deadline, "no new realm and BTS: {again:?}");
    }
    events.extend(again);
    assert_eq!(logged_times(&events, &booted), 2);
    assert_eq!(logged_times(&events, &destroyed), 1);
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A third card, which [`EVENT_CARD`] shows in a frame of its own.
const GRAND: &str = "app:///grand.web.bundle";

/// The worklet runtime, and what every events card files: `log(label)`, a
/// worklet, and `listen(label)`, a listener, each logging
/// `<label> <type> <target id>`, then the `detail`'s `x,y` when it has
/// them. Both, because an element holds one worklet per event name, whatever
/// its form.
const LOGGING_WORKLETS: &str = "
    globalThis.runWorklet = (value, params) => value.body(params[0]);
    const listen = (label) => (e) => console.log(
        label + ' ' + e.type + ' ' + e.target.id +
        (e.detail && e.detail.x !== undefined ? ' ' + e.detail.x + ',' + e.detail.y : ''));
    const log = (label) => ({ type: 'worklet', value: { body: listen(label) } });
";

/// The host page for the event tests: a `wrapper` padded 20px left and 30px
/// top holds a 200×200 `frame` showing `src`, so the frame's
/// border box starts at (20, 30). The page captures `tap`, the wrapper binds
/// it, the page binds it globally and binds `keydown`; it captures
/// `uiappear` too. `updatePage`'s `op` changes them: `catch` files a
/// `capture-catch` on the wrapper in place of its bind, `src` writes
/// [`SECOND`] to the frame's `src`, `detach` removes the frame, and
/// `remove-on-tap` and `src-on-tap` add a capturing listener on the page
/// that does either of those inside the next tap; each logs `page-op <op>`
/// once done.
fn event_host(src: &str) -> String {
    format!(
        r"
        {LOGGING_WORKLETS}
        const page = __CreatePage('0', 0);
        __SetID(page, 'page');
        const wrapper = __CreateView(0);
        __SetID(wrapper, 'wrapper');
        __SetInlineStyles(wrapper, 'padding-left:20px;padding-top:30px;width:300px;height:300px');
        const frame = __CreateFrame(0);
        __SetID(frame, 'frame');
        __SetInlineStyles(frame, 'width:200px;height:200px');
        __AppendElement(wrapper, frame);
        __AppendElement(page, wrapper);
        __AddEvent(page, 'capture-bind', 'tap', log('page-capture'));
        __AddEvent(wrapper, 'bindEvent', 'tap', log('wrapper'));
        __AddEvent(page, 'global-bindEvent', 'tap', log('page-global'));
        __AddEvent(page, 'bindEvent', 'keydown', log('page-key'));
        __AddEvent(page, 'capture-bind', 'uiappear', log('page-appear'));
        __SetAttribute(frame, 'src', '{src}');
        __FlushElementTree();
        globalThis.updatePage = (data) => {{
            if (data.op === 'catch') {{
                __AddEvent(wrapper, 'capture-catch', 'tap', log('wrapper-catch'));
            }} else if (data.op === 'src') {{
                __SetAttribute(frame, 'src', '{SECOND}');
            }} else if (data.op === 'detach') {{
                __RemoveElement(wrapper, frame);
            }} else if (data.op === 'remove-on-tap') {{
                __AddEventListener(page, 'tap', () => __RemoveElement(wrapper, frame), {{ capture: true }});
            }} else if (data.op === 'src-on-tap') {{
                __AddEventListener(page, 'tap', () => __SetAttribute(frame, 'src', '{SECOND}'), {{ capture: true }});
            }}
            __FlushElementTree();
            console.log('page-op ' + data.op);
        }};
    "
    )
}

/// The card [`event_host`]'s frame shows: a column of three 200×60 boxes —
/// `cv`, a view; `swiper`, an `x-swiper` with no items, so its own shadow
/// content is what a tap there hits; `inner`, a frame showing [`GRAND`].
/// `cv` catches `tap` with a worklet and binds it with a background-thread
/// handler, `card-handler`; the card's page captures `tap` with a listener,
/// binds it with a worklet and binds it globally; the swiper binds it; `cv` binds `keydown` and
/// binds it globally, and listens for its own `uiappear`.
fn event_card() -> String {
    format!(
        r"
        {LOGGING_WORKLETS}
        globalThis.renderPage = () => {{
            const p = __CreatePage('0', 0);
            __SetID(p, 'card-page');
            __SetInlineStyles(p, 'display:flex;flex-direction:column');
            const box = (make, id) => {{
                const element = make();
                __SetID(element, id);
                __SetInlineStyles(element, 'flex-shrink:0;width:200px;height:60px');
                __AppendElement(p, element);
                return element;
            }};
            const cv = box(() => __CreateView(0), 'cv');
            const swiper = box(() => __CreateElement('x-swiper', 0), 'swiper');
            const inner = box(() => __CreateFrame(0), 'inner');
            __AddEventListener(p, 'tap', listen('card-capture'), {{ capture: true }});
            __AddEvent(p, 'bindEvent', 'tap', log('card-page'));
            __AddEvent(p, 'global-bindEvent', 'tap', log('card-global'));
            __AddEvent(cv, 'catchEvent', 'tap', log('card-view'));
            __AddEvent(cv, 'bindEvent', 'tap', 'card-handler');
            __AddEvent(swiper, 'bindEvent', 'tap', log('card-swiper'));
            __AddEvent(cv, 'global-bindEvent', 'keydown', log('card-key-global'));
            __AddEvent(cv, 'bindEvent', 'keydown', log('card-key-bind'));
            __AddEventListener(cv, 'uiappear', (e) => console.log('card-appear ' + e.target.id), {{}});
            __SetAttribute(inner, 'src', '{GRAND}');
        }};
    "
    )
}

/// The card [`event_card`]'s inner frame shows: one 200×60 `gv` the page
/// captures `tap` over and `gv` binds, and which listens for its own
/// `uiappear`.
fn grand_card() -> String {
    format!(
        r"
        {LOGGING_WORKLETS}
        globalThis.renderPage = () => {{
            const p = __CreatePage('0', 0);
            __SetID(p, 'grand-page');
            const gv = __CreateView(0);
            __SetID(gv, 'gv');
            __SetInlineStyles(gv, 'width:200px;height:60px');
            __AppendElement(p, gv);
            __AddEvent(p, 'capture-bind', 'tap', log('grand-capture'));
            __AddEvent(gv, 'bindEvent', 'tap', log('grand-view'));
            __AddEventListener(gv, 'uiappear', (e) => console.log('grand-appear ' + e.target.id), {{}});
        }};
    "
    )
}

/// The card [`event_host`]'s `src` op writes: one 200×60 `sv` that binds
/// `tap`.
fn second_event_card() -> String {
    format!(
        r"
        {LOGGING_WORKLETS}
        globalThis.renderPage = () => {{
            const p = __CreatePage('0', 0);
            const sv = __CreateView(0);
            __SetID(sv, 'sv');
            __SetInlineStyles(sv, 'width:200px;height:60px');
            __AppendElement(p, sv);
            __AddEvent(sv, 'bindEvent', 'tap', log('second-view'));
        }};
    "
    )
}

/// The card [`event_view`]'s host shows.
const EVENT_CARD: &str = CARD;

/// A view running [`event_host`], serving [`EVENT_CARD`], [`GRAND`] and
/// [`SECOND`], each with a BTS boot; [`EVENT_CARD`]'s logs every event
/// published to it as `bts <card> published <handler> <target id> <x,y>`.
fn event_view() -> TestEngine {
    let spec = TestViewSpec::new(&event_host(EVENT_CARD))
        .with_fetchable(EVENT_CARD)
        .with_module(
            &format!("{EVENT_CARD}/root.js"),
            &super::card_entry(&event_card()),
        )
        .with_fetchable(GRAND)
        .with_module(
            &format!("{GRAND}/root.js"),
            &super::card_entry(&grand_card()),
        )
        .with_fetchable(SECOND)
        .with_module(
            &format!("{SECOND}/root.js"),
            &super::card_entry(&second_event_card()),
        )
        .with_style_sheet_probe();
    let spec = with_bts(
        spec,
        EVENT_CARD,
        "app.publishEvent = (name, event) => log('published ' + name + ' ' + \
         event.target.id + ' ' + event.detail.x + ',' + event.detail.y);",
    );
    with_bts(with_bts(spec, GRAND, ""), SECOND, "").create(Arc::new(NoWakeup))
}

/// What the published frame hits at `(x, y)`: the hit node's `id`, and the
/// `id` of the shadow host of the tree it is in. Asked briefly, as
/// [`pump_until`] asks: the document's thread may be parked on a source the
/// next turn of this one answers.
fn hit_ids(engine: &mut TestEngine, x: f32, y: f32) -> Option<(Option<String>, Option<String>)> {
    let frame = engine.published_frame()?;
    let node = frame.hit(dom::Point2D::new(x, y), &|_| None, None)?.node;
    engine
        .view
        .probe_document_within(Duration::from_millis(20), move |tree| {
            let id = |node| {
                tree.get(node)
                    .and_then(|live| live.attribute("id"))
                    .map(str::to_owned)
            };
            let host = tree
                .shadow_root_of(node)
                .and_then(|root| tree.shadow_host(root));
            (id(node), host.and_then(id))
        })
}

/// Pumps until the published frame hits what `wanted` accepts at `(x, y)`,
/// collecting every event.
fn pump_until_hit(
    engine: &mut TestEngine,
    events: &mut Vec<EngineEvent>,
    (x, y): (f32, f32),
    wanted: impl Fn(&(Option<String>, Option<String>)) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in engine.pump() {
            assert!(!event.is_fatal(), "the view ended: {event:?}");
            events.push(event);
        }
        let hit = hit_ids(engine, x, y);
        if hit.as_ref().is_some_and(&wanted) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never hit what was wanted at ({x}, {y}), last {hit:?}: {events:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A tap at `(x, y)`: a touch down and its release in place.
fn tap(engine: &mut TestEngine, (x, y): (f32, f32)) {
    for phase in [dom::input::PointerPhase::Down, dom::input::PointerPhase::Up] {
        engine.dispatch_input(dom::input::InputEvent::pointer(
            dom::Point2D::new(x, y),
            1,
            dom::input::PointerKind::Touch,
            phase,
        ));
    }
}

/// The console lines among `events` that a `tap` or a `keydown` handler of
/// the event tests logged, in order: none of the `uiappear` lines, whose
/// delivery entry may land anywhere among them, nor the BTS's.
fn event_lines(events: &[EngineEvent]) -> Vec<&str> {
    const LABELS: [&str; 13] = [
        "page-capture ",
        "page-global ",
        "page-key ",
        "wrapper",
        "card-capture ",
        "card-page ",
        "card-global ",
        "card-view ",
        "card-swiper ",
        "card-key",
        "grand-capture ",
        "grand-view ",
        "second-view ",
    ];
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ConsoleMessage { message, .. }
                if LABELS.iter().any(|label| message.starts_with(label)) =>
            {
                Some(message.as_str())
            }
            _ => None,
        })
        .collect()
}

/// Pumps until `last` is logged, then a little longer for anything that
/// should not come, and answers the event lines logged meanwhile; nothing
/// may be reported.
fn lines_until(engine: &mut TestEngine, last: &str) -> Vec<String> {
    let mut events = Vec::new();
    pump_until_logged(engine, &mut events, last);
    events.extend(pump_for(engine, Duration::from_millis(60)));
    assert_eq!(reported(&events), Vec::<&str>::new());
    event_lines(&events)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// Runs one of [`event_host`]'s ops and waits until it ran.
fn event_op(engine: &mut TestEngine, op: &str) {
    operate(engine, op);
    let mut events = Vec::new();
    pump_until_logged(engine, &mut events, &format!("page-op {op}"));
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// Boots [`event_view`] and waits until its card and grand-card are hit
/// where they are drawn.
fn booted_event_view() -> TestEngine {
    let mut engine = event_view();
    let mut events = Vec::new();
    pump_until_hit(&mut engine, &mut events, CV, |hit| {
        hit.0.as_deref() == Some("cv")
    });
    pump_until_hit(&mut engine, &mut events, GV, |hit| {
        hit.0.as_deref() == Some("gv")
    });
    assert_eq!(reported(&events), Vec::<&str>::new());
    engine
}

/// Inside `cv`: the frame's border box starts at (20, 30), so (100, 30) in
/// the card.
const CV: (f32, f32) = (120.0, 60.0);
/// Inside the swiper's own shadow content, its `#content`: (100, 90) in the
/// card.
const SWIPER: (f32, f32) = (120.0, 120.0);
/// Inside the grand-card's `gv`: the inner frame starts at (20, 150) in the
/// viewport, so (100, 30) in the grand-card and (100, 150) in the card.
const GV: (f32, f32) = (120.0, 180.0);
/// Inside the wrapper's padding, outside the frame.
const PADDING: (f32, f32) = (5.0, 5.0);

/// A tap inside a frame's card runs the page's handlers first, at the
/// frame — `target` the frame, retargeted, and `detail` in the viewport —
/// then the card's own, at its node, with `detail` relative to the frame's
/// border box; the card's background-thread handler is published to the
/// card's own BTS. The kernel calls the page realm and then the card's
/// realm, each once at the path's top, and each walks the steps of the path
/// it holds handles for. Each card's `catch` ends its own
/// walk alone: `cv`'s `catchtap` stops the card's page's bind and leaves the
/// host's wrapper's alone (web-core's semantics, user ruling).
#[test]
fn a_tap_in_a_frames_card_runs_the_pages_handlers_then_the_cards() {
    let mut engine = booted_event_view();
    tap(&mut engine, CV);
    let mut events = Vec::new();
    pump_until_logged(&mut engine, &mut events, "card-global tap cv 100,30");
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {EVENT_CARD} published card-handler cv 100,30"),
    );
    events.extend(pump_for(&mut engine, Duration::from_millis(60)));
    assert_eq!(
        event_lines(&events),
        [
            "page-capture tap frame 120,60",
            "wrapper tap frame 120,60",
            "page-global tap frame 120,60",
            "card-capture tap cv 100,30",
            "card-view tap cv 100,30",
            "card-global tap cv 100,30",
        ]
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// The other direction: the page's `capture-catch` on the wrapper ends the
/// page's walk — its frame and the wrapper's bubble hear nothing — and the
/// card's whole dispatch still runs, as each web-core card walks its own.
#[test]
fn a_pages_catch_does_not_stop_the_frames_card() {
    let mut engine = booted_event_view();
    event_op(&mut engine, "catch");
    tap(&mut engine, CV);
    let lines = lines_until(&mut engine, "card-global tap cv 100,30");
    assert_eq!(
        lines,
        [
            "page-capture tap frame 120,60",
            "wrapper-catch tap frame 120,60",
            "page-global tap frame 120,60",
            "card-capture tap cv 100,30",
            "card-view tap cv 100,30",
            "card-global tap cv 100,30",
        ]
    );
}

/// A card's `global-bindtap` hears the taps inside its frame, and no
/// other: a tap in the wrapper's padding is the page's alone. The card's
/// registration is reached only by a path through the frame's shadow root,
/// its tree's root, as web-core's card listens on its own shadow root.
#[test]
fn a_cards_global_bind_hears_only_taps_inside_its_frame() {
    let mut engine = booted_event_view();
    let mut events = Vec::new();
    pump_until_hit(&mut engine, &mut events, PADDING, |hit| {
        hit.0.as_deref() == Some("wrapper")
    });
    tap(&mut engine, PADDING);
    let lines = lines_until(&mut engine, "page-global tap wrapper 5,5");
    assert_eq!(
        lines,
        [
            "page-capture tap wrapper 5,5",
            "wrapper tap wrapper 5,5",
            "page-global tap wrapper 5,5",
        ]
    );
}

/// A non-composed event at a card's node — `cv`'s own `uiappear` — stays in
/// the card's tree: the card hears it, and the page, which captures
/// `uiappear`, hears only its own. So does one in the grand-card, whose
/// frame is no step of the path.
#[test]
fn a_non_composed_event_at_a_cards_node_reaches_only_the_card() {
    let mut engine = event_view();
    let mut events = Vec::new();
    pump_until_logged(&mut engine, &mut events, "card-appear cv");
    pump_until_logged(&mut engine, &mut events, "grand-appear gv");
    events.extend(pump_for(&mut engine, Duration::from_millis(100)));
    let appeared: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ConsoleMessage { message, .. } if message.contains("appear") => {
                Some(message.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(
        appeared.iter().all(|line| [
            "card-appear cv",
            "grand-appear gv",
            "page-appear uiappear page"
        ]
        .contains(line)),
        "{appeared:?}"
    );
    assert_eq!(logged_times(&events, "card-appear cv"), 1);
    assert_eq!(logged_times(&events, "grand-appear gv"), 1);
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A UA component inside a card stays what it is to script: a tap on its
/// own shadow content reaches the card at the component, retargeted, and
/// nothing names an inner node. The page hears it at the frame.
#[test]
fn a_ua_component_inside_a_card_is_retargeted_to_its_host() {
    let mut engine = booted_event_view();
    let mut events = Vec::new();
    pump_until_hit(&mut engine, &mut events, SWIPER, |hit| {
        hit.1.as_deref() == Some("swiper")
    });
    tap(&mut engine, SWIPER);
    let lines = lines_until(&mut engine, "card-global tap swiper 100,90");
    assert_eq!(
        lines,
        [
            "page-capture tap frame 120,120",
            "wrapper tap frame 120,120",
            "page-global tap frame 120,120",
            "card-capture tap swiper 100,90",
            "card-swiper tap swiper 100,90",
            "card-page tap swiper 100,90",
            "card-global tap swiper 100,90",
        ]
    );
}

/// A frame inside a frame's card: the page, then the card at its inner
/// frame, then the grand-card at its own node, each `detail` relative to
/// that realm's own frame.
#[test]
fn a_grand_cards_steps_follow_the_cards() {
    let mut engine = booted_event_view();
    tap(&mut engine, GV);
    let lines = lines_until(&mut engine, "grand-view tap gv 100,30");
    assert_eq!(
        lines,
        [
            "page-capture tap frame 120,180",
            "wrapper tap frame 120,180",
            "page-global tap frame 120,180",
            "card-capture tap inner 100,150",
            "card-page tap inner 100,150",
            "card-global tap inner 100,150",
            "grand-capture tap gv 100,30",
            "grand-view tap gv 100,30",
        ]
    );
}

/// A key reaches every card's `global-bindkeydown`, with no path in the
/// card — its `bindkeydown` hears nothing, and its `target` is the element
/// that bound it — after the page, which hears it at its page as before.
#[test]
fn a_key_reaches_a_cards_global_bind() {
    let mut engine = booted_event_view();
    engine.dispatch_input(dom::input::InputEvent::key(
        dom::input::KeyPhase::Down,
        "a",
        "KeyA",
        false,
        dom::input::Modifiers::default(),
    ));
    let lines = lines_until(&mut engine, "card-key-global keydown cv");
    assert_eq!(
        lines,
        ["page-key keydown page", "card-key-global keydown cv"]
    );
}

/// A frame showing no card — here one whose `src` does not fetch, so no
/// realm is made for it — has no realm of its own to call: a tap
/// inside it is the page's alone, at the frame, and nothing but the one
/// fetch diagnostic is reported.
#[test]
fn a_frame_showing_no_card_leaves_its_taps_to_the_page() {
    let missing = "app:///missing.web.bundle";
    let mut engine = TestViewSpec::new(&event_host(missing)).create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    pump_until_hit(&mut engine, &mut events, CV, |hit| {
        hit.0.as_deref() == Some("frame") || hit.1.as_deref() == Some("frame")
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while reported(&events).is_empty() {
        for event in engine.pump() {
            assert!(!event.is_fatal(), "the view ended: {event:?}");
            events.push(event);
        }
        assert!(Instant::now() < deadline, "no diagnostic: {events:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
    let messages = reported(&events);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains(&format!("frame {missing}")),
        "{messages:?}"
    );

    tap(&mut engine, CV);
    let lines = lines_until(&mut engine, "page-global tap frame 120,60");
    assert_eq!(
        lines,
        [
            "page-capture tap frame 120,60",
            "wrapper tap frame 120,60",
            "page-global tap frame 120,60",
        ]
    );
}

/// A new `src` hands the frame's events to the new card; a disconnection
/// to nobody, the page alone hearing what lands where the frame was.
///
/// The tap does not wait for the first card's `closeRealm`: the new `src`'s
/// `resetFrame` freed that card's page, and its rows went with its nodes, so
/// only the new card is called, whether or not the first card's background
/// thread has acknowledged its disposal yet.
#[test]
fn a_frames_events_follow_its_card() {
    let mut engine = booted_event_view();
    let mut events = Vec::new();
    operate(&mut engine, "src");
    pump_until_logged(&mut engine, &mut events, "page-op src");
    pump_until_hit(&mut engine, &mut events, CV, |hit| {
        hit.0.as_deref() == Some("sv")
    });
    assert_eq!(reported(&events), Vec::<&str>::new());
    tap(&mut engine, CV);
    let lines = lines_until(&mut engine, "second-view tap sv 100,30");
    assert_eq!(
        lines,
        [
            "page-capture tap frame 120,60",
            "wrapper tap frame 120,60",
            "page-global tap frame 120,60",
            "second-view tap sv 100,30",
        ]
    );

    event_op(&mut engine, "detach");
    pump_until_hit(&mut engine, &mut events, CV, |hit| {
        hit.0.as_deref() == Some("wrapper")
    });
    tap(&mut engine, CV);
    let lines = lines_until(&mut engine, "page-global tap wrapper 120,60");
    assert_eq!(
        lines,
        [
            "page-capture tap wrapper 120,60",
            "wrapper tap wrapper 120,60",
            "page-global tap wrapper 120,60",
        ]
    );
}

/// A page listener that takes the frame out of the tree inside the tap: the
/// card is still called, after the page's walk — the
/// behavior #428 did not have, where the realm of the frame found the frame
/// out of its page tree and handed the card nothing. Here the realms a
/// dispatch calls are fixed before its first step, and the card's realm
/// keeps its registrations until its `closeRealm` runs, which the
/// frame's disconnection starts in a later entry (design
/// `event-dispatch-root-registration-design.md` §4.5; deviations.md, the
/// realms of a dispatch are fixed before its walk). The frame has left the
/// layout by then, so the card's `boundingClientRect` readback answers no
/// box and its `detail` falls back to the viewport's origin (`pageOrigin`).
#[test]
fn a_frame_an_earlier_listener_removed_still_has_its_card_called() {
    let mut engine = booted_event_view();
    event_op(&mut engine, "remove-on-tap");
    tap(&mut engine, CV);
    let lines = lines_until(&mut engine, "card-global tap cv 120,60");
    assert_eq!(
        lines,
        [
            "page-capture tap frame 120,60",
            "wrapper tap frame 120,60",
            "page-global tap frame 120,60",
            "card-capture tap cv 120,60",
            "card-view tap cv 120,60",
            "card-global tap cv 120,60",
        ]
    );
}

/// A page listener that gives the frame a new `src` inside the tap: the
/// page's walk goes on — the wrapper's bind and the page's global after the
/// listener still run — the first card hears nothing, and nothing is
/// reported, as #428 had it. The new `src` runs the first card's `teardown`
/// inside the listener, and its `resetFrame` frees that card's page, whose
/// nodes take the card's rows with them, so the card's call later at the
/// same step finds it no registration for `tap` and is skipped, though its
/// realm stays until its background thread acknowledges the disposal.
#[test]
fn a_frame_an_earlier_listener_gave_a_new_src_mid_dispatch() {
    let mut engine = booted_event_view();
    event_op(&mut engine, "src-on-tap");
    tap(&mut engine, CV);
    let lines = lines_until(&mut engine, "page-global tap frame 120,60");
    assert_eq!(
        lines,
        [
            "page-capture tap frame 120,60",
            "wrapper tap frame 120,60",
            "page-global tap frame 120,60",
        ],
    );
}

// --- exposure and the nested lifecycle -----------------------------------
//
// Each exposure record is a `bobcat:exposure` event at its element and each
// frame connection a `bobcat:frameconnected`/`bobcat:framedisconnected`
// event at its frame; the listener table calls the realm that holds the
// handle. Each card's BTS so hears its own card's records alone, and a frame
// a card created is that card's to load and let go of.

/// The page realm's BTS for the exposure tests: it logs each `exposure` and
/// `disexposure` list it hears as `page-bts <list> <ids>`.
const EXPOSURE_PAGE_BTS: &str = "
import { lynx } from 'bobcat:bts-runtime';
const emitter = lynx.getJSModule('GlobalEventEmitter');
for (const name of ['exposure', 'disexposure']) {
  emitter.addListener(name, (list) => console.log(
    'page-bts ' + name + ' ' + list.map((record) => record['exposure-id']).join(',')));
}
";

/// What every exposure card's BTS adds to [`bts_boot`]: each `exposure` and
/// `disexposure` list it hears, logged as `bts <card> <list> <id>:<typeof
/// dataset.rowIndex>,…` — a typed value under its own camelCase key reads
/// `number` — and an `updateCardData` whose `op` is `stop` running the card's
/// own `lynx.stopExposure()`.
const EXPOSURE_CARD_BTS: &str = "
const emitter = lynx.getJSModule('GlobalEventEmitter');
for (const name of ['exposure', 'disexposure']) {
  emitter.addListener(name, (list) => log(name + ' ' + list.map((record) =>
    record['exposure-id'] + ':' + typeof record.dataset.rowIndex).join(',')));
}
app.updateCardData = (data) => {
  if (data.op === 'stop') {
    lynx.stopExposure();
    log('stopped');
  }
};
";

/// The host page of the exposure tests: a 50×50 `page-own` with an
/// `exposure-id`, then a 200×300 frame showing [`CARD`]. `updatePage`'s `op`:
/// `src` writes [`SECOND`] to the frame's `src`, `src-back` [`CARD`] again,
/// each also logging `reset-at-once <whether the frame's nested page was
/// replaced inside the write>`; `card` writes `{op: data.card}` to the
/// frame's `data`, which the card's `updatePage` and its BTS take; `late`
/// appends a second exposed element, `page-late`, and has the card add one
/// of its own in the same update; `detach` takes the frame out of the page,
/// keeping its handle, and `attach` puts it back. Each logs `page-op <op>`
/// once done.
fn exposure_host() -> String {
    format!(
        r"
        import {{ framePage }} from 'bobcat-internal:host';
        const page = __CreatePage('0', 0);
        __SetInlineStyles(page, 'display:flex;flex-direction:column');
        const exposed = (id) => {{
            const view = __CreateView(0);
            __SetInlineStyles(view, 'flex-shrink:0;width:50px;height:50px');
            __SetAttribute(view, 'exposure-id', id);
            __AppendElement(page, view);
        }};
        exposed('page-own');
        const frame = __CreateFrame(0);
        __SetInlineStyles(frame, 'flex-shrink:0;width:200px;height:300px');
        __AppendElement(page, frame);
        __SetAttribute(frame, 'src', '{CARD}');
        __FlushElementTree();
        const writeSrc = (src) => {{
            const before = framePage(__GetElementUniqueID(frame));
            __SetAttribute(frame, 'src', src);
            const after = framePage(__GetElementUniqueID(frame));
            console.log('reset-at-once ' + (before !== after));
        }};
        globalThis.updatePage = (data) => {{
            if (data.op === 'src') {{
                writeSrc('{SECOND}');
            }} else if (data.op === 'src-back') {{
                writeSrc('{CARD}');
            }} else if (data.op === 'card') {{
                __SetAttribute(frame, 'data', {{ op: data.card }});
            }} else if (data.op === 'late') {{
                exposed('page-late');
                __SetAttribute(frame, 'data', {{ op: 'add' }});
            }} else if (data.op === 'detach') {{
                __RemoveElement(page, frame);
            }} else if (data.op === 'attach') {{
                __AppendElement(page, frame);
            }}
            __FlushElementTree();
            console.log('page-op ' + (data.card ?? data.op));
        }};
    "
    )
}

/// [`CARD`] for the exposure tests: a column of a 100×50 `card-cv`, exposed,
/// with a typed `rowIndex` in its dataset, and a 200×100 frame `inner`
/// showing [`GRAND`], which this card creates and so holds. Its `updatePage`
/// takes the `op` the host writes to its frame's `data`: `remove-cv` takes
/// `card-cv` out of its tree, `detach-inner` the inner frame, `attach-inner`
/// puts the inner frame back, `move-inner` does both in one call, `add`
/// appends a `card-late` with an `exposure-id`; each keeps its handles. Every `op`, these and the
/// ones only its BTS acts on, logs `card-op <op>` once done.
fn exposure_card() -> String {
    format!(
        r"
        let p, cv, inner;
        globalThis.renderPage = () => {{
            p = __CreatePage('0', 0);
            __SetInlineStyles(p, 'display:flex;flex-direction:column');
            cv = __CreateView(0);
            __SetInlineStyles(cv, 'flex-shrink:0;width:100px;height:50px');
            __AddDataset(cv, 'rowIndex', 5);
            __SetAttribute(cv, 'exposure-id', 'card-cv');
            __AppendElement(p, cv);
            inner = __CreateFrame(0);
            __SetInlineStyles(inner, 'flex-shrink:0;width:200px;height:100px');
            __AppendElement(p, inner);
            __SetAttribute(inner, 'src', '{GRAND}');
        }};
        globalThis.updatePage = (data) => {{
            if (data.op === 'remove-cv') {{
                __RemoveElement(p, cv);
            }} else if (data.op === 'detach-inner') {{
                __RemoveElement(p, inner);
            }} else if (data.op === 'attach-inner') {{
                __AppendElement(p, inner);
            }} else if (data.op === 'move-inner') {{
                __RemoveElement(p, inner);
                __AppendElement(p, inner);
            }} else if (data.op === 'add') {{
                const late = __CreateView(0);
                __SetInlineStyles(late, 'flex-shrink:0;width:100px;height:50px');
                __SetAttribute(late, 'exposure-id', 'card-late');
                __AppendElement(p, late);
            }}
            __FlushElementTree();
            console.log('card-op ' + data.op);
        }};
    "
    )
}

/// [`GRAND`] for the exposure tests: one 100×50 `grand-gv`, exposed, with a
/// typed `rowIndex`.
fn exposure_grand_card() -> String {
    r"
        globalThis.renderPage = () => {
            const p = __CreatePage('0', 0);
            const gv = __CreateView(0);
            __SetInlineStyles(gv, 'width:100px;height:50px');
            __AddDataset(gv, 'rowIndex', 7);
            __SetAttribute(gv, 'exposure-id', 'grand-gv');
            __AppendElement(p, gv);
        };
    "
    .to_owned()
}

/// [`SECOND`] for the exposure tests: one 100×50 `second-sv`, exposed, with
/// no dataset.
fn exposure_second_card() -> String {
    r"
        globalThis.renderPage = () => {
            const p = __CreatePage('0', 0);
            const sv = __CreateView(0);
            __SetInlineStyles(sv, 'width:100px;height:50px');
            __SetAttribute(sv, 'exposure-id', 'second-sv');
            __AppendElement(p, sv);
        };
    "
    .to_owned()
}

/// A view running [`exposure_host`] with [`EXPOSURE_PAGE_BTS`], serving
/// `card` as [`CARD`]'s root and [`GRAND`] and [`SECOND`] as the exposure
/// cards, each with a BTS boot that logs with [`EXPOSURE_CARD_BTS`]. Its
/// fetcher has a fetch probe, as a caching resource system does, so a card
/// fetched once starts again in a microtask.
fn exposure_view_with(card: &str) -> TestEngine {
    let spec = TestViewSpec::new(&exposure_host())
        .with_background_entry(EXPOSURE_PAGE_BTS)
        .with_fetchable(CARD)
        .with_module(&format!("{CARD}/root.js"), &super::card_entry(card))
        .with_fetchable(GRAND)
        .with_module(
            &format!("{GRAND}/root.js"),
            &super::card_entry(&exposure_grand_card()),
        )
        .with_fetchable(SECOND)
        .with_module(
            &format!("{SECOND}/root.js"),
            &super::card_entry(&exposure_second_card()),
        )
        .with_fetch_probe()
        .with_style_sheet_probe();
    let spec = with_bts(spec, CARD, EXPOSURE_CARD_BTS);
    let spec = with_bts(spec, GRAND, EXPOSURE_CARD_BTS);
    with_bts(spec, SECOND, EXPOSURE_CARD_BTS).create(Arc::new(NoWakeup))
}

/// The console lines among `events` that start with `prefix`, in order.
fn lines_with<'a>(events: &'a [EngineEvent], prefix: &str) -> Vec<&'a str> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ConsoleMessage { message, .. } if message.starts_with(prefix) => {
                Some(message.as_str())
            }
            _ => None,
        })
        .collect()
}

/// Every exposure line among `events`, from any of the BTSs.
fn exposure_lines(events: &[EngineEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ConsoleMessage { message, .. } if message.contains("exposure ") => {
                Some(message.as_str())
            }
            _ => None,
        })
        .collect()
}

/// Boots the exposure view and waits until each of its three BTSs heard its
/// own element's exposure; answers every event so far.
fn booted_exposure_view() -> (TestEngine, Vec<EngineEvent>) {
    let mut engine = exposure_view_with(&exposure_card());
    let mut events = Vec::new();
    pump_until_logged(&mut engine, &mut events, "page-bts exposure page-own");
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {CARD} exposure card-cv:number"),
    );
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {GRAND} exposure grand-gv:number"),
    );
    (engine, events)
}

/// Runs one of [`exposure_host`]'s ops and waits until it ran, collecting
/// every event.
fn exposure_op(engine: &mut TestEngine, events: &mut Vec<EngineEvent>, op: &str) {
    operate(engine, op);
    pump_until_logged(engine, events, &format!("page-op {op}"));
}

/// Runs one of [`exposure_card`]'s ops, through the frame's `data`, and
/// waits until the card ran it.
fn exposure_card_op(engine: &mut TestEngine, events: &mut Vec<EngineEvent>, op: &str) {
    engine
        .view
        .update_data(format!(r#"{{"op":"card","card":"{op}"}}"#), String::new())
        .expect("the view takes an update");
    pump_until_logged(engine, events, &format!("card-op {op}"));
}

/// Pumps until `line` has been logged `times` times, collecting every event.
fn pump_until_logged_times(
    engine: &mut TestEngine,
    events: &mut Vec<EngineEvent>,
    line: &str,
    times: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while logged_times(events, line) < times {
        events.extend(pump_for(engine, Duration::from_millis(5)));
        assert!(
            Instant::now() < deadline,
            "{line:?} not logged {times} times: {events:?}"
        );
    }
}

/// The nested page of the host's frame, the page element's only `frame`,
/// and the tags of its element children.
fn host_nested_page(engine: &mut TestEngine) -> (dom::NodeId, Vec<String>) {
    engine
        .probe_document(|tree| {
            let frame = tree
                .document_element()
                .children()
                .find(|child| child.tag_name() == Some("frame"))
                .map(dom::Node::id)
                .expect("the page holds the frame");
            let page = crate::main::tree::nested_page(tree, frame).expect("a frame");
            let tags = tree
                .get(page)
                .into_iter()
                .flat_map(|page| page.children().filter(|child| child.is_element()))
                .filter_map(|child| child.tag_name().map(str::to_owned))
                .collect();
            (page, tags)
        })
        .expect("the document answers")
}

/// Each BTS hears its own card's elements' records alone — the page's BTS
/// the page's element, the card's BTS the card's, the grand-card's BTS the
/// grand-card's — each built from that realm's own handle, so a typed
/// dataset value stays a number under its own camelCase key. The page realm
/// holds no handle for a card's node and sends its BTS nothing of them.
#[test]
fn each_background_thread_hears_its_own_cards_exposure_alone() {
    let (mut engine, mut events) = booted_exposure_view();
    events.extend(pump_for(&mut engine, Duration::from_millis(100)));
    // Three realms' logs, each ordered only among its own lines.
    let mut lines = exposure_lines(&events);
    lines.sort_unstable();
    let card = format!("bts {CARD} exposure card-cv:number");
    let grand = format!("bts {GRAND} exposure grand-gv:number");
    let mut expected = vec!["page-bts exposure page-own", &card, &grand];
    expected.sort_unstable();
    assert_eq!(lines, expected);
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A card taking one of its own elements out of its tree sends the card's
/// own BTS the `disexposure`, built from the card's handle; the page's BTS
/// hears nothing of it.
#[test]
fn a_card_removing_its_element_disexposes_it_to_its_own_background_thread() {
    let (mut engine, mut events) = booted_exposure_view();
    exposure_card_op(&mut engine, &mut events, "remove-cv");
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {CARD} disexposure card-cv:number"),
    );
    events.extend(pump_for(&mut engine, Duration::from_millis(100)));
    assert_eq!(
        lines_with(&events, "page-bts disexposure"),
        Vec::<&str>::new()
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A card that takes its own frame out of its tree, keeping the handle, lets
/// go of that frame's card, as each web-core nested `lynx-view` hears its
/// own `disconnectedCallback`: the disconnection is an event at the frame,
/// which the card's realm, holding its handle, hears. The grand-card's BTS
/// hears its element's `disexposure` once — dispatched at the detached
/// element, whose path starts at itself — and is then disposed of. Putting
/// the frame back boots the grand-card again, with a BTS of its own that
/// hears its element again.
#[test]
fn a_card_detaching_its_own_frame_lets_go_of_that_frames_card() {
    let (mut engine, mut events) = booted_exposure_view();
    exposure_card_op(&mut engine, &mut events, "detach-inner");
    let destroyed = format!("bts {GRAND} destroyed");
    pump_until_logged(&mut engine, &mut events, &destroyed);
    let grand = lines_with(&events, &format!("bts {GRAND} "));
    let left = format!("bts {GRAND} disexposure grand-gv:number");
    assert_eq!(
        grand
            .iter()
            .filter(|line| **line == left || **line == destroyed)
            .collect::<Vec<_>>(),
        [&left.as_str(), &destroyed.as_str()],
        "{grand:?}"
    );

    exposure_card_op(&mut engine, &mut events, "attach-inner");
    let booted = format!("bts {GRAND} booted undefined/undefined");
    pump_until_logged_times(&mut engine, &mut events, &booted, 2);
    let again = format!("bts {GRAND} exposure grand-gv:number");
    pump_until_logged_times(&mut engine, &mut events, &again, 2);
    events.extend(pump_for(&mut engine, Duration::from_millis(100)));
    assert_eq!(logged_times(&events, &booted), 2);
    assert_eq!(logged_times(&events, &left), 1);
    assert_eq!(logged_times(&events, &destroyed), 1);
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A frame removed from the page, its handle kept, with a card that shows a
/// frame of its own: both cards are let go of and each BTS hears its last
/// `disexposure` list once, before its disposal, dispatched at the detached
/// cards' elements. The outer card's teardown closes the inner frame's
/// card, so the inner frame's own disconnection, which the same removal
/// queued, finds nothing left to let go of. Putting the frame back boots
/// both cards again.
#[test]
fn a_removed_frame_sends_each_cards_last_list_and_lets_go_of_both() {
    let (mut engine, mut events) = booted_exposure_view();
    exposure_op(&mut engine, &mut events, "detach");
    for card in [CARD, GRAND] {
        pump_until_logged(&mut engine, &mut events, &format!("bts {card} destroyed"));
    }
    events.extend(pump_for(&mut engine, Duration::from_millis(100)));
    for (card, id) in [(CARD, "card-cv:number"), (GRAND, "grand-gv:number")] {
        let lines = lines_with(&events, &format!("bts {card} "));
        let last = format!("bts {card} disexposure {id}");
        let destroyed = format!("bts {card} destroyed");
        let at = |line: &str| lines.iter().position(|seen| *seen == line);
        assert!(
            at(&last).is_some() && at(&last) < at(&destroyed),
            "{card}: {lines:?}"
        );
        assert_eq!(logged_times(&events, &last), 1, "{card}: {lines:?}");
        assert_eq!(logged_times(&events, &destroyed), 1, "{card}: {lines:?}");
    }

    exposure_op(&mut engine, &mut events, "attach");
    for (card, id) in [(CARD, "card-cv:number"), (GRAND, "grand-gv:number")] {
        let booted = format!("bts {card} booted undefined/undefined");
        pump_until_logged_times(&mut engine, &mut events, &booted, 2);
        let exposed = format!("bts {card} exposure {id}");
        pump_until_logged_times(&mut engine, &mut events, &exposed, 2);
    }
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A card that takes its own frame out and puts it back in one call
/// re-renders that frame's card, as a web-core `lynx-view` moved in the DOM
/// does: the disconnection lets go of the card, whose last list is sent,
/// and the connection loads it again, which boots once the nested page was
/// reset.
#[test]
fn a_card_moving_its_own_frame_re_renders_that_frames_card() {
    let (mut engine, mut events) = booted_exposure_view();
    exposure_card_op(&mut engine, &mut events, "move-inner");
    let booted = format!("bts {GRAND} booted undefined/undefined");
    pump_until_logged_times(&mut engine, &mut events, &booted, 2);
    let again = format!("bts {GRAND} exposure grand-gv:number");
    pump_until_logged_times(&mut engine, &mut events, &again, 2);
    events.extend(pump_for(&mut engine, Duration::from_millis(100)));
    let lines = lines_with(&events, &format!("bts {GRAND} "));
    let left = format!("bts {GRAND} disexposure grand-gv:number");
    let destroyed = format!("bts {GRAND} destroyed");
    let at = |line: &str| lines.iter().position(|seen| *seen == line);
    let rebooted = lines.iter().rposition(|seen| *seen == booted);
    assert!(
        at(&left).is_some() && at(&left) < at(&destroyed) && at(&left) < rebooted,
        "{lines:?}"
    );
    assert_eq!(logged_times(&events, &left), 1);
    assert_eq!(logged_times(&events, &destroyed), 1);
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A card let go of while its elements are exposed sends its BTS one last
/// `disexposure` list before its BTS is disposed of — native's order, user
/// ruling, option A — and nothing after, its grand-card too: the nested page
/// is not reset inside the `src` write, but once the records were dispatched
/// at the card's still-live elements. Switching back to the first card,
/// which is fetched by then and so starts in a microtask, waits for that
/// reset and boots into a fresh nested page: the new card's BTS hears its
/// own element alone, and no card hears another's.
#[test]
fn a_card_let_go_of_sends_its_last_disexposure_before_its_disposal() {
    let (mut engine, mut events) = booted_exposure_view();
    let (first_page, _) = host_nested_page(&mut engine);
    exposure_op(&mut engine, &mut events, "src");
    assert!(logged(&events, "reset-at-once false"), "{events:?}");
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {SECOND} exposure second-sv:undefined"),
    );
    pump_until_logged(&mut engine, &mut events, &format!("bts {CARD} destroyed"));
    events.extend(pump_for(&mut engine, Duration::from_millis(100)));
    let (second_page, tags) = host_nested_page(&mut engine);
    assert_ne!(
        second_page, first_page,
        "the second card got a page of its own"
    );
    assert_eq!(tags, ["view"], "and rendered into it");

    // Back to the first card, which is fetched by now.
    exposure_op(&mut engine, &mut events, "src-back");
    assert_eq!(logged_times(&events, "reset-at-once false"), 2);
    pump_until_logged(&mut engine, &mut events, &format!("bts {SECOND} destroyed"));
    let again = format!("bts {CARD} exposure card-cv:number");
    pump_until_logged_times(&mut engine, &mut events, &again, 2);
    events.extend(pump_for(&mut engine, Duration::from_millis(200)));
    let (third_page, tags) = host_nested_page(&mut engine);
    assert!(
        third_page != second_page && third_page != first_page,
        "the first card again, in a page of its own"
    );
    // Booted into the page the reset built, not into the second card's,
    // which the reset freed with whatever was rendered into it.
    assert_eq!(tags, ["view", "frame"], "and rendered into it");

    // In each card's BTS's own log: its last list, then its disposal, once.
    for (card, id) in [
        (CARD, "card-cv:number"),
        (GRAND, "grand-gv:number"),
        (SECOND, "second-sv:undefined"),
    ] {
        let lines = lines_with(&events, &format!("bts {card} "));
        let last = format!("bts {card} disexposure {id}");
        let destroyed = format!("bts {card} destroyed");
        let at = |line: &str| lines.iter().position(|seen| *seen == line);
        assert!(
            at(&last).is_some() && at(&last) < at(&destroyed),
            "{card}: {lines:?}"
        );
        assert_eq!(logged_times(&events, &last), 1, "{card}: {lines:?}");
    }
    // No card heard another's element.
    let lines = exposure_lines(&events);
    assert!(
        lines.iter().all(|line| {
            (line.starts_with(&format!("bts {SECOND} ")) && line.contains("second-sv"))
                || (line.starts_with(&format!("bts {CARD} ")) && line.contains("card-cv"))
                || (line.starts_with(&format!("bts {GRAND} ")) && line.contains("grand-gv"))
                || line.starts_with("page-bts ") && line.contains("page-own")
        }),
        "{lines:?}"
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A card with nothing exposed is let go of at once, as before the last
/// list existed: the new `src` frees its nested page inside the write, and
/// its BTS is disposed of.
#[test]
fn a_card_teardown_with_nothing_exposed_proceeds_at_once() {
    let mut engine = exposure_view_with(&root(CARD));
    let mut events = Vec::new();
    pump_until_logged(&mut engine, &mut events, "page-bts exposure page-own");
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {CARD} booted undefined/undefined"),
    );
    exposure_op(&mut engine, &mut events, "src");
    assert!(logged(&events, "reset-at-once true"), "{events:?}");
    pump_until_logged(&mut engine, &mut events, &format!("bts {CARD} destroyed"));
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {SECOND} exposure second-sv:undefined"),
    );
    assert_eq!(
        lines_with(&events, &format!("bts {CARD} disexposure")),
        Vec::<&str>::new()
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}

/// A card's own `lynx.stopExposure()`, from its BTS, stops its own elements
/// alone — the card realm lists the elements it registered: its BTS hears
/// its element's `disexposure`, an element it adds later is stopped as it is
/// registered and so not detected, and the page's exposure goes on — an
/// element the page adds in the same update is exposed to the page's BTS.
#[test]
fn a_cards_stop_exposure_leaves_the_pages_exposure_running() {
    let (mut engine, mut events) = booted_exposure_view();
    exposure_card_op(&mut engine, &mut events, "stop");
    pump_until_logged(&mut engine, &mut events, &format!("bts {CARD} stopped"));
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {CARD} disexposure card-cv:number"),
    );

    exposure_op(&mut engine, &mut events, "late");
    pump_until_logged(&mut engine, &mut events, "page-bts exposure page-late");
    events.extend(pump_for(&mut engine, Duration::from_millis(200)));
    assert_eq!(
        lines_with(&events, &format!("bts {CARD} exposure card-late")),
        Vec::<&str>::new(),
        "the card's own elements are not detected while it is stopped"
    );
    assert_eq!(
        lines_with(&events, "page-bts disexposure"),
        Vec::<&str>::new()
    );
    // The grand-card's elements are its own realm's: still exposed, never
    // disexposed.
    assert_eq!(
        lines_with(&events, &format!("bts {GRAND} disexposure")),
        Vec::<&str>::new()
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}
