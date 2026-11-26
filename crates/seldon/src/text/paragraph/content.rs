//! Flattened block content, CSS whitespace collapsing, and the source map.
//!
//! Lynx offsets — `text-maxlength`, the layout event's per-line ranges — are
//! UTF-16 code units over the *pre-collapse* flattened content, with each
//! atomic box counting exactly one unit; parley indexes are UTF-8 bytes over
//! the *post-collapse* string. This module owns both spaces and the mapping
//! between them. The map is a segment list proportional to structure (runs,
//! boxes, collapse events), never a second copy of the text: intra-segment
//! queries re-decode the one normalized `String` the paragraph needs anyway.
//! Every lookup is a binary search over the segments, and an intra-segment
//! offset is either arithmetic, in an ASCII or a masked segment, or a decode
//! of at most [`VERBATIM_DECODE`] bytes, the longest a segment with any other
//! character grows.
//!
//! The collapse rules are css-text-3 §4.1's, with two additions: source-unit
//! accounting, and boxes as content (an atomic box flushes pending whitespace
//! and resets the after-break state, the way parley's tree builder treats an
//! inline box).
//!
//! `white-space-collapse` is per run. Under `preserve` nothing collapses:
//! every space, tab and carriage return emits one byte and every segment
//! break is a preserved break, so the only thing the mode changes is which
//! source units survive — never how they are counted. A preserved tab is
//! emitted as a space and listed in [`NormalizedBlock::tabs`]; its advance is
//! the tab stop's business (`super::tab`), not the glyph's.
//!
//! A run under `-webkit-text-security` ([`TextRunItem::text_security`]) is
//! masked before any of that: every source character, white space and
//! newlines included, emits one mask character, so the run opens no
//! collapsible span and forces no break. Its characters are a
//! [`SegmentKind::Masked`] segment, whose offsets in both spaces are
//! arithmetic: one mask of [`MASK_BYTES`] bytes per source character, and
//! one or two units each.
//!
//! [`WhiteSpaceProcessing::Verbatim`] is the native Lynx `<text>`: no run's
//! `white-space-collapse` is read, every run is processed as `preserve`, and
//! with `first_line_only` everything after the first forced break is dropped
//! from the text while its source units stay counted.
//!
//! Two hooks serve a consumer that marks places in its content and lays
//! text of its own in there:
//!
//! - A marker ([`InlineItem::Marker`]) counts no source unit, emits no text, and leaves white space
//!   collapsing as it would be without it. The normalizer reports where each one fell
//!   ([`MarkerAt`]): the text length when it was met, and the collapsible white space sequence open
//!   then, with whether that sequence kept its space ([`MarkerPlaces::space_kept`]). The report is
//!   returned beside the block, not kept in it.
//! - [`NormalizedBlock::splice`] inserts consumer text into the normalized text after the fact:
//!   bytes with no source unit, each shaped in a style of its own, returned as [`InsertedSegment`]s
//!   for the consumer to keep (`super::Additions::inserted`). Every offset a caller reads is
//!   unchanged by them.

use core::ops::Range;

use super::style::{RunStyle, TextSecurity, VerticalAlign, WhiteSpaceCollapse};
use crate::geometry::Size;

#[derive(Clone, Copy, Debug)]
pub enum InlineItem<'src> {
    Run(TextRunItem<'src>),
    Box(InlineBoxSpec),
    Marker,
}

#[derive(Clone, Copy, Debug)]
pub struct TextRunItem<'src> {
    pub text: &'src str,
    pub style: &'src RunStyle,
    pub white_space: WhiteSpaceCollapse,
    pub text_security: TextSecurity,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineBoxSpec {
    pub id: u64,
    pub size: Size<f32>,
    pub baseline: Option<f32>,
    pub vertical_align: VerticalAlign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text) enum WhiteSpaceProcessing {
    Css,
    Verbatim { first_line_only: bool },
}

#[derive(Debug)]
pub(in crate::text) struct NormalizedBlock {
    pub(in crate::text) text: String,
    pub(in crate::text) ranges: Vec<StyledRange>,
    pub(in crate::text) boxes: Vec<BoxAt>,
    pub(in crate::text) map: SourceMap,
    pub(in crate::text) item_units: Vec<Range<u32>>,
    pub(in crate::text) tabs: Vec<u32>,
    pub(in crate::text) dropped_boxes: Vec<u32>,
}

#[derive(Debug)]
pub(in crate::text) struct MarkerPlaces {
    pub(in crate::text) markers: Vec<MarkerAt>,
    pub(in crate::text) space_kept: Vec<bool>,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::text) struct MarkerAt {
    pub(in crate::text) item: u32,
    pub(in crate::text) byte: u32,
    pub(in crate::text) space: Option<u32>,
    pub(in crate::text) atoms_before: u32,
}

#[derive(Debug, Clone)]
pub(in crate::text) struct Insertion {
    pub(in crate::text) at: u32,
    pub(in crate::text) atoms_before: u32,
    pub(in crate::text) text: String,
    pub(in crate::text) style: RunStyle,
    pub(in crate::text) extra_spacing: f32,
    pub(in crate::text) item: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::text) struct InsertedSegment {
    pub(in crate::text) bytes: Range<u32>,
    pub(in crate::text) style: RunStyle,
    pub(in crate::text) extra_spacing: f32,
    pub(in crate::text) item: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::text) struct StyledRange {
    pub(in crate::text) bytes: Range<u32>,
    pub(in crate::text) item: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text) struct BoxAt {
    pub(in crate::text) byte: u32,
    pub(in crate::text) unit: u32,
    pub(in crate::text) item: u32,
}

#[derive(Debug, Default)]
pub(in crate::text) struct SourceMap {
    segments: Vec<Segment>,
    source_len: u32,
    shown_len: u32,
    hanging_spaces: bool,
    final_empty_line: bool,
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    norm: Range2,
    units: Range2,
    kind: SegmentKind,
}

#[derive(Debug, Clone, Copy)]
struct Range2 {
    start: u32,
    end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentKind {
    Verbatim,
    Collapsed,
    Box,
    Inserted,
    Masked,
}

pub(in crate::text) const MASK_BYTES: u32 = 3;

const VERBATIM_DECODE: u32 = 128;

impl Segment {
    const fn is_ascii(&self) -> bool {
        self.units.end - self.units.start == self.norm.end - self.norm.start
    }

    const fn units_per_mask(&self) -> u32 {
        (self.units.end - self.units.start) / ((self.norm.end - self.norm.start) / MASK_BYTES)
    }

    fn units_before(&self, text: &str, byte: u32) -> u32 {
        if self.kind == SegmentKind::Masked {
            (byte - self.norm.start) / MASK_BYTES * self.units_per_mask()
        } else if self.is_ascii() {
            byte - self.norm.start
        } else {
            utf16_len(&text[self.norm.start as usize..byte as usize])
        }
    }
}

impl SourceMap {
    pub(in crate::text) const fn source_len(&self) -> u32 {
        self.source_len
    }

    pub(in crate::text) const fn shown_len(&self) -> u32 {
        self.shown_len
    }

    pub(in crate::text) const fn final_empty_line(&self) -> bool {
        self.final_empty_line
    }

    pub(in crate::text) fn boxes_before(&self, unit: u32) -> u32 {
        let before = self
            .segments
            .partition_point(|segment| segment.units.start < unit);
        let boxes = self.segments[..before]
            .iter()
            .filter(|segment| segment.kind == SegmentKind::Box)
            .count();
        u32::try_from(boxes).expect("a box count fits u32")
    }

    pub(in crate::text) fn byte_to_unit(&self, text: &str, byte: u32) -> u32 {
        let segment = self
            .segments
            .get(
                self.segments
                    .partition_point(|segment| segment.norm.end <= byte),
            )
            .expect("a start offset lies inside the normalized text");
        debug_assert!(segment.norm.start <= byte, "the segments cover the text");
        match segment.kind {
            SegmentKind::Verbatim | SegmentKind::Masked => {
                segment.units.start + segment.units_before(text, byte)
            }
            SegmentKind::Collapsed | SegmentKind::Inserted => segment.units.start,
            SegmentKind::Box => unreachable!("a box segment spans no bytes"),
        }
    }

    pub(in crate::text) fn unit_before(&self, text: &str, byte: u32) -> u32 {
        let before = self.segments.partition_point(|segment| {
            segment.norm.end < byte || (segment.norm.end == byte && segment.norm.start < byte)
        });
        let mut result = before
            .checked_sub(1)
            .map_or(0, |last| self.segments[last].units.end);
        for segment in &self.segments[before..] {
            if segment.norm.start == byte && segment.norm.end == byte {
                match segment.kind {
                    SegmentKind::Box => break,
                    SegmentKind::Collapsed => result = segment.units.end,
                    SegmentKind::Verbatim | SegmentKind::Masked | SegmentKind::Inserted => {
                        unreachable!("verbatim, masked and inserted segments are never empty")
                    }
                }
            } else if matches!(segment.kind, SegmentKind::Verbatim | SegmentKind::Masked)
                && segment.norm.start < byte
            {
                result = segment.units.start + segment.units_before(text, byte);
                break;
            } else {
                break;
            }
        }
        result
    }

    pub(in crate::text) fn visible_end(&self, text: &str, start: u32, end: u32) -> u32 {
        let mut visible = end;
        for (offset, character) in text[start as usize..end as usize].char_indices().rev() {
            let byte = start + u32::try_from(offset).expect("text fits u32");
            let trailing = match character {
                '\n' => true,
                ' ' => self.hanging_spaces || self.is_collapsed_byte(byte),
                _ => false,
            };
            if !trailing {
                break;
            }
            visible = byte;
        }
        visible
    }

    fn is_collapsed_byte(&self, byte: u32) -> bool {
        let index = self
            .segments
            .partition_point(|segment| segment.norm.end <= byte);
        self.segments[index..]
            .iter()
            .take_while(|segment| segment.norm.start <= byte)
            .any(|segment| segment.kind == SegmentKind::Collapsed && segment.norm.start == byte)
    }

    pub(in crate::text) fn unit_to_byte(&self, text: &str, unit: u32) -> Option<u32> {
        if unit >= self.shown_len {
            return None;
        }
        let segment = self
            .segments
            .get(
                self.segments
                    .partition_point(|segment| segment.units.end <= unit),
            )
            .expect("every source unit lies in a segment");
        debug_assert!(segment.units.start <= unit, "the segments cover the units");
        match segment.kind {
            SegmentKind::Verbatim if segment.is_ascii() => {
                Some(segment.norm.start + (unit - segment.units.start))
            }
            SegmentKind::Verbatim => {
                let mut remaining = unit - segment.units.start;
                let mut byte = segment.norm.start;
                for character in
                    text[segment.norm.start as usize..segment.norm.end as usize].chars()
                {
                    let width = u32::try_from(character.len_utf16()).expect("1 or 2");
                    if remaining < width {
                        break;
                    }
                    remaining -= width;
                    byte += u32::try_from(character.len_utf8()).expect("1 to 4");
                }
                Some(byte)
            }
            SegmentKind::Masked => Some(
                segment.norm.start
                    + (unit - segment.units.start) / segment.units_per_mask() * MASK_BYTES,
            ),
            SegmentKind::Collapsed | SegmentKind::Box => Some(segment.norm.start),
            SegmentKind::Inserted => unreachable!("an inserted segment holds no source unit"),
        }
    }
}

fn utf16_len(text: &str) -> u32 {
    text.chars()
        .map(|character| u32::try_from(character.len_utf16()).expect("1 or 2"))
        .sum()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Space,
    SegmentBreak,
    Suppressed,
}

struct Pending {
    unit_start: u32,
    item: u32,
    kind: PendingKind,
    id: Option<u32>,
}

struct Normalizer {
    text: String,
    ranges: Vec<StyledRange>,
    boxes: Vec<BoxAt>,
    segments: Vec<Segment>,
    units: u32,
    pending: Option<Pending>,
    at_line_start: bool,
    tabs: Vec<u32>,
    processing: WhiteSpaceProcessing,
    dropped_from: Option<u32>,
    dropped_boxes: Vec<u32>,
    markers: Vec<MarkerAt>,
    space_kept: Vec<bool>,
}

const OBJECT_REPLACEMENT: char = '\u{FFFC}';

pub(in crate::text) fn normalize(
    items: &[InlineItem<'_>],
    processing: WhiteSpaceProcessing,
) -> (NormalizedBlock, MarkerPlaces) {
    let mut state = Normalizer {
        text: String::new(),
        ranges: Vec::new(),
        boxes: Vec::new(),
        segments: Vec::new(),
        units: 0,
        pending: None,
        at_line_start: processing == WhiteSpaceProcessing::Css,
        tabs: Vec::new(),
        processing,
        dropped_from: None,
        dropped_boxes: Vec::new(),
        markers: Vec::new(),
        space_kept: Vec::new(),
    };

    let mut item_units = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_index = u32::try_from(index).expect("item count fits u32");
        let start = state.units;
        match item {
            InlineItem::Run(run) => state.consume_run(run, item_index),
            InlineItem::Box(_) => state.consume_box(item_index),
            InlineItem::Marker => state.mark(item_index),
        }
        item_units.push(start..state.units);
    }
    state.finish(item_units)
}

impl Normalizer {
    fn consume_run(&mut self, run: &TextRunItem<'_>, item: u32) {
        let white_space = match self.processing {
            WhiteSpaceProcessing::Verbatim { .. } => WhiteSpaceCollapse::Preserve,
            WhiteSpaceProcessing::Css => run.white_space,
        };
        let preserve_breaks = white_space != WhiteSpaceCollapse::Collapse;
        let preserve_spaces = matches!(
            white_space,
            WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces
        );
        let mask = run.text_security.mask();
        for character in run.text.chars() {
            if self.dropped_from.is_some() {
                self.units += u32::try_from(character.len_utf16()).expect("1 or 2");
                continue;
            }
            if let Some(mask) = mask {
                self.flush_pending(mask);
                self.emit_masked(
                    mask,
                    u32::try_from(character.len_utf16()).expect("1 or 2"),
                    item,
                );
                self.at_line_start = false;
                continue;
            }
            match character {
                '\n' if preserve_breaks => self.preserved_break(character, item),
                '\n' => self.queue_segment_break(item),
                ' ' | '\t' | '\r' if preserve_spaces => self.preserved_space(character, item),
                ' ' | '\t' | '\r' => self.queue_space(character, item),
                _ => {
                    self.flush_pending(character);
                    self.emit_verbatim(character, item);
                    self.at_line_start = false;
                }
            }
        }
    }

    fn mark(&mut self, item: u32) {
        let space_kept = &mut self.space_kept;
        let space = self.pending.as_mut().map(|pending| {
            *pending.id.get_or_insert_with(|| {
                space_kept.push(false);
                u32::try_from(space_kept.len() - 1).expect("sequence count fits u32")
            })
        });
        self.markers.push(MarkerAt {
            item,
            byte: self.byte_len(),
            space,
            atoms_before: u32::try_from(self.boxes.len()).expect("box count fits u32"),
        });
    }

    fn consume_box(&mut self, item: u32) {
        if self.dropped_from.is_some() {
            self.units += 1;
            self.dropped_boxes.push(item);
            return;
        }
        self.flush_pending(OBJECT_REPLACEMENT);
        let byte = self.byte_len();
        self.boxes.push(BoxAt {
            byte,
            unit: self.units,
            item,
        });
        self.push_segment(SegmentKind::Box, byte..byte, 1);
        self.at_line_start = false;
    }

    fn queue_space(&mut self, character: char, item: u32) {
        debug_assert_eq!(character.len_utf16(), 1);
        if self.pending.is_none() {
            self.pending = Some(Pending {
                unit_start: self.units,
                item,
                kind: if self.at_line_start {
                    PendingKind::Suppressed
                } else {
                    PendingKind::Space
                },
                id: None,
            });
        }
        self.units += 1;
    }

    fn queue_segment_break(&mut self, item: u32) {
        if let Some(pending) = &mut self.pending {
            if pending.kind == PendingKind::Space {
                pending.kind = PendingKind::SegmentBreak;
                pending.item = item;
            }
        } else {
            self.pending = Some(Pending {
                unit_start: self.units,
                item,
                kind: if self.at_line_start {
                    PendingKind::Suppressed
                } else {
                    PendingKind::SegmentBreak
                },
                id: None,
            });
        }
        self.units += 1;
    }

    fn preserved_space(&mut self, character: char, item: u32) {
        debug_assert_eq!(character.len_utf16(), 1);
        self.flush_pending(' ');
        if character == '\t' {
            self.tabs.push(self.byte_len());
        }
        self.emit_verbatim(' ', item);
        self.at_line_start = false;
    }

    fn preserved_break(&mut self, character: char, item: u32) {
        if let Some(pending) = self.pending.take() {
            self.close_collapsed(pending.unit_start, 0);
        }
        self.emit_verbatim(character, item);
        self.at_line_start = true;
        if self.processing
            == (WhiteSpaceProcessing::Verbatim {
                first_line_only: true,
            })
        {
            self.dropped_from = Some(self.units);
        }
    }

    fn flush_pending(&mut self, next: char) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let emit = match pending.kind {
            PendingKind::Suppressed => false,
            PendingKind::Space => true,
            PendingKind::SegmentBreak => {
                let previous = if self
                    .boxes
                    .last()
                    .is_some_and(|entry| entry.byte == self.byte_len())
                {
                    Some(OBJECT_REPLACEMENT)
                } else {
                    self.last_emitted_char()
                };
                !should_remove_segment_break(previous, next)
            }
        };
        if emit {
            let start = self.byte_len();
            self.text.push(' ');
            self.record_style(start, pending.item);
            self.close_collapsed(pending.unit_start, 1);
            if let Some(id) = pending.id {
                self.space_kept[id as usize] = true;
            }
        } else {
            self.close_collapsed(pending.unit_start, 0);
        }
    }

    fn close_collapsed(&mut self, unit_start: u32, bytes: u32) {
        let end = self.byte_len();
        self.segments.push(Segment {
            norm: Range2 {
                start: end - bytes,
                end,
            },
            units: Range2 {
                start: unit_start,
                end: self.units,
            },
            kind: SegmentKind::Collapsed,
        });
    }

    fn emit_verbatim(&mut self, character: char, item: u32) {
        let start = self.byte_len();
        self.text.push(character);
        self.record_style(start, item);
        self.push_segment(
            SegmentKind::Verbatim,
            start..self.byte_len(),
            u32::try_from(character.len_utf16()).expect("1 or 2"),
        );
    }

    fn emit_masked(&mut self, mask: char, units: u32, item: u32) {
        debug_assert_eq!(mask.len_utf8(), MASK_BYTES as usize);
        let start = self.byte_len();
        self.text.push(mask);
        self.record_style(start, item);
        self.push_segment(SegmentKind::Masked, start..self.byte_len(), units);
    }

    fn push_segment(&mut self, kind: SegmentKind, norm: Range<u32>, units: u32) {
        let unit_end = self.units + units;
        if kind == SegmentKind::Masked
            && let Some(last) = self.segments.last_mut()
            && last.kind == SegmentKind::Masked
            && last.norm.end == norm.start
            && last.units.end == self.units
            && last.units_per_mask() == units
        {
            last.norm.end = norm.end;
            last.units.end = unit_end;
        } else if kind == SegmentKind::Verbatim
            && let Some(last) = self.segments.last_mut()
            && last.kind == SegmentKind::Verbatim
            && last.norm.end == norm.start
            && last.units.end == self.units
            && (norm.end - last.norm.start <= VERBATIM_DECODE
                || unit_end - last.units.start == norm.end - last.norm.start)
        {
            last.norm.end = norm.end;
            last.units.end = unit_end;
        } else {
            self.segments.push(Segment {
                norm: Range2 {
                    start: norm.start,
                    end: norm.end,
                },
                units: Range2 {
                    start: self.units,
                    end: unit_end,
                },
                kind,
            });
        }
        self.units = unit_end;
    }

    fn record_style(&mut self, start: u32, item: u32) {
        let end = self.byte_len();
        if let Some(last) = self.ranges.last_mut()
            && last.item == item
            && last.bytes.end == start
        {
            last.bytes.end = end;
        } else {
            self.ranges.push(StyledRange {
                bytes: start..end,
                item,
            });
        }
    }

    fn last_emitted_char(&self) -> Option<char> {
        self.text.chars().next_back()
    }

    fn byte_len(&self) -> u32 {
        u32::try_from(self.text.len()).expect("normalized text fits u32")
    }

    fn finish(mut self, item_units: Vec<Range<u32>>) -> (NormalizedBlock, MarkerPlaces) {
        if let Some(pending) = self.pending.take() {
            let content_seen = !self.text.is_empty() || !self.boxes.is_empty();
            if content_seen && pending.kind != PendingKind::Suppressed {
                let start = self.byte_len();
                self.text.push(' ');
                self.record_style(start, pending.item);
                if let Some(id) = pending.id {
                    self.space_kept[id as usize] = true;
                }
                self.segments.push(Segment {
                    norm: Range2 {
                        start,
                        end: start + 1,
                    },
                    units: Range2 {
                        start: pending.unit_start,
                        end: self.units,
                    },
                    kind: SegmentKind::Collapsed,
                });
            } else {
                self.close_collapsed(pending.unit_start, 0);
            }
        }
        debug_assert!(
            self.boxes
                .iter()
                .all(|entry| self.text.is_char_boundary(entry.byte as usize)),
            "box indexes are emission positions, always char boundaries",
        );
        let block = NormalizedBlock {
            text: self.text,
            ranges: self.ranges,
            boxes: self.boxes,
            map: SourceMap {
                segments: self.segments,
                source_len: self.units,
                shown_len: self.dropped_from.unwrap_or(self.units),
                hanging_spaces: matches!(self.processing, WhiteSpaceProcessing::Verbatim { .. }),
                final_empty_line: self.processing
                    == (WhiteSpaceProcessing::Verbatim {
                        first_line_only: false,
                    }),
            },
            item_units,
            tabs: self.tabs,
            dropped_boxes: self.dropped_boxes,
        };
        let places = MarkerPlaces {
            markers: self.markers,
            space_kept: self.space_kept,
        };
        (block, places)
    }
}

impl NormalizedBlock {
    pub(in crate::text) fn splice(&mut self, insertions: Vec<Insertion>) -> Vec<InsertedSegment> {
        if insertions.is_empty() {
            return Vec::new();
        }
        debug_assert!(
            insertions.windows(2).all(
                |pair| (pair[0].at, pair[0].atoms_before) <= (pair[1].at, pair[1].atoms_before)
            ),
            "insertions come in text order",
        );
        let shifts = Shifts {
            insertions: &insertions,
        };
        let box_bytes: Vec<u32> = self
            .boxes
            .iter()
            .enumerate()
            .map(|(slot, entry)| {
                let slot = u32::try_from(slot).expect("box count fits u32");
                entry.byte + shifts.before_box(entry.byte, slot)
            })
            .collect();
        for (entry, &byte) in self.boxes.iter_mut().zip(&box_bytes) {
            entry.byte = byte;
        }

        let mut ranges = Vec::with_capacity(self.ranges.len() + insertions.len());
        for range in &self.ranges {
            let mut from = range.bytes.start;
            for point in shifts.cuts(range.bytes.start, range.bytes.end) {
                let moved = shifts.before(from, true);
                ranges.push(StyledRange {
                    bytes: from + moved..point + moved,
                    item: range.item,
                });
                from = point;
            }
        }
        self.ranges = ranges;
        for tab in &mut self.tabs {
            *tab += shifts.before(*tab, true);
        }
        let segments = self.spliced_segments(&shifts, &box_bytes);
        debug_assert!(
            segments.iter().all(|segment| {
                matches!(segment.kind, SegmentKind::Collapsed | SegmentKind::Box)
                    || segment.norm.start < segment.norm.end
            }),
            "verbatim, masked and inserted segments are never empty",
        );
        self.map.segments = segments;

        let mut text = String::with_capacity(
            self.text.len()
                + insertions
                    .iter()
                    .map(|insertion| insertion.text.len())
                    .sum::<usize>(),
        );
        let mut copied = 0;
        let mut moved = 0;
        let mut inserted = Vec::with_capacity(insertions.len());
        for insertion in insertions {
            text.push_str(&self.text[copied..insertion.at as usize]);
            text.push_str(&insertion.text);
            copied = insertion.at as usize;
            let length = u32::try_from(insertion.text.len()).expect("fits u32");
            let start = insertion.at + moved;
            moved += length;
            inserted.push(InsertedSegment {
                bytes: start..start + length,
                style: insertion.style,
                extra_spacing: insertion.extra_spacing,
                item: insertion.item,
            });
        }
        text.push_str(&self.text[copied..]);
        self.text = text;
        inserted
    }

    fn spliced_segments(&mut self, shifts: &Shifts<'_>, box_bytes: &[u32]) -> Vec<Segment> {
        let old = core::mem::take(&mut self.map.segments);
        let mut sorted: Vec<(u32, u8, Segment)> =
            Vec::with_capacity(old.len() + shifts.insertions.len());
        let at = |byte: u32| Range2 {
            start: byte,
            end: byte,
        };
        let mut box_slots = box_bytes.iter();
        for segment in old {
            if segment.kind == SegmentKind::Box {
                let byte = *box_slots.next().expect("one box segment per atom");
                sorted.push((
                    byte,
                    0,
                    Segment {
                        norm: at(byte),
                        ..segment
                    },
                ));
            } else if segment.norm.start == segment.norm.end {
                let byte = segment.norm.start + shifts.before(segment.norm.start, false);
                sorted.push((
                    byte,
                    0,
                    Segment {
                        norm: at(byte),
                        ..segment
                    },
                ));
            } else if matches!(segment.kind, SegmentKind::Verbatim | SegmentKind::Masked) {
                let mut from = segment.norm.start;
                let mut unit = segment.units.start;
                for point in shifts.cuts(segment.norm.start, segment.norm.end) {
                    let units = segment.units_before(&self.text, point)
                        - segment.units_before(&self.text, from);
                    let moved = shifts.before(from, true);
                    sorted.push((
                        from + moved,
                        2,
                        Segment {
                            norm: Range2 {
                                start: from + moved,
                                end: point + moved,
                            },
                            units: Range2 {
                                start: unit,
                                end: unit + units,
                            },
                            kind: segment.kind,
                        },
                    ));
                    unit += units;
                    from = point;
                }
            } else {
                let moved = shifts.before(segment.norm.start, true);
                sorted.push((
                    segment.norm.start + moved,
                    2,
                    Segment {
                        norm: Range2 {
                            start: segment.norm.start + moved,
                            end: segment.norm.end + moved,
                        },
                        ..segment
                    },
                ));
            }
        }
        let mut moved = 0;
        for insertion in shifts.insertions {
            let length = u32::try_from(insertion.text.len()).expect("fits u32");
            let start = insertion.at + moved;
            sorted.push((
                start,
                1,
                Segment {
                    norm: Range2 {
                        start,
                        end: start + length,
                    },
                    units: at(0),
                    kind: SegmentKind::Inserted,
                },
            ));
            moved += length;
        }
        sorted.sort_by_key(|&(start, rank, _)| (start, rank));
        let mut unit = 0;
        sorted
            .into_iter()
            .map(|(_, _, mut segment)| {
                if segment.kind == SegmentKind::Inserted {
                    segment.units = at(unit);
                } else {
                    unit = segment.units.end;
                }
                segment
            })
            .collect()
    }
}

struct Shifts<'a> {
    insertions: &'a [Insertion],
}

impl Shifts<'_> {
    fn before(&self, byte: u32, inclusive: bool) -> u32 {
        self.insertions
            .iter()
            .take_while(|insertion| {
                if inclusive {
                    insertion.at <= byte
                } else {
                    insertion.at < byte
                }
            })
            .map(|insertion| u32::try_from(insertion.text.len()).expect("fits u32"))
            .sum()
    }

    fn before_box(&self, byte: u32, slot: u32) -> u32 {
        self.insertions
            .iter()
            .take_while(|insertion| insertion.at <= byte)
            .filter(|insertion| insertion.at < byte || insertion.atoms_before <= slot)
            .map(|insertion| u32::try_from(insertion.text.len()).expect("fits u32"))
            .sum()
    }

    fn cuts(&self, start: u32, end: u32) -> impl Iterator<Item = u32> + '_ {
        let mut last = start;
        self.insertions
            .iter()
            .map(|insertion| insertion.at)
            .filter(move |&point| start < point && point < end)
            .chain(core::iter::once(end))
            .filter(move |&point| {
                let fresh = last < point;
                last = point;
                fresh
            })
    }
}

pub(in crate::text) fn should_remove_segment_break(previous: Option<char>, next: char) -> bool {
    previous.is_some_and(|character| character == '\u{200B}')
        || next == '\u{200B}'
        || previous.is_some_and(|character| {
            is_east_asian_without_word_separators(character)
                && is_east_asian_without_word_separators(next)
        })
}

pub(in crate::text) const fn is_east_asian_without_word_separators(character: char) -> bool {
    matches!(
        character as u32,
        0x2E80..=0x312F
            | 0x3190..=0xA4CF
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE1F
            | 0xFE30..=0xFE6F
            | 0xFF01..=0xFF9F
            | 0xFFE0..=0xFFE6
            | 0x16FE0..=0x18D8F
            | 0x1AFF0..=0x1B2FF
            | 0x1F200..=0x1F2FF
            | 0x20000..=0x323AF
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::geometry::Size;

    fn normalize(items: &[InlineItem<'_>]) -> NormalizedBlock {
        super::normalize(items, WhiteSpaceProcessing::Css).0
    }

    fn run<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            white_space: WhiteSpaceCollapse::Collapse,
            text_security: TextSecurity::None,
        })
    }

    fn raw<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            white_space: WhiteSpaceCollapse::PreserveBreaks,
            text_security: TextSecurity::None,
        })
    }

    fn pre<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            white_space: WhiteSpaceCollapse::Preserve,
            text_security: TextSecurity::None,
        })
    }

    fn masked<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            white_space: WhiteSpaceCollapse::Collapse,
            text_security: TextSecurity::Disc,
        })
    }

    fn atom(id: u64) -> InlineItem<'static> {
        InlineItem::Box(InlineBoxSpec {
            id,
            size: Size::new(10.0, 10.0),
            baseline: None,
            vertical_align: VerticalAlign::Baseline,
        })
    }

    #[test]
    fn collapsing_matches_the_measurement_path_and_counts_source_units() {
        let style = RunStyle::default();
        let items = [run(&style, "a \t\r\n"), run(&style, "  b\u{a0}c")];
        let block = normalize(&items);

        assert_eq!(block.text, "a b\u{a0}c");
        assert_eq!(block.map.source_len(), 10);
        assert_eq!(block.map.byte_to_unit(&block.text, 0), 0);
        assert_eq!(block.map.byte_to_unit(&block.text, 1), 1);
        assert_eq!(block.map.byte_to_unit(&block.text, 2), 7);
        let end = u32::try_from(block.text.len()).expect("fits");
        assert_eq!(block.map.unit_before(&block.text, end), 10);
        for unit in 1..7 {
            assert_eq!(block.map.unit_to_byte(&block.text, unit), Some(1));
        }
        assert_eq!(block.map.unit_to_byte(&block.text, 7), Some(2));
        assert_eq!(block.map.unit_to_byte(&block.text, 10), None);
    }

    #[test]
    fn item_attribution_survives_collapsing_without_merging_across_items() {
        let first = RunStyle::default();
        let second = RunStyle::default();
        let items = [run(&first, "a "), run(&second, " b")];
        let block = normalize(&items);

        assert_eq!(block.text, "a b");
        assert_eq!(
            block.ranges,
            [
                StyledRange {
                    bytes: 0..2,
                    item: 0
                },
                StyledRange {
                    bytes: 2..3,
                    item: 1
                },
            ],
        );
    }

    #[test]
    fn preserved_newlines_emit_and_suppress_adjacent_collapsible_whitespace() {
        let style = RunStyle::default();
        let items = [raw(&style, "one \r\n \t two\nthree")];
        let block = normalize(&items);

        assert_eq!(block.text, "one\ntwo\nthree");
        assert_eq!(block.map.source_len(), 18);
        assert_eq!(block.map.byte_to_unit(&block.text, 4), 9);
        assert_eq!(block.map.unit_to_byte(&block.text, 6), Some(4));
    }

    #[test]
    fn segment_break_removal_applies_between_east_asian_characters() {
        let style = RunStyle::default();
        let removed = normalize(&[run(&style, "你\n好")]);
        assert_eq!(removed.text, "你好");
        assert_eq!(removed.map.source_len(), 3);
        assert_eq!(removed.map.unit_to_byte(&removed.text, 1), Some(3));
        assert_eq!(removed.map.byte_to_unit(&removed.text, 3), 2);

        let kept = normalize(&[run(&style, "a\nb")]);
        assert_eq!(kept.text, "a b");
    }

    #[test]
    fn item_units_count_every_source_unit_whatever_collapsing_keeps() {
        let style = RunStyle::default();
        let items = [
            run(&style, "a \n"),
            atom(7),
            run(&style, "  "),
            run(&style, "b\u{1F600}"),
        ];
        let block = normalize(&items);
        assert_eq!(block.text, "a  b\u{1F600}");
        assert_eq!(block.item_units, [0..3, 3..4, 4..6, 6..9]);
        assert_eq!(block.map.source_len(), 9);
        assert_eq!(normalize(&[]).item_units, []);
    }

    #[test]
    fn a_box_is_content_that_flushes_whitespace_and_counts_one_unit() {
        let style = RunStyle::default();
        let items = [run(&style, "a \n"), atom(7), run(&style, " b")];
        let block = normalize(&items);

        assert_eq!(block.text, "a  b");
        assert_eq!(block.boxes.len(), 1);
        let placed = block.boxes[0];
        assert_eq!(placed.byte, 2);
        assert_eq!(placed.item, 1);
        assert_eq!(placed.unit, 3);
        assert_eq!(block.map.source_len(), 6);
        assert_eq!(block.map.unit_to_byte(&block.text, 3), Some(2));
        assert_eq!(block.map.byte_to_unit(&block.text, 2), 4);
        assert_eq!(block.map.byte_to_unit(&block.text, 3), 5);
    }

    #[test]
    fn boxes_before_counts_the_boxes_whose_unit_lies_before() {
        let style = RunStyle::default();
        let items = [
            atom(1),
            run(&style, "ab"),
            atom(2),
            atom(3),
            run(&style, "c"),
        ];
        let block = normalize(&items);
        assert_eq!(block.map.source_len(), 6);
        let counts: Vec<u32> = (0..=6).map(|unit| block.map.boxes_before(unit)).collect();
        assert_eq!(counts, [0, 1, 1, 1, 2, 3, 3]);
        assert_eq!(normalize(&[run(&style, "abc")]).map.boxes_before(3), 0);
    }

    #[test]
    fn leading_boxes_and_box_only_content_normalize_without_text() {
        let style = RunStyle::default();
        let block = normalize(&[atom(1), atom(2)]);
        assert_eq!(block.text, "");
        assert_eq!(block.ranges, []);
        assert_eq!(block.map.source_len(), 2);
        assert_eq!(
            block.boxes,
            [
                BoxAt {
                    byte: 0,
                    unit: 0,
                    item: 0
                },
                BoxAt {
                    byte: 0,
                    unit: 1,
                    item: 1
                },
            ],
        );
        assert_eq!(block.map.unit_to_byte("", 0), Some(0));
        assert_eq!(block.map.unit_to_byte("", 2), None);

        let trailing = normalize(&[atom(1), run(&style, "  ")]);
        assert_eq!(trailing.text, " ");
        assert_eq!(trailing.map.source_len(), 3);
    }

    #[test]
    fn surrogate_pairs_round_down_and_round_trip() {
        let style = RunStyle::default();
        let block = normalize(&[run(&style, "a🙂b")]);
        assert_eq!(block.map.source_len(), 4);
        assert_eq!(block.map.unit_to_byte(&block.text, 1), Some(1));
        assert_eq!(block.map.unit_to_byte(&block.text, 2), Some(1));
        assert_eq!(block.map.unit_to_byte(&block.text, 3), Some(5));
        assert_eq!(block.map.byte_to_unit(&block.text, 1), 1);
        assert_eq!(block.map.byte_to_unit(&block.text, 5), 3);
    }

    #[test]
    fn fully_collapsible_and_empty_inputs_produce_nothing() {
        let style = RunStyle::default();
        let whitespace = normalize(&[run(&style, " \t\r\n")]);
        assert_eq!(whitespace.text, "");
        assert_eq!(whitespace.ranges, []);
        assert_eq!(whitespace.map.source_len(), 4);
        assert_eq!(whitespace.map.unit_to_byte("", 2), Some(0));

        let nothing = normalize(&[]);
        assert_eq!(nothing.text, "");
        assert_eq!(nothing.map.source_len(), 0);
        assert_eq!(nothing.map.unit_to_byte("", 0), None);
    }

    #[test]
    fn trailing_collapsible_whitespace_keeps_one_space_covering_its_units() {
        let style = RunStyle::default();
        let block = normalize(&[run(&style, "ab \t ")]);
        assert_eq!(block.text, "ab ");
        assert_eq!(block.map.source_len(), 5);
        assert_eq!(block.map.byte_to_unit(&block.text, 2), 2);
        assert_eq!(block.map.unit_before(&block.text, 3), 5);
        for unit in 2..5 {
            assert_eq!(block.map.unit_to_byte(&block.text, unit), Some(2));
        }
    }

    #[test]
    fn a_space_before_a_preserved_break_is_dropped_unless_a_box_intervenes() {
        let style = RunStyle::default();
        let items = [run(&style, "a "), atom(1), raw(&style, "\nb")];
        let block = normalize(&items);
        assert_eq!(block.text, "a \nb");
        assert_eq!(block.boxes[0].byte, 2);

        let plain = normalize(&[run(&style, "a "), raw(&style, "\nb")]);
        assert_eq!(plain.text, "a\nb");
    }

    #[test]
    fn an_end_offset_covers_collapsed_whitespace_but_stops_at_a_box() {
        let style = RunStyle::default();
        let trailing_box = normalize(&[run(&style, "aaa"), atom(1)]);
        assert_eq!(trailing_box.map.source_len(), 4);
        assert_eq!(trailing_box.map.unit_before(&trailing_box.text, 3), 3);

        let box_then_space = normalize(&[run(&style, "a"), atom(1), run(&style, "  ")]);
        assert_eq!(box_then_space.text, "a ");
        assert_eq!(box_then_space.map.unit_before(&box_then_space.text, 1), 1);
        assert_eq!(box_then_space.map.unit_before(&box_then_space.text, 2), 4);

        let mid = normalize(&[run(&style, "a"), atom(1), run(&style, "b")]);
        assert_eq!(mid.text, "ab");
        assert_eq!(mid.map.unit_before(&mid.text, 1), 1);
        assert_eq!(mid.map.byte_to_unit(&mid.text, 1), 2);

        let east_asian = normalize(&[run(&style, "\u{4f60}\n\u{597d}")]);
        assert_eq!(east_asian.map.unit_before(&east_asian.text, 3), 2);
    }

    #[test]
    fn preserve_keeps_every_space_tab_and_break_as_its_own_unit() {
        let style = RunStyle::default();
        let block = normalize(&[pre(&style, "  a\t b\r\n  c ")]);

        assert_eq!(block.text, "  a  b \n  c ");
        assert_eq!(block.tabs, [3]);
        assert_eq!(block.map.source_len(), 12);
        assert_eq!(block.item_units.first(), Some(&(0..12)));
        for unit in 0..12 {
            assert_eq!(block.map.unit_to_byte(&block.text, unit), Some(unit));
            assert_eq!(block.map.byte_to_unit(&block.text, unit), unit);
        }
        assert_eq!(block.map.unit_before(&block.text, 12), 12);
        assert_eq!(block.map.visible_end(&block.text, 0, 8), 7);
        assert_eq!(block.map.visible_end(&block.text, 8, 12), 12);
    }

    #[test]
    fn preserving_changes_what_survives_never_how_units_are_counted() {
        let style = RunStyle::default();
        let source = ["a \t\n", "  b\r\n\n c"];
        let collapsed = normalize(&[run(&style, source[0]), run(&style, source[1])]);
        let breaks = normalize(&[raw(&style, source[0]), raw(&style, source[1])]);
        let preserved = normalize(&[pre(&style, source[0]), pre(&style, source[1])]);

        assert_eq!(collapsed.text, "a b c");
        assert_eq!(breaks.text, "a\nb\n\nc");
        assert_eq!(preserved.text, "a  \n  b \n\n c");
        assert!(collapsed.tabs.is_empty() && breaks.tabs.is_empty());
        assert_eq!(preserved.tabs, [2]);
        for block in [&collapsed, &breaks, &preserved] {
            assert_eq!(block.item_units, [0..4, 4..12]);
            assert_eq!(block.map.source_len(), 12);
            let byte = block.map.unit_to_byte(&block.text, 11).expect("in range");
            assert_eq!(&block.text[byte as usize..], "c");
        }
    }

    #[test]
    fn preserved_and_collapsible_whitespace_meet_at_run_boundaries() {
        let style = RunStyle::default();
        let before = normalize(&[run(&style, "a  "), pre(&style, "  b")]);
        assert_eq!(before.text, "a   b");
        assert_eq!(before.item_units, [0..3, 3..6]);
        assert_eq!(before.map.byte_to_unit(&before.text, 1), 1);
        assert_eq!(before.map.byte_to_unit(&before.text, 2), 3);

        let after = normalize(&[pre(&style, "a  "), run(&style, "  b")]);
        assert_eq!(after.text, "a   b");

        let trailing = normalize(&[pre(&style, "a "), run(&style, "  ")]);
        assert_eq!(trailing.text, "a  ");
        assert_eq!(trailing.map.visible_end(&trailing.text, 0, 3), 2);
        let collapsed = normalize(&[run(&style, "a  ")]);
        assert_eq!(collapsed.map.visible_end(&collapsed.text, 0, 2), 1);
    }

    #[test]
    fn preserve_breaks_still_drops_the_whitespace_around_its_breaks() {
        let style = RunStyle::default();
        assert_eq!(normalize(&[raw(&style, "a  \n    b")]).text, "a\nb");
        assert_eq!(normalize(&[pre(&style, "a  \n    b")]).text, "a  \n    b");
    }

    #[test]
    fn a_box_stands_in_as_the_content_before_a_segment_break() {
        let style = RunStyle::default();
        let removed = normalize(&[run(&style, "你\n好")]);
        assert_eq!(removed.text, "你好");

        let kept = normalize(&[run(&style, "你"), atom(1), run(&style, "\n好")]);
        assert_eq!(kept.text, "你 好");
        assert_eq!(kept.boxes[0].byte, 3);
    }

    #[test]
    fn css_processing_removes_the_collapsible_whitespace_a_paragraph_starts_with() {
        let style = RunStyle::default();
        let items = [run(&style, " \t"), run(&style, "\n  a b ")];
        let block = normalize(&items);
        assert_eq!(block.text, "a b ");
        assert_eq!(block.map.source_len(), 9);
        assert_eq!(block.map.byte_to_unit(&block.text, 0), 5);
    }

    #[test]
    fn css_processing_keeps_preserved_spaces_at_the_paragraph_start() {
        let style = RunStyle::default();
        assert_eq!(normalize(&[pre(&style, "  a")]).text, "  a");
        assert_eq!(normalize(&[run(&style, " "), pre(&style, " a")]).text, " a");
    }

    mod scan {
        use super::super::{MASK_BYTES, Segment, SegmentKind, SourceMap, utf16_len};

        fn masked_units(segment: &Segment, text: &str, byte: u32) -> u32 {
            let masks = text[segment.norm.start as usize..segment.norm.end as usize]
                .chars()
                .count();
            let per_mask = (segment.units.end - segment.units.start) as usize / masks;
            let before = text[segment.norm.start as usize..byte as usize]
                .chars()
                .count();
            u32::try_from(before * per_mask).unwrap()
        }

        pub(super) fn byte_to_unit(map: &SourceMap, text: &str, byte: u32) -> u32 {
            let segment = map
                .segments
                .iter()
                .find(|segment| segment.norm.start <= byte && byte < segment.norm.end)
                .expect("a start offset lies inside the normalized text");
            match segment.kind {
                SegmentKind::Verbatim => {
                    segment.units.start
                        + utf16_len(&text[segment.norm.start as usize..byte as usize])
                }
                SegmentKind::Masked => segment.units.start + masked_units(segment, text, byte),
                SegmentKind::Collapsed | SegmentKind::Inserted => segment.units.start,
                SegmentKind::Box => unreachable!(),
            }
        }

        pub(super) fn unit_before(map: &SourceMap, text: &str, byte: u32) -> u32 {
            let mut result = 0;
            for segment in &map.segments {
                if segment.norm.end < byte
                    || (segment.norm.end == byte && segment.norm.start < byte)
                {
                    result = segment.units.end;
                } else if segment.norm.start == byte && segment.norm.end == byte {
                    match segment.kind {
                        SegmentKind::Box => break,
                        SegmentKind::Collapsed => result = segment.units.end,
                        SegmentKind::Verbatim | SegmentKind::Masked | SegmentKind::Inserted => {
                            unreachable!()
                        }
                    }
                } else if segment.kind == SegmentKind::Verbatim && segment.norm.start < byte {
                    result = segment.units.start
                        + utf16_len(&text[segment.norm.start as usize..byte as usize]);
                    break;
                } else if segment.kind == SegmentKind::Masked && segment.norm.start < byte {
                    result = segment.units.start + masked_units(segment, text, byte);
                    break;
                } else {
                    break;
                }
            }
            result
        }

        pub(super) fn unit_to_byte(map: &SourceMap, text: &str, unit: u32) -> Option<u32> {
            if unit >= map.shown_len {
                return None;
            }
            let segment = map
                .segments
                .iter()
                .find(|segment| segment.units.start <= unit && unit < segment.units.end)
                .expect("every source unit lies in a segment");
            match segment.kind {
                SegmentKind::Verbatim => {
                    let mut remaining = unit - segment.units.start;
                    let mut byte = segment.norm.start;
                    for character in
                        text[segment.norm.start as usize..segment.norm.end as usize].chars()
                    {
                        let width = u32::try_from(character.len_utf16()).unwrap();
                        if remaining < width {
                            break;
                        }
                        remaining -= width;
                        byte += u32::try_from(character.len_utf8()).unwrap();
                    }
                    Some(byte)
                }
                SegmentKind::Masked => {
                    let mut byte = segment.norm.start;
                    while byte + MASK_BYTES < segment.norm.end
                        && segment.units.start + masked_units(segment, text, byte + MASK_BYTES)
                            <= unit
                    {
                        byte += MASK_BYTES;
                    }
                    Some(byte)
                }
                SegmentKind::Collapsed | SegmentKind::Box => Some(segment.norm.start),
                SegmentKind::Inserted => unreachable!(),
            }
        }
    }

    fn assert_lookups_match_the_scan(block: &NormalizedBlock) {
        let text = block.text.as_str();
        let map = &block.map;
        for byte in (0..text.len()).filter(|&byte| text.is_char_boundary(byte)) {
            let byte = u32::try_from(byte).unwrap();
            assert_eq!(
                map.byte_to_unit(text, byte),
                scan::byte_to_unit(map, text, byte),
                "byte_to_unit({byte}) of {text:?}",
            );
        }
        for byte in (0..=text.len()).filter(|&byte| text.is_char_boundary(byte)) {
            let byte = u32::try_from(byte).unwrap();
            assert_eq!(
                map.unit_before(text, byte),
                scan::unit_before(map, text, byte),
                "unit_before({byte}) of {text:?}",
            );
        }
        for unit in 0..=map.source_len() + 1 {
            assert_eq!(
                map.unit_to_byte(text, unit),
                scan::unit_to_byte(map, text, unit),
                "unit_to_byte({unit}) of {text:?}",
            );
        }
    }

    fn assert_spliced_lookups_match_the_scan(
        items: &[InlineItem<'_>],
        processing: WhiteSpaceProcessing,
    ) {
        for per_point in [1, 2] {
            let (mut block, _) = super::normalize(items, processing);
            assert_lookups_match_the_scan(&block);
            let boxes: Vec<(u32, u32)> = block
                .boxes
                .iter()
                .enumerate()
                .map(|(slot, entry)| (entry.byte, u32::try_from(slot).unwrap()))
                .collect();
            let length = u32::try_from(block.text.len()).unwrap();
            let mut points: Vec<(u32, u32)> = [length / 3, length / 2, length]
                .into_iter()
                .filter(|&at| block.text.is_char_boundary(at as usize))
                .map(|at| {
                    (
                        at,
                        u32::try_from(block.boxes.iter().filter(|entry| entry.byte < at).count())
                            .unwrap(),
                    )
                })
                .chain(boxes.first().copied())
                .collect();
            points.sort_unstable();
            points.dedup();
            block.splice(
                points
                    .into_iter()
                    .flat_map(|point| core::iter::repeat_n(point, per_point))
                    .map(|(at, atoms_before)| Insertion {
                        at,
                        atoms_before,
                        text: "\u{200B}x".to_owned(),
                        style: RunStyle::default(),
                        extra_spacing: 0.0,
                        item: 0,
                    })
                    .collect(),
            );
            assert_lookups_match_the_scan(&block);
        }
    }

    #[test]
    fn lookups_answer_as_a_scan_of_every_segment() {
        let style = RunStyle::default();
        let cjk =
            "活动规则在期间带话题暑假科普手抄报发布优质原创图文作品图片数量张上榜作者能赢取周边"
                .repeat(4);
        let emoji = "😀a😀".repeat(30);
        let latin = "word ".repeat(60);
        let mixed = format!("{latin}{cjk}{latin}é{latin}");
        let cases: Vec<(Vec<InlineItem<'_>>, WhiteSpaceProcessing)> = vec![
            (vec![run(&style, &latin)], WhiteSpaceProcessing::Css),
            (vec![run(&style, &cjk)], WhiteSpaceProcessing::Css),
            (vec![run(&style, &mixed)], WhiteSpaceProcessing::Css),
            (
                vec![pre(&style, &emoji), atom(0), raw(&style, &cjk)],
                WhiteSpaceProcessing::Css,
            ),
            (
                vec![
                    run(&style, "  a  b \n "),
                    atom(0),
                    atom(1),
                    run(&style, " \n c"),
                    pre(&style, "\t x\r  \n"),
                    run(&style, &cjk),
                    atom(2),
                    run(&style, "  "),
                ],
                WhiteSpaceProcessing::Css,
            ),
            (
                vec![run(&style, &mixed), atom(0), run(&style, "\n\t z")],
                WhiteSpaceProcessing::Verbatim {
                    first_line_only: false,
                },
            ),
            (
                vec![
                    run(&style, &cjk),
                    run(&style, "\n"),
                    atom(0),
                    run(&style, &latin),
                ],
                WhiteSpaceProcessing::Verbatim {
                    first_line_only: true,
                },
            ),
        ];
        for (items, processing) in &cases {
            assert_spliced_lookups_match_the_scan(items, *processing);
        }
    }

    #[test]
    fn masked_lookups_answer_as_a_scan_of_every_segment() {
        let style = RunStyle::default();
        let emoji = "😀a😀".repeat(30);
        let mixed = format!("{}活动规则é{}", "word ".repeat(30), "word ".repeat(30));
        assert_spliced_lookups_match_the_scan(
            &[
                run(&style, "a  "),
                masked(&style, &mixed),
                masked(&style, &emoji),
                atom(0),
                masked(&style, " \n x"),
                run(&style, " b"),
            ],
            WhiteSpaceProcessing::Css,
        );
        assert_spliced_lookups_match_the_scan(
            &[
                masked(&style, "ab\ncd"),
                run(&style, "\n"),
                masked(&style, &emoji),
            ],
            WhiteSpaceProcessing::Verbatim {
                first_line_only: true,
            },
        );
    }

    #[test]
    fn verbatim_segments_with_other_than_ascii_stay_short() {
        let style = RunStyle::default();
        let cjk = "活动规则".repeat(100);
        let block = normalize(&[run(
            &style,
            &format!("{}{cjk}{}", "a".repeat(500), "b".repeat(500)),
        )]);
        let lengths: Vec<(bool, u32)> = block
            .map
            .segments
            .iter()
            .map(|segment| (segment.is_ascii(), segment.norm.end - segment.norm.start))
            .collect();
        assert!(
            lengths
                .iter()
                .all(|&(ascii, length)| ascii || length <= VERBATIM_DECODE),
            "{lengths:?}",
        );
        assert_eq!(
            lengths.first(),
            Some(&(true, 500)),
            "ASCII coalesces in full"
        );
        assert_lookups_match_the_scan(&block);
    }

    #[test]
    fn css_processing_removes_whitespace_after_a_preserved_break_as_before() {
        let style = RunStyle::default();
        assert_eq!(normalize(&[raw(&style, "a\n  b")]).text, "a\nb");
        assert_eq!(
            normalize(&[raw(&style, "a\n"), run(&style, "  b")]).text,
            "a\nb"
        );
        assert_eq!(normalize(&[run(&style, " \n\t ")]).text, "");
    }
    #[test]
    fn a_masked_run_shows_one_mask_per_source_character_white_space_included() {
        let style = RunStyle::default();
        let block = normalize(&[masked(&style, "a\u{1F600} \n\tb")]);
        assert_eq!(block.text, "\u{2022}".repeat(6));
        assert_eq!(block.tabs, []);
        assert_eq!(block.map.source_len(), 7);
        assert_eq!(block.item_units.first(), Some(&(0..7)));
        let starts = [0, 1, 3, 4, 5, 6];
        for (mask, &unit) in starts.iter().enumerate() {
            let byte = u32::try_from(mask).unwrap() * MASK_BYTES;
            assert_eq!(block.map.byte_to_unit(&block.text, byte), unit);
            assert_eq!(block.map.unit_before(&block.text, byte), unit);
            assert_eq!(block.map.unit_to_byte(&block.text, unit), Some(byte));
        }
        assert_eq!(block.map.unit_to_byte(&block.text, 2), Some(3));
        assert_eq!(block.map.unit_before(&block.text, 18), 7);
        assert_eq!(block.map.unit_to_byte(&block.text, 7), None);
        assert_eq!(block.map.visible_end(&block.text, 0, 18), 18);
        assert_lookups_match_the_scan(&block);

        for (security, mask) in [
            (TextSecurity::Circle, '\u{25E6}'),
            (TextSecurity::Disc, '\u{2022}'),
            (TextSecurity::Square, '\u{25A0}'),
        ] {
            assert_eq!(security.mask(), Some(mask));
            assert_eq!(mask.len_utf8(), MASK_BYTES as usize);
        }
        assert_eq!(TextSecurity::None.mask(), None);
    }

    #[test]
    fn a_masked_run_resolves_the_white_space_an_unmasked_neighbour_left_open() {
        let style = RunStyle::default();
        let block = normalize(&[run(&style, "a  "), masked(&style, "  "), run(&style, "  b")]);
        assert_eq!(block.text, "a \u{2022}\u{2022} b");
        assert_eq!(block.item_units, [0..3, 3..5, 5..8]);
        assert_eq!(block.map.byte_to_unit(&block.text, 1), 1);
        assert_eq!(block.map.byte_to_unit(&block.text, 2), 3);
        assert_eq!(block.map.byte_to_unit(&block.text, 5), 4);
        assert_eq!(block.map.byte_to_unit(&block.text, 8), 5);
        assert_eq!(block.map.unit_to_byte(&block.text, 4), Some(5));
        assert_lookups_match_the_scan(&block);

        let leading = normalize(&[masked(&style, " "), run(&style, " a")]);
        assert_eq!(leading.text, "\u{2022} a");

        let first_line = super::normalize(
            &[
                masked(&style, "a\nb"),
                run(&style, "\n"),
                masked(&style, "c\u{1F600}"),
            ],
            WhiteSpaceProcessing::Verbatim {
                first_line_only: true,
            },
        )
        .0;
        assert_eq!(first_line.text, format!("{}\n", "\u{2022}".repeat(3)));
        assert_eq!(first_line.map.source_len(), 7);
        assert_eq!(first_line.map.shown_len(), 4);
        assert_lookups_match_the_scan(&first_line);
    }

    #[test]
    fn a_splice_cuts_a_masked_segment_and_keeps_its_units() {
        let style = RunStyle::default();
        let mut block = normalize(&[masked(&style, "ab\u{1F600}\u{1F600}cd")]);
        assert_eq!(block.map.segments.len(), 3);
        block.splice(vec![
            Insertion {
                at: 3,
                atoms_before: 0,
                text: "x".to_owned(),
                style: style.clone(),
                extra_spacing: 0.0,
                item: 0,
            },
            Insertion {
                at: 9,
                atoms_before: 0,
                text: "yz".to_owned(),
                style: style.clone(),
                extra_spacing: 0.0,
                item: 0,
            },
        ]);
        let bullet = "\u{2022}";
        assert_eq!(
            block.text,
            format!("{bullet}x{bullet}{bullet}yz{bullet}{bullet}{bullet}"),
        );
        for (byte, unit) in [(0, 0), (4, 1), (7, 2), (12, 4), (15, 6), (18, 7)] {
            assert_eq!(block.map.byte_to_unit(&block.text, byte), unit, "{byte}");
        }
        assert_eq!(block.map.unit_before(&block.text, 4), 1);
        assert_eq!(block.map.unit_to_byte(&block.text, 3), Some(7));
        assert_eq!(block.map.unit_to_byte(&block.text, 5), Some(12));
        assert_eq!(block.map.unit_before(&block.text, 21), 8);
        assert_lookups_match_the_scan(&block);
    }

    #[test]
    fn only_a_white_space_sequence_a_marker_falls_in_is_reported() {
        let style = RunStyle::default();
        let (_, plain) = super::normalize(&[run(&style, "a  b \n c ")], WhiteSpaceProcessing::Css);
        assert!(plain.markers.is_empty());
        assert_eq!(plain.space_kept, []);

        let items = [
            run(&style, "  "),
            InlineItem::Marker,
            run(&style, "a "),
            InlineItem::Marker,
            run(&style, " b"),
            InlineItem::Marker,
            run(&style, "c"),
        ];
        let (block, places) = super::normalize(&items, WhiteSpaceProcessing::Css);
        assert_eq!(block.text, "a bc");
        let spaces: Vec<Option<u32>> = places.markers.iter().map(|marker| marker.space).collect();
        assert_eq!(spaces, [Some(0), Some(1), None]);
        assert_eq!(places.space_kept, [false, true]);
    }
}
