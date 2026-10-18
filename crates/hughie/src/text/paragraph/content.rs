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
//! [`WhiteSpaceProcessing::Verbatim`] is the native Lynx `<text>`: no run's
//! `white-space-collapse` is read, every run is processed as `preserve`, and
//! with `first_line_only` everything after the first forced break is dropped
//! from the text while its source units stay counted.

use core::ops::Range;

use super::style::{RunStyle, VerticalAlign, WhiteSpaceCollapse};
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
    /// The run's computed `white-space-collapse`. Lynx raw-text keeps
    /// literal newlines (`preserve-breaks` on the web target); a `pre`
    /// block preserves everything.
    pub white_space: WhiteSpaceCollapse,
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

/// How a paragraph's white space is processed before shaping.
///
/// The choice is the paragraph consumer's: [`normalize`] is the one place it
/// is read, and everything after it sees only the normalized block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text) enum WhiteSpaceProcessing {
    /// css-text-3 §4.1 per run, including §4.1.2 rule 1 at the start of the
    /// paragraph: a collapsible whitespace sequence there is removed, as it
    /// is after a preserved break.
    Css,
    /// No white-space processing: the native Lynx `<text>`. Every run is
    /// processed as `preserve` whatever its own `white-space-collapse` —
    /// native reads no such property — so every space, tab and carriage
    /// return is kept and every newline is a forced break; a newline that
    /// ends the text is followed by an empty last line
    /// ([`SourceMap::final_empty_line`]).
    ///
    /// `first_line_only` is native's `white-space: nowrap`: the text ends
    /// after the first newline, which is kept so a paragraph whose first
    /// character is that newline still shows its one empty line. What
    /// follows is dropped — counted in source units, emitted nowhere, and an
    /// atomic box in it listed in [`NormalizedBlock::dropped_boxes`].
    Verbatim { first_line_only: bool },
}

/// Whitespace-normalized paragraph content with its source map.
#[derive(Debug)]
pub(in crate::text) struct NormalizedBlock {
    pub(in crate::text) text: String,
    /// Contiguous, non-overlapping, covering `0..text.len()` — the
    /// `style_run_builder` precondition. `item` indexes the input slice.
    pub(in crate::text) ranges: Vec<StyledRange>,
    pub(in crate::text) boxes: Vec<BoxAt>,
    pub(in crate::text) map: SourceMap,
    /// Each input item's source units, indexed like the input slice.
    ///
    /// Every source character counts on arrival, whatever collapsing later
    /// makes of it, so these are contiguous, cover `0..map.source_len()`, and
    /// a run's span is its whole pre-collapse text even where none of it
    /// survived; a box's is its one unit.
    pub(in crate::text) item_units: Vec<Range<u32>>,
    /// The byte of every preserved tab, ascending. Each is one space in
    /// `text`, a cluster of its own, whose advance the block widens to the
    /// next tab stop at break time.
    pub(in crate::text) tabs: Vec<u32>,
    /// The box items a `first_line_only` processing dropped, in order: they
    /// have a source unit and no place in the text.
    pub(in crate::text) dropped_boxes: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::text) struct StyledRange {
    pub(in crate::text) bytes: Range<u32>,
    pub(in crate::text) item: u32,
}

/// One atomic box's place in both offset spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text) struct BoxAt {
    pub(in crate::text) byte: u32,
    pub(in crate::text) unit: u32,
    pub(in crate::text) item: u32,
}

/// Piecewise mapping between normalized UTF-8 bytes and source UTF-16 units.
#[derive(Debug, Default)]
pub(in crate::text) struct SourceMap {
    segments: Vec<Segment>,
    source_len: u32,
    /// Where the source content the text shows ends: `source_len`, or
    /// earlier where a `first_line_only` processing dropped the rest. The
    /// segments cover `0..shown_len`.
    shown_len: u32,
    /// Whether every space at a line's end hangs outside the line's content,
    /// preserved or not: a verbatim paragraph's, as native Lynx's line extent
    /// excludes trailing whitespace (Android `Layout.getLineMax`).
    hanging_spaces: bool,
    /// Whether a newline that ends the text is followed by an empty last
    /// line: a verbatim paragraph's without the `nowrap` cut, as Android's
    /// `StaticLayout` ends such text with an empty line. Under the cut the
    /// text ends at its first newline by construction, and shows one line.
    final_empty_line: bool,
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
    /// Every emitted char stands for one source char of the same UTF-16
    /// length — itself, or a space for a preserved tab or carriage return —
    /// so intra-segment offsets are recomputed from the normalized text.
    Verbatim,
    /// A whitespace span: several source units emitting zero or one byte.
    Collapsed,
    /// One source unit, zero bytes; `norm.start` is the box's byte index.
    Box,
}

impl SourceMap {
    pub(in crate::text) const fn source_len(&self) -> u32 {
        self.source_len
    }

    /// Where the source content the text shows ends: [`Self::source_len`]
    /// unless a `first_line_only` processing dropped what follows the first
    /// forced break.
    pub(in crate::text) const fn shown_len(&self) -> u32 {
        self.shown_len
    }

    /// Whether a newline that ends the text is followed by an empty last
    /// line rather than ending the paragraph, as css-text-3 has it.
    pub(in crate::text) const fn final_empty_line(&self) -> bool {
        self.final_empty_line
    }

    /// Source unit of a normalized byte offset used as a *start*: the unit
    /// of the character at that byte. A box or a dropped whitespace span
    /// sharing the byte lies before that character and is not part of what
    /// starts here.
    pub(in crate::text) fn byte_to_unit(&self, text: &str, byte: u32) -> u32 {
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
    pub(in crate::text) fn unit_before(&self, text: &str, byte: u32) -> u32 {
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

    /// The end of `text[start..end]` with its trailing newlines and
    /// *collapsible* spaces dropped — the line-end whitespace that belongs to
    /// no line's content. A preserved space is content and stops the walk,
    /// wherever it sits, except in a verbatim paragraph, where every space
    /// at a line's end hangs as native Lynx hangs it.
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

    /// Whether the byte is the one space a collapsed whitespace span emitted.
    fn is_collapsed_byte(&self, byte: u32) -> bool {
        let index = self
            .segments
            .partition_point(|segment| segment.norm.end <= byte);
        self.segments[index..]
            .iter()
            .take_while(|segment| segment.norm.start <= byte)
            .any(|segment| segment.kind == SegmentKind::Collapsed && segment.norm.start == byte)
    }

    /// Normalized byte of a source cut, or `None` when the cut lies at or
    /// past the end of the shown source (nothing to cut): past
    /// [`Self::shown_len`] the text already ends.
    ///
    /// Snap rules: a unit inside a surrogate pair rounds down to the char
    /// start; a unit inside a collapsed span lands on the span's emitted
    /// byte; a unit on a box lands on the box's byte (cutting there hides
    /// the box).
    pub(in crate::text) fn unit_to_byte(&self, text: &str, unit: u32) -> Option<u32> {
        if unit >= self.shown_len {
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
    /// Whitespace at the start of a line — after a preserved newline, and
    /// at the start of the paragraph under [`WhiteSpaceProcessing::Css`]:
    /// always removed (css-text-3 §4.1.2 rule 1).
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
    /// Whether nothing but removed whitespace has been emitted since the
    /// last line start that normalization can see: a preserved break, or the
    /// paragraph start when the processing removes leading whitespace there.
    at_line_start: bool,
    tabs: Vec<u32>,
    processing: WhiteSpaceProcessing,
    /// Where a `first_line_only` processing stopped emitting: the source
    /// unit after the first forced break. Everything from it on is counted
    /// and dropped.
    dropped_from: Option<u32>,
    dropped_boxes: Vec<u32>,
}

/// The sentinel handed to the segment-break predicate when the next content
/// is an atomic box rather than a character.
const OBJECT_REPLACEMENT: char = '\u{FFFC}';

/// Collapses whitespace across the flattened items and builds the source map.
pub(in crate::text) fn normalize(
    items: &[InlineItem<'_>],
    processing: WhiteSpaceProcessing,
) -> NormalizedBlock {
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
    };

    let mut item_units = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_index = u32::try_from(index).expect("item count fits u32");
        let start = state.units;
        match item {
            InlineItem::Run(run) => state.consume_run(run, item_index),
            InlineItem::Box(_) => state.consume_box(item_index),
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
        for character in run.text.chars() {
            if self.dropped_from.is_some() {
                self.units += u32::try_from(character.len_utf16()).expect("1 or 2");
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

    /// A collapsible space, tab, or carriage return joins the open whitespace
    /// span, opening one if none is open.
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
                    kind: if self.at_line_start {
                        PendingKind::Suppressed
                    } else {
                        PendingKind::SegmentBreak
                    },
                });
            }
        }
        self.units += 1;
    }

    /// A preserved space, tab or carriage return is content: it resolves any
    /// collapsible span still open before it (a neighbouring run's), then
    /// emits one space for its one source unit. css-text-3 §4
    /// (`#white-space-processing`) treats a carriage return as a space in all
    /// respects; a tab is recorded for the tab-stop pass. Unlike a collapsible
    /// space it is never suppressed after a preserved break — that is the
    /// indentation a `pre` block keeps.
    fn preserved_space(&mut self, character: char, item: u32) {
        debug_assert_eq!(character.len_utf16(), 1);
        self.flush_pending(' ');
        if character == '\t' {
            self.tabs.push(self.byte_len());
        }
        self.emit_verbatim(' ', item);
        self.at_line_start = false;
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
        self.at_line_start = true;
        if self.processing
            == (WhiteSpaceProcessing::Verbatim {
                first_line_only: true,
            })
        {
            self.dropped_from = Some(self.units);
        }
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

    /// Emits `character` for one source character of the same UTF-16 length.
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

    fn finish(mut self, item_units: Vec<Range<u32>>) -> NormalizedBlock {
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

    /// css-text-3 processing, which every test below that names no processing
    /// was written against. A `<text>` takes [`WhiteSpaceProcessing::Verbatim`].
    fn normalize(items: &[InlineItem<'_>]) -> NormalizedBlock {
        super::normalize(items, WhiteSpaceProcessing::Css)
    }

    fn run<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            white_space: WhiteSpaceCollapse::Collapse,
        })
    }

    fn raw<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            white_space: WhiteSpaceCollapse::PreserveBreaks,
        })
    }

    fn pre<'src>(style: &'src RunStyle, text: &'src str) -> InlineItem<'src> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            white_space: WhiteSpaceCollapse::Preserve,
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
        // 'a' and the collapsed " \n"; the box; two spaces collapsed into
        // nothing; 'b' and a surrogate pair.
        assert_eq!(block.item_units, [0..3, 3..4, 4..6, 6..9]);
        assert_eq!(block.map.source_len(), 9);
        assert!(normalize(&[]).item_units.is_empty());
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
    fn preserve_keeps_every_space_tab_and_break_as_its_own_unit() {
        let style = RunStyle::default();
        let block = normalize(&[pre(&style, "  a\t b\r\n  c ")]);

        // Nothing collapses: the tab and the carriage return each emit one
        // space, the newline is a preserved break, and the indentation after
        // it survives.
        assert_eq!(block.text, "  a  b \n  c ");
        assert_eq!(block.tabs, [3]);
        assert_eq!(block.map.source_len(), 12);
        assert_eq!(block.item_units.first(), Some(&(0..12)));
        // One byte per unit, both ways, across the whole run.
        for unit in 0..12 {
            assert_eq!(block.map.unit_to_byte(&block.text, unit), Some(unit));
            assert_eq!(block.map.byte_to_unit(&block.text, unit), unit);
        }
        assert_eq!(block.map.unit_before(&block.text, 12), 12);
        // A line's visible end drops its newline but keeps a preserved space
        // in front of it, and a trailing preserved space is content too.
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
            // `c` is unit 11 in every mode.
            let byte = block.map.unit_to_byte(&block.text, 11).expect("in range");
            assert_eq!(&block.text[byte as usize..], "c");
        }
    }

    #[test]
    fn preserved_and_collapsible_whitespace_meet_at_run_boundaries() {
        let style = RunStyle::default();
        // A collapsible space before a preserved one is not collapsed by it:
        // only a collapsible space following another collapsible one is.
        let before = normalize(&[run(&style, "a  "), pre(&style, "  b")]);
        assert_eq!(before.text, "a   b");
        assert_eq!(before.item_units, [0..3, 3..6]);
        assert_eq!(before.map.byte_to_unit(&before.text, 1), 1);
        assert_eq!(before.map.byte_to_unit(&before.text, 2), 3);

        // And one after a preserved space opens a span of its own.
        let after = normalize(&[pre(&style, "a  "), run(&style, "  b")]);
        assert_eq!(after.text, "a   b");

        // Only the collapsed trailing space is line-end whitespace.
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
        // Without the box the break between East Asian characters is
        // removed; with the box in between, the break is adjacent to the box
        // and collapses to a space instead.
        let removed = normalize(&[run(&style, "你\n好")]);
        assert_eq!(removed.text, "你好");

        let kept = normalize(&[run(&style, "你"), atom(1), run(&style, "\n好")]);
        assert_eq!(kept.text, "你 好");
        assert_eq!(kept.boxes[0].byte, 3);
    }

    #[test]
    fn css_processing_removes_the_collapsible_whitespace_a_paragraph_starts_with() {
        let style = RunStyle::default();
        // Spaces, a tab and a segment break, split across two runs.
        let items = [run(&style, " \t"), run(&style, "\n  a b ")];
        let block = normalize(&items);
        assert_eq!(block.text, "a b ");
        // Every source unit is still counted, the removed ones included.
        assert_eq!(block.map.source_len(), 9);
        assert_eq!(block.map.byte_to_unit(&block.text, 0), 5);
    }

    #[test]
    fn css_processing_keeps_preserved_spaces_at_the_paragraph_start() {
        let style = RunStyle::default();
        assert_eq!(normalize(&[pre(&style, "  a")]).text, "  a");
        // A collapsible span before a preserved one is still at the line
        // start, and goes.
        assert_eq!(normalize(&[run(&style, " "), pre(&style, " a")]).text, " a");
    }

    #[test]
    fn css_processing_removes_whitespace_after_a_preserved_break_as_before() {
        let style = RunStyle::default();
        assert_eq!(normalize(&[raw(&style, "a\n  b")]).text, "a\nb");
        assert_eq!(
            normalize(&[raw(&style, "a\n"), run(&style, "  b")]).text,
            "a\nb"
        );
        // Only whitespace: nothing at all.
        assert!(normalize(&[run(&style, " \n\t ")]).text.is_empty());
    }
}
