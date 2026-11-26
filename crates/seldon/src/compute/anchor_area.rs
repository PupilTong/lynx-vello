//! css-anchor-position-1 §3.1 `position-area`: the 3×3 position-area grid
//! (§3.1.1), the region a value selects (§3.1.2), and the area-specific
//! default alignment (§4.1).
//!
//! The fork's computed `PositionArea` already knows how to turn itself into
//! a pair of physical keywords (`PositionArea::to_physical`) and a physical
//! keyword into §4.1's alignment (`PositionAreaKeyword::to_self_alignment`);
//! this module supplies the writing modes — `horizontal-tb`, with `direction`
//! the only thing that varies — and the geometry.

use stylo::logical_geometry::{LogicalAxis, PhysicalAxis, WritingMode};
use stylo::values::computed::{PositionArea, PositionAreaKeyword};
use stylo::values::specified::align::AlignFlags;
use stylo::values::specified::position::{PositionAreaAxis, PositionAreaTrack};

use crate::geometry::{Edges, Point, Rect, Size};

#[derive(Debug, Clone, Copy)]
pub(super) struct AreaKeywords {
    horizontal: PositionAreaKeyword,
    vertical: PositionAreaKeyword,
}

#[inline]
fn writing_mode(rtl: bool) -> WritingMode {
    if rtl {
        WritingMode::RTL | WritingMode::INLINE_REVERSED
    } else {
        WritingMode::empty()
    }
}

impl AreaKeywords {
    pub(super) fn physical(value: PositionArea, containing_rtl: bool, self_rtl: bool) -> Self {
        let physical = value.to_physical(writing_mode(containing_rtl), writing_mode(self_rtl));
        let (mut horizontal, mut vertical) = (physical.first, physical.second);
        if horizontal.axis() == PositionAreaAxis::Vertical
            || vertical.axis() == PositionAreaAxis::Horizontal
        {
            core::mem::swap(&mut horizontal, &mut vertical);
        }
        debug_assert!(
            !matches!(horizontal.axis(), PositionAreaAxis::Vertical)
                && !matches!(vertical.axis(), PositionAreaAxis::Horizontal),
            "a physical <position-area> has one keyword per axis"
        );
        Self {
            horizontal,
            vertical,
        }
    }

    #[inline]
    fn keyword(self, axis: PhysicalAxis) -> PositionAreaKeyword {
        match axis {
            PhysicalAxis::Horizontal => self.horizontal,
            PhysicalAxis::Vertical => self.vertical,
        }
    }
}

fn tracks(keyword: PositionAreaKeyword, lines: [f32; 4]) -> (f32, f32) {
    let (start, end) = track_lines(keyword);
    (lines[start], lines[end])
}

fn track_lines(keyword: PositionAreaKeyword) -> (usize, usize) {
    match keyword.track().unwrap_or(PositionAreaTrack::SpanAll) {
        PositionAreaTrack::Start => (0, 1),
        PositionAreaTrack::SpanStart => (0, 2),
        PositionAreaTrack::Center => (1, 2),
        PositionAreaTrack::SpanEnd => (1, 3),
        PositionAreaTrack::End => (2, 3),
        PositionAreaTrack::SpanAll => (0, 3),
    }
}

fn anchor_line(index: usize, origin: f32, containing: f32, anchor: (f32, f32)) -> bool {
    match index {
        0 => anchor.0 < origin,
        3 => anchor.1 > origin + containing,
        _ => true,
    }
}

#[inline]
fn grid_lines(origin: f32, containing: f32, anchor_start: f32, anchor_end: f32) -> [f32; 4] {
    [
        origin.min(anchor_start),
        anchor_start,
        anchor_end,
        (origin + containing).max(anchor_end),
    ]
}

pub(super) fn position_area_region(
    keywords: AreaKeywords,
    original: Rect<f32>,
    anchor: Rect<f32>,
) -> Rect<f32> {
    let (left, right) = tracks(
        keywords.horizontal,
        grid_lines(
            original.origin.x,
            original.size.width,
            anchor.origin.x,
            anchor.origin.x + anchor.size.width,
        ),
    );
    let (top, bottom) = tracks(
        keywords.vertical,
        grid_lines(
            original.origin.y,
            original.size.height,
            anchor.origin.y,
            anchor.origin.y + anchor.size.height,
        ),
    );
    Rect::new(Point::new(left, top), Size::new(right - left, bottom - top))
}

pub(super) fn position_area_carried(
    keywords: AreaKeywords,
    original: Rect<f32>,
    anchor: Rect<f32>,
) -> Edges<bool> {
    let (left, right) = track_lines(keywords.horizontal);
    let (top, bottom) = track_lines(keywords.vertical);
    let horizontal = |index| {
        anchor_line(
            index,
            original.origin.x,
            original.size.width,
            (anchor.origin.x, anchor.origin.x + anchor.size.width),
        )
    };
    let vertical = |index| {
        anchor_line(
            index,
            original.origin.y,
            original.size.height,
            (anchor.origin.y, anchor.origin.y + anchor.size.height),
        )
    };
    Edges {
        left: horizontal(left),
        right: horizontal(right),
        top: vertical(top),
        bottom: vertical(bottom),
    }
}

pub(super) fn area_default_alignment(
    keywords: AreaKeywords,
    axis: PhysicalAxis,
    auto_insets: (bool, bool),
    containing_rtl: bool,
) -> AlignFlags {
    match auto_insets {
        (true, false) => {
            return AlignFlags::UNSAFE
                | match axis {
                    PhysicalAxis::Horizontal => AlignFlags::RIGHT,
                    PhysicalAxis::Vertical => AlignFlags::END,
                };
        }
        (false, true) => {
            return AlignFlags::UNSAFE
                | match axis {
                    PhysicalAxis::Horizontal => AlignFlags::LEFT,
                    PhysicalAxis::Vertical => AlignFlags::START,
                };
        }
        _ => {}
    }
    let logical = match axis {
        PhysicalAxis::Horizontal => LogicalAxis::Inline,
        PhysicalAxis::Vertical => LogicalAxis::Block,
    };
    keywords
        .keyword(axis)
        .to_self_alignment(logical, &writing_mode(containing_rtl))
        .unwrap_or(AlignFlags::ANCHOR_CENTER)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn area(first: PositionAreaKeyword, second: PositionAreaKeyword) -> PositionArea {
        PositionArea { first, second }
    }

    #[test]
    fn keywords_resolve_to_one_per_physical_axis() {
        use PositionAreaKeyword as K;
        let cases = [
            (area(K::Top, K::None), false, (K::SpanAll, K::Top)),
            (area(K::Left, K::None), false, (K::Left, K::SpanAll)),
            (area(K::Center, K::Top), false, (K::Center, K::Top)),
            (area(K::Center, K::SpanAll), false, (K::SpanAll, K::Center)),
            (area(K::Start, K::End), false, (K::Right, K::Top)),
            (area(K::Start, K::End), true, (K::Left, K::Top)),
            (area(K::SpanAll, K::None), false, (K::SpanAll, K::SpanAll)),
        ];
        for (value, rtl, (horizontal, vertical)) in cases {
            let keywords = AreaKeywords::physical(value, rtl, rtl);
            assert_eq!(
                (keywords.horizontal, keywords.vertical),
                (horizontal, vertical),
                "{value:?} rtl={rtl}"
            );
        }
    }

    #[test]
    #[allow(clippy::float_cmp, reason = "exact sums of small integers")]
    fn lines_extend_to_an_anchor_outside_the_containing_block() {
        assert_eq!(
            grid_lines(0.0, 300.0, 100.0, 200.0),
            [0.0, 100.0, 200.0, 300.0]
        );
        assert_eq!(
            grid_lines(0.0, 300.0, -50.0, 20.0),
            [-50.0, -50.0, 20.0, 300.0]
        );
        assert_eq!(
            grid_lines(0.0, 300.0, 280.0, 350.0),
            [0.0, 280.0, 350.0, 350.0]
        );
    }

    #[test]
    fn carried_lines_are_the_anchors_edges() {
        use PositionAreaKeyword as K;
        let keywords = |horizontal, vertical| AreaKeywords {
            horizontal,
            vertical,
        };
        let block = Rect::new(Point::ZERO, Size::new(300.0, 300.0));
        let anchor = Rect::new(Point::new(100.0, 0.0), Size::new(100.0, 50.0));
        let carried = position_area_carried(keywords(K::Center, K::Bottom), block, anchor);
        assert_eq!(
            (carried.left, carried.right, carried.top, carried.bottom),
            (true, true, true, false),
            "the bottom track's top line is the anchor's bottom edge"
        );
        let carried = position_area_carried(keywords(K::SpanAll, K::Top), block, anchor);
        assert_eq!(
            (carried.left, carried.right, carried.top, carried.bottom),
            (false, false, false, true),
            "the anchor's top at the block's top: line 0 is the block's"
        );
        let past = Rect::new(Point::new(280.0, -20.0), Size::new(70.0, 50.0));
        let carried = position_area_carried(keywords(K::Right, K::Top), block, past);
        assert_eq!(
            (carried.left, carried.right, carried.top, carried.bottom),
            (true, true, true, true),
            "an anchor past both outer lines carries them"
        );
    }
}
