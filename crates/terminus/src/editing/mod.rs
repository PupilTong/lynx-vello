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
//! The Selection API gives a document one selection. Here it is the focused
//! editing host's session selection, and nothing else: with nothing focused
//! the document has no selection (a browser's `rangeCount` 0), and
//! [`Document::selection`] answers `None`. Text outside an editing host
//! cannot be selected, so the session is the only place a selection can be.
//! This is a simplification of the standard: a browser keeps the
//! document's ranges after a script's `blur()`, while here a session ending
//! — a blur, focus moving to another host, the fixup rule — ends the
//! document's selection too. The host's own retained selection
//! ([`Document::host_selection`]) is a text control's
//! `selectionStart`/`selectionEnd`, a different thing, and is not the
//! document's.
//!
//! [`Document::selection`] reports the two boundary points apart, as UTF-16
//! offsets into the host's value: `anchor` where the selection was started,
//! `focus` where it was extended to, so `focus < anchor` is a backward
//! selection.
//!
//! Every change of the document's selection sets one flag, the Selection
//! API's "has scheduled selectionchange event" (HTML keeps the same flag on
//! each document and text control): a session starting
//! ([`Document::focus`]) or ending (a blur, the fixup rule), an applied
//! command that changed the value or left a selection other than the one
//! held ([`Document::commit_editing`]), and a value write to the focused
//! host that changed the value or the selection
//! ([`Document::set_host_value`]). A layout pass that places a pending
//! selection in the new paragraph changes no offsets and sets nothing. The
//! flag is a single bit, so any number of changes before the embedder takes
//! it ([`Document::take_selectionchange`]) owe one `selectionchange`, which
//! the embedder fires at the document node (not bubbling, not cancelable),
//! as the standard queues one task per set flag. `terminus` dispatches nothing.
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

pub use self::command::{
    Command, Direction, EditEvent, EditEventKind, EditOutcome, Granularity, PreparedEdit,
};
pub(crate) use self::input_state::{CARET_WIDTH, caret_box, empty_line_height};
pub use self::input_state::{EnterKeyHint, InputMode, InputType, TextInputState};
pub(crate) use self::paragraph::EditableParagraph;
use crate::tree::document::{Document, NodeId};

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

    /// Both ends moved down to a character boundary of `value` and clamped
    /// to its length: a selection retained while the value changed under it
    /// never ends inside a character, so never inside a surrogate pair.
    pub(crate) fn clamped(self, value: &str) -> Self {
        Self {
            anchor: offsets::floor_char_boundary(value, self.anchor),
            focus: offsets::floor_char_boundary(value, self.focus),
            affinity: self.affinity,
        }
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
    /// A session over `host` whose selection is `selection`, as value
    /// offsets the next layout pass places in the host's paragraph.
    fn new(host: NodeId, selection: ByteSelection) -> Self {
        Self {
            host,
            selection: Selection::default(),
            pending: Some(selection),
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
    /// How many elements of this document, connected or not, are editing
    /// hosts ([`Document::has_editing_hosts`]). Kept by the attribute setters
    /// and by [`Document::free_node`], the two places the answer can change.
    hosts: usize,
    /// The selection of each host that is not focused, as byte offsets into
    /// the value it had when the selection was retained
    /// ([`Document::host_selection`]). A host that is not focused has no
    /// session to hold it, and a browser's text control keeps its selection
    /// across a blur, so it is kept here, keyed by host. Only hosts that
    /// were focused once, or written by [`Document::set_host_value`], have
    /// an entry: losing focus and that write are the only insertions, and
    /// [`Document::free_node`] removes the entry with the node, so a retired
    /// id never keeps one. A focused host's entry is not read until its
    /// session ends and replaces it.
    retained: FxHashMap<NodeId, ByteSelection>,
    /// Whether the document's selection ([`Document::selection`]) changed
    /// since the embedder last took this ([`Document::take_selectionchange`]):
    /// the Selection API's "has scheduled selectionchange event" flag. Set by
    /// the session starting and ending and by every change of its offsets
    /// (the module's "The document's selection"); cleared only by the take.
    pub(crate) selectionchange_pending: bool,
}

/// The document's selection ([`Document::selection`]): the focused editing
/// host's, as UTF-16 unit offsets into the host's value
/// (`Document::host_value`). `anchor` and `focus` are the Selection API's
/// two boundary points, so `focus < anchor` is a backward selection and
/// `anchor == focus` a collapsed one (a caret).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentSelection {
    /// The focused editing host, whose direct text children hold both
    /// boundary points.
    pub host: NodeId,
    /// Where the selection was started, in UTF-16 units of the host's value.
    pub anchor: u32,
    /// Where the selection was extended to, in UTF-16 units of the host's
    /// value.
    pub focus: u32,
}

/// Whether an attribute value makes its element an editing host: present
/// and not an ASCII case-insensitive match for `"false"`.
pub(crate) fn makes_host(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.eq_ignore_ascii_case("false"))
}

/// Whether `node` is an editing host ([`Document::is_editing_host`]), read
/// off the node itself.
pub(crate) fn is_host_node<T>(node: &crate::tree::node::Node<T>) -> bool {
    node.is_element() && makes_host(node.attribute(CONTENTEDITABLE).as_deref())
}

/// Whether the paragraph `node` establishes keeps one empty line of its
/// strut when it has no content: `node` is an editing host, or was given
/// the line by [`Document::set_has_line_if_empty`]. What layout asks of the
/// element establishing a paragraph; Blink's `LayoutBlock::HasLineIfEmpty`,
/// true for an editable root or a style that says so.
pub(crate) fn has_line_if_empty<T>(node: &crate::tree::node::Node<T>) -> bool {
    node.has_line_if_empty() || is_host_node(node)
}

impl<T> Document<T> {
    /// Whether `node` is an editing host: an element whose `contenteditable`
    /// attribute is present and is not an ASCII case-insensitive match for
    /// `"false"`.
    #[must_use]
    pub fn is_editing_host(&self, node: NodeId) -> bool {
        self.get(node).is_some_and(is_host_node)
    }

    /// Whether any element of this document is an editing host, connected
    /// or not: one counter read. An embedder that does work only for
    /// documents that can be edited — routing pointer and key events to the
    /// editing default actions — asks this rather than walking the tree.
    ///
    /// A detached host still counts until it is freed, because it can be
    /// inserted again without any attribute changing.
    #[must_use]
    pub const fn has_editing_hosts(&self) -> bool {
        self.editing.hosts > 0
    }

    /// Gives `node`'s own paragraph one empty line of its strut when it has
    /// no content (`on`), or takes it away, whether or not `node` is an
    /// editing host: for a text control's editor, which is one line tall
    /// while editing is turned off too. Blink's text control inner editor
    /// carries the same bit in its UA style (`SetHasLineIfEmpty`), apart
    /// from being editable. An editing host has the line either way. A
    /// node that is not an element is left alone.
    pub fn set_has_line_if_empty(&mut self, node: NodeId, on: bool) {
        if !self
            .get(node)
            .is_some_and(crate::tree::node::Node::is_element)
        {
            return;
        }
        if self.live_node_mut(node).set_has_line_if_empty(on) != on {
            // A shaping input of the paragraph the element establishes, as
            // in `note_contenteditable_change`.
            self.invalidate_layout(node);
        }
    }

    /// Records that `node`'s `contenteditable` attribute is about to change
    /// from what it is now to `value` (`None` for a removal), keeping
    /// [`Self::has_editing_hosts`]'s count. Called by the attribute setters
    /// before they write.
    pub(crate) fn note_contenteditable_change(&mut self, node: NodeId, value: Option<&str>) {
        let was = self.is_editing_host(node);
        let is = makes_host(value);
        if is && !was {
            self.editing.hosts += 1;
        } else if was && !is {
            self.editing.hosts -= 1;
        }
        if is != was {
            // A paragraph the element establishes keeps one empty line for
            // no content while it is a host ([`has_line_if_empty`]), which
            // is a shaping input of that paragraph: lay it out again. Nothing in the style
            // system sees the attribute unless a selector names it.
            self.invalidate_layout(node);
        }
    }

    /// Records that a node being freed leaves the count if it was a host,
    /// and drops its retained selection. One comparison each for a document
    /// that has neither.
    pub(crate) fn note_freed_editing_host(&mut self, node: &crate::tree::node::Node<T>) {
        if !self.editing.retained.is_empty() {
            self.editing.retained.remove(&node.id());
        }
        if self.editing.hosts > 0 && is_host_node(node) {
            self.editing.hosts -= 1;
        }
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
    /// `focus` and `focusin` at `node`. `node`'s selection is the one it
    /// retained ([`Self::host_selection`]), clamped to its value as it is
    /// now, or a collapsed caret at the start of its value when it has
    /// none (HTML's `contenteditable` focus).
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
        // A session starting gives the document a selection where it had
        // none, or another host's.
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

    /// Unfocuses the focused element, queueing its `blur` and `focusout`.
    /// Answers whether anything was focused.
    pub fn blur(&mut self) -> bool {
        let Some(previous) = self.focused() else {
            return false;
        };
        self.unfocus(previous, None);
        true
    }

    /// Whether focus events are queued and not taken yet: one length read,
    /// for an embedder that dispatches them from a turn of its own when
    /// something other than its own input handling moved the focus (an
    /// element method's `focus()` or `blur()`).
    #[must_use]
    pub fn has_focus_events(&self) -> bool {
        !self.editing.events.is_empty()
    }

    /// Takes the focus events queued since the last call, in the order they
    /// formed.
    pub fn take_focus_events(&mut self) -> Vec<FocusEvent> {
        std::mem::take(&mut self.editing.events)
    }

    /// Whether the document's selection ([`Self::selection`]) changed since
    /// the last [`Self::take_selectionchange`]: the Selection API's "has
    /// scheduled selectionchange event" flag. One field read, for an
    /// embedder that decides once per turn whether it owes a task that
    /// fires `selectionchange`.
    #[must_use]
    pub const fn has_pending_selectionchange(&self) -> bool {
        self.editing.selectionchange_pending
    }

    /// Takes the flag [`Self::has_pending_selectionchange`] reads, answering
    /// whether it was set. The embedder fires `selectionchange` at the
    /// document node when it was — not bubbling, not cancelable — once for
    /// every change since the last take, as the standard's flag coalesces
    /// them. A listener that changes the selection again sets the flag
    /// again, for a later task.
    pub fn take_selectionchange(&mut self) -> bool {
        std::mem::take(&mut self.editing.selectionchange_pending)
    }

    /// The document's selection: the focused editing host's, its anchor and
    /// focus as UTF-16 unit offsets into the host's value, or `None` while
    /// nothing is focused (the module's "The document's selection").
    ///
    /// Read as [`Self::host_selection`] reads the focused host's: a selection
    /// held in a paragraph that cannot read it back as value offsets is
    /// reported collapsed at the value's end, as [`Self::text_input_state`]
    /// reports it. The two points stay apart here, where `host_selection`
    /// orders them, because a reader of the document's selection needs its
    /// direction.
    #[must_use]
    pub fn selection(&self) -> Option<DocumentSelection> {
        let session = self.editing.session.as_ref()?;
        let host = session.host;
        let value = self.host_value(host);
        let bytes = session
            .bytes(self.editable_paragraph(host).as_ref(), &value)
            .unwrap_or_else(|| ByteSelection::collapsed(value.len(), Affinity::Downstream));
        let to_units =
            |byte: usize| u32::try_from(offsets::unit_of_byte(&value, byte)).unwrap_or(u32::MAX);
        Some(DocumentSelection {
            host,
            anchor: to_units(bytes.anchor),
            focus: to_units(bytes.focus),
        })
    }

    /// The selection of `host` in UTF-16 units of its value, start ≤ end:
    /// the session's when `host` is focused (what
    /// [`Self::text_input_state`] reports as its selection), and otherwise
    /// the one it retained when it last lost focus or that
    /// [`Self::set_host_value`] last gave it, clamped to the value as it is
    /// now. `None` for a host that has none: it was never focused nor
    /// written.
    #[must_use]
    pub fn host_selection(&self, host: NodeId) -> Option<Range<u32>> {
        let value = self.host_value(host);
        let bytes = match self
            .editing
            .session
            .as_ref()
            .filter(|session| session.host == host)
        {
            // As `text_input_state` reads it: a selection the paragraph
            // cannot read back is collapsed at the value's end.
            Some(session) => session
                .bytes(self.editable_paragraph(host).as_ref(), &value)
                .map_or(value.len()..value.len(), ByteSelection::range),
            None => self.editing.retained.get(&host)?.clamped(&value).range(),
        };
        let to_units =
            |byte: usize| u32::try_from(offsets::unit_of_byte(&value, byte)).unwrap_or(u32::MAX);
        Some(to_units(bytes.start)..to_units(bytes.end))
    }

    /// Keeps the focused host's selection as value offsets before its
    /// session ends, for [`Self::focus`] to restore. A selection held in a
    /// paragraph that cannot read it back as value offsets
    /// ([`Session::bytes`]) retains nothing new.
    fn retain_selection(&mut self) {
        let Some(session) = self.editing.session.as_ref() else {
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
        // The session's end is the document selection's.
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

    /// Unfocuses `host` with no event: the fixup rule's ending. The host
    /// keeps its selection, as a control that became read-only keeps its
    /// own.
    fn drop_focus(&mut self, host: NodeId) {
        self.retain_selection();
        self.editing.session = None;
        // No focus event, but the document's selection is gone: the flag is
        // set whatever ended the session.
        self.editing.selectionchange_pending = true;
        if self
            .get(host)
            .is_some_and(crate::tree::node::Node::is_element)
        {
            self.update_element_state(host, stylo_dom::ElementState::FOCUS, false);
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
            if let Some(text) = self.get(child).and_then(crate::tree::node::Node::text) {
                value.push_str(text);
            }
        }
        value
    }

    /// Whether every child of `host` is a text node — the condition for
    /// commands that change its value.
    pub(crate) fn host_is_plain_text(&self, host: NodeId) -> bool {
        self.get(host).is_some_and(|node| {
            node.child_ids().iter().all(|&child| {
                self.get(child)
                    .is_some_and(crate::tree::node::Node::is_text_node)
            })
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
