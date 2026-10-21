//! Editing commands and the events they owe.
//!
//! The command set is Chromium's IME surface (`ImeSetComposition`,
//! `ImeCommitText`, `ImeFinishComposingText`,
//! `SetCompositionFromExistingText`, `DeleteSurroundingText`,
//! `ExtendSelectionAndDelete`, `SetEditableSelectionOffsets`) plus the local
//! editing actions a keyboard and a pointer produce. Offsets in commands are
//! UTF-16 code units into the host's value; points are CSS px from the
//! host's border-box origin.
//!
//! # The prepare/commit seam
//!
//! [`Document::apply_editing`] is [`Document::prepare_editing`] followed by
//! [`Document::commit_editing`]. Preparing computes the whole change — the
//! replaced ranges, the selection and composition after it, and the event
//! list — and touches nothing but layout. Committing applies it. An embedder
//! that dispatches into script calls the two halves itself: it dispatches
//! [`PreparedEdit::events`] up to [`PreparedEdit::commit_index`], commits,
//! then dispatches the rest. None of the events cancels the edit: this
//! engine's script events are not cancelable
//! (`docs/tracking/deviations.md`). A commit whose host lost focus, or whose
//! value a listener changed in between, applies nothing (last write wins, as
//! the design rules), and reports `applied: false`.
//!
//! # Event order
//!
//! UI Events' composition sequence with Input Events Level 2's input types:
//! `compositionstart`, then for each update `beforeinput`
//! (`insertCompositionText`), `compositionupdate`, the DOM change, `input`,
//! and finally `compositionend`, which no `beforeinput`/`input` pair
//! accompanies. Outside a composition a change is `beforeinput`, the change,
//! `input`. Committing a composition with text uses `insertCompositionText`
//! (Chromium's choice) rather than Safari's `deleteCompositionText` plus
//! `insertFromComposition` pair.

use core::ops::Range;

use parley::{Affinity, Cursor, Selection};

use super::offsets::{
    byte_of_unit, byte_of_unit_floor, floor_char_boundary, unit_of_byte, utf16_len,
};
use super::{ByteSelection, EditableParagraph, Session};
use crate::tree::document::{Document, NodeId};

/// One editing command, applied to the focused editing host.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// `ImeSetComposition`: replaces the composition — or, when none is
    /// active, the selection — with `text` and makes it the composition.
    /// `selection` is the selection afterwards, in UTF-16 units relative to
    /// the start of `text`. Empty `text` cancels an active composition,
    /// removing its text; with no composition it deletes a range selection
    /// (`deleteContentBackward`, no composition event) and places the
    /// selection relative to where the range began, as Blink does.
    SetComposition {
        text: String,
        selection: Range<usize>,
    },
    /// `ImeCommitText`: replaces the composition — or the selection — with
    /// `text` and ends the composition. The caret lands `relative_cursor`
    /// UTF-16 units after the end of the inserted text (Blink's convention:
    /// `0` is just after it, negative values move back into it and before).
    CommitText { text: String, relative_cursor: i32 },
    /// `ImeFinishComposingText`: keeps the composition's text as written
    /// and ends the composition. Without `keep_selection` the caret moves
    /// to the end of the former composition.
    FinishComposition { keep_selection: bool },
    /// `SetCompositionFromExistingText` (Android `setComposingRegion`):
    /// marks committed text as the composition, in UTF-16 units. An empty
    /// range finishes an active composition and keeps the selection.
    SetCompositionFromExistingText { range: Range<usize> },
    /// `DeleteSurroundingText`: deletes `before` UTF-16 units before the
    /// selection and `after` after it, leaving the selected text in place.
    /// Refused when either end would split a surrogate pair.
    DeleteSurrounding { before: usize, after: usize },
    /// `ExtendSelectionAndDelete`: widens the selection by `before` and
    /// `after` UTF-16 units, then deletes it. Refused when either end would
    /// split a surrogate pair.
    ExtendSelectionAndDelete { before: usize, after: usize },
    /// `SetEditableSelectionOffsets`: the selection, in UTF-16 units, anchor
    /// at `range.start`. An offset inside a surrogate pair moves to the start
    /// of its character; one past the end moves to the end.
    SetSelection { range: Range<usize> },
    /// Places the caret at `point` (host border-box CSS px), or with
    /// `extend` moves only the selection's focus there — at the granularity
    /// the selection was made with, so a drag after a double-click extends
    /// by words.
    SelectFromPoint {
        point: euclid::default::Point2D<f32>,
        extend: bool,
    },
    /// Selects the word at `point` (a double-click).
    SelectWordAtPoint {
        point: euclid::default::Point2D<f32>,
    },
    /// Selects the visual line at `point` (a triple-click).
    SelectLineAtPoint {
        point: euclid::default::Point2D<f32>,
    },
    /// Inserts text in place of the selection (`insertText`). An active
    /// composition is finished first.
    Insert(String),
    /// Inserts `"\n"` in place of the selection (`insertLineBreak`).
    InsertLineBreak,
    /// Deletes the selection, or the character before the caret: the whole
    /// grapheme cluster for an emoji or a line break, the last code point of
    /// the cluster otherwise (`deleteContentBackward`).
    DeleteBackward,
    /// Deletes the selection, or the grapheme cluster after the caret
    /// (`deleteContentForward`).
    DeleteForward,
    /// Deletes the selection, or back to the previous word boundary
    /// (`deleteWordBackward`).
    DeleteWordBackward,
    /// Deletes the selection, or up to the next word boundary
    /// (`deleteWordForward`).
    DeleteWordForward,
    /// Moves the selection's focus; with `extend` the anchor stays, without
    /// it the selection collapses at the new position.
    Move {
        granularity: Granularity,
        direction: Direction,
        extend: bool,
    },
    /// Selects the whole value.
    SelectAll,
}

/// How far a [`Command::Move`] goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    /// One grapheme cluster, in visual order (the arrow keys).
    Character,
    /// To the next word boundary, in visual order.
    Word,
    /// To the start or end of the visual line (Home/End).
    LineBoundary,
    /// One visual line up or down, keeping the horizontal position.
    Line,
    /// To the start or end of the value.
    Document,
}

/// Which way a [`Command::Move`] goes: `Backward` is left or up, `Forward`
/// right or down. For `Character` and `Word` the movement is visual, which
/// is logical order in left-to-right text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Backward,
    Forward,
}

/// Which event an [`EditEvent`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditEventKind {
    BeforeInput,
    Input,
    CompositionStart,
    CompositionUpdate,
    CompositionEnd,
}

impl EditEventKind {
    /// The event's type string.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BeforeInput => "beforeinput",
            Self::Input => "input",
            Self::CompositionStart => "compositionstart",
            Self::CompositionUpdate => "compositionupdate",
            Self::CompositionEnd => "compositionend",
        }
    }
}

/// One event a command owes script, at the editing host. Every one of them
/// bubbles and is composed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditEvent {
    pub kind: EditEventKind,
    /// `InputEvent.inputType` (Input Events Level 2); empty for the
    /// composition events, which have none.
    pub input_type: &'static str,
    /// `InputEvent.data` or `CompositionEvent.data`.
    pub data: Option<String>,
    /// `InputEvent.isComposing`; `false` for the composition events.
    pub is_composing: bool,
}

impl EditEvent {
    fn composition(kind: EditEventKind, data: &str) -> Self {
        Self {
            kind,
            input_type: "",
            data: Some(data.to_owned()),
            is_composing: false,
        }
    }

    fn input(
        kind: EditEventKind,
        input_type: &'static str,
        data: Option<&str>,
        composing: bool,
    ) -> Self {
        Self {
            kind,
            input_type,
            data: data.map(str::to_owned),
            is_composing: composing,
        }
    }
}

const INSERT_TEXT: &str = "insertText";
const INSERT_COMPOSITION_TEXT: &str = "insertCompositionText";
const INSERT_LINE_BREAK: &str = "insertLineBreak";
const DELETE_CONTENT_BACKWARD: &str = "deleteContentBackward";
const DELETE_CONTENT_FORWARD: &str = "deleteContentForward";
const DELETE_WORD_BACKWARD: &str = "deleteWordBackward";
const DELETE_WORD_FORWARD: &str = "deleteWordForward";

/// What a command did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditOutcome {
    /// Whether the command was carried out. `false` when nothing is focused,
    /// when the command had nothing to act on, and when it was refused: a
    /// value change on a read-only host (one with an element child), any
    /// command but `SetSelection` and `SelectAll` over a paragraph that is
    /// not built from the value alone, a command that must measure past a
    /// truncation cut, a surrogate pair split, or a commit whose value moved
    /// after it was prepared.
    pub applied: bool,
    /// The events the embedder dispatches at the host, in order. The value
    /// changed between the last `beforeinput` and the first `input`.
    pub events: Vec<EditEvent>,
}

impl EditOutcome {
    const fn refused() -> Self {
        Self {
            applied: false,
            events: Vec::new(),
        }
    }
}

/// The selection a prepared edit leaves.
#[derive(Clone, Copy, Debug)]
enum SelectionAfter {
    /// Byte offsets into the value after the change.
    Bytes(ByteSelection),
    /// A parley selection over the current layout, which a command that
    /// changes no value can keep — with its word or line granularity and its
    /// remembered horizontal position.
    Layout(Selection),
}

/// A command's complete effect, computed but not applied
/// ([`Document::prepare_editing`]).
#[derive(Clone, Debug)]
pub struct PreparedEdit {
    host: NodeId,
    /// The value the edit was computed against.
    base: String,
    /// Replacements in `base`'s byte offsets, ascending and disjoint.
    changes: Vec<(Range<usize>, String)>,
    selection: SelectionAfter,
    composition: Option<Range<usize>>,
    events: Vec<EditEvent>,
    commit_index: usize,
}

impl PreparedEdit {
    /// The host the edit applies to.
    #[must_use]
    pub const fn host(&self) -> NodeId {
        self.host
    }

    /// Every event the edit owes, in dispatch order.
    #[must_use]
    pub fn events(&self) -> &[EditEvent] {
        &self.events
    }

    /// How many of [`Self::events`] are dispatched before the commit; the
    /// rest follow it.
    #[must_use]
    pub const fn commit_index(&self) -> usize {
        self.commit_index
    }

    /// Whether committing changes the host's value.
    #[must_use]
    pub fn changes_value(&self) -> bool {
        !self.changes.is_empty()
    }

    /// An edit that changes only the selection or the composition.
    fn select(
        host: NodeId,
        base: String,
        selection: SelectionAfter,
        composition: Option<Range<usize>>,
    ) -> Self {
        Self {
            host,
            base,
            changes: Vec::new(),
            selection,
            composition,
            events: Vec::new(),
            commit_index: 0,
        }
    }
}

impl Command {
    /// Whether the command measures the layout, and so needs a current one.
    const fn reads_layout(&self) -> bool {
        matches!(
            self,
            Self::SelectFromPoint { .. }
                | Self::SelectWordAtPoint { .. }
                | Self::SelectLineAtPoint { .. }
                | Self::DeleteBackward
                | Self::DeleteForward
                | Self::DeleteWordBackward
                | Self::DeleteWordForward
                | Self::Move { .. }
        )
    }
}

/// What every command is computed from.
struct Context {
    host: NodeId,
    value: String,
    session: Session,
    /// Whether the value may change: no element child in the host.
    writable: bool,
    /// The selection as value bytes, read once from wherever the session
    /// holds it.
    selection: ByteSelection,
}

impl Context {
    fn selection(&self) -> ByteSelection {
        let bytes = self.selection;
        ByteSelection {
            anchor: floor_char_boundary(&self.value, bytes.anchor),
            focus: floor_char_boundary(&self.value, bytes.focus),
            affinity: bytes.affinity,
        }
    }

    fn composition(&self) -> Option<Range<usize>> {
        self.session.composition.clone()
    }

    fn composition_text(&self) -> Option<&str> {
        self.session
            .composition
            .as_ref()
            .and_then(|range| self.value.get(range.clone()))
    }

    /// One change of `range` to `text`, leaving `selection` (byte offsets in
    /// the new value) and `composition`, with `events` around it: the change
    /// lands before the first `input`.
    fn replace(
        self,
        range: Range<usize>,
        text: &str,
        selection: ByteSelection,
        composition: Option<Range<usize>>,
        events: Vec<EditEvent>,
    ) -> PreparedEdit {
        let commit_index = events
            .iter()
            .position(|event| event.kind == EditEventKind::Input)
            .unwrap_or(events.len());
        PreparedEdit {
            host: self.host,
            base: self.value,
            changes: vec![(range, text.to_owned())],
            selection: SelectionAfter::Bytes(selection),
            composition,
            events,
            commit_index,
        }
    }

    /// A deletion of `range` (bytes) outside any composition logic: a
    /// `beforeinput`/`input` pair of `input_type`, the caret collapsed at the
    /// range's start, and an active composition carried through it.
    fn delete(self, range: Range<usize>, input_type: &'static str) -> Option<PreparedEdit> {
        if range.is_empty() {
            return None;
        }
        let composing = self.session.composition.is_some();
        let composition = self
            .composition()
            .and_then(|composition| map_range(&composition, &[(range.clone(), String::new())]));
        let events = input_pair(input_type, None, composing);
        let caret = ByteSelection::collapsed(range.start, Affinity::Downstream);
        Some(self.replace(range, "", caret, composition, events))
    }
}

/// `beforeinput` and `input` of one input type.
fn input_pair(input_type: &'static str, data: Option<&str>, composing: bool) -> Vec<EditEvent> {
    vec![
        EditEvent::input(EditEventKind::BeforeInput, input_type, data, composing),
        EditEvent::input(EditEventKind::Input, input_type, data, composing),
    ]
}

/// `offset` in the value after `changes`, which are ascending and disjoint;
/// an offset inside a replaced range moves to the replacement's start.
fn map_offset(offset: usize, changes: &[(Range<usize>, String)]) -> usize {
    let mut shifted = offset;
    for (range, text) in changes {
        if offset >= range.end && !(range.is_empty() && offset == range.start) {
            shifted = shifted + text.len() - range.len();
        } else if offset > range.start {
            return shifted - (offset - range.start);
        } else {
            break;
        }
    }
    shifted
}

/// `range` after `changes`, or `None` when nothing of it is left.
fn map_range(range: &Range<usize>, changes: &[(Range<usize>, String)]) -> Option<Range<usize>> {
    let mapped = map_offset(range.start, changes)..map_offset(range.end, changes);
    (!mapped.is_empty()).then_some(mapped)
}

/// `changes` applied to `base`.
fn apply_changes(base: &str, changes: &[(Range<usize>, String)]) -> String {
    let mut value = String::with_capacity(base.len());
    let mut at = 0;
    for (range, text) in changes {
        value.push_str(&base[at..range.start]);
        value.push_str(text);
        at = range.end;
    }
    value.push_str(&base[at..]);
    value
}

impl<T: Sync> Document<T> {
    /// Computes what `command` would do to the focused host without doing
    /// it. `None` when nothing is focused or the command has nothing to act
    /// on or is refused.
    ///
    /// Commands that measure the layout run a layout pass first.
    pub fn prepare_editing(&mut self, command: Command) -> Option<PreparedEdit> {
        self.focused()?;
        if command.reads_layout() {
            self.layout();
        }
        let host = self.focused()?;
        let value = self.host_value(host);
        let session = self.editing.session.clone()?;
        let paragraph = self.editable_paragraph(host);
        // A paragraph built from more than the value — generated `content`,
        // an element child's text — has no offsets in the value: only the
        // commands that work on value offsets alone apply over it.
        let trusted = paragraph
            .as_ref()
            .is_none_or(|paragraph| paragraph.shows_value);
        if !trusted && !matches!(command, Command::SetSelection { .. } | Command::SelectAll) {
            return None;
        }
        let selection = session
            .bytes(paragraph.as_ref(), &value)
            .unwrap_or_else(|| ByteSelection::collapsed(value.len(), Affinity::Downstream));
        let context = Context {
            host,
            value,
            session,
            writable: self.host_is_plain_text(host),
            selection,
        };
        let paragraph = paragraph.as_ref();
        match command {
            Command::SetComposition { text, selection } => {
                set_composition(context, &text, &selection)
            }
            Command::CommitText {
                text,
                relative_cursor,
            } => commit_text(context, &text, relative_cursor),
            Command::FinishComposition { keep_selection } => {
                finish_composition(context, keep_selection)
            }
            Command::SetCompositionFromExistingText { range } => {
                composition_from_existing_text(context, &range)
            }
            Command::DeleteSurrounding { before, after } => {
                delete_surrounding(context, before, after)
            }
            Command::ExtendSelectionAndDelete { before, after } => {
                extend_selection_and_delete(context, before, after)
            }
            Command::SetSelection { range } => Some(set_selection(context, &range)),
            Command::SelectAll => Some(select_all(context)),
            Command::Insert(text) => insert(context, &text, INSERT_TEXT, Some(&text)),
            Command::InsertLineBreak => insert(context, "\n", INSERT_LINE_BREAK, None),
            Command::SelectFromPoint { point, extend } => {
                select_at_point(context, paragraph, point, |layout, selection, x, y| {
                    if extend {
                        selection.extend_to_point(layout, x, y)
                    } else {
                        Selection::from_point(layout, x, y)
                    }
                })
            }
            Command::SelectWordAtPoint { point } => {
                select_at_point(context, paragraph, point, |layout, _, x, y| {
                    Selection::word_from_point(layout, x, y)
                })
            }
            Command::SelectLineAtPoint { point } => {
                select_at_point(context, paragraph, point, |layout, _, x, y| {
                    Selection::line_from_point(layout, x, y)
                })
            }
            Command::Move {
                granularity,
                direction,
                extend,
            } => move_selection(context, paragraph, granularity, direction, extend),
            Command::DeleteBackward
            | Command::DeleteForward
            | Command::DeleteWordBackward
            | Command::DeleteWordForward => delete_by_layout(context, paragraph, &command),
        }
    }

    /// Applies a prepared edit: writes the value, places the selection and
    /// the composition, and answers with the edit's events. Applies nothing
    /// when the host is no longer focused or its value changed since the
    /// edit was prepared.
    ///
    /// An edit that changes the value, or leaves a selection other than the
    /// one held ([`selection_differs`]), sets the document's
    /// `selectionchange` flag ([`Self::has_pending_selectionchange`]); one
    /// that leaves the same offsets — a `SetSelection` to the range already
    /// selected — does not, as the Selection API fires `selectionchange`
    /// only when the selection changes.
    pub fn commit_editing(&mut self, edit: PreparedEdit) -> EditOutcome
    where
        T: Default,
    {
        if self.focused() != Some(edit.host) || self.host_value(edit.host) != edit.base {
            return EditOutcome::refused();
        }
        let changes_selection = !edit.changes.is_empty() || {
            let session = self
                .editing
                .session
                .as_ref()
                .expect("the focused host has a session");
            // The held selection as value offsets is read only where the
            // comparison needs it: offsets after, against a selection held
            // in the paragraph. Read before anything is written.
            let held = match (edit.selection, session.pending) {
                (SelectionAfter::Bytes(_), None) => {
                    session.bytes(self.editable_paragraph(edit.host).as_ref(), &edit.base)
                }
                _ => session.pending,
            };
            selection_differs(session, &edit.selection, held)
        };
        if changes_selection {
            self.editing.selectionchange_pending = true;
        }
        if !edit.changes.is_empty() {
            let value = apply_changes(&edit.base, &edit.changes);
            self.write_host_value(edit.host, value);
        }
        let session = self
            .editing
            .session
            .as_mut()
            .expect("the focused host has a session");
        match edit.selection {
            SelectionAfter::Bytes(bytes) => session.pending = Some(bytes),
            SelectionAfter::Layout(selection) => {
                session.selection = selection;
                session.pending = None;
            }
        }
        session.composition = edit.composition;
        self.note_visual_mutation();
        EditOutcome {
            applied: true,
            events: edit.events,
        }
    }

    /// [`Self::prepare_editing`] then [`Self::commit_editing`]: the whole
    /// command, for an embedder that has nothing to dispatch in between.
    pub fn apply_editing(&mut self, command: Command) -> EditOutcome
    where
        T: Default,
    {
        match self.prepare_editing(command) {
            Some(edit) => self.commit_editing(edit),
            None => EditOutcome::refused(),
        }
    }

    /// Replaces `host`'s value as a script's write to a form control's
    /// value does (`HTMLInputElement.value`): no event is owed, and when
    /// `host` is the focused host its composition ends and its selection
    /// becomes `selection`, in UTF-16 units of the new value (an offset
    /// inside a surrogate pair rounds down to its character's start, one
    /// past the end clamps, as [`Command::SetSelection`] reads them). A
    /// host that is not focused retains `selection` instead
    /// ([`Self::host_selection`]), for its next [`Self::focus`] to restore,
    /// as a browser's text control keeps the selection a value write left.
    ///
    /// The value is written as exactly one text node, the normalization the
    /// first edit performs, and only when it differs from the current one.
    /// `host` need not be an editing host: an element layer that turns
    /// editing off (a read-only field) still owns the text it shows.
    ///
    /// The element layer over this kernel (`x-input`, `x-textarea`) is the
    /// caller: it writes a script's value, and the value its own filters
    /// leave after an edit.
    pub fn set_host_value(&mut self, host: NodeId, value: &str, selection: Range<usize>)
    where
        T: Default,
    {
        if self.get(host).is_none_or(|node| !node.is_element()) {
            return;
        }
        let value_changes = self.host_value(host) != value;
        if value_changes {
            self.write_host_value(host, value.to_owned());
        }
        let selection = ByteSelection {
            anchor: byte_of_unit_floor(value, selection.start),
            focus: byte_of_unit_floor(value, selection.end),
            affinity: Affinity::Downstream,
        };
        if self.focused() != Some(host) {
            self.editing.retained.insert(host, selection);
            return;
        }
        let session = self
            .editing
            .session
            .as_mut()
            .expect("the focused host has a session");
        // The focused host's write changes the document's selection when it
        // changes the value or the offsets. A selection held in the
        // paragraph (`pending` is `None`) counts as changed: that form is
        // not compared here, and a write that places the selection again is
        // taken as a change of it.
        if value_changes || session.pending != Some(selection) {
            self.editing.selectionchange_pending = true;
        }
        session.pending = Some(selection);
        session.composition = None;
        self.note_visual_mutation();
    }

    /// Writes `value` into `host` as exactly one text node.
    fn write_host_value(&mut self, host: NodeId, value: String)
    where
        T: Default,
    {
        let texts: Vec<NodeId> = self
            .get(host)
            .map(|node| {
                node.child_ids()
                    .iter()
                    .copied()
                    .filter(|&child| {
                        self.get(child)
                            .is_some_and(crate::tree::node::Node::is_text_node)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let Some((&first, rest)) = texts.split_first() else {
            let text = self.create_text_node(value, T::default());
            self.append_child(host, text);
            return;
        };
        if self.get(first).and_then(crate::tree::node::Node::text) != Some(value.as_str()) {
            self.set_text_node_data(first, value);
        }
        for &other in rest {
            self.remove_element(other);
        }
    }
}

/// Whether committing `after` changes the session's selection: the
/// document's `selectionchange` flag's test for an edit that changes no
/// value ([`Document::commit_editing`]).
///
/// - Offsets after (`Bytes`) differ unless they equal `held`, the selection the session holds as
///   value offsets: its pending offsets, or, while it is held in the paragraph, what the paragraph
///   reads back ([`Session::bytes`]). `None` — a paragraph that cannot read it back — differs.
/// - A parley selection after (`Layout`) differs unless the session holds one in the paragraph too
///   (`pending` is `None`) with the same anchor and focus, each by byte index and affinity.
///   [`Selection`] has no `PartialEq`, so the two cursors are compared field by field; the
///   remembered horizontal position and the granularity are not offsets and are not compared.
fn selection_differs(
    session: &Session,
    after: &SelectionAfter,
    held: Option<ByteSelection>,
) -> bool {
    match after {
        SelectionAfter::Bytes(bytes) => held != Some(*bytes),
        SelectionAfter::Layout(selection) => {
            let same_cursor =
                |a: Cursor, b: Cursor| a.index() == b.index() && a.affinity() == b.affinity();
            session.pending.is_some()
                || !same_cursor(session.selection.anchor(), selection.anchor())
                || !same_cursor(session.selection.focus(), selection.focus())
        }
    }
}

/// A selection the layout shows only up to a truncation cut: refused when
/// either end lies at or past it, where the display layout shows the tail.
fn shown(paragraph: &EditableParagraph<'_>, selection: Selection) -> Option<Selection> {
    let cut = paragraph.map.cut_byte();
    let past = |cursor: Cursor| paragraph.map.truncated() && cursor.index() >= cut;
    (!past(selection.anchor()) && !past(selection.focus())).then_some(selection)
}

/// The session's selection as a parley value over `paragraph`'s layout,
/// for a command that measures from it: `None` when its pending offsets are
/// not shown there.
fn current_in(context: &Context, paragraph: &EditableParagraph<'_>) -> Option<Selection> {
    match context.session.pending {
        None => Some(context.session.selection),
        Some(_) => paragraph.place(&context.value, context.selection()),
    }
}

fn select_at_point(
    context: Context,
    paragraph: Option<&EditableParagraph<'_>>,
    point: euclid::default::Point2D<f32>,
    select: impl FnOnce(&parley::Layout<crate::layout::TextBrush>, &Selection, f32, f32) -> Selection,
) -> Option<PreparedEdit> {
    let composition = context.composition();
    let Some(paragraph) = paragraph else {
        // No paragraph: a host that establishes none and is empty, or whose
        // white space renders nothing. Every point is at the end of the
        // value.
        let end = context.value.len();
        return Some(PreparedEdit::select(
            context.host,
            context.value,
            SelectionAfter::Bytes(ByteSelection::collapsed(end, Affinity::Downstream)),
            composition,
        ));
    };
    let layout = paragraph.layout;
    let current = context.session.selection_in(paragraph, &context.value);
    let origin = paragraph.origin;
    let selection = shown(
        paragraph,
        select(layout, &current, point.x - origin.x, point.y - origin.y),
    )?;
    Some(PreparedEdit::select(
        context.host,
        context.value,
        SelectionAfter::Layout(selection),
        composition,
    ))
}

fn move_selection(
    context: Context,
    paragraph: Option<&EditableParagraph<'_>>,
    granularity: Granularity,
    direction: Direction,
    extend: bool,
) -> Option<PreparedEdit> {
    let forward = direction == Direction::Forward;
    let composition = context.composition();
    if granularity == Granularity::Document {
        // The value's ends are value offsets: no layout needed, and none
        // past a truncation cut is asked.
        let selection = context.selection();
        let end = if forward { context.value.len() } else { 0 };
        let anchor = if extend { selection.anchor } else { end };
        let affinity = if forward {
            Affinity::Upstream
        } else {
            Affinity::Downstream
        };
        return Some(PreparedEdit::select(
            context.host,
            context.value,
            SelectionAfter::Bytes(ByteSelection {
                anchor,
                focus: end,
                affinity,
            }),
            composition,
        ));
    }
    let Some(paragraph) = paragraph else {
        let selection = move_in_value(&context, granularity, forward, extend);
        return Some(PreparedEdit::select(
            context.host,
            context.value,
            SelectionAfter::Bytes(selection),
            composition,
        ));
    };
    let layout = paragraph.layout;
    let current = current_in(&context, paragraph)?;
    let step = |selection: &Selection| match granularity {
        Granularity::Character if forward => selection.next_visual(layout, extend),
        Granularity::Character => selection.previous_visual(layout, extend),
        Granularity::Word if forward => selection.next_visual_word(layout, extend),
        Granularity::Word => selection.previous_visual_word(layout, extend),
        Granularity::LineBoundary if forward => selection.line_end(layout, extend),
        Granularity::LineBoundary => selection.line_start(layout, extend),
        Granularity::Line if forward => selection.next_line(layout, extend),
        Granularity::Line => selection.previous_line(layout, extend),
        Granularity::Document => unreachable!("answered above"),
    };
    let mut selection = step(&current);
    if granularity == Granularity::Character {
        // A cluster with no source unit — an inline box's edge — is no
        // character of the value: a step over it alone moved the caret in
        // the layout and not in the value, so it steps on.
        let unit = |selection: &Selection| paragraph.map.byte_to_unit(selection.focus().index());
        let mut from = current;
        for _ in 0..MAX_EDGE_STEPS {
            if unit(&selection) != unit(&from) || selection.focus().index() == from.focus().index()
            {
                break;
            }
            from = selection;
            selection = step(&from);
        }
    }
    let selection = shown(paragraph, selection)?;
    Some(PreparedEdit::select(
        context.host,
        context.value,
        SelectionAfter::Layout(selection),
        composition,
    ))
}

/// How many edge clusters one character step walks over at most: an inline
/// box's start and end edges, nested a few deep, before the next character.
const MAX_EDGE_STEPS: usize = 8;

/// A move with no paragraph to measure in — a host that establishes none and
/// is empty, or whose white space renders nothing: one code point for a
/// character, the value's ends for every other granularity.
fn move_in_value(
    context: &Context,
    granularity: Granularity,
    forward: bool,
    extend: bool,
) -> ByteSelection {
    let selection = context.selection();
    let value = &context.value;
    let range = selection.range();
    let focus = match granularity {
        Granularity::Character if !extend && !range.is_empty() => {
            if forward {
                range.end
            } else {
                range.start
            }
        }
        Granularity::Character if forward => value[selection.focus..]
            .chars()
            .next()
            .map_or(selection.focus, |character| {
                selection.focus + character.len_utf8()
            }),
        Granularity::Character => value[..selection.focus]
            .char_indices()
            .next_back()
            .map_or(0, |(start, _)| start),
        _ if forward => value.len(),
        _ => 0,
    };
    ByteSelection {
        anchor: if extend { selection.anchor } else { focus },
        focus,
        affinity: Affinity::Downstream,
    }
}

/// The deletions that ask the layout where a cluster or a word ends:
/// parley's `PlainEditorDriver::{backdelete, delete, backdelete_word,
/// delete_word}`, against the host's value instead of an editor buffer,
/// every layout byte read back through the paragraph's source map.
fn delete_by_layout(
    context: Context,
    paragraph: Option<&EditableParagraph<'_>>,
    command: &Command,
) -> Option<PreparedEdit> {
    if !context.writable {
        return None;
    }
    let input_type = match command {
        Command::DeleteBackward => DELETE_CONTENT_BACKWARD,
        Command::DeleteForward => DELETE_CONTENT_FORWARD,
        Command::DeleteWordBackward => DELETE_WORD_BACKWARD,
        Command::DeleteWordForward => DELETE_WORD_FORWARD,
        _ => unreachable!("only the four deletions reach here"),
    };
    let selection = context.selection();
    if selection.anchor != selection.focus {
        return context.delete(selection.range(), input_type);
    }
    let caret = selection.focus;
    let value = &context.value;
    let Some(paragraph) = paragraph else {
        // No paragraph: one code point, or to the value's end for a word.
        let range = match command {
            Command::DeleteBackward => value[..caret]
                .char_indices()
                .next_back()
                .map_or(caret..caret, |(start, _)| start..caret),
            Command::DeleteForward => value[caret..]
                .chars()
                .next()
                .map_or(caret..caret, |character| {
                    caret..caret + character.len_utf8()
                }),
            Command::DeleteWordBackward => 0..caret,
            _ => caret..value.len(),
        };
        return context.delete(range, input_type);
    };
    let layout = paragraph.layout;
    let focus = shown(paragraph, current_in(&context, paragraph)?)?.focus();
    let range = match command {
        Command::DeleteBackward => {
            // Upstream cluster: the whole of it for a line break or an
            // emoji, its last code point otherwise.
            let cluster = focus.logical_clusters(layout)[0]?;
            let start = if cluster.is_hard_line_break() || cluster.is_emoji() {
                paragraph.value_byte(value, cluster.text_range().start)?
            } else {
                value.get(..caret)?.char_indices().next_back()?.0
            };
            start..caret
        }
        Command::DeleteForward => {
            let [_, after] = focus.logical_clusters(layout);
            caret..paragraph.value_end_byte(value, after?.text_range().end)?
        }
        Command::DeleteWordBackward => {
            paragraph.value_byte(value, focus.previous_logical_word(layout).index())?..caret
        }
        Command::DeleteWordForward => {
            caret
                ..paragraph
                    .value_end_byte(value, focus.next_logical_word(layout).index())?
                    .min(value.len())
        }
        _ => unreachable!("only the four deletions reach here"),
    };
    if range.start > range.end {
        return None;
    }
    value.get(range.clone())?;
    context.delete(range, input_type)
}

/// `SetEditableSelectionOffsets`: value offsets alone.
fn set_selection(context: Context, range: &Range<usize>) -> PreparedEdit {
    let anchor = byte_of_unit_floor(&context.value, range.start);
    let focus = byte_of_unit_floor(&context.value, range.end);
    let composition = context.composition();
    PreparedEdit::select(
        context.host,
        context.value,
        SelectionAfter::Bytes(ByteSelection {
            anchor,
            focus,
            affinity: Affinity::Downstream,
        }),
        composition,
    )
}

fn select_all(context: Context) -> PreparedEdit {
    let end = context.value.len();
    let composition = context.composition();
    PreparedEdit::select(
        context.host,
        context.value,
        SelectionAfter::Bytes(ByteSelection {
            anchor: 0,
            focus: end,
            affinity: Affinity::Upstream,
        }),
        composition,
    )
}

fn set_composition(context: Context, text: &str, selection: &Range<usize>) -> Option<PreparedEdit> {
    if !context.writable {
        return None;
    }
    let composition = context.composition();
    if text.is_empty() {
        let Some(range) = composition else {
            return delete_selection_for_empty_composition(context, selection);
        };
        // Cancelling: the composition's text goes, and the composition ends.
        let mut events = vec![
            EditEvent::input(
                EditEventKind::BeforeInput,
                INSERT_COMPOSITION_TEXT,
                Some(""),
                true,
            ),
            EditEvent::composition(EditEventKind::CompositionUpdate, ""),
            EditEvent::input(
                EditEventKind::Input,
                INSERT_COMPOSITION_TEXT,
                Some(""),
                true,
            ),
        ];
        events.push(EditEvent::composition(EditEventKind::CompositionEnd, ""));
        let caret = ByteSelection::collapsed(range.start, Affinity::Downstream);
        return Some(context.replace(range, "", caret, None, events));
    }
    let target = composition
        .clone()
        .unwrap_or_else(|| context.selection().range());
    let mut events = Vec::new();
    if composition.is_none() {
        events.push(EditEvent::composition(
            EditEventKind::CompositionStart,
            &context.value[target.clone()],
        ));
    }
    events.extend([
        EditEvent::input(
            EditEventKind::BeforeInput,
            INSERT_COMPOSITION_TEXT,
            Some(text),
            true,
        ),
        EditEvent::composition(EditEventKind::CompositionUpdate, text),
        EditEvent::input(
            EditEventKind::Input,
            INSERT_COMPOSITION_TEXT,
            Some(text),
            true,
        ),
    ]);
    let start = target.start;
    let selected = ByteSelection {
        anchor: start + byte_of_unit_floor(text, selection.start),
        focus: start + byte_of_unit_floor(text, selection.end),
        affinity: Affinity::Downstream,
    };
    Some(context.replace(
        target,
        text,
        selected,
        Some(start..start + text.len()),
        events,
    ))
}

/// `SetComposition` with empty text and no composition, Blink's third
/// case (`InputMethodController::SetComposition`, for IMEs such as
/// IBus-Anthy): a range selection is deleted with no composition event, by
/// `DeleteSelection`'s `beforeinput`/`input` pair of `deleteContentBackward`
/// (`TypingCommand::GetInputType` for `kDeleteSelection`), and the selection
/// is then set `selection` UTF-16 units from where the deleted range began,
/// as `SetEditableSelectionOffsets` does. A collapsed selection changes
/// nothing.
fn delete_selection_for_empty_composition(
    context: Context,
    selection: &Range<usize>,
) -> Option<PreparedEdit> {
    let range = context.selection().range();
    if range.is_empty() {
        return None;
    }
    let value = apply_changes(&context.value, &[(range.clone(), String::new())]);
    let start = unit_of_byte(&context.value, range.start);
    let place = |offset: usize| byte_of_unit_floor(&value, start.saturating_add(offset));
    let after = ByteSelection {
        anchor: place(selection.start),
        focus: place(selection.end),
        affinity: Affinity::Downstream,
    };
    let events = input_pair(DELETE_CONTENT_BACKWARD, None, false);
    Some(context.replace(range, "", after, None, events))
}

fn commit_text(context: Context, text: &str, relative_cursor: i32) -> Option<PreparedEdit> {
    if !context.writable {
        return None;
    }
    let composition = context.composition();
    let target = composition
        .clone()
        .unwrap_or_else(|| context.selection().range());
    if composition.is_none() && text.is_empty() && target.is_empty() {
        return None;
    }
    let events = if composition.is_some() {
        vec![
            EditEvent::input(
                EditEventKind::BeforeInput,
                INSERT_COMPOSITION_TEXT,
                Some(text),
                true,
            ),
            EditEvent::composition(EditEventKind::CompositionUpdate, text),
            EditEvent::input(
                EditEventKind::Input,
                INSERT_COMPOSITION_TEXT,
                Some(text),
                true,
            ),
            EditEvent::composition(EditEventKind::CompositionEnd, text),
        ]
    } else {
        input_pair(INSERT_TEXT, Some(text), false)
    };
    let value = apply_changes(&context.value, &[(target.clone(), text.to_owned())]);
    let end_units = unit_of_byte(&value, target.start + text.len());
    let caret_units = i64::try_from(end_units)
        .unwrap_or(i64::MAX)
        .saturating_add(i64::from(relative_cursor))
        .clamp(0, i64::try_from(utf16_len(&value)).unwrap_or(i64::MAX));
    let caret = byte_of_unit_floor(&value, usize::try_from(caret_units).unwrap_or(0));
    Some(context.replace(
        target,
        text,
        ByteSelection::collapsed(caret, Affinity::Downstream),
        None,
        events,
    ))
}

fn finish_composition(context: Context, keep_selection: bool) -> Option<PreparedEdit> {
    let composition = context.composition()?;
    let text = context.composition_text().unwrap_or_default().to_owned();
    let selection = if keep_selection {
        context.selection()
    } else {
        ByteSelection::collapsed(composition.end, Affinity::Downstream)
    };
    let mut edit = PreparedEdit::select(
        context.host,
        context.value,
        SelectionAfter::Bytes(selection),
        None,
    );
    edit.events
        .push(EditEvent::composition(EditEventKind::CompositionEnd, &text));
    Some(edit)
}

fn composition_from_existing_text(context: Context, range: &Range<usize>) -> Option<PreparedEdit> {
    if !context.writable {
        return None;
    }
    let start = byte_of_unit_floor(&context.value, range.start.min(range.end));
    let end = byte_of_unit_floor(&context.value, range.start.max(range.end));
    if start == end {
        return finish_composition(context, true);
    }
    let starting = context.session.composition.is_none();
    let selection = context.selection();
    let data = context.value[start..end].to_owned();
    let mut edit = PreparedEdit::select(
        context.host,
        context.value,
        SelectionAfter::Bytes(selection),
        Some(start..end),
    );
    if starting {
        edit.events.push(EditEvent::composition(
            EditEventKind::CompositionStart,
            &data,
        ));
    }
    Some(edit)
}

fn delete_surrounding(context: Context, before: usize, after: usize) -> Option<PreparedEdit> {
    if !context.writable {
        return None;
    }
    let selection = context.selection();
    let range = selection.range();
    let start_units = unit_of_byte(&context.value, range.start);
    let end_units = unit_of_byte(&context.value, range.end);
    let length = utf16_len(&context.value);
    let before_start = byte_of_unit(&context.value, start_units.saturating_sub(before))?;
    let after_end = byte_of_unit(&context.value, end_units.saturating_add(after).min(length))?;
    let changes: Vec<(Range<usize>, String)> = [before_start..range.start, range.end..after_end]
        .into_iter()
        .filter(|range| !range.is_empty())
        .map(|range| (range, String::new()))
        .collect();
    if changes.is_empty() {
        return None;
    }
    let input_type = if before_start < range.start {
        DELETE_CONTENT_BACKWARD
    } else {
        DELETE_CONTENT_FORWARD
    };
    let composing = context.session.composition.is_some();
    let composition = context
        .composition()
        .and_then(|composition| map_range(&composition, &changes));
    let after_selection = ByteSelection {
        anchor: map_offset(selection.anchor, &changes),
        focus: map_offset(selection.focus, &changes),
        affinity: selection.affinity,
    };
    let events = input_pair(input_type, None, composing);
    Some(PreparedEdit {
        host: context.host,
        base: context.value,
        changes,
        selection: SelectionAfter::Bytes(after_selection),
        composition,
        events,
        commit_index: 1,
    })
}

fn extend_selection_and_delete(
    context: Context,
    before: usize,
    after: usize,
) -> Option<PreparedEdit> {
    if !context.writable {
        return None;
    }
    let range = context.selection().range();
    let start_units = unit_of_byte(&context.value, range.start);
    let end_units = unit_of_byte(&context.value, range.end);
    let length = utf16_len(&context.value);
    let start = byte_of_unit(&context.value, start_units.saturating_sub(before))?;
    let end = byte_of_unit(&context.value, end_units.saturating_add(after).min(length))?;
    context.delete(start..end, DELETE_CONTENT_BACKWARD)
}

fn insert(
    context: Context,
    text: &str,
    input_type: &'static str,
    data: Option<&str>,
) -> Option<PreparedEdit> {
    if !context.writable || text.is_empty() {
        return None;
    }
    let mut events = Vec::new();
    if let Some(composed) = context.composition_text() {
        events.push(EditEvent::composition(
            EditEventKind::CompositionEnd,
            composed,
        ));
    }
    events.extend(input_pair(input_type, data, false));
    let target = context.selection().range();
    let caret = ByteSelection::collapsed(target.start + text.len(), Affinity::Downstream);
    Some(context.replace(target, text, caret, None, events))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn offsets_follow_the_changes_before_them() {
        let changes = vec![(2..4, String::new()), (6..6, "xyz".to_owned())];
        assert_eq!(map_offset(0, &changes), 0);
        assert_eq!(map_offset(2, &changes), 2);
        assert_eq!(map_offset(3, &changes), 2, "inside a deletion: its start");
        assert_eq!(map_offset(4, &changes), 2);
        assert_eq!(map_offset(5, &changes), 3);
        assert_eq!(map_offset(6, &changes), 4, "at an insertion: before it");
        assert_eq!(map_offset(7, &changes), 8);
        assert_eq!(apply_changes("abcdefgh", &changes), "abefxyzgh");
        assert_eq!(map_range(&(2..4), &changes), None);
        assert_eq!(map_range(&(1..5), &changes), Some(1..3));
    }
}
