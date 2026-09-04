//! Flattened block content, CSS whitespace collapsing, and the source map.
//!
//! Lynx offsets — `text-maxlength`, the layout event's per-line ranges — are
//! UTF-16 code units over the *pre-collapse* flattened content, with each
//! atomic box counting exactly one unit; parley indexes are UTF-8 bytes over
//! the *post-collapse* string. This module owns both spaces and the mapping
//! between them. The map is a segment list proportional to structure (runs,
//! boxes, collapse events), never a second copy of the text: intra-segment
//! queries re-decode the one normalized `String` the paragraph needs anyway.
//!
//! The collapse rules are the ones `crate::text::content` implements for the
//! measurement path, restated here with two additions that path has no concept
//! of: source-unit accounting, and boxes as content (an atomic box flushes
//! pending whitespace and resets the after-break state, the way parley's tree
//! builder treats an inline box). The nontrivial segment-break predicate is
//! shared, not copied.

use core::ops::Range;

use super::style::{RunStyle, VerticalAlign};
use crate::geometry::Size;

/// One flattened paragraph item, in source order.
#[derive(Clone, Copy, Debug)]
pub enum InlineItem<'src> {
    Run(TextRunItem<'src>),
    Box(InlineBoxSpec),
}

/// One text run carrying the fully resolved style of its innermost enclosing
/// text element.
#[derive(Clone, Copy, Debug)]
pub struct TextRunItem<'src> {
    pub text: &'src str,
    pub style: &'src RunStyle,
    /// Lynx raw-text keeps literal newlines (`white-space-collapse:
    /// preserve-breaks` on the web target); the host sets this for
    /// raw-text-backed runs.
    pub preserve_newlines: bool,
}

/// One atomic inline — a Lynx inline image or inline view.
///
/// It carries no content, no children, and no measure hook: the inner text of
/// an inline view can never join the paragraph because there is nothing to
/// put it in. The host measures the subtree independently and hands in the
/// margin-box numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineBoxSpec {
    /// Host-owned identity, unique within one block.
    pub id: u64,
    /// Margin-box size.
    pub size: Size<f32>,
    /// Distance from the box top to its baseline. `None` applies the Lynx
    /// rule: a box without a baseline sits with its bottom edge on the text
    /// baseline.
    pub baseline: Option<f32>,
    pub vertical_align: VerticalAlign,
}

/// Whitespace-normalized paragraph content with its source map.
#[derive(Debug)]
pub(in crate::text::block) struct NormalizedBlock {
    pub(in crate::text::block) text: String,
    /// Contiguous, non-overlapping, covering `0..text.len()` — the
    /// `style_run_builder` precondition. `item` indexes the input slice.
    pub(in crate::text::block) ranges: Vec<StyledRange>,
    pub(in crate::text::block) boxes: Vec<BoxAt>,
    pub(in crate::text::block) map: SourceMap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::text::block) struct StyledRange {
    pub(in crate::text::block) bytes: Range<u32>,
    pub(in crate::text::block) item: u32,
}

/// One atomic box's place in both offset spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text::block) struct BoxAt {
    pub(in crate::text::block) byte: u32,
    pub(in crate::text::block) unit: u32,
    pub(in crate::text::block) item: u32,
}

/// Piecewise mapping between normalized UTF-8 bytes and source UTF-16 units.
#[derive(Debug, Default)]
pub(in crate::text::block) struct SourceMap {
    segments: Vec<Segment>,
    source_len: u32,
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    norm: Range2,
    units: Range2,
    kind: SegmentKind,
}

/// A `Copy` byte/unit span (`core::ops::Range` is not `Copy`).
#[derive(Debug, Clone, Copy)]
struct Range2 {
    start: u32,
    end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentKind {
    /// Every emitted char equals its source char; intra-segment offsets are
    /// recomputed from the normalized text.
    Verbatim,
    /// A whitespace span: several source units emitting zero or one byte.
    Collapsed,
    /// One source unit, zero bytes; `norm.start` is the box's byte index.
    Box,
}

impl SourceMap {
    pub(in crate::text::block) const fn source_len(&self) -> u32 {
        self.source_len
    }

    /// Source unit of a normalized byte offset used as a *start*: the unit
    /// of the character at that byte. A box or a dropped whitespace span
    /// sharing the byte lies before that character and is not part of what
    /// starts here.
    pub(in crate::text::block) fn byte_to_unit(&self, text: &str, byte: u32) -> u32 {
        let segment = self
            .segments
            .iter()
            .find(|segment| segment.norm.start <= byte && byte < segment.norm.end)
            .expect("a start offset lies inside the normalized text");
        match segment.kind {
            SegmentKind::Verbatim => {
                let prefix = &text[segment.norm.start as usize..byte as usize];
                segment.units.start + utf16_len(prefix)
            }
            SegmentKind::Collapsed => segment.units.start,
            SegmentKind::Box => unreachable!("a box segment spans no bytes"),
        }
    }

    /// Source unit boundary at `byte` used as an *end*: everything strictly
    /// before the byte, plus collapsed whitespace sitting at it — but never a
    /// box at it, whether the byte is mid-text or the text end. A box at the
    /// boundary is content of whatever follows, and a caller whose range
    /// includes the box accounts for its unit separately.
    pub(in crate::text::block) fn unit_before(&self, text: &str, byte: u32) -> u32 {
        let mut result = 0;
        for segment in &self.segments {
            if segment.norm.end < byte || (segment.norm.end == byte && segment.norm.start < byte) {
                result = segment.units.end;
            } else if segment.norm.start == byte && segment.norm.end == byte {
                match segment.kind {
                    SegmentKind::Box => break,
                    SegmentKind::Collapsed => result = segment.units.end,
                    SegmentKind::Verbatim => unreachable!("verbatim segments are never empty"),
                }
            } else if segment.kind == SegmentKind::Verbatim && segment.norm.start < byte {
                let prefix = &text[segment.norm.start as usize..byte as usize];
                result = segment.units.start + utf16_len(prefix);
                break;
            } else {
                break;
            }
        }
        result
    }

    /// Normalized byte of a source cut, or `None` when the cut lies at or
    /// past the end of the source (nothing to cut).
    ///
    /// Snap rules: a unit inside a surrogate pair rounds down to the char
    /// start; a unit inside a collapsed span lands on the span's emitted
    /// byte; a unit on a box lands on the box's byte (cutting there hides
    /// the box).
    pub(in crate::text::block) fn unit_to_byte(&self, text: &str, unit: u32) -> Option<u32> {
        if unit >= self.source_len {
            return None;
        }
        let segment = self
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
                    let width = u32::try_from(character.len_utf16()).expect("1 or 2");
                    if remaining < width {
                        break;
                    }
                    remaining -= width;
                    byte += u32::try_from(character.len_utf8()).expect("1 to 4");
                }
                Some(byte)
            }
            SegmentKind::Collapsed | SegmentKind::Box => Some(segment.norm.start),
        }
    }
}

fn utf16_len(text: &str) -> u32 {
    text.chars()
        .map(|character| u32::try_from(character.len_utf16()).expect("1 or 2"))
        .sum()
}

/// What an open whitespace span will emit when it resolves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    /// Collapses to one space.
    Space,
    /// A collapsible segment break: a space, unless the segment-break
    /// transformation removes it.
    SegmentBreak,
    /// Whitespace after a preserved newline: always removed.
    Suppressed,
}

struct Pending {
    unit_start: u32,
    item: u32,
    kind: PendingKind,
}

struct Normalizer {
    text: String,
    ranges: Vec<StyledRange>,
    boxes: Vec<BoxAt>,
    segments: Vec<Segment>,
    units: u32,
    pending: Option<Pending>,
    after_preserved_break: bool,
}

/// The sentinel handed to the segment-break predicate when the next content
/// is an atomic box rather than a character.
const OBJECT_REPLACEMENT: char = '\u{FFFC}';

/// Collapses whitespace across the flattened items and builds the source map.
pub(in crate::text::block) fn normalize(items: &[InlineItem<'_>]) -> NormalizedBlock {
    let mut state = Normalizer {
        text: String::new(),
        ranges: Vec::new(),
        boxes: Vec::new(),
        segments: Vec::new(),
        units: 0,
        pending: None,
        after_preserved_break: false,
    };

    for (index, item) in items.iter().enumerate() {
        let item_index = u32::try_from(index).expect("item count fits u32");
        match item {
            InlineItem::Run(run) => state.consume_run(run, item_index),
            InlineItem::Box(_) => state.consume_box(item_index),
        }
    }
    state.finish()
}

impl Normalizer {
    fn consume_run(&mut self, run: &TextRunItem<'_>, item: u32) {
        for character in run.text.chars() {
            match character {
                '\n' if run.preserve_newlines => self.preserved_break(character, item),
                '\n' => self.queue_segment_break(item),
                ' ' | '\t' | '\r' => self.queue_space(character, item),
                _ => {
                    self.flush_pending(character);
                    self.emit_verbatim(character, item);
                    self.after_preserved_break = false;
                }
            }
        }
    }

    fn consume_box(&mut self, item: u32) {
        self.flush_pending(OBJECT_REPLACEMENT);
        let byte = self.byte_len();
        self.boxes.push(BoxAt {
            byte,
            unit: self.units,
            item,
        });
        self.push_segment(SegmentKind::Box, byte..byte, 1);
        self.after_preserved_break = false;
    }

    /// A collapsible space, tab, or carriage return joins the open whitespace
    /// span, opening one if none is open.
    fn queue_space(&mut self, character: char, item: u32) {
        debug_assert_eq!(character.len_utf16(), 1);
        if self.pending.is_none() {
            self.pending = Some(Pending {
                unit_start: self.units,
                item,
                kind: if self.after_preserved_break {
                    PendingKind::Suppressed
                } else {
                    PendingKind::Space
                },
            });
        }
        self.units += 1;
    }

    /// A collapsible newline upgrades the span to a segment break unless a
    /// preserved break directly precedes it.
    fn queue_segment_break(&mut self, item: u32) {
        match &mut self.pending {
            Some(pending) => {
                if pending.kind == PendingKind::Space {
                    pending.kind = PendingKind::SegmentBreak;
                    pending.item = item;
                }
            }
            None => {
                self.pending = Some(Pending {
                    unit_start: self.units,
                    item,
                    kind: if self.after_preserved_break {
                        PendingKind::Suppressed
                    } else {
                        PendingKind::SegmentBreak
                    },
                });
            }
        }
        self.units += 1;
    }

    /// A preserved newline drops the open whitespace span and emits itself.
    ///
    /// A collapsible space directly before the break is always still pending
    /// here — an emitted space is only ever produced with its following
    /// content (a character or a box) already appended — so dropping the span
    /// is the whole of the space-before-preserved-break rule.
    fn preserved_break(&mut self, character: char, item: u32) {
        if let Some(pending) = self.pending.take() {
            self.close_collapsed(pending.unit_start, 0);
        }
        self.emit_verbatim(character, item);
        self.after_preserved_break = true;
    }

    /// Resolves the open whitespace span against the next piece of content.
    fn flush_pending(&mut self, next: char) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let emit = match pending.kind {
            PendingKind::Suppressed => false,
            PendingKind::Space => true,
            PendingKind::SegmentBreak => {
                // A box between the last character and the break stands in as
                // the preceding content, exactly as a box after the break
                // does on the other side.
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
        } else {
            self.close_collapsed(pending.unit_start, 0);
        }
    }

    /// Records a collapsed span covering the units from `unit_start` to the
    /// current position. The span's units were already counted as their
    /// characters arrived, so this never advances the unit counter.
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

    /// Appends or extends a segment; adjacent verbatim segments coalesce
    /// because they are contiguous in both offset spaces.
    fn push_segment(&mut self, kind: SegmentKind, norm: Range<u32>, units: u32) {
        let unit_end = self.units + units;
        if kind == SegmentKind::Verbatim
            && let Some(last) = self.segments.last_mut()
            && last.kind == SegmentKind::Verbatim
            && last.norm.end == norm.start
            && last.units.end == self.units
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

    fn finish(mut self) -> NormalizedBlock {
        if let Some(pending) = self.pending.take() {
            let content_seen = !self.text.is_empty() || !self.boxes.is_empty();
            if content_seen && pending.kind != PendingKind::Suppressed {
                let start = self.byte_len();
                self.text.push(' ');
                self.record_style(start, pending.item);
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
        NormalizedBlock {
            text: self.text,
            ranges: self.ranges,
            boxes: self.boxes,
            map: SourceMap {
                segments: self.segments,
                source_len: self.units,
            },
        }
    }
}

/// The CSS segment-break transformation rules.
///
/// Lives here rather than on the measurement path because this is the
/// paragraph implementation that survives; that path borrows it back.
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

    fn run<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            preserve_newlines: false,
        })
    }

    fn raw<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            preserve_newlines: true,
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
        // Source: 'a', ' ', '\t', '\r', '\n', ' ', ' ', 'b', NBSP, 'c' = 10 units.
        assert_eq!(block.map.source_len(), 10);
        assert_eq!(block.map.byte_to_unit(&block.text, 0), 0);
        // The collapsed span covers units 1..7 and answers with its start.
        assert_eq!(block.map.byte_to_unit(&block.text, 1), 1);
        assert_eq!(block.map.byte_to_unit(&block.text, 2), 7);
        // An end offset at the text end covers the trailing units.
        let end = u32::try_from(block.text.len()).expect("fits");
        assert_eq!(block.map.unit_before(&block.text, end), 10);
        // A cut anywhere inside the collapsed span lands on the emitted space.
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
        // The pending space belongs to the item that opened it; identical
        // style values in different items stay distinct ranges.
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

        // ' ' and '\r' collapse away before the preserved '\n'; the
        // whitespace after it is suppressed; the second '\n' is preserved too.
        assert_eq!(block.text, "one\ntwo\nthree");
        assert_eq!(block.map.source_len(), 18);
        // Bytes of "two" start at 4; their units start at 9 (after
        // "one \r\n \t " = 9 units).
        assert_eq!(block.map.byte_to_unit(&block.text, 4), 9);
        assert_eq!(block.map.unit_to_byte(&block.text, 6), Some(4));
    }

    #[test]
    fn segment_break_removal_applies_between_east_asian_characters() {
        let style = RunStyle::default();
        let removed = normalize(&[run(&style, "你\n好")]);
        assert_eq!(removed.text, "你好");
        assert_eq!(removed.map.source_len(), 3);
        // The dropped break still occupies its source unit.
        assert_eq!(removed.map.unit_to_byte(&removed.text, 1), Some(3));
        assert_eq!(removed.map.byte_to_unit(&removed.text, 3), 2);

        let kept = normalize(&[run(&style, "a\nb")]);
        assert_eq!(kept.text, "a b");
    }

    #[test]
    fn a_box_is_content_that_flushes_whitespace_and_counts_one_unit() {
        let style = RunStyle::default();
        let items = [run(&style, "a \n"), atom(7), run(&style, " b")];
        let block = normalize(&items);

        // The segment break before the box resolves to a space (the box is
        // not an East Asian character), and the space after it collapses.
        assert_eq!(block.text, "a  b");
        assert_eq!(block.boxes.len(), 1);
        let placed = block.boxes[0];
        assert_eq!(placed.byte, 2);
        assert_eq!(placed.item, 1);
        // Units: 'a'=0, collapsed " \n"=1..3, box=3, collapsed " "=4, 'b'=5.
        assert_eq!(placed.unit, 3);
        assert_eq!(block.map.source_len(), 6);
        assert_eq!(block.map.unit_to_byte(&block.text, 3), Some(2));
        assert_eq!(block.map.byte_to_unit(&block.text, 2), 4);
        assert_eq!(block.map.byte_to_unit(&block.text, 3), 5);
    }

    #[test]
    fn leading_boxes_and_box_only_content_normalize_without_text() {
        let style = RunStyle::default();
        let block = normalize(&[atom(1), atom(2)]);
        assert!(block.text.is_empty());
        assert!(block.ranges.is_empty());
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

        // Trailing whitespace after a box survives as one hanging space even
        // with no text bytes before it.
        let trailing = normalize(&[atom(1), run(&style, "  ")]);
        assert_eq!(trailing.text, " ");
        assert_eq!(trailing.map.source_len(), 3);
    }

    #[test]
    fn surrogate_pairs_round_down_and_round_trip() {
        let style = RunStyle::default();
        let block = normalize(&[run(&style, "a🙂b")]);
        assert_eq!(block.map.source_len(), 4);
        // Unit 1 is the pair start, unit 2 is inside it: both land on the
        // emoji's first byte.
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
        assert!(whitespace.text.is_empty());
        assert!(whitespace.ranges.is_empty());
        assert_eq!(whitespace.map.source_len(), 4);
        assert_eq!(whitespace.map.unit_to_byte("", 2), Some(0));

        let nothing = normalize(&[]);
        assert!(nothing.text.is_empty());
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
        // The box flushes the pending space into an emitted byte; the break
        // then follows the box, not the space, so the space stays and the
        // box keeps its byte position.
        let items = [run(&style, "a "), atom(1), raw(&style, "\nb")];
        let block = normalize(&items);
        assert_eq!(block.text, "a \nb");
        assert_eq!(block.boxes[0].byte, 2);

        // Adjacent to the break, the pending space is dropped.
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
        // 'a' = 0, box = 1, collapsed spaces = 2..4. An end at the box's own
        // byte stops before it; an end past the emitted space consumes the
        // box and the whitespace both.
        assert_eq!(box_then_space.text, "a ");
        assert_eq!(box_then_space.map.unit_before(&box_then_space.text, 1), 1);
        assert_eq!(box_then_space.map.unit_before(&box_then_space.text, 2), 4);

        // Mid-text: a box sharing its byte with the following character is
        // not consumed by an end at that byte, while a dropped-whitespace
        // span at the boundary is.
        let mid = normalize(&[run(&style, "a"), atom(1), run(&style, "b")]);
        assert_eq!(mid.text, "ab");
        assert_eq!(mid.map.unit_before(&mid.text, 1), 1);
        assert_eq!(mid.map.byte_to_unit(&mid.text, 1), 2);

        let east_asian = normalize(&[run(&style, "\u{4f60}\n\u{597d}")]);
        assert_eq!(east_asian.map.unit_before(&east_asian.text, 3), 2);
    }

    #[test]
    fn a_box_stands_in_as_the_content_before_a_segment_break() {
        let style = RunStyle::default();
        // Without the box the break between East Asian characters is
        // removed; with the box in between, the break is adjacent to the box
        // and collapses to a space instead.
        let removed = normalize(&[run(&style, "你\n好")]);
        assert_eq!(removed.text, "你好");

        let kept = normalize(&[run(&style, "你"), atom(1), run(&style, "\n好")]);
        assert_eq!(kept.text, "你 好");
        assert_eq!(kept.boxes[0].byte, 3);
    }
}
