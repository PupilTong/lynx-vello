//! Retained Parley layouts and probe/commit artifact slots.

use std::collections::HashMap;

use parley::{Alignment, AlignmentOptions, IndentOptions, Layout, PositionedLayoutItem};

use super::AtomicInlineBox;
use crate::compute::LeafMetrics;
use crate::geometry::{Point, Size};
use crate::style::TextBrush;

/// A shaped paragraph retained across line-breaking constraints and painting.
#[derive(Debug, Clone)]
#[allow(
    clippy::box_collection,
    reason = "pure-text layouts keep atom-only indexes and line adjustments out of their hot object"
)]
pub struct TextLayout {
    parley_layout: Layout<TextBrush>,
    max_advance: Option<f32>,
    min_content_width: f32,
    has_content: bool,
    inline_boxes: Option<Box<[AtomicInlineBox]>>,
    inline_box_lookup: Option<Box<HashMap<u64, AtomicInlineBox>>>,
    line_adjustments: Option<Box<Vec<LineAdjustment>>>,
    extra_height: f32,
}

#[derive(Debug, Clone, Copy)]
struct LineAdjustment {
    block_offset: f32,
    block_start: f32,
}

impl TextLayout {
    pub(super) fn shaped_with_inline_boxes(
        parley_layout: Layout<TextBrush>,
        has_content: bool,
        inline_boxes: Vec<AtomicInlineBox>,
    ) -> Self {
        let min_content_width = parley_layout.calculate_content_widths().min;
        let inline_box_lookup = if inline_boxes.is_empty() {
            None
        } else {
            let mut lookup = HashMap::with_capacity(inline_boxes.len());
            for inline_box in &inline_boxes {
                assert!(
                    lookup.insert(inline_box.id, *inline_box).is_none(),
                    "atomic inline-box identifiers must be unique",
                );
            }
            Some(Box::new(lookup))
        };
        Self {
            parley_layout,
            max_advance: None,
            min_content_width,
            has_content,
            inline_boxes: (!inline_boxes.is_empty()).then(|| inline_boxes.into_boxed_slice()),
            inline_box_lookup,
            line_adjustments: None,
            extra_height: 0.0,
        }
    }

    pub(super) fn rebreak(&mut self, max_advance: Option<f32>, text_indent: f32) {
        self.parley_layout
            .set_text_indent(text_indent, IndentOptions::default());
        self.parley_layout.break_all_lines(max_advance);
        self.max_advance = max_advance;
        self.resolve_atomic_line_metrics();
    }

    pub(super) const fn min_content_width(&self) -> f32 {
        self.min_content_width
    }

    pub(super) fn inline_boxes_match(
        &self,
        inline_boxes: impl Iterator<Item = AtomicInlineBox>,
    ) -> bool {
        self.inline_boxes
            .as_deref()
            .unwrap_or_default()
            .iter()
            .copied()
            .eq(inline_boxes)
    }

    pub(super) fn align(&mut self, alignment: Alignment) {
        self.parley_layout
            .align(alignment, AlignmentOptions::default());
    }

    #[must_use]
    pub const fn parley_layout(&self) -> &Layout<TextBrush> {
        &self.parley_layout
    }

    #[must_use]
    pub fn size(&self) -> Size<f32> {
        Size::new(
            self.parley_layout.width(),
            self.parley_layout.height() + self.extra_height,
        )
    }

    #[must_use]
    pub fn first_baseline(&self) -> Option<f32> {
        self.has_content
            .then(|| self.parley_layout.get(0))
            .flatten()
            .map(|line| line.metrics().baseline)
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        if self.has_content {
            self.parley_layout.len()
        } else {
            0
        }
    }

    /// Whether painting this layout can emit at least one glyph run.
    #[must_use]
    pub fn has_glyphs(&self) -> bool {
        self.parley_layout.lines().any(|line| {
            line.items()
                .any(|item| matches!(item, PositionedLayoutItem::GlyphRun(_)))
        })
    }

    /// Returns all positioned atomic boxes in source order within their lines.
    ///
    /// `origin.y + baseline` equals Parley's line baseline. Parley models an
    /// inline box as ascent-only, so a baseline above the bottom edge can
    /// extend below its line; [`Self::size`] includes that terminal overflow,
    /// while Parley's spacing between subsequent lines remains unchanged.
    pub fn positioned_inline_boxes(&self) -> impl Iterator<Item = PositionedInlineBox> + '_ {
        let lookup = self.inline_box_lookup.as_deref();
        let adjustments = self.line_adjustments.as_deref();
        self.parley_layout
            .lines()
            .enumerate()
            .flat_map(move |(line_index, line)| {
                let adjustment = adjustments.and_then(|values| values.get(line_index));
                let block_offset = adjustment.map_or(0.0, |value| value.block_offset);
                let line_top = adjustment
                    .map_or_else(|| line.metrics().block_min_coord, |value| value.block_start);
                line.items().filter_map(move |item| match item {
                    PositionedLayoutItem::InlineBox(inline_box) => {
                        let atomic = lookup?.get(&inline_box.id)?;
                        Some(PositionedInlineBox {
                            id: inline_box.id,
                            origin: Point::new(
                                inline_box.x,
                                inline_box.y + inline_box.height - atomic.baseline + block_offset,
                            ),
                            size: Size::new(atomic.width, atomic.height),
                            baseline: atomic.baseline,
                            line_top,
                        })
                    }
                    PositionedLayoutItem::GlyphRun(_) => None,
                })
            })
    }

    /// Extra block-axis translation applied to one Parley line so atomic
    /// descents cannot overlap the following line.
    #[must_use]
    pub fn line_block_offset(&self, line_index: usize) -> f32 {
        self.line_adjustments
            .as_deref()
            .and_then(|adjustments| adjustments.get(line_index))
            .map_or(0.0, |adjustment| adjustment.block_offset)
    }

    /// Finds the positioned atomic box with the host-provided identifier.
    #[must_use]
    pub fn positioned_inline_box(&self, id: u64) -> Option<PositionedInlineBox> {
        self.positioned_inline_boxes()
            .find(|inline_box| inline_box.id == id)
    }

    #[must_use]
    pub const fn max_advance(&self) -> Option<f32> {
        self.max_advance
    }

    fn resolve_atomic_line_metrics(&mut self) {
        let Some(lookup) = self.inline_box_lookup.as_deref() else {
            self.line_adjustments = None;
            self.extra_height = 0.0;
            return;
        };
        let mut adjustments = Vec::with_capacity(self.parley_layout.len());
        let mut original_line_start = 0.0;
        let mut adjusted_line_start = 0.0;
        let mut needs_adjustments = false;
        for line in self.parley_layout.lines() {
            let metrics = line.metrics();
            let mut ascent = 0.0_f32;
            let mut descent = 0.0_f32;
            let mut has_atomic = false;
            for item in line.items() {
                match item {
                    PositionedLayoutItem::InlineBox(inline_box) => {
                        if let Some(atomic) = lookup.get(&inline_box.id) {
                            ascent = ascent.max(atomic.baseline);
                            descent = descent.max((atomic.height - atomic.baseline).max(0.0));
                            has_atomic = true;
                        }
                    }
                    PositionedLayoutItem::GlyphRun(glyph_run) => {
                        let run = glyph_run.run().metrics();
                        ascent = ascent.max(run.ascent);
                        descent = descent.max(run.descent);
                    }
                }
            }
            let (adjusted_line_height, adjusted_baseline) = if has_atomic {
                let line_height = metrics.line_height.max(ascent + descent);
                let leading_above = (line_height - ascent - descent).max(0.0) / 2.0;
                (line_height, ascent + leading_above)
            } else {
                (metrics.line_height, metrics.baseline - original_line_start)
            };
            let block_offset = adjusted_line_start + adjusted_baseline - metrics.baseline;
            needs_adjustments |= block_offset.abs() > 1.0e-5
                || (adjusted_line_start - metrics.block_min_coord).abs() > 1.0e-5
                || (adjusted_line_height - metrics.line_height).abs() > 1.0e-5;
            adjustments.push(LineAdjustment {
                block_offset,
                block_start: adjusted_line_start,
            });
            original_line_start += metrics.line_height;
            adjusted_line_start += adjusted_line_height;
        }
        self.extra_height = (adjusted_line_start - original_line_start).max(0.0);
        self.line_adjustments = needs_adjustments.then(|| Box::new(adjustments));
    }
}

/// Host-facing position of one laid-out atomic inline box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedInlineBox {
    pub id: u64,
    pub origin: Point<f32>,
    pub size: Size<f32>,
    /// Baseline offset from `origin.y` used for default inline alignment.
    pub baseline: f32,
    /// Block-start edge of the line containing this box.
    pub line_top: f32,
}

/// Borrowed view over a retained [`TextLayout`].
#[derive(Debug, Clone, Copy)]
pub struct TextMeasurement<'a> {
    layout: &'a TextLayout,
}

impl<'a> TextMeasurement<'a> {
    pub(super) const fn new(layout: &'a TextLayout) -> Self {
        Self { layout }
    }

    #[must_use]
    pub const fn layout(self) -> &'a TextLayout {
        self.layout
    }

    #[must_use]
    pub fn size(self) -> Size<f32> {
        self.layout.size()
    }

    #[must_use]
    pub fn first_baselines(self) -> Point<Option<f32>> {
        Point::new(None, self.layout.first_baseline())
    }

    pub(super) fn metrics(self) -> LeafMetrics {
        LeafMetrics::new(self.size()).with_first_baselines(self.first_baselines())
    }
}

/// Per-node retained artifacts for transient probes and durable layout.
#[derive(Debug, Default)]
pub struct TextLayoutStore {
    pub(super) probe: Option<Box<TextLayout>>,
    pub(super) committed: Option<Box<TextLayout>>,
}

impl TextLayoutStore {
    #[must_use]
    pub fn probe(&self) -> Option<&TextLayout> {
        self.probe.as_deref()
    }

    #[must_use]
    pub fn committed(&self) -> Option<&TextLayout> {
        self.committed.as_deref()
    }

    pub fn invalidate(&mut self) {
        self.probe = None;
        self.committed = None;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn empty_artifact() -> TextLayout {
        TextLayout::shaped_with_inline_boxes(Layout::default(), false, Vec::new())
    }

    #[test]
    fn borrowed_view_exposes_artifact_metrics() {
        let mut artifact = empty_artifact();
        artifact.rebreak(Some(30.0), 0.0);
        let view = TextMeasurement::new(&artifact);

        assert!(core::ptr::eq(view.layout(), core::ptr::from_ref(&artifact)));
        assert_eq!(view.size(), artifact.size());
        assert_eq!(view.first_baselines(), Point::NONE);
        assert_eq!(artifact.max_advance(), Some(30.0));
        assert_eq!(artifact.line_count(), 0);
    }

    #[test]
    fn artifact_invalidation_clears_both_lifetimes() {
        let mut slots = TextLayoutStore {
            probe: Some(Box::new(empty_artifact())),
            committed: Some(Box::new(empty_artifact())),
        };
        assert!(slots.probe().is_some());
        assert!(slots.committed().is_some());

        slots.invalidate();

        assert!(slots.probe().is_none());
        assert!(slots.committed().is_none());
    }

    #[test]
    fn artifact_slots_are_pointer_sized() {
        assert_eq!(
            size_of::<TextLayoutStore>(),
            2 * size_of::<*const TextLayout>()
        );
        assert!(size_of::<TextLayoutStore>() < size_of::<TextLayout>());
        assert!(
            size_of::<TextLayout>() <= size_of::<Layout<TextBrush>>() + size_of::<[usize; 8]>(),
            "retained text should add only intrinsic/break widths, lifecycle metadata, and an optional atomic-box slice"
        );
    }
}
