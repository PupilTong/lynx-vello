//! Lynx text-block layout: one flattened paragraph with inline atomic boxes.
//!
//! This is the standalone home of Lynx `<text>` layout semantics — the
//! flattened single paragraph, atomic inline views and images,
//! `text-maxline` / `text-maxlength` / `text-overflow` truncation with
//! inline-truncation content, and the per-line data the `layout` event
//! reports — implemented on the shared parley paragraph core
//! ([`crate::text::paragraph`]). It takes its own parameter structs instead of
//! [`LayoutInput`](crate::tree::LayoutInput): the capabilities that define it
//! are element attributes and Lynx-specific grammar with no home in the box
//! wire format. The behavioral ground truth is recorded in
//! `docs/tracking/css-text.md`; the web reference implementation's truncation
//! algorithm is the only inspectable one and this module follows it.
//!
//! Shaping, breaking, whitespace collapsing, the source map, tab stops, atom
//! `vertical-align` and the break memo are the paragraph core's
//! ([`crate::text::paragraph`]); this module adds what only a Lynx `<text>`
//! has: the `text-maxline` / `text-maxlength` limits and the truncation they
//! drive (`truncate`), the per-line reports of the `layout` event, the
//! hidden-box outcome of a cut, and the Lynx answers of the source-range
//! queries (`range`).
//!
//! Mutation tiers, cheapest to dearest:
//!
//! - **Re-position** — reading a finished layout costs nothing; positions are assembled once per
//!   [`TextBlock::probe`] or [`TextBlock::commit`] call.
//! - **Re-break** — a new width, or [`TextBlock::set_box_size`], re-breaks the retained shaped
//!   layout in place through `edit_inline_box`; shaping is not re-run. A repeated `layout` at
//!   unchanged inputs is a no-op. The one exception is a paragraph with preserved tabs
//!   (`white-space-collapse: preserve`): a break that moves a tab to a different place on its line
//!   moves its tab stop, which re-shapes (`tab`).
//! - **Re-shape (bounded)** — a truncating layout builds one extra display layout over the visible
//!   prefix plus the tail; the inline-truncation content's own width is shaped once and cached.
//! - **Rebuild** — changed text or styles mean a new [`TextBlock`]; there is no in-place path,
//!   matching parley's own mutability contract.

mod range;
mod style;
mod truncate;

use core::fmt;
use core::num::NonZeroU32;

use parley::{Alignment, AlignmentOptions, ContentWidths, Layout, PositionedLayoutItem};
pub use style::{BlockStyle, TextOverflow};
use truncate::{CutPlan, OverflowCut, Tail};

use super::paragraph::memo::{BreakMemo, Retained};
use super::paragraph::query::ReportedLine;
use super::paragraph::shape::{self, NaturalLine, ShapeSpan};
use super::paragraph::style::resolve_alignment;
pub use super::paragraph::{
    BlockConstraint, BlockMetrics, Direction, InlineBoxSpec, InlineItem, LineHeight, OffsetMap,
    RunStyle, TextAlign, TextIndent, TextRunItem, TextSecurity, TextWrap, VerticalAlign,
    WhiteSpaceCollapse, WordBreak,
};
use super::paragraph::{
    Flow, LineGeometry, OwnedItem, Paragraph, ParagraphRoot, RunBreaking, parley_boxes, sized_box,
};
use crate::geometry::{Point, Size};
use crate::style::TextBrush;
use crate::text::TextContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceItem {
    Strut,
    Content(u32),
    Truncation(u32),
    Ellipsis { item: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineInfo {
    pub source_start: u32,
    pub source_end: u32,
    pub ellipsis_count: u32,
    pub top: f32,
    pub baseline: f32,
    pub height: f32,
    pub advance: f32,
}

impl ReportedLine for LineInfo {
    fn source_range(&self) -> core::ops::Range<u32> {
        self.source_start..self.source_end
    }

    fn line_box(&self, _layout: &Layout<TextBrush>, _index: usize) -> LineGeometry {
        LineGeometry {
            top: self.top,
            height: self.height,
            baseline: self.baseline,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlacedBox {
    Visible {
        id: u64,
        line: u32,
        origin: Point<f32>,
        size: Size<f32>,
    },
    Hidden {
        id: u64,
    },
}

#[derive(Debug, Clone, Copy)]
enum SlotSource {
    Content(u32),
    Truncation(u32),
}

const DOTS: &str = "...";

struct LayoutResult {
    constraint: BlockConstraint,
    display: Option<Box<DisplayPart>>,
    lines: Vec<LineInfo>,
    boxes: Vec<PlacedBox>,
    size: Size<f32>,
    first_baseline: Option<f32>,
    truncation_visible: bool,
}

struct DisplayPart {
    layout: Layout<TextBrush>,
    sources: Vec<SourceItem>,
    cut_byte: u32,
    content_slots: usize,
}

pub struct TextBlock {
    style: BlockStyle,
    paragraph: Paragraph<SourceItem>,
    truncation: Option<Box<TruncationFlow>>,
    result: Option<LayoutResult>,
    memo: BreakMemo,
    dots_width: Option<(u32, f32)>,
}

#[derive(Debug)]
struct TruncationFlow {
    flow: Flow,
    natural_width: Option<f32>,
}

impl fmt::Debug for TextBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("TextBlock").finish_non_exhaustive()
    }
}

impl TextBlock {
    #[must_use]
    pub fn new(
        context: &mut TextContext,
        block_style: BlockStyle,
        items: &[InlineItem<'_>],
        truncation: Option<&[InlineItem<'_>]>,
    ) -> Self {
        let mut block_style = block_style;
        let strut = block_style
            .strut
            .take()
            .or_else(|| block_style.strut_line_when_empty.then(RunStyle::default));
        let root = ParagraphRoot {
            strut: strut.map(|strut| (strut, SourceItem::Strut)),
            direction: parley::BaseDirection::Auto,
            strut_line_when_empty: block_style.strut_line_when_empty,
        };
        let paragraph = Paragraph::new(
            context,
            RunBreaking::Uniform(block_style.breaking()),
            block_style.processing(true),
            items,
            SourceItem::Content,
            root,
        );
        let flow = &paragraph.content;
        let truncation = truncation.map(|items| {
            Box::new(TruncationFlow {
                flow: Flow::build(items, block_style.processing(false)).0,
                natural_width: None,
            })
        });
        debug_assert!(
            {
                let mut ids: Vec<u64> = flow
                    .block
                    .boxes
                    .iter()
                    .map(|entry| flow.box_spec(entry.item).id)
                    .chain(truncation.iter().flat_map(|part| {
                        part.flow
                            .block
                            .boxes
                            .iter()
                            .map(|entry| part.flow.box_spec(entry.item).id)
                    }))
                    .collect();
                ids.sort_unstable();
                ids.windows(2).all(|pair| pair[0] != pair[1])
            },
            "box ids are unique within one block",
        );
        Self {
            style: block_style,
            paragraph,
            truncation,
            result: None,
            memo: BreakMemo::new(),
            dots_width: None,
        }
    }

    pub fn set_style(&mut self, mut style: BlockStyle) {
        debug_assert!(
            self.shaped_as(&style),
            "a shaping input change needs a new block"
        );
        style.strut = None;
        if self.style == style {
            return;
        }
        self.style = style;
        self.memo.forget();
        self.result = None;
    }

    #[must_use]
    pub fn shaped_with<'a>(
        &self,
        style: &BlockStyle,
        runs: impl IntoIterator<Item = &'a RunStyle>,
    ) -> bool {
        let built = self
            .paragraph
            .content
            .items
            .iter()
            .chain(
                self.truncation
                    .iter()
                    .flat_map(|part| part.flow.items.iter()),
            )
            .filter_map(|item| match item {
                OwnedItem::Run { style, .. } => Some(style),
                OwnedItem::Box(_) | OwnedItem::Marker => None,
            });
        self.shaped_as(style) && built.eq(runs)
    }

    fn shaped_as(&self, style: &BlockStyle) -> bool {
        let strut = self.paragraph.root.strut.as_ref().map(|(strut, _)| strut);
        style.shapes_as(&self.style, strut)
    }

    pub fn probe(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        Retained::probe(self, context, constraint)
    }

    pub fn commit(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        Retained::commit(self, context, constraint)
    }

    #[must_use]
    pub const fn has_committed(&self) -> bool {
        self.memo.committed().is_some()
    }

    #[must_use]
    pub fn is_probe_dirty(&self) -> bool {
        Retained::is_probe_dirty(self)
    }

    pub fn restore_committed(&mut self, context: &mut TextContext) -> bool {
        Retained::restore_committed(self, context)
    }

    pub fn min_content_width(&mut self, context: &mut TextContext, indent: f32) -> f32 {
        if let Some(width) = self.memo.min_content(indent) {
            return width;
        }
        let laid_out_at_indent = self
            .result
            .as_ref()
            .is_some_and(|result| result.constraint.indent.to_bits() == indent.to_bits());
        if !laid_out_at_indent || self.memo.boxes_dirty() {
            let constraint = BlockConstraint::new(None, indent);
            self.layout_at(context, constraint);
            self.remember(constraint);
        }
        let width = self.paragraph.content_widths().min;
        self.memo.set_min_content(indent, width);
        width
    }

    #[must_use]
    pub const fn break_count(&self) -> u32 {
        self.memo.breaks()
    }

    pub fn set_box_size(&mut self, id: u64, size: Size<f32>, baseline: Option<f32>) {
        let spec = self
            .paragraph
            .content
            .items
            .iter_mut()
            .chain(
                self.truncation
                    .iter_mut()
                    .flat_map(|part| part.flow.items.iter_mut()),
            )
            .find_map(|item| match item {
                OwnedItem::Box(spec) if spec.id == id => Some(spec),
                _ => None,
            })
            .expect("set_box_size addresses a box this block was built with");
        if spec.size == size && spec.baseline == baseline {
            return;
        }
        spec.size = size;
        spec.baseline = baseline;
        self.memo.mark_boxes_dirty();
        self.memo.forget();
        if let Some(part) = &mut self.truncation
            && part
                .flow
                .items
                .iter()
                .any(|item| matches!(item, OwnedItem::Box(spec) if spec.id == id))
        {
            part.natural_width = None;
        }
    }

    fn natural_alignment(&self) -> Alignment {
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        if alignment == Alignment::Justify
            && self.truncation.is_some()
            && self.style.max_lines.is_some()
        {
            Alignment::Start
        } else {
            alignment
        }
    }

    fn finish_untruncated(
        &mut self,
        natural_lines: &[NaturalLine],
        constraint: BlockConstraint,
    ) -> LayoutResult {
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        self.paragraph.realign(
            alignment,
            constraint.max_advance(),
            self.style.max_lines.map(NonZeroU32::get),
        );
        let slot_specs: Vec<InlineBoxSpec> = self
            .paragraph
            .content
            .block
            .boxes
            .iter()
            .map(|entry| *self.paragraph.content.box_spec(entry.item))
            .collect();
        let hidden = self
            .dropped_boxes()
            .chain(
                self.truncation
                    .iter()
                    .flat_map(|part| part.flow.block.boxes.iter())
                    .map(|entry| PlacedBox::Hidden {
                        id: self
                            .truncation
                            .as_ref()
                            .expect("iterating this part")
                            .flow
                            .box_spec(entry.item)
                            .id,
                    }),
            )
            .collect();
        let (lines, boxes, size, first_baseline) = assemble(
            &self.paragraph.natural,
            None,
            natural_lines,
            natural_lines.len(),
            None,
            &slot_specs,
            hidden,
        );
        LayoutResult {
            constraint,
            display: None,
            lines,
            boxes,
            size,
            first_baseline,
            truncation_visible: false,
        }
    }

    fn finish_truncated(
        &self,
        context: &mut TextContext,
        plan: &CutPlan,
        natural_lines: &[NaturalLine],
        constraint: BlockConstraint,
        recycled: Option<Box<DisplayPart>>,
    ) -> LayoutResult {
        let width = constraint.max_advance();
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        let visible = plan.cut_line as usize + 1;
        let (display, sources, slots) =
            self.rebuild(context, plan, constraint.indent, width, visible, alignment);
        let slot_specs: Vec<InlineBoxSpec> = slots
            .iter()
            .map(|slot| match *slot {
                SlotSource::Content(item) => *self.paragraph.content.box_spec(item),
                SlotSource::Truncation(item) => *self
                    .truncation
                    .as_ref()
                    .expect("truncation slots come from truncation content")
                    .flow
                    .box_spec(item),
            })
            .collect();
        let hidden = self.hidden_boxes(plan);
        let content_slots = slots
            .iter()
            .filter(|slot| matches!(slot, SlotSource::Content(_)))
            .count();
        let (lines, boxes, size, first_baseline) = assemble(
            &display,
            Some(&self.paragraph.natural),
            natural_lines,
            visible,
            Some(plan),
            &slot_specs,
            hidden,
        );
        let part = DisplayPart {
            layout: display,
            sources,
            cut_byte: plan.cut_byte,
            content_slots,
        };
        let display = match recycled {
            Some(mut held) => {
                *held = part;
                held
            }
            None => Box::new(part),
        };
        LayoutResult {
            constraint,
            display: Some(display),
            lines,
            boxes,
            size,
            first_baseline,
            truncation_visible: plan.truncation_visible,
        }
    }

    fn overflow_cut(
        &mut self,
        context: &mut TextContext,
        natural_lines: &[NaturalLine],
        width: Option<f32>,
    ) -> Option<OverflowCut> {
        if self.style.text_wrap != TextWrap::NoWrap {
            return None;
        }
        let width = width?;
        let [line] = natural_lines else { return None };
        let line = *line;
        let visible_advance = {
            let rendered = self.paragraph.natural.get(0)?;
            let metrics = rendered.metrics();
            metrics.advance - metrics.hanging_advance
        };
        if visible_advance <= width {
            return None;
        }

        let text = &self.paragraph.content.block.text;
        let map = &self.paragraph.content.block.map;
        let fit = truncate::clip_boundary(
            &self.paragraph.natural,
            0,
            line,
            text,
            map,
            &self.paragraph.content.block.boxes,
            width,
        );

        if self.truncation.is_some() {
            let marker = self.measure_truncation(context);
            if marker >= width {
                return Some(OverflowCut {
                    unit: line.start_unit,
                    truncation_visible: false,
                });
            }
            let unit = truncate::retreat_for_width(
                &self.paragraph.natural,
                0,
                NaturalLine {
                    end_unit: fit,
                    ..line
                },
                &self.paragraph.content.block.text,
                &self.paragraph.content.block.map,
                &self.paragraph.content.block.boxes,
                marker,
            );
            return Some(OverflowCut {
                unit,
                truncation_visible: true,
            });
        }

        if self.style.overflow != TextOverflow::Ellipsis {
            return None;
        }
        let cut_byte = map
            .unit_to_byte(text, fit)
            .unwrap_or_else(|| u32::try_from(text.len()).expect("text fits u32"));
        let item = truncate::dots_item(&self.paragraph.content.block.ranges, cut_byte)?;
        let dots = self.measure_dots(context, item);
        let unit = truncate::clip_boundary(
            &self.paragraph.natural,
            0,
            line,
            &self.paragraph.content.block.text,
            &self.paragraph.content.block.map,
            &self.paragraph.content.block.boxes,
            (width - dots).max(0.0),
        );
        Some(OverflowCut {
            unit,
            truncation_visible: false,
        })
    }

    fn measure_dots(&mut self, context: &mut TextContext, item: u32) -> f32 {
        if let Some((cached, width)) = self.dots_width
            && cached == item
        {
            return width;
        }
        let width = {
            let spans = [ShapeSpan {
                bytes: 0..DOTS.len(),
                style: self.paragraph.content.run_style(item),
                breaking: self.style.breaking(),
                hanging: parley::WhiteSpaceCollapse::Collapse,
                source: SourceItem::Ellipsis { item },
                extra_spacing: 0.0,
            }];
            shape::probe_width(context, DOTS, &spans, Vec::new())
        };
        self.dots_width = Some((item, width));
        width
    }

    fn measure_truncation(&mut self, context: &mut TextContext) -> f32 {
        let breaking = self.style.breaking();
        let part = self
            .truncation
            .as_mut()
            .expect("caller checked for truncation content");
        if let Some(width) = part.natural_width {
            return width;
        }
        let spans = part
            .flow
            .spans(0, SourceItem::Truncation, RunBreaking::Uniform(breaking));
        let boxes = parley_boxes(&part.flow, 0, 0, self.paragraph.percent_basis);
        let width = shape::probe_width(context, &part.flow.block.text, &spans, boxes);
        drop(spans);
        part.natural_width = Some(width);
        width
    }

    fn rebuild(
        &self,
        context: &mut TextContext,
        plan: &CutPlan,
        indent: f32,
        width: Option<f32>,
        visible: usize,
        alignment: Alignment,
    ) -> (Layout<TextBrush>, Vec<SourceItem>, Vec<SlotSource>) {
        let cut = plan.cut_byte as usize;
        let mut text = String::with_capacity(cut + 16);
        text.push_str(&self.paragraph.content.block.text[..cut]);

        let breaking = self.style.breaking();
        let mut spans: Vec<ShapeSpan<'_, SourceItem>> =
            self.paragraph
                .content
                .spans(0, SourceItem::Content, RunBreaking::Uniform(breaking));
        spans.retain_mut(|span| {
            span.bytes.end = span.bytes.end.min(cut);
            span.bytes.start < cut
        });
        let mut slots = Vec::new();
        let mut boxes = Vec::new();
        for entry in &self.paragraph.content.block.boxes {
            if entry.unit >= plan.cut_unit {
                break;
            }
            let spec = self.paragraph.content.box_spec(entry.item);
            boxes.push(sized_box(
                spec,
                slots.len(),
                entry.byte as usize,
                self.paragraph.percent_basis,
            ));
            slots.push(SlotSource::Content(entry.item));
        }

        match plan.tail {
            Tail::None => {}
            Tail::Dots { count, item } => {
                let start = text.len();
                text.push_str(&DOTS[..count as usize]);
                spans.push(ShapeSpan {
                    bytes: start..text.len(),
                    style: self.paragraph.content.run_style(item),
                    breaking,
                    hanging: parley::WhiteSpaceCollapse::Collapse,
                    source: SourceItem::Ellipsis { item },
                    extra_spacing: 0.0,
                });
            }
            Tail::Truncation => {
                let part = &self
                    .truncation
                    .as_ref()
                    .expect("a truncation tail needs truncation content")
                    .flow;
                let offset = u32::try_from(text.len()).expect("display text fits u32");
                text.push_str(&part.block.text);
                spans.extend(part.spans(
                    offset,
                    SourceItem::Truncation,
                    RunBreaking::Uniform(breaking),
                ));
                for entry in &part.block.boxes {
                    let spec = part.box_spec(entry.item);
                    boxes.push(sized_box(
                        spec,
                        slots.len(),
                        (entry.byte + offset) as usize,
                        self.paragraph.percent_basis,
                    ));
                    slots.push(SlotSource::Truncation(entry.item));
                }
            }
        }

        let mut sources = Vec::new();
        let mut display = shape::shape(
            context,
            &text,
            &spans,
            boxes,
            &mut sources,
            shape::LineRoot {
                strut_line_when_empty: false,
                ..self.paragraph.root.line_root()
            },
        );
        drop(spans);
        display.set_text_indent(indent, BlockStyle::INDENT_KEYWORDS.options());
        display.set_alignment(alignment, AlignmentOptions::default());
        let limit = u32::try_from(visible).expect("line count fits u32");
        shape::break_clamped(&mut display, width, Some(limit));
        (display, sources, slots)
    }

    fn dropped_boxes(&self) -> impl Iterator<Item = PlacedBox> + '_ {
        let content = &self.paragraph.content;
        content
            .block
            .dropped_boxes
            .iter()
            .map(|&item| PlacedBox::Hidden {
                id: content.box_spec(item).id,
            })
    }

    fn hidden_boxes(&self, plan: &CutPlan) -> Vec<PlacedBox> {
        let mut hidden = Vec::new();
        for entry in &self.paragraph.content.block.boxes {
            if entry.unit >= plan.cut_unit {
                hidden.push(PlacedBox::Hidden {
                    id: self.paragraph.content.box_spec(entry.item).id,
                });
            }
        }
        hidden.extend(self.dropped_boxes());
        if !plan.truncation_visible
            && let Some(part) = &self.truncation
        {
            for entry in &part.flow.block.boxes {
                hidden.push(PlacedBox::Hidden {
                    id: part.flow.box_spec(entry.item).id,
                });
            }
        }
        hidden
    }

    #[must_use]
    pub fn display(&self) -> &Layout<TextBrush> {
        let result = self.expect_result();
        result
            .display
            .as_ref()
            .map_or(&self.paragraph.natural, |part| &part.layout)
    }

    #[must_use]
    pub fn offset_map(&self) -> OffsetMap<'_> {
        let block = &self.paragraph.content.block;
        let (cut_byte, shown_boxes) = self
            .expect_result()
            .display
            .as_ref()
            .map_or((block.text.len(), block.boxes.len()), |part| {
                (part.cut_byte as usize, part.content_slots)
            });
        OffsetMap::new(&block.text, &block.map, cut_byte, &block.boxes, shown_boxes)
    }

    #[must_use]
    pub fn style_count(&self) -> usize {
        let result = self.expect_result();
        result
            .display
            .as_ref()
            .map_or(&self.paragraph.natural_sources, |part| &part.sources)
            .len()
    }

    #[must_use]
    pub fn source_of(&self, style_index: u16) -> SourceItem {
        let result = self.expect_result();
        let sources = result
            .display
            .as_ref()
            .map_or(&self.paragraph.natural_sources, |part| &part.sources);
        sources[usize::from(style_index)]
    }

    #[must_use]
    pub fn lines(&self) -> &[LineInfo] {
        &self.expect_result().lines
    }

    #[must_use]
    pub fn boxes(&self) -> &[PlacedBox] {
        &self.expect_result().boxes
    }

    #[must_use]
    pub fn size(&self) -> Size<f32> {
        self.expect_result().size
    }

    #[must_use]
    pub fn first_baseline(&self) -> Option<f32> {
        self.expect_result().first_baseline
    }

    #[must_use]
    pub fn last_baseline(&self) -> Option<f32> {
        self.expect_result().lines.last().map(|line| line.baseline)
    }

    #[must_use]
    pub fn truncated(&self) -> bool {
        self.expect_result().display.is_some()
    }

    #[must_use]
    pub fn truncation_visible(&self) -> bool {
        self.expect_result().truncation_visible
    }

    #[must_use]
    pub fn content_widths(&self) -> ContentWidths {
        self.expect_result();
        self.paragraph.content_widths()
    }

    fn expect_result(&self) -> &LayoutResult {
        self.result
            .as_ref()
            .expect("a TextBlock is read after layout()")
    }
}

impl Retained for TextBlock {
    fn memo(&self) -> &BreakMemo {
        &self.memo
    }

    fn memo_mut(&mut self) -> &mut BreakMemo {
        &mut self.memo
    }

    fn retained(&self) -> Option<BlockConstraint> {
        self.result.as_ref().map(|result| result.constraint)
    }

    fn layout_at(&mut self, context: &mut TextContext, constraint: BlockConstraint) {
        let left =
            resolve_alignment(self.style.text_align, self.style.direction) == Alignment::Left;
        if self.memo.holds(
            self.retained(),
            self.fits_from().filter(|_| left),
            constraint,
        ) {
            if let Some(result) = &mut self.result {
                result.constraint = constraint;
            }
            return;
        }
        self.memo.begin_break();
        let recycled = self
            .result
            .take()
            .and_then(|result| result.display)
            .map(|mut part| {
                part.layout = Layout::default();
                part.sources = Vec::new();
                part
            });
        let width = constraint.max_advance();

        let natural_lines = self.paragraph.break_lines(
            context,
            RunBreaking::Uniform(self.style.breaking()),
            constraint,
            self.style.max_lines.map(NonZeroU32::get),
            BlockStyle::INDENT_KEYWORDS,
            self.natural_alignment(),
        );

        if self.paragraph.is_empty_strut_line() {
            let mut result = self.finish_untruncated(&natural_lines, constraint);
            result.size.width = 0.0;
            for line in &mut result.lines {
                line.advance = 0.0;
            }
            self.result = Some(result);
            return;
        }

        let more_content = natural_lines
            .last()
            .is_some_and(|line| line.consumed_end < self.paragraph.content.block.map.shown_len());

        let truncation_width = if more_content && self.truncation.is_some() {
            Some(self.measure_truncation(context))
        } else {
            None
        };

        let overflow_cut = self.overflow_cut(context, &natural_lines, width);

        let plan = truncate::plan(
            &self.paragraph.natural,
            &natural_lines,
            &self.paragraph.content.block.text,
            &self.paragraph.content.block.map,
            &self.paragraph.content.block.ranges,
            &self.paragraph.content.block.boxes,
            &self.style,
            more_content,
            self.truncation.is_some(),
            truncation_width,
            overflow_cut,
            width,
        );

        let result = match plan {
            None => self.finish_untruncated(&natural_lines, constraint),
            Some(plan) => {
                self.finish_truncated(context, &plan, &natural_lines, constraint, recycled)
            }
        };
        self.result = Some(result);
    }

    fn reported(&self) -> BlockMetrics {
        let result = self.expect_result();
        BlockMetrics {
            size: result.size,
            first_baseline: result.first_baseline,
            last_baseline: result.lines.last().map(|line| line.baseline),
        }
    }

    fn fits_from(&self) -> Option<f32> {
        let result = self.result.as_ref()?;
        (result.constraint.max_advance().is_none() && result.display.is_none())
            .then(|| self.paragraph.natural.width())
    }
}

fn reported_line<'a>(
    final_layout: &'a Layout<TextBrush>,
    natural_fallback: Option<&'a Layout<TextBrush>>,
    natural_line: NaturalLine,
    index: usize,
) -> (parley::Line<'a, TextBrush>, bool) {
    let rendered = final_layout
        .get(index)
        .filter(|line| natural_line.is_empty() || !shape::is_blank_line(line));
    let fallback = rendered.is_none();
    let line = rendered
        .or_else(|| natural_fallback.and_then(|layout| layout.get(index)))
        .expect("every visible line exists in one of the layouts");
    (line, fallback)
}

fn assemble(
    final_layout: &Layout<TextBrush>,
    natural_fallback: Option<&Layout<TextBrush>>,
    natural_lines: &[NaturalLine],
    visible: usize,
    plan: Option<&CutPlan>,
    slot_specs: &[InlineBoxSpec],
    hidden: Vec<PlacedBox>,
) -> (Vec<LineInfo>, Vec<PlacedBox>, Size<f32>, Option<f32>) {
    let mut lines = Vec::with_capacity(visible);
    let mut boxes = Vec::new();
    let mut bottom = 0.0f32;
    let mut used_fallback = false;

    for (index, &natural_line) in natural_lines.iter().enumerate().take(visible) {
        let (line, fallback) = reported_line(final_layout, natural_fallback, natural_line, index);
        used_fallback |= fallback;
        let geometry = LineGeometry::of(&line);
        bottom = bottom.max(geometry.top + geometry.height);
        let ellipsis_count = plan
            .filter(|plan| plan.cut_line as usize == index)
            .map_or(0, |plan| {
                natural_line.end_unit.saturating_sub(plan.cut_unit)
            });
        lines.push(LineInfo {
            source_start: natural_line.start_unit,
            source_end: natural_line.end_unit,
            ellipsis_count,
            top: geometry.top,
            baseline: geometry.baseline,
            height: geometry.height,
            advance: line.metrics().advance,
        });
    }

    let mut line_boxes = shape::line_boxes(final_layout);
    for (index, line) in final_layout.lines().enumerate() {
        let count = line_boxes.on(&line).len();
        if count == 0 {
            continue;
        }
        let placed = line.items().filter_map(|item| match item {
            PositionedLayoutItem::InlineBox(inline_box) => Some(inline_box),
            PositionedLayoutItem::GlyphRun(_) => None,
        });
        for inline_box in placed.take(count) {
            let slot = usize::try_from(inline_box.id).expect("slot ids are table indexes");
            let spec = &slot_specs[slot];
            boxes.push(PlacedBox::Visible {
                id: spec.id,
                line: u32::try_from(index).expect("line count fits u32"),
                origin: Point::new(inline_box.x, inline_box.y),
                size: spec.size,
            });
        }
    }
    boxes.extend(hidden);

    let height = if used_fallback || lines.len() < final_layout.len() {
        bottom.max(0.0)
    } else {
        final_layout.height()
    };
    let size = Size::new(final_layout.width(), height);
    let first_baseline = lines.first().map(|line| line.baseline);
    (lines, boxes, size, first_baseline)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::Atom;
    use stylo::values::computed::font::{
        FamilyName, FontFamily, FontFamilyList, FontFamilyNameSyntax, SingleFontFamily,
    };

    use super::*;
    use crate::text::{FontBlob, TextContext};

    const AHEM: &[u8] = include_bytes!("../../../tests/fixtures/Ahem.ttf");

    pub(super) fn text_context() -> TextContext {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        context
    }

    pub(super) fn ahem_style() -> RunStyle {
        RunStyle {
            font_family: FontFamily {
                families: FontFamilyList {
                    list: stylo::ArcSlice::from_iter(std::iter::once(
                        SingleFontFamily::FamilyName(FamilyName {
                            name: Atom::from("Ahem"),
                            syntax: FontFamilyNameSyntax::Identifiers,
                        }),
                    )),
                },
                is_system_font: false,
                is_initial: false,
            },
            font_size: 10.0,
            ..RunStyle::default()
        }
    }

    #[test]
    fn one_shaping_serves_every_break_and_box_resize() {
        let mut context = text_context();
        let style = ahem_style();
        let items = [
            InlineItem::Run(TextRunItem {
                text: "aaaa aaaa",
                style: &style,
                white_space: WhiteSpaceCollapse::Collapse,
                text_security: TextSecurity::None,
            }),
            InlineItem::Box(InlineBoxSpec {
                id: 1,
                size: Size::new(10.0, 10.0),
                baseline: None,
                vertical_align: VerticalAlign::Baseline,
            }),
        ];
        let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);
        assert_eq!(context.shape_count(), 1);

        block.commit(&mut context, BlockConstraint::new(Some(50.0), 0.0));
        assert_eq!(context.shape_count(), 1, "breaking never re-shapes");
        let broken_lines = block.lines().len();

        block.commit(&mut context, BlockConstraint::new(Some(50.0), 0.0));
        assert_eq!(context.shape_count(), 1, "a repeated layout is a no-op");

        block.commit(&mut context, BlockConstraint::new(None, 0.0));
        assert_eq!(context.shape_count(), 1);
        assert_eq!(block.lines().len(), 1);
        assert_ne!(block.lines().len(), broken_lines);

        block.set_box_size(1, Size::new(40.0, 40.0), None);
        block.commit(&mut context, BlockConstraint::new(Some(50.0), 0.0));
        assert_eq!(context.shape_count(), 1, "a box resize re-breaks in place");
        assert_eq!(block.lines().len(), 3);
    }

    #[test]
    fn truncation_measures_its_content_once_and_reshapes_only_the_display() {
        let mut context = text_context();
        let style = ahem_style();
        let items = [InlineItem::Run(TextRunItem {
            text: "aaaa aaaa aaaa",
            style: &style,
            white_space: WhiteSpaceCollapse::Collapse,
            text_security: TextSecurity::None,
        })];
        let tail_style = ahem_style();
        let tail = [InlineItem::Run(TextRunItem {
            text: "XX",
            style: &tail_style,
            white_space: WhiteSpaceCollapse::Collapse,
            text_security: TextSecurity::None,
        })];
        let block_style = BlockStyle {
            max_lines: core::num::NonZeroU32::new(2),
            ..BlockStyle::default()
        };
        let mut block = TextBlock::new(&mut context, block_style, &items, Some(&tail));
        assert_eq!(
            context.shape_count(),
            1,
            "truncation content is not shaped up front"
        );

        block.commit(&mut context, BlockConstraint::new(Some(50.0), 0.0));
        assert_eq!(
            context.shape_count(),
            3,
            "one truncation-content measurement plus one display rebuild",
        );
        assert!(block.truncation_visible());

        block.commit(&mut context, BlockConstraint::new(Some(50.0), 0.0));
        assert_eq!(context.shape_count(), 3, "a repeated layout is a no-op");

        block.commit(&mut context, BlockConstraint::new(Some(60.0), 0.0));
        assert_eq!(
            context.shape_count(),
            4,
            "a new width rebuilds the display; the content width stays cached",
        );
    }

    fn ahem_run<'a>(text: &'a str, style: &'a RunStyle) -> InlineItem<'a> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            white_space: WhiteSpaceCollapse::Collapse,
            text_security: TextSecurity::None,
        })
    }

    #[test]
    fn a_width_the_max_content_lines_fit_reuses_them() {
        let mut context = text_context();
        let style = ahem_style();
        let items = [ahem_run("aaaa aaaa", &style)];
        let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);

        let max_content = block.probe(&mut context, BlockConstraint::new(None, 0.0));
        assert_eq!(block.break_count(), 1);
        assert!((max_content.size.width - 90.0).abs() < f32::EPSILON);

        assert_eq!(
            block.probe(&mut context, BlockConstraint::new(Some(120.0), 0.0)),
            max_content
        );
        assert_eq!(block.break_count(), 1);

        assert_eq!(
            block.commit(&mut context, BlockConstraint::new(Some(90.0), 0.0)),
            max_content
        );
        assert_eq!(block.break_count(), 1);
        assert!(!block.is_probe_dirty());
        assert!(!block.restore_committed(&mut context));
        assert_eq!(block.lines().len(), 1);

        let wrapped = block.probe(&mut context, BlockConstraint::new(Some(89.0), 0.0));
        assert_eq!(block.break_count(), 2);
        assert!(wrapped.size.height > max_content.size.height);
        block.probe(&mut context, BlockConstraint::new(Some(120.0), 10.0));
        assert_eq!(block.break_count(), 3);
    }

    #[test]
    fn a_max_content_answer_moves_no_line_of_another_alignment() {
        let mut context = text_context();
        let style = ahem_style();
        let items = [ahem_run("aaaa aaaa", &style)];
        let centered = BlockStyle {
            text_align: TextAlign::Center,
            ..BlockStyle::default()
        };
        let mut block = TextBlock::new(&mut context, centered, &items, None);

        let max_content = block.probe(&mut context, BlockConstraint::new(None, 0.0));
        assert_eq!(
            block.probe(&mut context, BlockConstraint::new(Some(120.0), 0.0)),
            max_content
        );
        assert_eq!(block.break_count(), 1);
        block.commit(&mut context, BlockConstraint::new(Some(120.0), 0.0));
        assert_eq!(block.break_count(), 2);
    }

    #[test]
    fn a_truncated_max_content_answer_is_no_other_widths_answer() {
        let mut context = text_context();
        let style = ahem_style();
        let items = [ahem_run("aaaa aaaa", &style)];

        let maxlength = BlockStyle {
            max_chars: Some(4),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        };
        let mut block = TextBlock::new(&mut context, maxlength, &items, None);
        let max_content = block.probe(&mut context, BlockConstraint::new(None, 0.0));
        assert!(block.truncated());
        assert!(max_content.size.width < 90.0);
        block.probe(
            &mut context,
            BlockConstraint::new(Some(max_content.size.width), 0.0),
        );
        assert_eq!(block.break_count(), 2);
        block.commit(&mut context, BlockConstraint::new(Some(200.0), 0.0));
        assert_eq!(block.break_count(), 3);

        let first_line = [ahem_run("Hi\nthere", &style)];
        let tail = [ahem_run("XXXX", &style)];
        let maxline = BlockStyle {
            max_lines: NonZeroU32::new(1),
            ..BlockStyle::default()
        };
        let mut block = TextBlock::new(&mut context, maxline, &first_line, Some(&tail));
        let max_content = block.probe(&mut context, BlockConstraint::new(None, 0.0));
        assert!(block.truncation_visible());
        block.commit(
            &mut context,
            BlockConstraint::new(Some(max_content.size.width), 0.0),
        );
        assert_eq!(block.break_count(), 2);
        assert!(!block.truncation_visible());
    }

    #[test]
    fn tabs_reshape_only_when_a_break_moves_their_stop() {
        let mut context = text_context();
        let style = ahem_style();
        let items = [InlineItem::Run(TextRunItem {
            text: "ab c\td",
            style: &style,
            white_space: WhiteSpaceCollapse::Preserve,
            text_security: TextSecurity::None,
        })];
        let mut block = TextBlock::new(&mut context, BlockStyle::default(), &items, None);
        assert_eq!(
            context.shape_count(),
            1,
            "the paragraph alone: a native stop measures no `0`",
        );

        block.commit(&mut context, BlockConstraint::new(Some(500.0), 0.0));
        assert_eq!(
            context.shape_count(),
            2,
            "one re-shape settles every tab of an unbroken paragraph",
        );
        block.commit(&mut context, BlockConstraint::new(None, 0.0));
        assert_eq!(context.shape_count(), 2, "no break, no tab moves");

        block.commit(&mut context, BlockConstraint::new(Some(25.0), 0.0));
        assert_eq!(context.shape_count(), 3, "the wrap moves the tab's stop");
        block.commit(&mut context, BlockConstraint::new(Some(500.0), 0.0));
        assert_eq!(context.shape_count(), 4, "and unwrapping moves it back");

        block.commit(&mut context, BlockConstraint::new(Some(500.0), 10.0));
        assert_eq!(
            context.shape_count(),
            4,
            "a native stop counts from the text start after the indent",
        );
        assert_eq!(block.break_count(), 5);
    }
}
