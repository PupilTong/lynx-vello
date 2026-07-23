//! Retained Parley layouts and probe/commit artifact slots.

use parley::{Alignment, AlignmentOptions, IndentOptions, Layout};

use crate::compute::LeafMetrics;
use crate::geometry::{Point, Size};
use crate::style::TextBrush;

/// A shaped paragraph retained across line-breaking constraints and painting.
#[derive(Debug, Clone)]
pub struct TextLayout {
    parley_layout: Layout<TextBrush>,
    metrics: LeafMetrics,
    line_count: usize,
    max_advance: Option<f32>,
    min_content_width: f32,
    has_text: bool,
}

impl TextLayout {
    pub(super) fn shaped(parley_layout: Layout<TextBrush>, has_text: bool) -> Self {
        let min_content_width = parley_layout.calculate_content_widths().min;
        Self {
            parley_layout,
            metrics: LeafMetrics::default(),
            line_count: 0,
            max_advance: None,
            min_content_width,
            has_text,
        }
    }

    pub(super) fn rebreak(&mut self, max_advance: Option<f32>, text_indent: f32) {
        self.parley_layout
            .set_text_indent(text_indent, IndentOptions::default());
        self.parley_layout.break_all_lines(max_advance);
        self.max_advance = max_advance;
        self.refresh_metrics();
    }

    pub(super) const fn min_content_width(&self) -> f32 {
        self.min_content_width
    }

    pub(super) fn align(&mut self, alignment: Alignment) {
        self.parley_layout
            .align(alignment, AlignmentOptions::default());
        self.refresh_metrics();
    }

    fn refresh_metrics(&mut self) {
        let baseline = self
            .has_text
            .then(|| self.parley_layout.get(0))
            .flatten()
            .map(|line| line.metrics().baseline);
        self.metrics = LeafMetrics::new(Size::new(
            self.parley_layout.width(),
            self.parley_layout.height(),
        ))
        .with_first_baselines(Point::new(None, baseline));
        self.line_count = if self.has_text {
            self.parley_layout.len()
        } else {
            0
        };
    }

    #[must_use]
    pub const fn parley_layout(&self) -> &Layout<TextBrush> {
        &self.parley_layout
    }

    #[must_use]
    pub const fn size(&self) -> Size<f32> {
        self.metrics.size
    }

    #[must_use]
    pub const fn first_baseline(&self) -> Option<f32> {
        self.metrics.first_baselines.y
    }

    #[must_use]
    pub const fn line_count(&self) -> usize {
        self.line_count
    }

    #[must_use]
    pub const fn max_advance(&self) -> Option<f32> {
        self.max_advance
    }
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
    pub const fn size(self) -> Size<f32> {
        self.layout.size()
    }

    #[must_use]
    pub const fn first_baselines(self) -> Point<Option<f32>> {
        self.layout.metrics.first_baselines
    }

    pub(super) const fn metrics(self) -> LeafMetrics {
        self.layout.metrics
    }
}

/// Per-node retained artifacts for transient probes and durable layout.
#[derive(Debug, Default)]
pub struct TextLayoutStore {
    pub(super) probe: Option<TextLayout>,
    pub(super) committed: Option<TextLayout>,
}

impl TextLayoutStore {
    #[must_use]
    pub const fn probe(&self) -> Option<&TextLayout> {
        self.probe.as_ref()
    }

    #[must_use]
    pub const fn committed(&self) -> Option<&TextLayout> {
        self.committed.as_ref()
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
        TextLayout::shaped(Layout::default(), false)
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
            probe: Some(empty_artifact()),
            committed: Some(empty_artifact()),
        };
        assert!(slots.probe().is_some());
        assert!(slots.committed().is_some());

        slots.invalidate();

        assert!(slots.probe().is_none());
        assert!(slots.committed().is_none());
    }
}
