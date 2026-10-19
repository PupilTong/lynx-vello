//! Source-range geometry over a laid-out paragraph: the boxes a source range
//! occupies and the source unit drawn under a point, for both paragraph
//! kinds.
//!
//! Each consumer hands in the layout its glyphs are drawn from, its reported
//! lines' source ranges and line boxes, and where the content ends in that
//! layout's text; the walk over clusters is the core's
//! ([`Paragraph::for_each_extent`]), so the two queries — and the two
//! paragraph kinds — cannot disagree about which unit a pixel belongs to.

use parley::Layout;

use super::{Additions, LineGeometry, Paragraph};
use crate::geometry::{Point, Rect, Size};
use crate::style::TextBrush;

/// One reported line's source range: its first unit, and the end of its
/// visible content — the collapsible whitespace hanging at its end belongs
/// to no line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text) struct LineSpan {
    pub(in crate::text) start: u32,
    pub(in crate::text) end: u32,
}

/// Where a consumer's displayed layout keeps its content: the layout, its
/// reported lines index for index with the layout's, and the byte and box
/// slot the content ends at in the layout's text.
pub(in crate::text) struct Displayed<'a> {
    pub(in crate::text) layout: &'a Layout<TextBrush>,
    pub(in crate::text) lines: &'a [LineSpan],
    pub(in crate::text) geometry: &'a [LineGeometry],
    pub(in crate::text) cut_byte: usize,
    pub(in crate::text) content_slots: usize,
}

impl<S, A: Additions> Paragraph<S, A> {
    /// The boxes the source range `[start, end)` occupies in `displayed`: one
    /// per line it shows content on, from the leftmost selected cluster or
    /// atomic box to the rightmost, as tall as the line box. `start == end`
    /// answers one zero-width box at the origin as tall as the line holding
    /// `start`; `None` when the range is inverted or runs past the source,
    /// or, for an empty range, when there is no line.
    pub(in crate::text) fn range_boxes_in(
        &self,
        displayed: &Displayed<'_>,
        start: u32,
        end: u32,
    ) -> Option<Vec<Rect<f32>>> {
        if start > end || end > self.content.block.map.source_len() {
            return None;
        }
        let lines = displayed.lines;
        let geometry = displayed.geometry;
        if start == end {
            let line = lines
                .iter()
                .rposition(|line| line.start <= start)
                .or_else(|| (!lines.is_empty()).then_some(0))?;
            return Some(vec![Rect::new(
                Point::new(0.0, 0.0),
                Size::new(0.0, geometry[line].height),
            )]);
        }

        let mut boxes = Vec::new();
        for ((line, span), place) in displayed.layout.lines().zip(lines).zip(geometry) {
            // Clipping to the line's own range is what drops its trailing
            // collapsible whitespace, which the range excludes.
            let low = start.max(span.start);
            let high = end.min(span.end);
            if low >= high {
                continue;
            }
            let mut extent: Option<(f32, f32)> = None;
            self.for_each_extent(
                &line,
                displayed.cut_byte,
                displayed.content_slots,
                |units, left, right| {
                    if units.start < high && units.end > low {
                        extent =
                            Some(extent.map_or((left, right), |(min, max)| {
                                (min.min(left), max.max(right))
                            }));
                    }
                },
            );
            if let Some((left, right)) = extent {
                boxes.push(Rect::new(
                    Point::new(left, place.top),
                    Size::new(right - left, place.height),
                ));
            }
        }
        Some(boxes)
    }

    /// The source unit of the cluster or atomic box under `point` in
    /// `displayed`: the inverse of [`Self::range_boxes_in`] over one unit. A
    /// line owns its whole line box, half-open at the bottom; a cluster owns
    /// `[left, left + advance)` and answers its first unit; `None` off
    /// content.
    pub(in crate::text) fn index_at_point_in(
        &self,
        displayed: &Displayed<'_>,
        point: Point<f32>,
    ) -> Option<u32> {
        let (line, span) = displayed
            .layout
            .lines()
            .zip(displayed.lines)
            .zip(displayed.geometry)
            .find(|(_, place)| place.top <= point.y && point.y < place.top + place.height)
            .map(|(pair, _)| pair)?;
        let mut hit = None;
        self.for_each_extent(
            &line,
            displayed.cut_byte,
            displayed.content_slots,
            |units, left, right| {
                if hit.is_none()
                    && left <= point.x
                    && point.x < right
                    && units.start < span.end
                    && units.end > span.start
                {
                    hit = Some(units.start.max(span.start));
                }
            },
        );
        hit
    }
}
