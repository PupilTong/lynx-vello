//! css-scroll-snap-1: snap positions from `scroll-snap-type`,
//! `scroll-snap-align`, `scroll-snap-stop`, `scroll-padding` and
//! `scroll-margin`, and the two ways a scroll settles onto them.
//!
//! The document computes a scroll container's snap positions from live
//! layout ([`Document::snap_positions`]) and the paint build copies them
//! into the committed frame's scroll-slot table, so the runtime's painter
//! snaps against the same numbers without the document. The choosing rules
//! ([`SnapAxis::settle`] and [`SnapAxis::step`]) are pure over those
//! numbers; both sides reach them through [`resolve_step`] and
//! [`settle_offset`].
//!
//! What is implemented, by section of css-scroll-snap-1:
//!
//! - §6.1 `scroll-snap-type`: the axis (`block` and `inline` are `y` and `x` — this engine lays out
//!   horizontal-tb only) and `mandatory` or `proximity`. The proximity range is [`PROXIMITY_RATIO`]
//!   of the scrollport on that axis, the ratio Blink uses.
//! - §6.2 `scroll-padding`: the snapport is the scrollport inset by it; `auto` is `0`, since this
//!   engine has no UA-chosen inset. §6.3 `scroll-margin`: a snap area is the border box outset by
//!   it.
//! - §6.4 `scroll-snap-align` per axis, `start`, `end` or `center`, each clamped into the
//!   scrollable range. An area larger than the snapport on an axis is a *range* of valid positions
//!   (§6.2: the scroll position may rest anywhere the snapport is inside the area) rather than a
//!   point.
//! - §6.5 `scroll-snap-stop: always`: an operation cannot pass such a position — one that would
//!   travel past it settles on it instead.
//! - §7 choosing: a drag settles on the position nearest to where it ended. A discrete directed
//!   step (a wheel tick) moves to the next position in its direction under `mandatory`; under
//!   `proximity` it takes its natural end and snaps only when a position is within range.
//! - §6.1's "must rest on a snap position": the runtime settles a container from its own offset
//!   whenever a commit publishes one — initial layout, a relayout that moved areas, a programmatic
//!   scroll — as long as no drag is holding it.
//!
//! Deliberately absent: css-scroll-snap-2's `scrollsnapchange` and
//! `scrollsnapchanging` events (scoped out of the request), snap
//! *animation* (`scroll-behavior` is absent, so a snap is instantaneous),
//! §7's preference for the same element on both axes (axes are chosen
//! independently), and inertial scrolling to feed §7's intended end
//! position from.
//!
//! Snap areas are the elements in the container's DOM subtree whose nearest
//! scroll container on the containing-block chain is this one. The walk
//! does not enter a nested scroll container's subtree, so an out-of-flow box
//! escaping from inside a nested scroller to this container is not found —
//! a recorded approximation. Transforms are ignored, as they are for every
//! measurement in this crate.

use euclid::default::{Point2D, Rect, Size2D, Vector2D};
use hughie::style::CoreStyle;
use stylo::values::computed::{
    Length, NonNegativeLengthPercentageOrAuto, ScrollSnapAxis, ScrollSnapStop, ScrollSnapStrictness,
};
use stylo::values::specified::box_::ScrollSnapAlignKeyword;

use super::{clamp_to, is_scroll_container};
use crate::NodeId;
use crate::layout::{DisplayMode, StyleView, box_parent, display_mode};
use crate::tree::document::Document;

/// The share of the scrollport, per axis, within which `proximity` snaps.
pub const PROXIMITY_RATIO: f32 = 1.0 / 3.0;

/// How strictly a container snaps on an axis (`scroll-snap-type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapStrictness {
    /// The scroll position must rest on a snap position.
    Mandatory,
    /// It snaps only when it comes to rest within the proximity range.
    Proximity,
}

/// One snap position on one axis: the offset range `min..=max` at which
/// the snapport rests aligned with a snap area. A point when the two are
/// equal; a range for an area larger than the snapport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapPoint {
    pub min: f32,
    pub max: f32,
    /// `scroll-snap-stop: always` on the area.
    pub stop: bool,
}

impl SnapPoint {
    #[must_use]
    fn distance(&self, offset: f32) -> f32 {
        if offset < self.min {
            self.min - offset
        } else if offset > self.max {
            offset - self.max
        } else {
            0.0
        }
    }

    #[must_use]
    fn nearest(&self, offset: f32) -> f32 {
        offset.clamp(self.min, self.max)
    }
}

/// One axis's snap data, borrowed from wherever its points live: the
/// document's [`SnapPositions`] or the committed frame's slot table.
#[derive(Debug, Clone, Copy)]
pub struct SnapAxis<'a> {
    pub strictness: SnapStrictness,
    /// Sorted by `min`.
    pub points: &'a [SnapPoint],
}

impl SnapAxis<'_> {
    /// The distance within which `proximity` snaps, for a scrollport of
    /// `extent` on this axis.
    #[must_use]
    pub fn proximity_threshold(extent: f32) -> f32 {
        extent * PROXIMITY_RATIO
    }

    fn nearest(&self, offset: f32) -> Option<SnapPoint> {
        self.points
            .iter()
            .copied()
            .min_by(|a, b| a.distance(offset).total_cmp(&b.distance(offset)))
    }

    /// The first `always` stop strictly between `from` and `to`, in travel
    /// order: the position an operation from `from` to `to` may not pass.
    fn stop_between(&self, from: f32, to: f32) -> Option<f32> {
        if to > from {
            self.points
                .iter()
                .filter(|point| point.stop && point.min > from && point.min < to)
                .map(|point| point.min)
                .min_by(f32::total_cmp)
        } else if to < from {
            self.points
                .iter()
                .filter(|point| point.stop && point.max < from && point.max > to)
                .map(|point| point.max)
                .max_by(f32::total_cmp)
        } else {
            None
        }
    }

    /// Where an operation that began at `start` and ended at `end` settles
    /// (§7, intended end position): the nearest snap position to `end` —
    /// always under `mandatory`, within `threshold` under `proximity` — or
    /// the first `always` stop the operation would have passed.
    #[must_use]
    pub fn settle(&self, start: f32, end: f32, threshold: f32) -> f32 {
        if let Some(stop) = self.stop_between(start, end) {
            return stop;
        }
        let Some(point) = self.nearest(end) else {
            return end;
        };
        match self.strictness {
            SnapStrictness::Mandatory => point.nearest(end),
            SnapStrictness::Proximity if point.distance(end) <= threshold => point.nearest(end),
            SnapStrictness::Proximity => end,
        }
    }

    /// Where a discrete step with a direction and no end position of its own
    /// (§7, intended direction — a wheel tick) lands, or `None` when snapping
    /// has nothing to say and the step keeps its `natural` end. `current` is
    /// where the container stands and `natural` where the step would put it
    /// unsnapped.
    ///
    /// Only positions ahead of `current` in the step's direction count, so a
    /// tick never snaps back behind where it started and a run of small
    /// ticks still makes progress. `mandatory` moves to the next such
    /// position, however far, and stays put when there is none; a `natural`
    /// end that is already a valid position (inside a range) is kept.
    /// `proximity` keeps the natural end unless the next position is within
    /// `threshold` of it. Either way an `always` stop between the two ends
    /// the step there. A `Some` equal to `current` means the container
    /// refuses the step: it stays, and [`resolve_step`] lets the delta
    /// chain on to the container above.
    #[must_use]
    #[allow(
        clippy::float_cmp,
        reason = "an unmoved natural end is the clamped current offset itself, bit for bit"
    )]
    pub fn step(&self, current: f32, natural: f32, threshold: f32) -> Option<f32> {
        if natural == current || self.points.is_empty() {
            return None;
        }
        if let Some(stop) = self.stop_between(current, natural) {
            return Some(stop);
        }
        if self
            .points
            .iter()
            .any(|point| point.distance(natural) == 0.0)
        {
            return Some(natural);
        }
        let next = if natural > current {
            self.points
                .iter()
                .filter(|point| point.min > current)
                .map(|point| point.min)
                .min_by(f32::total_cmp)
        } else {
            self.points
                .iter()
                .filter(|point| point.max < current)
                .map(|point| point.max)
                .max_by(f32::total_cmp)
        };
        match self.strictness {
            SnapStrictness::Mandatory => Some(next.unwrap_or(current)),
            SnapStrictness::Proximity => next.filter(|next| (next - natural).abs() <= threshold),
        }
    }
}

/// One axis's snap positions, owned.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapAxisPositions {
    pub strictness: SnapStrictness,
    /// Sorted by `min`.
    pub points: Vec<SnapPoint>,
}

impl SnapAxisPositions {
    #[must_use]
    pub fn axis(&self) -> SnapAxis<'_> {
        SnapAxis {
            strictness: self.strictness,
            points: &self.points,
        }
    }
}

/// A scroll container's snap positions on the axes it snaps on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapPositions {
    pub x: Option<SnapAxisPositions>,
    pub y: Option<SnapAxisPositions>,
}

impl SnapPositions {
    #[must_use]
    pub fn x(&self) -> Option<SnapAxis<'_>> {
        self.x.as_ref().map(SnapAxisPositions::axis)
    }

    #[must_use]
    pub fn y(&self) -> Option<SnapAxis<'_>> {
        self.y.as_ref().map(SnapAxisPositions::axis)
    }
}

/// What kind of scrolling operation a chain step belongs to (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollKind {
    /// One step of a continuing gesture — a drag. Applied raw; the gesture
    /// settles onto a snap position when it ends.
    Gesture,
    /// A discrete step with a direction and no end position of its own — a
    /// wheel tick. Snaps as it lands.
    Directed,
}

/// Resolves one container's chain step: where the `admitted` delta lands,
/// and how much of it the container absorbs — which is what the chain
/// subtracts before moving outward.
///
/// A gesture step and an unsnapped directed step absorb exactly what they
/// move. A directed step that snapping moved absorbs the whole admitted
/// delta on that axis even when the snap moved it less: the tick was spent
/// on this container, and does not chain on. One snapping refused — a
/// `mandatory` axis with no position ahead — absorbs nothing, so the tick
/// chains on the way any boundary does.
#[must_use]
#[allow(
    clippy::float_cmp,
    reason = "a refused step hands back the current offset itself, bit for bit"
)]
pub fn resolve_step(
    kind: ScrollKind,
    offset: Vector2D<f32>,
    admitted: Vector2D<f32>,
    max: Vector2D<f32>,
    scrollport: Size2D<f32>,
    snap_x: Option<SnapAxis<'_>>,
    snap_y: Option<SnapAxis<'_>>,
) -> (Vector2D<f32>, Vector2D<f32>) {
    let natural = clamp_to(offset + admitted, max);
    let axis = |current: f32, natural: f32, delta: f32, snap: Option<SnapAxis<'_>>, extent: f32| {
        if delta == 0.0 {
            return (current, 0.0);
        }
        if kind == ScrollKind::Directed
            && let Some(snap) = snap
            && let Some(snapped) =
                snap.step(current, natural, SnapAxis::proximity_threshold(extent))
        {
            return (snapped, if snapped == current { 0.0 } else { delta });
        }
        (natural, natural - current)
    };
    let (x, absorbed_x) = axis(offset.x, natural.x, admitted.x, snap_x, scrollport.width);
    let (y, absorbed_y) = axis(offset.y, natural.y, admitted.y, snap_y, scrollport.height);
    (Vector2D::new(x, y), Vector2D::new(absorbed_x, absorbed_y))
}

/// Where a container standing at `offset` settles after a scrolling
/// operation that began at `start` (§7, intended end position), per axis.
/// With `start == offset` this is the at-rest rule: a `mandatory` axis
/// moves to its nearest snap position, a `proximity` one only when within
/// range.
#[must_use]
pub fn settle_offset(
    start: Vector2D<f32>,
    offset: Vector2D<f32>,
    scrollport: Size2D<f32>,
    snap_x: Option<SnapAxis<'_>>,
    snap_y: Option<SnapAxis<'_>>,
) -> Vector2D<f32> {
    let axis = |start: f32, offset: f32, snap: Option<SnapAxis<'_>>, extent: f32| {
        snap.map_or(offset, |snap| {
            snap.settle(start, offset, SnapAxis::proximity_threshold(extent))
        })
    };
    Vector2D::new(
        axis(start.x, offset.x, snap_x, scrollport.width),
        axis(start.y, offset.y, snap_y, scrollport.height),
    )
}

/// The snap position one area contributes on one axis, given the area's
/// and the snapport's extents in the container's scrolling coordinates.
fn snap_point(
    align: ScrollSnapAlignKeyword,
    area: (f32, f32),
    snapport: (f32, f32),
    max: f32,
    stop: bool,
) -> SnapPoint {
    let clamp = |offset: f32| offset.clamp(0.0, max.max(0.0));
    let (area_start, area_end) = area;
    let (port_start, port_end) = snapport;
    if area_end - area_start > port_end - port_start {
        // Larger than the snapport: every offset that keeps the snapport
        // inside the area is valid, whatever the alignment asked for.
        let min = clamp(area_start - port_start);
        let max = clamp(area_end - port_end);
        if min <= max {
            return SnapPoint { min, max, stop };
        }
    }
    let offset = match align {
        ScrollSnapAlignKeyword::Start => area_start - port_start,
        ScrollSnapAlignKeyword::End => area_end - port_end,
        ScrollSnapAlignKeyword::Center => (area_start + area_end - port_start - port_end) / 2.0,
        ScrollSnapAlignKeyword::None => unreachable!("an area with no alignment is not collected"),
    };
    let offset = clamp(offset);
    SnapPoint {
        min: offset,
        max: offset,
        stop,
    }
}

fn scroll_padding(value: NonNegativeLengthPercentageOrAuto, basis: f32) -> f32 {
    match value {
        NonNegativeLengthPercentageOrAuto::Auto => 0.0,
        NonNegativeLengthPercentageOrAuto::LengthPercentage(length) => {
            length.0.resolve(Length::new(basis)).px()
        }
    }
}

impl<T> Document<T> {
    /// This scroll container's snap positions, from the last layout, or
    /// `None` when it is not a scroll container or its `scroll-snap-type`
    /// is `none`. Each returned axis is one the container snaps on, even
    /// when no area contributes a point to it.
    #[must_use]
    pub fn snap_positions(&self, id: NodeId) -> Option<SnapPositions> {
        let node = self.get(id)?;
        let style = node.layout_computed_style()?;
        let snap_type = style.clone_scroll_snap_type();
        let strictness = match snap_type.strictness() {
            ScrollSnapStrictness::None => return None,
            ScrollSnapStrictness::Mandatory => SnapStrictness::Mandatory,
            ScrollSnapStrictness::Proximity => SnapStrictness::Proximity,
        };
        let (snaps_x, snaps_y) = match snap_type.axis() {
            ScrollSnapAxis::X | ScrollSnapAxis::Inline => (true, false),
            ScrollSnapAxis::Y | ScrollSnapAxis::Block => (false, true),
            ScrollSnapAxis::Both => (true, true),
        };
        let scroll_box = self.scroll_box(id)?;
        let port = scroll_box.scrollport;
        let max = scroll_box.max_offset();
        let snapport_x = (
            scroll_padding(style.clone_scroll_padding_left(), port.width),
            port.width - scroll_padding(style.clone_scroll_padding_right(), port.width),
        );
        let snapport_y = (
            scroll_padding(style.clone_scroll_padding_top(), port.height),
            port.height - scroll_padding(style.clone_scroll_padding_bottom(), port.height),
        );

        let mut x = Vec::new();
        let mut y = Vec::new();
        let mut pending: Vec<NodeId> = node.child_ids().iter().rev().copied().collect();
        while let Some(child_id) = pending.pop() {
            let Some(child) = self.get(child_id) else {
                continue;
            };
            let Some(view) = StyleView::try_of(child) else {
                continue;
            };
            let mode = display_mode(view.display());
            if mode == DisplayMode::None {
                continue;
            }
            let values = view.values();
            if mode != DisplayMode::Contents {
                let align = values.clone_scroll_snap_align();
                let wants_x = snaps_x && align.inline() != ScrollSnapAlignKeyword::None;
                let wants_y = snaps_y && align.block() != ScrollSnapAlignKeyword::None;
                if (wants_x || wants_y)
                    && self.nearest_scroll_container(child_id) == Some(id)
                    && let Some(rect) = self.rect_in_scroll_container(child_id, id)
                {
                    let stop = values.clone_scroll_snap_stop() == ScrollSnapStop::Always;
                    let area_x = (
                        rect.min_x() - values.clone_scroll_margin_left().px(),
                        rect.max_x() + values.clone_scroll_margin_right().px(),
                    );
                    let area_y = (
                        rect.min_y() - values.clone_scroll_margin_top().px(),
                        rect.max_y() + values.clone_scroll_margin_bottom().px(),
                    );
                    if wants_x {
                        x.push(snap_point(align.inline(), area_x, snapport_x, max.x, stop));
                    }
                    if wants_y {
                        y.push(snap_point(align.block(), area_y, snapport_y, max.y, stop));
                    }
                }
            }
            if is_scroll_container(values) {
                // Its subtree snaps to it, not to us; the container itself
                // may still be one of our areas.
                continue;
            }
            pending.extend(child.child_ids().iter().rev());
        }
        x.sort_by(|a, b| a.min.total_cmp(&b.min));
        y.sort_by(|a, b| a.min.total_cmp(&b.min));
        let axis = |snaps: bool, points: Vec<SnapPoint>| {
            snaps.then_some(SnapAxisPositions { strictness, points })
        };
        Some(SnapPositions {
            x: axis(snaps_x, x),
            y: axis(snaps_y, y),
        })
    }

    /// Settles `id` after a scrolling operation that began at `start` —
    /// [`settle_offset`] over live geometry — and returns the offset it
    /// rests at. A container that does not snap keeps its offset.
    pub fn settle_scroll(&mut self, id: NodeId, start: Vector2D<f32>) -> Vector2D<f32> {
        let Some(scroll_box) = self.scroll_box(id) else {
            return Vector2D::zero();
        };
        let Some(positions) = self.snap_positions(id) else {
            return scroll_box.offset;
        };
        let settled = settle_offset(
            start,
            scroll_box.offset,
            scroll_box.scrollport,
            positions.x(),
            positions.y(),
        );
        self.scroll_to(id, settled)
    }

    /// The first scroll container above `id` on its containing-block chain.
    fn nearest_scroll_container(&self, id: NodeId) -> Option<NodeId> {
        let mut current = self.scroll_parent(id);
        while let Some(node_id) = current {
            if self.is_scroll_container(node_id) {
                return Some(node_id);
            }
            current = self.scroll_parent(node_id);
        }
        None
    }

    /// `id`'s border box in `container`'s scrolling coordinates: relative to
    /// the container's padding-box origin, unscrolled. `None` when `id` is
    /// not laid out under `container`.
    fn rect_in_scroll_container(&self, id: NodeId, container: NodeId) -> Option<Rect<f32>> {
        let layout = self.rounded_layout(id)?;
        let mut origin = Point2D::new(layout.location.x, layout.location.y);
        let mut current = self.get(id)?;
        loop {
            let ancestor = box_parent(current)?;
            if ancestor.id() == container {
                break;
            }
            let ancestor_layout = self.rounded_layout(ancestor.id())?;
            origin += Vector2D::new(ancestor_layout.location.x, ancestor_layout.location.y);
            current = ancestor;
        }
        let container_layout = self.rounded_layout(container)?;
        origin -= Vector2D::new(container_layout.border.left, container_layout.border.top);
        Some(Rect::new(
            origin,
            Size2D::new(layout.size.width, layout.size.height),
        ))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp, reason = "the expectations are exact offsets")]
mod tests {
    use super::*;
    use crate::StylesheetOrigin;
    use crate::tree::document::tests::device;

    fn point(offset: f32) -> SnapPoint {
        SnapPoint {
            min: offset,
            max: offset,
            stop: false,
        }
    }

    fn stop(offset: f32) -> SnapPoint {
        SnapPoint {
            min: offset,
            max: offset,
            stop: true,
        }
    }

    fn axis(strictness: SnapStrictness, points: &[SnapPoint]) -> SnapAxis<'_> {
        SnapAxis { strictness, points }
    }

    #[test]
    fn settling_picks_the_nearest_position_within_the_strictness() {
        let points = [point(0.0), point(100.0), point(200.0)];
        let mandatory = axis(SnapStrictness::Mandatory, &points);
        let proximity = axis(SnapStrictness::Proximity, &points);

        assert_eq!(mandatory.settle(0.0, 140.0, 33.0), 100.0);
        assert_eq!(mandatory.settle(0.0, 160.0, 33.0), 200.0);
        assert_eq!(proximity.settle(0.0, 140.0, 33.0), 140.0, "out of range");
        assert_eq!(proximity.settle(0.0, 120.0, 33.0), 100.0, "in range");
        assert_eq!(
            axis(SnapStrictness::Mandatory, &[]).settle(0.0, 140.0, 33.0),
            140.0,
            "no positions, no snapping",
        );
    }

    #[test]
    fn a_range_position_is_valid_anywhere_inside_it() {
        let points = [
            point(0.0),
            SnapPoint {
                min: 100.0,
                max: 300.0,
                stop: false,
            },
        ];
        let mandatory = axis(SnapStrictness::Mandatory, &points);
        assert_eq!(mandatory.settle(0.0, 180.0, 0.0), 180.0);
        assert_eq!(mandatory.settle(0.0, 320.0, 0.0), 300.0);
        assert_eq!(mandatory.step(180.0, 220.0, 0.0), Some(220.0));
        assert_eq!(mandatory.step(0.0, 30.0, 0.0), Some(100.0));
    }

    #[test]
    fn an_always_stop_is_never_passed_in_either_direction() {
        let points = [point(0.0), stop(100.0), point(200.0)];
        let mandatory = axis(SnapStrictness::Mandatory, &points);
        assert_eq!(mandatory.settle(0.0, 190.0, 0.0), 100.0);
        assert_eq!(mandatory.settle(200.0, 10.0, 0.0), 100.0);
        assert_eq!(
            mandatory.settle(100.0, 190.0, 0.0),
            200.0,
            "from the stop itself"
        );
        assert_eq!(mandatory.step(0.0, 250.0, 0.0), Some(100.0));
        let proximity = axis(SnapStrictness::Proximity, &points);
        assert_eq!(proximity.settle(0.0, 150.0, 10.0), 100.0);
    }

    #[test]
    fn a_directed_step_moves_to_the_next_position_ahead_of_it() {
        let points = [point(0.0), point(100.0), point(200.0)];
        let mandatory = axis(SnapStrictness::Mandatory, &points);
        assert_eq!(mandatory.step(0.0, 30.0, 33.0), Some(100.0));
        assert_eq!(mandatory.step(100.0, 90.0, 33.0), Some(0.0));
        assert_eq!(
            mandatory.step(200.0, 230.0, 33.0),
            Some(200.0),
            "nothing ahead: stays"
        );
        assert_eq!(
            mandatory.step(100.0, 100.0, 33.0),
            None,
            "no movement, nothing to say"
        );

        let proximity = axis(SnapStrictness::Proximity, &points);
        assert_eq!(
            proximity.step(0.0, 30.0, 33.0),
            None,
            "keeps its natural end"
        );
        assert_eq!(
            proximity.step(60.0, 90.0, 33.0),
            Some(100.0),
            "within range of the next"
        );
        assert_eq!(
            proximity.step(130.0, 110.0, 33.0),
            Some(100.0),
            "backwards, the next is the one behind",
        );
        assert_eq!(proximity.step(20.0, 10.0, 33.0), Some(0.0));
    }

    #[test]
    fn a_snapped_directed_step_absorbs_its_whole_delta() {
        let points = [point(0.0), point(100.0)];
        let mandatory = axis(SnapStrictness::Mandatory, &points);
        let (applied, absorbed) = resolve_step(
            ScrollKind::Directed,
            Vector2D::zero(),
            Vector2D::new(0.0, 30.0),
            Vector2D::new(0.0, 100.0),
            Size2D::new(100.0, 100.0),
            None,
            Some(mandatory),
        );
        assert_eq!(applied, Vector2D::new(0.0, 100.0));
        assert_eq!(absorbed, Vector2D::new(0.0, 30.0));

        let (applied, absorbed) = resolve_step(
            ScrollKind::Directed,
            Vector2D::new(0.0, 100.0),
            Vector2D::new(0.0, 30.0),
            Vector2D::new(0.0, 100.0),
            Size2D::new(100.0, 100.0),
            None,
            Some(mandatory),
        );
        assert_eq!(applied, Vector2D::new(0.0, 100.0), "nothing ahead: stays");
        assert_eq!(absorbed, Vector2D::zero(), "and the tick chains on");

        let (applied, absorbed) = resolve_step(
            ScrollKind::Directed,
            Vector2D::new(0.0, 100.0),
            Vector2D::new(0.0, 30.0),
            Vector2D::new(0.0, 200.0),
            Size2D::new(100.0, 100.0),
            None,
            Some(mandatory),
        );
        assert_eq!(
            applied,
            Vector2D::new(0.0, 100.0),
            "room left, but no position ahead"
        );
        assert_eq!(absorbed, Vector2D::zero(), "so the tick still chains on");

        let (applied, absorbed) = resolve_step(
            ScrollKind::Gesture,
            Vector2D::zero(),
            Vector2D::new(0.0, 130.0),
            Vector2D::new(0.0, 100.0),
            Size2D::new(100.0, 100.0),
            None,
            Some(mandatory),
        );
        assert_eq!(applied, Vector2D::new(0.0, 100.0), "a drag step is raw");
        assert_eq!(
            absorbed,
            Vector2D::new(0.0, 100.0),
            "and absorbs only what it moved"
        );
    }

    fn snapping_page(
        container_css: &str,
        page_css: &str,
        heights: &[f32],
    ) -> (Document<()>, NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; width: 800px; height: 600px; }}
                 .scroller {{ display: flex; flex-direction: column; overflow: scroll;
                              width: 100px; height: 100px; {container_css} }}
                 .page {{ flex-shrink: 0; width: 100px; scroll-snap-align: start; {page_css} }}"
            ),
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let scroller = document.create_element("view", ());
        document.add_class(scroller, "scroller");
        document.append_child(root, scroller);
        for height in heights {
            let page = document.create_element("view", ());
            document.add_class(page, "page");
            document.set_inline_style(page, &format!("height: {height}px"));
            document.append_child(scroller, page);
        }
        document.layout();
        (document, scroller)
    }

    fn offsets(positions: &SnapAxisPositions) -> Vec<(f32, f32)> {
        positions
            .points
            .iter()
            .map(|point| (point.min, point.max))
            .collect()
    }

    #[test]
    fn positions_follow_the_alignment_and_clamp_into_the_scrollable_range() {
        let heights = [50.0; 4];
        let (document, scroller) = snapping_page("scroll-snap-type: y mandatory;", "", &heights);
        let positions = document.snap_positions(scroller).expect("snaps on y");
        assert!(positions.x.is_none(), "y only");
        let y = positions.y.as_ref().expect("y axis");
        assert_eq!(y.strictness, SnapStrictness::Mandatory);
        assert_eq!(
            offsets(y),
            [(0.0, 0.0), (50.0, 50.0), (100.0, 100.0), (100.0, 100.0)]
        );

        let (document, scroller) =
            snapping_page("scroll-snap-type: y;", "scroll-snap-align: end;", &heights);
        let positions = document.snap_positions(scroller).expect("snaps on y");
        let y = positions.y.as_ref().expect("y axis");
        assert_eq!(
            y.strictness,
            SnapStrictness::Proximity,
            "the default strictness"
        );
        assert_eq!(
            offsets(y),
            [(0.0, 0.0), (0.0, 0.0), (50.0, 50.0), (100.0, 100.0)]
        );

        let (document, scroller) = snapping_page(
            "scroll-snap-type: both mandatory;",
            "scroll-snap-align: center;",
            &heights,
        );
        let positions = document.snap_positions(scroller).expect("snaps");
        assert_eq!(
            offsets(positions.y.as_ref().expect("y axis")),
            [(0.0, 0.0), (25.0, 25.0), (75.0, 75.0), (100.0, 100.0)]
        );
        let x = positions.x.as_ref().expect("x axis is asked for");
        assert_eq!(
            offsets(x),
            [(0.0, 0.0); 4],
            "no x overflow: every position clamps to 0"
        );
    }

    #[test]
    fn scroll_padding_and_margin_move_the_positions() {
        let heights = [50.0; 4];
        let (document, scroller) = snapping_page(
            "scroll-snap-type: y mandatory; scroll-padding: 10px;",
            "scroll-margin-top: 5px;",
            &heights,
        );
        let positions = document.snap_positions(scroller).expect("snaps on y");
        assert_eq!(
            offsets(positions.y.as_ref().expect("y axis")),
            [(0.0, 0.0), (35.0, 35.0), (85.0, 85.0), (100.0, 100.0)]
        );

        let (document, scroller) = snapping_page(
            "scroll-snap-type: y mandatory; scroll-padding-top: 20%;",
            "",
            &heights,
        );
        let positions = document.snap_positions(scroller).expect("snaps on y");
        assert_eq!(
            offsets(positions.y.as_ref().expect("y axis")),
            [(0.0, 0.0), (30.0, 30.0), (80.0, 80.0), (100.0, 100.0)],
            "a percentage resolves against the scrollport",
        );
    }

    #[test]
    fn an_area_larger_than_the_snapport_is_a_range_and_a_stop_is_carried() {
        let (mut document, scroller) =
            snapping_page("scroll-snap-type: y mandatory;", "", &[50.0, 300.0, 50.0]);
        let tall = document
            .get(scroller)
            .and_then(|node| node.child_ids().get(1).copied())
            .expect("the tall page");
        document.set_inline_style(tall, "height: 300px; scroll-snap-stop: always");
        document.layout();
        let positions = document.snap_positions(scroller).expect("snaps on y");
        let y = positions.y.as_ref().expect("y axis");
        assert_eq!(offsets(y), [(0.0, 0.0), (50.0, 250.0), (300.0, 300.0)]);
        assert!(y.points[1].stop);
        assert!(!y.points[0].stop);
    }

    #[test]
    fn nested_scroll_containers_keep_their_own_areas() {
        let (mut document, scroller) =
            snapping_page("scroll-snap-type: y mandatory;", "", &[50.0, 50.0]);
        let first = document
            .get(scroller)
            .and_then(|node| node.child_ids().first().copied())
            .expect("a page");
        document.set_inline_style(first, "height: 50px; overflow: scroll");
        let nested = document.create_element("view", ());
        document.add_class(nested, "page");
        document.set_inline_style(nested, "height: 50px");
        document.append_child(first, nested);
        document.layout();
        let positions = document.snap_positions(scroller).expect("snaps on y");
        assert_eq!(
            offsets(positions.y.as_ref().expect("y axis")),
            [(0.0, 0.0), (0.0, 0.0)],
            "both outer pages, the nested container among them; the area inside it is its own",
        );
        assert_eq!(
            document.snap_positions(first),
            None,
            "the nested container itself does not snap",
        );
    }

    #[test]
    fn the_document_settles_and_steps_through_its_own_positions() {
        let (mut document, scroller) =
            snapping_page("scroll-snap-type: y mandatory;", "", &[50.0; 4]);
        document.scroll_to(scroller, Vector2D::new(0.0, 60.0));
        assert_eq!(
            document.settle_scroll(scroller, Vector2D::zero()),
            Vector2D::new(0.0, 50.0)
        );
        assert_eq!(document.scroll_offset(scroller), Vector2D::new(0.0, 50.0));

        let page = document
            .get(scroller)
            .and_then(|node| node.child_ids().first().copied())
            .expect("a page");
        assert_eq!(
            document.scroll_chain_directed(page, Vector2D::new(0.0, 10.0)),
            Some((scroller, Vector2D::new(0.0, 10.0))),
            "the tick is absorbed",
        );
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(0.0, 100.0),
            "and lands on the next position",
        );
        assert_eq!(
            document.scroll_chain(page, Vector2D::new(0.0, -30.0)),
            Some((scroller, Vector2D::new(0.0, -30.0))),
        );
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(0.0, 70.0),
            "a drag step is raw",
        );
        assert_eq!(
            document.settle_scroll(scroller, Vector2D::new(0.0, 100.0)),
            Vector2D::new(0.0, 50.0),
        );
    }

    #[test]
    fn a_commit_publishes_the_positions_beside_the_slot() {
        let (mut document, scroller) =
            snapping_page("scroll-snap-type: y mandatory;", "", &[50.0; 4]);
        let frame = document.commit();
        let slot = frame
            .scroll_slots()
            .iter()
            .find(|slot| slot.node == scroller)
            .expect("the scroller has a slot");
        let (x, y) = frame.snap_axes(slot);
        assert!(x.is_none());
        let y = y.expect("y axis");
        assert_eq!(y.strictness, SnapStrictness::Mandatory);
        assert_eq!(
            y.points.iter().map(|point| point.min).collect::<Vec<_>>(),
            [0.0, 50.0, 100.0, 100.0]
        );
        assert_eq!(y.settle(0.0, 60.0, 0.0), 50.0);
    }
}
