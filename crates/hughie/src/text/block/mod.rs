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
//!   layout in place through `inline_boxes_mut`; shaping is not re-run. A repeated `layout` at
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

use parley::{AlignmentOptions, ContentWidths, Layout, PositionedLayoutItem};
pub use style::{BlockStyle, TextOverflow};
use truncate::{CutPlan, OverflowCut, Tail};

use super::paragraph::memo::{BreakMemo, Retained};
use super::paragraph::shape::{self, NaturalLine, ShapeSpan};
use super::paragraph::style::resolve_alignment;
pub use super::paragraph::{
    BlockConstraint, BlockMetrics, Direction, InlineBoxSpec, InlineItem, LineHeight, RunStyle,
    TextAlign, TextIndent, TextRunItem, TextWrap, VerticalAlign, WhiteSpaceCollapse, WordBreak,
};
use super::paragraph::{
    Flow, LineGeometry, OwnedItem, Paragraph, ParagraphRoot, RunBreaking, parley_boxes, sized_box,
};
use crate::geometry::{Point, Size};
use crate::style::TextBrush;
use crate::text::TextContext;

/// Paint identity of one parley style index or placed box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceItem {
    /// The root inline box's strut ([`BlockStyle::strut`]): parley's style 0,
    /// which no text is shaped in, so no glyph run carries it.
    Strut,
    /// Index into the `items` slice the block was built from.
    Content(u32),
    /// Index into the `truncation` slice.
    Truncation(u32),
    /// The synthesized dots run, carrying the content run item it was shaped
    /// in — the run holding the last visible byte, which is also whose colour
    /// the marker takes unless `tail-color-convert` overrides it.
    Ellipsis { item: u32 },
}

/// One reported line, in source UTF-16 units (an atomic box counts one).
///
/// Source ranges come from the natural (pre-truncation) line layout — the
/// layout event's contract — while the geometry is the rendered line's.
/// A range covers the line's visible content only: the collapsed whitespace
/// a soft wrap consumed belongs to no line, so consecutive ranges can have a
/// gap, matching the web reference's rect-derived ranges.
///
/// A verbatim paragraph whose text ends with a newline reports one more
/// line, empty, as Android's `StaticLayout` does: `source_start ==
/// source_end ==` the source length, with the height of the line before it.
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

/// The tail slot of the display slot table.
#[derive(Debug, Clone, Copy)]
enum SlotSource {
    Content(u32),
    Truncation(u32),
}

/// The dots-form ellipsis marker, as text.
const DOTS: &str = "...";

/// The output of one `layout_at` call.
struct LayoutResult {
    constraint: BlockConstraint,
    /// The truncated display layout and its style/slot tables; absent when
    /// the natural layout is the display.
    display: Option<DisplayPart>,
    lines: Vec<LineInfo>,
    /// The reported lines' placement, parley's own.
    geometry: Vec<LineGeometry>,
    boxes: Vec<PlacedBox>,
    size: Size<f32>,
    first_baseline: Option<f32>,
    truncated: bool,
    truncation_visible: bool,
}

struct DisplayPart {
    layout: Layout<TextBrush>,
    sources: Vec<SourceItem>,
    /// Where the source content ends in this layout's text: the bytes before
    /// it are the content's own normalized text, the bytes from it on the
    /// tail (the dots or the truncation content), which is no source text.
    cut_byte: u32,
    /// How many of this layout's box slots are content boxes. They come
    /// first, in the content's own order; the truncation content's follow.
    content_slots: usize,
}

/// A shaped, breakable, truncatable Lynx text block. One per text element.
pub struct TextBlock {
    style: BlockStyle,
    /// The content flow and its natural layout.
    paragraph: Paragraph<SourceItem>,
    truncation: Option<TruncationFlow>,
    result: Option<LayoutResult>,
    /// The answered constraints, the committed one, and the counters
    /// [`Self::break_count`] and [`Self::min_content_width`] read.
    memo: BreakMemo,
    /// The dots' advance in one content run's style, for the clip-edge cut.
    ///
    /// Constraint-independent, like the truncation content's own width: the
    /// run styles a block was built from never change without a rebuild, so
    /// one entry serves every `layout_at` that asks about the same run.
    dots_width: Option<(u32, f32)>,
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
        // Native Lynx keeps a `<text>`'s bidi paragraph level first-strong
        // (its platform engines detect it), and so does this.
        let root = ParagraphRoot {
            strut: block_style
                .strut
                .clone()
                .map(|strut| (strut, SourceItem::Strut)),
            direction: parley::BaseDirection::Auto,
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
        let truncation = truncation.map(|items| TruncationFlow {
            flow: Flow::build(items, block_style.processing(false)).0,
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
        Self {
            style: block_style,
            paragraph,
            truncation,
            result: None,
            memo: BreakMemo::new(),
            dots_width: None,
        }
    }

    /// Replaces the paragraph parameters without re-shaping.
    ///
    /// Sound only for values parley does not shape from: alignment and
    /// direction are applied after breaking, and truncation is decided from a
    /// finished layout. A font, a letter-spacing, a word-break, a
    /// `text_wrap` or a `verbatim` change is a different paragraph and needs a
    /// new block — the last two choose the white-space processing the text
    /// was normalized with. The host decides which it has, because it is the
    /// one holding the old style to compare.
    ///
    /// Invalidates the break memo, since `text-align` moves line offsets and
    /// `max_lines` moves the reported size.
    pub fn set_style(&mut self, style: BlockStyle) {
        if self.style == style {
            return;
        }
        self.style = style;
        self.memo.forget();
        self.result = None;
    }

    /// Reports `constraint` without disturbing the committed line breaks when
    /// the same constraint has already been answered.
    ///
    /// A probe is what a container issues while sizing — max-content,
    /// min-content, a trial width — and none of those answers is what paint
    /// reads. A memo hit therefore returns without re-entering parley *and*
    /// without moving the retained layout, which is the whole reason the memo
    /// exists rather than a single-entry cache.
    pub fn probe(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        Retained::probe(self, context, constraint)
    }

    /// Lays the block out for real. A commit's line breaks are the ones paint
    /// reads, so this can never be answered from the memo.
    pub fn commit(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        Retained::commit(self, context, constraint)
    }

    /// Whether a commit has ever produced this block's retained result.
    ///
    /// Every reader below panics if it has not; this is the question a host
    /// asks first, so a paragraph no commit produced is a paragraph that does
    /// not paint rather than one that panics.
    #[must_use]
    pub const fn has_committed(&self) -> bool {
        self.memo.committed().is_some()
    }

    /// Whether a probe left the retained result somewhere the committed
    /// constraint did not put it.
    ///
    /// Tolerates a missing result on purpose: the host's measure closure is
    /// not guaranteed to have run, and this is the one reader that must answer
    /// before that is known.
    #[must_use]
    pub fn is_probe_dirty(&self) -> bool {
        Retained::is_probe_dirty(self)
    }

    /// Puts the retained layout back where the last commit left it, so what
    /// paint reads is never a probe's line breaks.
    ///
    /// Unlike the measurement path's namesake this needs a context: a
    /// truncating block rebuilds its display layout, which re-shapes.
    /// Answers whether parley was re-entered.
    pub fn restore_committed(&mut self, context: &mut TextContext) -> bool {
        Retained::restore_committed(self, context)
    }

    /// The narrowest this block's content can be, in px, at the resolved
    /// `text-indent` `indent`, which counts on the lines it applies to — the
    /// first, and in a verbatim block every line after a newline
    /// ([`BlockStyle`]'s indent rule).
    ///
    /// Read off a laid-out result rather than recomputed, because parley's
    /// `calculate_content_widths` reflects the box sizes and the indent
    /// currently written into the layout — and at `new` no atom has a size
    /// yet. Forces one unconstrained layout at `indent` when no result was
    /// laid out at it. The answer is remembered per indent.
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

    /// How many times parley has broken these lines. Tests and benchmarks
    /// assert on it; nothing else should.
    #[must_use]
    pub const fn break_count(&self) -> u32 {
        self.memo.breaks()
    }

    /// Updates one atomic box's measured size — the Lynx measure/align round
    /// trip. The next [`Self::probe`] or [`Self::commit`] re-breaks without
    /// re-shaping.
    ///
    /// Panics on an unknown id: a correct host cannot produce one.
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
            // A host re-asserting the size it already gave must not invalidate
            // anything: the measure/align round trip repeats every pass, and
            // treating a repeat as a change would defeat the memo outright.
            return;
        }
        spec.size = size;
        spec.baseline = baseline;
        self.memo.mark_boxes_dirty();
        // A resized atom changes both the break and the intrinsic widths, so
        // every remembered answer is stale. `committed` deliberately survives:
        // it names where the retained layout must be put back, and clearing it
        // would make `has_committed` false and drop the paragraph from paint.
        self.memo.forget();
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

    fn finish_untruncated(
        &mut self,
        natural_lines: &[NaturalLine],
        constraint: BlockConstraint,
    ) -> LayoutResult {
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        self.paragraph
            .natural
            .align(alignment, AlignmentOptions::default());
        let slot_specs: Vec<InlineBoxSpec> = self
            .paragraph
            .content
            .block
            .boxes
            .iter()
            .map(|entry| *self.paragraph.content.box_spec(entry.item))
            .collect();
        // Truncation content is not shown when nothing truncates; its boxes
        // still report an outcome so a host can drive their visibility, as do
        // the boxes a `nowrap` verbatim paragraph dropped after its first
        // newline.
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
            geometry: parley_geometry(&lines),
            lines,
            boxes,
            size,
            first_baseline,
            truncated: false,
            truncation_visible: false,
        }
    }

    fn finish_truncated(
        &self,
        context: &mut TextContext,
        plan: &CutPlan,
        natural_lines: &[NaturalLine],
        constraint: BlockConstraint,
    ) -> LayoutResult {
        let width = constraint.max_advance();
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        let visible = plan.cut_line as usize + 1;
        let (mut display, sources, slots) =
            self.rebuild(context, plan, constraint.indent, width, visible);
        display.align(alignment, AlignmentOptions::default());
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
        LayoutResult {
            constraint,
            display: Some(DisplayPart {
                layout: display,
                sources,
                cut_byte: plan.cut_byte,
                content_slots,
            }),
            geometry: parley_geometry(&lines),
            lines,
            boxes,
            size,
            first_baseline,
            truncated: true,
            truncation_visible: plan.truncation_visible,
        }
    }

    /// The clip-edge cut a `white-space: nowrap` line wider than its measure
    /// takes — css-ui `text-overflow` in its own right, with neither
    /// `text-maxline` nor `text-maxlength` taking part.
    ///
    /// The clamp path cannot see this case: the one line consumed all of its
    /// source, so nothing "remains past the committed lines". What overflows
    /// is the line box, and the decision is therefore an advance against the
    /// constraint.
    ///
    /// Restricted to a paragraph whose nowrap breaking left exactly one line.
    /// A cut drops every line past the one it falls in, which is right for the
    /// single unbroken line `nowrap` normally produces and wrong for the
    /// several a preserved newline can still leave, so that shape is left
    /// uncut rather than half-served.
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
        // The widest prefix that fits the measure with nothing appended. Both
        // branches below start from it.
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
                // Too wide for the container, exactly as on the clamp path:
                // the content is hidden and the line keeps nothing.
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
            // `text-overflow: clip` appends nothing and cuts nothing: the line
            // overflows and the element's own `overflow` clips the ink.
            return None;
        }
        let cut_byte = map
            .unit_to_byte(text, fit)
            .unwrap_or_else(|| u32::try_from(text.len()).expect("text fits u32"));
        let item = truncate::dots_item(&self.paragraph.content.block.ranges, cut_byte)?;
        // The dots are shaped in the run at the fit boundary and measured
        // once. Backing off could in principle land the cut in an earlier run
        // with a different advance; re-measuring per candidate would cost a
        // shaping each, and the boundary run is the one the marker is drawn in
        // for every case a single-run or uniformly-sized paragraph produces.
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

    /// Shapes the three-dot marker in one run's style and caches its advance.
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

    /// Shapes the truncation content unconstrained and caches its width.
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
        text.push_str(&self.paragraph.content.block.text[..cut]);

        // The content's own spans, tab spacing included, clipped at the cut:
        // the visible prefix breaks exactly as it did in the natural layout.
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
            // The cut is a unit-space decision: a box shares its byte with
            // the character after it, so byte comparison cannot tell a cut
            // on the box from a cut after it.
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
        // The same root inline box as the natural layout, so the visible
        // lines keep their line boxes.
        let mut display = shape::shape(
            context,
            &text,
            &spans,
            boxes,
            &mut sources,
            self.paragraph.root.line_root(),
        );
        drop(spans);
        // The natural layout's indent rule, so every visible line keeps the
        // indent it was broken with.
        display.set_text_indent(indent, BlockStyle::INDENT_KEYWORDS.options());
        let limit = u32::try_from(visible).expect("line count fits u32");
        shape::break_clamped(&mut display, width, Some(limit));
        (display, sources, slots)
    }

    /// The boxes a `nowrap` verbatim paragraph dropped after its first
    /// newline: content with a source unit and no place in the text.
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

    /// The `Hidden` entries: main boxes at or past the cut, the boxes a
    /// `nowrap` verbatim paragraph dropped, plus the whole truncation content
    /// when it is not shown.
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

    /// The rendered layout — glyph geometry, decoration metrics,
    /// justification. Box positions in here are parley's, which
    /// [`Self::boxes`] reports with each box's host identity; read boxes
    /// there. Valid after [`Self::commit`].
    #[must_use]
    pub fn display(&self) -> &Layout<TextBrush> {
        let result = self.expect_result();
        result
            .display
            .as_ref()
            .map_or(&self.paragraph.natural, |part| &part.layout)
    }

    /// How many parley style indices this block's display layout has.
    ///
    /// The range [`Self::source_of`] is defined over, so a painter can build
    /// its per-run table without probing for the end.
    #[must_use]
    pub fn style_count(&self) -> usize {
        let result = self.expect_result();
        result
            .display
            .as_ref()
            .map_or(&self.paragraph.natural_sources, |part| &part.sources)
            .len()
    }

    /// Paint identity of one parley `style_index` from [`Self::display`].
    #[must_use]
    pub fn source_of(&self, style_index: u16) -> SourceItem {
        let result = self.expect_result();
        let sources = result
            .display
            .as_ref()
            .map_or(&self.paragraph.natural_sources, |part| &part.sources);
        sources[usize::from(style_index)]
    }

    /// The layout-event data: one entry per visible line.
    #[must_use]
    pub fn lines(&self) -> &[LineInfo] {
        &self.expect_result().lines
    }

    /// Where each reported line sits: one entry per [`Self::lines`] entry,
    /// parley's own placement.
    #[must_use]
    pub fn line_geometry(&self) -> &[LineGeometry] {
        &self.expect_result().geometry
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

    /// The last reported line's baseline, from the block's top; `None` when
    /// no line is reported. A line a clamp or a cut removed is not reported,
    /// so a clamped block's last baseline is its last visible line's.
    #[must_use]
    pub fn last_baseline(&self) -> Option<f32> {
        self.expect_result().lines.last().map(|line| line.baseline)
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
    /// the box sizes the last layout used. Valid after [`Self::commit`].
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

    /// Lays the block out at `constraint`, re-entering parley only when the
    /// retained result is not already the answer.
    ///
    /// Callers reach it through [`Self::probe`] or [`Self::commit`], which
    /// differ in what they are allowed to do with the memo.
    fn layout_at(&mut self, context: &mut TextContext, constraint: BlockConstraint) {
        if self.memo.holds(self.retained(), constraint) {
            return;
        }
        self.memo.begin_break();
        let width = constraint.max_advance();

        let natural_lines = self.paragraph.break_lines(
            context,
            RunBreaking::Uniform(self.style.breaking()),
            constraint,
            self.style.max_lines.map(NonZeroU32::get),
            BlockStyle::INDENT_KEYWORDS,
        );

        // Whether content remains past the committed lines is a unit-space
        // fact, never the breaker's: parley reports itself unfinished while
        // only a renderless final line is pending. What a `nowrap` verbatim
        // paragraph dropped after its first newline is not content that
        // remains: native removes it before laying the text out.
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
            Some(plan) => self.finish_truncated(context, &plan, &natural_lines, constraint),
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
}

/// The reported lines' placement as parley laid them out.
fn parley_geometry(lines: &[LineInfo]) -> Vec<LineGeometry> {
    lines
        .iter()
        .map(|line| LineGeometry {
            top: line.top,
            height: line.height,
            baseline: line.baseline,
        })
        .collect()
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
        // A blank line stands only where the natural line is the empty last
        // line a verbatim paragraph keeps; anywhere else it is a truncated
        // layout's, and the natural line supplies the metrics.
        let rendered = final_layout
            .get(index)
            .filter(|line| natural_line.is_empty() || !shape::is_blank_line(line));
        if rendered.is_none() {
            used_fallback = true;
        }
        let line = rendered
            .or_else(|| natural_fallback.and_then(|layout| layout.get(index)))
            .expect("every visible line exists in one of the layouts");
        // The one line-box definition every reader goes through, the inline
        // formatting context's included.
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

    // Parley places every box by its `vertical-align` (`position`). Only a
    // line holding boxes is walked for their positions, up to its last box.
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
        })];
        let tail_style = ahem_style();
        let tail = [InlineItem::Run(TextRunItem {
            text: "XX",
            style: &tail_style,
            white_space: WhiteSpaceCollapse::Collapse,
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

    #[test]
    fn tabs_reshape_only_when_a_break_moves_their_stop() {
        let mut context = text_context();
        let style = ahem_style();
        // Unbroken, the tab starts at 40 and is 20 wide; wrapped at 25, `c`
        // starts the second line and the tab at 10 is 10 wide.
        let items = [InlineItem::Run(TextRunItem {
            text: "ab c\td",
            style: &style,
            white_space: WhiteSpaceCollapse::Preserve,
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
