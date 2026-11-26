//! Plain-text editing: editing hosts, the focused element, the selection and
//! the IME composition over a host's committed paragraph, and the commands a
//! keyboard, a pointer and a platform input method apply to it.
//!
//! `docs/editable-text-design.md` is the design; this module is its first
//! part (PR E1). The browser standard is the model throughout: HTML's
//! `contenteditable`, its focus update steps, the Selection/caret model, and
//! the event sequences of UI Events and Input Events Level 2. `terminus`
//! dispatches nothing ([`crate::event`]), so every event a change owes is
//! *reported* — focus changes through [`Document::take_focus_events`], edit
//! events in the [`EditOutcome`] of the command that caused them — for the
//! embedder to dispatch.
//!
//! # Editing host
//!
//! An element whose `contenteditable` attribute is present and is not
//! `"false"` (ASCII case-insensitive) is an editing host
//! ([`Document::is_editing_host`]). `"true"`, `""` and `"plaintext-only"` all
//! mean plain-text editing here. Its **value** is the concatenation of its
//! direct text-node children, in content-walk order.
//!
//! Editing does not decide how the host is laid out: the host keeps the
//! layout its element kind and its `display` give it, and its text is
//! edited in whichever paragraph that is (`paragraph`): a Lynx text
//! element's own Lynx paragraph, a block container's own inline formatting
//! context, or — for a flex, grid, linear or relative host such as a
//! `view` — the inline formatting context of the anonymous block box around
//! its loose text. The embedder's UA sheet only defaults the host's
//! `white-space` to `pre-wrap`, so typed spaces and line breaks show; an
//! author may override it, and a `display: none` host simply cannot be
//! focused.
//!
//! The first edit that changes the value normalizes the host to exactly one
//! text node: the first text child takes the whole new value and the others
//! are removed from the host (unlinked, not freed: whoever minted them still
//! owns them), or a text node is created when the host has none.
//!
//! A host with an element child is **read-only** here: every command that
//! would change its value is refused ([`EditOutcome::applied`] is `false`).
//! Rich-text editing of nested elements is deferred (the design's
//! "Deferred" list).
//!
//! # Focus
//!
//! A document has at most one focused element ([`Document::focused`]). Only
//! connected editing hosts that are being rendered are focusable.
//! [`Document::focus`] and [`Document::blur`] run HTML's focus update steps
//! in their simplest form and queue `blur`, `focusout`, `focus`, `focusin`
//! ([`FocusEvent`]) in that order; the focused element matches `:focus`
//! (`ElementState::FOCUS`). When the focused element leaves the document,
//! stops being an editing host or stops being rendered (`display: none` on
//! it or an ancestor), focus is dropped with no event, as HTML's focus
//! fixup rule does.
//!
//! A host keeps its selection while it is not focused, as a browser's text
//! control keeps `selectionStart`/`selectionEnd` across a blur: losing focus,
//! with or without events, retains the session's selection as value offsets,
//! [`Document::set_host_value`] on a host that is not focused replaces it, and
//! the next [`Document::focus`] restores it, clamped to the value as it is
//! then. A host that was never focused nor written has none, and focusing it
//! places a collapsed caret at the start, as HTML's `contenteditable` focus
//! does. [`Document::host_selection`] reads either form. A host that stops
//! being an editing host keeps its retained selection, as a read-only
//! control keeps its own; freeing the host drops it.
//!
//! # The document's selection
//!
//! The Selection API gives a document one selection. Here it is the one
//! session the document holds, and the element that session is in is the
//! **selection host** ([`Document::selection_host`]): the focused editing
//! host, or a **selectable element** selected without focus. An element is
//! selectable when the used value of its `user-select` is not `none`
//! (css-ui-4 "Content Selection", [`Document::is_user_selectable`]) and it
//! establishes a committed paragraph; [`Document::selectable_root_of`]
//! finds the one a node is in. [`Document::begin_selection`] starts a session
//! in one — after blurring a focused editing host, with that host's focus
//! events, as a browser's press on content that cannot be focused blurs the
//! focused element — and [`Document::clear_selection`] ends it; focusing an
//! editing host replaces it. With no session the document has no selection
//! (a browser's `rangeCount` 0), and [`Document::selection`] answers `None`.
//! This is a simplification of the standard: a browser keeps the document's
//! ranges after a script's `blur()` and lets a selection cross elements,
//! while here a session ending — a blur, focus moving to another host, the
//! fixup rule — ends the document's selection too, and a selection stays in
//! one paragraph. A host's own retained selection
//! ([`Document::host_selection`]) is a text control's
//! `selectionStart`/`selectionEnd`, a different thing, and is not the
//! document's.
//!
//! The session's **selection text** is what its offsets index: a focused
//! host's value, or, for a selectable element, the source text of its
//! paragraph — every run of its flattened content in order, DOM text and
//! generated `content` alike, with one U+FFFC per atomic inline box (an
//! inline `image` or `view`) — so its UTF-16 units are exactly the
//! paragraph's source units. That text is rebuilt on demand by the flatten
//! walk layout runs, never stored, so a selectable element costs nothing
//! while it is not selected. A selectable element is read only: the pointer
//! and movement commands work over its paragraph, nested scopes and atoms
//! included, and every command that would change a value or compose text is
//! refused. Its session ends, with the flag set and no event, when the
//! element leaves the document, computes `user-select: none`, stops being
//! rendered or loses its paragraph, and when it is freed.
//!
//! [`Document::selection`] reports the two boundary points apart, as UTF-16
//! offsets into the selection text: `anchor` where the selection was
//! started, `focus` where it was extended to, so `focus < anchor` is a
//! backward selection.
//!
//! Every change of the document's selection sets one flag, the Selection
//! API's "has scheduled selectionchange event" (HTML keeps the same flag on
//! each document and text control): a session starting ([`Document::focus`],
//! [`Document::begin_selection`]) or ending (a blur,
//! [`Document::clear_selection`], the fixup rule), an applied command that
//! changed the value or left a selection other than the one held
//! ([`Document::commit_editing`]), and a value write to the focused host that
//! changed the value or the selection ([`Document::set_host_value`]). A
//! layout pass that places a pending selection in the new paragraph changes
//! no offsets and sets nothing. The flag is a single bit, so any number of
//! changes before the embedder takes it ([`Document::take_selectionchange`])
//! owe one `selectionchange`, which the embedder fires at the document node
//! (not bubbling, not cancelable), as the standard queues one task per set
//! flag. `terminus` dispatches nothing.
//!
//! # Selection and composition
//!
//! The selection is a parley [`Selection`] over the selection host's
//! committed paragraph, refreshed after every layout pass
//! ([`Selection::refresh`]). A
//! command that changes the value cannot address the new text in the old
//! layout, so it leaves the selection as byte offsets into the new value
//! until the next layout pass places it in the new paragraph. The
//! composition is a byte range into the value, kept beside the selection.
//!
//! The paragraph's bytes are not the value's: white space the paragraph
//! collapses, an inline box's edges and a truncation tail all move them.
//! Every conversion goes through the paragraph's source map, whose source
//! units are the value's UTF-16 units (`paragraph`). Where the layout does
//! not show part of the value — past a `text-maxline`/`text-maxlength` cut —
//! a command that must measure there is refused. A paragraph that is not
//! built from the value alone (generated `content`, an element child) leaves
//! only `SetSelection` and `SelectAll`, which work on value offsets alone.
//! A host that establishes its own paragraph (a Lynx text element, a block
//! container) always has one: with no content, that paragraph lays out one
//! empty line of the host's strut, so the host is one line tall and its
//! caret sits on that line, as an HTML text control is never shorter than
//! one line. A text control's editor keeps that line when editing is turned
//! off and it is no longer a host: its component marks it
//! ([`Document::set_has_line_if_empty`]), as Blink's inner editor's UA style
//! does; every other empty paragraph lays out no line. A host with no paragraph — a flex, grid,
//! linear or relative host that is empty, or holds only white space it does not render — is
//! edited on value offsets alone, a cluster being one code point there.
//!
//! Commands that read the layout — pointer placement, caret movement,
//! cluster and word deletion — run a layout pass first
//! ([`Document::layout`]), as an editor must before it measures. Commands
//! that work on offsets alone do not.

mod command;
mod input_state;
mod offsets;
mod paragraph;
#[cfg(test)]
mod tests;

use core::ops::Range;

use parley::{Affinity, Selection};
use rustc_hash::FxHashMap;
use stylo::values::computed::UserSelect;

pub use self::command::{
    Command, Direction, EditEvent, EditEventKind, EditOutcome, Granularity, PreparedEdit,
};
pub(crate) use self::input_state::{CARET_WIDTH, caret_box, empty_line_height};
pub use self::input_state::{EnterKeyHint, InputMode, InputType, TextInputState};
pub(crate) use self::paragraph::EditableParagraph;
use crate::tree::document::{Document, NodeId};

const CONTENTEDITABLE: &str = "contenteditable";

pub(crate) fn is_contenteditable_attribute(name: &str) -> bool {
    name == CONTENTEDITABLE
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusEvent {
    pub kind: FocusEventKind,
    pub target: NodeId,
    pub related_target: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEventKind {
    Blur,
    FocusOut,
    Focus,
    FocusIn,
}

impl FocusEventKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Blur => "blur",
            Self::FocusOut => "focusout",
            Self::Focus => "focus",
            Self::FocusIn => "focusin",
        }
    }

    #[must_use]
    pub const fn bubbles(self) -> bool {
        matches!(self, Self::FocusIn | Self::FocusOut)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ByteSelection {
    pub(crate) anchor: usize,
    pub(crate) focus: usize,
    pub(crate) affinity: Affinity,
}

impl ByteSelection {
    pub(crate) const fn collapsed(at: usize, affinity: Affinity) -> Self {
        Self {
            anchor: at,
            focus: at,
            affinity,
        }
    }

    pub(crate) fn range(self) -> Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus)
    }

    pub(crate) fn clamped(self, value: &str) -> Self {
        Self {
            anchor: offsets::floor_char_boundary(value, self.anchor),
            focus: offsets::floor_char_boundary(value, self.focus),
            affinity: self.affinity,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Session {
    pub(crate) host: NodeId,
    pub(crate) focused: bool,
    pub(crate) selection: Selection,
    pub(crate) pending: Option<ByteSelection>,
    pub(crate) composition: Option<Range<usize>>,
}

impl Session {
    fn new(host: NodeId, selection: ByteSelection) -> Self {
        Self {
            host,
            focused: true,
            selection: Selection::default(),
            pending: Some(selection),
            composition: None,
        }
    }

    fn selectable(root: NodeId) -> Self {
        Self {
            host: root,
            focused: false,
            selection: Selection::default(),
            pending: Some(ByteSelection::collapsed(0, Affinity::Downstream)),
            composition: None,
        }
    }

    pub(crate) fn bytes(
        &self,
        paragraph: Option<&EditableParagraph<'_>>,
        value: &str,
    ) -> Option<ByteSelection> {
        if let Some(pending) = self.pending {
            return Some(pending);
        }
        let paragraph = paragraph.filter(|paragraph| paragraph.shows_value)?;
        paragraph.read(value, &self.selection)
    }

    pub(crate) fn selection_in(&self, paragraph: &EditableParagraph<'_>, value: &str) -> Selection {
        match self.pending {
            None => self.selection,
            Some(bytes) => paragraph.place_clamped(value, bytes),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct EditingState {
    pub(crate) session: Option<Session>,
    events: Vec<FocusEvent>,
    retained: FxHashMap<NodeId, ByteSelection>,
    pub(crate) selectionchange_pending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentSelection {
    pub host: NodeId,
    pub anchor: u32,
    pub focus: u32,
}

pub(crate) fn makes_host(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.eq_ignore_ascii_case("false"))
}

pub(crate) fn is_host_node<T>(node: &crate::tree::node::Node<T>) -> bool {
    node.is_element() && makes_host(node.attribute(CONTENTEDITABLE).as_deref())
}

pub(crate) fn has_line_if_empty<T>(node: &crate::tree::node::Node<T>) -> bool {
    node.has_line_if_empty() || is_host_node(node)
}

impl<T> Document<T> {
    #[must_use]
    pub fn is_editing_host(&self, node: NodeId) -> bool {
        self.get(node).is_some_and(is_host_node)
    }

    pub fn set_has_line_if_empty(&mut self, node: NodeId, on: bool) {
        if !self
            .get(node)
            .is_some_and(crate::tree::node::Node::is_element)
        {
            return;
        }
        if self.live_node_mut(node).set_has_line_if_empty(on) != on {
            self.invalidate_layout(node);
        }
    }

    pub(crate) fn note_contenteditable_change(&mut self, node: NodeId, value: Option<&str>) {
        if makes_host(value) != self.is_editing_host(node) {
            self.invalidate_layout(node);
        }
    }

    pub(crate) fn note_freed_editing_node(&mut self, node: &crate::tree::node::Node<T>) {
        let id = node.id();
        if !self.editing.retained.is_empty() {
            self.editing.retained.remove(&id);
        }
        if self.selection_host() == Some(id) {
            self.end_session();
        }
    }

    #[must_use]
    pub fn focused(&self) -> Option<NodeId> {
        self.focused_session().map(|session| session.host)
    }

    pub(crate) fn focused_session(&self) -> Option<&Session> {
        self.editing
            .session
            .as_ref()
            .filter(|session| session.focused)
    }

    #[must_use]
    pub fn selection_host(&self) -> Option<NodeId> {
        self.editing.session.as_ref().map(|session| session.host)
    }

    #[must_use]
    pub fn is_user_selectable(&self, node: NodeId) -> bool {
        let mut current = self.get(node);
        while let Some(here) = current {
            if here.is_element()
                && let Some(style) = here.layout_computed_style()
                && *style.get_user_select() != UserSelect::Auto
            {
                return *style.get_user_select() != UserSelect::None;
            }
            current = here.flat_parent();
        }
        true
    }

    #[must_use]
    pub fn selectable_root_of(&self, node: NodeId) -> Option<NodeId> {
        if !self.is_user_selectable(node) {
            return None;
        }
        let mut current = Some(node);
        while let Some(id) = current {
            let here = self.get(id)?;
            if here.is_element() && self.is_rendered(id) && self.paragraph_node_of(id).is_some() {
                return self.holds_selection(id).then_some(id);
            }
            current = here.parent_id();
        }
        None
    }

    fn holds_selection(&self, root: NodeId) -> bool {
        self.is_shadow_including_connected(root)
            && self.is_rendered(root)
            && self.paragraph_node_of(root).is_some()
            && self.is_user_selectable(root)
    }

    pub fn begin_selection(&mut self, root: NodeId) -> bool {
        if !self.holds_selection(root) {
            return false;
        }
        if self.selection_host() == Some(root) {
            return true;
        }
        if let Some(focused) = self.focused() {
            self.unfocus(focused, None);
        }
        self.editing.session = Some(Session::selectable(root));
        self.editing.selectionchange_pending = true;
        self.note_visual_mutation();
        true
    }

    pub fn clear_selection(&mut self) -> bool {
        if self
            .editing
            .session
            .as_ref()
            .is_none_or(|session| session.focused)
        {
            return false;
        }
        self.end_session();
        true
    }

    fn end_session(&mut self) {
        self.editing.session = None;
        self.editing.selectionchange_pending = true;
        self.note_visual_mutation();
    }

    pub(crate) fn session_text(&self, session: &Session) -> String {
        if session.focused {
            return self.host_value(session.host);
        }
        self.paragraph_node_of(session.host)
            .and_then(|paragraph| self.paragraph_source_text(paragraph))
            .unwrap_or_default()
    }

    #[must_use]
    pub fn selected_text(&self, root: NodeId) -> Option<String> {
        let session = self
            .editing
            .session
            .as_ref()
            .filter(|session| session.host == root)?;
        let text = self.session_text(session);
        let range = session
            .bytes(self.session_paragraph(session).as_ref(), &text)
            .map_or(text.len()..text.len(), |bytes| bytes.clamped(&text).range());
        Some(text[range].to_owned())
    }

    pub fn focus(&mut self, node: NodeId) -> bool {
        if !self.is_editing_host(node)
            || !self.is_shadow_including_connected(node)
            || self.in_display_none(node)
        {
            return false;
        }
        let previous = self.focused();
        if previous == Some(node) {
            return true;
        }
        if let Some(previous) = previous {
            self.unfocus(previous, Some(node));
        }
        let selection = match self.editing.retained.get(&node) {
            Some(retained) => retained.clamped(&self.host_value(node)),
            None => ByteSelection::collapsed(0, Affinity::Downstream),
        };
        self.editing.session = Some(Session::new(node, selection));
        self.editing.selectionchange_pending = true;
        self.update_element_state(node, stylo_dom::ElementState::FOCUS, true);
        self.note_visual_mutation();
        self.editing.events.extend([
            FocusEvent {
                kind: FocusEventKind::Focus,
                target: node,
                related_target: previous,
            },
            FocusEvent {
                kind: FocusEventKind::FocusIn,
                target: node,
                related_target: previous,
            },
        ]);
        true
    }

    pub fn blur(&mut self) -> bool {
        let Some(previous) = self.focused() else {
            return false;
        };
        self.unfocus(previous, None);
        true
    }

    #[must_use]
    pub fn has_focus_events(&self) -> bool {
        !self.editing.events.is_empty()
    }

    pub fn take_focus_events(&mut self) -> Vec<FocusEvent> {
        std::mem::take(&mut self.editing.events)
    }

    #[must_use]
    pub const fn has_pending_selectionchange(&self) -> bool {
        self.editing.selectionchange_pending
    }

    pub fn take_selectionchange(&mut self) -> bool {
        std::mem::take(&mut self.editing.selectionchange_pending)
    }

    #[must_use]
    pub fn selection(&self) -> Option<DocumentSelection> {
        let session = self.editing.session.as_ref()?;
        let host = session.host;
        let value = self.session_text(session);
        let bytes = session
            .bytes(self.session_paragraph(session).as_ref(), &value)
            .unwrap_or_else(|| ByteSelection::collapsed(value.len(), Affinity::Downstream));
        let to_units =
            |byte: usize| u32::try_from(offsets::unit_of_byte(&value, byte)).unwrap_or(u32::MAX);
        Some(DocumentSelection {
            host,
            anchor: to_units(bytes.anchor),
            focus: to_units(bytes.focus),
        })
    }

    #[must_use]
    pub fn host_selection(&self, host: NodeId) -> Option<Range<u32>> {
        let value = self.host_value(host);
        let bytes = match self
            .focused_session()
            .filter(|session| session.host == host)
        {
            Some(session) => session
                .bytes(self.editable_paragraph(host).as_ref(), &value)
                .map_or(value.len()..value.len(), ByteSelection::range),
            None => self.editing.retained.get(&host)?.clamped(&value).range(),
        };
        let to_units =
            |byte: usize| u32::try_from(offsets::unit_of_byte(&value, byte)).unwrap_or(u32::MAX);
        Some(to_units(bytes.start)..to_units(bytes.end))
    }

    fn retain_selection(&mut self) {
        let Some(session) = self.focused_session() else {
            return;
        };
        let host = session.host;
        if self.get(host).is_none() {
            return;
        }
        let value = self.host_value(host);
        let bytes = session.bytes(self.editable_paragraph(host).as_ref(), &value);
        if let Some(bytes) = bytes {
            self.editing.retained.insert(host, bytes);
        }
    }

    fn unfocus(&mut self, element: NodeId, related_target: Option<NodeId>) {
        self.retain_selection();
        self.editing.session = None;
        self.editing.selectionchange_pending = true;
        self.update_element_state(element, stylo_dom::ElementState::FOCUS, false);
        self.note_visual_mutation();
        self.editing.events.extend([
            FocusEvent {
                kind: FocusEventKind::Blur,
                target: element,
                related_target,
            },
            FocusEvent {
                kind: FocusEventKind::FocusOut,
                target: element,
                related_target,
            },
        ]);
    }

    pub(crate) fn fix_up_selection(&mut self) {
        let Some(session) = self.editing.session.as_ref() else {
            return;
        };
        let host = session.host;
        if session.focused {
            if self.is_editing_host(host) && self.is_shadow_including_connected(host) {
                return;
            }
            self.drop_focus(host);
        } else if !self.holds_selection(host) {
            self.end_session();
        }
    }

    fn drop_focus(&mut self, host: NodeId) {
        self.retain_selection();
        self.editing.session = None;
        self.editing.selectionchange_pending = true;
        if self
            .get(host)
            .is_some_and(crate::tree::node::Node::is_element)
        {
            self.update_element_state(host, stylo_dom::ElementState::FOCUS, false);
        }
        self.note_visual_mutation();
    }

    fn is_rendered(&self, node: NodeId) -> bool {
        self.get(node)
            .is_some_and(|node| node.layout_computed_style().is_some())
            && !self.in_display_none(node)
    }

    fn in_display_none(&self, node: NodeId) -> bool {
        let mut current = self.get(node);
        while let Some(node) = current {
            if node.is_element()
                && let Some(style) = node.layout_computed_style()
                && crate::layout::display_mode(node, *style.get_display())
                    == crate::layout::DisplayMode::None
            {
                return true;
            }
            current = node.flat_parent();
        }
        false
    }

    fn is_shadow_including_connected(&self, node: NodeId) -> bool {
        let mut current = node;
        loop {
            let Some(node) = self.get(current) else {
                return false;
            };
            if current == crate::tree::document::DOCUMENT_NODE_ID {
                return true;
            }
            match node.parent_id().or_else(|| node.shadow_host_id()) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
    }

    pub(crate) fn host_value(&self, host: NodeId) -> String {
        let Some(node) = self.get(host) else {
            return String::new();
        };
        let mut value = String::new();
        for &child in node.child_ids() {
            if let Some(text) = self.get(child).and_then(crate::tree::node::Node::text) {
                value.push_str(text);
            }
        }
        value
    }

    pub(crate) fn host_is_plain_text(&self, host: NodeId) -> bool {
        self.get(host).is_some_and(|node| {
            node.child_ids().iter().all(|&child| {
                self.get(child)
                    .is_some_and(crate::tree::node::Node::is_text_node)
            })
        })
    }

    pub(crate) fn refresh_editing_after_layout(&mut self) {
        let Some(session) = self.editing.session.as_ref() else {
            return;
        };
        let host = session.host;
        let focused = session.focused;
        if focused && !self.is_rendered(host) {
            self.drop_focus(host);
            return;
        }
        if !focused && !self.holds_selection(host) {
            self.end_session();
            return;
        }
        let Some(session) = self.editing.session.as_ref() else {
            return;
        };
        let value = self.session_text(session);
        let (selection, pending) = match self.session_paragraph(session) {
            Some(paragraph) => match session.pending {
                Some(bytes) => match paragraph.place(&value, bytes) {
                    Some(placed) => (placed, None),
                    None => (session.selection, Some(bytes)),
                },
                None => (session.selection.refresh(paragraph.layout), None),
            },
            None => (
                Selection::default(),
                Some(session.pending.unwrap_or_else(|| {
                    ByteSelection::collapsed(value.len(), Affinity::Downstream)
                })),
            ),
        };
        let session = self
            .editing
            .session
            .as_mut()
            .expect("the session was read above");
        session.selection = selection;
        session.pending = pending;
        if focused
            && let Some(composition) = &session.composition
            && (composition.end > value.len() || composition.start >= composition.end)
        {
            session.composition = None;
        }
    }
}
