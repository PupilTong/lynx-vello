//! Plain-text editing: editing hosts, the focused element, the selection and
//! the IME composition over a host's committed paragraph, and the commands a
//! keyboard, a pointer and a platform input method apply to it.
//!
//! `docs/editable-text-design.md` is the design; this module is its first
//! part (PR E1). The browser standard is the model throughout: HTML's
//! `contenteditable`, its focus update steps, the Selection/caret model, and
//! the event sequences of UI Events and Input Events Level 2. `dom`
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
//! # Selection and composition
//!
//! The selection is a parley [`Selection`] over the host's committed
//! paragraph, refreshed after every layout pass ([`Selection::refresh`]). A
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
//! A host with no paragraph — empty, or holding only white space a flex
//! container does not render — is edited on value offsets alone, a cluster
//! being one code point there.
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

pub use self::command::{
    Command, Direction, EditEvent, EditEventKind, EditOutcome, Granularity, PreparedEdit,
};
pub(crate) use self::input_state::{CARET_WIDTH, caret_box, empty_line_height};
pub use self::input_state::{EnterKeyHint, InputMode, InputType, TextInputState};
pub(crate) use self::paragraph::EditableParagraph;
use crate::Document;
use crate::tree::document::NodeId;

/// The attribute that makes an element an editing host.
const CONTENTEDITABLE: &str = "contenteditable";

/// Whether an attribute name is the one [`Document::is_editing_host`] reads,
/// so the attribute setters know when to re-check a focused host.
pub(crate) fn is_contenteditable_attribute(name: &str) -> bool {
    name == CONTENTEDITABLE
}

/// One focus event the embedder owes script, in the order they formed.
///
/// HTML's focus update steps fire `blur` then `focusout` at the element that
/// lost focus and `focus` then `focusin` at the one that gained it, each
/// naming the other as its `relatedTarget`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusEvent {
    pub kind: FocusEventKind,
    pub target: NodeId,
    pub related_target: Option<NodeId>,
}

/// Which of UI Events' four focus events a [`FocusEvent`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEventKind {
    Blur,
    FocusOut,
    Focus,
    FocusIn,
}

impl FocusEventKind {
    /// The event's type string.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Blur => "blur",
            Self::FocusOut => "focusout",
            Self::Focus => "focus",
            Self::FocusIn => "focusin",
        }
    }

    /// Whether the event bubbles: `focusin` and `focusout` do, `focus` and
    /// `blur` do not. None of the four is cancelable.
    #[must_use]
    pub const fn bubbles(self) -> bool {
        matches!(self, Self::FocusIn | Self::FocusOut)
    }
}

/// A selection as byte offsets into the value: what a command computes, and
/// how the session holds it until a layout pass can place it in the host's
/// paragraph (`paragraph`).
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
}

/// The editing state of the focused host.
#[derive(Clone, Debug)]
pub(crate) struct Session {
    pub(crate) host: NodeId,
    /// Over the host's committed paragraph; authoritative while `pending`
    /// is `None`.
    pub(crate) selection: Selection,
    /// The selection as value offsets, set by a change the committed
    /// paragraph does not show yet, and kept while the host has no
    /// paragraph or the paragraph cannot show it (past a truncation cut).
    pub(crate) pending: Option<ByteSelection>,
    /// The IME composition, as a byte range into the value.
    pub(crate) composition: Option<Range<usize>>,
}

impl Session {
    fn new(host: NodeId) -> Self {
        Self {
            host,
            selection: Selection::default(),
            // A host focused by script places its caret at the start, as
            // browsers do for `element.focus()` on a `contenteditable`.
            pending: Some(ByteSelection::collapsed(0, Affinity::Downstream)),
            composition: None,
        }
    }

    /// The selection as value bytes, whichever form it is held in. `None`
    /// when it is held in a paragraph that cannot say: none is given, it is
    /// not built from the value, or an end lies past its truncation cut.
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

    /// The selection as a parley value over `paragraph`'s layout, for
    /// painting and geometry: the held one, or the pending offsets placed
    /// in it, an end the layout does not show moved to the end of what it
    /// shows.
    pub(crate) fn selection_in(&self, paragraph: &EditableParagraph<'_>, value: &str) -> Selection {
        match self.pending {
            None => self.selection,
            Some(bytes) => paragraph.place_clamped(value, bytes),
        }
    }
}

/// The document's focus and editing state.
#[derive(Debug, Default)]
pub(crate) struct EditingState {
    /// The focused element and its editing state; `None` when nothing is
    /// focused.
    pub(crate) session: Option<Session>,
    /// Focus events not yet taken by the embedder.
    events: Vec<FocusEvent>,
}

impl<T> Document<T> {
    /// Whether `node` is an editing host: an element whose `contenteditable`
    /// attribute is present and is not an ASCII case-insensitive match for
    /// `"false"`.
    #[must_use]
    pub fn is_editing_host(&self, node: NodeId) -> bool {
        self.get(node)
            .filter(|node| node.is_element())
            .and_then(|node| node.attribute(CONTENTEDITABLE))
            .is_some_and(|value| !value.eq_ignore_ascii_case("false"))
    }

    /// The focused element, if any.
    #[must_use]
    pub fn focused(&self) -> Option<NodeId> {
        self.editing.session.as_ref().map(|session| session.host)
    }

    /// Focuses `node`, which must be a connected editing host that is being
    /// rendered — not `display: none`, nor inside an element that is;
    /// anything else is refused with `false` and changes nothing. A host no
    /// style flush has reached yet is accepted unless an ancestor is
    /// `display: none`, and the fixup rule after the next layout pass
    /// decides. Focusing the focused element is a no-op
    /// that answers `true`.
    ///
    /// Queues `blur` and `focusout` at the previously focused element, then
    /// `focus` and `focusin` at `node`, and places a collapsed caret at the
    /// start of `node`'s value.
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
        self.editing.session = Some(Session::new(node));
        self.update_element_state(node, crate::ElementState::FOCUS, true);
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

    /// Unfocuses the focused element, queueing its `blur` and `focusout`.
    /// Answers whether anything was focused.
    pub fn blur(&mut self) -> bool {
        let Some(previous) = self.focused() else {
            return false;
        };
        self.unfocus(previous, None);
        true
    }

    /// Takes the focus events queued since the last call, in the order they
    /// formed.
    pub fn take_focus_events(&mut self) -> Vec<FocusEvent> {
        std::mem::take(&mut self.editing.events)
    }

    fn unfocus(&mut self, element: NodeId, related_target: Option<NodeId>) {
        self.editing.session = None;
        self.update_element_state(element, crate::ElementState::FOCUS, false);
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

    /// HTML's focus fixup rule: a focused element that left the document or
    /// stopped being an editing host is no longer focused, and no event says
    /// so. Called from the removal path and the attribute setters.
    pub(crate) fn fix_up_focus(&mut self) {
        let Some(host) = self.focused() else {
            return;
        };
        if self.is_editing_host(host) && self.is_shadow_including_connected(host) {
            return;
        }
        self.drop_focus(host);
    }

    /// Unfocuses `host` with no event: the fixup rule's ending.
    fn drop_focus(&mut self, host: NodeId) {
        self.editing.session = None;
        if self.get(host).is_some_and(crate::Node::is_element) {
            self.update_element_state(host, crate::ElementState::FOCUS, false);
        }
        self.note_visual_mutation();
    }

    /// Whether `node` is being rendered: it is styled, and neither it nor a
    /// flat-tree ancestor is `display: none`. A `display: contents` host
    /// counts as rendered, as HTML's focusable areas do.
    fn is_rendered(&self, node: NodeId) -> bool {
        self.get(node)
            .is_some_and(|node| node.layout_computed_style().is_some())
            && !self.in_display_none(node)
    }

    /// Whether `node` or a flat-tree ancestor computes `display: none`, as
    /// of the last style flush. An element no flush has styled says nothing.
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

    /// Whether `node` is in the document, counting a node inside a shadow
    /// tree whose host is: a UA component's editing host lives in one.
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

    /// The host's value: its direct text-node children's data, concatenated.
    pub(crate) fn host_value(&self, host: NodeId) -> String {
        let Some(node) = self.get(host) else {
            return String::new();
        };
        let mut value = String::new();
        for &child in node.child_ids() {
            if let Some(text) = self.get(child).and_then(crate::Node::text) {
                value.push_str(text);
            }
        }
        value
    }

    /// Whether every child of `host` is a text node — the condition for
    /// commands that change its value.
    pub(crate) fn host_is_plain_text(&self, host: NodeId) -> bool {
        self.get(host).is_some_and(|node| {
            node.child_ids()
                .iter()
                .all(|&child| self.get(child).is_some_and(crate::Node::is_text_node))
        })
    }

    /// After a layout pass: drops the focus of a host that is no longer
    /// rendered (the fixup rule, no event), then places the focused host's
    /// selection in the paragraph the pass committed — from the pending
    /// offsets after a value change, with [`Selection::refresh`] otherwise.
    /// Offsets the paragraph cannot show stay pending, and a host with no
    /// paragraph keeps its selection as value offsets. Clamps a composition
    /// that a script's own edit left past the value's end.
    pub(crate) fn refresh_editing_after_layout(&mut self) {
        let Some(host) = self.focused() else {
            return;
        };
        if !self.is_rendered(host) {
            self.drop_focus(host);
            return;
        }
        let value = self.host_value(host);
        let Some(session) = self.editing.session.as_ref() else {
            return;
        };
        let (selection, pending) = match self.editable_paragraph(host) {
            Some(paragraph) => match session.pending {
                Some(bytes) => match paragraph.place(&value, bytes) {
                    Some(placed) => (placed, None),
                    None => (session.selection, Some(bytes)),
                },
                None => (session.selection.refresh(paragraph.layout), None),
            },
            // No paragraph to hold a parley selection: value offsets, the
            // end of the value when the one held was in a paragraph that is
            // gone.
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
            .expect("a focused element has a session");
        session.selection = selection;
        session.pending = pending;
        if let Some(composition) = &session.composition
            && (composition.end > value.len() || composition.start >= composition.end)
        {
            session.composition = None;
        }
    }
}
