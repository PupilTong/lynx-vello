//! Inline box edges: where a non-atomic inline box's inline-start and
//! inline-end margin, border and padding go in the shaped text.
//!
//! CSS 2.2 §10.3.1 and §8.3, §8.4: an inline box's horizontal margins,
//! borders and padding take part in the line, and with `box-decoration-break:
//! slice` (css-break-3 §5.4, the initial value and the only one the engine
//! has) the start edge sits on the box's first fragment and the end edge on
//! its last. An edge belongs to the content next to it: it never creates a
//! soft wrap opportunity, and where one exists at the box boundary the line
//! breaks after the boxes ending there and before the boxes starting there
//! (css-text-3 §5.1 takes the opportunity from the characters on either side,
//! as if no boundary were there).
//!
//! Parley has no inline box with edges, and its atomic boxes cannot stand in
//! for one: the breaker breaks before any box that does not fit and marks an
//! opportunity after every box (`parley::layout::line_break`), so an edge
//! made of one would be an opportunity on both sides. Each edge is therefore
//! an *edge cluster* of text this pass lays into the paragraph core's
//! normalized text as an inserted segment
//! (`crate::text::paragraph::content::NormalizedBlock::splice`) — one
//! cluster, shaped at font size 0 and line height 0 so it adds no glyph
//! extent and no line box contribution, whose advance is the edge, written
//! in as the segment's extra letter-spacing (parley applies spacing after
//! each cluster):
//!
//! - **An end edge** is [`END_MARK`], a default-ignorable combining mark (UAX #14 class CM,
//!   grapheme `Extend`, bidi NSM). UAX #14 LB9 attaches it to the character before it, so the
//!   opportunity after it is the one between the content on either side, and bidi rule W1 gives it
//!   the direction of that character, so it sits on that character's logical-after side.
//! - **A start edge** is a copy of the content character that follows it, then [`START_MARK`],
//!   another such mark, which joins the copy into one grapheme. The opportunity before the copy is
//!   UAX #14's between the character before the box and the box's first character, because the copy
//!   *is* that character; it has that character's bidi class too, so it sits on its logical-before
//!   side. Where the copy would join or change what is around it (`start_cluster`) the cluster is
//!   [`FALLBACK_START`] alone (WORD JOINER, class WJ: no opportunity before or after it): before
//!   white space, a break, a format character, a mark or another character that extends the
//!   grapheme before it, an atomic box, or the end of the text, which is UAX #14's answer before
//!   nearly all of them anyway (classes SP, BK, CM, ZWJ, ZW, WJ, GL, BA); before a character that
//!   continues the grapheme before the box, where UAX #14 has no opportunity either (`LB8a`, LB9,
//!   LB26, LB28); and before a character of a cursively joining script, whose shaping would read
//!   the copy as a neighbour (parley passes each run the text around it as shaping context,
//!   `parley_engine-0.12.0/src/shape/shaper.rs:346-347`), or of a script whose words UAX #14
//!   segments by dictionary (class SA), whose segmentation the copy would change. In the last two
//!   the only opportunity kept at the box's start is the one after a space, Chromium's — where a
//!   joining script breaks anyway, and where a dictionary script's break at a box boundary inside
//!   unspaced text is lost (`docs/tracking/deviations.md`). A regional indicator is copied twice,
//!   or not at all, so that the pairs UAX #14 and UAX #29 count are unchanged.
//!
//! Both marks are variation selectors (VS17, VS18): default-ignorable, class CM, grapheme
//! `Extend`, bidi NSM, which is what the breaker, the grapheme segmenter and the bidi resolver
//! read. A font may map a variation sequence of the copied character and VS17 — the ideographic
//! variation sequences use VS17 onward — but the cluster is shaped at font size 0, so whatever
//! glyph it gets is never drawn and has no extent.
//!
//! A box's boundaries reach the core as markers
//! (`crate::text::paragraph::InlineItem::Marker`), whose places after white
//! space collapsing the core reports; [`place`] decides from them where each
//! edge lands.
//!
//! The line-break override the paragraph is shaped with
//! ([`BREAK_OVERRIDE`], handed to the core as its consumer's own) settles
//! the pairs the marks take part in: nothing breaks before a mark or after a
//! start mark, and the opportunity after an end mark is Chromium's between
//! the characters around it. A paragraph without inline box edges inserts
//! nothing and is shaped with the core's override alone.
//!
//! An edge next to an atomic box is no cluster: it is added to the atom's
//! advance (the core's `Additions::box_advance`, which [`EdgeAdditions`]
//! answers from [`super::boxes::InlineBoxes::slot_edges`]), because the
//! breaker already treats an atom as one unbreakable piece with an
//! opportunity on each side, and a cluster beside it would be separable from
//! it.
//!
//! Several edges meeting at one place — boxes closing, then boxes opening
//! — share at most one end cluster and one start cluster, which list their
//! edges in logical order ([`EdgeRun::members`]). A box whose content ends
//! with a forced break ends before the break, so its end edge stays on the
//! line the break ends, as Blink's does (`landing`).
//!
//! The inserted bytes count no source unit, so every offset a caller sees is
//! unchanged by them.
//!
//! Recorded approximations (`docs/tracking/deviations.md`, "Text layout"):
//!
//! - An end edge sits before the collapsible space a box ends with, so that space still hangs at a
//!   soft wrap after the box; the space is then drawn after the edge rather than inside it.
//! - Edges follow the logical start and end of a box's content, not its visual extremes: in a box
//!   whose first or last character runs against the box's own direction, the edge sits beside that
//!   character.
//! - A copy answers every UAX #14 rule that reads the pair at the box's start or reads back from
//!   it. A rule that reads past the box's first character (`LB15b`, `LB19a`, LB25, `LB28a`) reads
//!   the copy's successor, which is that character again, and can answer differently there.
//!   Chromium's pair table settles every pair of printable ASCII characters other than the space
//!   before UAX #14 is asked, so this is non-ASCII text, such as a Brahmic orthographic syllable a
//!   box boundary splits before its final virama (`LB28a`).
//! - A box that starts between the two regional indicators of one flag keeps no opportunity between
//!   the flags after it in the box (`start_cluster`).

use core::ops::Range;

use icu_properties::CodePointMapData;
use icu_properties::props::{
    GeneralCategory, GeneralCategoryGroup, GraphemeClusterBreak, IndicConjunctBreak, JoiningType,
    LineBreak,
};
use parley::{CHROMIUM_LINE_BREAK_OVERRIDE, LineBreakContext};

use super::InlineFlowItem;
use super::boxes::{self, BoxSpec, InlineBoxes};
use crate::text::paragraph::Additions;
use crate::text::paragraph::content::{
    InsertedSegment, Insertion, MarkerAt, MarkerPlaces, NormalizedBlock,
};
use crate::text::paragraph::shape::BreakOverride;

/// The mark that ends a start edge cluster, after the copied character:
/// VARIATION SELECTOR-17. What makes it fit is its classes — UAX #14 CM,
/// grapheme `Extend`, bidi NSM — and that the cluster is shaped at font
/// size 0, so a glyph a font maps for the sequence is never drawn.
pub(super) const START_MARK: char = '\u{E0100}';

/// An end edge cluster: VARIATION SELECTOR-18, of the same classes.
pub(super) const END_MARK: char = '\u{E0101}';

/// A start edge cluster where the next character cannot be copied: WORD
/// JOINER.
pub(super) const FALLBACK_START: char = '\u{2060}';

/// Which edge of an inline box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EdgeSide {
    Start,
    End,
}

/// One edge cluster in the normalized text.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct EdgeRun {
    pub(super) bytes: Range<u32>,
    /// The edges it carries in logical order, as `(span, side)`; its advance
    /// is their sum.
    pub(super) members: Vec<(u32, EdgeSide)>,
    /// The span whose font the cluster is shaped in: the innermost one.
    pub(super) style_span: u32,
}

/// One non-atomic inline box's place in the normalized text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SpanAt {
    /// The `Open` item.
    pub(super) item: u32,
    /// The box it is nested in.
    pub(super) parent: Option<u32>,
    /// From where its start edge begins to where its end edge ends.
    pub(super) bytes: Range<u32>,
    /// The atomic boxes in it, as slots.
    pub(super) slots: Range<u32>,
}

/// What happens at one byte of the normalized text, in source order.
#[derive(Debug, Clone, Copy)]
enum Event {
    Marker(u32),
    Atom(u32),
}

/// An edge cluster being assembled.
struct Building {
    side: EdgeSide,
    members: Vec<(u32, EdgeSide)>,
}

/// One box boundary, where the core met its marker.
#[derive(Debug, Clone, Copy)]
struct Marker {
    /// The box, as an index into the open-order box list.
    span: u32,
    open: bool,
    /// The text length when the boundary was met.
    byte: u32,
    /// The collapsible white space sequence open at the time, if any. Such a
    /// boundary lies after that sequence's first space in source order, so it
    /// lands after the space should the sequence keep one.
    pending: Option<u32>,
    /// How many atomic boxes came before it, which orders it against an atom
    /// at the same byte.
    slots_before: u32,
}

/// One box as its boundaries met it.
#[derive(Debug, Clone)]
struct SpanRecord {
    /// The `Open` item.
    item: u32,
    parent: Option<u32>,
    /// Its `Open` and its `Close` boundary, as indexes into the boundaries.
    open: u32,
    close: u32,
    /// The atomic boxes in it, as slots.
    slots: Range<u32>,
}

/// The boxes and their boundaries.
struct Boundaries {
    spans: Vec<SpanRecord>,
    markers: Vec<Marker>,
    /// Whether each collapsible white space sequence kept its space.
    pending_emitted: Vec<bool>,
    item_span: Vec<Option<u32>>,
}

impl Boundaries {
    /// Pairs the core's markers with the items: each `Open` and each
    /// `Close` item is one marker, in item order, then come the markers
    /// appended for the boxes the items left open
    /// ([`super::InlineFlow::new`]). A `Close` with no box open is ignored.
    fn of(places: MarkerPlaces, items: &[InlineFlowItem<'_>]) -> Self {
        let mut spans: Vec<SpanRecord> = Vec::new();
        let mut markers: Vec<Marker> = Vec::new();
        let mut open: Vec<u32> = Vec::new();
        let mut item_span = Vec::with_capacity(items.len());
        let mut found = places.markers.iter();
        // The next marker, as the boundary `opening` or closing `span`: its
        // index among the boundaries, and how many atoms came before it.
        let boundary = |found: &mut core::slice::Iter<'_, MarkerAt>,
                        span: u32,
                        opening: bool,
                        markers: &mut Vec<Marker>| {
            let at = found.next().expect("one marker per box boundary");
            debug_assert!(
                !opening
                    || items
                        .get(at.item as usize)
                        .is_some_and(|item| matches!(item, InlineFlowItem::Open(_))),
                "an opening boundary is an `Open` item's marker",
            );
            markers.push(Marker {
                span,
                open: opening,
                byte: at.byte,
                pending: at.space,
                slots_before: at.atoms_before,
            });
            (
                u32::try_from(markers.len() - 1).expect("marker count fits u32"),
                at.atoms_before,
            )
        };
        for (index, item) in items.iter().enumerate() {
            let item_index = u32::try_from(index).expect("item count fits u32");
            match item {
                InlineFlowItem::Open(_) => {
                    let span = u32::try_from(spans.len()).expect("box count fits u32");
                    let (marker, slots) = boundary(&mut found, span, true, &mut markers);
                    spans.push(SpanRecord {
                        item: item_index,
                        parent: open.last().copied(),
                        open: marker,
                        close: u32::MAX,
                        slots: slots..slots,
                    });
                    open.push(span);
                    item_span.push(Some(span));
                }
                InlineFlowItem::Close => {
                    let Some(span) = open.pop() else {
                        debug_assert!(false, "a close item matches an open one");
                        found.next();
                        item_span.push(None);
                        continue;
                    };
                    let (marker, slots) = boundary(&mut found, span, false, &mut markers);
                    let record = &mut spans[span as usize];
                    record.close = marker;
                    record.slots.end = slots;
                    item_span.push(Some(span));
                }
                InlineFlowItem::Run(_) | InlineFlowItem::Atom(_) => {
                    item_span.push(open.last().copied());
                }
            }
        }
        // A box the items never closed ends with the content.
        while let Some(span) = open.pop() {
            let (marker, slots) = boundary(&mut found, span, false, &mut markers);
            let record = &mut spans[span as usize];
            record.close = marker;
            record.slots.end = slots;
        }
        Self {
            spans,
            markers,
            pending_emitted: places.space_kept,
            item_span,
        }
    }
}

/// The walk over one paragraph's boundaries and atoms, and what it produces.
struct Placer<'a> {
    markers: &'a [Marker],
    records: &'a [SpanRecord],
    specs: &'a [BoxSpec],
    /// Whether each box has a start edge and an end edge to lay in.
    present: Vec<(bool, bool)>,
    /// Each box's bytes in the new text.
    span_bytes: Vec<Range<u32>>,
    edges: Vec<EdgeRun>,
    insertions: Vec<Insertion>,
    /// The boxes whose start and end edge each atom carries.
    slot_edges: Vec<(Option<u32>, Option<u32>)>,
    /// How many atom events the walk has passed.
    atoms_passed: u32,
}

/// One byte's events being walked: where the new text is, and the cluster
/// being assembled there.
struct Group<'g> {
    events: &'g [Event],
    /// The byte of the text before insertion the events are at.
    at: u32,
    cursor: u32,
    building: Option<Building>,
    /// What a start cluster at `at` is made of, from the content around it.
    start: StartCluster,
}

impl Group<'_> {
    /// What a start cluster flushed before the event at `position` is made
    /// of: the fallback when an atom of this byte comes first.
    fn start_after(&self, position: usize) -> StartCluster {
        if self.events[position + 1..]
            .iter()
            .any(|event| matches!(event, Event::Atom(_)))
        {
            StartCluster::Fallback
        } else {
            self.start
        }
    }
}

impl Placer<'_> {
    fn has_edge(&self, span: u32, side: EdgeSide) -> bool {
        let (start, end) = self.present[span as usize];
        match side {
            EdgeSide::Start => start,
            EdgeSide::End => end,
        }
    }

    /// Whether `event` takes part in the edge walk: an atom does, and a
    /// boundary with an edge to lay in.
    fn bears(&self, event: Event) -> bool {
        match event {
            Event::Atom(_) => true,
            Event::Marker(index) => {
                let marker = self.markers[index as usize];
                self.has_edge(
                    marker.span,
                    if marker.open {
                        EdgeSide::Start
                    } else {
                        EdgeSide::End
                    },
                )
            }
        }
    }

    /// The atom right after (`forward`) or right before the event at
    /// `position`, if the nearest event that takes part is one.
    fn neighbour(&self, group: &Group<'_>, position: usize, forward: bool) -> Option<u32> {
        let found = if forward {
            group.events[position + 1..]
                .iter()
                .find(|&&other| self.bears(other))
        } else {
            group.events[..position]
                .iter()
                .rev()
                .find(|&&other| self.bears(other))
        };
        match found {
            Some(&Event::Atom(slot)) => Some(slot),
            _ => None,
        }
    }

    /// Walks one byte's events, in source order.
    fn walk(&mut self, group: &mut Group<'_>) {
        for position in 0..group.events.len() {
            match group.events[position] {
                Event::Atom(_) => {
                    self.flush(group, StartCluster::Fallback);
                    self.atoms_passed += 1;
                }
                Event::Marker(index) => {
                    let marker = self.markers[index as usize];
                    if marker.open {
                        self.open(group, position, marker.span);
                    } else {
                        self.close(group, position, marker.span);
                    }
                }
            }
        }
        let start = group.start;
        self.flush(group, start);
    }

    /// A box starts: its start edge goes into the atom right after it, or
    /// into the start cluster being assembled.
    fn open(&mut self, group: &mut Group<'_>, position: usize, span: u32) {
        if !self.has_edge(span, EdgeSide::Start) {
            self.flush(group, group.start_after(position));
            self.span_bytes[span as usize].start = group.cursor;
            return;
        }
        if let Some(slot) = self.neighbour(group, position, true)
            && self.slot_edges[slot as usize].0.is_none()
        {
            self.slot_edges[slot as usize].0 = Some(span);
            self.span_bytes[span as usize].start = group.cursor;
            return;
        }
        if group
            .building
            .as_ref()
            .is_some_and(|building| building.side == EdgeSide::End)
        {
            self.flush(group, StartCluster::Fallback);
        }
        group
            .building
            .get_or_insert_with(|| Building {
                side: EdgeSide::Start,
                members: Vec::new(),
            })
            .members
            .push((span, EdgeSide::Start));
    }

    /// A box ends: its end edge goes into the atom right before it, if that
    /// atom is its own, or into the cluster being assembled — a start
    /// cluster when the box opened at this byte and so holds nothing.
    fn close(&mut self, group: &mut Group<'_>, position: usize, span: u32) {
        if !self.has_edge(span, EdgeSide::End) {
            self.flush(group, group.start_after(position));
            self.span_bytes[span as usize].end = group.cursor;
            return;
        }
        if let Some(slot) = self.neighbour(group, position, false)
            && self.records[span as usize].slots.contains(&slot)
            && self.slot_edges[slot as usize].1.is_none()
        {
            self.slot_edges[slot as usize].1 = Some(span);
            self.span_bytes[span as usize].end = group.cursor;
            return;
        }
        group
            .building
            .get_or_insert_with(|| Building {
                side: EdgeSide::End,
                members: Vec::new(),
            })
            .members
            .push((span, EdgeSide::End));
    }

    /// Emits the cluster being assembled, if any, at the group's cursor in
    /// the new text — before its old byte — and moves the cursor past it. A
    /// start cluster is made as `start` says.
    fn flush(&mut self, group: &mut Group<'_>, start: StartCluster) {
        let Some(done) = group.building.take() else {
            return;
        };
        let mut text = String::new();
        match done.side {
            EdgeSide::End => text.push(END_MARK),
            EdgeSide::Start => start.push_to(&mut text),
        }
        let length = u32::try_from(text.len()).expect("a cluster is a few bytes");
        let bytes = group.cursor..group.cursor + length;
        for &(span, side) in &done.members {
            let range = &mut self.span_bytes[span as usize];
            match side {
                EdgeSide::Start => range.start = bytes.start,
                EdgeSide::End => range.end = bytes.end,
            }
        }
        let style_span = match done.side {
            EdgeSide::Start => done.members.last(),
            EdgeSide::End => done.members.first(),
        }
        .map(|&(span, _)| span)
        .expect("a cluster carries an edge");
        let edge = EdgeRun {
            bytes,
            members: done.members,
            style_span,
        };
        self.insertions.push(Insertion {
            at: group.at,
            atoms_before: self.atoms_passed,
            text,
            style: self.specs[style_span as usize].edge_style.clone(),
            extra_spacing: boxes::edge_advance(self.specs, &edge),
            item: self.records[style_span as usize].item,
        });
        self.edges.push(edge);
        group.cursor += length;
    }
}

/// Where boundary `index` lands in `text`, the text before insertion.
///
/// One met while a whitespace span was open came after that span's first
/// space, so it follows the space if the span kept one — except a box's end,
/// which stays before a space its own content opened, so that space can
/// still hang at a soft wrap after it.
///
/// A box whose content ends with a forced break ends before that break, so
/// its end edge is on the line the break ends rather than at the start of
/// the next (UAX #14 LB4, LB5 would put a mark after the break there). CSS
/// does not define this; Blink's line breaker does the same
/// (`LineBreaker::HandleForcedLineBreak`: "Include following close tags",
/// for preserved newlines and `<br>`), and Gecko for preserved newlines.
/// `atoms` are the bytes of the atomic boxes, in order: one after the break
/// is content of the box after it, and so is a box that opens after it.
fn landing(boundaries: &Boundaries, text: &str, atoms: &[u32], index: usize) -> u32 {
    let markers = &boundaries.markers;
    let marker = markers[index];
    let after_space = u32::from(
        marker
            .pending
            .is_some_and(|id| boundaries.pending_emitted[id as usize]),
    );
    if marker.open {
        return marker.byte + after_space;
    }
    let open = boundaries.spans[marker.span as usize].open as usize;
    let opened = markers[open];
    if marker.pending.is_some() && opened.pending == marker.pending {
        // The box opened in the same span: it holds no content at all, and
        // stays with its start.
        return marker.byte + after_space;
    }
    let last_atom = (marker.slots_before as usize)
        .checked_sub(1)
        .map(|slot| atoms[slot]);
    match before_forced_break(text, marker.byte) {
        Some(byte)
            if landing(boundaries, text, atoms, open) <= byte
                && last_atom.is_none_or(|atom| atom <= byte)
                && markers[..index]
                    .iter()
                    .rev()
                    .take_while(|earlier| earlier.byte > byte)
                    .all(|earlier| !earlier.open) =>
        {
            byte
        }
        _ => marker.byte,
    }
}

/// Where the forced break `text` has right before byte `end` starts — the
/// carriage return of a CR LF pair, which UAX #14 LB5 keeps together — or
/// `None` without one.
fn before_forced_break(text: &str, end: u32) -> Option<u32> {
    let before = &text[..end as usize];
    let last = before.chars().next_back()?;
    if !is_mandatory_break(last) {
        return None;
    }
    let mut start = before.len() - last.len_utf8();
    if last == '\n' && before[..start].ends_with('\r') {
        start -= 1;
    }
    Some(u32::try_from(start).expect("text offsets fit u32"))
}

/// What [`place`] lays into a flow: its edge clusters, as the core's
/// inserted segments, and the edges beside atoms, as advance.
#[derive(Debug, Default)]
pub(super) struct EdgeAdditions {
    /// One segment per edge cluster ([`EdgeRun`]), ascending.
    inserted: Vec<InsertedSegment>,
    /// The advance beside each atom, before and after it in visual order,
    /// indexed by slot. Empty when no atom carries an edge.
    box_advance: Vec<(f32, f32)>,
}

impl Additions for EdgeAdditions {
    fn inserted(&self) -> &[InsertedSegment] {
        &self.inserted
    }

    fn box_advance(&self, slot: usize) -> (f32, f32) {
        self.box_advance.get(slot).copied().unwrap_or((0.0, 0.0))
    }

    /// [`BREAK_OVERRIDE`] once there is an edge cluster.
    fn break_override(&self) -> Option<BreakOverride> {
        (!self.inserted.is_empty()).then_some(BREAK_OVERRIDE)
    }
}

/// Lays the edge clusters of the inline boxes among `items` into `block`, a
/// flow's normalized text built from them — every `Open` and `Close` a
/// marker, met where `marker_places` says — and answers every box's place
/// and what was laid in; `specs` are the boxes' parameters in open order. An
/// edge next to an atom goes into the atom's advance instead
/// (`Additions::box_advance`).
pub(super) fn place(
    block: &mut NormalizedBlock,
    marker_places: MarkerPlaces,
    items: &[InlineFlowItem<'_>],
    specs: Vec<BoxSpec>,
) -> (InlineBoxes, EdgeAdditions) {
    if specs.is_empty() {
        return (InlineBoxes::default(), EdgeAdditions::default());
    }
    let boundaries = Boundaries::of(marker_places, items);
    let records = &boundaries.spans;
    let events = events_in_order(&boundaries, block);
    let mut placer = Placer {
        markers: &boundaries.markers,
        records,
        specs: &specs,
        present: specs
            .iter()
            .map(|spec| (spec.edges.has_start(), spec.edges.has_end()))
            .collect(),
        span_bytes: vec![0..0; records.len()],
        edges: Vec::new(),
        insertions: Vec::new(),
        slot_edges: vec![(None, None); block.boxes.len()],
        atoms_passed: 0,
    };
    let mut shift = 0;
    let mut start = 0;
    while start < events.len() {
        let at = events[start].0;
        let end = events[start..]
            .iter()
            .position(|event| event.0 != at)
            .map_or(events.len(), |offset| start + offset);
        let group: Vec<Event> = events[start..end].iter().map(|event| event.1).collect();
        let mut walked = Group {
            events: &group,
            at,
            cursor: at + shift,
            building: None,
            start: start_cluster(&block.text[..at as usize], &block.text[at as usize..]),
        };
        placer.walk(&mut walked);
        shift = walked.cursor - at;
        start = end;
    }

    let Placer {
        span_bytes,
        edges,
        insertions,
        slot_edges,
        ..
    } = placer;
    let inserted = block.splice(insertions);
    debug_assert!(
        inserted
            .iter()
            .map(|segment| &segment.bytes)
            .eq(edges.iter().map(|edge| &edge.bytes)),
        "every edge cluster is one inserted segment, where the walk put it",
    );
    let box_advance = if slot_edges
        .iter()
        .any(|&(start, end)| start.is_some() || end.is_some())
    {
        (0..slot_edges.len())
            .map(|slot| boxes::slot_edge_sides(&specs, &slot_edges, slot))
            .collect()
    } else {
        Vec::new()
    };
    // A box whose boundaries carried no cluster takes the places they landed
    // at; one with an edge took its cluster's.
    let spans = records
        .iter()
        .zip(&span_bytes)
        .map(|(record, bytes)| SpanAt {
            item: record.item,
            parent: record.parent,
            bytes: bytes.start..bytes.end.max(bytes.start),
            slots: record.slots.clone(),
        })
        .collect();
    let boxes = InlineBoxes {
        specs,
        spans,
        edges,
        item_span: boundaries.item_span,
        slot_edges,
        areas: Vec::new(),
    };
    (
        boxes,
        EdgeAdditions {
            inserted,
            box_advance,
        },
    )
}

/// The boundaries and atoms in source order, each with the byte of the text
/// before insertion it is at: a boundary where it lands, an atom at its own
/// byte. An atom is ordered against a boundary at the same byte by how many
/// atoms that boundary followed.
fn events_in_order(boundaries: &Boundaries, block: &NormalizedBlock) -> Vec<(u32, Event)> {
    let atoms: Vec<u32> = block.boxes.iter().map(|entry| entry.byte).collect();
    let mut events: Vec<(u32, u32, u8, u32, Event)> = boundaries
        .markers
        .iter()
        .enumerate()
        .map(|(index, marker)| {
            let event = u32::try_from(index).expect("marker count fits u32");
            (
                landing(boundaries, &block.text, &atoms, index),
                marker.slots_before,
                0,
                event,
                Event::Marker(event),
            )
        })
        .chain(block.boxes.iter().enumerate().map(|(slot, entry)| {
            let slot = u32::try_from(slot).expect("box count fits u32");
            (entry.byte, slot, 1, 0, Event::Atom(slot))
        }))
        .collect();
    events.sort_by_key(|&(byte, slots, kind, index, _)| (byte, slots, kind, index));
    events
        .into_iter()
        .map(|(byte, _, _, _, event)| (byte, event))
        .collect()
}

/// What a start edge cluster is made of ([`start_cluster`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartCluster {
    /// A copy of the box's first character, then [`START_MARK`].
    Copy(char),
    /// Two copies of the regional indicator the box's first flag starts
    /// with, then [`START_MARK`].
    RegionalPair(char),
    /// [`START_MARK`] alone: the box starts between the two regional
    /// indicators of one pair.
    Mark,
    /// [`FALLBACK_START`] alone.
    Fallback,
}

impl StartCluster {
    fn push_to(self, text: &mut String) {
        match self {
            Self::Copy(character) => {
                text.push(character);
                text.push(START_MARK);
            }
            Self::RegionalPair(indicator) => {
                text.push(indicator);
                text.push(indicator);
                text.push(START_MARK);
            }
            Self::Mark => text.push(START_MARK),
            Self::Fallback => text.push(FALLBACK_START),
        }
    }
}

fn general_category(character: char) -> GeneralCategory {
    CodePointMapData::<GeneralCategory>::new().get(character)
}

fn grapheme_break(character: char) -> GraphemeClusterBreak {
    CodePointMapData::<GraphemeClusterBreak>::new().get(character)
}

fn line_break(character: char) -> LineBreak {
    CodePointMapData::<LineBreak>::new().get(character)
}

fn conjunct_break(character: char) -> IndicConjunctBreak {
    CodePointMapData::<IndicConjunctBreak>::new().get(character)
}

/// The start cluster of a box that starts between `before` and `after`, the
/// text before insertion on either side of it. The properties are the
/// Unicode Character Database's, from `icu_properties`.
///
/// The box's first character `c` is copied unless the copy would join, or
/// change, what is around it:
///
/// - `c`'s `General_Category` is a mark (`Mn`, `Mc`, `Me`), a format character (`Cf`), a control
///   (`Cc`) or a separator (`Zs`, `Zl`, `Zp`): white space and breaks, characters that attach to
///   the one before them, and characters that take no glyph of their own.
/// - `c`'s `Grapheme_Cluster_Break` is not `Other`, `L`, `LV` or `LVT` (UAX #29 §3.1): it extends
///   the grapheme before it (`Extend`, which includes the emoji modifiers, `SpacingMark`, `ZWJ`),
///   or it is a vowel or trailing Hangul jamo, a prepended character, or a control.
/// - `c`'s `Line_Break` is `SA` (UAX #14): its script's words are found by dictionary, and the
///   segmenter would read the copy as part of a word.
/// - `c`'s `Joining_Type` is `D`, `R`, `L` or `C` (Unicode §9.2): parley shapes each run with the
///   text around it as context (`parley_engine-0.12.0/src/shape/shaper.rs:346-347`), and a
///   cursively joining script reads that context, so a copy before the box would give the box's
///   first letter a joined form.
/// - The character before the box and `c` are one grapheme by UAX #29: the one before is a `ZWJ`
///   (GB11, an emoji ZWJ sequence) or `Prepend` (`GB9b`), a leading jamo before a jamo or syllable
///   (GB6), or a linker such as a virama before a consonant (`GB9c`). The copy would join that
///   grapheme, so the edge would not be a cluster of its own.
///
/// The fallback is transparent to joining (`Joining_Type` `T`, as every `Cf`
/// character is), so a joining letter after it joins what precedes the box,
/// as a browser, which shapes across inline box boundaries, joins it.
///
/// A regional indicator is never copied as one character: UAX #14 `LB30a` and
/// UAX #29 GB12 pair indicators by counting them, and one more would shift
/// every pair after the box. Where the indicators before the box are even
/// in number, `c` starts a flag and the cluster copies it twice; where they
/// are odd, the box starts inside a flag, where neither rule breaks, and the
/// cluster is the start mark alone, which LB9 attaches to the indicator
/// before it without changing UAX #14's count. UAX #29's count restarts
/// after the mark, and so does the shaper's, which pairs indicators within
/// one run only: the box's indicators pair from its first, and the
/// opportunities between the flags after it fall inside clusters and are
/// lost (`docs/tracking/deviations.md`). No opportunity is made inside a
/// flag.
fn start_cluster(before: &str, after: &str) -> StartCluster {
    let Some(character) = after.chars().next() else {
        return StartCluster::Fallback;
    };
    if line_break(character) == LineBreak::RegionalIndicator {
        let indicators = before
            .chars()
            .rev()
            .filter(|&previous| {
                !matches!(
                    line_break(previous),
                    LineBreak::CombiningMark | LineBreak::ZWJ
                )
            })
            .take_while(|&previous| line_break(previous) == LineBreak::RegionalIndicator)
            .count();
        return if indicators % 2 == 0 {
            StartCluster::RegionalPair(character)
        } else {
            StartCluster::Mark
        };
    }
    let category = general_category(character);
    let grapheme = grapheme_break(character);
    let excluded_category = GeneralCategoryGroup::Mark.contains(category)
        || GeneralCategoryGroup::Separator.contains(category)
        || matches!(category, GeneralCategory::Format | GeneralCategory::Control);
    let excluded_grapheme = !matches!(
        grapheme,
        GraphemeClusterBreak::Other
            | GraphemeClusterBreak::L
            | GraphemeClusterBreak::LV
            | GraphemeClusterBreak::LVT
    );
    let joining = matches!(
        CodePointMapData::<JoiningType>::new().get(character),
        JoiningType::DualJoining
            | JoiningType::RightJoining
            | JoiningType::LeftJoining
            | JoiningType::JoinCausing
    );
    if excluded_category
        || excluded_grapheme
        || joining
        || line_break(character) == LineBreak::ComplexContext
        || joins_previous(before, character, grapheme)
    {
        return StartCluster::Fallback;
    }
    StartCluster::Copy(character)
}

/// Whether UAX #29 keeps `character`, whose `Grapheme_Cluster_Break` is
/// `grapheme`, in one grapheme with the text `before` it, by a rule that
/// reads the character before it: GB6, `GB9b`, `GB9c` and GB11. The rules that
/// read `character` alone (GB9, `GB9a`, GB12, GB13) are settled by the caller.
fn joins_previous(before: &str, character: char, grapheme: GraphemeClusterBreak) -> bool {
    let Some(previous) = before.chars().next_back() else {
        return false;
    };
    match grapheme_break(previous) {
        GraphemeClusterBreak::ZWJ | GraphemeClusterBreak::Prepend => return true,
        GraphemeClusterBreak::L if grapheme != GraphemeClusterBreak::Other => return true,
        _ => {}
    }
    if conjunct_break(character) != IndicConjunctBreak::Consonant {
        return false;
    }
    // `GB9c`: a consonant, then linkers and extenders with a linker among them.
    let mut linked = false;
    for earlier in before.chars().rev() {
        match conjunct_break(earlier) {
            IndicConjunctBreak::Linker => linked = true,
            IndicConjunctBreak::Extend => {}
            IndicConjunctBreak::Consonant => return linked,
            _ => return false,
        }
    }
    false
}

/// Whether `character` is a mandatory break (UAX #14 BK, CR, LF, NL), which
/// parley analyses no soft opportunity around.
const fn is_mandatory_break(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// The opportunities around the edge marks, then `rest` for every other
/// pair.
fn edge_breaks(
    context: LineBreakContext,
    rest: fn(LineBreakContext) -> Option<bool>,
) -> Option<bool> {
    if context.after == START_MARK || context.after == END_MARK || context.before == START_MARK {
        return Some(false);
    }
    if context.before == END_MARK {
        // The pair across the end edge: the character before it and the one
        // after. LB9 already makes ICU's answer that pair's, so only the
        // override's own answer needs moving across.
        let previous = context.before_before?;
        if is_mandatory_break(previous) {
            return Some(false);
        }
        let mut across = context;
        across.before = previous;
        across.before_before = None;
        return rest(across);
    }
    rest(context)
}

fn chromium(context: LineBreakContext) -> Option<bool> {
    CHROMIUM_LINE_BREAK_OVERRIDE(context)
}

const fn icu(_: LineBreakContext) -> Option<bool> {
    None
}

fn edge_breaks_chromium(context: LineBreakContext) -> Option<bool> {
    edge_breaks(context, chromium)
}

fn edge_breaks_icu(context: LineBreakContext) -> Option<bool> {
    edge_breaks(context, icu)
}

/// The line-break override a paragraph with edge clusters is shaped with,
/// handed to the core as its consumer's own: the marks' pairs settled first,
/// then Chromium's override or UAX #14 alone, whichever the paragraph is
/// analysed with (`crate::text::paragraph::shape`).
pub(super) const BREAK_OVERRIDE: BreakOverride = BreakOverride {
    over_chromium: &(edge_breaks_chromium as fn(LineBreakContext) -> Option<bool>),
    over_icu: &(edge_breaks_icu as fn(LineBreakContext) -> Option<bool>),
};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use parley::LineBreakOverrideFn;

    use super::super::{InlineAtom, InlineBoxOpen, InlineRun, lay_in};
    use super::*;
    use crate::text::inline::InlineEdges;
    use crate::text::paragraph::content::SourceMap;
    use crate::text::paragraph::{
        BreakStyle, Direction, RunStyle, TextSecurity, VerticalAlign, WhiteSpaceCollapse,
    };

    fn run<'src>(style: &'src RunStyle, text: &'src str) -> InlineFlowItem<'src> {
        InlineFlowItem::Run(InlineRun {
            text,
            style,
            white_space: WhiteSpaceCollapse::Collapse,
            text_security: TextSecurity::None,
            breaking: BreakStyle::default(),
        })
    }

    /// An inline box whose start edge is `start` and end edge `end` px of
    /// padding.
    fn open(style: &RunStyle, start: f32, end: f32) -> InlineFlowItem<'_> {
        InlineFlowItem::Open(InlineBoxOpen {
            style,
            breaking: BreakStyle::default(),
            direction: Direction::Ltr,
            edges: InlineEdges {
                start_inset: start,
                end_inset: end,
                ..InlineEdges::default()
            },
        })
    }

    fn atom() -> InlineFlowItem<'static> {
        InlineFlowItem::Atom(InlineAtom {
            vertical_align: VerticalAlign::Baseline,
        })
    }

    /// What the edge pass made of some items: the normalized text with the
    /// clusters in it, and the boxes' places.
    struct Laid {
        text: String,
        map: SourceMap,
        item_units: Vec<Range<u32>>,
        edges: Vec<EdgeRun>,
        spans: Vec<SpanAt>,
        item_span: Vec<Option<u32>>,
        slot_edges: Vec<(Option<u32>, Option<u32>)>,
    }

    fn css(items: &[InlineFlowItem<'_>]) -> Laid {
        let (flow, boxes) = lay_in(items);
        let block = flow.block;
        Laid {
            text: block.text,
            map: block.map,
            item_units: block.item_units,
            edges: boxes.edges,
            spans: boxes.spans,
            item_span: boxes.item_span,
            slot_edges: boxes.slot_edges,
        }
    }

    /// The text with each edge cluster written as a token: `<c` for a start
    /// cluster copying `c` (`<cc` for one copying it twice), `<^` for the
    /// start mark alone, `<_` for the fallback start cluster, `>` for an end
    /// cluster.
    fn shown(block: &Laid) -> String {
        let mut out = String::new();
        let mut byte = 0;
        while byte < block.text.len() {
            let at = u32::try_from(byte).expect("fits");
            if let Some(edge) = block.edges.iter().find(|edge| edge.bytes.start == at) {
                let cluster = &block.text[edge.bytes.start as usize..edge.bytes.end as usize];
                match cluster.chars().next().expect("a cluster has a character") {
                    END_MARK => out.push('>'),
                    FALLBACK_START => out.push_str("<_"),
                    START_MARK => out.push_str("<^"),
                    _ => {
                        out.push('<');
                        out.extend(cluster.chars().filter(|&character| character != START_MARK));
                    }
                }
                byte = edge.bytes.end as usize;
                continue;
            }
            let character = block.text[byte..].chars().next().expect("a character");
            out.push(character);
            byte += character.len_utf8();
        }
        out
    }

    #[test]
    fn a_start_edge_copies_the_character_it_precedes_and_counts_no_unit() {
        let style = RunStyle::default();
        let items = [
            run(&style, "aa "),
            open(&style, 5.0, 0.0),
            run(&style, "bb"),
            InlineFlowItem::Close,
            run(&style, " cc"),
        ];
        let block = css(&items);
        assert_eq!(shown(&block), "aa <bbb cc");
        // The source is still the eight characters the runs hold, and the
        // box's own boundaries hold none.
        assert_eq!(block.map.source_len(), 8);
        assert_eq!(block.item_units[1], 3..3);
        assert_eq!(block.item_units[3], 5..5);
        let edge = &block.edges[0];
        assert_eq!(edge.members, [(0, EdgeSide::Start)]);
        // The cluster starts where the box's first unit is, and ends before
        // it: it covers no unit at all.
        assert_eq!(block.map.byte_to_unit(&block.text, edge.bytes.start), 3);
        assert_eq!(block.map.unit_before(&block.text, edge.bytes.end), 3);
        assert_eq!(block.map.byte_to_unit(&block.text, edge.bytes.end), 3);
        assert_eq!(block.map.unit_to_byte(&block.text, 3), Some(edge.bytes.end));
        // The box runs from its start cluster to the end of its content; it
        // has no end edge, so nothing follows.
        assert_eq!(block.spans[0].bytes, edge.bytes.start..edge.bytes.end + 2);
        assert_eq!(block.item_span, [None, Some(0), Some(0), Some(0), None]);
    }

    #[test]
    fn the_space_that_survives_collapsing_keeps_its_side_of_the_edge() {
        let style = RunStyle::default();
        // The space outside the box comes first and survives; the box's own
        // collapses into it, so the start edge follows it.
        let outside = [
            run(&style, "foo "),
            open(&style, 5.0, 0.0),
            run(&style, " bar"),
            InlineFlowItem::Close,
        ];
        assert_eq!(shown(&css(&outside)), "foo <bbar");
        // The only space is the box's own: it follows the start edge, and a
        // space cannot be copied, so the cluster is the fallback.
        let inside = [
            run(&style, "foo"),
            open(&style, 5.0, 0.0),
            run(&style, " bar"),
            InlineFlowItem::Close,
        ];
        assert_eq!(shown(&css(&inside)), "foo<_ bar");
    }

    #[test]
    fn an_end_edge_stays_before_the_collapsible_space_its_box_ends_with() {
        let style = RunStyle::default();
        let trailing = [
            open(&style, 0.0, 5.0),
            run(&style, "bar "),
            InlineFlowItem::Close,
            run(&style, "baz"),
        ];
        let block = css(&trailing);
        assert_eq!(shown(&block), "bar> baz");
        // The space is still the box's source.
        assert_eq!(block.item_units[1], 0..4);
        let after = [
            open(&style, 0.0, 5.0),
            run(&style, "bar"),
            InlineFlowItem::Close,
            run(&style, " baz"),
        ];
        assert_eq!(shown(&css(&after)), "bar> baz");
    }

    #[test]
    fn an_end_edge_stays_before_the_forced_break_its_box_ends_with() {
        let style = RunStyle::default();
        let preserved = |text| {
            InlineFlowItem::Run(InlineRun {
                text,
                style: &style,
                white_space: WhiteSpaceCollapse::Preserve,
                text_security: TextSecurity::None,
                breaking: BreakStyle::default(),
            })
        };
        let ended = [
            open(&style, 0.0, 5.0),
            preserved("ab\n"),
            InlineFlowItem::Close,
            run(&style, "cd"),
        ];
        let block = css(&ended);
        assert_eq!(shown(&block), "ab>\ncd");
        // The break is still the box's source, and its own bytes.
        assert_eq!(block.item_units[1], 0..3);
        // Nested boxes ending there end before it together, inner first.
        let nested = [
            open(&style, 0.0, 5.0),
            open(&style, 0.0, 5.0),
            preserved("ab\n"),
            InlineFlowItem::Close,
            InlineFlowItem::Close,
        ];
        let block = css(&nested);
        assert_eq!(shown(&block), "ab>\n");
        assert_eq!(
            block.edges[0].members,
            [(1, EdgeSide::End), (0, EdgeSide::End)]
        );
        // What follows the break inside the box keeps the edge after it: an
        // empty box, an atom.
        let empty_after = [
            open(&style, 0.0, 5.0),
            preserved("ab\n"),
            open(&style, 2.0, 2.0),
            InlineFlowItem::Close,
            InlineFlowItem::Close,
        ];
        let block = css(&empty_after);
        assert_eq!(shown(&block), "ab\n<_");
        assert_eq!(
            block.edges[0].members,
            [(1, EdgeSide::Start), (1, EdgeSide::End), (0, EdgeSide::End)]
        );
        let atom_after = [
            open(&style, 0.0, 5.0),
            preserved("ab\n"),
            atom(),
            InlineFlowItem::Close,
        ];
        let block = css(&atom_after);
        assert_eq!(shown(&block), "ab\n");
        assert_eq!(block.slot_edges, [(None, Some(0))]);
        // A box opening after the break is not one ending with it.
        let opened_after = [
            preserved("ab\n"),
            open(&style, 0.0, 5.0),
            run(&style, "cd"),
            InlineFlowItem::Close,
        ];
        assert_eq!(shown(&css(&opened_after)), "ab\ncd>");
    }

    #[test]
    fn edges_meeting_at_one_place_share_one_end_and_one_start_cluster() {
        let style = RunStyle::default();
        let siblings = [
            open(&style, 3.0, 3.0),
            run(&style, "x"),
            InlineFlowItem::Close,
            open(&style, 3.0, 3.0),
            run(&style, "y"),
            InlineFlowItem::Close,
        ];
        let block = css(&siblings);
        assert_eq!(shown(&block), "<xx><yy>");
        let nested = [
            open(&style, 3.0, 3.0),
            open(&style, 2.0, 2.0),
            run(&style, "x"),
            InlineFlowItem::Close,
            InlineFlowItem::Close,
        ];
        let block = css(&nested);
        assert_eq!(shown(&block), "<xx>");
        assert_eq!(
            block.edges[0].members,
            [(0, EdgeSide::Start), (1, EdgeSide::Start)]
        );
        assert_eq!(block.edges[0].style_span, 1, "the innermost box's font");
        assert_eq!(
            block.edges[1].members,
            [(1, EdgeSide::End), (0, EdgeSide::End)]
        );
        assert_eq!(block.spans[1].parent, Some(0));
    }

    /// One box closing and the next opening inside one stretch of text put
    /// an end cluster and a start cluster at one byte of the text before
    /// insertion. The source map holds the text on either side of the pair
    /// and the two clusters, nothing between them, so every offset from the
    /// end cluster to the box's first character maps to the unit between
    /// `x` and `y` — as a start and as an end, which a line breaking right
    /// after the start cluster asks for.
    #[test]
    fn an_end_and_a_start_cluster_at_one_byte_map_to_the_unit_between_them() {
        let style = RunStyle::default();
        let siblings = [
            open(&style, 3.0, 3.0),
            run(&style, "x"),
            InlineFlowItem::Close,
            open(&style, 3.0, 3.0),
            run(&style, "y"),
            InlineFlowItem::Close,
        ];
        let block = css(&siblings);
        assert_eq!(shown(&block), "<xx><yy>");
        let end = block.edges[1].bytes.clone();
        let start = block.edges[2].bytes.clone();
        assert_eq!(block.edges[1].members, [(0, EdgeSide::End)]);
        assert_eq!(block.edges[2].members, [(1, EdgeSide::Start)]);
        assert_eq!(end.end, start.start);
        for byte in [end.start, start.start, start.end] {
            assert_eq!(block.map.byte_to_unit(&block.text, byte), 1, "{byte}");
            assert_eq!(block.map.unit_before(&block.text, byte), 1, "{byte}");
        }
        assert_eq!(block.map.unit_to_byte(&block.text, 1), Some(start.end));
    }

    #[test]
    fn an_empty_box_is_one_start_cluster_carrying_both_its_edges() {
        let style = RunStyle::default();
        let items = [
            run(&style, "a"),
            open(&style, 2.0, 3.0),
            InlineFlowItem::Close,
            run(&style, "b"),
        ];
        let block = css(&items);
        assert_eq!(shown(&block), "a<bb");
        assert_eq!(
            block.edges[0].members,
            [(0, EdgeSide::Start), (0, EdgeSide::End)]
        );
        assert_eq!(block.spans[0].bytes, block.edges[0].bytes);
        // A box with no edge takes part in nothing and inserts nothing.
        let bare = [
            run(&style, "a"),
            open(&style, 0.0, 0.0),
            InlineFlowItem::Close,
            run(&style, "b"),
        ];
        let block = css(&bare);
        assert_eq!(shown(&block), "ab");
        assert_eq!(block.spans[0].bytes, 1..1);
    }

    #[test]
    fn an_edge_beside_an_atom_is_laid_into_the_atoms_advance() {
        let style = RunStyle::default();
        let items = [
            run(&style, "a"),
            open(&style, 5.0, 7.0),
            atom(),
            InlineFlowItem::Close,
            run(&style, "b"),
        ];
        let block = css(&items);
        assert_eq!(shown(&block), "ab");
        assert!(block.edges.is_empty());
        assert_eq!(block.slot_edges, [(Some(0), Some(0))]);
        assert_eq!(block.spans[0].slots, 0..1);
    }

    #[test]
    fn a_joining_script_s_letter_is_never_copied() {
        let style = RunStyle::default();
        // A copy of the box's first Arabic letter would be the context its
        // shaping joins to; the fallback is transparent to joining.
        let arabic = [
            run(&style, "\u{628}"),
            open(&style, 5.0, 0.0),
            run(&style, "\u{628}\u{628}"),
            InlineFlowItem::Close,
        ];
        assert_eq!(shown(&css(&arabic)), "\u{628}<_\u{628}\u{628}");
        // Hebrew does not join: its letters are copied.
        let hebrew = [
            run(&style, "\u{5d0}"),
            open(&style, 5.0, 0.0),
            run(&style, "\u{5d1}"),
            InlineFlowItem::Close,
        ];
        assert_eq!(shown(&css(&hebrew)), "\u{5d0}<\u{5d1}\u{5d1}");
    }

    /// The text a box with a 5px start edge, holding `inside`, after
    /// `before` normalizes to.
    fn started(before: &str, inside: &str) -> String {
        let style = RunStyle::default();
        let items = [
            run(&style, before),
            open(&style, 5.0, 0.0),
            run(&style, inside),
            InlineFlowItem::Close,
        ];
        shown(&css(&items))
    }

    #[test]
    fn joining_and_dictionary_scripts_are_found_by_property() {
        // `Joining_Type` D or R: Old Uyghur, Chorasmian, Syriac, Mongolian,
        // N'Ko, Adlam.
        for letter in [
            '\u{10F70}',
            '\u{10FB0}',
            '\u{710}',
            '\u{1820}',
            '\u{7CA}',
            '\u{1E900}',
        ] {
            assert_eq!(started("a", &letter.to_string()), format!("a<_{letter}"));
        }
        // `Line_Break` SA: Thai, Tai Le, Tai Tham, Myanmar, Khmer.
        for letter in ['\u{E01}', '\u{1950}', '\u{1A20}', '\u{1000}', '\u{1780}'] {
            assert_eq!(started("a", &letter.to_string()), format!("a<_{letter}"));
        }
        // Neither: a Khmer symbol and a Buginese letter are `Line_Break` AL,
        // which the segmenter reads without a dictionary, and do not join.
        for letter in ['\u{19E0}', '\u{1A00}'] {
            assert_eq!(
                started("a", &letter.to_string()),
                format!("a<{letter}{letter}")
            );
        }
    }

    #[test]
    fn a_character_that_joins_the_grapheme_before_the_box_is_not_copied() {
        // A dependent vowel sign, spacing (Mc) and nonspacing (Mn), and an
        // emoji modifier (grapheme Extend) attach to the character before
        // the box.
        assert_eq!(started("\u{915}", "\u{93F}"), "\u{915}<_\u{93F}");
        assert_eq!(started("\u{915}", "\u{947}"), "\u{915}<_\u{947}");
        assert_eq!(started("\u{1F44B}", "\u{1F3FD}"), "\u{1F44B}<_\u{1F3FD}");
        // After a ZWJ the box's emoji continues an emoji ZWJ sequence (GB11);
        // with no ZWJ before it, it starts a grapheme and is copied.
        assert_eq!(
            started("\u{1F468}\u{200D}", "\u{1F469}"),
            "\u{1F468}\u{200D}<_\u{1F469}"
        );
        assert_eq!(started("a", "\u{1F469}"), "a<\u{1F469}\u{1F469}");
        // A consonant after a virama is one conjunct (`GB9c`); after a vowel
        // sign it starts the next.
        assert_eq!(
            started("\u{915}\u{94D}", "\u{937}"),
            "\u{915}\u{94D}<_\u{937}"
        );
        assert_eq!(
            started("\u{915}\u{93E}", "\u{937}"),
            "\u{915}\u{93E}<\u{937}\u{937}"
        );
        // A Hangul syllable after a leading jamo is one syllable (GB6).
        assert_eq!(started("\u{1100}", "\u{AC00}"), "\u{1100}<_\u{AC00}");
        assert_eq!(started("a", "\u{AC00}"), "a<\u{AC00}\u{AC00}");
    }

    #[test]
    fn a_regional_indicator_is_copied_twice_or_not_at_all() {
        let flags = "\u{1F1EB}\u{1F1F7}";
        // Even indicators before the box: it starts a flag, and two copies
        // keep the pairs after it as they were.
        assert_eq!(
            started("\u{1F1FA}\u{1F1F8}", flags),
            "\u{1F1FA}\u{1F1F8}<\u{1F1EB}\u{1F1EB}\u{1F1EB}\u{1F1F7}"
        );
        assert_eq!(
            started("a", flags),
            "a<\u{1F1EB}\u{1F1EB}\u{1F1EB}\u{1F1F7}"
        );
        // Odd: the box starts inside a flag, and the start mark alone
        // attaches to the indicator before it.
        assert_eq!(started("\u{1F1FA}", "\u{1F1F8}"), "\u{1F1FA}<^\u{1F1F8}");
    }

    #[test]
    fn a_segment_break_between_cjk_characters_is_removed_across_a_boundary() {
        let style = RunStyle::default();
        let items = [
            run(&style, "中"),
            open(&style, 1.0, 1.0),
            run(&style, "\n文"),
            InlineFlowItem::Close,
        ];
        // css-text-3 §4.1.3: the break between two characters of a script
        // without word separators goes, whatever box boundary lies between.
        assert_eq!(shown(&css(&items)), "中<文文>");
    }

    #[test]
    fn the_override_glues_the_marks_and_carries_the_pair_across_an_end_edge() {
        // `LineBreakContext` is non-exhaustive: each question is a context
        // parley handed an override, with its characters rewritten.
        let ask = |before_before: Option<char>, before: char, after: char| {
            probe_context(before_before, before, after, BREAK_OVERRIDE.over_chromium)
        };
        // Nothing breaks before a mark, or after a start mark.
        assert_eq!(ask(Some('a'), 'b', START_MARK), Some(false));
        assert_eq!(ask(Some('a'), 'b', END_MARK), Some(false));
        assert_eq!(ask(Some('b'), START_MARK, 'b'), Some(false));
        // After an end mark, Chromium's answer for the characters around it:
        // after a space an opportunity, between two letters none, and none
        // where a forced break came before the mark.
        assert_eq!(ask(Some(' '), END_MARK, 'b'), Some(true));
        assert_eq!(ask(Some('a'), END_MARK, 'b'), Some(false));
        assert_eq!(ask(Some('\n'), END_MARK, 'b'), Some(false));
        // Between two plain characters it is Chromium's override unchanged.
        assert_eq!(
            ask(None, ' ', 'b'),
            probe_context(None, ' ', 'b', CHROMIUM_LINE_BREAK_OVERRIDE)
        );
    }

    /// Calls `answer` with a context whose fields are the three characters,
    /// taken from a real analysis that calls an override with them.
    fn probe_context(
        before_before: Option<char>,
        before: char,
        after: char,
        answer: &LineBreakOverrideFn,
    ) -> Option<bool> {
        use std::sync::Mutex;
        static SEEN: Mutex<Option<LineBreakContext>> = Mutex::new(None);
        fn record(context: LineBreakContext) -> Option<bool> {
            let mut seen = SEEN.lock().expect("not poisoned");
            if seen.is_none() {
                *seen = Some(context);
            }
            None
        }
        static RECORD: &LineBreakOverrideFn = &(record as fn(LineBreakContext) -> Option<bool>);
        let _guard = PROBE.lock().expect("not poisoned");
        *SEEN.lock().expect("not poisoned") = None;
        let mut context = crate::text::TextContext::without_system_fonts();
        let (fonts, layouts, _) = context.shaping_parts();
        let mut builder = layouts.style_run_builder(fonts, "ab", 1.0, false);
        builder.set_line_break_override(Some(RECORD));
        let index = builder.push_style(parley::TextStyle::default());
        builder.push_style_run(index, 0..2);
        let _ = builder.build("ab");
        let mut context = SEEN
            .lock()
            .expect("not poisoned")
            .take()
            .expect("parley asks the override about the pair");
        context.before_before = before_before;
        context.before = before;
        context.after = after;
        answer(context)
    }

    /// Serializes the probes, which share one recording slot.
    static PROBE: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
