//! Retained Parley layouts and probe/commit artifact slots.

use parley::{Alignment, AlignmentOptions, IndentOptions, Layout};

use crate::compute::{LeafMeasurement, LeafMetrics};
use crate::geometry::{Point, Size};
use crate::style::TextBrush;

/// A shaped paragraph retained across line-breaking constraints and painting.
///
/// Shaping data lives in [`Self::parley_layout`]; the remaining fields are
/// cheap derived values used by the box-layout protocol. A `TextLayout` can be
/// re-broken repeatedly without invoking Parley's shaping pipeline again.
#[derive(Debug, Clone)]
pub struct TextLayout {
    parley_layout: Layout<TextBrush>,
    metrics: LeafMetrics,
    line_count: usize,
    max_advance: Option<f32>,
    has_text: bool,
}

impl TextLayout {
    pub(super) fn shaped(parley_layout: Layout<TextBrush>, has_text: bool) -> Self {
        Self {
            parley_layout,
            metrics: LeafMetrics::default(),
            line_count: 0,
            max_advance: None,
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

    pub(super) fn align(&mut self, alignment: Alignment) {
        self.parley_layout
            .align(alignment, AlignmentOptions::default());
        self.refresh_metrics();
        if alignment != Alignment::Left {
            let aligned_width = self
                .parley_layout
                .lines()
                .map(|line| line.metrics().inline_max_coord)
                .fold(0.0_f32, f32::max);
            self.metrics.size.width = self.metrics.size.width.max(aligned_width);
        }
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

    /// The owned Parley layout used later by text painting.
    #[must_use]
    pub const fn parley_layout(&self) -> &Layout<TextBrush> {
        &self.parley_layout
    }

    /// Measured content-box size, including inline overflow and any committed
    /// non-left alignment line box needed to contain positioned glyphs.
    #[must_use]
    pub const fn size(&self) -> Size<f32> {
        self.metrics.size
    }

    /// First horizontal-line baseline from the content-box top edge.
    #[must_use]
    pub const fn first_baseline(&self) -> Option<f32> {
        self.metrics.first_baselines.y
    }

    /// Number of lines after the most recent break operation.
    #[must_use]
    pub const fn line_count(&self) -> usize {
        self.line_count
    }

    /// Inline constraint used by the most recent break operation.
    #[must_use]
    pub const fn max_advance(&self) -> Option<f32> {
        self.max_advance
    }
}

/// Borrowed [`LeafMeasurement`] view over a retained [`TextLayout`].
#[derive(Debug, Clone, Copy)]
pub struct TextLayoutView<'a> {
    artifact: &'a TextLayout,
}

impl<'a> TextLayoutView<'a> {
    pub(super) const fn new(artifact: &'a TextLayout) -> Self {
        Self { artifact }
    }

    /// Returns the retained artifact behind this lightweight view.
    #[must_use]
    pub const fn artifact(self) -> &'a TextLayout {
        self.artifact
    }
}

impl LeafMeasurement for TextLayoutView<'_> {
    fn size(&self) -> Size<f32> {
        self.artifact.size()
    }

    fn first_baselines(&self) -> Point<Option<f32>> {
        self.artifact.metrics.first_baselines
    }
}

/// Per-node retained artifacts for transient probes and durable layout.
///
/// A probe is always written to `probe` and therefore never evicts the last
/// committed layout needed by painting. Hosts must call [`Self::invalidate`]
/// together with clearing that node's box-layout cache when text, shaping
/// style, or registered fonts change.
#[derive(Debug, Default)]
pub struct ArtifactSlots {
    pub(super) probe: Option<TextLayout>,
    pub(super) committed: Option<TextLayout>,
}

impl ArtifactSlots {
    /// The most recent transient measurement artifact.
    #[must_use]
    pub const fn probe(&self) -> Option<&TextLayout> {
        self.probe.as_ref()
    }

    /// The durable artifact corresponding to committed box geometry.
    #[must_use]
    pub const fn committed(&self) -> Option<&TextLayout> {
        self.committed.as_ref()
    }

    /// Clears both text artifacts.
    ///
    /// The host must clear the corresponding box-layout cache in the same
    /// invalidation operation.
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
        let view = TextLayoutView::new(&artifact);

        assert!(core::ptr::eq(
            view.artifact(),
            core::ptr::from_ref(&artifact)
        ));
        assert_eq!(view.size(), artifact.size());
        assert_eq!(view.first_baselines(), Point::NONE);
        assert_eq!(artifact.max_advance(), Some(30.0));
        assert_eq!(artifact.line_count(), 0);
    }

    #[test]
    fn artifact_invalidation_clears_both_lifetimes() {
        let mut slots = ArtifactSlots {
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
