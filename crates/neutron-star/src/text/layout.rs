//! Retained Parley layouts and probe/commit artifact slots.

use parley::{Alignment, AlignmentOptions, IndentOptions, Layout};

use crate::compute::LeafMetrics;
use crate::geometry::{Point, Size};
use crate::style::TextBrush;

/// A shaped paragraph retained across line-breaking constraints and painting.
#[derive(Debug, Clone)]
pub struct TextLayout {
    parley_layout: Layout<TextBrush>,
    max_advance: Option<f32>,
    min_content_width: f32,
    has_text: bool,
}

impl TextLayout {
    pub(super) fn shaped(parley_layout: Layout<TextBrush>, has_text: bool) -> Self {
        let min_content_width = parley_layout.calculate_content_widths().min;
        Self {
            parley_layout,
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
    }

    pub(super) const fn min_content_width(&self) -> f32 {
        self.min_content_width
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
            size_of::<TextLayout>() <= size_of::<Layout<TextBrush>>() + 2 * size_of::<usize>(),
            "retained text should add only intrinsic-width, break-width, and lifecycle metadata"
        );
    }
}
