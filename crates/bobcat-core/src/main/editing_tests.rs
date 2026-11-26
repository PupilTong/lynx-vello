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

use terminus::link::Point2D;
use terminus::link::editing::{Command, TextInputState};
use terminus::link::input::{InputEvent, KeyPhase, Modifiers, PointerKind, PointerPhase};

use crate::EngineEvent;
use crate::editing::TextInputSource;
use crate::test_support::{TestEngine, TestViewSpec, wait_for_boot};

const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");

const HOST: u64 = 3;

fn node_id(bits: u64) -> terminus::link::NodeId {
    terminus::link::NodeId::from_bits(bits).expect("a well-formed packed handle")
}

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
                .and_then(|host| host.attribute("log").map(std::borrow::Cow::into_owned))
        })
        .flatten()
}

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

fn key(text: &str) -> InputEvent {
    InputEvent::key(KeyPhase::Down, text, "", false, Modifiers::default())
        .with_text(Some(text.into()))
}

fn focus_host(engine: &mut TestEngine, states: &mut States) {
    engine.dispatch_input(touch(PointerPhase::Down, 20.0, 20.0));
    engine.dispatch_input(touch(PointerPhase::Up, 20.0, 20.0));
    states.wait(engine, "focusing the host", |state| {
        state.host == Some(node_id(HOST))
    });
}

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

const NO_CANCELLATION: &str = "if ('preventDefault' in event || 'cancelable' in event \
     || 'defaultPrevented' in event) throw new Error('a cancellation member');";

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
        terminus::link::Size2D::new(200.0, 40.0),
        "the host's border box"
    );

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
    let state = states.wait(&mut engine, "with Ahem's 100px", |state| {
        (state.caret.min_x() - state.editable_bounds.min_x() - 100.0).abs() < f32::EPSILON
    });
    let origin = state.editable_bounds.origin;

    engine.painter.clock_mut().pin(10.0);
    engine.dispatch_input(mouse(PointerPhase::Down, origin.x + 45.0, origin.y + 10.0));
    states.wait(&mut engine, "with the caret at 2", |state| {
        state.selection == (2..2)
    });
    engine.dispatch_input(mouse(PointerPhase::Move, origin.x + 85.0, origin.y + 10.0));
    states.wait(&mut engine, "selecting cd", |state| {
        state.selection == (2..4)
    });
    engine.dispatch_input(mouse(PointerPhase::Up, origin.x + 85.0, origin.y + 10.0));

    engine.painter.clock_mut().pin(20.0);
    engine.dispatch_input(mouse(PointerPhase::Down, origin.x + 45.0, origin.y + 10.0));
    engine.dispatch_input(mouse(PointerPhase::Up, origin.x + 45.0, origin.y + 10.0));
    engine.painter.clock_mut().pin(20.2);
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

#[test]
fn selection_changes_fire_at_the_document_then_at_the_text() {
    let mut engine = TestViewSpec::new(SELECTION_PAGE).boot();
    let mut states = States::default();
    let mut expected = vec!["document"];

    focus_host(&mut engine, &mut states);
    wait_for_log(&mut engine, &expected.join("|"));
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

fn selectable_page(attributes: &str) -> TestEngine {
    let source = format!(
        r"
        import {{ __BobcatHandleOf }} from 'bobcat:element';
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const text = __CreateElement('text', 0);
          __AppendElement(page, text);
          __AppendElement(text, __CreateRawText('abcdef ghij'));
          globalThis.held = [page, text];
          globalThis.entries = [];
          {attributes}
          __SetInlineStyles(text, 'width:240px;height:40px;font-family:Ahem;font-size:20px;line-height:20px');
          const log = (entry) => {{
            entries.push(entry);
            __SetAttribute(text, 'log', entries.join('|'));
          }};
          __AddEventListener(text, 'selectionchange', (event) => log('text:' + event.detail.start
            + ':' + event.detail.end + ':' + event.detail.direction));
          __AddEventListener(__BobcatHandleOf(1), 'selectionchange', () => log('document'));
          __FlushElementTree();
        }};
        "
    );
    let mut engine = TestViewSpec::new(&source)
        .with_default_font("Ahem", AHEM)
        .boot();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let width = engine
            .probe_document(|tree| tree.text_block_size(node_id(HOST)).map(|size| size.width))
            .flatten();
        if width == Some(220.0) {
            return engine;
        }
        assert!(
            Instant::now() < deadline,
            "the text never measured in Ahem; last {width:?}"
        );
        std::thread::yield_now();
    }
}

fn selectable_text() -> TestEngine {
    selectable_page("__SetAttribute(text, 'text-selection', 'true');")
}

fn selection_of(engine: &mut TestEngine) -> Option<(terminus::link::NodeId, u32, u32)> {
    engine
        .probe_document(|tree| {
            tree.selection()
                .map(|selection| (selection.host, selection.anchor, selection.focus))
        })
        .flatten()
}

#[test]
fn a_mouse_press_and_drag_select_a_selectable_text() {
    let mut engine = selectable_text();
    let mut expected = vec!["document"];

    engine.painter.clock_mut().pin(10.0);
    engine.dispatch_input(mouse(PointerPhase::Down, 10.0, 10.0));
    wait_for_log(&mut engine, &expected.join("|"));
    engine.dispatch_input(mouse(PointerPhase::Move, 65.0, 10.0));
    expected.extend(["document", "text:0:3:forward"]);
    wait_for_log(&mut engine, &expected.join("|"));
    engine.dispatch_input(mouse(PointerPhase::Up, 65.0, 10.0));
    assert_eq!(
        engine.probe_document(|tree| tree.focused()).flatten(),
        None,
        "nothing is focused"
    );
    assert_eq!(selection_of(&mut engine), Some((node_id(HOST), 0, 3)));

    engine.painter.clock_mut().pin(20.0);
    engine.dispatch_input(mouse(PointerPhase::Down, 300.0, 300.0));
    engine.dispatch_input(mouse(PointerPhase::Up, 300.0, 300.0));
    expected.extend(["document", "text:-1:-1:forward"]);
    wait_for_log(&mut engine, &expected.join("|"));
    assert_eq!(selection_of(&mut engine), None);
}

#[test]
fn two_quick_presses_select_the_word_of_a_selectable_text() {
    let mut engine = selectable_text();
    engine.painter.clock_mut().pin(20.0);
    engine.dispatch_input(mouse(PointerPhase::Down, 45.0, 10.0));
    wait_for_log(&mut engine, "document");
    engine.dispatch_input(mouse(PointerPhase::Up, 45.0, 10.0));
    engine.painter.clock_mut().pin(20.2);
    engine.dispatch_input(mouse(PointerPhase::Down, 45.0, 10.0));
    engine.dispatch_input(mouse(PointerPhase::Up, 45.0, 10.0));
    wait_for_log(&mut engine, "document|document|text:0:6:forward");
}

#[test]
fn a_text_without_text_selection_is_not_selected() {
    let mut engine = selectable_page("");
    engine.painter.clock_mut().pin(10.0);
    engine.dispatch_input(mouse(PointerPhase::Down, 10.0, 10.0));
    engine.dispatch_input(mouse(PointerPhase::Move, 65.0, 10.0));
    engine.dispatch_input(mouse(PointerPhase::Up, 65.0, 10.0));
    engine.painter.clock_mut().pin(11.0);
    engine.dispatch_input(touch(PointerPhase::Down, 45.0, 10.0));
    engine.dispatch_input(touch(PointerPhase::Up, 45.0, 10.0));
    engine.probe_document(|_| ());
    assert_eq!(
        engine
            .probe_document(|tree| tree.selection_host())
            .flatten(),
        None
    );
    assert_eq!(log_of(&mut engine), None, "no selectionchange fired");
}

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
        terminus::link::editing::EnterKeyHint::Send
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
                    .map(terminus::link::Node::id)
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

#[test]
fn the_ng_aliases_are_fields() {
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
}

#[test]
fn the_lynx_tags_input_and_textarea_are_the_fields() {
    let input = |value: &str, at: u32| {
        format!(
            r#"input:{{"value":"{value}","textLength":{at},"cursor":{at},"isComposing":false,"selectionStart":{at},"selectionEnd":{at}}}"#
        )
    };
    for (tag, document_tag, attributes, log) in [
        (
            "input",
            "x-input",
            &[][..],
            vec![input("x", 1), r#"confirm:{"value":"x"}"#.to_owned()],
        ),
        (
            "textarea",
            "x-textarea",
            &[("confirm-enter", "")][..],
            vec![
                input("x", 1),
                input("x\\n", 2),
                r#"confirm:{"value":"x\n"}"#.to_owned(),
            ],
        ),
    ] {
        let mut engine = field_engine(tag, attributes, "note('tag:' + __GetTag(field));");
        let mut states = States::default();
        engine.dispatch_input(touch(PointerPhase::Down, 20.0, 20.0));
        engine.dispatch_input(touch(PointerPhase::Up, 20.0, 20.0));
        let focused = states.wait(&mut engine, "focusing the editor", |state| {
            state.host.is_some()
        });
        let (element_tag, shadow) = engine
            .probe_document(|tree| {
                let host = tree.get(node_id(HOST))?;
                Some((host.tag_name()?.to_owned(), tree.shadow_root(node_id(HOST))))
            })
            .flatten()
            .expect("the view answers with the element");
        assert_eq!(element_tag, document_tag);
        assert!(shadow.is_some(), "{tag}: the field's shadow root");
        assert_ne!(focused.host, Some(node_id(HOST)), "{tag}: the editor");
        engine.dispatch_input(key("x"));
        states.wait(&mut engine, "with x", |state| state.value == "x");
        engine.dispatch_input(enter());
        let mut expected = vec![r#"focus:{"value":""}"#.to_owned()];
        expected.extend(log);
        expected.push(format!("tag:{tag}"));
        wait_for_log(&mut engine, &expected.join("|"));
    }
}

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
    assert_eq!(
        state.caret,
        terminus::link::Rect::new(
            terminus::link::Point2D::new(0.0, 15.0),
            terminus::link::Size2D::new(1.0, 10.0)
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
