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

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    SetComposition {
        text: String,
        selection: Range<usize>,
    },
    CommitText {
        text: String,
        relative_cursor: i32,
    },
    FinishComposition {
        keep_selection: bool,
    },
    SetCompositionFromExistingText {
        range: Range<usize>,
    },
    DeleteSurrounding {
        before: usize,
        after: usize,
    },
    ExtendSelectionAndDelete {
        before: usize,
        after: usize,
    },
    SetSelection {
        range: Range<usize>,
    },
    SelectFromPoint {
        point: euclid::default::Point2D<f32>,
        extend: bool,
    },
    SelectWordAtPoint {
        point: euclid::default::Point2D<f32>,
    },
    SelectLineAtPoint {
        point: euclid::default::Point2D<f32>,
    },
    Insert(String),
    InsertLineBreak,
    DeleteBackward,
    DeleteForward,
    DeleteWordBackward,
    DeleteWordForward,
    Move {
        granularity: Granularity,
        direction: Direction,
        extend: bool,
    },
    SelectAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    Character,
    Word,
    LineBoundary,
    Line,
    Document,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Backward,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditEventKind {
    BeforeInput,
    Input,
    CompositionStart,
    CompositionUpdate,
    CompositionEnd,
}

impl EditEventKind {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditEvent {
    pub kind: EditEventKind,
    pub input_type: &'static str,
    pub data: Option<String>,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditOutcome {
    pub applied: bool,
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

#[derive(Clone, Copy, Debug)]
enum SelectionAfter {
    Bytes(ByteSelection),
    Layout(Selection),
}

#[derive(Clone, Debug)]
pub struct PreparedEdit {
    host: NodeId,
    base: String,
    changes: Vec<(Range<usize>, String)>,
    selection: SelectionAfter,
    composition: Option<Range<usize>>,
    events: Vec<EditEvent>,
    commit_index: usize,
}

impl PreparedEdit {
    #[must_use]
    pub const fn host(&self) -> NodeId {
        self.host
    }

    #[must_use]
    pub fn events(&self) -> &[EditEvent] {
        &self.events
    }

    #[must_use]
    pub const fn commit_index(&self) -> usize {
        self.commit_index
    }

    #[must_use]
    pub fn changes_value(&self) -> bool {
        !self.changes.is_empty()
    }

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

struct Context {
    host: NodeId,
    value: String,
    session: Session,
    writable: bool,
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

fn input_pair(input_type: &'static str, data: Option<&str>, composing: bool) -> Vec<EditEvent> {
    vec![
        EditEvent::input(EditEventKind::BeforeInput, input_type, data, composing),
        EditEvent::input(EditEventKind::Input, input_type, data, composing),
    ]
}

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

fn map_range(range: &Range<usize>, changes: &[(Range<usize>, String)]) -> Option<Range<usize>> {
    let mapped = map_offset(range.start, changes)..map_offset(range.end, changes);
    (!mapped.is_empty()).then_some(mapped)
}

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
    pub fn prepare_editing(&mut self, command: Command) -> Option<PreparedEdit> {
        self.editing.session.as_ref()?;
        if command.reads_layout() {
            self.layout();
        }
        let session = self.editing.session.clone()?;
        let host = session.host;
        let value = self.session_text(&session);
        let paragraph = self.session_paragraph(&session);
        let trusted = paragraph
            .as_ref()
            .is_none_or(|paragraph| paragraph.shows_value);
        if !trusted && !matches!(command, Command::SetSelection { .. } | Command::SelectAll) {
            return None;
        }
        let selection = session
            .bytes(paragraph.as_ref(), &value)
            .unwrap_or_else(|| ByteSelection::collapsed(value.len(), Affinity::Downstream));
        let writable =
            session.focused && self.is_editing_host(host) && self.host_is_plain_text(host);
        let context = Context {
            host,
            value,
            session,
            writable,
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

    pub fn commit_editing(&mut self, edit: PreparedEdit) -> EditOutcome
    where
        T: Default,
    {
        let Some(session) = self
            .editing
            .session
            .as_ref()
            .filter(|session| session.host == edit.host)
        else {
            return EditOutcome::refused();
        };
        if self.session_text(session) != edit.base {
            return EditOutcome::refused();
        }
        let changes_selection = !edit.changes.is_empty() || {
            let held = match (edit.selection, session.pending) {
                (SelectionAfter::Bytes(_), None) => {
                    session.bytes(self.session_paragraph(session).as_ref(), &edit.base)
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
            .expect("the selection host has a session");
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

    pub fn apply_editing(&mut self, command: Command) -> EditOutcome
    where
        T: Default,
    {
        match self.prepare_editing(command) {
            Some(edit) => self.commit_editing(edit),
            None => EditOutcome::refused(),
        }
    }

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
        if value_changes || session.pending != Some(selection) {
            self.editing.selectionchange_pending = true;
        }
        session.pending = Some(selection);
        session.composition = None;
        self.note_visual_mutation();
    }

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

fn shown(paragraph: &EditableParagraph<'_>, selection: Selection) -> Option<Selection> {
    let cut = paragraph.map.cut_byte();
    let past = |cursor: Cursor| paragraph.map.truncated() && cursor.index() >= cut;
    (!past(selection.anchor()) && !past(selection.focus())).then_some(selection)
}

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

const MAX_EDGE_STEPS: usize = 8;

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
