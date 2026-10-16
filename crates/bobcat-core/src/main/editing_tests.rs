//! Editable text end to end through a real view (PR E2): a host's
//! [`LynxView::edit`](crate::LynxView::edit) and the painter's routed key and
//! pointer events reach the focused editing host on the main thread, the
//! events they owe reach a realm listener, and
//! [`EngineEvent::TextInputStateChanged`] comes back.
//!
//! Every page here is one `<view contenteditable>` (node 3) whose listeners
//! append a description of each event to its `log` attribute, which a probe
//! reads on the view's own thread.

use std::time::{Duration, Instant};

use dom::Point2D;
use dom::editing::{Command, TextInputState};
use dom::input::{InputEvent, KeyPhase, Modifiers, PointerKind, PointerPhase};

use crate::EngineEvent;
use crate::test_support::{TestEngine, TestViewSpec, wait_for_boot};

/// Solid em squares: at 20px every character is a 20px-wide box.
const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

/// The editing host.
const HOST: u64 = 3;

fn node_id(bits: u64) -> dom::NodeId {
    dom::NodeId::from_bits(bits).expect("a well-formed packed handle")
}

/// One host 200×40 at the page's origin, listening for `names` with a
/// listener that logs `type:inputType:data:isComposing:relatedTarget` —
/// each member only when the event carries it — and running `extra` (which
/// sees `event`) first.
fn page(names: &[&str], extra: &str) -> String {
    let names = names
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const host = __CreateView(0);
          __AppendElement(page, host);
          globalThis.held = [page, host];
          globalThis.entries = [];
          __SetAttribute(host, 'contenteditable', '');
          __SetInlineStyles(host, 'width:200px;height:40px');
          const note = (event) => {{
            {extra}
            const related = 'relatedTarget' in event
              ? String(event.relatedTarget && event.relatedTarget.uid)
              : undefined;
            entries.push([event.type, event.inputType, event.data, event.isComposing, related]
              .filter((part) => part !== undefined).join(':'));
            __SetAttribute(host, 'log', entries.join('|'));
          }};
          for (const name of [{names}]) {{
            __AddEventListener(host, name, note, {{}});
          }}
          __FlushElementTree();
        }};
        "
    )
}

fn booted(names: &[&str], extra: &str) -> TestEngine {
    TestViewSpec::new(&page(names, extra)).boot()
}

fn log_of(engine: &mut TestEngine) -> Option<String> {
    engine
        .probe_document(|tree| {
            tree.get(node_id(HOST))
                .and_then(|host| host.attribute("log").map(str::to_owned))
        })
        .flatten()
}

/// Polls until the host's `log` equals `expected` exactly.
fn wait_for_log(engine: &mut TestEngine, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let log = log_of(engine);
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

/// Every [`TextInputState`] the view has published, in order, as a host
/// sees them through `pump`.
#[derive(Default)]
struct States(Vec<TextInputState>);

impl States {
    fn take(&mut self, engine: &mut TestEngine) {
        for event in engine.pump() {
            if let EngineEvent::TextInputStateChanged(state) = event {
                self.0.push(*state);
            }
        }
    }

    /// Pumps until the newest published state satisfies `until`, and
    /// answers it.
    fn wait(
        &mut self,
        engine: &mut TestEngine,
        what: &str,
        until: impl Fn(&TextInputState) -> bool,
    ) -> TextInputState {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.take(engine);
            if let Some(state) = self.0.last().filter(|state| until(state)) {
                return state.clone();
            }
            assert!(
                Instant::now() < deadline,
                "no published state {what}; the last was {:?}",
                self.0.last()
            );
            std::thread::yield_now();
        }
    }
}

fn touch(phase: PointerPhase, x: f32, y: f32) -> InputEvent {
    InputEvent::pointer(Point2D::new(x, y), 1, PointerKind::Touch, phase)
}

fn mouse(phase: PointerPhase, x: f32, y: f32) -> InputEvent {
    InputEvent::pointer(Point2D::new(x, y), 1, PointerKind::Mouse, phase)
}

/// A `keydown` of `text`, which the key typed and the platform reports with
/// it.
fn key(text: &str) -> InputEvent {
    InputEvent::key(KeyPhase::Down, text, "", false, Modifiers::default())
        .with_text(Some(text.into()))
}

/// Taps the host with a finger, which focuses it on the tap (the `click`),
/// and waits for the state that says so.
fn focus_host(engine: &mut TestEngine, states: &mut States) {
    engine.dispatch_input(touch(PointerPhase::Down, 20.0, 20.0));
    engine.dispatch_input(touch(PointerPhase::Up, 20.0, 20.0));
    states.wait(engine, "focusing the host", |state| {
        state.host == Some(node_id(HOST))
    });
}

/// A host's insertion dispatches `beforeinput` then `input`, with
/// `InputEvent`'s members, at the focused host, and the value follows.
#[test]
fn an_edit_from_the_host_fires_beforeinput_then_input_and_changes_the_value() {
    let mut engine = booted(&["beforeinput", "input"], "");
    let mut states = States::default();
    focus_host(&mut engine, &mut states);

    engine
        .view
        .edit(Command::Insert("a".to_owned()))
        .expect("a ready view takes an edit");
    wait_for_log(
        &mut engine,
        "beforeinput:insertText:a:false|input:insertText:a:false",
    );
    let state = states.wait(&mut engine, "with the value a", |state| state.value == "a");
    assert_eq!(state.selection, 1..1);
    assert_eq!(state.composition, None);
}

/// A listener that cancels the `beforeinput` drops the edit: no `input`, no
/// change. The next edit, not canceled, goes through.
#[test]
fn a_canceled_beforeinput_drops_the_edit() {
    let mut engine = booted(
        &["beforeinput", "input"],
        "if (event.type === 'beforeinput' && event.data === 'x') event.preventDefault();",
    );
    let mut states = States::default();
    focus_host(&mut engine, &mut states);

    engine.view.edit(Command::Insert("x".to_owned())).unwrap();
    engine.view.edit(Command::Insert("y".to_owned())).unwrap();
    wait_for_log(
        &mut engine,
        "beforeinput:insertText:x:false|beforeinput:insertText:y:false|input:insertText:y:false",
    );
    states.wait(&mut engine, "with the value y", |state| state.value == "y");
    assert!(
        states.0.iter().all(|state| !state.value.contains('x')),
        "the canceled insertion never reached the value: {:?}",
        states.0
    );
}

/// An input method's composition and commit fire the composition events
/// with their `data`, and `beforeinput`/`input` of `insertCompositionText`
/// with `isComposing` — which a listener cannot cancel (Input Events
/// Level 2), so the `preventDefault` here changes nothing.
#[test]
fn a_composition_fires_the_composition_events_and_cannot_be_canceled() {
    let mut engine = booted(
        &[
            "compositionstart",
            "compositionupdate",
            "compositionend",
            "beforeinput",
            "input",
        ],
        "if (event.type === 'beforeinput') event.preventDefault();",
    );
    let mut states = States::default();
    focus_host(&mut engine, &mut states);

    engine
        .view
        .edit(Command::SetComposition {
            text: "に".to_owned(),
            selection: 1..1,
        })
        .unwrap();
    let composing = states.wait(&mut engine, "composing に", |state| {
        state.composition.is_some()
    });
    assert_eq!(composing.value, "に");
    assert_eq!(composing.composition, Some(0..1));
    assert_eq!(composing.composition_bounds.len(), 1);

    engine
        .view
        .edit(Command::CommitText {
            text: "日".to_owned(),
            relative_cursor: 0,
        })
        .unwrap();
    wait_for_log(
        &mut engine,
        "compositionstart:|beforeinput:insertCompositionText:に:true|compositionupdate:に\
         |input:insertCompositionText:に:true|beforeinput:insertCompositionText:日:true\
         |compositionupdate:日|input:insertCompositionText:日:true|compositionend:日",
    );
    let committed = states.wait(&mut engine, "committed", |state| state.value == "日");
    assert_eq!(committed.composition, None);
    assert_eq!(committed.selection, 1..1);
}

/// The state reaches the host after an edit, and only when it changed: no
/// two published states are equal, and a command that changes nothing
/// publishes nothing.
#[test]
fn the_text_input_state_is_published_when_it_changes_and_only_then() {
    let mut engine = booted(&[], "");
    let mut states = States::default();
    focus_host(&mut engine, &mut states);
    engine.view.edit(Command::Insert("ab".to_owned())).unwrap();
    let typed = states.wait(&mut engine, "with the value ab", |state| {
        state.value == "ab"
    });
    assert_eq!(typed.selection, 2..2);
    assert!(typed.caret.size.height > 0.0, "a caret box: {typed:?}");
    assert_eq!(
        typed.editable_bounds.size,
        dom::Size2D::new(200.0, 40.0),
        "the host's border box"
    );

    // The caret is already at 2, so this changes nothing. Two probes after
    // it: the second is a burst of its own, queued after the epilogue of
    // the burst the command was in, so whatever that epilogue published is
    // in the notice queue once it answers.
    engine
        .view
        .edit(Command::SetSelection { range: 2..2 })
        .unwrap();
    engine.probe_document(|_| ());
    engine.probe_document(|_| ());
    let before = states.0.len();
    states.take(&mut engine);
    assert_eq!(
        states.0.len(),
        before,
        "nothing changed, nothing was published: {:?}",
        &states.0[before..]
    );

    engine
        .view
        .edit(Command::SetSelection { range: 0..1 })
        .unwrap();
    let selected = states.wait(&mut engine, "selecting a", |state| {
        state.selection == (0..1)
    });
    assert_eq!(selected.value, "ab");
    for pair in states.0.windows(2) {
        assert_ne!(pair[0], pair[1], "a state equal to the last was published");
    }
}

/// A key goes to the focused host, and a `keydown` nobody canceled inserts
/// the text it typed; one a listener canceled does not.
#[test]
fn a_keydown_types_into_the_focused_host_unless_canceled() {
    let mut engine = booted(
        &["keydown"],
        "if (event.key === 'q') event.preventDefault();",
    );
    let mut states = States::default();
    focus_host(&mut engine, &mut states);

    engine.dispatch_input(key("q"));
    engine.dispatch_input(key("x"));
    wait_for_log(&mut engine, "keydown|keydown");
    states.wait(&mut engine, "with the value x", |state| state.value == "x");
    assert!(
        states.0.iter().all(|state| !state.value.contains('q')),
        "the canceled key typed nothing: {:?}",
        states.0
    );
    let target = engine.probe_document(|tree| tree.focused()).flatten();
    assert_eq!(target, Some(node_id(HOST)));
}

/// A key the platform reports without its text — its input method delivers
/// the text instead, as macOS's `insertText:` and a browser's `beforeinput`
/// do — types nothing by its `keydown`; the host's commit of that text is
/// the one insertion.
#[test]
fn a_keydown_without_text_types_nothing_and_the_commit_inserts_once() {
    let mut engine = booted(&["keydown"], "");
    let mut states = States::default();
    focus_host(&mut engine, &mut states);

    engine.dispatch_input(InputEvent::key(
        KeyPhase::Down,
        "y",
        "KeyY",
        false,
        Modifiers::default(),
    ));
    engine
        .view
        .edit(Command::CommitText {
            text: "y".to_owned(),
            relative_cursor: 0,
        })
        .expect("a booted view takes edits");
    engine.dispatch_input(key("z"));
    wait_for_log(&mut engine, "keydown|keydown");
    let typed = states.wait(&mut engine, "with the value yz", |state| {
        state.value == "yz"
    });
    assert_eq!(typed.selection, 2..2);
    assert!(
        states.0.iter().all(|state| state.value != "yy"),
        "the key without text inserted nothing: {:?}",
        states.0
    );
}

/// A finger's tap on a host nobody listens to the pointer events of
/// focuses it (`focus` then `focusin`, with no `relatedTarget`); keys typed
/// with no `keydown` listener reach it; a mouse press places the caret at
/// the character pressed, a mouse drag extends the selection, and a quick
/// second press selects the word. Ahem makes every character a 20px box, so
/// offsets are exact.
#[test]
fn a_press_focuses_the_host_and_places_the_caret_where_it_landed() {
    let mut engine = TestViewSpec::new(&page(&["focus", "focusin"], ""))
        .with_style_sheet(
            "@font-face { font-family: TestAhem; src: url(\"app:///ahem.ttf\"); }
             view { font-family: TestAhem; font-size: 20px; line-height: 20px; }",
        )
        .with_font("app:///ahem.ttf", AHEM)
        .create(std::sync::Arc::new(crate::view::NoWakeup));
    wait_for_boot(&mut engine);
    let mut states = States::default();

    focus_host(&mut engine, &mut states);
    wait_for_log(&mut engine, "focus:null|focusin:null");

    for text in ["a", "b", "c", "d", "e"] {
        engine.dispatch_input(key(text));
    }
    states.wait(&mut engine, "with the value abcde", |state| {
        state.value == "abcde"
    });
    // The face is a load of its own: the text is Ahem's once it is five
    // 20px squares wide.
    let deadline = Instant::now() + Duration::from_secs(30);
    let origin = loop {
        engine.pump();
        let measured = engine
            .probe_document(|tree| {
                let host = node_id(HOST);
                Some((
                    tree.text_block_size(host)?.width,
                    tree.bounding_client_rect(host)?.origin,
                ))
            })
            .flatten();
        if let Some((width, origin)) = measured
            && (width - 100.0).abs() < f32::EPSILON
        {
            break origin;
        }
        assert!(
            Instant::now() < deadline,
            "the host never measured Ahem's 100px: {measured:?}"
        );
        std::thread::yield_now();
    };

    // Inside `c`'s left half: the caret goes before it.
    engine.painter.clock.pin(10.0);
    engine.dispatch_input(mouse(PointerPhase::Down, origin.x + 45.0, origin.y + 10.0));
    states.wait(&mut engine, "with the caret at 2", |state| {
        state.selection == (2..2)
    });
    // Dragged with the button held into `e`'s left half: the selection
    // runs from the press to there.
    engine.dispatch_input(mouse(PointerPhase::Move, origin.x + 85.0, origin.y + 10.0));
    states.wait(&mut engine, "selecting cd", |state| {
        state.selection == (2..4)
    });
    engine.dispatch_input(mouse(PointerPhase::Up, origin.x + 85.0, origin.y + 10.0));

    // Two quick presses: the second selects the word.
    engine.painter.clock.pin(20.0);
    engine.dispatch_input(mouse(PointerPhase::Down, origin.x + 45.0, origin.y + 10.0));
    engine.dispatch_input(mouse(PointerPhase::Up, origin.x + 45.0, origin.y + 10.0));
    engine.painter.clock.pin(20.2);
    engine.dispatch_input(mouse(PointerPhase::Down, origin.x + 45.0, origin.y + 10.0));
    engine.dispatch_input(mouse(PointerPhase::Up, origin.x + 45.0, origin.y + 10.0));
    states.wait(&mut engine, "selecting the word", |state| {
        state.selection == (0..5)
    });
    assert_eq!(
        log_of(&mut engine).as_deref(),
        Some("focus:null|focusin:null"),
        "a press on the focused host does not focus it again"
    );
}

/// A finger that starts a scroll on a host does not focus it: touch focuses
/// on the tap, and a drag the scroller consumed is no tap. A tap afterwards
/// does. The host (node 4) is the first child of a 200px column scroller
/// (node 3) and twice its height.
#[test]
fn a_touch_drag_that_scrolls_does_not_focus_the_host_and_a_tap_does() {
    let mut engine = TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const scroller = __CreateView(0);
          const host = __CreateView(0);
          __AppendElement(page, scroller);
          __AppendElement(scroller, host);
          globalThis.held = [page, scroller, host];
          __SetInlineStyles(scroller, 'display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px');
          __SetInlineStyles(host, 'flex-shrink:0;width:200px;height:400px');
          __SetAttribute(host, 'contenteditable', '');
          __FlushElementTree();
        };
        ",
    )
    .boot();
    let focused = |engine: &mut TestEngine| engine.probe_document(|tree| tree.focused()).flatten();

    engine.dispatch_input(touch(PointerPhase::Down, 100.0, 100.0));
    engine.dispatch_input(touch(PointerPhase::Move, 100.0, 70.0));
    engine.dispatch_input(touch(PointerPhase::Up, 100.0, 70.0));
    let offset = engine
        .probe_document(|tree| tree.scroll_offset(node_id(3)))
        .expect("the view answers");
    assert!(offset.y > 0.0, "the drag scrolled: {offset:?}");
    assert_eq!(focused(&mut engine), None, "a scroll is no tap");

    engine.dispatch_input(touch(PointerPhase::Down, 100.0, 100.0));
    engine.dispatch_input(touch(PointerPhase::Up, 100.0, 100.0));
    assert_eq!(focused(&mut engine), Some(node_id(4)), "the tap focused it");
}
