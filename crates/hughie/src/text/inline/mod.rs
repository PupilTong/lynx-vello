//! The inline formatting context of a CSS block container.
//!
//! A block container (CSS 2.2 §9.2.1, css-display-3 `block flow`) lays its
//! inline-level content out in line boxes (CSS 2.2 §9.4.2). That content is
//! text runs and non-atomic inline boxes around them (§9.2.2), so the
//! paragraph is the core's ([`crate::text::paragraph`]) with CSS semantics on
//! top, and nothing the Lynx `<text>` block adds is representable: no line or
//! length limit, no truncation, no custom truncation content.
//!
//! What this module does that the core leaves to its consumer:
//!
//! - **White space** is css-text-3 §4.1 in full, including §4.1.2 rule 1 at the start of the
//!   paragraph: a collapsible sequence there is removed. It collapses across inline box boundaries
//!   as across none (§4.1.1).
//! - **Line breaking** reads each run's own `text-wrap-mode`, `word-break` and `overflow-wrap`.
//!   Parley takes its line-break override once per paragraph, so a paragraph with any `break-all`
//!   run is analysed without it (`docs/tracking/deviations.md`).
//! - **`text-indent`** honours its `each-line` and `hanging` keywords and counts toward the
//!   min-content and max-content widths.
//! - **Line boxes** follow CSS 2.2 §10.8.1, as parley 0.12 builds them: every line starts with the
//!   root inline box's strut — the container's first available font at its `line-height`, parley's
//!   style 0 — and every run on it adds its own half-leading box around the shared baseline; with
//!   `line-height: normal` a run shaped in a fallback font adds that font's box too.
//! - **Inline boxes** ([`InlineFlowItem::Open`]) are the full inline box model of CSS 2.2 §10.3.1
//!   and §10.6.1 with `box-decoration-break: slice`: the inline-start margin, border and padding
//!   are advance on the line before the box's first content and the inline-end ones after its last,
//!   never a soft wrap opportunity of their own (`edge`); vertical margins do nothing, and vertical
//!   padding and border are painted without changing the line box. A box with no content still
//!   takes part in the line when an inline-axis margin, border or padding of it is nonzero
//!   (§9.4.2). Each box reports one fragment per line it is on ([`InlineFragment`]): its content
//!   area — its own font's ascent and descent — grown by its vertical padding and border, from its
//!   first to its last content on that line plus the edges it owns there.
//! - **Direction** is the container's `direction`, handed to parley as the bidi paragraph level
//!   (UAX #9 P3 overridden, as css-writing-modes-3 §2.1 takes it from the block): `text-align:
//!   start` and the side trailing whitespace hangs on follow it, and a mixed line orders its runs
//!   by it.
//! - **Sizing** reports the CSS intrinsic widths, and for a definite available width the
//!   fit-content size `min(max-content, max(min-content, available))` (css-sizing-3) rather than
//!   the widest broken line.
//!
//! `text-overflow` is not applied: it takes effect only on a block whose
//! `overflow` is not `visible` (css-overflow-3 `text-overflow`), and the
//! producers of this context — the anonymous block box around loose text,
//! and the block containers of HTML content the engine builds — compute
//! `overflow: visible` or are unreachable by author CSS. A block container
//! that clipped would apply it per overflowing line box, after the break.
//!
//! Not implemented, recorded in `docs/tracking/deviations.md`:
//! `vertical-align` on a non-atomic inline box, and the line-height of an
//! inline box whose own font has no text on a line.

mod boxes;
mod edge;
mod fragment;

use core::fmt;
use core::ops::Range;
use std::sync::OnceLock;

pub use boxes::InlineEdges;
use boxes::{BoxSpec, InlineBoxes};
use parley::{AlignmentOptions, BaseDirection, Layout};

use crate::geometry::{Point, Rect, Size};
use crate::style::{TextBrush, TextContainerStyle, TextRunStyle};
use crate::text::TextContext;
use crate::text::paragraph::content::WhiteSpaceProcessing;
use crate::text::paragraph::memo::{BreakMemo, Retained};
use crate::text::paragraph::query::{Displayed, LineSpan};
use crate::text::paragraph::shape::NaturalLine;
use crate::text::paragraph::style::resolve_alignment;
use crate::text::paragraph::{
    BlockConstraint, BlockMetrics, BreakStyle, Direction, Flow, IndentKeywords, InlineItem,
    LineGeometry, NaturalBreak, OwnedItem, Paragraph, ParagraphRoot, RunBreaking, RunStyle,
    TextAlign, TextRunItem, WhiteSpaceCollapse,
};

/// The block container's own parameters: CSS properties only.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InlineFlowStyle {
    pub text_align: TextAlign,
    /// The paragraph's bidi base level, which also resolves `text-align:
    /// start`/`end`.
    pub direction: Direction,
    /// The block container's own font and `line-height`: the strut every
    /// line box starts with (CSS 2.2 §10.8.1). Its font fields and
    /// `line_height` are read; its `letter_spacing` is not.
    pub strut: RunStyle,
    /// The `text-indent` keywords. The indent's length is a
    /// [`BlockConstraint`] input, because a percentage resolves against the
    /// definite inline size only the caller knows.
    pub indent: IndentKeywords,
}

impl InlineFlowStyle {
    /// Builds the parameters from the block container's computed style:
    /// `container` for its text properties, `own_font` for the font and
    /// `line-height` of the strut — the same computed values, seen as a run.
    #[must_use]
    pub fn from_container_style<C: TextContainerStyle, R: TextRunStyle>(
        container: &C,
        own_font: &R,
    ) -> Self {
        let indent = container.text_indent();
        Self {
            text_align: container.text_align().into(),
            direction: container.direction().into(),
            strut: RunStyle::from_run_style(own_font),
            indent: IndentKeywords {
                each_line: indent.each_line,
                hanging: indent.hanging,
            },
        }
    }
}

/// One text run of the block's content, carrying the computed style of the
/// element its characters are in.
#[derive(Clone, Copy, Debug)]
pub struct InlineRun<'src> {
    pub text: &'src str,
    pub style: &'src RunStyle,
    /// The run's `white-space-collapse`.
    pub white_space: WhiteSpaceCollapse,
    /// The run's `word-break`, `text-wrap-mode` and `overflow-wrap`.
    pub breaking: BreakStyle,
}

/// One item of the block's inline content, in document order.
#[derive(Clone, Copy, Debug)]
pub enum InlineFlowItem<'src> {
    Run(InlineRun<'src>),
    /// The start of a non-atomic inline box: every item up to the matching
    /// [`Self::Close`] is its content.
    Open(InlineBoxOpen<'src>),
    /// The end of the innermost open inline box.
    Close,
}

impl<'src> From<InlineRun<'src>> for InlineFlowItem<'src> {
    fn from(run: InlineRun<'src>) -> Self {
        Self::Run(run)
    }
}

/// A non-atomic inline box's own parameters.
#[derive(Clone, Copy, Debug)]
pub struct InlineBoxOpen<'src> {
    /// The box's own font and `line-height`: the content area its fragments
    /// cover, and the font its edges are shaped in.
    pub style: &'src RunStyle,
    /// The box's own line-breaking values, which its edges are shaped with.
    pub breaking: BreakStyle,
    /// The box's `direction`, which says which physical side its start edge
    /// is on.
    pub direction: Direction,
    /// Its used margin, border and padding.
    pub edges: InlineEdges,
}

/// What a parley style index of [`InlineFlow::display`] was shaped from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowSource {
    /// The strut, which no text is shaped in.
    Strut,
    /// A text run: its index in the items the flow was built from.
    Run(u32),
    /// An edge cluster: no source text and nothing to draw, the inline-axis
    /// margin, border and padding of an inline box, named by its `Open`
    /// item.
    Edge(u32),
}

/// One line's piece of a non-atomic inline box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InlineFragment {
    /// The box's `Open` item.
    pub item: u32,
    pub line: u32,
    /// The fragment's border box, in the paragraph's coordinates: its
    /// content area grown by its vertical padding and border, from its first
    /// to its last content on the line, plus the edges it owns there.
    pub rect: Rect<f32>,
    /// Whether this is the box's first fragment, which holds its start edge,
    /// and its last, which holds its end edge (`box-decoration-break:
    /// slice`).
    pub first: bool,
    pub last: bool,
}

/// What one layout produced.
struct FlowResult {
    constraint: BlockConstraint,
    lines: Vec<LineGeometry>,
    /// The same lines in source space.
    natural: Vec<NaturalLine>,
    metrics: BlockMetrics,
    /// The inline boxes' fragments, worked out on first read: a probe needs
    /// none of them.
    fragments: OnceLock<Vec<InlineFragment>>,
}

/// The intrinsic widths at one indent.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Intrinsic {
    indent: f32,
    min: f32,
    max: f32,
}

/// A shaped, breakable inline formatting context.
pub struct InlineFlow {
    style: InlineFlowStyle,
    /// The content flow and its natural layout; a parley style index's paint
    /// identity is the index of the item it was shaped from — a run, or an
    /// inline box's `Open` for an edge cluster — `None` for the strut's.
    paragraph: Paragraph<Option<u32>>,
    /// Each item's line-breaking values, indexed like the items.
    breaking: Vec<BreakStyle>,
    /// The non-atomic inline boxes: their parameters, their places in the
    /// shaped text and their edge clusters.
    boxes: InlineBoxes,
    result: Option<FlowResult>,
    memo: BreakMemo,
    /// The intrinsic widths at the indent last asked about. Independent of
    /// the constraint, so one entry serves every width a pass asks at.
    intrinsic: Option<Intrinsic>,
}

impl fmt::Debug for InlineFlow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InlineFlow").finish_non_exhaustive()
    }
}

impl InlineFlow {
    /// Builds and shapes the block's content: `items` in document order.
    #[must_use]
    pub fn new(
        context: &mut TextContext,
        style: InlineFlowStyle,
        items: &[InlineFlowItem<'_>],
    ) -> Self {
        let breaking: Vec<BreakStyle> = items
            .iter()
            .map(|item| match item {
                InlineFlowItem::Run(run) => run.breaking,
                InlineFlowItem::Open(open) => open.breaking,
                InlineFlowItem::Close => BreakStyle::default(),
            })
            .collect();
        let (flow, mut boxes) = lay_in(items);
        let root = ParagraphRoot {
            strut: Some((style.strut.clone(), None)),
            direction: match style.direction {
                Direction::Ltr => BaseDirection::Ltr,
                Direction::Rtl => BaseDirection::Rtl,
            },
        };
        let paragraph = Paragraph::from_flow(
            context,
            RunBreaking::PerItem(&breaking),
            WhiteSpaceProcessing::Css,
            flow,
            Some,
            root,
        );
        boxes.areas = boxes::content_areas(context, &boxes.specs);
        Self {
            style,
            paragraph,
            breaking,
            boxes,
            result: None,
            memo: BreakMemo::new(),
            intrinsic: None,
        }
    }

    /// Replaces the parameters without re-shaping.
    ///
    /// Sound only while the strut and the direction are unchanged: both are
    /// shaping inputs — the strut is parley's style 0 and the direction the
    /// bidi paragraph level — so a change to either needs a new flow, which
    /// the host decides because it holds the old style to compare. Alignment
    /// and the indent keywords are applied at break time.
    pub fn set_style(&mut self, style: InlineFlowStyle) {
        if self.style == style {
            return;
        }
        debug_assert!(
            self.style.strut == style.strut && self.style.direction == style.direction,
            "a strut or direction change needs a new flow"
        );
        self.style = style;
        self.memo.forget();
        self.result = None;
        self.intrinsic = None;
    }

    /// Reports `constraint`, answering from the memo when the same
    /// constraint was already asked, without moving the committed lines.
    pub fn probe(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        Retained::probe(self, context, constraint)
    }

    /// Lays the flow out for real: a commit's lines are the ones paint reads.
    pub fn commit(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        Retained::commit(self, context, constraint)
    }

    /// Whether a commit has ever produced this flow's retained result.
    #[must_use]
    pub const fn has_committed(&self) -> bool {
        self.memo.committed().is_some()
    }

    /// Whether a probe left the retained result somewhere the committed
    /// constraint did not put it.
    #[must_use]
    pub fn is_probe_dirty(&self) -> bool {
        Retained::is_probe_dirty(self)
    }

    /// Puts the retained layout back where the last commit left it. Answers
    /// whether parley was re-entered.
    pub fn restore_committed(&mut self, context: &mut TextContext) -> bool {
        Retained::restore_committed(self, context)
    }

    /// The min-content width at `indent` (css-sizing-3): the widest
    /// line when every soft wrap opportunity is taken, the indent counted on
    /// the lines it applies to.
    ///
    /// May move the retained layout; a host restores it as after a probe.
    pub fn min_content_width(&mut self, context: &mut TextContext, indent: f32) -> f32 {
        self.intrinsic(context, indent).min
    }

    /// The max-content width at `indent`: the widest line when only forced
    /// breaks are taken, the indent counted on the lines it applies to.
    ///
    /// May move the retained layout; a host restores it as after a probe.
    pub fn max_content_width(&mut self, context: &mut TextContext, indent: f32) -> f32 {
        self.intrinsic(context, indent).max
    }

    /// How many times parley has broken these lines. Tests assert on it;
    /// nothing else should.
    #[must_use]
    pub const fn break_count(&self) -> u32 {
        self.memo.breaks()
    }

    /// The laid-out paragraph: glyph runs placed on the CSS line boxes
    /// [`Self::line_geometry`] reports.
    #[must_use]
    pub fn display(&self) -> &Layout<TextBrush> {
        self.expect_result();
        &self.paragraph.natural
    }

    /// The CSS line boxes, one per laid-out line.
    #[must_use]
    pub fn line_geometry(&self) -> &[LineGeometry] {
        &self.expect_result().lines
    }

    /// The reported size: the fit-content width and the stacked line boxes'
    /// height. No line box, no height.
    #[must_use]
    pub fn size(&self) -> Size<f32> {
        self.expect_result().metrics.size
    }

    /// The first line box's baseline; `None` without a line box.
    #[must_use]
    pub fn first_baseline(&self) -> Option<f32> {
        self.expect_result().metrics.first_baseline
    }

    /// The last line box's baseline; `None` without a line box.
    #[must_use]
    pub fn last_baseline(&self) -> Option<f32> {
        self.expect_result().metrics.last_baseline
    }

    /// How many parley style indices the layout has: the range
    /// [`Self::source_of`] is defined over.
    #[must_use]
    pub fn style_count(&self) -> usize {
        self.expect_result();
        self.paragraph.natural_sources.len()
    }

    /// What a parley `style_index` of [`Self::display`] was shaped from.
    #[must_use]
    pub fn source_of(&self, style_index: u16) -> FlowSource {
        self.expect_result();
        self.source_at(style_index)
    }

    fn source_at(&self, style_index: u16) -> FlowSource {
        match self.paragraph.natural_sources[usize::from(style_index)] {
            None => FlowSource::Strut,
            // A marker's paint identity names an edge cluster: the core
            // shapes no marker, only the segments laid in for one.
            Some(item) => match self.paragraph.content.items[item as usize] {
                OwnedItem::Marker => FlowSource::Edge(item),
                _ => FlowSource::Run(item),
            },
        }
    }

    /// Every non-atomic inline box's fragments, by box in tree order and,
    /// within a box, by line. A box with no content and no nonzero edge has
    /// none.
    #[must_use]
    pub fn fragments(&self) -> &[InlineFragment] {
        let result = self.expect_result();
        result.fragments.get_or_init(|| self.compute_fragments())
    }

    /// The source units item `item` contributed: a run's whole pre-collapse
    /// text, an inline box boundary none. Fixed when the flow is built.
    ///
    /// `None` when `item` is past the items the flow was built from.
    #[must_use]
    pub fn item_source_range(&self, item: u32) -> Option<Range<u32>> {
        // The core holds a marker more per box the items left open.
        if item as usize >= self.breaking.len() {
            return None;
        }
        self.paragraph.item_source_range(item)
    }

    /// The boxes the source range `[start, end)` occupies in the committed
    /// layout, in the paragraph's own coordinates: the contract of
    /// [`crate::text::block::TextBlock::range_boxes`], over this paragraph's
    /// lines. An inline box's edges are no source text and select nothing.
    #[must_use]
    pub fn range_boxes(&self, start: u32, end: u32) -> Option<Vec<Rect<f32>>> {
        let lines = self.line_spans();
        self.paragraph
            .range_boxes_in(&self.displayed(&lines), start, end)
    }

    /// The source unit under `point`, in the paragraph's own coordinates:
    /// the contract of [`crate::text::block::TextBlock::index_at_point`]. A
    /// point on an inline box's edges is on no content.
    #[must_use]
    pub fn index_at_point(&self, point: Point<f32>) -> Option<u32> {
        let lines = self.line_spans();
        self.paragraph
            .index_at_point_in(&self.displayed(&lines), point)
    }

    fn line_spans(&self) -> Vec<LineSpan> {
        self.expect_result()
            .natural
            .iter()
            .map(|line| LineSpan {
                start: line.start_unit,
                end: line.end_unit,
            })
            .collect()
    }

    fn displayed<'a>(&'a self, lines: &'a [LineSpan]) -> Displayed<'a> {
        let block = &self.paragraph.content.block;
        Displayed {
            layout: &self.paragraph.natural,
            lines,
            geometry: &self.expect_result().lines,
            cut_byte: block.text.len(),
            content_slots: block.boxes.len(),
        }
    }

    fn expect_result(&self) -> &FlowResult {
        self.result
            .as_ref()
            .expect("an InlineFlow is read after a layout")
    }

    /// Breaks the natural layout at `constraint`, counting the break.
    fn break_at(&mut self, context: &mut TextContext, constraint: BlockConstraint) -> NaturalBreak {
        self.memo.begin_break();
        self.paragraph.break_lines(
            context,
            RunBreaking::PerItem(&self.breaking),
            constraint,
            None,
            self.style.indent,
        )
    }

    /// The intrinsic widths at `indent`, from two breaks of the natural
    /// layout: unconstrained for max-content, at zero for min-content.
    ///
    /// Both are read as parley's widest line, trailing whitespace excluded
    /// and the indent added on the lines it applies to, which is what makes
    /// the indent count. At zero every soft wrap opportunity is taken; an
    /// arbitrary break `overflow-wrap: break-word` allows would also be
    /// taken there, and contributes nothing to min-content (css-text-3
    /// §5.5), so parley's own minimum — which honours that — is the floor.
    fn intrinsic(&mut self, context: &mut TextContext, indent: f32) -> Intrinsic {
        if let Some(intrinsic) = self.intrinsic
            && intrinsic.indent.to_bits() == indent.to_bits()
        {
            return intrinsic;
        }
        self.break_at(context, BlockConstraint::new(None, indent));
        let unconstrained = self.paragraph.natural.width();
        let narrowest = self.break_at(context, BlockConstraint::new(Some(0.0), indent));
        let min = narrowest
            .content_widths
            .min
            .max(self.paragraph.natural.width());
        // A hanging indent can make the narrowest layout wider than the
        // unconstrained one; max-content is never below min-content.
        let intrinsic = Intrinsic {
            indent,
            min,
            max: unconstrained.max(min),
        };
        // The natural layout is now broken at zero, which no result names.
        self.result = None;
        self.intrinsic = Some(intrinsic);
        intrinsic
    }
}

/// Builds the content flow of `items` and lays their inline boxes' edges
/// into it, unshaped.
///
/// The paragraph core sees every `Open` and every `Close` as a marker, and
/// one marker more at the end of the content for each box the items leave
/// open, which closes it there. Its places after white space collapsing
/// decide where each edge goes (`edge::place`).
fn lay_in(items: &[InlineFlowItem<'_>]) -> (Flow, InlineBoxes) {
    let mut specs = Vec::new();
    let mut open = 0_usize;
    let mut core: Vec<InlineItem<'_>> = items
        .iter()
        .map(|item| match item {
            InlineFlowItem::Run(run) => InlineItem::Run(TextRunItem {
                text: run.text,
                style: run.style,
                white_space: run.white_space,
            }),
            InlineFlowItem::Open(spec) => {
                specs.push(BoxSpec::new(spec.style, spec.direction, spec.edges));
                open += 1;
                InlineItem::Marker
            }
            InlineFlowItem::Close => {
                open = open.saturating_sub(1);
                InlineItem::Marker
            }
        })
        .collect();
    core.extend(core::iter::repeat_n(InlineItem::Marker, open));
    let mut flow = Flow::build(&core, WhiteSpaceProcessing::Css);
    let boxes = edge::place(&mut flow.block, items, specs);
    if !boxes.edges.is_empty() {
        flow.break_override = Some(edge::BREAK_OVERRIDE);
    }
    (flow, boxes)
}

impl Retained for InlineFlow {
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
        if self.memo.holds(self.retained(), constraint) {
            return;
        }
        let intrinsic = self.intrinsic(context, constraint.indent);
        let natural = self.break_at(context, constraint);
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        self.paragraph
            .natural
            .align(alignment, AlignmentOptions::default());
        // Parley's lines are the CSS line boxes; the blank trailing lines it
        // commits (`capture_lines`) are none of them.
        let lines: Vec<LineGeometry> = self
            .paragraph
            .natural
            .lines()
            .take(natural.lines.len())
            .map(|line| LineGeometry::of(&line))
            .collect();
        // css-sizing-3's fit-content size: the available width clamped between
        // the intrinsic widths. Max-content is the unconstrained answer.
        let width = constraint.max_advance().map_or(intrinsic.max, |available| {
            intrinsic.max.min(intrinsic.min.max(available))
        });
        let height = lines.last().map_or(0.0, |line| line.top + line.height);
        let metrics = BlockMetrics {
            size: Size::new(width, height),
            first_baseline: lines.first().map(|line| line.baseline),
            last_baseline: lines.last().map(|line| line.baseline),
        };
        self.result = Some(FlowResult {
            constraint,
            lines,
            natural: natural.lines,
            metrics,
            fragments: OnceLock::new(),
        });
    }

    fn reported(&self) -> BlockMetrics {
        self.expect_result().metrics
    }
}
