//! One retained Parley layout per text node, plus the break state that says
//! which constraint its line-break output currently reflects.
//!
//! Shaping is the expensive half of text layout and is invariant under
//! line-breaking: of the ten vectors Parley's `LayoutData` owns, eight
//! (styles, inline boxes, fonts, coords, runs, items, clusters, glyphs) are
//! shaping output, and only `lines`/`line_items` are rebuilt by
//! [`Layout::break_all_lines`]. A node therefore keeps **one** shaped layout
//! and re-breaks it in place for whichever constraint is being asked about,
//! rather than keeping a second, deep-cloned copy for transient probes.
//!
//! That makes the retained layout mutable during a pass, so it carries three
//! pieces of bookkeeping:
//!
//! - [`TextLayout::broken`] — the constraint the current line-break output reflects. Re-breaking at
//!   the same constraint is a no-op, which is what collapses the "probe at W then commit at W" pair
//!   into one break.
//! - [`TextLayout::measured`] — what a handful of already-answered constraints reported. A probe
//!   that hits it neither re-enters Parley nor moves `broken`, which matters because containers
//!   interleave constraints: a flex item is asked max-content, min-content, its used width, then
//!   max-content again, and a break-state memo alone re-breaks on every alternation.
//! - [`TextLayout::committed`] — the constraint *and alignment* the last committed measurement
//!   left. Everything outside a layout pass (painting, hit testing) reads a committed layout, so a
//!   probe that moves `broken` away from `committed` owes a [`TextLayout::restore_committed`]
//!   before the pass ends.
//!
//! Measured over the canonical Lynx label (an auto-sized `text` element in a
//! definite-width flex row), one pass costs nine line breaks plus one deep
//! clone with neither memo, four with the break memo, and two with both.

use parley::{Alignment, AlignmentOptions, IndentOptions, Layout};

use crate::compute::LeafMetrics;
use crate::geometry::{Point, Size};
use crate::style::TextBrush;

/// The inputs of one `break_all_lines` call.
///
/// `max_advance` is unpacked, not an `Option`: Parley itself represents "no
/// constraint" as `f32::MAX` inside the line breaker, and keeping the same
/// shape here halves the constraint so a node can remember several of them for
/// the price of one `Option<f32>` pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct BreakConstraint {
    /// The line-break width, or [`f32::INFINITY`] for max-content.
    max_advance: f32,
    /// The resolved `text-indent` in pixels.
    indent: f32,
}

impl BreakConstraint {
    pub(super) fn new(max_advance: Option<f32>, indent: f32) -> Self {
        Self {
            max_advance: max_advance.unwrap_or(f32::INFINITY),
            indent,
        }
    }

    fn max_advance(self) -> Option<f32> {
        self.max_advance.is_finite().then_some(self.max_advance)
    }
}

/// How many already-answered constraints one layout remembers.
///
/// Three: containers cycle a leaf through max-content, min-content, and its
/// used width, so anything smaller thrashes and anything larger only pays for
/// itself on a constraint set no algorithm here produces.
const MEASURED_BREAKS: u8 = 3;

/// What one constraint reported, so a repeat answers without touching Parley.
#[derive(Debug, Clone, Copy)]
struct MeasuredBreak {
    constraint: BreakConstraint,
    metrics: TextMeasurement,
}

/// The resting state a committed measurement leaves behind.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CommittedState {
    constraint: BreakConstraint,
    alignment: Alignment,
}

/// A shaped paragraph retained across line-breaking constraints and painting.
#[derive(Debug, Clone)]
pub struct TextLayout {
    parley_layout: Layout<TextBrush>,
    /// The constraint `parley_layout`'s lines currently reflect, or `None`
    /// while it has never been broken.
    broken: Option<BreakConstraint>,
    /// Constraints this layout has already reported on, most recent last.
    measured: [Option<MeasuredBreak>; MEASURED_BREAKS as usize],
    /// Round-robin write cursor into `measured`.
    next_measured: u8,
    /// The state the last committed measurement left, if any.
    committed: Option<CommittedState>,
    min_content_width: f32,
    /// How many times Parley has actually broken these lines. The memo is only
    /// worth its state if callers can see it working, and this is what the
    /// tests and benchmarks assert on.
    breaks: u32,
    has_text: bool,
    /// Debug-only identity of the content and style this layout was shaped
    /// from. Re-checked on every measurement so a missed eviction surfaces as
    /// a panic instead of silently stale text.
    #[cfg(debug_assertions)]
    fingerprint: ShapeFingerprint,
}

impl TextLayout {
    pub(super) fn shaped(
        parley_layout: Layout<TextBrush>,
        has_text: bool,
        #[cfg(debug_assertions)] fingerprint: ShapeFingerprint,
    ) -> Self {
        let min_content_width = parley_layout.calculate_content_widths().min;
        Self {
            parley_layout,
            broken: None,
            measured: [None; MEASURED_BREAKS as usize],
            next_measured: 0,
            committed: None,
            min_content_width,
            breaks: 0,
            has_text,
            #[cfg(debug_assertions)]
            fingerprint,
        }
    }

    /// Breaks lines for `constraint`, skipping the work when the retained
    /// output already reflects it.
    ///
    /// Returns whether Parley was actually re-entered — the benchmarks and
    /// tests assert on this to keep the memo honest.
    pub(super) fn break_to(&mut self, constraint: BreakConstraint) -> bool {
        if self.broken == Some(constraint) {
            return false;
        }
        self.parley_layout
            .set_text_indent(constraint.indent, IndentOptions::default());
        self.parley_layout.break_all_lines(constraint.max_advance());
        self.broken = Some(constraint);
        self.breaks = self.breaks.saturating_add(1);
        true
    }

    /// Reports `constraint` without disturbing the retained line breaks when
    /// the same constraint has already been answered.
    pub(super) fn probe(&mut self, constraint: BreakConstraint) -> TextMeasurement {
        if let Some(remembered) = self.remembered(constraint) {
            return remembered;
        }
        self.break_to(constraint);
        self.remember(constraint)
    }

    /// Breaks lines for `constraint` for real — a commit's line breaks are the
    /// ones painting reads, so this one cannot be answered from the memo.
    pub(super) fn commit_break(&mut self, constraint: BreakConstraint) -> TextMeasurement {
        self.break_to(constraint);
        self.remember(constraint)
    }

    fn remembered(&self, constraint: BreakConstraint) -> Option<TextMeasurement> {
        self.measured
            .iter()
            .flatten()
            .find(|entry| entry.constraint == constraint)
            .map(|entry| entry.metrics)
    }

    fn remember(&mut self, constraint: BreakConstraint) -> TextMeasurement {
        let metrics = self.current_metrics();
        let entry = MeasuredBreak {
            constraint,
            metrics,
        };
        // Re-recording a constraint the memo already holds — a commit landing
        // on the width a probe asked about — must overwrite that entry rather
        // than consume a fresh slot, or it evicts a constraint the pass is
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

    fn current_metrics(&self) -> TextMeasurement {
        TextMeasurement {
            size: Size::new(self.parley_layout.width(), self.parley_layout.height()),
            first_baseline: self.first_baseline(),
            line_count: u32::try_from(self.line_count()).unwrap_or(u32::MAX),
        }
    }

    pub(super) const fn min_content_width(&self) -> f32 {
        self.min_content_width
    }

    pub(super) fn align(&mut self, alignment: Alignment) {
        self.parley_layout
            .align(alignment, AlignmentOptions::default());
    }

    /// Records the state this measurement is leaving behind as the committed
    /// resting state.
    pub(super) fn mark_committed(&mut self, alignment: Alignment) {
        let constraint = self
            .broken
            .expect("a committed measurement always breaks lines first");
        self.committed = Some(CommittedState {
            constraint,
            alignment,
        });
    }

    /// Whether the retained layout has drifted off its committed resting state
    /// and owes a [`Self::restore_committed`].
    #[must_use]
    pub fn is_probe_dirty(&self) -> bool {
        self.committed
            .is_some_and(|committed| self.broken != Some(committed.constraint))
    }

    /// Re-breaks and re-aligns to the committed resting state. A no-op when a
    /// probe left the layout untouched (the common case, because a probe at
    /// the committed constraint never re-enters Parley) or when nothing has
    /// been committed yet.
    pub fn restore_committed(&mut self) -> bool {
        let Some(committed) = self.committed else {
            return false;
        };
        if !self.break_to(committed.constraint) {
            return false;
        }
        self.align(committed.alignment);
        true
    }

    /// Whether a committed measurement has ever produced this layout.
    #[must_use]
    pub const fn has_committed(&self) -> bool {
        self.committed.is_some()
    }

    #[must_use]
    pub const fn parley_layout(&self) -> &Layout<TextBrush> {
        &self.parley_layout
    }

    #[must_use]
    pub fn size(&self) -> Size<f32> {
        Size::new(self.parley_layout.width(), self.parley_layout.height())
    }

    #[must_use]
    pub fn first_baseline(&self) -> Option<f32> {
        self.has_text
            .then(|| self.parley_layout.get(0))
            .flatten()
            .map(|line| line.metrics().baseline)
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        if self.has_text {
            self.parley_layout.len()
        } else {
            0
        }
    }

    /// The break width the retained lines reflect, or `None` for max-content
    /// (and before the first break).
    #[must_use]
    pub fn max_advance(&self) -> Option<f32> {
        self.broken?.max_advance()
    }

    /// How many times Parley has broken these lines since the layout was
    /// shaped. Every increment is one `O(clusters)` pass, so this is the
    /// number the measurement path exists to keep down.
    #[must_use]
    pub const fn break_count(&self) -> u32 {
        self.breaks
    }

    #[cfg(debug_assertions)]
    pub(super) fn assert_shaped_from(&self, fingerprint: ShapeFingerprint) {
        assert_eq!(
            self.fingerprint, fingerprint,
            "a retained text layout outlived the content or style it was shaped from: \
             some eviction path failed to drop it, which would paint stale text"
        );
    }
}

/// One constraint's measurement, detached from the layout that produced it.
///
/// A value rather than a borrow because the memo can answer a probe while the
/// retained layout is broken at a *different* constraint; handing back a
/// reference would let a caller read line breaks that do not belong to the
/// measurement it asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMeasurement {
    size: Size<f32>,
    first_baseline: Option<f32>,
    line_count: u32,
}

impl TextMeasurement {
    #[must_use]
    pub const fn size(self) -> Size<f32> {
        self.size
    }

    #[must_use]
    pub const fn first_baselines(self) -> Point<Option<f32>> {
        Point::new(None, self.first_baseline)
    }

    #[must_use]
    pub const fn line_count(self) -> usize {
        self.line_count as usize
    }

    pub(super) fn metrics(self) -> LeafMetrics {
        LeafMetrics::new(self.size()).with_first_baselines(self.first_baselines())
    }
}

/// The per-node retained text artifact.
///
/// One slot, not two: probes re-break the same shaped layout the commit uses
/// and restore it afterwards, so nothing here holds a duplicate of Parley's
/// shaped vectors and there is no second slot to leak past its owner's next
/// pass.
#[derive(Debug, Default)]
pub struct TextLayoutStore {
    pub(super) artifact: Option<Box<TextLayout>>,
}

impl TextLayoutStore {
    /// The retained layout in whatever break state it currently holds.
    #[must_use]
    pub fn retained(&self) -> Option<&TextLayout> {
        self.artifact.as_deref()
    }

    /// The committed layout — what painting and hit testing read.
    ///
    /// Returns `None` until a commit has produced one. Reading it while a
    /// probe still owes a restore is a bug in the pass driver, and trips a
    /// debug assertion rather than painting stale line breaks.
    #[must_use]
    pub fn committed(&self) -> Option<&TextLayout> {
        let artifact = self.artifact.as_deref()?;
        if !artifact.has_committed() {
            return None;
        }
        debug_assert!(
            !artifact.is_probe_dirty(),
            "a text layout was read while still broken at a probe constraint: \
             the pass must restore probed text before anything reads it"
        );
        Some(artifact)
    }

    /// Whether a probe left the retained layout off its committed state.
    #[must_use]
    pub fn is_probe_dirty(&self) -> bool {
        self.artifact
            .as_deref()
            .is_some_and(TextLayout::is_probe_dirty)
    }

    /// Returns the retained layout to its committed break state. Cheap and
    /// idempotent; the pass driver calls it for every node it probed.
    pub fn restore_committed(&mut self) -> bool {
        self.artifact
            .as_deref_mut()
            .is_some_and(TextLayout::restore_committed)
    }

    /// Drops the shaped layout. Only correct where the shaping inputs
    /// themselves may have changed — a purely geometric invalidation should
    /// clear the box cache and leave this alone.
    pub fn invalidate(&mut self) {
        self.artifact = None;
    }
}

/// Debug-only identity of the text and style a retained layout was shaped
/// from.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ShapeFingerprint(u64);

#[cfg(debug_assertions)]
impl ShapeFingerprint {
    pub(super) const fn from_hash(hash: u64) -> Self {
        Self(hash)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn empty_artifact() -> TextLayout {
        TextLayout::shaped(
            Layout::default(),
            false,
            #[cfg(debug_assertions)]
            ShapeFingerprint::from_hash(0),
        )
    }

    fn constraint(max_advance: Option<f32>) -> BreakConstraint {
        BreakConstraint::new(max_advance, 0.0)
    }

    #[test]
    fn a_measurement_reports_the_constraint_it_was_taken_at() {
        let mut artifact = empty_artifact();
        let measured = artifact.probe(constraint(Some(30.0)));

        assert_eq!(measured.size(), artifact.size());
        assert_eq!(measured.first_baselines(), Point::NONE);
        assert_eq!(measured.line_count(), 0);
        assert_eq!(artifact.max_advance(), Some(30.0));
    }

    #[test]
    fn a_remembered_constraint_answers_without_moving_the_retained_break() {
        let mut artifact = empty_artifact();
        let wide = constraint(None);
        let narrow = constraint(Some(30.0));

        let wide_metrics = artifact.probe(wide);
        let narrow_metrics = artifact.probe(narrow);
        assert_eq!(artifact.break_count(), 2);

        // Containers alternate constraints; the memo has to answer the repeat
        // without dragging the retained lines back and forth.
        assert_eq!(artifact.probe(wide), wide_metrics);
        assert_eq!(artifact.probe(narrow), narrow_metrics);
        assert_eq!(artifact.break_count(), 2);
        assert_eq!(
            artifact.max_advance(),
            Some(30.0),
            "a memo hit leaves the retained lines where the last real break put them",
        );

        // A commit is what painting reads, so it always lands the real break.
        artifact.commit_break(wide);
        assert_eq!(artifact.break_count(), 3);
        assert_eq!(artifact.max_advance(), None);
    }

    #[test]
    fn the_memo_forgets_the_least_recently_written_constraint() {
        let mut artifact = empty_artifact();
        for width in [10.0, 20.0, 30.0, 40.0] {
            artifact.probe(constraint(Some(width)));
        }
        assert_eq!(artifact.break_count(), 4);

        // 20/30/40 are still remembered; 10 was overwritten by 40.
        for width in [20.0, 30.0, 40.0] {
            artifact.probe(constraint(Some(width)));
        }
        assert_eq!(artifact.break_count(), 4);
        artifact.probe(constraint(Some(10.0)));
        assert_eq!(artifact.break_count(), 5);
    }

    #[test]
    fn re_recording_a_held_constraint_does_not_evict_a_different_one() {
        let mut artifact = empty_artifact();
        for width in [10.0, 20.0, 30.0] {
            artifact.probe(constraint(Some(width)));
        }
        assert_eq!(artifact.break_count(), 3);

        // A commit landing on a width a probe already asked about overwrites
        // that entry rather than taking a fourth slot, so 10 and 20 survive
        // it and none of the three has to break again.
        artifact.commit_break(constraint(Some(30.0)));
        for width in [10.0, 20.0, 30.0] {
            artifact.probe(constraint(Some(width)));
        }
        assert_eq!(artifact.break_count(), 3, "the memo still holds all three");
    }

    #[test]
    fn breaking_at_the_retained_constraint_is_a_no_op() {
        let mut artifact = empty_artifact();

        assert!(artifact.break_to(constraint(Some(30.0))));
        assert!(!artifact.break_to(constraint(Some(30.0))));
        assert!(artifact.break_to(constraint(Some(31.0))));
        assert!(artifact.break_to(BreakConstraint::new(Some(31.0), 4.0)));
        assert!(artifact.break_to(constraint(None)));
        assert!(!artifact.break_to(constraint(None)));
        assert_eq!(artifact.max_advance(), None);
    }

    #[test]
    fn a_probe_owes_a_restore_only_when_it_moved_the_committed_constraint() {
        let mut artifact = empty_artifact();
        assert!(!artifact.is_probe_dirty(), "nothing committed yet");
        assert!(!artifact.restore_committed());

        artifact.break_to(constraint(Some(30.0)));
        artifact.align(Alignment::Center);
        artifact.mark_committed(Alignment::Center);
        assert!(artifact.has_committed());
        assert!(!artifact.is_probe_dirty());

        // A probe at the committed constraint never re-enters Parley, so it
        // leaves nothing to restore.
        assert!(!artifact.break_to(constraint(Some(30.0))));
        assert!(!artifact.is_probe_dirty());
        assert!(!artifact.restore_committed());

        assert!(artifact.break_to(constraint(Some(12.0))));
        assert!(artifact.is_probe_dirty());
        assert!(artifact.restore_committed());
        assert!(!artifact.is_probe_dirty());
        assert_eq!(artifact.max_advance(), Some(30.0));
        assert!(!artifact.restore_committed());
    }

    #[test]
    fn artifact_invalidation_drops_the_shaped_layout() {
        let mut slots = TextLayoutStore {
            artifact: Some(Box::new(empty_artifact())),
        };
        assert!(slots.retained().is_some());
        assert!(slots.committed().is_none(), "nothing committed yet");

        let artifact = slots.artifact.as_deref_mut().expect("retained");
        artifact.break_to(constraint(Some(30.0)));
        artifact.mark_committed(Alignment::Left);
        assert!(slots.committed().is_some());
        assert!(!slots.is_probe_dirty());

        slots
            .artifact
            .as_deref_mut()
            .expect("retained")
            .break_to(constraint(Some(10.0)));
        assert!(slots.is_probe_dirty());
        assert!(slots.restore_committed());
        assert!(!slots.is_probe_dirty());

        slots.invalidate();
        assert!(slots.retained().is_none());
        assert!(slots.committed().is_none());
        assert!(!slots.is_probe_dirty());
        assert!(!slots.restore_committed());
    }

    #[test]
    fn the_artifact_slot_is_one_pointer_and_the_break_state_rides_behind_it() {
        assert_eq!(size_of::<TextLayoutStore>(), size_of::<*const TextLayout>());
        assert!(size_of::<TextLayoutStore>() < size_of::<TextLayout>());
        // Retained text adds the intrinsic width, the lifecycle flag, and the
        // two break-state records the single-slot design needs: the constraint
        // the lines reflect, and the committed constraint plus alignment a
        // probe has to restore.
        // 128 bytes over Parley's own layout, three quarters of it the three
        // remembered constraints. That is one allocation-free answer per
        // remembered constraint against an `O(clusters)` line break, on a
        // struct that already owns ten heap vectors.
        let overhead = size_of::<TextLayout>() - size_of::<Layout<TextBrush>>();
        assert!(
            overhead <= 16 * size_of::<usize>(),
            "retained text adds {overhead} bytes of break state to Parley's layout"
        );
    }
}
