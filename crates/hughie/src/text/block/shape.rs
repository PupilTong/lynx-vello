//! Parley build and line breaking for the block path.
//!
//! One shaping per content revision; breaking re-runs per layout call. The
//! breaker is always dropped before anything reads committed lines — the
//! committed line storage lives inside the breaker until its `Drop` runs.

use core::ops::Range;

use parley::{CHROMIUM_LINE_BREAK_OVERRIDE, InlineBox, Layout, PositionedLayoutItem};

use super::SourceItem;
use super::content::SourceMap;
use super::style::{BlockStyle, RunStyle, WordBreak};
use crate::style::TextBrush;
use crate::text::TextContext;

/// One contiguous styled span handed to the shaper.
pub(in crate::text::block) struct ShapeSpan<'block> {
    pub(in crate::text::block) bytes: Range<usize>,
    pub(in crate::text::block) style: &'block RunStyle,
    pub(in crate::text::block) source: SourceItem,
}

/// Shapes one paragraph. `sources` receives the parley style-index → source
/// identity table: push order is the style index, which is the run-identity
/// channel that leaves `TextBrush = ()` untouched crate-wide.
///
/// Fully empty content short-circuits to an empty layout: parley shapes a
/// substitute space for an empty build, whose glyph run would carry a style
/// index this module never pushed.
pub(in crate::text::block) fn shape(
    context: &mut TextContext,
    block: &BlockStyle,
    text: &str,
    spans: &[ShapeSpan<'_>],
    boxes: Vec<InlineBox>,
    sources: &mut Vec<SourceItem>,
) -> Layout<TextBrush> {
    sources.clear();
    if text.is_empty() && boxes.is_empty() {
        return Layout::default();
    }
    #[cfg(test)]
    context.record_shape();
    let (font_context, layout_context) = context.font_and_layout_contexts();
    let mut builder = layout_context.style_run_builder(font_context, text, 1.0, false);
    if block.word_break != WordBreak::BreakAll {
        builder.set_line_break_override(Some(CHROMIUM_LINE_BREAK_OVERRIDE));
    }
    builder.reserve(spans.len(), spans.len());
    for span in spans {
        let style = super::style::parley_style(span.style, block);
        let index = builder.push_style(style);
        debug_assert_eq!(usize::from(index), sources.len());
        sources.push(span.source);
        builder.push_style_run(index, span.bytes.clone());
    }
    for inline_box in boxes {
        debug_assert!(text.is_char_boundary(inline_box.index));
        builder.push_inline_box(inline_box);
    }
    builder.build(text)
}

/// Breaks lines at `width`, committing at most `max_lines` of them.
///
/// Whether content remains past the committed lines is decided by the caller
/// from the captured lines' consumed units — never from the breaker: parley
/// leaves `is_done` false while only an empty final line remains pending
/// (after a trailing explicit break, or after a box overflowed onto the last
/// line), which would misreport fully consumed content as overflowing.
///
/// The unlimited path routes through `break_all_lines`, which also covers the
/// empty-layout width workaround parley applies inside `break_next`. The
/// clamp path must initialize both breaker advances — `BreakerState` defaults
/// them to zero, under which every cluster overflows.
pub(in crate::text::block) fn break_clamped(
    layout: &mut Layout<TextBrush>,
    width: Option<f32>,
    max_lines: Option<u32>,
) {
    let Some(limit) = max_lines else {
        layout.break_all_lines(width);
        return;
    };
    let advance = width.unwrap_or(f32::MAX);
    let mut breaker = layout.break_lines();
    breaker.state_mut().set_layout_max_advance(advance);
    breaker.state_mut().set_line_max_advance(advance);
    let mut committed = 0;
    while committed < limit && breaker.break_next().is_some() {
        committed += 1;
    }
}

/// Whether a committed line renders nothing: no boxes and an empty text
/// range. Parley commits one after a box overflows onto its own line at the
/// end of content, and after a trailing preserved newline — cases where CSS
/// produces no line box.
pub(in crate::text::block) fn is_blank_line(line: &parley::Line<'_, TextBrush>) -> bool {
    let range = line.text_range();
    range.start >= range.end
        && !line
            .items()
            .any(|item| matches!(item, PositionedLayoutItem::InlineBox(_)))
}

/// One committed line's place in both offset spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text::block) struct NaturalLine {
    pub(in crate::text::block) start_unit: u32,
    /// End of the line's visible content — trailing collapsed whitespace
    /// excluded, matching the web reference's rect-derived line ranges. This
    /// is the basis for the reported range, the ellipsis arithmetic, and the
    /// dots back-off.
    pub(in crate::text::block) end_unit: u32,
    /// Everything the line consumed, trailing whitespace included — the
    /// basis for locating a cut and for whether content remains.
    pub(in crate::text::block) consumed_end: u32,
    /// The line's text bytes; for a box-only line both bounds sit at the
    /// boxes' shared byte position.
    pub(in crate::text::block) start_byte: u32,
    pub(in crate::text::block) end_byte: u32,
}

/// Reads the committed lines back into source space, dropping trailing blank
/// lines.
///
/// `Line::text_range()` reports an inverted `usize::MAX..0` range for a line
/// whose items are all inline boxes, so byte bounds fall back to the line's
/// first box.
pub(in crate::text::block) fn capture_lines(
    layout: &Layout<TextBrush>,
    text: &str,
    map: &SourceMap,
    slot_bytes: &[u32],
    slot_units: &[u32],
) -> Vec<NaturalLine> {
    let mut lines = Vec::with_capacity(layout.len());
    let mut blank_tail = 0;
    for line in layout.lines() {
        if is_blank_line(&line) {
            // Blank lines only ever trail (every interior line carries its
            // break byte or a box) and are about to be popped; a zeroed
            // record keeps the indexes aligned without consulting the map at
            // an offset that may sit past the text.
            blank_tail += 1;
            lines.push(NaturalLine {
                start_unit: 0,
                end_unit: 0,
                consumed_end: 0,
                start_byte: 0,
                end_byte: 0,
            });
            continue;
        }
        blank_tail = 0;
        let range = line.text_range();
        let has_text = range.start <= range.end;
        let mut start_unit = u32::MAX;
        let mut end_unit = 0;
        let mut consumed_end = 0;
        let mut first_box_byte = None;
        for item in line.items() {
            if let PositionedLayoutItem::InlineBox(inline_box) = item {
                let slot = usize::try_from(inline_box.id).expect("slot ids are table indexes");
                let unit = slot_units[slot];
                start_unit = start_unit.min(unit);
                end_unit = end_unit.max(unit + 1);
                consumed_end = consumed_end.max(unit + 1);
                if first_box_byte.is_none() {
                    first_box_byte = Some(slot_bytes[slot]);
                }
            }
        }
        let (start_byte, end_byte) = if has_text {
            let start = u32::try_from(range.start).expect("text fits u32");
            let end = u32::try_from(range.end).expect("text fits u32");
            let visible_end = start
                + u32::try_from(
                    text[range.start..range.end]
                        .trim_end_matches([' ', '\n'])
                        .len(),
                )
                .expect("text fits u32");
            start_unit = start_unit.min(map.byte_to_unit(text, start));
            // End offsets go through the end-sided lookup: a box sharing the
            // end byte belongs to a later line unless the items walk above
            // already counted it as this line's.
            end_unit = end_unit.max(map.unit_before(text, visible_end));
            consumed_end = consumed_end.max(map.unit_before(text, end));
            (start, end)
        } else {
            let byte = first_box_byte.expect("a committed line has items");
            (byte, byte)
        };
        lines.push(NaturalLine {
            start_unit,
            end_unit,
            consumed_end,
            start_byte,
            end_byte,
        });
    }
    lines.truncate(lines.len() - blank_tail);
    lines
}
