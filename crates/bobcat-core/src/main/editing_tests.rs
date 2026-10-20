//! Editable text end to end through a real view (PR E2): a host's
//! [`LynxView::edit`](crate::LynxView::edit) and the painter's routed key and
//! pointer events reach the focused editing host on the main thread, the
//! events they owe reach a realm listener, and
//! [`EngineEvent::TextInputStateChanged`] comes back.
//!
//! Every page here is one `<view contenteditable>` (node 3) whose listeners
//! append a description of each event to its `log` attribute, which a probe
//! reads on the view's own thread.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use terminus::Point2D;
use terminus::editing::{Command, TextInputState};
use terminus::input::{InputEvent, KeyPhase, Modifiers, PointerKind, PointerPhase};

use crate::EngineEvent;
use crate::editing::TextInputSource;
use crate::test_support::{TestEngine, TestViewSpec, wait_for_boot};

/// Solid em squares: at 20px every character is a 20px-wide box.
const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");

/// The editing host.
const HOST: u64 = 3;

fn node_id(bits: u64) -> terminus::NodeId {
    terminus::NodeId::from_bits(bits).expect("a well-formed packed handle")
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

/// What a listener runs first to show the event carries no cancellation
/// member: a listener that throws logs nothing, so the expected log is
/// never reached.
const NO_CANCELLATION: &str = "if ('preventDefault' in event || 'cancelable' in event \
     || 'defaultPrevented' in event) throw new Error('a cancellation member');";

/// A `beforeinput` listener cannot stop the edit: the event has no
/// `preventDefault` (`docs/tracking/deviations.md`), the value changes and
/// `input` follows.
#[test]
fn a_beforeinput_listener_cannot_stop_the_edit() {
    let mut engine = booted(&["beforeinput", "input"], NO_CANCELLATION);
    let mut states = States::default();
    focus_host(&mut engine, &mut states);

    engine.view.edit(Command::Insert("x".to_owned())).unwrap();
    wait_for_log(
        &mut engine,
        "beforeinput:insertText:x:false|input:insertText:x:false",
    );
    states.wait(&mut engine, "with the value x", |state| state.value == "x");
}

/// An input method's composition and commit fire the composition events
/// with their `data`, and `beforeinput`/`input` of `insertCompositionText`
/// with `isComposing`.
#[test]
fn a_composition_fires_the_composition_events() {
    let mut engine = booted(
        &[
            "compositionstart",
            "compositionupdate",
            "compositionend",
            "beforeinput",
            "input",
        ],
        NO_CANCELLATION,
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
        terminus::Size2D::new(200.0, 40.0),
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

/// A key goes to the focused host, and a `keydown` inserts the text it
/// typed whatever its listener does: the event has no `preventDefault`.
#[test]
fn a_keydown_types_into_the_focused_host_whatever_its_listener_does() {
    let mut engine = booted(&["keydown"], NO_CANCELLATION);
    let mut states = States::default();
    focus_host(&mut engine, &mut states);

    engine.dispatch_input(key("q"));
    engine.dispatch_input(key("x"));
    wait_for_log(&mut engine, "keydown|keydown");
    states.wait(&mut engine, "with the value qx", |state| {
        state.value == "qx"
    });
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

/// A host that sets [`TextInputSource::Commands`] — the browser host, whose
/// `<textarea>` proxy reports every insertion and deletion as a
/// `beforeinput` it forwards as a command — gets only movement and
/// select-all from a `keydown`: a key's text types nothing, and a Backspace
/// sent both as a key and as the proxy's command deletes once.
#[test]
fn a_host_that_sends_commands_deletes_once_and_keys_only_move() {
    let mut engine = booted(&["keydown"], "");
    engine
        .view
        .set_text_input_source(TextInputSource::Commands)
        .expect("a live view takes the setting");
    let mut states = States::default();
    focus_host(&mut engine, &mut states);

    engine.view.edit(Command::Insert("abc".to_owned())).unwrap();
    engine.dispatch_input(InputEvent::key(
        KeyPhase::Down,
        "Backspace",
        "Backspace",
        false,
        Modifiers::default(),
    ));
    engine.view.edit(Command::DeleteBackward).unwrap();
    engine.dispatch_input(key("x"));
    engine.dispatch_input(InputEvent::key(
        KeyPhase::Down,
        "ArrowLeft",
        "ArrowLeft",
        false,
        Modifiers::default(),
    ));
    wait_for_log(&mut engine, "keydown|keydown|keydown");
    let moved = states.wait(&mut engine, "with the caret moved left", |state| {
        state.selection == (1..1)
    });
    // Keys that deleted and typed would have left "ax".
    assert_eq!(moved.value, "ab");
    assert!(
        states
            .0
            .iter()
            .all(|state| state.value != "a" && !state.value.contains('x')),
        "a key typed or deleted: {:?}",
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
    // The face is a load of its own: the text is Ahem's once the caret
    // after it is five 20px squares in. The host is a `view`, so its text is
    // laid out by the anonymous block box around it, which the published
    // state measures through.
    let state = states.wait(&mut engine, "with Ahem's 100px", |state| {
        (state.caret.min_x() - state.editable_bounds.min_x() - 100.0).abs() < f32::EPSILON
    });
    let origin = state.editable_bounds.origin;

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

// --- `selectionchange` ---------------------------------------------------------

/// A `<text contenteditable>` (node 3) logging, to its `log` attribute, the
/// W3C `selectionchange` at the document (`document`, through the handle of
/// the document node, id 1) and its own Lynx `selectionchange` with web-core's
/// detail (`text:start:end:direction`).
const SELECTION_PAGE: &str = r"
    import { __BobcatHandleOf } from 'bobcat:element';
    globalThis.renderPage = function () {
      const page = __CreatePage('card', 0);
      const host = __CreateElement('text', 0);
      __AppendElement(page, host);
      globalThis.held = [page, host];
      globalThis.entries = [];
      __SetAttribute(host, 'contenteditable', '');
      __SetInlineStyles(host, 'width:200px;height:40px');
      const log = (entry) => {
        entries.push(entry);
        __SetAttribute(host, 'log', entries.join('|'));
      };
      __AddEventListener(host, 'selectionchange', (event) => log('text:' + event.detail.start
        + ':' + event.detail.end + ':' + event.detail.direction));
      __AddEventListener(__BobcatHandleOf(1), 'selectionchange', () => log('document'));
      __FlushElementTree();
    };
";

/// A key with Shift held and no text: an arrow that extends the selection.
fn shift_key(key: &str) -> InputEvent {
    InputEvent::key(
        KeyPhase::Down,
        key,
        key,
        false,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    )
}

/// Every change of the document's selection fires the W3C
/// `selectionchange` at the document — focusing the host, typing, placing
/// the caret, each Shift+arrow, the blur of a tap outside it — and the
/// `<text>` hears its Lynx `selectionchange` right after it, in the same
/// entry, whenever its record changed: a range in its text, forward or
/// backward, and the empty record `-1:-1:forward` once its selection is gone.
/// A collapsed caret is the empty record, which is not sent before a range
/// was.
#[test]
fn selection_changes_fire_at_the_document_then_at_the_text() {
    let mut engine = TestViewSpec::new(SELECTION_PAGE).boot();
    let mut states = States::default();
    let mut expected = vec!["document"];

    focus_host(&mut engine, &mut states);
    wait_for_log(&mut engine, &expected.join("|"));
    // The host's text is typed: a `raw-text` carrier is no text node, and an
    // editing host's value is its text nodes.
    engine
        .view
        .edit(Command::Insert("abcdef".to_owned()))
        .unwrap();
    expected.push("document");
    wait_for_log(&mut engine, &expected.join("|"));
    engine
        .view
        .edit(Command::SetSelection { range: 0..0 })
        .unwrap();
    expected.push("document");
    wait_for_log(&mut engine, &expected.join("|"));

    for (key, text) in [
        ("ArrowRight", "text:0:1:forward"),
        ("ArrowRight", "text:0:2:forward"),
        ("ArrowLeft", "text:0:1:forward"),
    ] {
        engine.dispatch_input(shift_key(key));
        expected.extend(["document", text]);
        wait_for_log(&mut engine, &expected.join("|"));
    }

    engine.dispatch_input(touch(PointerPhase::Down, 300.0, 300.0));
    engine.dispatch_input(touch(PointerPhase::Up, 300.0, 300.0));
    expected.extend(["document", "text:-1:-1:forward"]);
    wait_for_log(&mut engine, &expected.join("|"));
    states.wait(&mut engine, "blurred", |state| state.host.is_none());
}

// --- `x-input` and `x-textarea` (E5) -----------------------------------------

/// The text field (node 3), 200×40 with 10px padding in 20px Ahem, with
/// `attributes`, and listeners for the Lynx events and the standard
/// `beforeinput`, each logging `type:detail` — the detail as JSON — to the
/// field's `log` attribute. `on_confirm` runs in the `confirm` listener
/// with `field` and `note` in scope, after the confirm is logged.
fn field_page(tag: &str, attributes: &[(&str, &str)], on_confirm: &str) -> String {
    let mut set = String::new();
    for (name, value) in attributes {
        write!(set, "__SetAttribute(field, '{name}', '{value}');").expect("writing to a String");
    }
    let attributes = set;
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const field = __CreateElement('{tag}', 0);
          __AppendElement(page, field);
          globalThis.held = [page, field];
          globalThis.entries = [];
          {attributes}
          __SetInlineStyles(field, 'width:200px;height:40px;padding:10px;font-family:Ahem;font-size:20px;line-height:20px');
          const note = (line) => {{
            entries.push(line);
            __SetAttribute(field, 'log', entries.join('|'));
          }};
          for (const name of ['input', 'confirm', 'focus', 'blur', 'selection', 'beforeinput']) {{
            __AddEventListener(field, name, (event) => {{
              note(event.type + ':' + JSON.stringify(event.detail));
              if (event.type === 'confirm') {{
                {on_confirm}
              }}
            }}, {{}});
          }}
          __FlushElementTree();
        }};
        "
    )
}

fn field_engine(tag: &str, attributes: &[(&str, &str)], on_confirm: &str) -> TestEngine {
    TestViewSpec::new(&field_page(tag, attributes, on_confirm))
        .with_font("Ahem", AHEM)
        .boot()
}

fn enter() -> InputEvent {
    InputEvent::key(
        KeyPhase::Down,
        "Enter",
        "Enter",
        false,
        Modifiers::default(),
    )
    .with_text(Some("\r".into()))
}

/// A tap in the field's padding focuses its editor; keys type into it and
/// fire web-core's `input`, never the standard `beforeinput` (which stays in
/// the shadow tree); `maxlength` refuses the fourth character; Enter fires
/// `confirm`, whose listener reads `getValue` and blurs the field, which
/// fires `blur`.
#[test]
fn an_x_input_types_confirms_and_reports_in_web_core_s_shape() {
    let mut engine = field_engine(
        "x-input",
        &[("maxlength", "3"), ("placeholder", "say")],
        "const answer = [];
         __InvokeUIMethod(field, 'getValue', {}, (result) => answer.push(result));
         note('getValue:' + JSON.stringify(answer[0]));
         __InvokeUIMethod(field, 'blur', {}, () => {});",
    );
    let mut states = States::default();
    engine.dispatch_input(touch(PointerPhase::Down, 3.0, 3.0));
    engine.dispatch_input(touch(PointerPhase::Up, 3.0, 3.0));
    let focused = states.wait(&mut engine, "focusing the editor", |state| {
        state.host.is_some()
    });
    assert_ne!(
        focused.host,
        Some(node_id(HOST)),
        "the editor, not the host"
    );
    assert_eq!(
        focused.enter_key_hint,
        terminus::editing::EnterKeyHint::Send
    );

    for text in ["a", "b", "c", "d"] {
        engine.dispatch_input(key(text));
    }
    states.wait(&mut engine, "with abc", |state| state.value == "abc");
    engine.dispatch_input(enter());
    let detail = |value: &str, at: u32| {
        format!(
            r#"{{"value":"{value}","textLength":{at},"cursor":{at},"isComposing":false,"selectionStart":{at},"selectionEnd":{at}}}"#
        )
    };
    wait_for_log(
        &mut engine,
        &[
            r#"focus:{"value":""}"#.to_owned(),
            format!("input:{}", detail("a", 1)),
            format!("input:{}", detail("ab", 2)),
            format!("input:{}", detail("abc", 3)),
            r#"confirm:{"value":"abc"}"#.to_owned(),
            r#"getValue:{"code":0,"data":{"value":"abc","selectionStart":3,"selectionEnd":3,"isComposing":false,"selectionBegin":3}}"#.to_owned(),
            r#"blur:{"value":"abc"}"#.to_owned(),
        ]
        .join("|"),
    );
    let blurred = states.wait(&mut engine, "blurred", |state| state.host.is_none());
    assert_eq!(blurred, TextInputState::default());
}

/// A host's input-method commands reach the field's editor: a composition
/// fires one `input` at its end, and `setValue`/`setSelectionRange` from a
/// listener move the value and the selection with no `input`.
#[test]
fn an_x_textarea_reports_a_composition_once_and_takes_script_writes() {
    let mut engine = field_engine(
        "x-textarea",
        &[("confirm-enter", "")],
        "__InvokeUIMethod(field, 'setValue', { value: 'hello' }, () => {});
         __InvokeUIMethod(field, 'setSelectionRange', { selectionStart: 1, selectionEnd: 3 }, () => {});",
    );
    let mut states = States::default();
    engine.dispatch_input(touch(PointerPhase::Down, 20.0, 20.0));
    engine.dispatch_input(touch(PointerPhase::Up, 20.0, 20.0));
    states.wait(&mut engine, "focusing the editor", |state| {
        state.host.is_some()
    });
    for text in ["に", "日"] {
        engine
            .view
            .edit(Command::SetComposition {
                text: text.to_owned(),
                selection: 1..1,
            })
            .unwrap();
    }
    engine
        .view
        .edit(Command::FinishComposition {
            keep_selection: false,
        })
        .unwrap();
    engine.dispatch_input(enter());
    wait_for_log(
        &mut engine,
        &[
            r#"focus:{"value":""}"#,
            r#"input:{"value":"日","textLength":1,"cursor":1,"isComposing":false,"selectionStart":1,"selectionEnd":1}"#,
            r#"input:{"value":"日\n","textLength":2,"cursor":2,"isComposing":false,"selectionStart":2,"selectionEnd":2}"#,
            r#"confirm:{"value":"日\n"}"#,
            r#"selection:{"selectionStart":1,"selectionEnd":3}"#,
        ]
        .join("|"),
    );
    let state = states.wait(&mut engine, "with hello", |state| state.value == "hello");
    assert_eq!(state.selection, 1..3);
}

/// A `readonly` field cannot be focused: a tap focuses nothing and keys
/// type nothing.
#[test]
fn a_readonly_x_input_is_not_focused_by_a_tap() {
    let mut engine = field_engine("x-input", &[("readonly", ""), ("value", "fixed")], "");
    engine.dispatch_input(touch(PointerPhase::Down, 20.0, 20.0));
    engine.dispatch_input(touch(PointerPhase::Up, 20.0, 20.0));
    engine.dispatch_input(key("x"));
    let (focused, value) = engine
        .probe_document(|tree| {
            let editor = tree.shadow_root(node_id(HOST)).and_then(|root| {
                tree.get(root)?
                    .children()
                    .find(|child| child.is_element())
                    .map(terminus::Node::id)
            });
            let value = editor
                .and_then(|editor| tree.get(editor)?.first_child()?.text().map(str::to_owned));
            (tree.focused(), value)
        })
        .expect("the view answers");
    assert_eq!(focused, None);
    assert_eq!(value.as_deref(), Some("fixed"));
    assert_eq!(log_of(&mut engine), None, "no event fired");
}

/// The `-ng` aliases are the same fields, and an `x-textarea-ng` keeps its
/// value within `maxlines` through the runtime: Enter on the second line is
/// taken back and fires no `input`. The HTML names `input` and `textarea`
/// build no field, so a tap there focuses nothing.
#[test]
fn the_ng_aliases_are_fields_and_the_html_names_are_not() {
    let mut engine = field_engine("x-textarea-ng", &[("maxlines", "2")], "");
    let mut states = States::default();
    engine.dispatch_input(touch(PointerPhase::Down, 20.0, 20.0));
    engine.dispatch_input(touch(PointerPhase::Up, 20.0, 20.0));
    states.wait(&mut engine, "focusing the editor", |state| {
        state.host.is_some()
    });
    engine.dispatch_input(key("a"));
    engine.dispatch_input(enter());
    engine.dispatch_input(key("b"));
    states.wait(&mut engine, "with two lines", |state| state.value == "a\nb");
    engine.dispatch_input(enter());
    engine.dispatch_input(key("c"));
    let state = states.wait(&mut engine, "the third line taken back", |state| {
        state.value == "a\nbc"
    });
    assert_eq!(state.selection, 4..4);
    let input = |value: &str, at: u32| {
        format!(
            r#"input:{{"value":"{value}","textLength":{at},"cursor":{at},"isComposing":false,"selectionStart":{at},"selectionEnd":{at}}}"#
        )
    };
    wait_for_log(
        &mut engine,
        &[
            r#"focus:{"value":""}"#.to_owned(),
            input("a", 1),
            input("a\\n", 2),
            input("a\\nb", 3),
            input("a\\nbc", 4),
        ]
        .join("|"),
    );

    for tag in ["input", "textarea"] {
        let mut engine = field_engine(tag, &[], "");
        engine.dispatch_input(touch(PointerPhase::Down, 20.0, 20.0));
        engine.dispatch_input(touch(PointerPhase::Up, 20.0, 20.0));
        engine.dispatch_input(key("x"));
        let (shadow, focused) = engine
            .probe_document(|tree| (tree.shadow_root(node_id(HOST)), tree.focused()))
            .expect("the view answers");
        assert_eq!(shadow, None, "{tag}");
        assert_eq!(focused, None, "{tag}");
    }
}

/// An empty, focused `x-input` paints its caret: the editor's paragraph has
/// one empty line even with no value, and the caret stands on it, one line
/// tall at the content box's origin, in the field's text colour.
#[test]
fn an_empty_focused_x_input_paints_its_caret() {
    const SOURCE: &str = r"
    globalThis.renderPage = function () {
      const page = __CreatePage('card', 0);
      __SetInlineStyles(page, 'background-color:white');
      const field = __CreateElement('x-input', 0);
      __AppendElement(page, field);
      globalThis.held = [page, field];
      __SetInlineStyles(field, 'width:200px;height:40px;font-family:Ahem;font-size:10px;line-height:10px;color:black;background-color:white');
      __FlushElementTree();
    };
    ";
    let mut engine = TestViewSpec::new(SOURCE)
        .with_font("Ahem", AHEM)
        .offscreen(200.0, 40.0)
        .boot();
    let mut states = States::default();
    engine.dispatch_input(touch(PointerPhase::Down, 100.0, 20.0));
    engine.dispatch_input(touch(PointerPhase::Up, 100.0, 20.0));
    let state = states.wait(&mut engine, "focusing the editor", |state| {
        state.host.is_some()
    });
    // The editor is centred in the 40px field: a 10px line from y = 15.
    assert_eq!(
        state.caret,
        terminus::Rect::new(
            terminus::Point2D::new(0.0, 15.0),
            terminus::Size2D::new(1.0, 10.0)
        )
    );

    let pixel = |shot: &crate::view::Screenshot, x: u32, y: u32| {
        let at = ((y * shot.size.width + x) * 4) as usize;
        [
            shot.pixels[at],
            shot.pixels[at + 1],
            shot.pixels[at + 2],
            shot.pixels[at + 3],
        ]
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        engine.tick(true).expect("a frame");
        let shot = engine.capture().expect("capture the committed page");
        assert_eq!(shot.size.width, 200);
        if pixel(&shot, 0, 20) == [0, 0, 0, 255] {
            // The caret alone: one column, one line.
            assert_eq!(pixel(&shot, 0, 14), [255, 255, 255, 255], "above the line");
            assert_eq!(pixel(&shot, 0, 25), [255, 255, 255, 255], "below the line");
            assert_eq!(
                pixel(&shot, 3, 20),
                [255, 255, 255, 255],
                "beside the caret"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "no caret painted at the content origin; (0,20) is {:?}",
            pixel(&shot, 0, 20)
        );
        std::thread::yield_now();
    }
}
