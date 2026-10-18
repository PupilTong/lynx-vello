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

use super::style::IndentKeywords;
use crate::style::TextBrush;

/// The `tab-size` every preserved tab uses: the property's initial value, in
/// spaces.
pub(in crate::text) const TAB_SIZE: f32 = 8.0;

/// Android's tab stop interval, in px: `Layout.TAB_INCREMENT`.
pub(in crate::text) const NATIVE_TAB_INCREMENT: f32 = 20.0;

/// Where a paragraph's tab stops are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text) enum TabStops {
    /// css-text-3: every [`TAB_SIZE`] space advances, a stop closer than
    /// `0.5ch` passed.
    Css,
    /// Native Lynx: every [`NATIVE_TAB_INCREMENT`] px, the first stop strictly
    /// past the tab's position.
    Native,
}

/// How many times one layout re-shapes to settle its tab advances.
///
/// Under `nowrap` one is always enough: [`resolve`] walks each line in order
/// using the advances it has just decided, so a tab's position never depends
/// on a stale one. A re-shape can only move a *soft* break, which needs a
/// further round; the bound stops a pathological paragraph whose breaks and
/// tab advances keep trading places, and the last round's breaks — the ones
/// committed — are always taken against the last shaping.
pub(in crate::text) const MAX_ROUNDS: u32 = 3;

/// Two advances this close are the same advance: the walk sums floats in the
/// same order every time, so a settled paragraph reproduces its spacing to
/// well within this.
const SETTLED: f32 = 1.0e-3;

/// The shift from `x` to the next tab stop of `size`, passing a stop closer
/// than `min`. A non-positive tab size renders the tab as nothing (§4.1.2).
pub(in crate::text) fn advance_to_stop(x: f32, size: f32, min: f32) -> f32 {
    if size.is_nan() || size <= 0.0 {
        return 0.0;
    }
    let columns = x / size;
    // A position a rounding error short of a stop is on that stop, so the
    // tab moves a whole tab size rather than the error.
    let nearest = columns.round();
    let passed = if (columns - nearest).abs() < 1.0e-4 {
        nearest
    } else {
        columns.floor()
    };
    let shift = (passed + 1.0).mul_add(size, -x);
    if shift < min { shift + size } else { shift }
}

/// The shift from `h`, a position measured from the start of the line's text,
/// to Android's next tab stop: `nextTab(h) = floor((h + 20) / 20) * 20`, which
/// is strictly greater than `h`. No tolerance: a position a rounding error
/// short of a stop moves by that error, as Android's float arithmetic does.
pub(in crate::text) fn native_tab_shift(h: f32) -> f32 {
    ((h + NATIVE_TAB_INCREMENT) / NATIVE_TAB_INCREMENT).floor() * NATIVE_TAB_INCREMENT - h
}

/// The extra spacing each preserved tab needs at the positions the committed
/// lines give it, or `None` when every tab already has what it needs.
///
/// `tabs` are the tabs' bytes, ascending; `spacing` the extra spacing each
/// was last shaped with, `min` its `0.5ch`. A tab on no committed line — past
/// a `text-maxline` clamp — keeps its spacing. A tab's own cluster advance is
/// one space plus the run's letter-spacing plus its current extra, so the
/// tab size is read back off the layout without a second measurement.
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
    for (index, line) in layout.lines().enumerate() {
        // An indented line starts at the `text-indent` parley applied to it.
        // css-text-3 measures stops from the content edge, so the indent
        // counts toward the position; Android measures them from the start
        // of the line's text, after its leading margin, so it does not.
        let mut x = match stops {
            TabStops::Css => applied_indent(layout, index, indent, keywords),
            TabStops::Native => 0.0,
        };
        // A shaping run is split into one glyph run per style; its clusters
        // are walked once, from its first glyph run.
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
                        let byte =
                            u32::try_from(cluster.text_range().start).expect("text fits u32");
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

/// The indent parley applied to line `index` of `layout`: `amount` on the
/// first line, and with `each-line` on every line after a forced break, the
/// two sets swapped by `hanging` — parley's own rule
/// (`BreakLines::resolve_indent`), read back from the committed lines.
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
    #![allow(clippy::float_cmp)] // Exact binary fractions throughout.

    use super::*;

    #[test]
    fn a_tab_shifts_to_the_next_stop_past_its_position() {
        // A tab size of 80 (eight 10 px spaces).
        assert_eq!(advance_to_stop(0.0, 80.0, 5.0), 80.0);
        assert_eq!(advance_to_stop(30.0, 80.0, 5.0), 50.0);
        assert_eq!(advance_to_stop(80.0, 80.0, 5.0), 80.0);
        assert_eq!(advance_to_stop(110.0, 80.0, 5.0), 50.0);
        // A first-line `text-indent` can put the line start before the
        // content edge.
        assert_eq!(advance_to_stop(-20.0, 80.0, 5.0), 20.0);
    }

    #[test]
    fn a_stop_closer_than_half_a_ch_is_passed() {
        assert_eq!(advance_to_stop(76.0, 80.0, 5.0), 84.0);
        assert_eq!(advance_to_stop(75.0, 80.0, 5.0), 5.0);
        // A rounding error short of a stop is on the stop.
        assert_eq!(advance_to_stop(80.0 - 1.0e-6, 80.0, 5.0), 80.0);
    }

    #[test]
    fn a_native_tab_moves_to_the_next_multiple_of_twenty_strictly_past() {
        assert_eq!(native_tab_shift(0.0), 20.0);
        assert_eq!(native_tab_shift(10.0), 10.0);
        // On a stop: the next one.
        assert_eq!(native_tab_shift(20.0), 20.0);
        assert_eq!(native_tab_shift(35.0), 5.0);
        // No tolerance: just short of a stop moves to that stop, by the
        // remainder.
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
