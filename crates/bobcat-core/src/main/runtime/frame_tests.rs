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

use terminus::link::stylo::color::AbsoluteColor;

use crate::main::tree::LynxDocument;
use crate::test_support::{TestEngine, TestViewSpec};
use crate::view::{EngineEvent, NoWakeup};

const CARD: &str = "app:///child.web.bundle";

const SECOND: &str = "app:///second.web.bundle";

fn root(card: &str) -> String {
    format!(
        "if (__Card__ !== '{card}') throw Error('__Card__ is ' + __Card__); \
        if (SystemInfo.pixelWidth !== 393) throw Error('SystemInfo ' + JSON.stringify(SystemInfo)); \
        globalThis.renderPage = () => {{ const p = __CreatePage('0', 0); \
        const t = __CreateText(0); __AppendElement(t, __CreateRawText('nested')); \
        __AppendElement(p, t); }};"
    )
}

fn host_entry(src: &str) -> String {
    host_page(src, true)
}

fn host_page(src: &str, attached: bool) -> String {
    host_page_with(src, attached, "")
}

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

fn with_bts(spec: TestViewSpec, card: &str, extra: &str) -> TestViewSpec {
    spec.with_module(&format!("{card}#background-thread"), &bts_boot(card, extra))
}

fn serving_the_card(src: &str) -> TestViewSpec {
    serving_a_root(src, &root(CARD))
}

fn serving_a_root(src: &str, root: &str) -> TestViewSpec {
    serving_a_root_under(&host_entry(src), root)
}

fn serving_a_root_under(entry: &str, root: &str) -> TestViewSpec {
    let spec = TestViewSpec::new(entry)
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

fn write_src(engine: &mut TestEngine, src: &str) {
    engine
        .view
        .update_data(format!(r#"{{"src":{src}}}"#), String::new())
        .expect("the view takes an update");
}

fn operate(engine: &mut TestEngine, op: &str) {
    engine
        .view
        .update_data(format!(r#"{{"op":"{op}"}}"#), String::new())
        .expect("the view takes an update");
}

struct FrameState {
    frame_size: (f32, f32),
    nested: Vec<terminus::link::NodeId>,
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
            .and_then(terminus::link::Node::computed_style)
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

fn logged(events: &[EngineEvent], message: &str) -> bool {
    events.iter().any(
        |event| matches!(event, EngineEvent::ConsoleMessage { message: seen, .. } if seen == message),
    )
}

fn logged_times(events: &[EngineEvent], message: &str) -> usize {
    events
        .iter()
        .filter(
            |event| matches!(event, EngineEvent::ConsoleMessage { message: seen, .. } if seen == message),
        )
        .count()
}

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

#[test]
fn a_frames_src_boots_the_card_inside_it() {
    a_frame_boots(CARD);
}

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

fn load_host(src: &str) -> String {
    host_page_with(
        src,
        true,
        r"
        globalThis.runWorklet = (value, params) => value.body(params[0]);
        const heard = (label) => (event) => console.log('load ' + label + ' ' +
            JSON.stringify(event.detail) + ' ' +
            (event.target.uid === __GetElementUniqueID(frame)) + ' ' + event.eventPhase);
        const worklet = (label) => ({ type: 'worklet', value: { body: heard(label) } });
        __AddEvent(frame, 'bindEvent', 'load', worklet('bind'));
        __AddEventListener(frame, 'load', heard('frame'), {});
        __AddEventListener(page, 'load', heard('page'), {});
        __AddEvent(own, 'global-bindEvent', 'load', worklet('global'));
        ",
    )
}

fn load_root(after: &str) -> String {
    format!(
        "const cardPage = __CreatePage('0', 0); \
        __AddEventListener(cardPage, 'load', (event) => console.log('load card ' + \
            JSON.stringify(event.detail) + ' ' + \
            (event.target.uid === __GetElementUniqueID(cardPage)) + ' ' + event.eventPhase), {{}}); \
        {} {after}",
        root(CARD)
    )
}

fn load_lines(src: &str) -> [String; 5] {
    let detail = format!(r#"{{"statusCode":0,"statusMessage":"success","url":"{src}"}}"#);
    [
        format!("load bind {detail} true 2"),
        format!("load frame {detail} true 2"),
        format!("load page {detail} true 3"),
        format!("load global {detail} true 0"),
        format!("load card {detail} true 2"),
    ]
}

fn load_logged(events: &[EngineEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ConsoleMessage { message, .. } if message.starts_with("load ") => {
                Some(message.as_str())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_frames_card_fires_load_once_its_root_has_loaded() {
    let src = "child.web.bundle";
    let mut engine =
        serving_a_root_under(&load_host(src), &load_root("")).create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    let lines = load_lines(src);
    pump_until_logged(&mut engine, &mut events, &lines[4]);
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
    assert_eq!(load_logged(&events), lines.each_ref().map(String::as_str));
    assert_eq!(reported(&events), Vec::<&str>::new());
}

#[test]
fn a_frames_card_whose_root_threw_still_fires_load() {
    let mut engine = serving_a_root_under(
        &load_host(CARD),
        &load_root("throw Error('the root threw');"),
    )
    .create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    let lines = load_lines(CARD);
    pump_until_logged(&mut engine, &mut events, &lines[4]);
    for _ in 0..20 {
        events.extend(engine.pump());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(load_logged(&events), lines.each_ref().map(String::as_str));
    let messages = reported(&events);
    assert_eq!(messages.len(), 1, "{events:?}");
    assert!(messages[0].contains("the root threw"), "{messages:?}");
}

#[test]
fn a_frame_src_that_does_not_fetch_fires_no_load() {
    let missing = "app:///missing.web.bundle";
    let mut engine =
        serving_a_root_under(&load_host(missing), &load_root("")).create(Arc::new(NoWakeup));
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
    for _ in 0..20 {
        events.extend(engine.pump());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(load_logged(&events), Vec::<&str>::new());
    let messages = reported(&events);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].contains(&format!("frame {missing}")),
        "{messages:?}"
    );
}

#[test]
fn a_load_listener_that_writes_a_new_src_replaces_the_booting_card() {
    let entry = host_page_with(
        CARD,
        true,
        &format!(
            r"
            __AddEventListener(frame, 'load', (event) => {{
                console.log('load ' + event.detail.url);
                if (event.detail.url === '{CARD}') __SetAttribute(frame, 'src', '{SECOND}');
            }}, {{}});
            "
        ),
    );
    let mut engine = serving_a_root_under(&entry, &root(CARD)).create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    pump_until_logged(&mut engine, &mut events, &format!("load {SECOND}"));
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {SECOND} booted undefined/undefined"),
    );
    pump_until(
        &mut engine,
        &mut events,
        "the second card rendered",
        |state| state.nested.len() == 2,
    );
    for _ in 0..20 {
        events.extend(engine.pump());
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        load_logged(&events),
        [format!("load {CARD}"), format!("load {SECOND}")]
    );
    assert_eq!(
        logged_times(&events, &format!("bts {CARD} booted undefined/undefined")),
        0,
        "the card let go of during its boot connected no BTS: {events:?}"
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}

fn ticking_root() -> String {
    format!("{} setInterval(() => console.log('tick'), 5);", root(CARD))
}

fn ticks(events: &[EngineEvent]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(event, EngineEvent::ConsoleMessage { message, .. } if message == "tick")
        })
        .count()
}

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

fn pump_until_ticking(engine: &mut TestEngine, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while ticks(&pump_for(engine, Duration::from_millis(10))) == 0 {
        assert!(Instant::now() < deadline, "{what}: no tick");
    }
}

fn assert_no_more_ticks(engine: &mut TestEngine, what: &str) {
    pump_for(engine, Duration::from_millis(150));
    let later = pump_for(engine, Duration::from_millis(200));
    assert_eq!(ticks(&later), 0, "{what}: {later:?}");
}

fn nested_of(
    engine: &mut TestEngine,
    frame: terminus::link::NodeId,
) -> Vec<terminus::link::NodeId> {
    engine
        .probe_document(move |tree| {
            let page = crate::main::tree::nested_page(tree, frame).expect("a frame");
            let mut nested = vec![page];
            nested.extend(
                tree.get(page)
                    .into_iter()
                    .flat_map(|page| page.children().filter(|child| child.is_element()))
                    .map(terminus::link::Node::id),
            );
            nested
        })
        .expect("the document answers")
}

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
                .map(terminus::link::Node::id)
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

fn write_frame(engine: &mut TestEngine, key: &str, value: &str) {
    engine
        .view
        .update_data(format!(r#"{{"{key}":{value}}}"#), String::new())
        .expect("the view takes an update");
}

fn nested_text(tree: &mut LynxDocument) -> Option<(terminus::link::NodeId, String)> {
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
    Some((text.id(), raw.attribute("text")?.into_owned()))
}

fn pump_until_text(
    engine: &mut TestEngine,
    events: &mut Vec<EngineEvent>,
    expected: &str,
    replacing: Option<terminus::link::NodeId>,
) -> terminus::link::NodeId {
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

fn boots(events: &[EngineEvent], src: &str) -> usize {
    let booted = format!("booted {src}");
    events
        .iter()
        .filter(|event| {
            matches!(event, EngineEvent::ConsoleMessage { message, .. } if *message == booted)
        })
        .count()
}

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

    for ignored in ["3", "[1]", r#""nope""#, r#""[1]""#] {
        write_frame(&mut engine, "data", ignored);
        write_frame(&mut engine, "globalProps", ignored);
    }
    write_src(&mut engine, &format!("{SECOND:?}"));
    let second = pump_until_text(&mut engine, &mut events, "second/there", Some(text));
    assert_eq!(boots(&events, SECOND), 1);

    operate(&mut engine, "detach");
    write_frame(&mut engine, "data", r#"{"label":"third"}"#);
    operate(&mut engine, "attach");
    let third = pump_until_text(&mut engine, &mut events, "third/there", Some(second));
    assert_eq!(boots(&events, SECOND), 2);

    write_frame(&mut engine, "data", "null");
    write_frame(&mut engine, "globalProps", r#"{"message":"last"}"#);
    assert_eq!(
        pump_until_text(&mut engine, &mut events, "third/last", None),
        third
    );
    operate(&mut engine, "detach");
    operate(&mut engine, "attach");
    pump_until_text(&mut engine, &mut events, "undefined/last", Some(third));
    assert_eq!(boots(&events, SECOND), 3);
    assert_eq!(reported(&events), Vec::<&str>::new());
}

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

#[test]
fn a_frames_card_background_thread_loads_the_views_native_es_modules() {
    let spec = TestViewSpec::new(&host_entry(CARD))
        .with_fetchable(CARD)
        .with_module(&format!("{CARD}/root.js"), &super::card_entry(&root(CARD)))
        .with_style_sheet_probe()
        .with_echo_esm_module(
            "Echo",
            "export const url = import.meta.url;
             globalThis.esmRuns = (globalThis.esmRuns ?? 0) + 1;",
            true,
        );
    let mut engine = with_bts(
        spec,
        CARD,
        "const ranBefore = globalThis.esmRuns;
         import('lynx:Echo').then(echo => log('esm ' + echo.url + ' ' + ranBefore + ' ' + globalThis.esmRuns));",
    )
    .create(Arc::new(NoWakeup));
    let mut events = Vec::new();
    pump_until_logged(
        &mut engine,
        &mut events,
        &format!("bts {CARD} esm lynx:Echo 1 1"),
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}

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

const GRAND: &str = "app:///grand.web.bundle";

const LOGGING_WORKLETS: &str = "
    globalThis.runWorklet = (value, params) => value.body(params[0]);
    const listen = (label) => (e) => console.log(
        label + ' ' + e.type + ' ' + e.target.id +
        (e.detail && e.detail.x !== undefined ? ' ' + e.detail.x + ',' + e.detail.y : ''));
    const log = (label) => ({ type: 'worklet', value: { body: listen(label) } });
";

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

const EVENT_CARD: &str = CARD;

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

fn hit_ids(engine: &mut TestEngine, x: f32, y: f32) -> Option<(Option<String>, Option<String>)> {
    let frame = engine.published_frame()?;
    let node = frame
        .hit(terminus::link::Point2D::new(x, y), &|_| None, None)?
        .node;
    engine
        .view
        .probe_document_within(Duration::from_millis(20), move |tree| {
            let id = |node| {
                tree.get(node)
                    .and_then(|live| live.attribute("id"))
                    .map(std::borrow::Cow::into_owned)
            };
            let host = tree
                .shadow_root_of(node)
                .and_then(|root| tree.shadow_host(root));
            (id(node), host.and_then(id))
        })
}

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

fn tap(engine: &mut TestEngine, (x, y): (f32, f32)) {
    for phase in [
        terminus::link::input::PointerPhase::Down,
        terminus::link::input::PointerPhase::Up,
    ] {
        engine.dispatch_input(terminus::link::input::InputEvent::pointer(
            terminus::link::Point2D::new(x, y),
            1,
            terminus::link::input::PointerKind::Touch,
            phase,
        ));
    }
}

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

fn event_op(engine: &mut TestEngine, op: &str) {
    operate(engine, op);
    let mut events = Vec::new();
    pump_until_logged(engine, &mut events, &format!("page-op {op}"));
    assert_eq!(reported(&events), Vec::<&str>::new());
}

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

const CV: (f32, f32) = (120.0, 60.0);
const SWIPER: (f32, f32) = (120.0, 120.0);
const GV: (f32, f32) = (120.0, 180.0);
const PADDING: (f32, f32) = (5.0, 5.0);

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

#[test]
fn a_key_reaches_a_cards_global_bind() {
    let mut engine = booted_event_view();
    engine.dispatch_input(terminus::link::input::InputEvent::key(
        terminus::link::input::KeyPhase::Down,
        "a",
        "KeyA",
        false,
        terminus::link::input::Modifiers::default(),
    ));
    let lines = lines_until(&mut engine, "card-key-global keydown cv");
    assert_eq!(
        lines,
        ["page-key keydown page", "card-key-global keydown cv"]
    );
}

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

const EXPOSURE_PAGE_BTS: &str = "
import { lynx } from 'bobcat:bts-runtime';
const emitter = lynx.getJSModule('GlobalEventEmitter');
for (const name of ['exposure', 'disexposure']) {
  emitter.addListener(name, (list) => console.log(
    'page-bts ' + name + ' ' + list.map((record) => record['exposure-id']).join(',')));
}
";

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

fn exposure_op(engine: &mut TestEngine, events: &mut Vec<EngineEvent>, op: &str) {
    operate(engine, op);
    pump_until_logged(engine, events, &format!("page-op {op}"));
}

fn exposure_card_op(engine: &mut TestEngine, events: &mut Vec<EngineEvent>, op: &str) {
    engine
        .view
        .update_data(format!(r#"{{"op":"card","card":"{op}"}}"#), String::new())
        .expect("the view takes an update");
    pump_until_logged(engine, events, &format!("card-op {op}"));
}

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

fn host_nested_page(engine: &mut TestEngine) -> (terminus::link::NodeId, Vec<String>) {
    engine
        .probe_document(|tree| {
            let frame = tree
                .document_element()
                .children()
                .find(|child| child.tag_name() == Some("frame"))
                .map(terminus::link::Node::id)
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

#[test]
fn each_background_thread_hears_its_own_cards_exposure_alone() {
    let (mut engine, mut events) = booted_exposure_view();
    events.extend(pump_for(&mut engine, Duration::from_millis(100)));
    let mut lines = exposure_lines(&events);
    lines.sort_unstable();
    let card = format!("bts {CARD} exposure card-cv:number");
    let grand = format!("bts {GRAND} exposure grand-gv:number");
    let mut expected = vec!["page-bts exposure page-own", &card, &grand];
    expected.sort_unstable();
    assert_eq!(lines, expected);
    assert_eq!(reported(&events), Vec::<&str>::new());
}

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
    assert_eq!(tags, ["view", "frame"], "and rendered into it");

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
    assert_eq!(
        lines_with(&events, &format!("bts {GRAND} disexposure")),
        Vec::<&str>::new()
    );
    assert_eq!(reported(&events), Vec::<&str>::new());
}

#[test]
fn a_frames_card_fires_loadmetrics_once_its_load_completes() {
    let src = "child.web.bundle";
    let host = host_page_with(
        src,
        true,
        r"
        globalThis.runWorklet = (value, params) => value.body(params[0]);
        const heard = (label) => (event) => console.log('loadmetrics ' + label + ' ' +
            JSON.stringify({
                url: event.detail.url,
                mode: event.detail.mode,
                entryType: event.detail.entry.entryType,
                name: event.detail.entry.name,
                frame: event.target.uid === __GetElementUniqueID(frame),
            }));
        __AddEvent(frame, 'bindEvent', 'loadmetrics', { type: 'worklet', value: { body: heard('bind') } });
        __AddEventListener(page, 'loadmetrics', heard('page'), {});
        ",
    );
    let root = "globalThis.renderPage = () => { const p = __CreatePage('0', 0); \
                const t = __CreateText(0); __AppendElement(t, __CreateRawText('nested')); \
                __AppendElement(p, t); };";
    let mut engine = serving_a_root_under(&host, root)
        .offscreen(120.0, 80.0)
        .create(Arc::new(NoWakeup));
    let heard = format!(
        r#"{{"url":"{src}","mode":"standard","entryType":"pipeline","name":"loadBundle","frame":true}}"#
    );
    let lines = [
        format!("loadmetrics bind {heard}"),
        format!("loadmetrics page {heard}"),
    ];
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !logged(&events, &lines[1]) {
        let _ = engine.tick(true);
        for event in engine.pump() {
            assert!(!event.is_fatal(), "the view ended: {event:?}");
            events.push(event);
        }
        assert!(Instant::now() < deadline, "no loadmetrics: {events:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..40 {
        let _ = engine.tick(true);
        events.extend(engine.pump());
        std::thread::sleep(Duration::from_millis(2));
    }
    let heard: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ConsoleMessage { message, .. } if message.starts_with("loadmetrics ") => {
                Some(message.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(heard, lines.each_ref().map(String::as_str));
    assert_eq!(reported(&events), Vec::<&str>::new());
}
