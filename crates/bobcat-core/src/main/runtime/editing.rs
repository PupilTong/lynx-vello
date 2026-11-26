//! The editing half of the main thread: `terminus::link::editing` wired to the realm
//! and to the host (`docs/editable-text-design.md`, PR E2).
//!
//! `terminus` computes every edit and reports the events it owes; it dispatches
//! nothing. This module is where those events reach script and where the
//! browser's default actions that edit run:
//!
//! - **An editing command** — a host's [`LynxView::edit`](crate::LynxView::edit), a key's default
//!   action, a pointer's caret placement — is [`MainThreadRuntime::apply_edit`]: `terminus`
//!   prepares it, the events before the change are dispatched at the host, `terminus` commits it,
//!   and the events after the change follow. No listener can stop it: no event is cancelable from
//!   script (the 2026-08-17 ruling, `docs/tracking/deviations.md`).
//! - **Focus events** are drained from `terminus` ([`terminus::link::Document::take_focus_events`])
//!   by [`MainThreadRuntime::dispatch_focus_events`] right after the one operation here that moves
//!   focus — a pointer press — so `focus` and `focusin` reach script before the events the rest of
//!   the press dispatches, as a browser's focus update runs inside its `mousedown` default action.
//!   `terminus` queues focus events only from [`terminus::link::Document::focus`] and
//!   [`terminus::link::Document::blur`], and only this module calls them; the focus fixup rule (a
//!   removed or no longer editable host) fires nothing.
//! - **`selectionchange`** is two events of one name. The engine-only one at the host, after every
//!   command [`MainThreadRuntime::apply_edit`] applied, reaches the text field component alone. The
//!   W3C one is separate: `terminus` sets its flag whenever the document's selection changes
//!   ([`terminus::link::Document::has_pending_selectionchange`]) — focus, blur, a command, a value
//!   write — and the entry that delivers component events fires it at the document node
//!   ([`MainThreadRuntime::dispatch_component_events`]), followed by the `text` component's Lynx
//!   `selectionchange` derived from it.
//! - **Key events** go to the focused element when there is one, and a `keydown` at a focused host
//!   runs the keyboard's editing action ([`key_command`]).
//! - **Pointer events** over an editing host place the caret. A mouse's primary press focuses the
//!   host and places it ([`Command::SelectFromPoint`]), a second and third quick press select the
//!   word and the line, and a drag with the button held extends the selection. A finger or a pen
//!   does the same on the tap — the `click` — rather than the press, as touch browsers do, so a
//!   scroll that begins on a host does not focus it; it never drags a selection.
//! - **Pointer events over a selectable element** — one whose used `user-select` is not `none`, a
//!   `<text text-selection>` under the UA sheet, which
//!   [`terminus::link::Document::selectable_root_of`] finds — select its text without focusing it,
//!   as a browser does: the same press, tap, count and drag start the document's selection there
//!   ([`terminus::link::Document::begin_selection`]) and then run the same command, blurring a
//!   focused host first. A press or a tap that is in no editing host and no selectable element
//!   blurs the focused host and clears the selection. A finger's or a pen's long press selects
//!   nothing: touch selection (a touch browser's long press with handles) is not built, the
//!   supported platforms having no touch screen, and routing `longpress` on this account would make
//!   the painter suppress every long hold's `tap` on the page (its gate is name-level).
//! - **The painter routes those names by state.** It forwards an event only when its name is in the
//!   published routed set, so the slot routes names there on an editing account of its own
//!   ([`EditingRoute`]), one more term of what the slot routes beside the listener table and the
//!   components' observed names, published through the same path an observed name's change takes
//!   (`DocumentSlot::change_names`): `pointerdown` and `click` from the page's first epilogue on —
//!   `user-select` resolves to `text` at the page, so there is no document state that says nothing
//!   can be selected, and a press outside everything is what blurs and clears — `keydown` while an
//!   element is focused (both from the page's epilogue,
//!   [`super::DocumentSlot::sync_editing_routes`]), and `pointermove`, `pointerup` and
//!   `pointercancel` only while a mouse drag is held, opened by the press and closed when the drag
//!   ends ([`MainThreadRuntime::set_pressed`]). An event routed only on that account calls no
//!   listener — the kernel's dispatch builds no path for a name nothing hears — and
//!   [`MainThreadRuntime::deliver_input_event`] runs the default action whenever the account routes
//!   the name.
//! - **[`TextInputState`](terminus::link::editing::TextInputState)** is compared after each
//!   epilogue's commit and published to the host when it changed
//!   ([`MainThreadRuntime::publish_text_input_state`]).

use std::sync::Arc;

use terminus::link::editing::{Command, Direction, EditEvent, EditEventKind, Granularity};
use terminus::link::event::EventDetail;
use terminus::link::input::{Modifiers, PointerId, PointerKind};

use super::{
    CLICK_EVENT, DOUBLE_CLICK_MILLISECONDS, DocumentSlot, MainThreadError, MainThreadRuntime,
    ScriptRuntime,
};
use crate::editing::TextInputSource;
use crate::link::{InputEventPayload, ViewNotice};
use crate::main::tree::{LynxDocument, SELECTION_CHANGE, focus_delegate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditingRoute {
    Pointer,
    Focus,
    Drag,
}

impl EditingRoute {
    pub(super) const ALL: [Self; 3] = [Self::Pointer, Self::Focus, Self::Drag];

    pub(super) const fn names(self) -> &'static [&'static str] {
        match self {
            Self::Pointer => &["pointerdown", "click"],
            Self::Focus => &["keydown"],
            Self::Drag => &["pointermove", "pointerup", "pointercancel"],
        }
    }

    pub(super) const fn bit(self) -> u8 {
        match self {
            Self::Pointer => 1,
            Self::Focus => 2,
            Self::Drag => 4,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct PointerEditing {
    pressed: Option<(PointerId, terminus::link::NodeId)>,
    last_press: Option<(terminus::link::NodeId, f64, u32)>,
}

fn editing_host_of(
    document: &LynxDocument,
    node: terminus::link::NodeId,
) -> Option<terminus::link::NodeId> {
    let mut host = None;
    let mut current = Some(node);
    while let Some(id) = current {
        let node = document.get(id)?;
        if node.is_element() {
            match node.attribute("contenteditable") {
                Some(value) if value.eq_ignore_ascii_case("false") => break,
                Some(_) => host = Some(id),
                None => {}
            }
        }
        current = node.parent_id();
    }
    host.filter(|&host| document.is_editing_host(host))
}

fn is_shadow_content(document: &LynxDocument, node: terminus::link::NodeId) -> bool {
    document.shadow_root_of(node).is_some()
}

fn is_insertable(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

pub(super) fn key_command(
    key: &str,
    text: Option<&str>,
    modifiers: Modifiers,
    source: TextInputSource,
) -> Option<Command> {
    let shortcut = modifiers.ctrl || modifiers.meta;
    let movement = |granularity, direction| {
        Some(Command::Move {
            granularity,
            direction,
            extend: modifiers.shift,
        })
    };
    let horizontal = |direction| {
        let granularity = if modifiers.meta {
            Granularity::LineBoundary
        } else if modifiers.alt {
            Granularity::Word
        } else {
            Granularity::Character
        };
        movement(granularity, direction)
    };
    let vertical = |direction| {
        let granularity = if modifiers.meta {
            Granularity::Document
        } else {
            Granularity::Line
        };
        movement(granularity, direction)
    };
    match key {
        "ArrowLeft" => horizontal(Direction::Backward),
        "ArrowRight" => horizontal(Direction::Forward),
        "ArrowUp" => vertical(Direction::Backward),
        "ArrowDown" => vertical(Direction::Forward),
        "Home" => movement(Granularity::LineBoundary, Direction::Backward),
        "End" => movement(Granularity::LineBoundary, Direction::Forward),
        "a" | "A" if shortcut => Some(Command::SelectAll),
        _ if shortcut || source == TextInputSource::Commands => None,
        "Enter" => Some(Command::InsertLineBreak),
        "Backspace" if modifiers.alt => Some(Command::DeleteWordBackward),
        "Backspace" => Some(Command::DeleteBackward),
        "Delete" if modifiers.alt => Some(Command::DeleteWordForward),
        "Delete" => Some(Command::DeleteForward),
        _ => text
            .filter(|text| is_insertable(text))
            .map(|text| Command::Insert(text.to_owned())),
    }
}

fn edit_detail(event: &EditEvent) -> EventDetail {
    match event.kind {
        EditEventKind::BeforeInput | EditEventKind::Input => EventDetail::Input {
            input_type: event.input_type,
            data: event.data.as_deref().map(Arc::from),
            is_composing: event.is_composing,
        },
        _ => EventDetail::Composition {
            data: Arc::from(event.data.as_deref().unwrap_or_default()),
        },
    }
}

impl MainThreadRuntime {
    pub(crate) fn deliver_input_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: terminus::link::NodeId,
        name: &str,
        payload: &InputEventPayload,
        release: Option<terminus::link::NodeId>,
    ) -> Vec<MainThreadError> {
        let mut failures = Vec::new();
        if let Some(release) = release {
            self.dispatch_input_event(
                js_runtime,
                target,
                name,
                payload,
                Some(release),
                &mut failures,
            );
            if self.slot.borrow().editing_routes(CLICK_EVENT)
                && let EventDetail::Pointer {
                    position,
                    id,
                    device: device @ (PointerKind::Touch | PointerKind::Pen),
                    ..
                } = payload.detail
            {
                self.press(
                    js_runtime,
                    release,
                    position,
                    id,
                    device,
                    payload.timestamp,
                    &mut failures,
                );
            }
            return failures;
        }
        let (target, editing) = {
            let slot = self.slot.borrow();
            let Some(document) = slot.document.as_ref() else {
                return failures;
            };
            let target = match &payload.detail {
                EventDetail::Key { .. } => document.focused().unwrap_or(target),
                _ => target,
            };
            (target, slot.editing_routes(name))
        };
        self.dispatch_input_event(js_runtime, target, name, payload, None, &mut failures);
        if editing {
            self.input_default_action(js_runtime, target, name, payload, &mut failures);
        }
        failures
    }

    fn input_default_action(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: terminus::link::NodeId,
        name: &str,
        payload: &InputEventPayload,
        failures: &mut Vec<MainThreadError>,
    ) {
        let timestamp = payload.timestamp;
        match (&payload.detail, name) {
            (EventDetail::Pointer { id, .. }, "pointerup" | "pointercancel")
                if self
                    .pointer_editing
                    .pressed
                    .is_some_and(|(held, _)| held == *id) =>
            {
                self.set_pressed(None);
            }
            (
                EventDetail::Key {
                    key,
                    modifiers,
                    text,
                    ..
                },
                "keydown",
            ) => {
                let focused = self.slot.borrow().document_ref().focused();
                if focused.is_some()
                    && let Some(command) =
                        key_command(key, text.as_deref(), *modifiers, self.text_input_source)
                {
                    failures.extend(self.apply_edit(js_runtime, command, timestamp));
                }
            }
            (
                EventDetail::Pointer {
                    position,
                    id,
                    device: device @ PointerKind::Mouse,
                    button: 0,
                    ..
                },
                "pointerdown",
            ) => self.press(
                js_runtime, target, *position, *id, *device, timestamp, failures,
            ),
            (
                EventDetail::Pointer {
                    position,
                    id,
                    buttons,
                    ..
                },
                "pointermove",
            ) => {
                let Some((held, host)) = self.pointer_editing.pressed else {
                    return;
                };
                if held != *id {
                    return;
                }
                let selection_host = self.slot.borrow().document_ref().selection_host();
                if buttons & 1 == 0 || selection_host != Some(host) {
                    self.set_pressed(None);
                    return;
                }
                if let Some(point) = self.host_point(host, *position) {
                    failures.extend(self.apply_edit(
                        js_runtime,
                        Command::SelectFromPoint {
                            point,
                            extend: true,
                        },
                        timestamp,
                    ));
                }
            }
            _ => {}
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one press's facts, read out of its detail by the caller"
    )]
    fn press(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: terminus::link::NodeId,
        position: terminus::link::Point2D<f32>,
        pointer: PointerId,
        device: PointerKind,
        timestamp: f64,
        failures: &mut Vec<MainThreadError>,
    ) {
        let host = {
            let mut slot = self.slot.borrow_mut();
            let document = slot.document_mut();
            if document.get(target).is_none() {
                return;
            }
            let host =
                editing_host_of(document, target).or_else(|| focus_delegate(document, target));
            if let Some(host) = host {
                document.focus(host);
                Some(host)
            } else if let Some(root) = document.selectable_root_of(target) {
                document.begin_selection(root);
                Some(root)
            } else {
                document.blur();
                document.clear_selection();
                None
            }
        };
        failures.extend(self.dispatch_focus_events(js_runtime, timestamp));
        self.set_pressed(None);
        let Some(host) = host else {
            self.pointer_editing.last_press = None;
            return;
        };
        let count = match self.pointer_editing.last_press {
            Some((last, then, count))
                if last == host && timestamp - then <= DOUBLE_CLICK_MILLISECONDS =>
            {
                count + 1
            }
            _ => 1,
        };
        self.pointer_editing.last_press = Some((host, timestamp, count));
        if self.slot.borrow().document_ref().selection_host() != Some(host) {
            return;
        }
        let Some(point) = self.host_point(host, position) else {
            return;
        };
        let command = match count {
            1 => Command::SelectFromPoint {
                point,
                extend: false,
            },
            2 => Command::SelectWordAtPoint { point },
            _ => Command::SelectLineAtPoint { point },
        };
        failures.extend(self.apply_edit(js_runtime, command, timestamp));
        if device == PointerKind::Mouse {
            self.set_pressed(Some((pointer, host)));
        }
    }

    pub(super) fn set_pressed(&mut self, pressed: Option<(PointerId, terminus::link::NodeId)>) {
        self.pointer_editing.pressed = pressed;
        self.slot
            .borrow_mut()
            .set_editing_route(EditingRoute::Drag, pressed.is_some());
    }

    fn host_point(
        &self,
        host: terminus::link::NodeId,
        position: terminus::link::Point2D<f32>,
    ) -> Option<terminus::link::Point2D<f32>> {
        let origin = self
            .slot
            .borrow()
            .document_ref()
            .bounding_client_rect(host)?
            .origin;
        Some(terminus::link::Point2D::new(
            position.x - origin.x,
            position.y - origin.y,
        ))
    }

    pub(crate) fn apply_edit(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        command: Command,
        timestamp: f64,
    ) -> Vec<MainThreadError> {
        let mut failures = Vec::new();
        let Some(edit) = self
            .slot
            .borrow_mut()
            .document_mut()
            .prepare_editing(command)
        else {
            return failures;
        };
        let host = edit.host();
        let events = edit.events().to_vec();
        let (before, after) = events.split_at(edit.commit_index());
        for event in before {
            self.dispatch_edit_event(js_runtime, host, event, timestamp, &mut failures);
        }
        if !self
            .slot
            .borrow_mut()
            .document_mut()
            .commit_editing(edit)
            .applied
        {
            return failures;
        }
        for event in after {
            self.dispatch_edit_event(js_runtime, host, event, timestamp, &mut failures);
        }
        self.slot
            .borrow_mut()
            .document_mut()
            .dispatch_element_event(
                host,
                SELECTION_CHANGE,
                &EventDetail::None,
                false,
                false,
                timestamp,
            );
        failures
    }

    pub(crate) fn set_text_input_source(&mut self, source: TextInputSource) {
        self.text_input_source = source;
    }

    pub(crate) fn apply_host_edit(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        command: Command,
    ) -> Vec<MainThreadError> {
        let timestamp = self.timeline_milliseconds();
        self.apply_edit(js_runtime, command, timestamp)
    }

    fn dispatch_edit_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        host: terminus::link::NodeId,
        event: &EditEvent,
        timestamp: f64,
        failures: &mut Vec<MainThreadError>,
    ) {
        let composed = !is_shadow_content(self.slot.borrow().document_ref(), host);
        let detail = edit_detail(event);
        self.dispatch(
            js_runtime,
            host,
            event.kind.name(),
            true,
            composed,
            timestamp,
            &detail,
            &[],
            failures,
        );
    }

    pub(crate) fn dispatch_focus_events(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        timestamp: f64,
    ) -> Vec<MainThreadError> {
        let events = self.slot.borrow_mut().document_mut().take_focus_events();
        let mut failures = Vec::new();
        for event in events {
            let composed = !is_shadow_content(self.slot.borrow().document_ref(), event.target);
            let detail = EventDetail::Focus {
                related_target: event.related_target,
            };
            self.dispatch(
                js_runtime,
                event.target,
                event.kind.name(),
                event.kind.bubbles(),
                composed,
                timestamp,
                &detail,
                &[],
                &mut failures,
            );
        }
        failures
    }

    #[cfg(test)]
    pub(crate) fn sync_editing_routes(&mut self) {
        self.slot.borrow_mut().sync_editing_routes();
    }
}

impl DocumentSlot {
    pub(super) fn publish_text_input_state(
        &self,
        published: &mut terminus::link::editing::TextInputState,
    ) {
        let Some(document) = self.document.as_ref() else {
            return;
        };
        if document.focused().is_none() && published.host.is_none() {
            return;
        }
        let state = document.text_input_state();
        if state == *published {
            return;
        }
        *published = state.clone();
        self.outbox
            .notify(ViewNotice::TextInputState(Box::new(state)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        alt: false,
        ctrl: false,
        meta: false,
        shift: false,
    };
    const ALT: Modifiers = Modifiers { alt: true, ..NONE };
    const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };
    const META: Modifiers = Modifiers { meta: true, ..NONE };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..NONE
    };

    fn key_command(key: &str, text: Option<&str>, modifiers: Modifiers) -> Option<Command> {
        super::key_command(key, text, modifiers, TextInputSource::Keys)
    }

    fn both(first: Modifiers, second: Modifiers) -> Modifiers {
        Modifiers {
            alt: first.alt || second.alt,
            ctrl: first.ctrl || second.ctrl,
            meta: first.meta || second.meta,
            shift: first.shift || second.shift,
        }
    }

    #[test]
    fn the_typed_text_is_inserted_and_never_the_key() {
        for text in ["x", "X", " ", "1", "é", "e\u{301}", "😀", "👩‍👩‍👧", "中", "ab"]
        {
            assert_eq!(
                key_command("Unidentified", Some(text), NONE),
                Some(Command::Insert(text.to_owned())),
                "{text:?}"
            );
        }
        for key in [
            "x",
            "é",
            "中",
            "Process",
            "Dead",
            "Unidentified",
            "Tab",
            "F1",
            "",
        ] {
            assert_eq!(key_command(key, None, NONE), None, "{key:?}");
        }
        assert_eq!(key_command("Tab", Some("\t"), NONE), None);
        assert_eq!(key_command("Escape", Some("\u{1b}"), NONE), None);
        assert_eq!(key_command("Unidentified", Some("\u{7f}"), NONE), None);
        assert_eq!(
            key_command("Enter", Some("\r"), NONE),
            Some(Command::InsertLineBreak)
        );
        assert_eq!(
            key_command("Backspace", Some("\u{8}"), NONE),
            Some(Command::DeleteBackward)
        );
        assert_eq!(
            key_command("å", Some("å"), ALT),
            Some(Command::Insert("å".to_owned())),
            "Option types text on macOS"
        );
        for shortcut in [CTRL, META] {
            assert_eq!(key_command("x", Some("x"), shortcut), None);
            assert_eq!(
                key_command("c", Some("c"), shortcut),
                None,
                "copy is deferred"
            );
            assert_eq!(
                key_command("v", Some("v"), shortcut),
                None,
                "paste is deferred"
            );
            assert_eq!(
                key_command("a", Some("a"), shortcut),
                Some(Command::SelectAll)
            );
            assert_eq!(key_command("a", None, shortcut), Some(Command::SelectAll));
            assert_eq!(key_command("Backspace", Some("\u{8}"), shortcut), None);
        }
    }

    #[test]
    fn editing_keys_map_to_their_commands() {
        assert_eq!(
            key_command("Enter", None, NONE),
            Some(Command::InsertLineBreak)
        );
        assert_eq!(
            key_command("Backspace", None, NONE),
            Some(Command::DeleteBackward)
        );
        assert_eq!(
            key_command("Backspace", None, ALT),
            Some(Command::DeleteWordBackward)
        );
        assert_eq!(
            key_command("Delete", None, NONE),
            Some(Command::DeleteForward)
        );
        assert_eq!(
            key_command("Delete", None, ALT),
            Some(Command::DeleteWordForward)
        );
        let moved = |granularity, direction, extend| {
            Some(Command::Move {
                granularity,
                direction,
                extend,
            })
        };
        assert_eq!(
            key_command("ArrowLeft", None, NONE),
            moved(Granularity::Character, Direction::Backward, false)
        );
        assert_eq!(
            key_command("ArrowRight", None, SHIFT),
            moved(Granularity::Character, Direction::Forward, true)
        );
        assert_eq!(
            key_command("ArrowLeft", None, both(ALT, SHIFT)),
            moved(Granularity::Word, Direction::Backward, true)
        );
        assert_eq!(
            key_command("ArrowRight", None, META),
            moved(Granularity::LineBoundary, Direction::Forward, false)
        );
        assert_eq!(
            key_command("ArrowUp", None, NONE),
            moved(Granularity::Line, Direction::Backward, false)
        );
        assert_eq!(
            key_command("ArrowDown", None, both(META, SHIFT)),
            moved(Granularity::Document, Direction::Forward, true)
        );
        assert_eq!(
            key_command("Home", None, NONE),
            moved(Granularity::LineBoundary, Direction::Backward, false)
        );
        assert_eq!(
            key_command("End", None, SHIFT),
            moved(Granularity::LineBoundary, Direction::Forward, true)
        );
    }

    #[test]
    fn a_host_that_sends_commands_gets_only_movement_and_select_all_from_keys() {
        let commands = |key, text, modifiers| {
            super::key_command(key, text, modifiers, TextInputSource::Commands)
        };
        for (key, text, modifiers) in [
            ("x", Some("x"), NONE),
            ("Unidentified", Some("中"), NONE),
            ("Enter", Some("\r"), NONE),
            ("Enter", None, NONE),
            ("Backspace", Some("\u{8}"), NONE),
            ("Backspace", None, ALT),
            ("Delete", None, NONE),
            ("Delete", None, ALT),
            ("v", Some("v"), META),
        ] {
            assert_eq!(commands(key, text, modifiers), None, "{key:?} {text:?}");
        }
        for key in [
            "ArrowLeft",
            "ArrowRight",
            "ArrowUp",
            "ArrowDown",
            "Home",
            "End",
        ] {
            for modifiers in [NONE, SHIFT, ALT, META, both(META, SHIFT)] {
                assert_eq!(
                    commands(key, None, modifiers),
                    key_command(key, None, modifiers),
                    "{key:?} moves alike"
                );
                assert!(commands(key, None, modifiers).is_some(), "{key:?}");
            }
        }
        for shortcut in [CTRL, META] {
            assert_eq!(commands("a", None, shortcut), Some(Command::SelectAll));
        }
    }
}
