//! The editing half of the main thread: `dom::editing` wired to the realm
//! and to the host (`docs/editable-text-design.md`, PR E2).
//!
//! `dom` computes every edit and reports the events it owes; it dispatches
//! nothing. This module is where those events reach script and where the
//! browser's default actions that edit run:
//!
//! - **An editing command** — a host's [`LynxView::edit`](crate::LynxView::edit), a key's default
//!   action, a pointer's caret placement — is [`MainThreadRuntime::apply_edit`]: `dom` prepares it,
//!   the events before the change are dispatched at the host, a canceled cancelable `beforeinput`
//!   drops it, `dom` commits it, and the events after the change follow.
//! - **Focus events** are drained from `dom` ([`dom::Document::take_focus_events`]) by
//!   [`MainThreadRuntime::dispatch_focus_events`] right after the one operation here that moves
//!   focus — a pointer press — so `focus` and `focusin` reach script before the events the rest of
//!   the press dispatches, as a browser's focus update runs inside its `mousedown` default action.
//!   `dom` queues focus events only from [`dom::Document::focus`] and [`dom::Document::blur`], and
//!   only this module calls them; the focus fixup rule (a removed or no longer editable host) fires
//!   nothing.
//! - **Key events** go to the focused element when there is one, and a `keydown` nobody canceled
//!   runs the keyboard's editing action ([`key_command`]).
//! - **Pointer events** over an editing host place the caret: a primary press focuses the host and
//!   places it ([`Command::SelectFromPoint`]), a second and third quick press select the word and
//!   the line, and a mouse drag with the button held extends the selection.
//! - **The painter routes those names while the document has an editing host.** It forwards an
//!   event only when its name is in the published listener-name set, so the slot opens
//!   [`EDITING_ROUTED_EVENTS`] in that set while [`dom::Document::has_editing_hosts`] says yes and
//!   closes them when it says no, through the same edges a realm registration moves
//!   ([`super::DocumentSlot::sync_editing_routes`]). An event routed only on that account calls no
//!   listener: [`MainThreadRuntime::deliver_input_event`] skips the realm when nothing hears the
//!   name.
//! - **[`TextInputState`](dom::editing::TextInputState)** is compared after each epilogue's commit
//!   and published to the host when it changed ([`MainThreadRuntime::publish_text_input_state`]).

use std::sync::Arc;

use dom::editing::{Command, Direction, EditEvent, EditEventKind, Granularity};
use dom::event::EventDetail;
use dom::input::{Modifiers, PointerId, PointerKind};

use super::{
    DOUBLE_CLICK_MILLISECONDS, Dispatched, EventFlags, MainThreadError, MainThreadRuntime,
    ScriptRuntime,
};
use crate::link::{InputEventPayload, ViewNotice};
use crate::main::tree::LynxDocument;

/// The event names the painter routes to this thread while the document has
/// an editing host, whether or not the realm listens for them: the press,
/// the drag and the release that place the caret and extend the selection,
/// the cancellation that ends a drag, and the `keydown` the keyboard's
/// editing actions follow.
pub(super) const EDITING_ROUTED_EVENTS: [&str; 5] = [
    "pointerdown",
    "pointermove",
    "pointerup",
    "pointercancel",
    "keydown",
];

/// What the pointer's editing default actions remember between events.
#[derive(Debug, Default)]
pub(super) struct PointerEditing {
    /// The mouse pointer whose press placed the caret in this host and whose
    /// primary button is still held: its moves extend the selection.
    pressed: Option<(PointerId, dom::NodeId)>,
    /// The last primary press on an editing host, its timestamp, and how
    /// many quick presses on that host it ended: two select a word, three or
    /// more a line, as a browser's click count does.
    last_press: Option<(dom::NodeId, f64, u32)>,
}

/// The editing host `node` is in, by HTML's rule for a plain-text host:
/// the outermost element of the unbroken run of `contenteditable` elements
/// above `node` (a nested `contenteditable` is content of the outer host),
/// with a `contenteditable="false"` element ending the run (its subtree is
/// not editable). The walk stays inside `node`'s tree: a UA shadow tree's
/// editing host is found from its own content.
fn editing_host_of(document: &LynxDocument, node: dom::NodeId) -> Option<dom::NodeId> {
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

/// Whether a W3C `key` value is text a key typed rather than a named key:
/// non-empty, free of control characters, and not one of UI Events' key
/// names, every one of which is two or more ASCII letters and digits
/// (`"Enter"`, `"F1"`, `"Dead"`, `"Process"`, `"Unidentified"`). A key that
/// typed one character, or one grapheme of several code points, is text.
fn is_typed_text(key: &str) -> bool {
    let named = key.len() > 1 && key.bytes().all(|byte| byte.is_ascii_alphanumeric());
    !key.is_empty() && !named && !key.chars().any(char::is_control)
}

/// The browser's editing default action for a `keydown` at a focused
/// editing host, or `None` for a key that has none here.
///
/// - Text a key typed is inserted, unless ctrl or meta is held (a shortcut, not text).
/// - `"Enter"` inserts a line break; `"Backspace"` and `"Delete"` delete the character before or
///   after the caret, or the word with alt; with ctrl or meta held they do nothing.
/// - `"ArrowLeft"` and `"ArrowRight"` move by a character, by a word with alt, to the line's edge
///   with meta; `"ArrowUp"` and `"ArrowDown"` by a line, or to the value's start or end with meta;
///   `"Home"` and `"End"` to the line's edge. Shift extends the selection instead of moving the
///   caret.
/// - Ctrl-A or meta-A selects everything. Copy, cut and paste do nothing (clipboard access is
///   deferred), and so do `"Process"` (a key the input method consumed: its text arrives as an
///   input-method command) and `"Dead"`.
pub(super) fn key_command(key: &str, modifiers: Modifiers) -> Option<Command> {
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
        _ if shortcut => None,
        "Enter" => Some(Command::InsertLineBreak),
        "Backspace" if modifiers.alt => Some(Command::DeleteWordBackward),
        "Backspace" => Some(Command::DeleteBackward),
        "Delete" if modifiers.alt => Some(Command::DeleteWordForward),
        "Delete" => Some(Command::DeleteForward),
        _ if is_typed_text(key) => Some(Command::Insert(key.to_owned())),
        _ => None,
    }
}

/// The detail one edit event crosses with: `InputEvent`'s members for
/// `beforeinput` and `input`, `CompositionEvent.data` for the three
/// composition events.
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
    /// Delivers one routed event the painting side decided, then runs the
    /// editing default action it has, if nobody canceled it. What the page
    /// applies a [`crate::link::ToMain::DispatchEvent`] with.
    ///
    /// A key event goes to the focused element when there is one, and to
    /// the target the painter named (the document element) otherwise. An
    /// event nothing in the document hears — routed only because the
    /// document has an editing host — skips the realm and runs its default
    /// action alone.
    ///
    /// Returns what the listeners threw, in order; a listener that throws is
    /// nonfatal and its event's default action still runs.
    pub(crate) fn deliver_input_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        name: &str,
        payload: &InputEventPayload,
        release: Option<dom::NodeId>,
    ) -> Vec<MainThreadError> {
        let mut failures = Vec::new();
        if release.is_some() {
            if let Err(error) =
                self.dispatch_input_event(js_runtime, target, name, payload, release)
            {
                failures.push(error);
            }
            return failures;
        }
        let (target, heard) = {
            let slot = self.slot.borrow();
            let Some(document) = slot.document.as_ref() else {
                return failures;
            };
            let target = match &payload.detail {
                EventDetail::Key { .. } => document.focused().unwrap_or(target),
                _ => target,
            };
            (target, slot.listens(name))
        };
        let dispatched = if heard {
            self.dispatch_input_event(js_runtime, target, name, payload, None)
                .unwrap_or_else(|error| {
                    failures.push(error);
                    Dispatched::default()
                })
        } else {
            Dispatched::default()
        };
        self.input_default_action(js_runtime, target, name, payload, dispatched, &mut failures);
        failures
    }

    /// The editing default actions of a routed event: the keyboard's
    /// editing commands after a `keydown`, the caret placement and selection
    /// drag after the pointer events. Nothing runs for a canceled event,
    /// except that a pointer's release or cancellation always ends its drag.
    fn input_default_action(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        name: &str,
        payload: &InputEventPayload,
        dispatched: Dispatched,
        failures: &mut Vec<MainThreadError>,
    ) {
        let timestamp = payload.timestamp;
        match (&payload.detail, name) {
            (EventDetail::Pointer { id, .. }, "pointerup" | "pointercancel") => {
                self.pointer_editing.pressed.take_if(|(held, _)| held == id);
            }
            _ if dispatched.default_prevented => {}
            (EventDetail::Key { key, modifiers, .. }, "keydown") => {
                let focused = self.slot.borrow().document_ref().focused();
                if focused.is_some()
                    && let Some(command) = key_command(key, *modifiers)
                {
                    failures.extend(self.apply_edit(js_runtime, command, timestamp));
                }
            }
            (
                EventDetail::Pointer {
                    position,
                    id,
                    device,
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
                // Released without a `pointerup` reaching here, or the focus
                // moved on (a listener's doing): the drag is over.
                let focused = self.slot.borrow().document_ref().focused();
                if buttons & 1 == 0 || focused != Some(host) {
                    self.pointer_editing.pressed = None;
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

    /// A primary press nobody canceled: inside an editing host it focuses
    /// the host and places the caret — or, as the second or third press in
    /// quick succession on that host, selects the word or the line — and
    /// outside one it takes the focus away, as a press on a page's
    /// unfocusable area does in a browser.
    #[allow(
        clippy::too_many_arguments,
        reason = "one press's facts, read out of its detail by the caller"
    )]
    fn press(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        position: dom::Point2D<f32>,
        pointer: PointerId,
        device: PointerKind,
        timestamp: f64,
        failures: &mut Vec<MainThreadError>,
    ) {
        let host = {
            let mut slot = self.slot.borrow_mut();
            let document = slot.document_mut();
            // A target freed since the press formed decides nothing, as a
            // dispatch at it delivers nothing.
            if document.get(target).is_none() {
                return;
            }
            let host = editing_host_of(document, target);
            match host {
                Some(host) => {
                    document.focus(host);
                }
                None => {
                    document.blur();
                }
            }
            host
        };
        failures.extend(self.dispatch_focus_events(js_runtime, timestamp));
        let Some(host) = host else {
            self.pointer_editing = PointerEditing::default();
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
        self.pointer_editing.pressed = None;
        // A `focus` listener can move the focus on, and then the press
        // places nothing in a host that is no longer the focused one.
        if self.slot.borrow().document_ref().focused() != Some(host) {
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
        // Only a mouse drags a selection: a finger or a pen dragging over a
        // host is the scroll gesture, as on a touch browser.
        if device == PointerKind::Mouse {
            self.pointer_editing.pressed = Some((pointer, host));
        }
    }

    /// A viewport position in `host`'s border-box space, which the pointer
    /// commands take: the position less the host's
    /// [`bounding_client_rect`](dom::Document::bounding_client_rect) origin,
    /// from the last committed layout — the geometry the press was made
    /// against.
    fn host_point(
        &self,
        host: dom::NodeId,
        position: dom::Point2D<f32>,
    ) -> Option<dom::Point2D<f32>> {
        let origin = self
            .slot
            .borrow()
            .document_ref()
            .bounding_client_rect(host)?
            .origin;
        Some(dom::Point2D::new(
            position.x - origin.x,
            position.y - origin.y,
        ))
    }

    /// Applies one editing command to the focused editing host with the
    /// events it owes, by the prepare/commit seam: the events before the
    /// change are dispatched at the host in order, a canceled cancelable
    /// `beforeinput` drops the edit (nothing is committed and no later
    /// event fires), then the change is committed and the events after it
    /// are dispatched. A command with nothing focused, or one the host
    /// refuses, does nothing; so does a commit a listener made stale by
    /// moving the focus or rewriting the value (last write wins).
    ///
    /// Returns what the listeners threw. A listener that throws does not
    /// cancel: its event counts as not canceled, as an uncaught exception in
    /// a browser's listener does not cancel the event either.
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
            let canceled = self
                .dispatch_edit_event(js_runtime, host, event, timestamp)
                .unwrap_or_else(|error| {
                    failures.push(error);
                    false
                });
            if canceled && event.cancelable() {
                return failures;
            }
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
            if let Err(error) = self.dispatch_edit_event(js_runtime, host, event, timestamp) {
                failures.push(error);
            }
        }
        failures
    }

    /// [`Self::apply_edit`] for a host's [`crate::link::ToMain::Edit`]: the
    /// command carries no reading of the view's timeline, so its events are
    /// stamped with the last one this side was handed.
    pub(crate) fn apply_host_edit(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        command: Command,
    ) -> Vec<MainThreadError> {
        let timestamp = self.timeline_milliseconds;
        self.apply_edit(js_runtime, command, timestamp)
    }

    /// One edit event at `host`: bubbling and composed, as Input Events and
    /// UI Events fire all five, cancelable as [`EditEvent::cancelable`]
    /// says. Answers whether a listener canceled it; an event nothing hears
    /// is not dispatched at all.
    fn dispatch_edit_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        host: dom::NodeId,
        event: &EditEvent,
        timestamp: f64,
    ) -> Result<bool, MainThreadError> {
        let name = event.kind.name();
        if !self.slot.borrow().listens(name) {
            return Ok(false);
        }
        let flags = EventFlags {
            bubbles: true,
            composed: true,
            cancelable: event.cancelable(),
        };
        let detail = edit_detail(event);
        self.dispatch(js_runtime, host, name, flags, timestamp, &detail, &[])
            .map(|dispatched| dispatched.default_prevented)
    }

    /// Dispatches the focus events `dom` has queued, in the order they
    /// formed: `blur` and `focusout` at the element that lost focus, then
    /// `focus` and `focusin` at the one that gained it, each naming the
    /// other as its `relatedTarget`. All four are composed and none is
    /// cancelable; `focusin` and `focusout` bubble. An event nothing hears
    /// is not dispatched.
    pub(crate) fn dispatch_focus_events(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        timestamp: f64,
    ) -> Vec<MainThreadError> {
        let events = self.slot.borrow_mut().document_mut().take_focus_events();
        let mut failures = Vec::new();
        for event in events {
            let name = event.kind.name();
            if !self.slot.borrow().listens(name) {
                continue;
            }
            let flags = EventFlags {
                bubbles: event.kind.bubbles(),
                composed: true,
                cancelable: false,
            };
            let detail = EventDetail::Focus {
                related_target: event.related_target,
            };
            if let Err(error) = self.dispatch(
                js_runtime,
                event.target,
                name,
                flags,
                timestamp,
                &detail,
                &[],
            ) {
                failures.push(error);
            }
        }
        failures
    }

    /// Opens or closes [`EDITING_ROUTED_EVENTS`] in the painter's name set as
    /// the document gains its first editing host or loses its last. Asked by
    /// the page's epilogue, once per entry, before the commit that publishes
    /// the frame those hosts are in.
    pub(crate) fn sync_editing_routes(&mut self) {
        self.slot.borrow_mut().sync_editing_routes();
    }

    /// Publishes the focused host's [`TextInputState`](dom::editing::TextInputState) to the host
    /// when it differs from the last one published. Asked by the page's epilogue
    /// after its commit, so the geometry is the committed frame's. Nothing is
    /// computed while nothing is focused and nothing was.
    pub(crate) fn publish_text_input_state(&mut self) {
        let slot = self.slot.borrow();
        let Some(document) = slot.document.as_ref() else {
            return;
        };
        if document.focused().is_none() && self.text_input.host.is_none() {
            return;
        }
        let state = document.text_input_state();
        if state == self.text_input {
            return;
        }
        self.text_input = state.clone();
        slot.outbox
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

    /// Two modifiers held together.
    fn both(first: Modifiers, second: Modifiers) -> Modifiers {
        Modifiers {
            alt: first.alt || second.alt,
            ctrl: first.ctrl || second.ctrl,
            meta: first.meta || second.meta,
            shift: first.shift || second.shift,
        }
    }

    #[test]
    fn typed_text_is_inserted_and_named_keys_are_not() {
        for key in ["x", "X", " ", "1", "é", "e\u{301}", "😀", "👩‍👩‍👧", "中"] {
            assert_eq!(
                key_command(key, NONE),
                Some(Command::Insert(key.to_owned())),
                "{key:?}"
            );
        }
        for key in [
            "Process",
            "Dead",
            "Unidentified",
            "Tab",
            "Escape",
            "F1",
            "Shift",
            "",
        ] {
            assert_eq!(key_command(key, NONE), None, "{key:?}");
        }
        assert_eq!(key_command("\u{7f}", NONE), None, "a control character");
        assert_eq!(
            key_command("å", ALT),
            Some(Command::Insert("å".to_owned())),
            "alt types text on macOS"
        );
        for shortcut in [CTRL, META] {
            assert_eq!(key_command("x", shortcut), None);
            assert_eq!(key_command("c", shortcut), None, "copy is deferred");
            assert_eq!(key_command("v", shortcut), None, "paste is deferred");
            assert_eq!(key_command("a", shortcut), Some(Command::SelectAll));
            assert_eq!(key_command("Backspace", shortcut), None);
        }
    }

    #[test]
    fn editing_keys_map_to_their_commands() {
        assert_eq!(key_command("Enter", NONE), Some(Command::InsertLineBreak));
        assert_eq!(
            key_command("Backspace", NONE),
            Some(Command::DeleteBackward)
        );
        assert_eq!(
            key_command("Backspace", ALT),
            Some(Command::DeleteWordBackward)
        );
        assert_eq!(key_command("Delete", NONE), Some(Command::DeleteForward));
        assert_eq!(key_command("Delete", ALT), Some(Command::DeleteWordForward));
        let moved = |granularity, direction, extend| {
            Some(Command::Move {
                granularity,
                direction,
                extend,
            })
        };
        assert_eq!(
            key_command("ArrowLeft", NONE),
            moved(Granularity::Character, Direction::Backward, false)
        );
        assert_eq!(
            key_command("ArrowRight", SHIFT),
            moved(Granularity::Character, Direction::Forward, true)
        );
        assert_eq!(
            key_command("ArrowLeft", both(ALT, SHIFT)),
            moved(Granularity::Word, Direction::Backward, true)
        );
        assert_eq!(
            key_command("ArrowRight", META),
            moved(Granularity::LineBoundary, Direction::Forward, false)
        );
        assert_eq!(
            key_command("ArrowUp", NONE),
            moved(Granularity::Line, Direction::Backward, false)
        );
        assert_eq!(
            key_command("ArrowDown", both(META, SHIFT)),
            moved(Granularity::Document, Direction::Forward, true)
        );
        assert_eq!(
            key_command("Home", NONE),
            moved(Granularity::LineBoundary, Direction::Backward, false)
        );
        assert_eq!(
            key_command("End", SHIFT),
            moved(Granularity::LineBoundary, Direction::Forward, true)
        );
    }
}
