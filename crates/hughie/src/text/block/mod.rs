//! Lynx text-block layout: one flattened paragraph with inline atomic boxes.
//!
//! This is the standalone home of Lynx `<text>` layout semantics — the
//! flattened single paragraph, atomic inline views and images,
//! `text-maxline` / `text-maxlength` / `text-overflow` truncation with
//! inline-truncation content, and the per-line data the `layout` event
//! reports — implemented directly on parley and deliberately not wired into
//! the box-protocol measurement path (`crate::text::TextMeasurer` and its
//! consumers are untouched). It takes its own parameter structs instead of
//! [`LayoutInput`](crate::tree::LayoutInput): the capabilities that define it
//! are element attributes and Lynx-specific grammar with no home in the box
//! wire format. The behavioral ground truth is recorded in
//! `docs/tracking/css-text.md`; the web reference implementation's truncation
//! algorithm is the only inspectable one and this module follows it.
//!
//! Mutation tiers, cheapest to dearest:
//!
//! - **Re-position** — reading a finished layout costs nothing; positions are assembled once per
//!   [`TextBlock::layout`] call.
//! - **Re-break** — a new width, or [`TextBlock::set_box_size`], re-breaks the retained shaped
//!   layout in place through `inline_boxes_mut`; shaping is not re-run. A repeated `layout` at
//!   unchanged inputs is a no-op.
//! - **Re-shape (bounded)** — a truncating layout builds one extra display layout over the visible
//!   prefix plus the tail; the inline-truncation content's own width is shaped once and cached.
//! - **Rebuild** — changed text or styles mean a new [`TextBlock`]; there is no in-place path,
//!   matching parley's own mutability contract.

mod content;
mod position;
mod shape;
mod style;
mod truncate;

use core::fmt;
use core::num::NonZeroU32;

pub use content::{InlineBoxSpec, InlineItem, TextRunItem};
use content::{NormalizedBlock, normalize};
use parley::{
    AlignmentOptions, ContentWidths, IndentOptions, InlineBox, InlineBoxKind, Layout,
    PositionedLayoutItem,
};
use shape::{NaturalLine, ShapeSpan};
use style::resolve_alignment;
pub use style::{
    BlockStyle, Direction, LineHeight, RunStyle, TextAlign, TextIndent, TextOverflow, TextWrap,
    VerticalAlign, WordBreak,
};
use truncate::{CutPlan, Tail};

use crate::geometry::{Point, Size};
use crate::style::TextBrush;
use crate::text::TextContext;

/// Paint identity of one parley style index or placed box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceItem {
    /// Index into the `items` slice the block was built from.
    Content(u32),
    /// Index into the `truncation` slice.
    Truncation(u32),
    /// The synthesized dots run.
    Ellipsis,
}

/// One reported line, in source UTF-16 units (an atomic box counts one).
///
/// Source ranges come from the natural (pre-truncation) line layout — the
/// layout event's contract — while the geometry is the rendered line's.
/// A range covers the line's visible content only: the collapsed whitespace
/// a soft wrap consumed belongs to no line, so consecutive ranges can have a
/// gap, matching the web reference's rect-derived ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineInfo {
    pub source_start: u32,
    pub source_end: u32,
    /// `source_end − cut` when the cut falls in this line, else zero.
    pub ellipsis_count: u32,
    pub top: f32,
    pub baseline: f32,
    pub height: f32,
    pub advance: f32,
}

/// Where one atomic box ended up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlacedBox {
    Visible {
        /// The host identity from [`InlineBoxSpec::id`].
        id: u64,
        line: u32,
        /// Top-left corner; vertical alignment applied.
        origin: Point<f32>,
        size: Size<f32>,
    },
    /// Truncated away — the Lynx `HideView` surface. No position exists.
    Hidden { id: u64 },
}

/// One owned flattened item.
#[derive(Debug)]
enum OwnedItem {
    Run(RunStyle),
    Box(InlineBoxSpec),
}

/// One owned flattened flow: the main content, or the truncation content.
#[derive(Debug)]
struct Flow {
    items: Vec<OwnedItem>,
    block: NormalizedBlock,
}

impl Flow {
    fn build(items: &[InlineItem<'_>]) -> Self {
        Self {
            items: items
                .iter()
                .map(|item| match item {
                    InlineItem::Run(run) => OwnedItem::Run(run.style.clone()),
                    InlineItem::Box(spec) => OwnedItem::Box(*spec),
                })
                .collect(),
            block: normalize(items),
        }
    }

    fn run_style(&self, item: u32) -> &RunStyle {
        match &self.items[item as usize] {
            OwnedItem::Run(style) => style,
            OwnedItem::Box(_) => unreachable!("styled ranges only reference run items"),
        }
    }

    fn box_spec(&self, item: u32) -> &InlineBoxSpec {
        match &self.items[item as usize] {
            OwnedItem::Box(spec) => spec,
            OwnedItem::Run(_) => unreachable!("box slots only reference box items"),
        }
    }

    /// The shaper spans for this flow, offset into a combined display string.
    fn spans(&self, byte_offset: u32, source: fn(u32) -> SourceItem) -> Vec<ShapeSpan<'_>> {
        self.block
            .ranges
            .iter()
            .map(|range| ShapeSpan {
                bytes: (range.bytes.start + byte_offset) as usize
                    ..(range.bytes.end + byte_offset) as usize,
                style: self.run_style(range.item),
                source: source(range.item),
            })
            .collect()
    }
}

/// The tail slot of the display slot table.
#[derive(Debug, Clone, Copy)]
enum SlotSource {
    Content(u32),
    Truncation(u32),
}

/// The output of one `layout` call.
struct LayoutResult {
    width: Option<f32>,
    /// The truncated display layout and its style/slot tables; absent when
    /// the natural layout is the display.
    display: Option<DisplayPart>,
    lines: Vec<LineInfo>,
    boxes: Vec<PlacedBox>,
    size: Size<f32>,
    first_baseline: Option<f32>,
    truncated: bool,
    truncation_visible: bool,
    /// Read off the unjustified pre-alignment layout with the box sizes this
    /// layout used, so a justified alignment or a pending box resize can
    /// never leak into it.
    content_widths: ContentWidths,
}

struct DisplayPart {
    layout: Layout<TextBrush>,
    sources: Vec<SourceItem>,
}

/// A shaped, breakable, truncatable Lynx text block. One per text element.
pub struct TextBlock {
    style: BlockStyle,
    content: Flow,
    truncation: Option<TruncationFlow>,
    natural: Layout<TextBrush>,
    natural_sources: Vec<SourceItem>,
    boxes_dirty: bool,
    result: Option<LayoutResult>,
}

#[derive(Debug)]
struct TruncationFlow {
    flow: Flow,
    /// Cached unconstrained width of the truncation content — constraint-
    /// independent, so it is measured at most once per block.
    natural_width: Option<f32>,
}

impl fmt::Debug for TextBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("TextBlock").finish_non_exhaustive()
    }
}

impl TextBlock {
    /// Builds and shapes one block from flattened content.
    ///
    /// Host contract (the Lynx flattening semantics): walk the `<text>`
    /// subtree iteratively — nesting depth is unbounded — emitting the
    /// establishing element's own text first, then children in document
    /// order; a nested text contributes runs carrying its own resolved
    /// [`RunStyle`]; a wrapper is iterated in place; an inline image or view
    /// contributes exactly one [`InlineItem::Box`] measured independently
    /// (its inner content never joins this paragraph); any other element is
    /// skipped; only the first `inline-truncation` subtree becomes
    /// `truncation`. Box ids are unique across both slices.
    #[must_use]
    pub fn new(
        context: &mut TextContext,
        block_style: BlockStyle,
        items: &[InlineItem<'_>],
        truncation: Option<&[InlineItem<'_>]>,
    ) -> Self {
        let flow = Flow::build(items);
        let truncation = truncation.map(|items| TruncationFlow {
            flow: Flow::build(items),
            natural_width: None,
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

        let spans = flow.spans(0, SourceItem::Content);
        let boxes = parley_boxes(&flow, 0, 0);
        let mut natural_sources = Vec::new();
        let natural = shape::shape(
            context,
            &block_style,
            &flow.block.text,
            &spans,
            boxes,
            &mut natural_sources,
        );
        drop(spans);
        Self {
            style: block_style,
            content: flow,
            truncation,
            natural,
            natural_sources,
            boxes_dirty: false,
            result: None,
        }
    }

    /// Updates one atomic box's measured size — the Lynx measure/align round
    /// trip. The next [`Self::layout`] re-breaks without re-shaping.
    ///
    /// Panics on an unknown id: a correct host cannot produce one.
    pub fn set_box_size(&mut self, id: u64, size: Size<f32>, baseline: Option<f32>) {
        let spec = self
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
        spec.size = size;
        spec.baseline = baseline;
        self.boxes_dirty = true;
        // Only a resized truncation box invalidates the cached truncation
        // content width; a main-flow resize leaves it valid.
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

    /// Lays the block out at `width` (`None` = unconstrained). A repeat call
    /// with unchanged inputs returns without touching parley.
    pub fn layout(&mut self, context: &mut TextContext, width: Option<f32>) {
        let width = width.map(|value| value.max(0.0));
        if !self.boxes_dirty
            && self
                .result
                .as_ref()
                .is_some_and(|result| result.width == width)
        {
            return;
        }
        self.boxes_dirty = false;

        // Box widths and vertical-align line contributions feed the breaker.
        for (slot, entry) in self.natural.inline_boxes_mut().iter_mut().enumerate() {
            let spec = self.content.box_spec(self.content.block.boxes[slot].item);
            entry.width = spec.size.width;
            entry.height = position::line_contribution(spec);
        }

        let indent = match self.style.text_indent {
            TextIndent::Px(value) => value,
            TextIndent::Percent(fraction) => width.map_or(0.0, |basis| fraction * basis),
        };
        self.natural
            .set_text_indent(indent, IndentOptions::default());

        shape::break_clamped(
            &mut self.natural,
            width,
            self.style.max_lines.map(NonZeroU32::get),
        );
        // Content widths are read off the just-unjustified, pre-alignment
        // layout, with the current box sizes written in.
        let content_widths = self.natural.calculate_content_widths();

        let slot_bytes: Vec<u32> = self
            .content
            .block
            .boxes
            .iter()
            .map(|entry| entry.byte)
            .collect();
        let slot_units: Vec<u32> = self
            .content
            .block
            .boxes
            .iter()
            .map(|entry| entry.unit)
            .collect();
        let natural_lines = shape::capture_lines(
            &self.natural,
            &self.content.block.text,
            &self.content.block.map,
            &slot_bytes,
            &slot_units,
        );

        // Whether content remains past the committed lines is a unit-space
        // fact, never the breaker's: parley reports itself unfinished while
        // only a renderless final line is pending.
        let more_content = natural_lines
            .last()
            .is_some_and(|line| line.consumed_end < self.content.block.map.source_len());

        let truncation_width = if more_content && self.truncation.is_some() {
            Some(self.measure_truncation(context))
        } else {
            None
        };

        let plan = truncate::plan(
            &self.natural,
            &natural_lines,
            &self.content.block.text,
            &self.content.block.map,
            &self.content.block.ranges,
            &slot_units,
            &self.style,
            more_content,
            self.truncation.is_some(),
            truncation_width,
            width,
        );

        let result = match plan {
            None => self.finish_untruncated(&natural_lines, width, content_widths),
            Some(plan) => self.finish_truncated(
                context,
                &plan,
                &natural_lines,
                indent,
                width,
                content_widths,
            ),
        };
        self.result = Some(result);
    }

    fn finish_untruncated(
        &mut self,
        natural_lines: &[NaturalLine],
        width: Option<f32>,
        content_widths: ContentWidths,
    ) -> LayoutResult {
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        self.natural.align(alignment, AlignmentOptions::default());
        let slot_specs: Vec<InlineBoxSpec> = self
            .content
            .block
            .boxes
            .iter()
            .map(|entry| *self.content.box_spec(entry.item))
            .collect();
        // Truncation content is not shown when nothing truncates; its boxes
        // still report an outcome so a host can drive their visibility.
        let hidden = self
            .truncation
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
            })
            .collect();
        let (lines, boxes, size, first_baseline) = assemble(
            &self.natural,
            None,
            natural_lines,
            natural_lines.len(),
            None,
            &slot_specs,
            hidden,
        );
        LayoutResult {
            width,
            display: None,
            lines,
            boxes,
            size,
            first_baseline,
            truncated: false,
            truncation_visible: false,
            content_widths,
        }
    }

    fn finish_truncated(
        &self,
        context: &mut TextContext,
        plan: &CutPlan,
        natural_lines: &[NaturalLine],
        indent: f32,
        width: Option<f32>,
        content_widths: ContentWidths,
    ) -> LayoutResult {
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        let visible = plan.cut_line as usize + 1;
        let (mut display, sources, slots) = self.rebuild(context, plan, indent, width, visible);
        display.align(alignment, AlignmentOptions::default());
        let slot_specs: Vec<InlineBoxSpec> = slots
            .iter()
            .map(|slot| match *slot {
                SlotSource::Content(item) => *self.content.box_spec(item),
                SlotSource::Truncation(item) => *self
                    .truncation
                    .as_ref()
                    .expect("truncation slots come from truncation content")
                    .flow
                    .box_spec(item),
            })
            .collect();
        let hidden = self.hidden_boxes(plan);
        let (lines, boxes, size, first_baseline) = assemble(
            &display,
            Some(&self.natural),
            natural_lines,
            visible,
            Some(plan),
            &slot_specs,
            hidden,
        );
        LayoutResult {
            width,
            display: Some(DisplayPart {
                layout: display,
                sources,
            }),
            lines,
            boxes,
            size,
            first_baseline,
            truncated: true,
            truncation_visible: plan.truncation_visible,
            content_widths,
        }
    }

    /// Shapes the truncation content unconstrained and caches its width.
    fn measure_truncation(&mut self, context: &mut TextContext) -> f32 {
        let style = &self.style;
        let part = self
            .truncation
            .as_mut()
            .expect("caller checked for truncation content");
        if let Some(width) = part.natural_width {
            return width;
        }
        let spans = part.flow.spans(0, SourceItem::Truncation);
        let boxes = parley_boxes(&part.flow, 0, 0);
        let mut sources = Vec::new();
        let mut probe = shape::shape(
            context,
            style,
            &part.flow.block.text,
            &spans,
            boxes,
            &mut sources,
        );
        drop(spans);
        probe.break_all_lines(None);
        let width = probe.width();
        part.natural_width = Some(width);
        width
    }

    /// Builds the truncated display layout: the visible prefix plus the tail,
    /// broken to at most the visible line count so a wrapped tail is clamped
    /// away rather than adding a line.
    fn rebuild(
        &self,
        context: &mut TextContext,
        plan: &CutPlan,
        indent: f32,
        width: Option<f32>,
        visible: usize,
    ) -> (Layout<TextBrush>, Vec<SourceItem>, Vec<SlotSource>) {
        let cut = plan.cut_byte as usize;
        let mut text = String::with_capacity(cut + 16);
        text.push_str(&self.content.block.text[..cut]);

        let mut spans: Vec<ShapeSpan<'_>> = Vec::new();
        for range in &self.content.block.ranges {
            if range.bytes.start >= plan.cut_byte {
                break;
            }
            spans.push(ShapeSpan {
                bytes: range.bytes.start as usize..range.bytes.end.min(plan.cut_byte) as usize,
                style: self.content.run_style(range.item),
                source: SourceItem::Content(range.item),
            });
        }
        let mut slots = Vec::new();
        let mut boxes = Vec::new();
        for entry in &self.content.block.boxes {
            // The cut is a unit-space decision: a box shares its byte with
            // the character after it, so byte comparison cannot tell a cut
            // on the box from a cut after it.
            if entry.unit >= plan.cut_unit {
                break;
            }
            let spec = self.content.box_spec(entry.item);
            boxes.push(sized_box(spec, slots.len(), entry.byte as usize));
            slots.push(SlotSource::Content(entry.item));
        }

        match plan.tail {
            Tail::None => {}
            Tail::Dots { count, item } => {
                let start = text.len();
                text.push_str(&"..."[..count as usize]);
                spans.push(ShapeSpan {
                    bytes: start..text.len(),
                    style: self.content.run_style(item),
                    source: SourceItem::Ellipsis,
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
                spans.extend(part.spans(offset, SourceItem::Truncation));
                for entry in &part.block.boxes {
                    let spec = part.box_spec(entry.item);
                    boxes.push(sized_box(spec, slots.len(), (entry.byte + offset) as usize));
                    slots.push(SlotSource::Truncation(entry.item));
                }
            }
        }

        let mut sources = Vec::new();
        let mut display = shape::shape(context, &self.style, &text, &spans, boxes, &mut sources);
        drop(spans);
        display.set_text_indent(indent, IndentOptions::default());
        let limit = u32::try_from(visible).expect("line count fits u32");
        shape::break_clamped(&mut display, width, Some(limit));
        (display, sources, slots)
    }

    /// The `Hidden` entries: main boxes at or past the cut, plus the whole
    /// truncation content when it is not shown.
    fn hidden_boxes(&self, plan: &CutPlan) -> Vec<PlacedBox> {
        let mut hidden = Vec::new();
        for entry in &self.content.block.boxes {
            if entry.unit >= plan.cut_unit {
                hidden.push(PlacedBox::Hidden {
                    id: self.content.box_spec(entry.item).id,
                });
            }
        }
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

    /// The rendered layout — glyph geometry, decoration metrics,
    /// justification. Box positions in here are not valid; [`Self::boxes`]
    /// is the only box source. Valid after [`Self::layout`].
    #[must_use]
    pub fn display(&self) -> &Layout<TextBrush> {
        let result = self.expect_result();
        result
            .display
            .as_ref()
            .map_or(&self.natural, |part| &part.layout)
    }

    /// Paint identity of one parley `style_index` from [`Self::display`].
    #[must_use]
    pub fn source_of(&self, style_index: u16) -> SourceItem {
        let result = self.expect_result();
        let sources = result
            .display
            .as_ref()
            .map_or(&self.natural_sources, |part| &part.sources);
        sources[usize::from(style_index)]
    }

    /// The layout-event data: one entry per visible line.
    #[must_use]
    pub fn lines(&self) -> &[LineInfo] {
        &self.expect_result().lines
    }

    /// Every atomic box's outcome, visible boxes with vertical alignment
    /// applied.
    #[must_use]
    pub fn boxes(&self) -> &[PlacedBox] {
        &self.expect_result().boxes
    }

    /// The rendered size: widest line (trailing whitespace excluded) by
    /// stacked line height.
    #[must_use]
    pub fn size(&self) -> Size<f32> {
        self.expect_result().size
    }

    /// The first rendered line's baseline.
    #[must_use]
    pub fn first_baseline(&self) -> Option<f32> {
        self.expect_result().first_baseline
    }

    /// Whether a cut was applied.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.expect_result().truncated
    }

    /// Whether the inline-truncation content is shown — the web
    /// `x-show-inline-truncation` surface.
    #[must_use]
    pub fn truncation_visible(&self) -> bool {
        self.expect_result().truncation_visible
    }

    /// Min- and max-content widths of the full (untruncated) content, with
    /// the box sizes the last layout used. Valid after [`Self::layout`].
    #[must_use]
    pub fn content_widths(&self) -> ContentWidths {
        self.expect_result().content_widths
    }

    fn expect_result(&self) -> &LayoutResult {
        self.result
            .as_ref()
            .expect("a TextBlock is read after layout()")
    }
}

/// The parley boxes of one flow, sized from the current specs and carrying
/// their vertical-align line contribution as the height.
fn parley_boxes(flow: &Flow, byte_offset: u32, base_slot: usize) -> Vec<InlineBox> {
    flow.block
        .boxes
        .iter()
        .enumerate()
        .map(|(slot, entry)| {
            let spec = flow.box_spec(entry.item);
            sized_box(spec, base_slot + slot, (entry.byte + byte_offset) as usize)
        })
        .collect()
}

fn sized_box(spec: &InlineBoxSpec, slot: usize, index: usize) -> InlineBox {
    InlineBox {
        id: slot as u64,
        kind: InlineBoxKind::InFlow,
        index,
        width: spec.size.width,
        height: position::line_contribution(spec),
    }
}

/// Walks the final layout once, assembling line reports and box placements.
///
/// A cut at a line start can leave the display layout one line short of the
/// visible count; the natural layout — still committed at the same widths —
/// supplies that line's geometry.
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
        let rendered = final_layout
            .get(index)
            .filter(|line| !shape::is_blank_line(line));
        if rendered.is_none() {
            used_fallback = true;
        }
        let line = rendered
            .or_else(|| natural_fallback.and_then(|layout| layout.get(index)))
            .expect("every visible line exists in one of the layouts");
        let metrics = line.metrics();
        bottom = bottom.max(metrics.block_max_coord);
        let ellipsis_count = plan
            .filter(|plan| plan.cut_line as usize == index)
            .map_or(0, |plan| {
                natural_line.end_unit.saturating_sub(plan.cut_unit)
            });
        lines.push(LineInfo {
            source_start: natural_line.start_unit,
            source_end: natural_line.end_unit,
            ellipsis_count,
            top: metrics.block_min_coord,
            baseline: metrics.baseline,
            height: metrics.line_height,
            advance: metrics.advance,
        });
    }

    for (index, line) in final_layout.lines().enumerate() {
        let refs = position::line_refs(&line);
        let metrics = *line.metrics();
        for item in line.items() {
            if let PositionedLayoutItem::InlineBox(inline_box) = item {
                let slot = usize::try_from(inline_box.id).expect("slot ids are table indexes");
                let spec = &slot_specs[slot];
                let top = position::box_top(spec, &metrics, refs);
                boxes.push(PlacedBox::Visible {
                    id: spec.id,
                    line: u32::try_from(index).expect("line count fits u32"),
                    origin: Point::new(inline_box.x, top),
                    size: spec.size,
                });
            }
        }
    }
    boxes.extend(hidden);

    // Blank or clamped-away trailing lines still count toward parley's own
    // height, so the reported height comes from the last reported line's
    // bottom edge whenever the two disagree.
    let height = if used_fallback || lines.len() < final_layout.len() {
        bottom.max(0.0)
    } else {
        final_layout.height()
    };
    let size = Size::new(final_layout.width(), height);
    // The first reported line's baseline, whichever layout supplied it — so
    // this can never disagree with `lines()`.
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

    fn text_context() -> TextContext {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        context
    }

    fn ahem_style() -> RunStyle {
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
                preserve_newlines: false,
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

        block.layout(&mut context, Some(50.0));
        assert_eq!(context.shape_count(), 1, "breaking never re-shapes");
        let broken_lines = block.lines().len();

        block.layout(&mut context, Some(50.0));
        assert_eq!(context.shape_count(), 1, "a repeated layout is a no-op");

        block.layout(&mut context, None);
        assert_eq!(context.shape_count(), 1);
        assert_eq!(block.lines().len(), 1);
        assert_ne!(block.lines().len(), broken_lines);

        block.set_box_size(1, Size::new(40.0, 40.0), None);
        block.layout(&mut context, Some(50.0));
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
            preserve_newlines: false,
        })];
        let tail_style = ahem_style();
        let tail = [InlineItem::Run(TextRunItem {
            text: "XX",
            style: &tail_style,
            preserve_newlines: false,
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

        block.layout(&mut context, Some(50.0));
        assert_eq!(
            context.shape_count(),
            3,
            "one truncation-content measurement plus one display rebuild",
        );
        assert!(block.truncation_visible());

        block.layout(&mut context, Some(50.0));
        assert_eq!(context.shape_count(), 3, "a repeated layout is a no-op");

        block.layout(&mut context, Some(60.0));
        assert_eq!(
            context.shape_count(),
            4,
            "a new width rebuilds the display; the content width stays cached",
        );
    }
}
