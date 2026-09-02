//! Vertical alignment of atomic boxes within their lines.
//!
//! Parley has no vertical-align: it places every in-flow box bottom-on-
//! baseline and counts the box height as pure ascent. The block keeps
//! parley's line stacking and works its lever from both sides — what a box
//! contributes to line height is written into `InlineBox::height` before
//! every break (the contribution table), and where the box sits is overridden
//! per positioned item after alignment (the placement table). The line is
//! always tall enough — every non-baseline value contributes the full box
//! height — but the baseline's position inside a line grown by a
//! non-baseline-aligned tall box follows parley's all-ascent rule rather than
//! the CSS ascent/descent split, and a baseline-aligned box's below-baseline
//! part does not reserve descent. Both are recorded deviations.

use parley::{Line, LineMetrics};

use super::content::InlineBoxSpec;
use super::style::VerticalAlign;
use crate::style::TextBrush;

/// The `sub` baseline drop as a fraction of the reference font size. CSS
/// leaves the exact offset to the user agent.
const SUB_BASELINE_FACTOR: f32 = 0.20;
/// The `super` baseline raise as a fraction of the reference font size.
const SUPER_BASELINE_FACTOR: f32 = 0.34;

/// What one box adds to its line's height, written into `InlineBox::height`.
///
/// Baseline-anchored values contribute the above-baseline part; a positive
/// length raise adds itself on top; every extent-anchored value contributes
/// the full height. `Super`'s raise depends on line metrics that do not exist
/// until the line is assembled, so it contributes like `Baseline` and the
/// raise is placement-only.
pub(in crate::text::block) fn line_contribution(spec: &InlineBoxSpec) -> f32 {
    let above_baseline = spec.baseline.unwrap_or(spec.size.height);
    match spec.vertical_align {
        VerticalAlign::Baseline | VerticalAlign::Sub | VerticalAlign::Super => above_baseline,
        VerticalAlign::Length(raise) => above_baseline + raise.max(0.0),
        VerticalAlign::Top
        | VerticalAlign::TextTop
        | VerticalAlign::Middle
        | VerticalAlign::Bottom
        | VerticalAlign::TextBottom
        | VerticalAlign::Percent(_)
        | VerticalAlign::Center => spec.size.height,
    }
}

/// Per-line reference metrics for text-anchored alignment values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::text::block) struct LineRefs {
    ascent: f32,
    descent: f32,
    x_height: f32,
    font_size: f32,
}

/// Collects the reference metrics from the line's glyph runs; a box-only line
/// falls back to the line box itself, degrading the text-anchored values to
/// baseline-anchored behavior.
pub(in crate::text::block) fn line_refs(line: &Line<'_, TextBrush>) -> LineRefs {
    let mut refs = LineRefs {
        ascent: 0.0,
        descent: 0.0,
        x_height: 0.0,
        font_size: 0.0,
    };
    let mut saw_run = false;
    for run in line.runs() {
        saw_run = true;
        let metrics = run.metrics();
        refs.ascent = refs.ascent.max(metrics.ascent);
        refs.descent = refs.descent.max(metrics.descent);
        refs.x_height = refs.x_height.max(metrics.x_height.unwrap_or(0.0));
        refs.font_size = refs.font_size.max(run.font_size());
    }
    if !saw_run {
        let metrics = line.metrics();
        refs.ascent = metrics.ascent;
        refs.descent = metrics.descent;
    }
    refs
}

/// The placement table: the box's top edge, overriding parley's
/// bottom-on-baseline position for every value.
pub(in crate::text::block) fn box_top(
    spec: &InlineBoxSpec,
    metrics: &LineMetrics,
    refs: LineRefs,
) -> f32 {
    let height = spec.size.height;
    let above_baseline = spec.baseline.unwrap_or(height);
    match spec.vertical_align {
        VerticalAlign::Baseline => metrics.baseline - above_baseline,
        VerticalAlign::Sub => {
            metrics.baseline + SUB_BASELINE_FACTOR * refs.font_size - above_baseline
        }
        VerticalAlign::Super => {
            metrics.baseline - SUPER_BASELINE_FACTOR * refs.font_size - above_baseline
        }
        VerticalAlign::Length(raise) => metrics.baseline - raise - above_baseline,
        VerticalAlign::Percent(fraction) => {
            metrics.baseline - fraction * metrics.line_height - above_baseline
        }
        VerticalAlign::Top => metrics.block_min_coord,
        VerticalAlign::TextTop => metrics.baseline - refs.ascent,
        VerticalAlign::Middle | VerticalAlign::Center => {
            metrics.baseline - refs.x_height / 2.0 - height / 2.0
        }
        VerticalAlign::Bottom => metrics.block_max_coord - height,
        VerticalAlign::TextBottom => metrics.baseline + refs.descent - height,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;
    use crate::geometry::Size;

    fn spec(vertical_align: VerticalAlign, baseline: Option<f32>) -> InlineBoxSpec {
        InlineBoxSpec {
            id: 1,
            size: Size::new(10.0, 30.0),
            baseline,
            vertical_align,
        }
    }

    #[test]
    fn contribution_splits_baseline_anchored_from_extent_anchored_values() {
        assert_eq!(
            line_contribution(&spec(VerticalAlign::Baseline, None)),
            30.0,
            "no baseline: the bottom edge sits on the baseline, all height above",
        );
        assert_eq!(
            line_contribution(&spec(VerticalAlign::Baseline, Some(24.0))),
            24.0,
        );
        assert_eq!(
            line_contribution(&spec(VerticalAlign::Sub, Some(24.0))),
            24.0
        );
        assert_eq!(
            line_contribution(&spec(VerticalAlign::Super, Some(24.0))),
            24.0,
            "the raise is placement-only; the contribution stays baseline-shaped",
        );
        assert_eq!(
            line_contribution(&spec(VerticalAlign::Length(6.0), Some(24.0))),
            30.0,
        );
        assert_eq!(
            line_contribution(&spec(VerticalAlign::Length(-6.0), Some(24.0))),
            24.0,
        );
        for value in [
            VerticalAlign::Top,
            VerticalAlign::TextTop,
            VerticalAlign::Middle,
            VerticalAlign::Bottom,
            VerticalAlign::TextBottom,
            VerticalAlign::Percent(0.5),
            VerticalAlign::Center,
        ] {
            assert_eq!(line_contribution(&spec(value, Some(24.0))), 30.0);
        }
    }
}
