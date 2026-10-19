//! The editing half of the main thread: `dom::editing` wired to the realm
//! and to the host (`docs/editable-text-design.md`, PR E2).
//!
//! `dom` computes every edit and reports the events it owes; it dispatches
//! nothing. This module is where those events reach script and where the
//! browser's default actions that edit run:
//!
//! - **An editing command** — a host's [`LynxView::edit`](crate::LynxView::edit), a key's default
//!   action, a pointer's caret placement — is [`MainThreadRuntime::apply_edit`]: `dom` prepares it,
//!   the events before the change are dispatched at the host, `dom` commits it, and the events
//!   after the change follow. No listener can stop it: no event is cancelable from script (the
//!   2026-08-17 ruling, `docs/tracking/deviations.md`).
//! - **Focus events** are drained from `dom` ([`dom::Document::take_focus_events`]) by
//!   [`MainThreadRuntime::dispatch_focus_events`] right after the one operation here that moves
//!   focus — a pointer press — so `focus` and `focusin` reach script before the events the rest of
//!   the press dispatches, as a browser's focus update runs inside its `mousedown` default action.
//!   `dom` queues focus events only from [`dom::Document::focus`] and [`dom::Document::blur`], and
//!   only this module calls them; the focus fixup rule (a removed or no longer editable host) fires
//!   nothing.
//! - **Key events** go to the focused element when there is one, and a `keydown` at a focused host
//!   runs the keyboard's editing action ([`key_command`]).
//! - **Pointer events** over an editing host place the caret. A mouse's primary press focuses the
//!   host and places it ([`Command::SelectFromPoint`]), a second and third quick press select the
//!   word and the line, and a drag with the button held extends the selection. A finger or a pen
//!   does the same on the tap — the `click` — rather than the press, as touch browsers do, so a
//!   scroll that begins on a host does not focus it; it never drags a selection.
//! - **The painter routes those names by state.** It forwards an event only when its name is in the
//!   published routed set, so the slot routes names there on an editing account of its own
//!   ([`EditingRoute`]), one more term of what the slot routes beside the listener table and the
//!   components' observed names, published through the same path an observed name's change takes
//!   (`DocumentSlot::change_names`): `pointerdown` and `click` while the document has an editing
//!   host, `keydown` while an element is focused (both from the page's epilogue,
//!   [`super::DocumentSlot::sync_editing_routes`]), and `pointermove`, `pointerup` and
//!   `pointercancel` only while a mouse drag is held, opened by the press and closed when the drag
//!   ends ([`MainThreadRuntime::set_pressed`]). An event routed only on that account calls no
//!   listener — the kernel's dispatch builds no path for a name nothing hears — and
//!   [`MainThreadRuntime::deliver_input_event`] runs the default action whenever the account routes
//!   the name.
//! - **[`TextInputState`](dom::editing::TextInputState)** is compared after each epilogue's commit
//!   and published to the host when it changed ([`MainThreadRuntime::publish_text_input_state`]).

use std::sync::Arc;

use dom::editing::{Command, Direction, EditEvent, EditEventKind, Granularity};
use dom::event::EventDetail;
use dom::input::{Modifiers, PointerId, PointerKind};

use super::{
    CLICK_EVENT, DOUBLE_CLICK_MILLISECONDS, ListenerFailures, MainThreadError, MainThreadRuntime,
    ScriptRuntime,
};
use crate::editing::TextInputSource;
use crate::link::{InputEventPayload, ViewNotice};
use crate::main::tree::text_field::SELECTION_CHANGE;
use crate::main::tree::{LynxDocument, focus_delegate};

/// One group of event names the painter routes to this thread on the
/// editing account, whether or not the realm listens for them, each while
/// the state that needs it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditingRoute {
    /// `pointerdown` and `click`, while the document has an editing host:
    /// a mouse press and a touch or pen tap are what focus a host and place
    /// its caret, and a press outside every host is what blurs one.
    Hosts,
    /// `keydown`, while an element is focused: the keyboard's editing
    /// actions need a focused host.
    Focus,
    /// `pointermove`, `pointerup` and `pointercancel`, while a mouse drag
    /// that placed a caret is held: the moves extend the selection and the
    /// release or cancellation ends the drag.
    Drag,
}

impl EditingRoute {
    pub(super) const ALL: [Self; 3] = [Self::Hosts, Self::Focus, Self::Drag];

    /// The names the route holds open.
    pub(super) const fn names(self) -> &'static [&'static str] {
        match self {
            Self::Hosts => &["pointerdown", "click"],
            Self::Focus => &["keydown"],
            Self::Drag => &["pointermove", "pointerup", "pointercancel"],
        }
    }

    /// The route's bit in `DocumentSlot::editing_routes`.
    pub(super) const fn bit(self) -> u8 {
        match self {
            Self::Hosts => 1,
            Self::Focus => 2,
            Self::Drag => 4,
        }
    }
}

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

/// Whether `node` is inside a shadow tree. Every shadow tree here is a UA
/// one, built by a component (no script can attach one), and an editing
/// host inside one is that component's own editor: an `x-input`'s
/// `contenteditable`, whose input, composition and focus events stay in its
/// tree as a browser's `<input>` keeps its inner editor's, and which the
/// component re-fires at its host in Lynx's shape
/// ([`crate::main::tree::text_field`]).
fn is_shadow_content(document: &LynxDocument, node: dom::NodeId) -> bool {
    document.shadow_root_of(node).is_some()
}

/// Whether the text a key typed is text to insert: non-empty and free of
/// control characters. A platform reports Enter's `"\r"`, Tab's `"\t"`,
/// Backspace's `"\u{8}"` and Escape's `"\u{1b}"` as typed text too (winit's
/// `KeyEvent.text` does); none of them is inserted as it is.
fn is_insertable(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

/// The browser's editing default action for a `keydown` at a focused
/// editing host, or `None` for a key that has none here.
///
/// With [`TextInputSource::Keys`] (every host but the browser's):
///
/// - The text a key typed (`text`, which the host reports with the key: never `key`) is inserted,
///   unless ctrl or meta is held (a shortcut, not text) or it is a control character. A host whose
///   platform delivers typed text through its input method reports `None` here and sends the text
///   as an input-method commit, so it is inserted once.
/// - `"Enter"` inserts a line break; `"Backspace"` and `"Delete"` delete the character before or
///   after the caret, or the word with alt; with ctrl or meta held they do nothing.
///
/// With [`TextInputSource::Commands`] the host's text proxy reports every insertion, deletion and
/// line break as an input event of its own (a browser `<textarea>`'s `beforeinput`), which arrives
/// as a [`Command`], so the two items above do nothing — a Backspace deletes once, by its command —
/// and only the items below run.
///
/// - `"ArrowLeft"` and `"ArrowRight"` move by a character, by a word with alt, to the line's edge
///   with meta; `"ArrowUp"` and `"ArrowDown"` by a line, or to the value's start or end with meta;
///   `"Home"` and `"End"` to the line's edge. Shift extends the selection instead of moving the
///   caret.
/// - Ctrl-A or meta-A selects everything. Copy, cut and paste do nothing (clipboard access is
///   deferred), and so do `"Process"` (a key the input method consumed: its text arrives as an
///   input-method command) and `"Dead"`.
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
    /// editing default action it has. What the page applies a
    /// [`crate::link::ToMain::DispatchEvent`] with.
    ///
    /// A key event goes to the focused element when there is one, and to
    /// the target the painter named (the document element) otherwise. The
    /// dispatch is the kernel's ([`Self::dispatch_input_event`]), which
    /// builds no path for a name no realm and no component hears — one the
    /// editing account alone routes. The default action is gated on that
    /// account itself (`DocumentSlot::editing_routes`), not on the dispatch:
    /// it runs whenever the account routes the name, heard or not, and
    /// whatever a listener did, since no event is cancelable from script.
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
        if let Some(release) = release {
            if let Err(failed) =
                self.dispatch_input_event(js_runtime, target, name, payload, Some(release))
            {
                failures.extend(failed.into_errors());
            }
            // A finger's or a pen's tap is its press, as a touch browser
            // focuses and places the caret on the tap: at the release, where
            // the tap landed.
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
        if let Err(failed) = self.dispatch_input_event(js_runtime, target, name, payload, None) {
            failures.extend(failed.into_errors());
        }
        if editing {
            self.input_default_action(js_runtime, target, name, payload, &mut failures);
        }
        failures
    }

    /// The editing default actions of a routed event: the keyboard's
    /// editing commands after a `keydown`, the caret placement and selection
    /// drag after the pointer events. A pointer's release or cancellation
    /// ends its drag.
    fn input_default_action(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
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
                // Released without a `pointerup` reaching here, or the focus
                // moved on (a listener's doing): the drag is over.
                let focused = self.slot.borrow().document_ref().focused();
                if buttons & 1 == 0 || focused != Some(host) {
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

    /// A mouse's primary press, or a finger's or pen's tap:
    /// inside an editing host it focuses the host and places the caret —
    /// or, as the second or third in quick succession on that host, selects
    /// the word or the line — and outside one it takes the focus away, as a
    /// press on a page's unfocusable area does in a browser. A mouse press
    /// that placed a caret starts a drag.
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
            // A press in a text field outside its editor — its padding,
            // above or below its line — focuses the editor.
            let host =
                editing_host_of(document, target).or_else(|| focus_delegate(document, target));
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
            self.set_pressed(Some((pointer, host)));
        }
    }

    /// Records the mouse drag a press started, or its end, and opens or
    /// closes [`EditingRoute::Drag`] with it: the drag's moves and its
    /// release reach this thread while it is held, and nothing of a mouse
    /// that is only hovering does.
    ///
    /// The routed-name change is written inside this job and seen by the
    /// painter at its next routing pass (`DocumentSlot::set_editing_route`,
    /// through `DocumentSlot::change_names`), so a move the
    /// painter decided before the press ran here was dropped there: the
    /// first move that crosses extends to its own position, which catches
    /// the selection up. A release decided that early never crosses either;
    /// the drag then ends at the next move that reports the button up or
    /// the focus moved, or at the next press, which is routed whenever the
    /// document has a host.
    pub(super) fn set_pressed(&mut self, pressed: Option<(PointerId, dom::NodeId)>) {
        self.pointer_editing.pressed = pressed;
        self.slot
            .borrow_mut()
            .set_editing_route(EditingRoute::Drag, pressed.is_some());
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
    /// change are dispatched at the host in order, then the change is
    /// committed and the events after it are dispatched. No listener can
    /// stop the change: none of the events is cancelable from script (the
    /// 2026-08-17 ruling, `docs/tracking/deviations.md`). A command with
    /// nothing focused, or one the host refuses, does nothing; so does a
    /// commit a listener made stale by moving the focus or rewriting the
    /// value (last write wins).
    ///
    /// Returns what the listeners threw; a listener that throws stops
    /// nothing either.
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
            if let Err(failed) = self.dispatch_edit_event(js_runtime, host, event, timestamp) {
                failures.extend(failed.into_errors());
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
            if let Err(failed) = self.dispatch_edit_event(js_runtime, host, event, timestamp) {
                failures.extend(failed.into_errors());
            }
        }
        // The engine's own `selectionchange` at the host, after every
        // command that applied: no realm hears it (`selectionchange` is not
        // built for script); the text field component reads its `selection`
        // event from it. One set lookup when no component observes it.
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

    /// What a host's [`crate::link::ToMain::TextInputSource`] sets: what a
    /// `keydown`'s editing default action does from the next key on.
    pub(crate) fn set_text_input_source(&mut self, source: TextInputSource) {
        self.text_input_source = source;
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
    /// UI Events fire all five, and not cancelable (the 2026-08-17 ruling) —
    /// except that a host inside a UA shadow tree (an `x-input`'s editor)
    /// dispatches it non-composed ([`is_shadow_content`]). An event nothing
    /// hears builds no path ([`Self::dispatch`]).
    fn dispatch_edit_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        host: dom::NodeId,
        event: &EditEvent,
        timestamp: f64,
    ) -> Result<(), ListenerFailures> {
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
        )
        .map(|_| ())
    }

    /// Dispatches the focus events `dom` has queued, in the order they
    /// formed: `blur` and `focusout` at the element that lost focus, then
    /// `focus` and `focusin` at the one that gained it, each naming the
    /// other as its `relatedTarget`. All four are composed — but for an
    /// element inside a UA shadow tree ([`is_shadow_content`]); `focusin`
    /// and `focusout` bubble. An event nothing hears builds no path
    /// ([`Self::dispatch`]).
    ///
    /// Called right after a press, and from the entry that delivers
    /// component events, for the focus a UI method moved.
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
            if let Err(error) = self.dispatch(
                js_runtime,
                event.target,
                event.kind.name(),
                event.kind.bubbles(),
                composed,
                timestamp,
                &detail,
                &[],
            ) {
                failures.extend(error.into_errors());
            }
        }
        failures
    }

    /// Opens or closes [`EditingRoute::Hosts`] as the document gains its
    /// first editing host or loses its last, and [`EditingRoute::Focus`] as
    /// an element gains or loses focus. Asked by the page's epilogue, once
    /// per entry, before the commit that publishes the frame those hosts are
    /// in.
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

    /// [`super::key_command`] for a host that reports typed text with its
    /// keys, which every test below but the last is about.
    fn key_command(key: &str, text: Option<&str>, modifiers: Modifiers) -> Option<Command> {
        super::key_command(key, text, modifiers, TextInputSource::Keys)
    }

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
    fn the_typed_text_is_inserted_and_never_the_key() {
        for text in ["x", "X", " ", "1", "é", "e\u{301}", "😀", "👩‍👩‍👧", "中", "ab"]
        {
            assert_eq!(
                key_command("Unidentified", Some(text), NONE),
                Some(Command::Insert(text.to_owned())),
                "{text:?}"
            );
        }
        // A key whose text the platform delivers another way (its input
        // method) types nothing here, whatever its `key`.
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
        // The text a platform reports for the named keys is control
        // characters, which are not inserted; Enter and Backspace keep their
        // own commands.
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

    /// A host whose text proxy reports insertions, deletions and line
    /// breaks as commands of their own gets only movement and select-all
    /// from a `keydown`: its Backspace deletes once, by the proxy's command.
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
