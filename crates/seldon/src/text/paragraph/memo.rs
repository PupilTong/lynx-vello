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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockConstraint {
    max_advance: f32,
    pub(in crate::text) indent: f32,
}

impl BlockConstraint {
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockMetrics {
    pub size: Size<f32>,
    pub first_baseline: Option<f32>,
    pub last_baseline: Option<f32>,
}

pub(in crate::text) const MEASURED_BREAKS: u8 = 3;

#[derive(Debug, Clone, Copy)]
struct MeasuredBreak {
    constraint: BlockConstraint,
    metrics: BlockMetrics,
    fits_from: Option<f32>,
}

#[derive(Debug)]
pub(in crate::text) struct BreakMemo {
    measured: [Option<MeasuredBreak>; MEASURED_BREAKS as usize],
    next_measured: u8,
    committed: Option<BlockConstraint>,
    boxes_dirty: bool,
    min_content: Option<MinContent>,
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

    pub(in crate::text) fn forget(&mut self) {
        self.measured = [None; MEASURED_BREAKS as usize];
        self.next_measured = 0;
        self.min_content = None;
    }

    pub(in crate::text) const fn mark_boxes_dirty(&mut self) {
        self.boxes_dirty = true;
    }

    pub(in crate::text) const fn boxes_dirty(&self) -> bool {
        self.boxes_dirty
    }

    pub(in crate::text) fn holds(
        &self,
        retained: Option<BlockConstraint>,
        fits_from: Option<f32>,
        constraint: BlockConstraint,
    ) -> bool {
        !self.boxes_dirty
            && retained.is_some_and(|retained| retained.answers(fits_from, constraint))
    }

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

    pub(in crate::text) fn is_probe_dirty(&self, retained: Option<BlockConstraint>) -> bool {
        let Some(committed) = self.committed else {
            return false;
        };
        self.boxes_dirty || retained.is_none_or(|retained| retained != committed)
    }

    pub(in crate::text) fn min_content(&self, indent: f32) -> Option<f32> {
        self.min_content
            .filter(|entry| entry.indent.to_bits() == indent.to_bits())
            .map(|entry| entry.width)
    }

    pub(in crate::text) const fn set_min_content(&mut self, indent: f32, width: f32) {
        self.min_content = Some(MinContent { indent, width });
    }
}

#[derive(Debug, Clone, Copy)]
struct MinContent {
    indent: f32,
    width: f32,
}

pub(in crate::text) trait Retained {
    fn memo(&self) -> &BreakMemo;

    fn memo_mut(&mut self) -> &mut BreakMemo;

    fn retained(&self) -> Option<BlockConstraint>;

    fn layout_at(&mut self, context: &mut TextContext, constraint: BlockConstraint);

    fn reported(&self) -> BlockMetrics;

    fn fits_from(&self) -> Option<f32>;

    fn probe(&mut self, context: &mut TextContext, constraint: BlockConstraint) -> BlockMetrics {
        if let Some(remembered) = self.memo().remembered(constraint) {
            return remembered;
        }
        self.layout_at(context, constraint);
        self.remember(constraint)
    }

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
