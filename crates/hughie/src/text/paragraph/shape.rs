//! Parley build and line breaking for every paragraph.
//!
//! One shaping per content revision; breaking re-runs per layout call. The
//! breaker is always dropped before anything reads committed lines — the
//! committed line storage lives inside the breaker until its `Drop` runs.

use core::ops::Range;

use parley::{
    BaseDirection, CHROMIUM_LINE_BREAK_OVERRIDE, InlineBox, Layout, LineHeight as ParleyLineHeight,
    PositionedLayoutItem, TextStyle as ParleyTextStyle,
};

use super::content::SourceMap;
use super::style::{BreakStyle, RunStyle, WordBreak};
use crate::style::TextBrush;
use crate::text::TextContext;

/// One contiguous styled span handed to the shaper.
///
/// `S` is the consumer's paint identity for the span, recorded per parley
/// style index by [`shape`].
pub(in crate::text) struct ShapeSpan<'block, S> {
    pub(in crate::text) bytes: Range<usize>,
    pub(in crate::text) style: &'block RunStyle,
    /// The line-breaking values written into this span's parley style.
    pub(in crate::text) breaking: BreakStyle,
    /// How the span's whitespace hangs at a line end and whether a wrap
    /// opportunity follows each space: the `white_space_collapse` written
    /// into its parley style. The text is already normalized, so parley
    /// reads it for nothing else.
    pub(in crate::text) hanging: parley::WhiteSpaceCollapse,
    pub(in crate::text) source: S,
    /// Added to the run's `letter-spacing` for this span alone. Zero except
    /// on a preserved tab's one-space span, where it widens the space to the
    /// tab stop (`super::tab`), and on the `0` the tab pass measures `ch`
    /// from, where it cancels the run's spacing.
    pub(in crate::text) extra_spacing: f32,
}

/// The root inline box of one shaped paragraph (CSS 2.2 §9.2.1, css-inline-3
/// root inline box): its style, which every line box starts with as its strut
/// (CSS 2.2 §10.8.1), and the paragraph's base direction.
#[derive(Clone, Copy)]
pub(in crate::text) struct LineRoot<'block, S> {
    /// The strut's style and the paint identity its parley style index
    /// records. No text is shaped in it: it is parley's style 0, which parley
    /// 0.12 starts every line box with, so the content's own styles start at
    /// index 1. `None` leaves style 0 to the first span — the shape a
    /// one-run measurement probe wants — and, with no span at all, to a
    /// style with no extent, so that no strut applies.
    pub(in crate::text) strut: Option<(&'block RunStyle, S)>,
    /// The bidi paragraph level (UAX #9 P2–P3): a CSS block container's
    /// `direction`, or first-strong detection for a Lynx `<text>`.
    pub(in crate::text) direction: BaseDirection,
}

impl<S> LineRoot<'_, S> {
    /// No strut, first-strong direction: a measurement probe.
    pub(in crate::text) const PROBE: Self = Self {
        strut: None,
        direction: BaseDirection::Auto,
    };
}

/// Shapes one paragraph. `sources` receives the parley style-index → source
/// identity table: push order is the style index, which is the run-identity
/// channel that leaves `TextBrush = ()` untouched crate-wide. With a strut,
/// index 0 is the strut's and no glyph run carries it.
///
/// Fully empty content short-circuits to an empty layout: parley shapes a
/// substitute space for an empty build, whose glyph run would carry a style
/// index this module never pushed.
///
/// The line-break override is one decision per parley builder, so it cannot
/// follow each span's `word-break`: the paragraph takes it unless a span is
/// `break-all`, whose opportunities between ASCII letters the override's
/// pair table would suppress.
pub(in crate::text) fn shape<S: Copy>(
    context: &mut TextContext,
    text: &str,
    spans: &[ShapeSpan<'_, S>],
    boxes: Vec<InlineBox>,
    sources: &mut Vec<S>,
    root: LineRoot<'_, S>,
) -> Layout<TextBrush> {
    sources.clear();
    if text.is_empty() && boxes.is_empty() {
        return Layout::default();
    }
    #[cfg(test)]
    context.record_shape();
    let (font_context, layout_context) = context.font_and_layout_contexts();
    // `text` is already whitespace-normalized by `super::content::normalize`,
    // and `style_run_builder` takes it verbatim — parley's own
    // `WhiteSpaceCollapse` lives on the `TreeBuilder` path, which this does not
    // use. That division is load-bearing rather than incidental: parley's
    // collapse only recognises `is_ascii_whitespace` and has no notion of a
    // segment break, so it would fold `\n` between two CJK characters into a
    // visible space — the case `should_remove_segment_break` exists to delete.
    // Moving to `TreeBuilder` would collapse this text a second time under
    // those weaker rules, and the symptom would be stray spaces in East Asian
    // text rather than an error. Parley's `WhiteSpaceCollapse::Preserve` is
    // the identity on the text, which is what this builder already is, so
    // `white-space-collapse: preserve` needs nothing from parley beyond it:
    // the normalizer keeps every space, and the tab stops are `super::tab`'s.
    let mut builder = layout_context.style_run_builder(font_context, text, 1.0, false);
    if spans
        .iter()
        .all(|span| span.breaking.word_break != WordBreak::BreakAll)
    {
        // Parley 0.12 takes a mandatory break before it looks for a wrap
        // opportunity, so the override's opportunity after the space of
        // `"a \nb"` no longer swallows the newline's break.
        builder.set_line_break_override(Some(CHROMIUM_LINE_BREAK_OVERRIDE));
    }
    builder.set_base_direction(root.direction);
    builder.reserve(spans.len() + 1, spans.len());
    if let Some((strut, source)) = root.strut {
        let index = builder.push_style(super::style::parley_style(strut, BreakStyle::default()));
        debug_assert_eq!(index, 0, "the strut is parley's style 0");
        sources.push(source);
    } else if spans.is_empty() {
        // Boxes and no run, and no strut asked for: parley would make its own
        // default style the strut. A style with no extent above or below the
        // baseline is no strut at all; no glyph run carries it, so it records
        // no paint identity.
        builder.push_style(ParleyTextStyle {
            font_size: 0.0,
            line_height: ParleyLineHeight::Absolute(0.0),
            ..ParleyTextStyle::default()
        });
    }
    for span in spans {
        let mut style = super::style::parley_style(span.style, span.breaking);
        style.letter_spacing += span.extra_spacing;
        style.white_space_collapse = span.hanging;
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
pub(in crate::text) fn break_clamped(
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
/// produces no line box. A verbatim paragraph keeps the second as its empty
/// last line ([`capture_lines`]).
pub(in crate::text) fn is_blank_line(line: &parley::Line<'_, TextBrush>) -> bool {
    let range = line.text_range();
    range.start >= range.end
        && !line
            .items()
            .any(|item| matches!(item, PositionedLayoutItem::InlineBox(_)))
}

/// One committed line's place in both offset spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text) struct NaturalLine {
    pub(in crate::text) start_unit: u32,
    /// End of the line's visible content — trailing collapsed whitespace
    /// excluded, matching the web reference's rect-derived line ranges, and
    /// trailing *preserved* spaces kept, because they are content, except in
    /// a verbatim paragraph, whose line-end spaces all hang as native Lynx's
    /// do. This is the basis for the reported range, the ellipsis
    /// arithmetic, and the dots back-off.
    pub(in crate::text) end_unit: u32,
    /// Everything the line consumed, trailing whitespace included — the
    /// basis for locating a cut and for whether content remains.
    pub(in crate::text) consumed_end: u32,
    /// The line's text bytes; for a box-only line both bounds sit at the
    /// boxes' shared byte position.
    pub(in crate::text) start_byte: u32,
    pub(in crate::text) end_byte: u32,
}

impl NaturalLine {
    /// Whether the line consumed no source at all: the empty last line a
    /// verbatim paragraph keeps after a newline that ends its text. Every
    /// other line consumes at least a character, a newline or a box.
    pub(in crate::text) const fn is_empty(&self) -> bool {
        self.start_unit == self.consumed_end
    }
}

/// Reads the committed lines back into source space, dropping trailing blank
/// lines — except, where the source map asks for it
/// ([`SourceMap::final_empty_line`]), the one parley commits after a newline
/// that ends the text. That line is kept as an empty line at the end of the
/// source: its start, end and consumed end are all the source unit after
/// the newline, and both its bytes the text's end, so it shows no content,
/// selects nothing in a range query, and reports `start == end` in the line
/// list, as Android's `getLineStart`/`getLineEnd` do for it.
///
/// `Line::text_range()` reports an inverted `usize::MAX..0` range for a line
/// whose items are all inline boxes, so byte bounds fall back to the line's
/// first box.
pub(in crate::text) fn capture_lines(
    layout: &Layout<TextBrush>,
    text: &str,
    map: &SourceMap,
    slot_bytes: &[u32],
    slot_units: &[u32],
) -> Vec<NaturalLine> {
    let mut lines = Vec::with_capacity(layout.len());
    let mut blank_tail = 0;
    // Whether the last line with content ends with the newline that ends
    // the text.
    let mut ends_with_break = false;
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
        ends_with_break = has_text
            && range.end == text.len()
            && text.as_bytes()[..range.end].last() == Some(&b'\n');
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
            // Only collapsible whitespace and the break itself trail outside
            // the line's content; a preserved space is content, except where
            // the map hangs every line-end space (a verbatim paragraph).
            let visible_end = map.visible_end(text, start, end);
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
    let kept = usize::from(blank_tail > 0 && ends_with_break && map.final_empty_line());
    if kept == 1 {
        let end = u32::try_from(text.len()).expect("text fits u32");
        let unit = map.unit_before(text, end);
        let index = lines.len() - blank_tail;
        lines[index] = NaturalLine {
            start_unit: unit,
            end_unit: unit,
            consumed_end: unit,
            start_byte: end,
            end_byte: end,
        };
    }
    lines.truncate(lines.len() - blank_tail + kept);
    lines
}
