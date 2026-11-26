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

pub(in crate::text) const OBJECT: char = '\u{FFFC}';

const OBJECT_LEN: u32 = 3;

#[derive(Debug, Default)]
pub(in crate::text) struct Objects {
    at: Vec<u32>,
}

impl Objects {
    pub(in crate::text) fn of(layout: &Layout<TextBrush>) -> Self {
        Self {
            at: layout
                .inline_boxes()
                .map(|inline_box| u32::try_from(inline_box.index).expect("text fits u32"))
                .collect(),
        }
    }

    pub(in crate::text) fn text_byte(&self, byte: usize) -> u32 {
        let byte = u32::try_from(byte).expect("text fits u32");
        let before = self.at.partition_point(|&at| at < byte);
        byte - OBJECT_LEN * u32::try_from(before).expect("box count fits u32")
    }

    pub(in crate::text) fn slot_at(&self, byte: usize) -> Option<usize> {
        let byte = u32::try_from(byte).ok()?;
        self.at.binary_search(&byte).ok()
    }
}

pub(in crate::text) struct ShapeSpan<'block, S> {
    pub(in crate::text) bytes: Range<usize>,
    pub(in crate::text) style: &'block RunStyle,
    pub(in crate::text) breaking: BreakStyle,
    pub(in crate::text) hanging: parley::WhiteSpaceCollapse,
    pub(in crate::text) source: S,
    pub(in crate::text) extra_spacing: f32,
}

#[derive(Clone, Copy)]
pub(in crate::text) struct LineRoot<'block, S> {
    pub(in crate::text) strut: Option<(&'block RunStyle, S)>,
    pub(in crate::text) direction: BaseDirection,
    pub(in crate::text) strut_line_when_empty: bool,
}

impl<S> LineRoot<'_, S> {
    pub(in crate::text) const PROBE: Self = Self {
        strut: None,
        direction: BaseDirection::Auto,
        strut_line_when_empty: false,
    };

    pub(in crate::text) const fn shapes_empty_content(&self) -> bool {
        self.strut_line_when_empty && self.strut.is_some()
    }
}

#[derive(Clone, Copy)]
pub(in crate::text) struct BreakOverride {
    pub(in crate::text) over_chromium: &'static LineBreakOverrideFn,
    pub(in crate::text) over_icu: &'static LineBreakOverrideFn,
}

impl core::fmt::Debug for BreakOverride {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BreakOverride")
            .finish_non_exhaustive()
    }
}

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
    let object_family = (root.strut.is_none() && spans.is_empty() && !boxes.is_empty())
        .then(|| context.last_resort_family())
        .flatten();
    let object_text = ObjectText::new(text, &boxes);
    let (font_context, layout_context, scratch) = context.shaping_parts();
    let mut builder =
        layout_context.style_run_builder(font_context, object_text.text(), 1.0, false);
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

struct ObjectRoot<'a, S> {
    strut: Option<(&'a RunStyle, S)>,
    first: Option<(&'a RunStyle, BreakStyle, S)>,
    locale: Option<parley::Language>,
    family: Option<String>,
}

impl<S: Copy> ObjectRoot<'_, S> {
    fn source(&self) -> Option<S> {
        self.strut
            .map(|(_, source)| source)
            .or_else(|| self.first.map(|(_, _, source)| source))
    }

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

fn empty_root_style() -> ParleyTextStyle<'static, 'static, TextBrush> {
    ParleyTextStyle {
        font_size: 0.0,
        line_height: ParleyLineHeight::Absolute(0.0),
        ..ParleyTextStyle::default()
    }
}

struct StyleRun {
    bytes: Range<usize>,
    index: u16,
    breaking: BreakStyle,
}

struct ObjectText<'a> {
    text: Cow<'a, str>,
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

    fn object_byte(&self, slot: usize) -> usize {
        self.boxes[slot] + OBJECT_LEN as usize * slot
    }

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

pub(in crate::text) fn is_blank_line(line: &parley::Line<'_, TextBrush>) -> bool {
    line.text_range().is_empty()
}

pub(in crate::text) struct LineBoxes<'a> {
    boxes: Vec<&'a InlineBox>,
    next: usize,
}

pub(in crate::text) fn line_boxes(layout: &Layout<TextBrush>) -> LineBoxes<'_> {
    LineBoxes {
        boxes: layout.inline_boxes().collect(),
        next: 0,
    }
}

impl<'a> LineBoxes<'a> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text) struct NaturalLine {
    pub(in crate::text) start_unit: u32,
    pub(in crate::text) end_unit: u32,
    pub(in crate::text) consumed_end: u32,
    pub(in crate::text) start_byte: u32,
    pub(in crate::text) end_byte: u32,
}

impl NaturalLine {
    pub(in crate::text) const fn is_empty(&self) -> bool {
        self.start_unit == self.consumed_end
    }
}

pub(in crate::text) fn capture_lines(
    layout: &Layout<TextBrush>,
    text: &str,
    map: &SourceMap,
    slots: &[BoxAt],
) -> Vec<NaturalLine> {
    let mut lines = Vec::with_capacity(layout.len());
    let mut blank_tail = 0;
    let mut ends_with_break = false;
    let objects = Objects::of(layout);
    let mut line_boxes = line_boxes(layout);
    for line in layout.lines() {
        let boxes = line_boxes.on(&line);
        let layout_range = line.text_range();
        if layout_range.is_empty() {
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
            let visible_end = map.visible_end(text, start, end);
            start_unit = start_unit.min(map.byte_to_unit(text, start));
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
