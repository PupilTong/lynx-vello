//! The break memo: which constraints a retained paragraph has already
//! answered, and which one its committed lines were broken at.
//!
//! A container asks one paragraph several constraints per pass —
//! max-content, min-content, a trial width, the used width — and only the
//! committed one is what paint reads. [`BreakMemo`] keeps the last
//! [`MEASURED_BREAKS`] answers so a repeated question is answered without
//! re-entering parley, and records the committed constraint so a probe that
//! moved the retained layout can be undone. The consumer owns the layout and
//! the result; this owns only the bookkeeping, so every paragraph consumer
//! runs the same probe/commit protocol over it.

use crate::geometry::Size;
use crate::text::TextContext;

/// Everything a block's line breaking depends on.
///
/// Two `layout_at` calls at the same constraint produce the same lines, which
/// is what makes the memo below sound. `max_advance` is stored the way the breaker itself stores
/// it, as `f32::INFINITY` for max-content, so a remembered constraint costs one
/// `f32` pair rather than an `Option`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockConstraint {
    max_advance: f32,
    pub(in crate::text) indent: f32,
}

impl BlockConstraint {
    /// `max_advance` of `None` is max-content. `indent` is the **resolved**
    /// `text-indent` in px: a percentage's basis is the definite inline size,
    /// which only the caller knows, so the block never resolves one itself.
    #[must_use]
    pub fn new(max_advance: Option<f32>, indent: f32) -> Self {
        Self {
            max_advance: max_advance.map_or(f32::INFINITY, |value| value.max(0.0)),
            indent,
        }
    }

    pub(in crate::text) fn max_advance(self) -> Option<f32> {
        self.max_advance.is_finite().then_some(self.max_advance)
    }

    /// Whether a result laid out at `self` is also the answer at
    /// `constraint`: the same constraint, or — for a max-content result
    /// whose lines fit every width from `fits_from` on
    /// ([`Retained::fits_from`]) — a definite width at least that wide at
    /// the same indent, where the breaker takes no break max-content did not.
    #[expect(
        clippy::float_cmp,
        reason = "the same indent is the exact equality the derived `PartialEq` compares"
    )]
    pub(in crate::text) fn answers(self, fits_from: Option<f32>, constraint: Self) -> bool {
        self == constraint
            || (self.max_advance == f32::INFINITY
                && self.indent == constraint.indent
                && fits_from.is_some_and(|width| width <= constraint.max_advance))
    }
}

/// What one constraint reported, so a repeat answers without touching parley.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockMetrics {
    pub size: Size<f32>,
    /// The first line box's baseline, from the paragraph's top; `None`
    /// without a line box.
    pub first_baseline: Option<f32>,
    /// The last line box's baseline, from the paragraph's top; `None`
    /// without a line box. For a Lynx `<text>`, the last *reported* line's:
    /// a line a clamp or a cut removed is no line box.
    pub last_baseline: Option<f32>,
}

/// How many already-answered constraints one block remembers.
///
/// Three, for the reason `MEASURED_BREAKS` is three on the measurement path:
/// containers cycle a leaf through max-content, min-content and its used
/// width, so anything smaller thrashes and anything larger only pays for
/// itself on a constraint set no algorithm here produces.
pub(in crate::text) const MEASURED_BREAKS: u8 = 3;

#[derive(Debug, Clone, Copy)]
struct MeasuredBreak {
    constraint: BlockConstraint,
    metrics: BlockMetrics,
    /// [`Retained::fits_from`] of the result that answered: the narrowest
    /// definite width this max-content answer is also the answer at.
    fits_from: Option<f32>,
}

/// The memo and commit state of one retained paragraph.
///
/// `retained` arguments name the constraint the consumer's current result was
/// laid out at, `None` when it holds no result.
#[derive(Debug)]
pub(in crate::text) struct BreakMemo {
    /// Constraints already answered, so a repeat reports without re-breaking
    /// and — the part that matters — without moving the retained lines away
    /// from the committed ones.
    measured: [Option<MeasuredBreak>; MEASURED_BREAKS as usize],
    next_measured: u8,
    /// The constraint the last commit left. `None` until one has happened,
    /// which is what [`Self::committed`] reports and what stops a reader
    /// seeing a paragraph no commit ever produced.
    committed: Option<BlockConstraint>,
    /// Whether an atomic box was resized since the retained layout was
    /// broken, which makes that layout stale at every constraint.
    boxes_dirty: bool,
    /// `content_widths.min` from a layout that had box sizes written in,
    /// never from construction (where no atom has a size yet), with the
    /// `text-indent` that layout was broken at: the indent counts toward the
    /// width on the lines it applies to, so it keys the answer.
    min_content: Option<MinContent>,
    /// How many times parley has actually broken these lines. The memo is
    /// only worth its state if a caller can see it working.
    breaks: u32,
}

impl BreakMemo {
    pub(in crate::text) const fn new() -> Self {
        Self {
            measured: [None; MEASURED_BREAKS as usize],
            next_measured: 0,
            committed: None,
            boxes_dirty: false,
            min_content: None,
            breaks: 0,
        }
    }

    /// The answer `constraint` already got, if the memo still holds it: its
    /// own, or a max-content answer whose lines fit it
    /// ([`BlockConstraint::answers`]).
    pub(in crate::text) fn remembered(&self, constraint: BlockConstraint) -> Option<BlockMetrics> {
        let entries = self.measured.iter().flatten();
        entries
            .clone()
            .find(|entry| entry.constraint == constraint)
            .or_else(|| {
                entries
                    .clone()
                    .find(|entry| entry.constraint.answers(entry.fits_from, constraint))
            })
            .map(|entry| entry.metrics)
    }

    /// Records `metrics` as `constraint`'s answer and hands them back.
    /// `fits_from` is the answering result's [`Retained::fits_from`].
    pub(in crate::text) fn remember(
        &mut self,
        constraint: BlockConstraint,
        metrics: BlockMetrics,
        fits_from: Option<f32>,
    ) -> BlockMetrics {
        let entry = MeasuredBreak {
            constraint,
            metrics,
            fits_from,
        };
        // Re-recording a constraint the memo already holds — a commit landing
        // on the width a probe asked about — overwrites that entry rather than
        // consuming a fresh slot, which would evict a constraint the pass is
        // still going to ask for.
        if let Some(existing) = self
            .measured
            .iter_mut()
            .flatten()
            .find(|held| held.constraint == constraint)
        {
            *existing = entry;
            return metrics;
        }
        self.measured[usize::from(self.next_measured)] = Some(entry);
        self.next_measured = (self.next_measured + 1) % MEASURED_BREAKS;
        metrics
    }

    /// Drops every remembered answer and the min-content width, which a new
    /// paragraph parameter or a resized atom makes stale. The committed
    /// constraint survives: it names where the retained layout must be put
    /// back.
    pub(in crate::text) fn forget(&mut self) {
        self.measured = [None; MEASURED_BREAKS as usize];
        self.next_measured = 0;
        self.min_content = None;
    }

    /// Records that an atomic box changed size after the retained layout was
    /// broken.
    pub(in crate::text) const fn mark_boxes_dirty(&mut self) {
        self.boxes_dirty = true;
    }

    pub(in crate::text) const fn boxes_dirty(&self) -> bool {
        self.boxes_dirty
    }

    /// Whether the consumer's result at `retained` is already the answer for
    /// `constraint`, so laying out again would change nothing: laid out at
    /// it, or a max-content result whose lines fit it and sit where a break
    /// at it would put them, for which the consumer passes `fits_from`
    /// ([`BlockConstraint::answers`]).
    pub(in crate::text) fn holds(
        &self,
        retained: Option<BlockConstraint>,
        fits_from: Option<f32>,
        constraint: BlockConstraint,
    ) -> bool {
        !self.boxes_dirty
            && retained.is_some_and(|retained| retained.answers(fits_from, constraint))
    }

    /// Marks the start of one real break: the retained layout is about to be
    /// recomputed with the current box sizes.
    pub(in crate::text) const fn begin_break(&mut self) {
        self.boxes_dirty = false;
        self.breaks = self.breaks.saturating_add(1);
    }

    pub(in crate::text) const fn breaks(&self) -> u32 {
        self.breaks
    }

    pub(in crate::text) const fn committed(&self) -> Option<BlockConstraint> {
        self.committed
    }

    pub(in crate::text) const fn set_committed(&mut self, constraint: BlockConstraint) {
        self.committed = Some(constraint);
    }

    /// Whether a probe left the retained result somewhere the committed
    /// constraint did not put it. `false` until a commit has happened.
    pub(in crate::text) fn is_probe_dirty(&self, retained: Option<BlockConstraint>) -> bool {
        let Some(committed) = self.committed else {
            return false;
        };
        self.boxes_dirty || retained.is_none_or(|retained| retained != committed)
    }

    /// The min-content width remembered for `indent`, if any.
    pub(in crate::text) fn min_content(&self, indent: f32) -> Option<f32> {
        self.min_content
            .filter(|entry| entry.indent.to_bits() == indent.to_bits())
            .map(|entry| entry.width)
    }

    pub(in crate::text) const fn set_min_content(&mut self, indent: f32, width: f32) {
        self.min_content = Some(MinContent { indent, width });
    }
}

/// One remembered min-content width and the indent it was measured at.
#[derive(Debug, Clone, Copy)]
struct MinContent {
    indent: f32,
    width: f32,
}

/// The probe/commit protocol over a [`BreakMemo`], written once for every
/// retained paragraph.
///
/// A consumer supplies its memo, the constraint its current result was laid
/// out at, the layout itself and the metrics a result reports; the protocol
/// — when the memo answers, when parley is re-entered, what a commit records
/// and how a probe is undone — is these provided methods. Each consumer's
/// public `probe`, `commit`, `is_probe_dirty` and `restore_committed` call
/// them by their trait path, so the two paragraph kinds cannot drift.
pub(in crate::text) trait Retained {
    fn memo(&self) -> &BreakMemo;

    fn memo_mut(&mut self) -> &mut BreakMemo;

    /// The constraint the current result was laid out at, `None` without
    /// one.
    fn retained(&self) -> Option<BlockConstraint>;

    /// Lays the paragraph out at `constraint`, re-entering parley only when
    /// the retained result is not already the answer.
    fn layout_at(&mut self, context: &mut TextContext, constraint: BlockConstraint);

    /// What the current result reports.
    fn reported(&self) -> BlockMetrics;

    /// For an untruncated max-content result, its natural layout's own
    /// width; `None` for any other result.
    ///
    /// Every max-content line ends at a forced break or the content's end
    /// and is at most that wide, so a break at a definite width at least
    /// that wide, at the same indent, takes the same breaks: the result is
    /// that width's answer too ([`BlockConstraint::answers`]). A truncated
    /// result is not, because its cut can depend on the width: the
    /// inline-truncation content shows only where it is narrower than the
    /// width, and a `text-maxlength` cut leaves a display narrower than the
    /// natural lines a narrower width re-breaks.
    ///
    /// Where parley's breaker would still wrap at exactly that width — its
    /// fit test rounding a non-zero `text-indent` differently from its
    /// width, or a cluster with a negative advance — the max-content lines
    /// stand. The user ruled that change accepted (2026-10-22).
    fn fits_from(&self) -> Option<f32>;

    /// Answers `constraint` from the memo when it can, and from a layout
    /// otherwise, recording the answer.
    fn probe(&mut self, context: &mut TextContext, constraint: BlockConstraint) -> BlockMetrics {
        if let Some(remembered) = self.memo().remembered(constraint) {
            return remembered;
        }
        self.layout_at(context, constraint);
        self.remember(constraint)
    }

    /// Lays the paragraph out for real and records `constraint` as the one
    /// paint reads.
    fn commit(&mut self, context: &mut TextContext, constraint: BlockConstraint) -> BlockMetrics {
        self.layout_at(context, constraint);
        let metrics = self.remember(constraint);
        self.memo_mut().set_committed(constraint);
        metrics
    }

    fn remember(&mut self, constraint: BlockConstraint) -> BlockMetrics {
        let metrics = self.reported();
        let fits_from = self.fits_from();
        self.memo_mut().remember(constraint, metrics, fits_from)
    }

    fn is_probe_dirty(&self) -> bool {
        self.memo().is_probe_dirty(self.retained())
    }

    /// Lays the paragraph out at the committed constraint again when a probe
    /// moved it. Answers whether parley was re-entered.
    fn restore_committed(&mut self, context: &mut TextContext) -> bool {
        let Some(committed) = self.memo().committed() else {
            return false;
        };
        if !Retained::is_probe_dirty(self) {
            return false;
        }
        self.layout_at(context, committed);
        true
    }
}
