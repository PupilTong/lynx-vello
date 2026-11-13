//! Parley build and line breaking for every paragraph.
//!
//! One shaping per content revision; breaking re-runs per layout call. The
//! breaker is always dropped before anything reads committed lines — the
//! committed line storage lives inside the breaker until its `Drop` runs,
//! and so does the alignment: parley 0.13 aligns every break as the breaker
//! drops, with the alignment set on the layout beforehand
//! (`Layout::set_alignment`), which building the layout resets to `Start`.
//!
//! Parley 0.13 represents an atomic inline box — an *inline object* — by a
//! U+FFFC OBJECT REPLACEMENT CHARACTER in the text it shapes ([`OBJECT`]),
//! and indexes every layout it builds by the bytes of that text. The
//! normalized text the paragraph core keeps has no such character: a box is
//! a place between two bytes there (`super::content`). Building a layout
//! inserts one before each box's byte, in a style run of its own
//! ([`build_into_scratch`]); [`Objects`] maps a layout's bytes back.

use core::ops::Range;
use std::borrow::Cow;

use parley::fontique::Attributes;
use parley::{
    BaseDirection, CHROMIUM_LINE_BREAK_OVERRIDE, FontFamily as ParleyFontFamily, FontFamilyName,
    InlineBox, Layout, LineBreakOverrideFn, LineHeight as ParleyLineHeight, PositionedLayoutItem,
    StyleRunBuilder, TextStyle as ParleyTextStyle,
};

use super::content::{BoxAt, SourceMap};
use super::style::{BreakStyle, RunStyle, WithBreaking, WordBreak};
use crate::style::TextBrush;
use crate::text::TextContext;

/// What stands for an inline object in the text parley shapes.
pub(in crate::text) const OBJECT: char = '\u{FFFC}';

/// [`OBJECT`]'s UTF-8 length.
const OBJECT_LEN: u32 = 3;

/// Where one layout's inline objects sit in the text it was built from: the
/// layout byte of each object's [`OBJECT`], ascending, which is box slot
/// order. Read off the layout's own boxes, so it serves any layout this
/// module built — the natural one, a truncated display, a probe.
///
/// Each object is the box's place in the normalized text with three bytes
/// added, so a layout byte less three per object before it is the byte of
/// the text the layout was built over; an object's own byte answers its
/// box's.
#[derive(Debug, Default)]
pub(in crate::text) struct Objects {
    at: Vec<u32>,
}

impl Objects {
    /// The objects of `layout`.
    pub(in crate::text) fn of(layout: &Layout<TextBrush>) -> Self {
        Self {
            at: layout
                .inline_boxes()
                .map(|inline_box| u32::try_from(inline_box.index).expect("text fits u32"))
                .collect(),
        }
    }

    /// The byte of the text the layout was built over that layout byte
    /// `byte` stands at.
    pub(in crate::text) fn text_byte(&self, byte: usize) -> u32 {
        let byte = u32::try_from(byte).expect("text fits u32");
        let before = self.at.partition_point(|&at| at < byte);
        byte - OBJECT_LEN * u32::try_from(before).expect("box count fits u32")
    }

    /// The box slot whose object is at layout byte `byte`, if any.
    pub(in crate::text) fn slot_at(&self, byte: usize) -> Option<usize> {
        let byte = u32::try_from(byte).ok()?;
        self.at.binary_search(&byte).ok()
    }
}

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
    /// Whether content with no text and no box is still shaped, so that it
    /// lays out one empty line of the strut rather than no line: the line
    /// an editing host's caret sits on. Read only with a strut.
    pub(in crate::text) strut_line_when_empty: bool,
}

impl<S> LineRoot<'_, S> {
    /// No strut, first-strong direction: a measurement probe.
    pub(in crate::text) const PROBE: Self = Self {
        strut: None,
        direction: BaseDirection::Auto,
        strut_line_when_empty: false,
    };

    /// Whether fully empty content is shaped into one empty line of the
    /// strut ([`Self::strut_line_when_empty`]).
    pub(in crate::text) const fn shapes_empty_content(&self) -> bool {
        self.strut_line_when_empty && self.strut.is_some()
    }
}

/// A consumer's own line-break override, asked before the paragraph's for
/// every pair: one function for each override a paragraph may be analysed
/// with — Chromium's pair table, or none when a span is `break-all` — since
/// parley takes one static function per builder. Each answers the pairs its
/// consumer settles and defers every other one to the base it is named for.
#[derive(Clone, Copy)]
pub(in crate::text) struct BreakOverride {
    /// Used where the paragraph is analysed with Chromium's override.
    pub(in crate::text) over_chromium: &'static LineBreakOverrideFn,
    /// Used where it is analysed with UAX #14 alone.
    pub(in crate::text) over_icu: &'static LineBreakOverrideFn,
}

impl core::fmt::Debug for BreakOverride {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BreakOverride")
            .finish_non_exhaustive()
    }
}

/// Shapes one paragraph into a layout the caller keeps. `sources` receives
/// the parley style-index → source identity table: push order is the style
/// index, which is the run-identity channel that leaves `TextBrush = ()`
/// untouched crate-wide. With a strut, index 0 is the strut's and no glyph
/// run carries it. With boxes, the last indices are the styles their
/// objects are shaped in ([`object_style`]), each recording style 0's
/// identity, which no glyph run carries either.
///
/// The shaping happens in the context's scratch layout and the caller gets a
/// copy of it. Parley reserves a layout's shaping buffers for the text's
/// UTF-8 byte length (`parley_engine`'s `shape_text`, an upper bound on the
/// characters, about three per CJK character) and never shrinks them; a
/// cloned `Vec` holds exactly its elements, so the retained layout costs what
/// was shaped, and the scratch's buffers serve the next shaping.
///
/// Fully empty content short-circuits to an empty layout. A root that keeps
/// a line for empty content ([`LineRoot::shapes_empty_content`]) is the
/// exception: its strut is style 0, and parley 0.13 breaks empty text into
/// one line with no item whose box is the strut's. It is no content: each
/// consumer reports that line zero wide (`Paragraph::is_empty_strut_line`).
pub(in crate::text) fn shape<S: Copy>(
    context: &mut TextContext,
    text: &str,
    spans: &[ShapeSpan<'_, S>],
    boxes: Vec<InlineBox>,
    sources: &mut Vec<S>,
    root: LineRoot<'_, S>,
) -> Layout<TextBrush> {
    shape_overriding(context, text, spans, boxes, sources, root, None)
}

/// [`shape`], with `overriding`, a consumer's own line-break override, in
/// front of the paragraph's.
pub(in crate::text) fn shape_overriding<S: Copy>(
    context: &mut TextContext,
    text: &str,
    spans: &[ShapeSpan<'_, S>],
    boxes: Vec<InlineBox>,
    sources: &mut Vec<S>,
    root: LineRoot<'_, S>,
    overriding: Option<BreakOverride>,
) -> Layout<TextBrush> {
    if !build_into_scratch(context, text, spans, boxes, sources, root, overriding) {
        return Layout::default();
    }
    let (_, _, scratch) = context.shaping_parts();
    let layout = scratch.clone();
    context.trim_scratch(text.len());
    layout
}

/// Shapes a measurement probe in the context's scratch layout and answers
/// what `read` reads from it there, keeping nothing: no layout is copied,
/// and the scratch is trimmed after, as after [`shape`]. The probe has no
/// strut and first-strong direction ([`LineRoot::PROBE`]); `read` breaks it
/// if it needs lines. `None`, without calling `read`, for fully empty
/// content.
pub(in crate::text) fn with_probe<S: Copy, R>(
    context: &mut TextContext,
    text: &str,
    spans: &[ShapeSpan<'_, S>],
    boxes: Vec<InlineBox>,
    read: impl FnOnce(&mut Layout<TextBrush>) -> R,
) -> Option<R> {
    if !build_into_scratch(
        context,
        text,
        spans,
        boxes,
        &mut Vec::new(),
        LineRoot::PROBE,
        None,
    ) {
        return None;
    }
    let (_, _, scratch) = context.shaping_parts();
    let answer = read(scratch);
    context.trim_scratch(text.len());
    Some(answer)
}

/// Shapes a measurement probe in the context's scratch layout, breaks it as
/// one unbounded line and answers its width, keeping nothing.
pub(in crate::text) fn probe_width<S: Copy>(
    context: &mut TextContext,
    text: &str,
    spans: &[ShapeSpan<'_, S>],
    boxes: Vec<InlineBox>,
) -> f32 {
    with_probe(context, text, spans, boxes, |layout| {
        layout.break_all_lines(None);
        layout.width()
    })
    .unwrap_or(0.0)
}

/// Builds `text` into the context's scratch layout, reusing its buffers.
/// `false`, building nothing, for fully empty content, unless the root keeps
/// a line for it ([`LineRoot::shapes_empty_content`]).
///
/// The line-break override is one decision per parley builder, so it cannot
/// follow each span's `word-break`: the paragraph takes it unless a span is
/// `break-all`, whose opportunities between ASCII letters the override's
/// pair table would suppress. A consumer's own override, `overriding`, goes
/// in front of whichever of the two that leaves.
fn build_into_scratch<S: Copy>(
    context: &mut TextContext,
    text: &str,
    spans: &[ShapeSpan<'_, S>],
    boxes: Vec<InlineBox>,
    sources: &mut Vec<S>,
    root: LineRoot<'_, S>,
    overriding: Option<BreakOverride>,
) -> bool {
    sources.clear();
    if text.is_empty() && boxes.is_empty() && !root.shapes_empty_content() {
        return false;
    }
    #[cfg(test)]
    context.record_shape();
    let locale = context.locale();
    let styles: Vec<_> = spans
        .iter()
        .map(|span| super::style::parley_style(span.style, span.breaking))
        .collect();
    // A span at font size 0 draws nothing, so no fallback font is asked for
    // it: an inline box's edge cluster (`crate::text::inline`), whose marks
    // no font is expected to map.
    context.cover(
        text,
        spans
            .iter()
            .zip(&styles)
            .filter(|(_, style)| style.font_size != 0.0)
            .map(|(span, style)| {
                (
                    span.bytes.clone(),
                    &style.font_family,
                    Attributes {
                        width: style.font_width,
                        weight: style.font_weight,
                        style: style.font_style,
                    },
                )
            }),
    );
    // Boxes with no text and no strut name no family their objects can be
    // shaped in but parley's default.
    let object_family = (root.strut.is_none() && spans.is_empty() && !boxes.is_empty())
        .then(|| context.last_resort_family())
        .flatten();
    // Each box is an inline object, which parley 0.13 shapes as an `OBJECT`
    // in the text, before the character at the box's byte.
    let object_text = ObjectText::new(text, &boxes);
    let (font_context, layout_context, scratch) = context.shaping_parts();
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
    let mut builder =
        layout_context.style_run_builder(font_context, object_text.text(), 1.0, false);
    // Parley 0.12 takes a mandatory break before it looks for a wrap
    // opportunity, so the override's opportunity after the space of `"a \nb"`
    // no longer swallows the newline's break.
    let chromium = spans
        .iter()
        .all(|span| span.breaking.word_break != WordBreak::BreakAll);
    let line_break_override = match (overriding, chromium) {
        (None, true) => Some(CHROMIUM_LINE_BREAK_OVERRIDE),
        (None, false) => None,
        (Some(overriding), true) => Some(overriding.over_chromium),
        (Some(overriding), false) => Some(overriding.over_icu),
    };
    if let Some(overrides) = line_break_override {
        builder.set_line_break_override(Some(overrides));
    }
    builder.set_base_direction(root.direction);
    builder.reserve(spans.len() + 3, spans.len() + 2 * boxes.len());
    if let Some((strut, source)) = root.strut {
        let mut style = super::style::parley_style(strut, BreakStyle::default());
        style.locale = locale;
        let index = builder.push_style(style);
        debug_assert_eq!(index, 0, "the strut is parley's style 0");
        sources.push(source);
    } else if spans.is_empty() {
        // Boxes and no run, and no strut asked for: parley would make its own
        // default style the strut. A style with no extent above or below the
        // baseline is no strut at all; no glyph run carries it, so it records
        // no paint identity.
        builder.push_style(empty_root_style());
    }
    let mut runs = Vec::with_capacity(if boxes.is_empty() { 0 } else { spans.len() });
    for (span, mut style) in spans.iter().zip(styles) {
        style.locale = locale;
        style.letter_spacing += span.extra_spacing;
        style.white_space_collapse = span.hanging;
        let index = builder.push_style(style);
        debug_assert_eq!(usize::from(index), sources.len());
        sources.push(span.source);
        if boxes.is_empty() {
            builder.push_style_run(index, span.bytes.clone());
        } else {
            object_text.span_runs(span.bytes.clone(), index, span.breaking, &mut runs);
        }
    }
    if !boxes.is_empty() {
        let object_root = ObjectRoot {
            strut: root.strut,
            first: spans
                .first()
                .map(|span| (span.style, span.breaking, span.source)),
            locale,
            family: object_family,
        };
        push_object_runs(&mut builder, &object_text, runs, &object_root, sources);
    }
    for (slot, mut inline_box) in boxes.into_iter().enumerate() {
        debug_assert!(text.is_char_boundary(inline_box.index));
        inline_box.index = object_text.object_byte(slot);
        builder.push_inline_box(inline_box);
    }
    builder.build_into(scratch, object_text.text());
    true
}

/// What the styles a layout's objects are shaped in are copies of: parley's
/// style 0 — the strut, the empty style of boxes with neither text nor a
/// strut, or else the first span's — and the paint identity it records.
struct ObjectRoot<'a, S> {
    strut: Option<(&'a RunStyle, S)>,
    /// The first span's style, line-breaking values and paint identity.
    first: Option<(&'a RunStyle, BreakStyle, S)>,
    locale: Option<parley::Language>,
    /// The family of the empty style ([`TextContext::last_resort_family`]).
    family: Option<String>,
}

impl<S: Copy> ObjectRoot<'_, S> {
    /// Style 0's paint identity; none for the empty style, which records
    /// none.
    fn source(&self) -> Option<S> {
        self.strut
            .map(|(_, source)| source)
            .or_else(|| self.first.map(|(_, _, source)| source))
    }

    /// Pushes the object style ([`object_style`]) for the line-breaking
    /// values `breaking`, answering its index.
    fn push(&self, builder: &mut StyleRunBuilder<'_, TextBrush>, breaking: BreakStyle) -> u16 {
        match (self.strut, self.first) {
            (Some((strut, _)), _) => builder.push_style(object_style(
                super::style::parley_style(strut, BreakStyle::default()),
                self.locale,
                breaking,
            )),
            (None, Some((style, span_breaking, _))) => builder.push_style(object_style(
                super::style::parley_style(style, span_breaking),
                self.locale,
                breaking,
            )),
            (None, None) => {
                let mut style = empty_root_style();
                if let Some(family) = &self.family {
                    style.font_family =
                        ParleyFontFamily::Single(FontFamilyName::Named(Cow::Owned(family.clone())));
                }
                builder.push_style(object_style(style, self.locale, breaking))
            }
        }
    }
}

/// Pushes the style runs of a layout with boxes: `runs`, the spans', and
/// one run per box for its object, in text order, which together cover the
/// text. Each object takes the object style of the line-breaking values of
/// the text before it, pushed after every span's style the first time those
/// values are met; each records [`ObjectRoot::source`] in `sources`.
fn push_object_runs<S: Copy>(
    builder: &mut StyleRunBuilder<'_, TextBrush>,
    object_text: &ObjectText<'_>,
    runs: Vec<StyleRun>,
    root: &ObjectRoot<'_, S>,
    sources: &mut Vec<S>,
) {
    let mut object_styles: Vec<(BreakStyle, u16)> = Vec::new();
    let mut objects = Vec::with_capacity(object_text.boxes.len());
    for slot in 0..object_text.boxes.len() {
        let at = object_text.object_byte(slot);
        // The values of the text before the object, or parley's initial
        // ones at the paragraph start: see `object_style`.
        let breaking = runs[..runs.partition_point(|run| run.bytes.end <= at)]
            .last()
            .map_or_else(BreakStyle::default, |run| run.breaking);
        let index =
            if let Some(&(_, index)) = object_styles.iter().find(|(seen, _)| *seen == breaking) {
                index
            } else {
                let index = root.push(builder, breaking);
                if let Some(source) = root.source() {
                    debug_assert_eq!(usize::from(index), sources.len());
                    sources.push(source);
                }
                object_styles.push((breaking, index));
                index
            };
        objects.push(StyleRun {
            bytes: at..at + OBJECT_LEN as usize,
            index,
            breaking,
        });
    }
    let mut spans = runs.into_iter().peekable();
    let mut objects = objects.into_iter().peekable();
    loop {
        let run = match (spans.peek(), objects.peek()) {
            (Some(span), Some(object)) if span.bytes.start < object.bytes.start => spans.next(),
            (_, Some(_)) => objects.next(),
            (Some(_), None) => spans.next(),
            (None, None) => break,
        };
        let run = run.expect("peeked");
        builder.push_style_run(run.index, run.bytes);
    }
}

/// The style an inline object is shaped in: parley's style 0 — the root
/// inline box, whose box parley 0.12 aligned every atomic box against — with
/// nothing of its own that parley 0.12 did not give an atomic box.
///
/// Parley 0.13 takes an object's parent from the style of its `OBJECT` and
/// treats the object as an atom of that style. So the copy keeps style 0's
/// font, size and `vertical-align: baseline` (a child of the root with the
/// root's metrics, which places a box as the root itself does), and drops
/// what would reach the box: letter-spacing, which parley adds after every
/// atom, and a line height, which with `normal` would add the object's run
/// box in the font of the text before it. Its line-breaking values are
/// `breaking`, the text's before the object: parley 0.12 decided the
/// opportunities on both sides of a box from the `text-wrap-mode` of the
/// last text before it, its breaker's lagged value, which is `Wrap` before
/// any text.
fn object_style<'family, 'settings>(
    root: ParleyTextStyle<'family, 'settings, TextBrush>,
    locale: Option<parley::Language>,
    breaking: BreakStyle,
) -> ParleyTextStyle<'family, 'settings, TextBrush> {
    ParleyTextStyle {
        locale,
        letter_spacing: 0.0,
        word_spacing: 0.0,
        line_height: ParleyLineHeight::Absolute(0.0),
        vertical_align: parley::VerticalAlign::default(),
        ..root
    }
    .with_breaking(breaking)
}

/// Parley's style 0 for boxes with no run and no strut: no extent above or
/// below the baseline.
fn empty_root_style() -> ParleyTextStyle<'static, 'static, TextBrush> {
    ParleyTextStyle {
        font_size: 0.0,
        line_height: ParleyLineHeight::Absolute(0.0),
        ..ParleyTextStyle::default()
    }
}

/// One style run of a layout with boxes, in the bytes of its object text.
struct StyleRun {
    bytes: Range<usize>,
    index: u16,
    breaking: BreakStyle,
}

/// The text one layout is built over: the normalized text with an
/// [`OBJECT`] before the byte of each box, in slot order — the normalized
/// text itself when there is no box.
struct ObjectText<'a> {
    text: Cow<'a, str>,
    /// Each box's byte in the normalized text, ascending.
    boxes: Vec<usize>,
}

impl<'a> ObjectText<'a> {
    fn new(text: &'a str, inline_boxes: &[InlineBox]) -> Self {
        if inline_boxes.is_empty() {
            return Self {
                text: Cow::Borrowed(text),
                boxes: Vec::new(),
            };
        }
        let at: Vec<usize> = inline_boxes
            .iter()
            .map(|inline_box| inline_box.index)
            .collect();
        debug_assert!(at.is_sorted(), "boxes come in text order");
        let mut with_objects = String::with_capacity(text.len() + OBJECT_LEN as usize * at.len());
        let mut copied = 0;
        for &byte in &at {
            with_objects.push_str(&text[copied..byte]);
            with_objects.push(OBJECT);
            copied = byte;
        }
        with_objects.push_str(&text[copied..]);
        Self {
            text: Cow::Owned(with_objects),
            boxes: at,
        }
    }

    fn text(&self) -> &str {
        &self.text
    }

    /// The byte of box slot `slot`'s object.
    fn object_byte(&self, slot: usize) -> usize {
        self.boxes[slot] + OBJECT_LEN as usize * slot
    }

    /// The runs of the normalized bytes `bytes` in style `index`, appended to
    /// `runs`: one, unless a box's object falls inside them. A box shares
    /// its byte with the character after it, so its object precedes that
    /// character and follows a run ending there.
    fn span_runs(
        &self,
        bytes: Range<usize>,
        index: u16,
        breaking: BreakStyle,
        runs: &mut Vec<StyleRun>,
    ) {
        let first = self.boxes.partition_point(|&at| at <= bytes.start);
        let last = self.boxes.partition_point(|&at| at < bytes.end);
        let mut start = bytes.start + OBJECT_LEN as usize * first;
        for slot in first..last {
            let at = self.object_byte(slot);
            if start < at {
                runs.push(StyleRun {
                    bytes: start..at,
                    index,
                    breaking,
                });
            }
            start = at + OBJECT_LEN as usize;
        }
        let end = bytes.end + OBJECT_LEN as usize * last;
        if start < end {
            runs.push(StyleRun {
                bytes: start..end,
                index,
                breaking,
            });
        }
    }
}

/// Breaks lines at `width`, committing at most `max_lines` of them.
///
/// Whether content remains past the committed lines is decided by the caller
/// from the captured lines' consumed units — never from the breaker: parley
/// leaves `is_done` false while only an empty final line remains pending
/// (after a trailing explicit break), which would misreport fully consumed
/// content as overflowing.
///
/// The unlimited path routes through `break_all_lines`. The clamp path must
/// initialize both breaker advances — `BreakerState` defaults them to zero,
/// under which every cluster overflows. Either way the lines are aligned as
/// the breaker drops, by the alignment set on the layout.
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

/// Whether a committed line renders nothing: an empty text range, which
/// holds no box either, a box being an [`OBJECT`] of the text. Parley
/// commits one after a trailing preserved newline — where CSS produces no
/// line box. A verbatim paragraph keeps it as its empty last line
/// ([`capture_lines`]).
pub(in crate::text) fn is_blank_line(line: &parley::Line<'_, TextBrush>) -> bool {
    line.text_range().is_empty()
}

/// The inline boxes of a layout's committed lines, line by line, in logical
/// order, without positioning anything.
///
/// Each box is an [`OBJECT`] of the layout's text, which parley commits to
/// exactly one line, so the boxes of a line are the ones whose object lies
/// in its text range; [`Layout::inline_boxes`] is in text order. Every line
/// is read in order, from the first; a layout without boxes answers each
/// line without reading it.
pub(in crate::text) struct LineBoxes<'a> {
    boxes: Vec<&'a InlineBox>,
    /// The first box not on a line read yet.
    next: usize,
}

/// The inline boxes of `layout`'s committed lines ([`LineBoxes`]).
pub(in crate::text) fn line_boxes(layout: &Layout<TextBrush>) -> LineBoxes<'_> {
    LineBoxes {
        boxes: layout.inline_boxes().collect(),
        next: 0,
    }
}

impl<'a> LineBoxes<'a> {
    /// The boxes on `line`, the line after the one the last call read, in
    /// logical order.
    pub(in crate::text) fn on(
        &mut self,
        line: &parley::Line<'_, TextBrush>,
    ) -> impl ExactSizeIterator<Item = &'a InlineBox> + '_ {
        let mut on = self.next..self.next;
        if self.next < self.boxes.len() {
            let range = line.text_range();
            let rest = &self.boxes[self.next..];
            let start = rest.partition_point(|inline_box| inline_box.index < range.start);
            let end = rest.partition_point(|inline_box| inline_box.index < range.end);
            on = self.next + start..self.next + end;
            self.next += end;
        }
        debug_assert_eq!(
            on.len(),
            line.items()
                .filter(|item| matches!(item, PositionedLayoutItem::InlineBox(_)))
                .count(),
            "a line's boxes are the objects in its text",
        );
        self.boxes[on].iter().copied()
    }
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
/// `Line::text_range()` is in the layout's bytes, which count the boxes'
/// objects ([`Objects`]); a line whose items are all boxes has no byte of
/// the normalized text, so its byte bounds fall back to its first box.
pub(in crate::text) fn capture_lines(
    layout: &Layout<TextBrush>,
    text: &str,
    map: &SourceMap,
    slots: &[BoxAt],
) -> Vec<NaturalLine> {
    let mut lines = Vec::with_capacity(layout.len());
    let mut blank_tail = 0;
    // Whether the last line with content ends with the newline that ends
    // the text.
    let mut ends_with_break = false;
    let objects = Objects::of(layout);
    let mut line_boxes = line_boxes(layout);
    for line in layout.lines() {
        let boxes = line_boxes.on(&line);
        let layout_range = line.text_range();
        // `is_blank_line`.
        if layout_range.is_empty() {
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
        let range = objects.text_byte(layout_range.start) as usize
            ..objects.text_byte(layout_range.end) as usize;
        let has_text = range.start < range.end;
        ends_with_break = has_text
            && range.end == text.len()
            && text.as_bytes()[..range.end].last() == Some(&b'\n');
        let mut start_unit = u32::MAX;
        let mut end_unit = 0;
        let mut consumed_end = 0;
        let mut first_box_byte = None;
        // The boxes come in logical order rather than parley's visual one;
        // only the first one's byte is read, for a line without text, whose
        // boxes all sit at one byte because no text lies between them.
        for inline_box in boxes {
            let slot = &slots[usize::try_from(inline_box.id).expect("slot ids are table indexes")];
            start_unit = start_unit.min(slot.unit);
            end_unit = end_unit.max(slot.unit + 1);
            consumed_end = consumed_end.max(slot.unit + 1);
            if first_box_byte.is_none() {
                first_box_byte = Some(slot.byte);
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
            // end byte belongs to a later line unless the box walk above
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
