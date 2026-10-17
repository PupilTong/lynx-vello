//! The inline formatting context of a CSS block container whose content is
//! text runs only.
//!
//! A block container (CSS 2.2 §9.2.1, css-display-3 `block flow`) lays its
//! inline content out in line boxes. Here that content is text runs — no
//! inline-level box exists, because the `lynx` display grammar has none — so
//! the paragraph is the core's ([`crate::text::paragraph`]) with CSS
//! semantics on top, and nothing the Lynx `<text>` block adds is
//! representable: no line or length limit, no truncation, no custom
//! truncation content.
//!
//! What this module does that the core leaves to its consumer:
//!
//! - **White space** is css-text-3 §4.1 in full, including §4.1.2 rule 1 at the start of the
//!   paragraph: a collapsible sequence there is removed.
//! - **Line breaking** reads each run's own `text-wrap-mode`, `word-break` and `overflow-wrap`.
//!   Parley takes its line-break override once per paragraph, so a paragraph with any `break-all`
//!   run is analysed without it (`docs/tracking/deviations.md`).
//! - **`text-indent`** honours its `each-line` and `hanging` keywords and counts toward the
//!   min-content and max-content widths.
//! - **Line boxes** follow CSS 2.2 §10.8.1, as parley 0.12 builds them: every line starts with the
//!   root inline box's strut — the container's first available font at its `line-height`, parley's
//!   style 0 — and every run on it adds its own half-leading box around the shared baseline; with
//!   `line-height: normal` a run shaped in a fallback font adds that font's box too.
//! - **Direction** is the container's `direction`, handed to parley as the bidi paragraph level
//!   (UAX #9 P3 overridden, as css-writing-modes-3 §2.1 takes it from the block): `text-align:
//!   start` and the side trailing whitespace hangs on follow it, and a mixed line orders its runs
//!   by it.
//! - **Sizing** reports the CSS intrinsic widths, and for a definite available width the
//!   fit-content size `min(max-content, max(min-content, available))` (css-sizing-3) rather than
//!   the widest broken line.
//!
//! `text-overflow` is not applied: it takes effect only on a block whose
//! `overflow` is not `visible` (css-overflow-3 `text-overflow`), and the one producer of
//! this context — the anonymous block box around loose text — computes
//! `overflow: visible` and is unreachable by author CSS, so no value an
//! author writes can reach it. A non-anonymous block container would apply
//! it per overflowing line box, after the break.

use core::fmt;

use parley::{AlignmentOptions, BaseDirection, Layout};

use crate::geometry::Size;
use crate::style::{TextBrush, TextContainerStyle, TextRunStyle};
use crate::text::TextContext;
use crate::text::paragraph::content::WhiteSpaceProcessing;
use crate::text::paragraph::memo::{BreakMemo, Retained};
use crate::text::paragraph::style::resolve_alignment;
use crate::text::paragraph::{
    BlockConstraint, BlockMetrics, BreakStyle, Direction, IndentKeywords, InlineItem, LineGeometry,
    NaturalBreak, Paragraph, ParagraphRoot, RunBreaking, RunStyle, TextAlign, TextRunItem,
    WhiteSpaceCollapse,
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

/// What one layout produced.
struct FlowResult {
    constraint: BlockConstraint,
    lines: Vec<LineGeometry>,
    metrics: BlockMetrics,
}

/// The intrinsic widths at one indent.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Intrinsic {
    indent: f32,
    min: f32,
    max: f32,
}

/// A shaped, breakable inline formatting context of text runs.
pub struct InlineFlow {
    style: InlineFlowStyle,
    /// The content flow and its natural layout; a parley style index's paint
    /// identity is the index of the run it was shaped from, `None` for the
    /// strut's.
    paragraph: Paragraph<Option<u32>>,
    /// Each run's line-breaking values, indexed like the runs.
    breaking: Vec<BreakStyle>,
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
    /// Builds and shapes the block's content: `runs` in document order.
    #[must_use]
    pub fn new(context: &mut TextContext, style: InlineFlowStyle, runs: &[InlineRun<'_>]) -> Self {
        let items: Vec<InlineItem<'_>> = runs
            .iter()
            .map(|run| {
                InlineItem::Run(TextRunItem {
                    text: run.text,
                    style: run.style,
                    white_space: run.white_space,
                })
            })
            .collect();
        let breaking: Vec<BreakStyle> = runs.iter().map(|run| run.breaking).collect();
        let root = ParagraphRoot {
            strut: Some((style.strut.clone(), None)),
            direction: match style.direction {
                Direction::Ltr => BaseDirection::Ltr,
                Direction::Rtl => BaseDirection::Rtl,
            },
        };
        let paragraph = Paragraph::new(
            context,
            RunBreaking::PerItem(&breaking),
            WhiteSpaceProcessing::Css,
            &items,
            Some,
            root,
        );
        Self {
            style,
            paragraph,
            breaking,
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

    /// The run a parley `style_index` of [`Self::display`] was shaped from;
    /// `None` for the strut's, which no text is shaped in.
    #[must_use]
    pub fn source_of(&self, style_index: u16) -> Option<u32> {
        self.expect_result();
        self.paragraph.natural_sources[usize::from(style_index)]
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
            metrics,
        });
    }

    fn reported(&self) -> BlockMetrics {
        self.expect_result().metrics
    }
}
