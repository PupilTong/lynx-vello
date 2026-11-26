//! Preserved tabs: the advance to the next tab stop.
//!
//! css-text-3 §4.1.2 `#white-space-phase-2`, rule 2 (css-text-4 keeps it): a
//! preserved tab is rendered as a horizontal shift that lines up the start
//! edge of the next glyph with the next tab stop, skipping to the stop after
//! it when the shift would be less than `0.5ch`. Tab stops sit at multiples of
//! the tab size from the starting content edge, and the tab size is
//! `tab-size` times the advance of the space character including
//! `letter-spacing` (§4.2 `#tab-size-property`; Lynx has no `word-spacing`).
//!
//! The normalizer emits each preserved tab as one space with a span of its
//! own; this module decides how much `letter-spacing` that span adds so the
//! space's advance becomes the shift. The shift depends on where the tab sits
//! on its line, which depends on the line breaks, so it is settled at break
//! time against the lines parley actually committed: the block breaks,
//! [`resolve`] reads each tab's position, and when an advance moved the block
//! re-shapes and breaks again. A paragraph without preserved tabs never gets
//! past the first emptiness check.
//!
//! Recorded simplifications:
//!
//! - `tab-size` is fixed at its initial value, [`TAB_SIZE`]: the fork does not parse the property
//!   under `lynx` (`vendor/stylo/style/tests/lynx_disabled_properties.rs`).
//! - The space advance and `ch` are the tab's own run's font, where the spec names the font of the
//!   tab's nearest block container; they differ only when a nested text element changes the font of
//!   the run holding the tab.
//! - Positions are accumulated over a line in visual order from its left edge, so a tab in
//!   right-to-left text measures its stops from the left.
//! - Tabs inside the inline-truncation content keep the plain space advance: that content is laid
//!   in at a cut, not at a position of its own.
//! - A tab is shaped as a space, so `text-align: justify` stretches it like one; stops are decided
//!   before alignment.
//!
//! A native Lynx `<text>` places its stops differently ([`TabStops::Native`]):
//! Android's `Layout` puts one every `TAB_INCREMENT` = 20px and advances a tab
//! to the first stop strictly past its position, with no minimum shift (AOSP
//! `Layout.java:3714`, minikin's `nextTab`), by the exact formula
//! [`native_tab_shift`] carries. The position is measured from the start of
//! the line's text, after the leading indent: Lynx applies `text-indent` as
//! a `LeadingMarginSpan.Standard(indent, 0)`
//! (`lynx/platform/android/lynx_android/src/main/java/com/lynx/tasm/behavior/shadow/text/
//! TextShadowNode.java:295-298`), a margin outside the text `TextLine` measures a tab's position
//! in, so an indented line's stops sit at the indent plus multiples of 20. iOS and
//! Harmony leave tabs to their platform text engines; this follows Android.

use parley::{BreakReason, Layout, PositionedLayoutItem};

use super::shape::Objects;
use super::style::IndentKeywords;
use crate::style::TextBrush;

pub(in crate::text) const TAB_SIZE: f32 = 8.0;

pub(in crate::text) const NATIVE_TAB_INCREMENT: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text) enum TabStops {
    Css,
    Native,
}

pub(in crate::text) const MAX_ROUNDS: u32 = 3;

const SETTLED: f32 = 1.0e-3;

pub(in crate::text) fn advance_to_stop(x: f32, size: f32, min: f32) -> f32 {
    if size.is_nan() || size <= 0.0 {
        return 0.0;
    }
    let columns = x / size;
    let nearest = columns.round();
    let passed = if (columns - nearest).abs() < 1.0e-4 {
        nearest
    } else {
        columns.floor()
    };
    let shift = (passed + 1.0).mul_add(size, -x);
    if shift < min { shift + size } else { shift }
}

pub(in crate::text) fn native_tab_shift(h: f32) -> f32 {
    ((h + NATIVE_TAB_INCREMENT) / NATIVE_TAB_INCREMENT).floor() * NATIVE_TAB_INCREMENT - h
}

pub(in crate::text) fn resolve(
    layout: &Layout<TextBrush>,
    tabs: &[u32],
    spacing: &[f32],
    min: &[f32],
    indent: f32,
    keywords: IndentKeywords,
    stops: TabStops,
) -> Option<Vec<f32>> {
    if tabs.is_empty() {
        return None;
    }
    debug_assert_eq!(tabs.len(), spacing.len());
    debug_assert_eq!(tabs.len(), min.len());
    let mut wanted = spacing.to_vec();
    let objects = Objects::of(layout);
    for (index, line) in layout.lines().enumerate() {
        let mut x = match stops {
            TabStops::Css => applied_indent(layout, index, indent, keywords),
            TabStops::Native => 0.0,
        };
        let mut walked_run = None;
        for item in line.items() {
            match item {
                PositionedLayoutItem::GlyphRun(glyph_run) => {
                    let run = glyph_run.run();
                    if walked_run == Some(run.index()) {
                        continue;
                    }
                    walked_run = Some(run.index());
                    for cluster in run.visual_clusters() {
                        let byte = objects.text_byte(cluster.text_range().start);
                        let Ok(tab) = tabs.binary_search(&byte) else {
                            x += cluster.advance();
                            continue;
                        };
                        let space = cluster.advance() - spacing[tab];
                        let shift = match stops {
                            TabStops::Css => advance_to_stop(x, TAB_SIZE * space, min[tab]),
                            TabStops::Native => native_tab_shift(x),
                        };
                        wanted[tab] = shift - space;
                        x += shift;
                    }
                }
                PositionedLayoutItem::InlineBox(inline_box) => x += inline_box.width,
            }
        }
    }
    let settled = wanted
        .iter()
        .zip(spacing)
        .all(|(wanted, held)| (wanted - held).abs() < SETTLED);
    (!settled).then_some(wanted)
}

pub(in crate::text) fn applied_indent(
    layout: &Layout<TextBrush>,
    index: usize,
    amount: f32,
    keywords: IndentKeywords,
) -> f32 {
    let starts_scope = index == 0
        || (keywords.each_line
            && layout
                .get(index - 1)
                .is_some_and(|previous| previous.break_reason() == BreakReason::Explicit));
    if starts_scope == keywords.hanging {
        0.0
    } else {
        amount
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;

    #[test]
    fn a_tab_shifts_to_the_next_stop_past_its_position() {
        assert_eq!(advance_to_stop(0.0, 80.0, 5.0), 80.0);
        assert_eq!(advance_to_stop(30.0, 80.0, 5.0), 50.0);
        assert_eq!(advance_to_stop(80.0, 80.0, 5.0), 80.0);
        assert_eq!(advance_to_stop(110.0, 80.0, 5.0), 50.0);
        assert_eq!(advance_to_stop(-20.0, 80.0, 5.0), 20.0);
    }

    #[test]
    fn a_stop_closer_than_half_a_ch_is_passed() {
        assert_eq!(advance_to_stop(76.0, 80.0, 5.0), 84.0);
        assert_eq!(advance_to_stop(75.0, 80.0, 5.0), 5.0);
        assert_eq!(advance_to_stop(80.0 - 1.0e-6, 80.0, 5.0), 80.0);
    }

    #[test]
    fn a_native_tab_moves_to_the_next_multiple_of_twenty_strictly_past() {
        assert_eq!(native_tab_shift(0.0), 20.0);
        assert_eq!(native_tab_shift(10.0), 10.0);
        assert_eq!(native_tab_shift(20.0), 20.0);
        assert_eq!(native_tab_shift(35.0), 5.0);
        let short = 20.0 - 1.0e-3;
        assert_eq!(native_tab_shift(short), 20.0 - short);
    }

    #[test]
    fn a_zero_or_negative_tab_size_renders_nothing() {
        assert_eq!(advance_to_stop(30.0, 0.0, 5.0), 0.0);
        assert_eq!(advance_to_stop(30.0, -8.0, 5.0), 0.0);
        assert_eq!(advance_to_stop(30.0, f32::NAN, 5.0), 0.0);
    }
}
