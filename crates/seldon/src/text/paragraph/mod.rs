//! The shared parley paragraph core: one shaped, breakable paragraph of
//! styled text runs and atomic inline boxes, with no consumer's own
//! semantics.
//!
//! Every paragraph this crate lays out goes through these modules:
//!
//! - `content` collapses whitespace across the flattened items and maps normalized UTF-8 bytes to
//!   source UTF-16 units and back.
//! - `style` holds the run-level value vocabulary and its translation into parley styles.
//! - `shape` builds the parley layout, breaks it, and reads the committed lines back into source
//!   space, dropping the blank trailing lines parley commits.
//! - `tab` widens each preserved tab to its tab stop at break time.
//! - `position` places atomic boxes by `vertical-align`.
//! - `memo` remembers the constraints a paragraph has already answered and the one it committed.
//! - `extent` walks a laid-out line cluster by cluster in source units.
//! - `query` answers source-range boxes and hit points over a laid-out paragraph, for both
//!   consumers.
//! - `offsets` is the read-only bridge between a laid-out paragraph's layout bytes and its source
//!   units ([`OffsetMap`]), for a host that addresses the layout in the offsets of its own text.
//!
//! A consumer that lays text of its own into the content uses four generic
//! hooks, none of which the Lynx block needs: markers in the item stream,
//! whose places the normalizer reports (`content::MarkerPlaces`); segments it
//! splices into the normalized text, with no source unit and a style of
//! their own (`content::NormalizedBlock::splice`); a line-break override of
//! its own for the pairs those segments take part in
//! ([`shape::BreakOverride`]); and advance laid beside an atomic box. It
//! builds the flow ([`Flow::build`]), uses them, keeps what the core reads
//! again at every shaping and break in the flow's [`Additions`], then shapes
//! ([`Paragraph::from_flow`]). The Lynx block's flows are `Flow<()>`, which
//! hold none of it.
//!
//! `Paragraph` owns one shaped natural layout over one flow of items and
//! re-breaks it in place: a new constraint, or a resized atomic box, never
//! re-shapes, except where a break moves a preserved tab to a different stop.
//! What a consumer does with the broken lines — alignment, truncation, line
//! reports, box placement — is the consumer's. There are two consumers: the
//! Lynx `<text>` block ([`crate::text::block`]) and the CSS block container's
//! inline formatting context ([`crate::text::inline`]).
//!
//! Where a consumer places its lines in the block axis is a [`LineGeometry`]
//! per line: parley's own line metrics, which since parley 0.12 are CSS 2.2
//! §10.8.1 line boxes — every line starts with the root inline box's strut
//! ([`ParagraphRoot`]), and every span and atomic box adds its own box around
//! the shared baseline. Every reader of a laid-out paragraph — paint, range
//! boxes, hit points — positions a line through it.
//!
//! White-space processing is decided in one place: `content::normalize`,
//! called only from `Flow::build` with the consumer's
//! `content::WhiteSpaceProcessing`. Everything after it reads only the
//! `NormalizedBlock` it returns — the normalized text, the styled ranges, the
//! box positions, the list of tabs to widen, and the source map — so a
//! consumer that processes white space differently needs a different
//! processing value behind that one call, not a different shaping or
//! breaking path.

pub(in crate::text) mod content;
mod extent;
pub(in crate::text) mod memo;
mod offsets;
pub(in crate::text) mod position;
pub(in crate::text) mod query;
pub(in crate::text) mod shape;
pub(in crate::text) mod style;
mod tab;

use core::ops::Range;

pub use content::{InlineBoxSpec, InlineItem, TextRunItem};
use content::{InsertedSegment, MarkerPlaces, NormalizedBlock, WhiteSpaceProcessing, normalize};
pub use memo::{BlockConstraint, BlockMetrics};
pub use offsets::OffsetMap;
use parley::{
    Alignment, AlignmentOptions, ContentWidths, InlineBox, InlineBoxKind, Layout,
    WhiteSpaceCollapse as ParleyWhiteSpace,
};
pub use position::used_line_height;
use shape::{BreakOverride, NaturalLine, ShapeSpan};
pub use style::{
    BreakStyle, Direction, IndentKeywords, LineHeight, OverflowWrap, RunStyle, TextAlign,
    TextIndent, TextSecurity, TextWrap, VerticalAlign, WhiteSpaceCollapse, WordBreak,
};

use crate::style::TextBrush;
use crate::text::TextContext;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineGeometry {
    pub top: f32,
    pub height: f32,
    pub baseline: f32,
}

impl LineGeometry {
    pub(in crate::text) fn of(line: &parley::Line<'_, TextBrush>) -> Self {
        let metrics = line.metrics();
        Self {
            top: metrics.block_min_coord,
            height: metrics.block_max_coord - metrics.block_min_coord,
            baseline: metrics.baseline,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::text) enum RunBreaking<'a> {
    Uniform(BreakStyle),
    PerItem(&'a [BreakStyle]),
}

impl RunBreaking<'_> {
    pub(in crate::text) fn of(self, item: u32) -> BreakStyle {
        match self {
            Self::Uniform(breaking) => breaking,
            Self::PerItem(values) => values[item as usize],
        }
    }
}

#[derive(Debug)]
pub(in crate::text) enum OwnedItem {
    Run {
        style: RunStyle,
        hanging: ParleyWhiteSpace,
    },
    Box(InlineBoxSpec),
    Marker,
}

pub(in crate::text) trait Additions: Default {
    fn inserted(&self) -> &[InsertedSegment];

    fn box_advance(&self, slot: usize) -> (f32, f32);

    fn break_override(&self) -> Option<BreakOverride>;
}

impl Additions for () {
    fn inserted(&self) -> &[InsertedSegment] {
        &[]
    }

    fn box_advance(&self, _slot: usize) -> (f32, f32) {
        (0.0, 0.0)
    }

    fn break_override(&self) -> Option<BreakOverride> {
        None
    }
}

#[derive(Debug)]
pub(in crate::text) struct Flow<A = ()> {
    pub(in crate::text) items: Vec<OwnedItem>,
    pub(in crate::text) block: NormalizedBlock,
    pub(in crate::text) tab_spacing: Vec<f32>,
    pub(in crate::text) tab_min: Vec<f32>,
    pub(in crate::text) additions: A,
}

impl<A: Additions> Flow<A> {
    pub(in crate::text) fn build(
        items: &[InlineItem<'_>],
        processing: WhiteSpaceProcessing,
    ) -> (Self, MarkerPlaces) {
        let (block, places) = normalize(items, processing);
        let tabs = block.tabs.len();
        let flow = Self {
            items: items
                .iter()
                .map(|item| match item {
                    InlineItem::Run(run) => OwnedItem::Run {
                        style: run.style.clone(),
                        hanging: white_space_hanging(processing, run.white_space),
                    },
                    InlineItem::Box(spec) => OwnedItem::Box(*spec),
                    InlineItem::Marker => OwnedItem::Marker,
                })
                .collect(),
            block,
            tab_spacing: vec![0.0; tabs],
            tab_min: vec![0.0; tabs],
            additions: A::default(),
        };
        (flow, places)
    }

    pub(in crate::text) fn item_at(&self, byte: u32) -> u32 {
        let ranges = &self.block.ranges;
        ranges[ranges.partition_point(|range| range.bytes.end <= byte)].item
    }

    pub(in crate::text) fn run_style(&self, item: u32) -> &RunStyle {
        match &self.items[item as usize] {
            OwnedItem::Run { style, .. } => style,
            _ => unreachable!("styled ranges only reference run items"),
        }
    }

    pub(in crate::text) fn box_spec(&self, item: u32) -> &InlineBoxSpec {
        match &self.items[item as usize] {
            OwnedItem::Box(spec) => spec,
            _ => unreachable!("box slots only reference box items"),
        }
    }

    pub(in crate::text) fn hanging(&self, item: u32) -> ParleyWhiteSpace {
        match &self.items[item as usize] {
            OwnedItem::Run { hanging, .. } => *hanging,
            _ => unreachable!("styled ranges only reference run items"),
        }
    }

    pub(in crate::text) fn spans<S>(
        &self,
        byte_offset: u32,
        source: fn(u32) -> S,
        breaking: RunBreaking<'_>,
    ) -> Vec<ShapeSpan<'_, S>> {
        let tabs = &self.block.tabs;
        let inserted = self.additions.inserted();
        let mut spans =
            Vec::with_capacity(self.block.ranges.len() + 2 * tabs.len() + inserted.len());
        let shifted = |bytes: &core::ops::Range<u32>| {
            (bytes.start + byte_offset) as usize..(bytes.end + byte_offset) as usize
        };
        let inserted_span = |index: usize| {
            let segment = &inserted[index];
            ShapeSpan {
                bytes: shifted(&segment.bytes),
                style: &segment.style,
                breaking: breaking.of(segment.item),
                hanging: ParleyWhiteSpace::Collapse,
                source: source(segment.item),
                extra_spacing: segment.extra_spacing,
            }
        };
        let mut next_inserted = 0;
        let run_span = |bytes: core::ops::Range<u32>, item: u32, extra_spacing: f32| ShapeSpan {
            bytes: shifted(&bytes),
            style: self.run_style(item),
            breaking: breaking.of(item),
            hanging: self.hanging(item),
            source: source(item),
            extra_spacing,
        };
        let mut next_tab = 0;
        for range in &self.block.ranges {
            while let Some(segment) = inserted.get(next_inserted)
                && segment.bytes.start < range.bytes.start
            {
                spans.push(inserted_span(next_inserted));
                next_inserted += 1;
            }
            let mut start = range.bytes.start;
            while let Some(&tab) = tabs.get(next_tab)
                && tab < range.bytes.end
            {
                if start < tab {
                    spans.push(run_span(start..tab, range.item, 0.0));
                }
                spans.push(run_span(
                    tab..tab + 1,
                    range.item,
                    self.tab_spacing[next_tab],
                ));
                start = tab + 1;
                next_tab += 1;
            }
            if start < range.bytes.end {
                spans.push(run_span(start..range.bytes.end, range.item, 0.0));
            }
        }
        spans.extend((next_inserted..inserted.len()).map(inserted_span));
        spans
    }
}

fn white_space_hanging(
    processing: WhiteSpaceProcessing,
    white_space: WhiteSpaceCollapse,
) -> ParleyWhiteSpace {
    match processing {
        WhiteSpaceProcessing::Verbatim { .. } => ParleyWhiteSpace::Collapse,
        WhiteSpaceProcessing::Css => match white_space {
            WhiteSpaceCollapse::Collapse => ParleyWhiteSpace::Collapse,
            WhiteSpaceCollapse::PreserveBreaks => ParleyWhiteSpace::PreserveBreaks,
            WhiteSpaceCollapse::Preserve => ParleyWhiteSpace::Preserve,
            WhiteSpaceCollapse::BreakSpaces => ParleyWhiteSpace::BreakSpaces,
        },
    }
}

pub(in crate::text) struct Paragraph<S, A = ()> {
    pub(in crate::text) content: Flow<A>,
    pub(in crate::text) natural: Layout<TextBrush>,
    pub(in crate::text) natural_sources: Vec<S>,
    source: fn(u32) -> S,
    tab_stops: tab::TabStops,
    pub(in crate::text) root: ParagraphRoot<S>,
    pub(in crate::text) percent_basis: f32,
}

#[derive(Debug, Clone)]
pub(in crate::text) struct ParagraphRoot<S> {
    pub(in crate::text) strut: Option<(RunStyle, S)>,
    pub(in crate::text) direction: parley::BaseDirection,
    pub(in crate::text) strut_line_when_empty: bool,
}

impl<S: Copy> ParagraphRoot<S> {
    pub(in crate::text) fn line_root(&self) -> shape::LineRoot<'_, S> {
        shape::LineRoot {
            strut: self.strut.as_ref().map(|(style, source)| (style, *source)),
            direction: self.direction,
            strut_line_when_empty: self.strut_line_when_empty,
        }
    }
}

impl<S: Copy, A: Additions> Paragraph<S, A> {
    pub(in crate::text) fn new(
        context: &mut TextContext,
        breaking: RunBreaking<'_>,
        processing: WhiteSpaceProcessing,
        items: &[InlineItem<'_>],
        source: fn(u32) -> S,
        root: ParagraphRoot<S>,
    ) -> Self {
        let (flow, _) = Flow::build(items, processing);
        Self::from_flow(context, breaking, processing, flow, source, root)
    }

    pub(in crate::text) fn from_flow(
        context: &mut TextContext,
        breaking: RunBreaking<'_>,
        processing: WhiteSpaceProcessing,
        mut flow: Flow<A>,
        source: fn(u32) -> S,
        root: ParagraphRoot<S>,
    ) -> Self {
        let tab_stops = match processing {
            WhiteSpaceProcessing::Verbatim { .. } => tab::TabStops::Native,
            WhiteSpaceProcessing::Css => {
                flow.tab_min = tab_minimums(context, breaking, &flow, source);
                tab::TabStops::Css
            }
        };
        let spans = flow.spans(0, source, breaking);
        let boxes = parley_boxes(&flow, 0, 0, 0.0);
        let mut natural_sources = Vec::new();
        let natural = shape::shape_overriding(
            context,
            &flow.block.text,
            &spans,
            boxes,
            &mut natural_sources,
            root.line_root(),
            flow.additions.break_override(),
        );
        drop(spans);
        let basis_style = root.strut.as_ref().map(|(style, _)| style).or_else(|| {
            flow.items.iter().find_map(|item| match item {
                OwnedItem::Run { style, .. } => Some(style),
                _ => None,
            })
        });
        let percent_basis = position::percent_basis(&natural, basis_style);
        Self {
            content: flow,
            natural,
            natural_sources,
            source,
            tab_stops,
            root,
            percent_basis,
        }
    }

    pub(in crate::text) fn break_lines(
        &mut self,
        context: &mut TextContext,
        breaking: RunBreaking<'_>,
        constraint: BlockConstraint,
        max_lines: Option<u32>,
        keywords: IndentKeywords,
        alignment: Alignment,
    ) -> Vec<NaturalLine> {
        let width = constraint.max_advance();

        self.write_boxes();

        let indent = constraint.indent;
        let deferred = alignment == Alignment::Justify && !self.content.block.tabs.is_empty();
        let rounds_alignment = if deferred {
            Alignment::Start
        } else {
            alignment
        };
        let mut tab_rounds = 0;
        loop {
            self.natural.set_text_indent(indent, keywords.options());
            self.natural
                .set_alignment(rounds_alignment, AlignmentOptions::default());
            shape::break_clamped(&mut self.natural, width, max_lines);
            if tab_rounds == tab::MAX_ROUNDS {
                break;
            }
            let Some(spacing) = tab::resolve(
                &self.natural,
                &self.content.block.tabs,
                &self.content.tab_spacing,
                &self.content.tab_min,
                indent,
                keywords,
                self.tab_stops,
            ) else {
                break;
            };
            self.content.tab_spacing = spacing;
            self.reshape(context, breaking);
            tab_rounds += 1;
        }
        if deferred {
            self.natural
                .set_alignment(alignment, AlignmentOptions::default());
            shape::break_clamped(&mut self.natural, width, max_lines);
        }
        if self.is_empty_strut_line() {
            debug_assert_eq!(
                self.natural.len(),
                1,
                "parley breaks empty text into one line"
            );
            return vec![NaturalLine {
                start_unit: 0,
                end_unit: 0,
                consumed_end: 0,
                start_byte: 0,
                end_byte: 0,
            }];
        }
        shape::capture_lines(
            &self.natural,
            &self.content.block.text,
            &self.content.block.map,
            &self.content.block.boxes,
        )
    }

    pub(in crate::text) fn is_empty_strut_line(&self) -> bool {
        self.root.line_root().shapes_empty_content()
            && self.content.block.text.is_empty()
            && self.content.block.boxes.is_empty()
    }

    pub(in crate::text) fn realign(
        &mut self,
        alignment: Alignment,
        width: Option<f32>,
        max_lines: Option<u32>,
    ) {
        if self.natural.alignment() == Some(alignment) {
            return;
        }
        self.natural
            .set_alignment(alignment, AlignmentOptions::default());
        shape::break_clamped(&mut self.natural, width, max_lines);
    }

    fn write_boxes(&mut self) {
        let count = self.natural.inline_boxes().len();
        for slot in 0..count {
            let spec = self.content.box_spec(self.content.block.boxes[slot].item);
            let (before, after) = self.content.additions.box_advance(slot);
            let percent_basis = self.percent_basis;
            self.natural.edit_inline_box(slot, |entry| {
                let sized = sized_box(spec, slot, entry.index, percent_basis);
                entry.width = sized.width + before + after;
                entry.height = sized.height;
                entry.baseline = sized.baseline;
                entry.vertical_align = sized.vertical_align;
            });
        }
    }

    pub(in crate::text) fn content_widths_at(
        &mut self,
        indent: f32,
        keywords: IndentKeywords,
    ) -> ContentWidths {
        self.write_boxes();
        self.natural.set_text_indent(indent, keywords.options());
        self.content_widths()
    }

    pub(in crate::text) fn box_widths(&self) -> impl Iterator<Item = f32> + '_ {
        self.content
            .block
            .boxes
            .iter()
            .enumerate()
            .map(|(slot, entry)| {
                let (before, after) = self.content.additions.box_advance(slot);
                self.content.box_spec(entry.item).size.width + before + after
            })
    }

    pub(in crate::text) fn release_line_capacity(&mut self) {
        self.natural = self.natural.clone();
    }

    pub(in crate::text) fn content_widths(&self) -> ContentWidths {
        if self.is_empty_strut_line() {
            return ContentWidths { min: 0.0, max: 0.0 };
        }
        self.natural.calculate_content_widths()
    }

    fn reshape(&mut self, context: &mut TextContext, breaking: RunBreaking<'_>) {
        let spans = self.content.spans(0, self.source, breaking);
        let boxes = parley_boxes(&self.content, 0, 0, self.percent_basis);
        self.natural = shape::shape_overriding(
            context,
            &self.content.block.text,
            &spans,
            boxes,
            &mut self.natural_sources,
            self.root.line_root(),
            self.content.additions.break_override(),
        );
    }
}

impl<S, A> Paragraph<S, A> {
    pub(in crate::text) fn item_source_range(&self, item: u32) -> Option<Range<u32>> {
        self.content
            .block
            .item_units
            .get(usize::try_from(item).ok()?)
            .cloned()
    }
}

fn tab_minimums<S: Copy, A: Additions>(
    context: &mut TextContext,
    breaking: RunBreaking<'_>,
    flow: &Flow<A>,
    source: fn(u32) -> S,
) -> Vec<f32> {
    const ZERO: &str = "0";
    let mut measured: Vec<(u32, f32)> = Vec::new();
    flow.block
        .tabs
        .iter()
        .map(|&byte| {
            let item = flow.item_at(byte);
            if let Some(&(_, half)) = measured.iter().find(|(seen, _)| *seen == item) {
                return half;
            }
            let style = flow.run_style(item);
            let spans = [ShapeSpan {
                bytes: 0..ZERO.len(),
                style,
                breaking: breaking.of(item),
                hanging: parley::WhiteSpaceCollapse::Collapse,
                source: source(item),
                extra_spacing: -style.letter_spacing,
            }];
            let half = shape::probe_width(context, ZERO, &spans, Vec::new()) * 0.5;
            measured.push((item, half));
            half
        })
        .collect()
}

pub(in crate::text) fn parley_boxes<A: Additions>(
    flow: &Flow<A>,
    byte_offset: u32,
    base_slot: usize,
    percent_basis: f32,
) -> Vec<InlineBox> {
    flow.block
        .boxes
        .iter()
        .enumerate()
        .map(|(slot, entry)| {
            let spec = flow.box_spec(entry.item);
            let mut sized = sized_box(
                spec,
                base_slot + slot,
                (entry.byte + byte_offset) as usize,
                percent_basis,
            );
            let (before, after) = flow.additions.box_advance(slot);
            sized.width += before + after;
            sized
        })
        .collect()
}

pub(in crate::text) fn sized_box(
    spec: &InlineBoxSpec,
    slot: usize,
    index: usize,
    percent_basis: f32,
) -> InlineBox {
    InlineBox {
        id: slot as u64,
        kind: InlineBoxKind::InFlow,
        index,
        width: spec.size.width,
        height: spec.size.height,
        baseline: spec.baseline,
        vertical_align: position::parley_vertical_align(spec.vertical_align, percent_basis),
    }
}
