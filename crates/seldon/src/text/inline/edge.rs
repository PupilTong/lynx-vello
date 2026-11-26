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
//! - **An end edge** is [`END_MARK`], a combining mark (UAX #14 class CM, grapheme `Extend`, bidi
//!   NSM). UAX #14 LB9 attaches it to the character before it, so the opportunity after it is the
//!   one between the content on either side, and bidi rule W1 gives it the direction of that
//!   character, so it sits on that character's logical-after side.
//! - **A start edge** is a copy of the content character that follows it, then [`START_MARK`],
//!   another such mark, which joins the copy into one grapheme. The opportunity before the copy is
//!   UAX #14's between the character before the box and the box's first character, because the copy
//!   *is* that character; it has that character's bidi class too, so it sits on its logical-before
//!   side. Where the copy would join or change what is around it (`start_cluster`) the cluster is
//!   [`FALLBACK_START`] alone, a third such mark that the override gives WORD JOINER's
//!   opportunities (none before or after it, but Chromium's after a space): before white space, a
//!   break, a format character, a mark or another character that extends the grapheme before it, an
//!   atomic box, or the end of the text, which is UAX #14's answer before nearly all of them anyway
//!   (classes SP, BK, CM, ZWJ, ZW, WJ, GL, BA); before a character that continues the grapheme
//!   before the box, where UAX #14 has no opportunity either (`LB8a`, LB9, LB26, LB28); and before
//!   a character of a cursively joining script, whose shaping would read the copy as a neighbour
//!   (parley passes each run the text around it as shaping context,
//!   `parley_engine-0.13.0/src/shape/shaper.rs:370-371`), or of a script whose words UAX #14
//!   segments by dictionary (class SA), whose segmentation the copy would change. In the last two
//!   the only opportunity kept at the box's start is the one after a space, Chromium's — where a
//!   joining script breaks anyway, and where a dictionary script's break at a box boundary inside
//!   unspaced text is lost (`docs/tracking/deviations.md`). A regional indicator is copied twice,
//!   or not at all, so that the pairs UAX #14 and UAX #29 count are unchanged.
//!
//! The marks are combining enclosing marks (U+20E0, U+20DD, U+20DE): class CM, grapheme `Extend`,
//! bidi NSM, script `Inherited`, combining class 0 and joining type T — the classes of the
//! variation selectors VS17 and VS18 the edges were first made of, which is what the breaker, the
//! grapheme segmenter, the bidi resolver and the shaper read — and none is default-ignorable:
//! parley 0.13, as Blink does, adds no letter-spacing after a cluster whose first character is,
//! and the spacing is the edge (`parley-0.13.0/src/layout/spacing.rs:99-136`). Each cluster is
//! shaped at font size 0, so whatever glyph a font maps for it is never drawn and has no extent,
//! and the core asks no fallback font for it (`crate::text::paragraph::shape`).
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
//! the characters around it. The fallback start mark has WORD JOINER's: an
//! opportunity before it only where Chromium's override puts one after a
//! space, and none after it. A paragraph without inline box edges inserts
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

pub(super) const START_MARK: char = '\u{20E0}';

pub(super) const END_MARK: char = '\u{20DD}';

pub(super) const FALLBACK_START: char = '\u{20DE}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EdgeSide {
    Start,
    End,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EdgeRun {
    pub(super) bytes: Range<u32>,
    pub(super) members: Vec<(u32, EdgeSide)>,
    pub(super) style_span: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SpanAt {
    pub(super) item: u32,
    pub(super) parent: Option<u32>,
    pub(super) bytes: Range<u32>,
    pub(super) slots: Range<u32>,
}

#[derive(Debug, Clone, Copy)]
enum Event {
    Marker(u32),
    Atom(u32),
}

struct Building {
    side: EdgeSide,
    members: Vec<(u32, EdgeSide)>,
}

#[derive(Debug, Clone, Copy)]
struct Marker {
    span: u32,
    open: bool,
    byte: u32,
    pending: Option<u32>,
    slots_before: u32,
}

#[derive(Debug, Clone)]
struct SpanRecord {
    item: u32,
    parent: Option<u32>,
    open: u32,
    close: u32,
    slots: Range<u32>,
}

struct Boundaries {
    spans: Vec<SpanRecord>,
    markers: Vec<Marker>,
    pending_emitted: Vec<bool>,
    item_span: Vec<Option<u32>>,
}

impl Boundaries {
    fn of(places: MarkerPlaces, items: &[InlineFlowItem<'_>]) -> Self {
        let mut spans: Vec<SpanRecord> = Vec::new();
        let mut markers: Vec<Marker> = Vec::new();
        let mut open: Vec<u32> = Vec::new();
        let mut item_span = Vec::with_capacity(items.len());
        let mut found = places.markers.iter();
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

struct Placer<'a> {
    markers: &'a [Marker],
    records: &'a [SpanRecord],
    specs: &'a [BoxSpec],
    present: Vec<(bool, bool)>,
    span_bytes: Vec<Range<u32>>,
    edges: Vec<EdgeRun>,
    insertions: Vec<Insertion>,
    slot_edges: Vec<(Option<u32>, Option<u32>)>,
    atoms_passed: u32,
}

struct Group<'g> {
    events: &'g [Event],
    at: u32,
    cursor: u32,
    building: Option<Building>,
    start: StartCluster,
}

impl Group<'_> {
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
            style: self.specs[style_span as usize].edge_style(),
            extra_spacing: boxes::edge_advance(self.specs, &edge),
            item: self.records[style_span as usize].item,
        });
        self.edges.push(edge);
        group.cursor += length;
    }
}

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

#[derive(Debug, Default)]
pub(super) struct EdgeAdditions {
    inserted: Vec<InsertedSegment>,
    box_advance: Vec<(f32, f32)>,
}

impl Additions for EdgeAdditions {
    fn inserted(&self) -> &[InsertedSegment] {
        &self.inserted
    }

    fn box_advance(&self, slot: usize) -> (f32, f32) {
        self.box_advance.get(slot).copied().unwrap_or((0.0, 0.0))
    }

    fn break_override(&self) -> Option<BreakOverride> {
        (!self.inserted.is_empty()).then_some(BREAK_OVERRIDE)
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartCluster {
    Copy(char),
    RegionalPair(char),
    Mark,
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

const fn is_mandatory_break(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

fn edge_breaks(
    context: LineBreakContext,
    rest: fn(LineBreakContext) -> Option<bool>,
) -> Option<bool> {
    if context.after == START_MARK || context.after == END_MARK || context.before == START_MARK {
        return Some(false);
    }
    if context.before == FALLBACK_START {
        return Some(false);
    }
    if context.before == END_MARK {
        let previous = context.before_before?;
        if is_mandatory_break(previous) {
            return Some(false);
        }
        let mut across = context;
        across.before = previous;
        across.before_before = None;
        return rest(across);
    }
    if context.after == FALLBACK_START {
        return Some(rest(context) == Some(true));
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
        assert_eq!(block.map.source_len(), 8);
        assert_eq!(block.item_units[1], 3..3);
        assert_eq!(block.item_units[3], 5..5);
        let edge = &block.edges[0];
        assert_eq!(edge.members, [(0, EdgeSide::Start)]);
        assert_eq!(block.map.byte_to_unit(&block.text, edge.bytes.start), 3);
        assert_eq!(block.map.unit_before(&block.text, edge.bytes.end), 3);
        assert_eq!(block.map.byte_to_unit(&block.text, edge.bytes.end), 3);
        assert_eq!(block.map.unit_to_byte(&block.text, 3), Some(edge.bytes.end));
        assert_eq!(block.spans[0].bytes, edge.bytes.start..edge.bytes.end + 2);
        assert_eq!(block.item_span, [None, Some(0), Some(0), Some(0), None]);
    }

    #[test]
    fn the_space_that_survives_collapsing_keeps_its_side_of_the_edge() {
        let style = RunStyle::default();
        let outside = [
            run(&style, "foo "),
            open(&style, 5.0, 0.0),
            run(&style, " bar"),
            InlineFlowItem::Close,
        ];
        assert_eq!(shown(&css(&outside)), "foo <bbar");
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
        assert_eq!(block.item_units[1], 0..3);
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
        assert_eq!(block.edges, []);
        assert_eq!(block.slot_edges, [(Some(0), Some(0))]);
        assert_eq!(block.spans[0].slots, 0..1);
    }

    #[test]
    fn a_joining_script_s_letter_is_never_copied() {
        let style = RunStyle::default();
        let arabic = [
            run(&style, "\u{628}"),
            open(&style, 5.0, 0.0),
            run(&style, "\u{628}\u{628}"),
            InlineFlowItem::Close,
        ];
        assert_eq!(shown(&css(&arabic)), "\u{628}<_\u{628}\u{628}");
        let hebrew = [
            run(&style, "\u{5d0}"),
            open(&style, 5.0, 0.0),
            run(&style, "\u{5d1}"),
            InlineFlowItem::Close,
        ];
        assert_eq!(shown(&css(&hebrew)), "\u{5d0}<\u{5d1}\u{5d1}");
    }

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
        for letter in ['\u{E01}', '\u{1950}', '\u{1A20}', '\u{1000}', '\u{1780}'] {
            assert_eq!(started("a", &letter.to_string()), format!("a<_{letter}"));
        }
        for letter in ['\u{19E0}', '\u{1A00}'] {
            assert_eq!(
                started("a", &letter.to_string()),
                format!("a<{letter}{letter}")
            );
        }
    }

    #[test]
    fn a_character_that_joins_the_grapheme_before_the_box_is_not_copied() {
        assert_eq!(started("\u{915}", "\u{93F}"), "\u{915}<_\u{93F}");
        assert_eq!(started("\u{915}", "\u{947}"), "\u{915}<_\u{947}");
        assert_eq!(started("\u{1F44B}", "\u{1F3FD}"), "\u{1F44B}<_\u{1F3FD}");
        assert_eq!(
            started("\u{1F468}\u{200D}", "\u{1F469}"),
            "\u{1F468}\u{200D}<_\u{1F469}"
        );
        assert_eq!(started("a", "\u{1F469}"), "a<\u{1F469}\u{1F469}");
        assert_eq!(
            started("\u{915}\u{94D}", "\u{937}"),
            "\u{915}\u{94D}<_\u{937}"
        );
        assert_eq!(
            started("\u{915}\u{93E}", "\u{937}"),
            "\u{915}\u{93E}<\u{937}\u{937}"
        );
        assert_eq!(started("\u{1100}", "\u{AC00}"), "\u{1100}<_\u{AC00}");
        assert_eq!(started("a", "\u{AC00}"), "a<\u{AC00}\u{AC00}");
    }

    #[test]
    fn a_regional_indicator_is_copied_twice_or_not_at_all() {
        let flags = "\u{1F1EB}\u{1F1F7}";
        assert_eq!(
            started("\u{1F1FA}\u{1F1F8}", flags),
            "\u{1F1FA}\u{1F1F8}<\u{1F1EB}\u{1F1EB}\u{1F1EB}\u{1F1F7}"
        );
        assert_eq!(
            started("a", flags),
            "a<\u{1F1EB}\u{1F1EB}\u{1F1EB}\u{1F1F7}"
        );
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
        assert_eq!(shown(&css(&items)), "中<文文>");
    }

    #[test]
    fn the_override_glues_the_marks_and_carries_the_pair_across_an_end_edge() {
        let ask = |before_before: Option<char>, before: char, after: char| {
            probe_context(before_before, before, after, BREAK_OVERRIDE.over_chromium)
        };
        assert_eq!(ask(Some('a'), 'b', START_MARK), Some(false));
        assert_eq!(ask(Some('a'), 'b', END_MARK), Some(false));
        assert_eq!(ask(Some('b'), START_MARK, 'b'), Some(false));
        assert_eq!(ask(Some(' '), END_MARK, 'b'), Some(true));
        assert_eq!(ask(Some('a'), END_MARK, 'b'), Some(false));
        assert_eq!(ask(Some('\n'), END_MARK, 'b'), Some(false));
        assert_eq!(ask(Some('a'), ' ', FALLBACK_START), Some(true));
        assert_eq!(ask(Some(' '), 'a', FALLBACK_START), Some(false));
        assert_eq!(ask(Some('a'), FALLBACK_START, ' '), Some(false));
        assert_eq!(ask(Some('a'), FALLBACK_START, 'b'), Some(false));
        let icu = |before_before: Option<char>, before: char, after: char| {
            probe_context(before_before, before, after, BREAK_OVERRIDE.over_icu)
        };
        assert_eq!(icu(Some('a'), ' ', FALLBACK_START), Some(false));
        assert_eq!(icu(Some('a'), FALLBACK_START, 'b'), Some(false));
        for mark in [START_MARK, END_MARK, FALLBACK_START] {
            assert!(
                !icu_properties::CodePointSetData::new::<
                    icu_properties::props::DefaultIgnorableCodePoint,
                >()
                .contains(mark),
                "{mark:?}",
            );
        }
        assert_eq!(
            ask(None, ' ', 'b'),
            probe_context(None, ' ', 'b', CHROMIUM_LINE_BREAK_OVERRIDE)
        );
    }

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

    static PROBE: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
