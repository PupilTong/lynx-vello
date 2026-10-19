//! Vertical alignment of atomic boxes within their lines, in parley's
//! vocabulary.
//!
//! Parley 0.12 aligns an inline box itself: CSS 2.2 §10.8.1 line boxes, with
//! `vertical-align` as css-inline-3's `alignment-baseline` plus
//! `baseline-shift`. A box sits against the span that contains it, which for
//! every box a `StyleRunBuilder` takes is the root inline box — parley's
//! style 0, the paragraph's strut — so `text-top`, `text-bottom`, `middle`,
//! `sub` and `super` read the root's font, and `top` and `bottom` the line
//! box. Parley also places the box: `PositionedInlineBox::y` is its top edge.

use parley::{Layout, VerticalAlign as ParleyVerticalAlign, WhiteSpaceCollapse};

use super::shape::{self, ShapeSpan};
use super::style::{BreakStyle, LineHeight, RunStyle, VerticalAlign};
use crate::style::TextBrush;
use crate::text::TextContext;

/// The value parley aligns a box with. A percentage raises the box by that
/// fraction of `percent_basis` (`percent_basis`).
pub(in crate::text) fn parley_vertical_align(
    value: VerticalAlign,
    percent_basis: f32,
) -> ParleyVerticalAlign {
    match value {
        VerticalAlign::Baseline => ParleyVerticalAlign::BASELINE,
        VerticalAlign::Sub => ParleyVerticalAlign::SUB,
        VerticalAlign::Super => ParleyVerticalAlign::SUPER,
        VerticalAlign::Top => ParleyVerticalAlign::TOP,
        VerticalAlign::TextTop => ParleyVerticalAlign::TEXT_TOP,
        VerticalAlign::Middle | VerticalAlign::Center => ParleyVerticalAlign::MIDDLE,
        VerticalAlign::Bottom => ParleyVerticalAlign::BOTTOM,
        VerticalAlign::TextBottom => ParleyVerticalAlign::TEXT_BOTTOM,
        VerticalAlign::Length(raise) => ParleyVerticalAlign::length(raise),
        VerticalAlign::Percent(fraction) => ParleyVerticalAlign::length(fraction * percent_basis),
    }
}

/// What a `vertical-align` percentage is a fraction of: the root inline
/// box's `line-height`.
///
/// CSS takes the percentage of the aligned element's own `line-height`; an
/// atomic box here carries no `line-height` of its own, so the root's — the
/// value its content inherits — stands in. `normal` is read as the root's
/// first available font's ascent plus descent, without its line gap, which
/// parley keeps to itself. `root` is the strut's style, or the run that
/// stands in for it; with neither the basis is zero.
pub(in crate::text) fn percent_basis(layout: &Layout<TextBrush>, root: Option<&RunStyle>) -> f32 {
    match root.map(|style| (style.line_height, style.font_size)) {
        Some((LineHeight::Px(px), _)) => px,
        Some((LineHeight::Number(factor), font_size)) => factor * font_size,
        Some((LineHeight::Normal, _)) => layout
            .root_span_metrics()
            .map_or(0.0, |metrics| metrics.ascent + metrics.descent),
        None => 0.0,
    }
}

/// The used value of `style`'s `line-height` (CSS 2.2 §10.8.1): a length as
/// given, a number times the font size, and `normal` the height parley gives
/// a line box of the style's first available font alone — its ascent,
/// descent and line gap, as Chrome takes it.
///
/// What a `vertical-align` percentage on an element is a fraction of, the
/// element's own `line-height` (§10.8.1). [`VerticalAlign::Percent`] on an
/// atom is taken of the root inline box's instead (`percent_basis`), so a
/// host that knows the atom's style resolves the percentage into a
/// [`VerticalAlign::Length`] with this first.
pub fn used_line_height(context: &mut TextContext, style: &RunStyle) -> f32 {
    match style.line_height {
        LineHeight::Px(px) => px,
        LineHeight::Number(factor) => factor * style.font_size,
        LineHeight::Normal => {
            const PROBE: &str = "0";
            let spans = [ShapeSpan {
                bytes: 0..PROBE.len(),
                style,
                breaking: BreakStyle::default(),
                hanging: WhiteSpaceCollapse::Collapse,
                source: (),
                extra_spacing: 0.0,
            }];
            shape::with_probe(context, PROBE, &spans, Vec::new(), |layout| {
                layout.break_all_lines(None);
                layout
                    .lines()
                    .next()
                    .map_or(0.0, |line| line.metrics().line_height)
            })
            .unwrap_or(0.0)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;

    #[test]
    fn every_value_maps_onto_parley_s_vocabulary() {
        for (value, expected) in [
            (VerticalAlign::Baseline, ParleyVerticalAlign::BASELINE),
            (VerticalAlign::Sub, ParleyVerticalAlign::SUB),
            (VerticalAlign::Super, ParleyVerticalAlign::SUPER),
            (VerticalAlign::Top, ParleyVerticalAlign::TOP),
            (VerticalAlign::TextTop, ParleyVerticalAlign::TEXT_TOP),
            (VerticalAlign::Middle, ParleyVerticalAlign::MIDDLE),
            (VerticalAlign::Center, ParleyVerticalAlign::MIDDLE),
            (VerticalAlign::Bottom, ParleyVerticalAlign::BOTTOM),
            (VerticalAlign::TextBottom, ParleyVerticalAlign::TEXT_BOTTOM),
            (VerticalAlign::Length(6.0), ParleyVerticalAlign::length(6.0)),
            (
                VerticalAlign::Percent(0.5),
                ParleyVerticalAlign::length(10.0),
            ),
        ] {
            assert_eq!(parley_vertical_align(value, 20.0), expected, "{value:?}");
        }
    }
}
