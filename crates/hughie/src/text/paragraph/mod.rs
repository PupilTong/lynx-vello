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
//! per line. The Lynx block reports parley's own line metrics there; the
//! inline formatting context reports the CSS 2.2 §10.8 line boxes, whose
//! baselines differ from parley's. Every reader of a laid-out paragraph —
//! paint, range boxes, hit points — positions a line through it.
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
pub(in crate::text) mod shape;
pub(in crate::text) mod style;
mod tab;

use core::ops::Range;

pub use content::{InlineBoxSpec, InlineItem, TextRunItem};
use content::{NormalizedBlock, WhiteSpaceProcessing, normalize};
pub use memo::{BlockConstraint, BlockMetrics};
use parley::{ContentWidths, InlineBox, InlineBoxKind, Layout};
use shape::{NaturalLine, ShapeSpan};
pub use style::{
    BreakStyle, Direction, IndentKeywords, LineHeight, OverflowWrap, RunStyle, TextAlign,
    TextIndent, TextWrap, VerticalAlign, WhiteSpaceCollapse, WordBreak,
};

use crate::style::TextBrush;
use crate::text::TextContext;

/// Where one laid-out line sits in its paragraph's block axis.
///
/// The one source of line placement every reader goes through. `shift` is
/// what a y coordinate parley positioned on this line — a glyph run's
/// baseline, a run's ascent and descent, the line's own extent — needs added
/// to land on this geometry: zero where the consumer reports parley's own
/// metrics, the distance from parley's baseline to the line box's otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineGeometry {
    /// The top of the line box, from the paragraph's top.
    pub top: f32,
    /// The line box's block size.
    pub height: f32,
    /// The line's alphabetic baseline, from the paragraph's top.
    pub baseline: f32,
    /// The offset from parley's placement of this line to this one.
    pub shift: f32,
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
    Run(RunStyle),
    Box(InlineBoxSpec),
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
    /// The extra letter-spacing each verbatim space byte with an advance of
    /// its own is shaped with, indexed like `block.space_runs`. Measured by
    /// [`Self::measure_space_runs`].
    pub(in crate::text) space_spacing: Vec<f32>,
}

impl Flow {
    pub(in crate::text) fn build(
        items: &[InlineItem<'_>],
        processing: WhiteSpaceProcessing,
    ) -> Self {
        let block = normalize(items, processing);
        let tabs = block.tabs.len();
        let space_runs = block.space_runs.len();
        Self {
            items: items
                .iter()
                .map(|item| match item {
                    InlineItem::Run(run) => OwnedItem::Run(run.style.clone()),
                    InlineItem::Box(spec) => OwnedItem::Box(*spec),
                })
                .collect(),
            block,
            tab_spacing: vec![0.0; tabs],
            tab_min: vec![0.0; tabs],
            space_spacing: vec![0.0; space_runs],
        }
    }

    /// Gives each verbatim space byte with an advance of its own that
    /// advance: a run's one space widened by `count - 1` times the advance
    /// one space of the run's style takes, letter-spacing included, and a
    /// space before a forced break narrowed to nothing.
    ///
    /// One shaping per distinct run item holding such a byte, when the flow
    /// is built: a flow without one shapes nothing here.
    pub(in crate::text) fn measure_space_runs<S: Copy>(
        &mut self,
        context: &mut TextContext,
        breaking: RunBreaking<'_>,
        source: fn(u32) -> S,
    ) {
        const SPACE: &str = " ";
        let mut measured: Vec<(u32, f32)> = Vec::new();
        let spacing = self
            .block
            .space_runs
            .iter()
            .map(|run| {
                let item = self.item_at(run.byte);
                let advance =
                    if let Some(&(_, advance)) = measured.iter().find(|(seen, _)| *seen == item) {
                        advance
                    } else {
                        let spans = [ShapeSpan {
                            bytes: 0..SPACE.len(),
                            style: self.run_style(item),
                            breaking: breaking.of(item),
                            source: source(item),
                            extra_spacing: 0.0,
                        }];
                        let mut probe =
                            shape::shape(context, SPACE, &spans, Vec::new(), &mut Vec::new());
                        probe.break_all_lines(None);
                        // A lone space is all trailing whitespace, which `width`
                        // leaves out.
                        let advance = probe.full_width();
                        measured.push((item, advance));
                        advance
                    };
                if run.before_break {
                    return -advance;
                }
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a space count is far below f32's exact integer range"
                )]
                let others = (run.count - 1) as f32;
                advance * others
            })
            .collect();
        self.space_spacing = spacing;
    }

    /// The run item holding byte `byte` of the normalized text.
    pub(in crate::text) fn item_at(&self, byte: u32) -> u32 {
        let ranges = &self.block.ranges;
        ranges[ranges.partition_point(|range| range.bytes.end <= byte)].item
    }

    pub(in crate::text) fn run_style(&self, item: u32) -> &RunStyle {
        match &self.items[item as usize] {
            OwnedItem::Run(style) => style,
            OwnedItem::Box(_) => unreachable!("styled ranges only reference run items"),
        }
    }

    pub(in crate::text) fn box_spec(&self, item: u32) -> &InlineBoxSpec {
        match &self.items[item as usize] {
            OwnedItem::Box(spec) => spec,
            OwnedItem::Run(_) => unreachable!("box slots only reference box items"),
        }
    }

    /// The shaper spans for this flow, offset into a combined display string.
    ///
    /// One span per styled range, except that every preserved tab's space and
    /// every verbatim space run's one space is a span of its own carrying its
    /// extra spacing — the only way to give one cluster an advance of its own
    /// in parley.
    pub(in crate::text) fn spans<S>(
        &self,
        byte_offset: u32,
        source: fn(u32) -> S,
        breaking: RunBreaking<'_>,
    ) -> Vec<ShapeSpan<'_, S>> {
        // Every one-byte span with an advance of its own, ascending by byte.
        let mut widened: Vec<(u32, f32)> = self
            .block
            .tabs
            .iter()
            .copied()
            .zip(self.tab_spacing.iter().copied())
            .chain(
                self.block
                    .space_runs
                    .iter()
                    .map(|run| run.byte)
                    .zip(self.space_spacing.iter().copied()),
            )
            .collect();
        widened.sort_unstable_by_key(|&(byte, _)| byte);
        let mut spans = Vec::with_capacity(self.block.ranges.len() + 2 * widened.len());
        let mut push = |bytes: core::ops::Range<u32>, item: u32, extra_spacing: f32| {
            spans.push(ShapeSpan {
                bytes: (bytes.start + byte_offset) as usize..(bytes.end + byte_offset) as usize,
                style: self.run_style(item),
                breaking: breaking.of(item),
                source: source(item),
                extra_spacing,
            });
        };
        let mut next = 0;
        for range in &self.block.ranges {
            let mut start = range.bytes.start;
            while let Some(&(byte, spacing)) = widened.get(next)
                && byte < range.bytes.end
            {
                if start < byte {
                    push(start..byte, range.item, 0.0);
                }
                push(byte..byte + 1, range.item, spacing);
                start = byte + 1;
                next += 1;
            }
            if start < range.bytes.end {
                push(start..range.bytes.end, range.item, 0.0);
            }
        }
        spans
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
}

/// What one natural break reports, in the content's own offset spaces.
pub(in crate::text) struct NaturalBreak {
    /// The committed lines, trailing blank lines dropped.
    pub(in crate::text) lines: Vec<NaturalLine>,
    /// The source unit of each atomic box slot, in slot order.
    pub(in crate::text) slot_units: Vec<u32>,
    /// Min- and max-content widths of the full content, read off the
    /// unjustified, pre-alignment layout with the current box sizes written
    /// in, so a justified alignment can never leak into them.
    pub(in crate::text) content_widths: ContentWidths,
}

impl<S: Copy> Paragraph<S> {
    /// Builds and shapes the content flow of `items`.
    ///
    /// `breaking` is the line-breaking style each item's spans are shaped
    /// with, `processing` how its white space is processed, and `source` the
    /// paint identity a content item's spans record.
    pub(in crate::text) fn new(
        context: &mut TextContext,
        breaking: RunBreaking<'_>,
        processing: WhiteSpaceProcessing,
        items: &[InlineItem<'_>],
        source: fn(u32) -> S,
    ) -> Self {
        let mut flow = Flow::build(items, processing);
        flow.measure_space_runs(context, breaking, source);
        let tab_stops = match processing {
            WhiteSpaceProcessing::Verbatim { .. } => tab::TabStops::Native,
            WhiteSpaceProcessing::Css | WhiteSpaceProcessing::CssKeepingLeadingSpace => {
                // Native stops take no minimum shift, so only css-text-3's
                // `0.5ch` is measured.
                flow.tab_min = tab_minimums(context, breaking, &flow, source);
                tab::TabStops::Css
            }
        };
        let spans = flow.spans(0, source, breaking);
        let boxes = parley_boxes(&flow, 0, 0);
        let mut natural_sources = Vec::new();
        let natural = shape::shape(
            context,
            &flow.block.text,
            &spans,
            boxes,
            &mut natural_sources,
        );
        drop(spans);
        Self {
            content: flow,
            natural,
            natural_sources,
            source,
            tab_stops,
        }
    }

    /// Breaks the natural layout at `constraint`, committing at most
    /// `max_lines` lines, and reads the committed lines back.
    ///
    /// The atomic boxes' current sizes and `vertical-align` line
    /// contributions are written in first, and the constraint's indent goes
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
    ) -> NaturalBreak {
        let width = constraint.max_advance();

        // Box widths and vertical-align line contributions feed the breaker.
        for (slot, entry) in self.natural.inline_boxes_mut().iter_mut().enumerate() {
            let spec = self.content.box_spec(self.content.block.boxes[slot].item);
            entry.width = spec.size.width;
            entry.height = position::line_contribution(spec);
        }

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
        let lines = shape::capture_lines(
            &self.natural,
            &self.content.block.text,
            &self.content.block.map,
            &slot_bytes,
            &slot_units,
        );
        NaturalBreak {
            lines,
            slot_units,
            content_widths,
        }
    }

    /// Shapes the natural layout again, at the content's current tab
    /// spacing and box sizes. Only the tab pass needs this: nothing else a
    /// layout changes is a shaping input.
    fn reshape(&mut self, context: &mut TextContext, breaking: RunBreaking<'_>) {
        let spans = self.content.spans(0, self.source, breaking);
        let boxes = parley_boxes(&self.content, 0, 0);
        self.natural = shape::shape(
            context,
            &self.content.block.text,
            &spans,
            boxes,
            &mut self.natural_sources,
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
                source: source(item),
                extra_spacing: -style.letter_spacing,
            }];
            let mut probe = shape::shape(context, ZERO, &spans, Vec::new(), &mut Vec::new());
            probe.break_all_lines(None);
            let half = probe.width() * 0.5;
            measured.push((item, half));
            half
        })
        .collect()
}

/// The parley boxes of one flow, sized from the current specs and carrying
/// their vertical-align line contribution as the height.
pub(in crate::text) fn parley_boxes(
    flow: &Flow,
    byte_offset: u32,
    base_slot: usize,
) -> Vec<InlineBox> {
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

pub(in crate::text) fn sized_box(spec: &InlineBoxSpec, slot: usize, index: usize) -> InlineBox {
    InlineBox {
        id: slot as u64,
        kind: InlineBoxKind::InFlow,
        index,
        width: spec.size.width,
        height: position::line_contribution(spec),
    }
}
