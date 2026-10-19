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
//!
//! A consumer that lays text of its own into the content uses four generic
//! hooks, none of which the Lynx block needs: markers in the item stream,
//! whose places the normalizer reports (`content::MarkerAt`); segments it
//! splices into the normalized text, with no source unit and a style of
//! their own (`content::NormalizedBlock::splice`); a line-break override of
//! its own for the pairs those segments take part in
//! ([`shape::BreakOverride`]); and advance laid beside an atomic box
//! (`content::NormalizedBlock::box_advance`). It builds the flow
//! ([`Flow::build`]), uses them, then shapes ([`Paragraph::from_flow`]).
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
pub(in crate::text) mod position;
pub(in crate::text) mod query;
pub(in crate::text) mod shape;
pub(in crate::text) mod style;
mod tab;

use core::ops::Range;

pub use content::{InlineBoxSpec, InlineItem, TextRunItem};
use content::{NormalizedBlock, WhiteSpaceProcessing, normalize};
pub use memo::{BlockConstraint, BlockMetrics};
use parley::{
    ContentWidths, InlineBox, InlineBoxKind, Layout, WhiteSpaceCollapse as ParleyWhiteSpace,
};
pub use position::used_line_height;
use shape::{BreakOverride, NaturalLine, ShapeSpan};
pub use style::{
    BreakStyle, Direction, IndentKeywords, LineHeight, OverflowWrap, RunStyle, TextAlign,
    TextIndent, TextWrap, VerticalAlign, WhiteSpaceCollapse, WordBreak,
};

use crate::style::TextBrush;
use crate::text::TextContext;

/// Where one laid-out line sits in its paragraph's block axis: parley's own
/// line box, which is also where it placed the line's glyphs and boxes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineGeometry {
    /// The top of the line box, from the paragraph's top.
    pub top: f32,
    /// The line box's block size.
    pub height: f32,
    /// The line's alphabetic baseline, from the paragraph's top.
    pub baseline: f32,
}

impl LineGeometry {
    /// Parley's placement of `line`.
    pub(in crate::text) fn of(line: &parley::Line<'_, TextBrush>) -> Self {
        let metrics = line.metrics();
        Self {
            top: metrics.block_min_coord,
            height: metrics.block_max_coord - metrics.block_min_coord,
            baseline: metrics.baseline,
        }
    }
}

/// The line-breaking values each content item is shaped with.
#[derive(Debug, Clone, Copy)]
pub(in crate::text) enum RunBreaking<'a> {
    /// One value for every item: a paragraph whose values are
    /// container-wide.
    Uniform(BreakStyle),
    /// One value per item, indexed like the items the flow was built from.
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

/// One owned flattened item.
#[derive(Debug)]
pub(in crate::text) enum OwnedItem {
    Run {
        style: RunStyle,
        /// How the run's whitespace hangs at a line end and whether a wrap
        /// opportunity follows each space, in parley's vocabulary
        /// (`white_space_hanging`).
        hanging: ParleyWhiteSpace,
    },
    Box(InlineBoxSpec),
    /// A marker ([`InlineItem::Marker`]).
    Marker,
}

/// One owned flattened flow: the main content, or the truncation content.
#[derive(Debug)]
pub(in crate::text) struct Flow {
    pub(in crate::text) items: Vec<OwnedItem>,
    pub(in crate::text) block: NormalizedBlock,
    /// The extra letter-spacing each preserved tab's space is shaped with,
    /// indexed like `block.tabs`. Zero — one plain space — until the tab pass
    /// settles it (`tab::resolve`).
    pub(in crate::text) tab_spacing: Vec<f32>,
    /// Each preserved tab's `0.5ch`, the shortest shift it takes; indexed
    /// like `block.tabs`. Measured once, for the content flow only.
    pub(in crate::text) tab_min: Vec<f32>,
    /// The consumer's own line-break override, for the pairs its inserted
    /// segments take part in ([`content::NormalizedBlock::splice`]). `None`
    /// shapes with the paragraph's alone.
    pub(in crate::text) break_override: Option<BreakOverride>,
}

impl Flow {
    pub(in crate::text) fn build(
        items: &[InlineItem<'_>],
        processing: WhiteSpaceProcessing,
    ) -> Self {
        let block = normalize(items, processing);
        let tabs = block.tabs.len();
        Self {
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
            break_override: None,
        }
    }

    /// The run item holding byte `byte` of the normalized text.
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

    /// How run item `item`'s whitespace hangs and breaks, in parley's
    /// vocabulary.
    pub(in crate::text) fn hanging(&self, item: u32) -> ParleyWhiteSpace {
        match &self.items[item as usize] {
            OwnedItem::Run { hanging, .. } => *hanging,
            _ => unreachable!("styled ranges only reference run items"),
        }
    }

    /// The shaper spans for this flow, offset into a combined display string.
    ///
    /// One span per styled range, except that every preserved tab's space is
    /// a span of its own carrying its tab spacing — the only way to give one
    /// cluster an advance of its own in parley — and every inserted segment
    /// is one in its own style, with its extra spacing and the paint identity
    /// of its item.
    pub(in crate::text) fn spans<S>(
        &self,
        byte_offset: u32,
        source: fn(u32) -> S,
        breaking: RunBreaking<'_>,
    ) -> Vec<ShapeSpan<'_, S>> {
        let tabs = &self.block.tabs;
        let inserted = &self.block.inserted;
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
            // Inserted segments sit between styled ranges, never inside one.
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

/// How a run's whitespace hangs at a line end, and whether a wrap
/// opportunity follows each space, under `processing`.
///
/// The normalizer has already collapsed or kept every character, so parley
/// reads this only for hanging and for `break-spaces`' opportunities
/// (`parley::WhiteSpaceCollapse` on a `StyleRunBuilder`). A CSS paragraph
/// hands each run its own value: collapsible spaces always hang, preserved
/// ones hang when the line wraps (conditionally before a forced break, CSS
/// Text 4 §4.3.2), `break-spaces` ones never hang. A verbatim paragraph
/// hangs every space at a line end, unconditionally — native Lynx's line
/// extent leaves trailing whitespace out (Android `Layout.getLineMax`)
/// whether or not the line wraps and whatever follows — which is parley's
/// `Collapse` hanging.
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

/// One paragraph's content flow, shaped once into its natural layout.
///
/// `S` is the consumer's paint identity for a parley style index; the
/// natural layout records [`Self::source`] of each content item.
pub(in crate::text) struct Paragraph<S> {
    pub(in crate::text) content: Flow,
    /// The content shaped as one paragraph, broken at the constraint the last
    /// [`Self::break_lines`] was given.
    pub(in crate::text) natural: Layout<TextBrush>,
    /// The natural layout's style-index → paint-identity table.
    pub(in crate::text) natural_sources: Vec<S>,
    /// The paint identity of content item `n`.
    source: fn(u32) -> S,
    /// Where the content's preserved tabs stop: css-text-3's stops, or native
    /// Lynx's for a paragraph whose white space is processed verbatim.
    tab_stops: tab::TabStops,
    /// The root inline box every line starts with.
    pub(in crate::text) root: ParagraphRoot<S>,
    /// What a `vertical-align` percentage of an atomic box is a fraction
    /// of: the root inline box's line height (`position::percent_basis`).
    pub(in crate::text) percent_basis: f32,
}

/// The root inline box of a paragraph, owned: its strut style with the paint
/// identity parley's style 0 records, and the base direction
/// ([`shape::LineRoot`]).
#[derive(Debug, Clone)]
pub(in crate::text) struct ParagraphRoot<S> {
    pub(in crate::text) strut: Option<(RunStyle, S)>,
    pub(in crate::text) direction: parley::BaseDirection,
}

impl<S: Copy> ParagraphRoot<S> {
    pub(in crate::text) fn line_root(&self) -> shape::LineRoot<'_, S> {
        shape::LineRoot {
            strut: self.strut.as_ref().map(|(style, source)| (style, *source)),
            direction: self.direction,
        }
    }
}

impl<S: Copy> Paragraph<S> {
    /// Builds and shapes the content flow of `items`.
    ///
    /// `breaking` is the line-breaking style each item's spans are shaped
    /// with, `processing` how its white space is processed, `source` the
    /// paint identity a content item's spans record, and `root` the root
    /// inline box every line starts with.
    pub(in crate::text) fn new(
        context: &mut TextContext,
        breaking: RunBreaking<'_>,
        processing: WhiteSpaceProcessing,
        items: &[InlineItem<'_>],
        source: fn(u32) -> S,
        root: ParagraphRoot<S>,
    ) -> Self {
        let flow = Flow::build(items, processing);
        Self::from_flow(context, breaking, processing, flow, source, root)
    }

    /// Shapes `flow`, a flow [`Flow::build`] built with `processing`, once a
    /// consumer has used the hooks between building and shaping: its
    /// markers' places, the segments it splices in, its line-break override,
    /// and the advance it lays beside atomic boxes. The rest is as
    /// [`Self::new`].
    pub(in crate::text) fn from_flow(
        context: &mut TextContext,
        breaking: RunBreaking<'_>,
        processing: WhiteSpaceProcessing,
        mut flow: Flow,
        source: fn(u32) -> S,
        root: ParagraphRoot<S>,
    ) -> Self {
        let tab_stops = match processing {
            WhiteSpaceProcessing::Verbatim { .. } => tab::TabStops::Native,
            WhiteSpaceProcessing::Css => {
                // Native stops take no minimum shift, so only css-text-3's
                // `0.5ch` is measured.
                flow.tab_min = tab_minimums(context, breaking, &flow, source);
                tab::TabStops::Css
            }
        };
        let spans = flow.spans(0, source, breaking);
        // Box sizes are written in before every break; at construction the
        // percentage basis is not known yet, and nothing reads the boxes.
        let boxes = parley_boxes(&flow, 0, 0, 0.0);
        let mut natural_sources = Vec::new();
        let natural = shape::shape_overriding(
            context,
            &flow.block.text,
            &spans,
            boxes,
            &mut natural_sources,
            root.line_root(),
            flow.break_override,
        );
        drop(spans);
        // With no strut the first run stands in for the root, as it does as
        // parley's style 0.
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

    /// Breaks the natural layout at `constraint`, committing at most
    /// `max_lines` lines, and reads the committed lines back in the
    /// content's own offset spaces, trailing blank lines dropped.
    ///
    /// The atomic boxes' current sizes, baselines and `vertical-align` values
    /// are written in first, and the constraint's indent goes
    /// on the lines `keywords` name. Shaping is not re-run, except that a
    /// paragraph with preserved tabs re-shapes when a break moved a tab to a
    /// different stop (`tab`), at most `tab::MAX_ROUNDS` times; `breaking` is
    /// what such a re-shape uses.
    pub(in crate::text) fn break_lines(
        &mut self,
        context: &mut TextContext,
        breaking: RunBreaking<'_>,
        constraint: BlockConstraint,
        max_lines: Option<u32>,
        keywords: IndentKeywords,
    ) -> Vec<NaturalLine> {
        let width = constraint.max_advance();

        // Box sizes, baselines and alignments feed the breaker, which sizes
        // and places the line boxes around them.
        self.write_boxes();

        let indent = constraint.indent;
        let mut tab_rounds = 0;
        loop {
            self.natural.set_text_indent(indent, keywords.options());
            shape::break_clamped(&mut self.natural, width, max_lines);
            if tab_rounds == tab::MAX_ROUNDS {
                break;
            }
            // Empty-checked first thing: a paragraph without preserved tabs
            // leaves here on its first break.
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
        shape::capture_lines(
            &self.natural,
            &self.content.block.text,
            &self.content.block.map,
            &self.content.block.boxes,
        )
    }

    /// Writes the atomic boxes' current sizes, baselines and alignments into
    /// the natural layout, each widened by the advance laid beside it.
    fn write_boxes(&mut self) {
        for (slot, entry) in self.natural.inline_boxes_mut().enumerate() {
            let spec = self.content.box_spec(self.content.block.boxes[slot].item);
            let sized = sized_box(spec, slot, entry.index, self.percent_basis);
            let (before, after) = self.content.block.box_advance(slot);
            entry.width = sized.width + before + after;
            entry.height = sized.height;
            entry.baseline = sized.baseline;
            entry.vertical_align = sized.vertical_align;
        }
    }

    /// The min- and max-content widths of the full content at `indent`, on
    /// the lines `keywords` name, at the atomic boxes' current sizes and the
    /// tab stops the last break settled: [`Self::content_widths`] with no
    /// break. The committed lines are left as they were, while the boxes
    /// and the indent written in are this call's.
    pub(in crate::text) fn content_widths_at(
        &mut self,
        indent: f32,
        keywords: IndentKeywords,
    ) -> ContentWidths {
        self.write_boxes();
        self.natural.set_text_indent(indent, keywords.options());
        self.content_widths()
    }

    /// The width of every atomic box as the next break writes it in.
    pub(in crate::text) fn box_widths(&self) -> impl Iterator<Item = f32> + '_ {
        self.content
            .block
            .boxes
            .iter()
            .enumerate()
            .map(|(slot, entry)| {
                let (before, after) = self.content.block.box_advance(slot);
                self.content.box_spec(entry.item).size.width + before + after
            })
    }

    /// Frees the line storage an earlier break left in the natural layout
    /// beyond what its current lines hold.
    ///
    /// Parley keeps a layout's line and line item buffers across breaks and
    /// never shrinks them, so after a break at a narrow width they hold
    /// about one line per word for as long as the paragraph lives. A copy of
    /// the layout holds exactly its current contents.
    pub(in crate::text) fn release_line_capacity(&mut self) {
        self.natural = self.natural.clone();
    }

    /// The min- and max-content widths of the full content at the box sizes,
    /// indent and tab stops the last [`Self::break_lines`] wrote in.
    ///
    /// Read on demand rather than with every break: parley measures them
    /// from the shaped content, the inline boxes and the indent alone, never
    /// from the committed lines or an alignment, so the widths are the same
    /// whenever after that break they are asked for, and a justified
    /// alignment does not change them.
    pub(in crate::text) fn content_widths(&self) -> ContentWidths {
        self.natural.calculate_content_widths()
    }

    /// Shapes the natural layout again, at the content's current tab
    /// spacing and box sizes. Only the tab pass needs this: nothing else a
    /// layout changes is a shaping input.
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
            self.content.break_override,
        );
    }
}

impl<S> Paragraph<S> {
    /// The source units content item `item` contributed: a run's whole
    /// pre-collapse text, an atomic box's one unit. Fixed when the paragraph
    /// is built.
    ///
    /// `None` when `item` is past the content items.
    pub(in crate::text) fn item_source_range(&self, item: u32) -> Option<Range<u32>> {
        self.content
            .block
            .item_units
            .get(usize::try_from(item).ok()?)
            .cloned()
    }
}

/// Each preserved tab's `0.5ch` — half the advance of `0` in the tab's own
/// run, without its letter-spacing (`ch` is a font metric).
///
/// One shaping per distinct run item holding a tab, at construction: `ch`
/// never changes without a rebuild, and a paragraph without preserved tabs
/// shapes nothing here.
fn tab_minimums<S: Copy>(
    context: &mut TextContext,
    breaking: RunBreaking<'_>,
    flow: &Flow,
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

/// The parley boxes of one flow, sized and aligned from the current specs.
pub(in crate::text) fn parley_boxes(
    flow: &Flow,
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
            let (before, after) = flow.block.box_advance(slot);
            sized.width += before + after;
            sized
        })
        .collect()
}

/// One atomic box as parley lays it out: its margin box, its baseline from
/// the box's top edge (`None` puts the bottom edge on the baseline), and its
/// `vertical-align`, which parley 0.12 resolves against the root inline box.
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
