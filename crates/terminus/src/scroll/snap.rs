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
//!   scrollable range (normalized into the period on a circular axis, below). An area larger than
//!   the snapport on an axis is a *range* of valid positions (§6.2: the scroll position may rest
//!   anywhere the snapport is inside the area) rather than a point.
//! - §6.5 `scroll-snap-stop: always`: an operation cannot pass such a position — one that would
//!   travel past it settles on it instead.
//! - §7 choosing: a drag settles on the position nearest to where it ended. A discrete directed
//!   step (a wheel tick) moves to the next position in its direction under `mandatory`; under
//!   `proximity` it takes its natural end and snaps only when a position is within range.
//! - §6.1's "must rest on a snap position": the runtime settles a container from its own offset
//!   whenever a commit publishes one — initial layout, a relayout that moved areas, a programmatic
//!   scroll — as long as no drag is holding it.
//! - §4.1.3 re-snapping after a layout change: a container that stood on an area's snap position
//!   before the change stands on that same area's position after it, whatever the strictness and
//!   however far the position moved, as long as the area still contributes one. Every position
//!   names its area ([`SnapPoint::area`]), so "snapped to" is read off the positions the last
//!   commit published and the offset the container stands at ([`SnapAxis::snapped_area`]) and needs
//!   no record of its own; the area's new position is [`SnapAxis::position_of`]. The document does
//!   this for its own offset inside the render that publishes the new positions
//!   (`Document::resnap`), and the runtime's painter for the offset it holds when it adopts that
//!   commit, at rest only. Where several areas are aligned at the offset, §6.2.1 picks the first in
//!   tree order ([`SnapPoint::order`]). The move is a jump, which §4.1.3 leaves to the user agent
//!   for a re-snap to the same box. A container snapped to nothing, or whose area is gone, falls to
//!   the at-rest rule above.
//!
//! An axis with `overscroll-behavior: circular` has no boundary on the
//! painter's side, so its snap positions repeat: a [`SnapAxis`] with a
//! [`period`](SnapAxis::period) holds every point shifted by every whole
//! number of periods. The choosing rules normalize the query into one
//! period and search the three copies around it — that period and its two
//! neighbours — so their work does not depend on how far the offset has
//! travelled. A flick forward from the last page therefore lands on the
//! first page's next copy instead of stopping at the end. The document's own
//! axes carry the period too ([`SnapAxisPositions::period`]), and each
//! position it computes there is normalized into `[0, period)` instead of
//! clamped into the scrollable range, so an alignment that needs an offset
//! past `max_offset` or before `0` is a position across the seam;
//! [`Document::settle_scroll`] and the directed chain steps
//! ([`Document::scroll_chain_directed`]) find positions across the seam as
//! the painter does.
//!
//! Deliberately absent: css-scroll-snap-2's `scrollsnapchange` and
//! `scrollsnapchanging` events (scoped out of the request), snap
//! *animation* in this crate (every snap the document applies is
//! instantaneous; the runtime's painter animates the snap after a drag's
//! release), and §7's preference for the same element on both axes (axes
//! are chosen independently). §6.2.1's other two steps are absent with it:
//! there is no focused or targeted box to prefer (this crate has no focus
//! model), and an area is not set aside for a descendant of it aligned at
//! the same position.
//!
//! Snap areas are the elements in the container's DOM subtree whose nearest
//! scroll container on the containing-block chain is this one. The walk
//! does not enter a nested scroll container's subtree, so an out-of-flow box
//! escaping from inside a nested scroller to this container is not found —
//! a recorded approximation. Transforms are ignored, as they are for every
//! measurement in this crate.

use euclid::default::{Point2D, Rect, Size2D, Vector2D};
use stylo::values::computed::{
    Length, NonNegativeLengthPercentageOrAuto, ScrollSnapAxis, ScrollSnapStop, ScrollSnapStrictness,
};
use stylo::values::specified::box_::ScrollSnapAlignKeyword;

use super::{ScrollAxes, clamp_to, is_scroll_container, wrap_axis};
use crate::layout::{DisplayMode, StyleView, box_parent};
use crate::tree::document::{Document, NodeId};
use crate::visual::PaintOrder;

/// The share of the scrollport, per axis, within which `proximity` snaps.
pub(crate) const PROXIMITY_RATIO: f32 = 1.0 / 3.0;

/// How far from a snap position, in CSS px, an offset still stands on it
/// ([`SnapAxis::snapped_area`]). An offset at rest on a position is that
/// position's own number, except on a circular axis, where normalizing it
/// into the period rounds; no container comes to rest this near a position
/// without being on it.
pub(crate) const SNAPPED_TOLERANCE: f32 = 0.5;

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
    /// The element whose snap area this position aligns: what a container
    /// standing here is snapped to, and what it follows through a layout
    /// change (§4.1.3).
    pub area: NodeId,
    /// The area's place among its container's areas in flat-tree order,
    /// which chooses between areas aligned at one offset (§6.2.1).
    pub order: u32,
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
    /// The period of a circular axis (`overscroll-behavior: circular`): the
    /// extent of the whole scrolling area on that axis. With a period every
    /// point also exists shifted by every whole multiple of it, because the
    /// painter's offset on such an axis stands anywhere on an unbounded
    /// line that repeats the scrolling area; a query searches only the
    /// copies in the period holding it and the one on either side. `None`
    /// on a bounded axis.
    pub period: Option<f32>,
}

impl SnapAxis<'_> {
    /// The distance within which `proximity` snaps, for a scrollport of
    /// `extent` on this axis.
    #[must_use]
    pub fn proximity_threshold(extent: f32) -> f32 {
        extent * PROXIMITY_RATIO
    }

    /// The points a query at `value` may choose among.
    ///
    /// On a bounded axis these are the points themselves. On a circular one
    /// `value` is first normalized into one period: its base is the whole
    /// multiple of the period at or below it (`value` less its Euclidean
    /// remainder; a non-finite `value` counts as `0`), and the points are
    /// yielded shifted by that base and by one period either side of it.
    /// Every point lies inside one period of the scrolling area, so these
    /// three copies hold the nearest position on either side of `value` and
    /// every position within one period of it, and the work stays the same
    /// however far `value` lies from the origin.
    ///
    /// The copies are anchored at the caller's own `value`, so every result
    /// stays in the caller's unwrapped coordinate frame: settling `560` on
    /// points `0`, `200` and `400` with a period of `600` answers `600`,
    /// not `0`, because the painter's glide from `560` must move forward
    /// `40` px rather than jump back across the seam.
    fn copies_around(&self, value: f32) -> impl Iterator<Item = SnapPoint> + '_ {
        let period = self
            .period
            .filter(|period| period.is_finite() && *period > 0.0);
        let base = period.map_or(0.0, |period| {
            let value = if value.is_finite() { value } else { 0.0 };
            let remainder = value.rem_euclid(period);
            // `rem_euclid` rounds up to `period` itself for a value just
            // below a multiple of it; that value belongs to the period above.
            if remainder < period {
                value - remainder
            } else {
                value - remainder + period
            }
        });
        let periods = if period.is_some() { -1..=1 } else { 0..=0 };
        periods.flat_map(move |k: i8| {
            self.points.iter().map(move |point| match period {
                Some(period) => {
                    let shift = base + f32::from(k) * period;
                    SnapPoint {
                        min: point.min + shift,
                        max: point.max + shift,
                        ..*point
                    }
                }
                None => *point,
            })
        })
    }

    fn nearest(&self, offset: f32) -> Option<SnapPoint> {
        self.copies_around(offset)
            .min_by(|a, b| a.distance(offset).total_cmp(&b.distance(offset)))
    }

    /// The first `always` stop strictly between `from` and `to`, in travel
    /// order: the position an operation from `from` to `to` may not pass.
    ///
    /// On a circular axis the stops repeat every period, so the first one
    /// the travel meets lies within one period of `from`, and the copies
    /// around `from` hold every position within one period of it on either
    /// side. The nearest qualifying copy is therefore the first stop met
    /// however far `to` lies.
    fn stop_between(&self, from: f32, to: f32) -> Option<f32> {
        if to > from {
            self.copies_around(from)
                .filter(|point| point.stop && point.min > from && point.min < to)
                .map(|point| point.min)
                .min_by(f32::total_cmp)
        } else if to < from {
            self.copies_around(from)
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
    /// chain on to the container above. A circular axis always has a next
    /// position, so it never refuses: a tick at the last page moves on to
    /// the first page's next copy.
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
            .copies_around(natural)
            .any(|point| point.distance(natural) == 0.0)
        {
            return Some(natural);
        }
        let next = if natural > current {
            self.copies_around(current)
                .filter(|point| point.min > current)
                .map(|point| point.min)
                .min_by(f32::total_cmp)
        } else {
            self.copies_around(current)
                .filter(|point| point.max < current)
                .map(|point| point.max)
                .max_by(f32::total_cmp)
        };
        match self.strictness {
            SnapStrictness::Mandatory => Some(next.unwrap_or(current)),
            SnapStrictness::Proximity => next.filter(|next| (next - natural).abs() <= threshold),
        }
    }

    /// The area a container standing at `offset` is snapped to on this axis
    /// (§4.1.3), or `None` when it stands on no position: of the areas whose
    /// position holds `offset`, within [`SNAPPED_TOLERANCE`], the first in
    /// tree order (§6.2.1). The strictness does not enter: a `proximity`
    /// container that came to rest on a position is snapped to it. A
    /// non-finite offset stands nowhere.
    #[must_use]
    pub fn snapped_area(&self, offset: f32) -> Option<NodeId> {
        if !offset.is_finite() {
            return None;
        }
        self.copies_around(offset)
            .filter(|point| point.distance(offset) <= SNAPPED_TOLERANCE)
            .min_by_key(|point| point.order)
            .map(|point| point.area)
    }

    /// Where a container at `offset` rests on `area`'s position, or `None`
    /// when `area` contributes none to this axis: the position itself, or
    /// for an area larger than the snapport the offset in its range nearest
    /// `offset`. On a circular axis it is the copy nearest `offset`, in the
    /// caller's own frame as every other result here is.
    #[must_use]
    pub fn position_of(&self, area: NodeId, offset: f32) -> Option<f32> {
        self.copies_around(offset)
            .filter(|point| point.area == area)
            .min_by(|a, b| a.distance(offset).total_cmp(&b.distance(offset)))
            .map(|point| point.nearest(offset))
    }

    /// Where a container that stood at `stood` over `before` — this axis as
    /// the previous commit published it — stands now, when it was snapped
    /// to an area that still contributes a position (§4.1.3); `None` leaves
    /// it to the caller's own rule. `offset` is where it stands against the
    /// new geometry, which picks the copy on a circular axis and the offset
    /// inside a range.
    #[must_use]
    pub fn resnap(&self, before: &SnapAxis<'_>, stood: f32, offset: f32) -> Option<f32> {
        self.position_of(before.snapped_area(stood)?, offset)
    }
}

/// One axis's snap positions, owned.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapAxisPositions {
    pub strictness: SnapStrictness,
    /// Sorted by `min`.
    pub points: Vec<SnapPoint>,
    /// The axis's wrap period on a circular axis with content to scroll
    /// ([`ScrollBox::wrap_period`](super::ScrollBox::wrap_period)), `None`
    /// on a bounded one. With a period every point's `min` lies in
    /// `[0, period)`.
    pub period: Option<f32>,
}

impl SnapAxisPositions {
    #[must_use]
    pub fn axis(&self) -> SnapAxis<'_> {
        SnapAxis {
            strictness: self.strictness,
            points: &self.points,
            period: self.period,
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
pub(crate) enum ScrollKind {
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
///
/// An axis in `wrap` has no boundary (`overscroll-behavior: circular` with
/// content to scroll, [`ScrollBox::wrap_axes`](super::ScrollBox::wrap_axes)):
/// its natural end is not clamped, so the step absorbs its whole delta, and
/// the caller normalizes the result into the period where it consumes it —
/// the document through [`Document::scroll_to`], the painter at compose and
/// at each rebase.
#[must_use]
#[allow(
    clippy::float_cmp,
    reason = "a refused step hands back the current offset itself, bit for bit"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "one step's full inputs: the container's geometry, its snapping per axis, \
              and the axes the caller wraps"
)]
pub(crate) fn resolve_step(
    kind: ScrollKind,
    offset: Vector2D<f32>,
    admitted: Vector2D<f32>,
    max: Vector2D<f32>,
    scrollport: Size2D<f32>,
    snap_x: Option<SnapAxis<'_>>,
    snap_y: Option<SnapAxis<'_>>,
    wrap: ScrollAxes,
) -> (Vector2D<f32>, Vector2D<f32>) {
    let unbounded = offset + admitted;
    let clamped = clamp_to(unbounded, max);
    let natural = Vector2D::new(
        if wrap.x { unbounded.x } else { clamped.x },
        if wrap.y { unbounded.y } else { clamped.y },
    );
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
pub(crate) fn settle_offset(
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

/// The snap position one area contributes on one axis, as the `min` and
/// `max` of its [`SnapPoint`], given the area's and the snapport's extents
/// in the container's scrolling coordinates.
///
/// On a bounded axis the position is clamped into the scrollable range
/// `0..=max`. On an axis that wraps with `period` nothing is clamped: a
/// single position is normalized into `[0, period)`, and a range moves its
/// `min` into `[0, period)` and its `max` by the same shift, so the range
/// keeps its length and starts inside one period.
fn snap_range(
    align: ScrollSnapAlignKeyword,
    area: (f32, f32),
    snapport: (f32, f32),
    max: f32,
    period: Option<f32>,
) -> (f32, f32) {
    let clamp = |offset: f32| offset.clamp(0.0, max.max(0.0));
    let (area_start, area_end) = area;
    let (port_start, port_end) = snapport;
    if area_end - area_start > port_end - port_start {
        // Larger than the snapport: every offset that keeps the snapport
        // inside the area is valid, whatever the alignment asked for.
        let (min, max) = (area_start - port_start, area_end - port_end);
        if let Some(period) = period {
            let start = wrap_axis(min, period);
            return (start, max + (start - min));
        }
        let (min, max) = (clamp(min), clamp(max));
        if min <= max {
            return (min, max);
        }
    }
    let offset = match align {
        ScrollSnapAlignKeyword::Start => area_start - port_start,
        ScrollSnapAlignKeyword::End => area_end - port_end,
        ScrollSnapAlignKeyword::Center => (area_start + area_end - port_start - port_end) / 2.0,
        ScrollSnapAlignKeyword::None => unreachable!("an area with no alignment is not collected"),
    };
    let offset = match period {
        Some(period) => wrap_axis(offset, period),
        None => clamp(offset),
    };
    (offset, offset)
}

pub(crate) fn scroll_padding(value: &NonNegativeLengthPercentageOrAuto, basis: f32) -> f32 {
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
    /// when no area contributes a point to it. On an axis with a
    /// [`ScrollBox::wrap_period`](super::ScrollBox::wrap_period) the axis
    /// carries the period and its positions are normalized into it.
    #[must_use]
    pub fn snap_positions(&self, id: NodeId) -> Option<SnapPositions> {
        let node = self.get(id)?;
        let style = node.layout_computed_style()?;
        let snap_type = *style.get_scroll_snap_type();
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
        let (period_x, period_y) = scroll_box.wrap_period();
        let snapport_x = (
            scroll_padding(style.get_scroll_padding_left(), port.width),
            port.width - scroll_padding(style.get_scroll_padding_right(), port.width),
        );
        let snapport_y = (
            scroll_padding(style.get_scroll_padding_top(), port.height),
            port.height - scroll_padding(style.get_scroll_padding_bottom(), port.height),
        );

        let mut x = Vec::new();
        let mut y = Vec::new();
        // Counts the areas as the walk meets them, which is tree order.
        let mut order = 0_u32;
        // The flat tree: a snap area is a box, and a scroll container in a
        // shadow tree holds its slotted descendants' boxes.
        let mut pending: Vec<NodeId> = node.flat_children().iter().rev().copied().collect();
        while let Some(child_id) = pending.pop() {
            let Some(child) = self.get(child_id) else {
                continue;
            };
            let Some(view) = StyleView::try_of(child) else {
                continue;
            };
            let mode = view.display_mode();
            if mode == DisplayMode::None {
                continue;
            }
            let values = view.values();
            if mode != DisplayMode::Contents {
                let align = *values.get_scroll_snap_align();
                let wants_x = snaps_x && align.inline() != ScrollSnapAlignKeyword::None;
                let wants_y = snaps_y && align.block() != ScrollSnapAlignKeyword::None;
                if (wants_x || wants_y)
                    && self.nearest_scroll_container(child_id) == Some(id)
                    && let Some(rect) = self.rect_in_scroll_container(child_id, id)
                {
                    let stop = *values.get_scroll_snap_stop() == ScrollSnapStop::Always;
                    let area_x = (
                        rect.min_x() - values.get_scroll_margin_left().px(),
                        rect.max_x() + values.get_scroll_margin_right().px(),
                    );
                    let area_y = (
                        rect.min_y() - values.get_scroll_margin_top().px(),
                        rect.max_y() + values.get_scroll_margin_bottom().px(),
                    );
                    let point = |(min, max): (f32, f32)| SnapPoint {
                        min,
                        max,
                        stop,
                        area: child_id,
                        order,
                    };
                    let (inline, block) = (align.inline(), align.block());
                    if wants_x {
                        let range = snap_range(inline, area_x, snapport_x, max.x, period_x);
                        x.push(point(range));
                    }
                    if wants_y {
                        let range = snap_range(block, area_y, snapport_y, max.y, period_y);
                        y.push(point(range));
                    }
                    order += 1;
                }
            }
            if is_scroll_container(values) {
                // Its subtree snaps to it, not to us; the container itself
                // may still be one of our areas.
                continue;
            }
            pending.extend(child.flat_children().iter().rev());
        }
        x.sort_by(|a, b| a.min.total_cmp(&b.min));
        y.sort_by(|a, b| a.min.total_cmp(&b.min));
        let axis = |snaps: bool, points: Vec<SnapPoint>, period: Option<f32>| {
            snaps.then_some(SnapAxisPositions {
                strictness,
                points,
                period,
            })
        };
        Some(SnapPositions {
            x: axis(snaps_x, x, period_x),
            y: axis(snaps_y, y, period_y),
        })
    }

    /// Settles `id` after a scrolling operation that began at `start` —
    /// [`settle_offset`] over live geometry — and returns the offset it
    /// rests at. A container that does not snap keeps its offset. On a
    /// circular axis the positions repeat every period, so the nearest one
    /// may lie across the seam; [`Self::scroll_to`] admits it into the
    /// period.
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

    /// css-scroll-snap-1 §4.1.3 for the document's own offsets: each
    /// container of `frame` — the paint order just built — that stood on an
    /// area's snap position in the committed frame it replaces moves to
    /// that area's position in `frame`. Answers whether any offset moved, in
    /// which case `frame` no longer carries the offsets the document holds
    /// and the render builds it again.
    ///
    /// Both sets of positions are tables a build has already filled, so a
    /// render that moves nothing pays one scan of each snapping container's
    /// points and no walk of the tree.
    ///
    /// Only an offset that stood on a position moves. The at-rest rule for
    /// the others — a `mandatory` container whose area is gone, say — needs
    /// to know the container is at rest, which the runtime's painter does
    /// and the document does not: its offset under a drag is whatever that
    /// drag last posted. The write is a plain [`Self::scroll_to`] and no
    /// request, because a painter holding an offset of its own applies the
    /// same rule to it when it adopts the commit, and must not be moved
    /// while a drag or a glide has the container. A container with a
    /// pending request ([`Self::pending_scroll_request`]) is left to it: its
    /// position is the request's until the painter has carried it out.
    pub(crate) fn resnap(&mut self, frame: &PaintOrder) -> bool {
        if frame.snap_points().is_empty() {
            return false;
        }
        let Some(committed) = self.committed_frame() else {
            return false;
        };
        let mut moved = false;
        for slot in frame.slots() {
            let (after_x, after_y) = frame.snap_axes(slot);
            if after_x.is_none() && after_y.is_none() {
                continue;
            }
            let Some(before) = committed.slot_of(slot.node) else {
                continue;
            };
            if self.pending_scroll_request(slot.node).is_some() {
                continue;
            }
            let (before_x, before_y) =
                committed.snap_axes(&committed.scroll_slots()[before as usize]);
            // As it was written, against the geometry it was written for;
            // `slot.offset` is the same offset admitted against the new one.
            let stood = self.stored_scroll_offset(slot.node);
            let axis = |after: Option<SnapAxis<'_>>,
                        before: Option<SnapAxis<'_>>,
                        stood: f32,
                        offset: f32| {
                after
                    .zip(before)
                    .and_then(|(after, before)| after.resnap(&before, stood, offset))
                    .unwrap_or(offset)
            };
            let target = Vector2D::new(
                axis(after_x, before_x, stood.x, slot.offset.x),
                axis(after_y, before_y, stood.y, slot.offset.y),
            );
            if target != slot.offset {
                self.scroll_to(slot.node, target);
                moved = true;
            }
        }
        moved
    }

    /// The first scroll container above `id` on its containing-block chain.
    pub(crate) fn nearest_scroll_container(&self, id: NodeId) -> Option<NodeId> {
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
    pub(crate) fn rect_in_scroll_container(
        &self,
        id: NodeId,
        container: NodeId,
    ) -> Option<Rect<f32>> {
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
    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::tests::device;

    /// The area named `key` in the axis tests, which read no tree.
    fn area(key: u64) -> NodeId {
        NodeId::from_bits(key).expect("a well-formed handle")
    }

    /// The position `min..=max` of area `key`, the `key`th in tree order.
    fn range(min: f32, max: f32, key: u32) -> SnapPoint {
        SnapPoint {
            min,
            max,
            stop: false,
            area: area(u64::from(key)),
            order: key,
        }
    }

    fn point(offset: f32) -> SnapPoint {
        range(offset, offset, 1)
    }

    fn stop(offset: f32) -> SnapPoint {
        SnapPoint {
            stop: true,
            ..point(offset)
        }
    }

    fn axis(strictness: SnapStrictness, points: &[SnapPoint]) -> SnapAxis<'_> {
        SnapAxis {
            strictness,
            points,
            period: None,
        }
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
        let points = [point(0.0), range(100.0, 300.0, 2)];
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
            ScrollAxes::NONE,
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
            ScrollAxes::NONE,
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
            ScrollAxes::NONE,
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
            ScrollAxes::NONE,
        );
        assert_eq!(applied, Vector2D::new(0.0, 100.0), "a drag step is raw");
        assert_eq!(
            absorbed,
            Vector2D::new(0.0, 100.0),
            "and absorbs only what it moved"
        );
    }

    /// Three pages of 200px in a 200px scrollport: offsets `0..=400`, so
    /// the circular period is the whole 600px scrolling area.
    fn periodic(strictness: SnapStrictness, points: &[SnapPoint]) -> SnapAxis<'_> {
        SnapAxis {
            strictness,
            points,
            period: Some(600.0),
        }
    }

    #[test]
    fn a_periodic_axis_settles_on_the_nearest_copy_across_the_seam() {
        let points = [point(0.0), point(200.0), point(400.0)];
        let mandatory = periodic(SnapStrictness::Mandatory, &points);
        assert_eq!(
            mandatory.settle(400.0, 560.0, 0.0),
            600.0,
            "past the last page, the first page's next copy is nearer",
        );
        assert_eq!(mandatory.settle(0.0, -40.0, 0.0), 0.0);
        assert_eq!(
            mandatory.settle(0.0, -150.0, 0.0),
            -200.0,
            "the last page, one period back",
        );
    }

    #[test]
    fn a_periodic_axis_always_has_a_next_position() {
        let points = [point(0.0), point(200.0), point(400.0)];
        let mandatory = periodic(SnapStrictness::Mandatory, &points);
        assert_eq!(
            mandatory.step(400.0, 450.0, 0.0),
            Some(600.0),
            "a tick at the last page moves on to the first page's copy",
        );
        assert_eq!(mandatory.step(0.0, -10.0, 0.0), Some(-200.0));
    }

    #[test]
    fn a_periodic_always_stop_is_found_in_a_later_period() {
        let points = [point(0.0), stop(200.0), point(400.0)];
        let mandatory = periodic(SnapStrictness::Mandatory, &points);
        assert_eq!(mandatory.stop_between(550.0, 850.0), Some(800.0));
        assert_eq!(mandatory.stop_between(850.0, 550.0), Some(800.0));
        assert_eq!(
            mandatory.stop_between(200.0, 1000.0),
            Some(800.0),
            "standing on a stop, the next copy one whole period ahead still holds",
        );
        assert_eq!(mandatory.stop_between(800.0, 0.0), Some(200.0));
        assert_eq!(
            mandatory.settle(-500.0, 1900.0, 0.0),
            -400.0,
            "a long fling stops at the first stop ahead of it",
        );
    }

    #[test]
    fn a_far_query_searches_three_periods_in_its_own_frame() {
        let points = [point(0.0), point(200.0), point(400.0)];
        let mandatory = periodic(SnapStrictness::Mandatory, &points);
        assert_eq!(mandatory.copies_around(1.0e9).count(), 3 * 3);
        assert_eq!(mandatory.copies_around(f32::INFINITY).count(), 3 * 3);
        assert_eq!(
            mandatory.settle(0.0, 6_000_560.0, 0.0),
            6_000_600.0,
            "the nearest copy, in the caller's own frame",
        );
        assert_eq!(mandatory.step(60_000.0, 60_010.0, 0.0), Some(60_200.0));
        let bounded = axis(SnapStrictness::Mandatory, &points);
        assert_eq!(bounded.copies_around(1.0e9).count(), 3);
    }

    #[test]
    fn a_wrapped_axis_steps_past_its_maximum() {
        let wrap = ScrollAxes { x: false, y: true };
        let (applied, absorbed) = resolve_step(
            ScrollKind::Gesture,
            Vector2D::new(0.0, 380.0),
            Vector2D::new(50.0, 50.0),
            Vector2D::new(0.0, 400.0),
            Size2D::new(200.0, 200.0),
            None,
            None,
            wrap,
        );
        assert_eq!(applied, Vector2D::new(0.0, 430.0), "x clamps, y does not");
        assert_eq!(absorbed, Vector2D::new(0.0, 50.0));

        let points = [point(0.0), point(200.0), point(400.0)];
        let (applied, absorbed) = resolve_step(
            ScrollKind::Directed,
            Vector2D::new(0.0, 400.0),
            Vector2D::new(0.0, 30.0),
            Vector2D::new(0.0, 400.0),
            Size2D::new(200.0, 200.0),
            None,
            Some(periodic(SnapStrictness::Mandatory, &points)),
            wrap,
        );
        assert_eq!(applied, Vector2D::new(0.0, 600.0), "a tick wraps a page");
        assert_eq!(absorbed, Vector2D::new(0.0, 30.0));
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

    /// A circular snapping axis carries its period and normalizes each
    /// position into it instead of clamping. Four 50px pages centred in a
    /// 100px scrollport: `max` 100, period 200. The first page's centre
    /// needs −25, which is 175 on the circle; the last page's needs 125,
    /// past `max`, and keeps it.
    #[test]
    fn a_circular_axis_publishes_its_period_and_positions_across_the_seam() {
        let (document, scroller) = snapping_page(
            "scroll-snap-type: y mandatory; overscroll-behavior-y: circular;",
            "scroll-snap-align: center;",
            &[50.0; 4],
        );
        let positions = document.snap_positions(scroller).expect("snaps on y");
        let y = positions.y.as_ref().expect("y axis");
        assert_eq!(y.period, Some(200.0));
        assert_eq!(
            y.axis().period,
            Some(200.0),
            "the borrowed axis repeats too"
        );
        assert_eq!(
            offsets(y),
            [(25.0, 25.0), (75.0, 75.0), (125.0, 125.0), (175.0, 175.0)]
        );

        let (mut document, scroller) = snapping_page(
            "scroll-snap-type: y mandatory; overscroll-behavior-y: circular;",
            "",
            &[300.0, 50.0, 50.0],
        );
        let tall = document
            .get(scroller)
            .and_then(|node| node.child_ids().first().copied())
            .expect("the tall page");
        document.set_inline_style(tall, "height: 300px; scroll-margin-top: 20px");
        document.layout();
        let positions = document.snap_positions(scroller).expect("snaps on y");
        let y = positions.y.as_ref().expect("y axis");
        assert_eq!(y.period, Some(400.0));
        assert_eq!(
            offsets(y),
            [(300.0, 300.0), (350.0, 350.0), (380.0, 600.0)],
            "a range starting before 0 moves into the period whole",
        );

        let (document, scroller) = snapping_page("scroll-snap-type: y mandatory;", "", &[50.0; 4]);
        let positions = document.snap_positions(scroller).expect("snaps on y");
        assert_eq!(positions.y.expect("y axis").period, None, "bounded");
    }

    /// The document settles and steps across the seam on a circular axis,
    /// as the painter does. Standing just past `max` (110 of `max` 100,
    /// period 200), the nearest position is the last page's 125, not the
    /// clamped 100; a wheel tick forward from the first page's 175 moves on
    /// to the second page's 25, one period on.
    #[test]
    fn the_document_settles_and_steps_across_the_seam() {
        let (mut document, scroller) = snapping_page(
            "scroll-snap-type: y mandatory; overscroll-behavior-y: circular;",
            "scroll-snap-align: center;",
            &[50.0; 4],
        );
        assert_eq!(
            document.scroll_to(scroller, Vector2D::new(0.0, 110.0)),
            Vector2D::new(0.0, 110.0)
        );
        assert_eq!(
            document.settle_scroll(scroller, Vector2D::new(0.0, 110.0)),
            Vector2D::new(0.0, 125.0)
        );
        assert_eq!(
            document.settle_scroll(scroller, Vector2D::new(0.0, 125.0)),
            Vector2D::new(0.0, 125.0),
            "at rest on a position"
        );

        document.scroll_to(scroller, Vector2D::new(0.0, 190.0));
        assert_eq!(
            document.settle_scroll(scroller, Vector2D::new(0.0, 190.0)),
            Vector2D::new(0.0, 175.0),
            "the first page, across the seam"
        );
        let page = document
            .get(scroller)
            .and_then(|node| node.child_ids().first().copied())
            .expect("a page");
        assert_eq!(
            document.scroll_chain_directed(page, Vector2D::new(0.0, 10.0)),
            Some((scroller, Vector2D::new(0.0, 10.0))),
        );
        assert_eq!(document.scroll_offset(scroller), Vector2D::new(0.0, 25.0));
        assert_eq!(
            document.scroll_chain_directed(page, Vector2D::new(0.0, -10.0)),
            Some((scroller, Vector2D::new(0.0, -10.0))),
        );
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(0.0, 175.0),
            "and back across it"
        );
    }

    /// §6.2.1: of the areas whose position holds the offset, the first in
    /// tree order — not the first in the table, which is sorted by position.
    #[test]
    fn a_container_is_snapped_to_the_first_area_in_tree_order_at_its_offset() {
        let points = [
            range(0.0, 0.0, 1),
            range(50.0, 250.0, 5),
            range(100.0, 100.0, 4),
            range(100.0, 100.0, 3),
        ];
        for strictness in [SnapStrictness::Mandatory, SnapStrictness::Proximity] {
            let axis = axis(strictness, &points);
            assert_eq!(axis.snapped_area(0.0), Some(area(1)));
            assert_eq!(
                axis.snapped_area(100.0),
                Some(area(3)),
                "three areas hold 100; the first in tree order is the table's last",
            );
            assert_eq!(axis.snapped_area(100.4), Some(area(3)), "within tolerance");
            assert_eq!(axis.snapped_area(180.0), Some(area(5)), "inside the range");
            assert_eq!(axis.snapped_area(30.0), None, "on no position");
            assert_eq!(axis.snapped_area(f32::NAN), None);
        }
    }

    /// §4.1.3 over two commits' positions: the area the container stood on
    /// decides, not the position nearest the offset it was left with.
    #[test]
    fn a_resnap_follows_the_area_to_its_new_position() {
        let before = [
            range(0.0, 0.0, 1),
            range(100.0, 100.0, 2),
            range(200.0, 200.0, 3),
        ];
        let before = axis(SnapStrictness::Mandatory, &before);
        let after = [
            range(0.0, 0.0, 1),
            range(150.0, 150.0, 2),
            range(300.0, 300.0, 3),
        ];
        let after = axis(SnapStrictness::Proximity, &after);
        assert_eq!(
            after.resnap(&before, 200.0, 200.0),
            Some(300.0),
            "the third area, though the second's 150 is nearer 200",
        );
        assert_eq!(after.resnap(&before, 100.0, 100.0), Some(150.0));
        assert_eq!(
            after.resnap(&before, 120.0, 120.0),
            None,
            "stood on nothing"
        );

        let gone = [range(0.0, 0.0, 1), range(150.0, 150.0, 2)];
        let gone = axis(SnapStrictness::Mandatory, &gone);
        assert_eq!(gone.resnap(&before, 200.0, 150.0), None, "its area is gone");

        // Grown past the snapport, the area is a range: the offset in it
        // nearest where the container stands.
        let grown = [range(0.0, 0.0, 1), range(150.0, 400.0, 3)];
        let grown = axis(SnapStrictness::Mandatory, &grown);
        assert_eq!(grown.resnap(&before, 200.0, 200.0), Some(200.0));
        assert_eq!(grown.resnap(&before, 200.0, 100.0), Some(150.0));
    }

    /// On a circular axis the offset may stand any number of periods away,
    /// and the area's new position is the copy nearest it.
    #[test]
    fn a_periodic_resnap_reads_the_offset_modulo_the_old_period() {
        let before = [
            range(0.0, 0.0, 1),
            range(200.0, 200.0, 2),
            range(400.0, 400.0, 3),
        ];
        let before = periodic(SnapStrictness::Mandatory, &before);
        assert_eq!(
            before.snapped_area(1400.0),
            Some(area(2)),
            "200, two periods on"
        );
        assert_eq!(
            before.snapped_area(-200.0),
            Some(area(3)),
            "400, one period back"
        );

        let after = [
            range(0.0, 0.0, 1),
            range(300.0, 300.0, 2),
            range(850.0, 850.0, 3),
        ];
        let after = SnapAxis {
            strictness: SnapStrictness::Mandatory,
            points: &after,
            period: Some(900.0),
        };
        assert_eq!(after.resnap(&before, 1400.0, 500.0), Some(300.0));
        assert_eq!(
            after.resnap(&before, -200.0, 10.0),
            Some(-50.0),
            "850's copy across the seam is the one nearest 10",
        );
    }

    /// The pages of a snapping scroller: its element children.
    fn pages(document: &Document<()>, scroller: NodeId) -> Vec<NodeId> {
        document
            .get(scroller)
            .expect("the scroller")
            .child_ids()
            .to_vec()
    }

    /// Where the newest commit publishes `scroller`.
    fn committed_offset(document: &mut Document<()>, scroller: NodeId) -> Vector2D<f32> {
        let frame = document.commit();
        let slot = frame.slot_of(scroller).expect("the scroller has a slot");
        frame.scroll_slots()[slot as usize].offset
    }

    /// §4.1.3 when the container itself is resized. Four pages, each as tall
    /// as the scrollport, so every position moves with it: 0, 100, 200, 300
    /// at 100px. Standing on the third page (200), a scrollport of 150px
    /// puts that page at 300 — where the position nearest the old offset is
    /// the second page's 150 — and one of 60px puts it at 120.
    #[test]
    fn a_resized_container_stays_on_the_area_it_was_snapped_to() {
        for strictness in ["mandatory", "proximity"] {
            let (mut document, scroller) = snapping_page(
                &format!("scroll-snap-type: y {strictness};"),
                "height: 100%;",
                &[],
            );
            for _ in 0..4 {
                let page = document.create_element("view", ());
                document.add_class(page, "page");
                document.append_child(scroller, page);
            }
            document.commit();
            // As the runtime adopts a painter's post: no request.
            document.scroll_to(scroller, Vector2D::new(0.0, 200.0));

            document.set_inline_style(scroller, "height: 150px");
            assert_eq!(
                committed_offset(&mut document, scroller),
                Vector2D::new(0.0, 300.0),
                "{strictness}: the commit publishes the third page's new position",
            );
            assert_eq!(document.scroll_offset(scroller), Vector2D::new(0.0, 300.0));

            document.set_inline_style(scroller, "height: 60px");
            assert_eq!(
                committed_offset(&mut document, scroller),
                Vector2D::new(0.0, 120.0),
                "{strictness}: and again, from the offset the last one left",
            );
        }
    }

    /// §4.1.3 when an area ahead of the snapped one is resized, and the two
    /// strictnesses' difference: none while the container is snapped. Four
    /// 100px pages in a 100px scrollport, positions 0, 100, 200, 300; the
    /// first page grows to 300px, which moves the second to 300. A container
    /// standing on the second page follows it under `proximity` too, though
    /// 300 is six proximity ranges from 100 (the specification's own
    /// example: a log that stays at its end as lines are added).
    #[test]
    fn a_resized_area_carries_the_container_snapped_past_it() {
        for strictness in ["mandatory", "proximity"] {
            let (mut document, scroller) = snapping_page(
                &format!("scroll-snap-type: y {strictness};"),
                "",
                &[100.0; 4],
            );
            document.commit();
            document.scroll_to(scroller, Vector2D::new(0.0, 100.0));
            let first = pages(&document, scroller)[0];
            document.set_inline_style(first, "height: 300px");
            assert_eq!(
                committed_offset(&mut document, scroller),
                Vector2D::new(0.0, 300.0),
                "{strictness}",
            );
        }
    }

    /// A container that stood on no position is not the document's to move:
    /// whether it is at rest, and so owes the at-rest rule, is the painter's
    /// knowledge. 150 is half-way between two pages, where a `proximity`
    /// container may rest and a `mandatory` one is only ever passing.
    #[test]
    fn an_offset_on_no_position_is_left_to_the_at_rest_rule() {
        for strictness in ["mandatory", "proximity"] {
            let (mut document, scroller) = snapping_page(
                &format!("scroll-snap-type: y {strictness};"),
                "",
                &[100.0; 4],
            );
            document.commit();
            document.scroll_to(scroller, Vector2D::new(0.0, 150.0));
            let first = pages(&document, scroller)[0];
            document.set_inline_style(first, "height: 300px");
            assert_eq!(
                committed_offset(&mut document, scroller),
                Vector2D::new(0.0, 150.0),
                "{strictness}",
            );
        }
    }

    /// The snapped area removed: nothing to follow, so the offset stays —
    /// clamped as any offset is — for the at-rest rule.
    #[test]
    fn a_container_whose_area_is_gone_keeps_its_offset() {
        let (mut document, scroller) =
            snapping_page("scroll-snap-type: y mandatory;", "", &[100.0; 4]);
        document.commit();
        document.scroll_to(scroller, Vector2D::new(0.0, 300.0));
        let last = pages(&document, scroller)[3];
        document.remove_element(last);
        assert_eq!(
            committed_offset(&mut document, scroller),
            Vector2D::new(0.0, 200.0),
            "clamped to the range three pages leave",
        );
    }

    /// §6.2.1 through a commit. Four 50px pages in a 100px scrollport: the
    /// last page's start is clamped to the end of the range, 100, which is
    /// the third page's position too. Standing there the container is
    /// snapped to the third page, first in tree order, and stays on it when
    /// a 50px scrollport separates the two (100 and 150).
    #[test]
    fn two_areas_at_one_position_resnap_to_the_first_in_tree_order() {
        let (mut document, scroller) =
            snapping_page("scroll-snap-type: y mandatory;", "", &[50.0; 4]);
        document.commit();
        document.scroll_to(scroller, Vector2D::new(0.0, 100.0));
        document.set_inline_style(scroller, "height: 50px");
        assert_eq!(
            committed_offset(&mut document, scroller),
            Vector2D::new(0.0, 100.0)
        );
    }

    /// A circular axis: the period changes with the scrollport, and the
    /// area's position is found in the new one. Four full-height pages,
    /// period 400 at 100px and 600 at 150px: the last page moves from 300
    /// to 450, which a clamp to the old range's end would never reach.
    /// Then an area across the seam: four 50px pages centred in the 100px
    /// scrollport put the first at −25, 175 on a period of 200; grown to
    /// 80px it centres at −10, 220 on a period of 230.
    #[test]
    fn a_circular_container_follows_its_area_into_the_new_period() {
        let circular = "scroll-snap-type: y mandatory; overscroll-behavior-y: circular;";
        let (mut document, scroller) = snapping_page(circular, "height: 100%;", &[]);
        for _ in 0..4 {
            let page = document.create_element("view", ());
            document.add_class(page, "page");
            document.append_child(scroller, page);
        }
        document.commit();
        document.scroll_to(scroller, Vector2D::new(0.0, 300.0));
        document.set_inline_style(scroller, "height: 150px");
        assert_eq!(
            committed_offset(&mut document, scroller),
            Vector2D::new(0.0, 450.0)
        );

        let (mut document, scroller) =
            snapping_page(circular, "scroll-snap-align: center;", &[50.0; 4]);
        document.commit();
        assert_eq!(
            document.scroll_to(scroller, Vector2D::new(0.0, 175.0)),
            Vector2D::new(0.0, 175.0),
            "the first page, across the seam",
        );
        let first = pages(&document, scroller)[0];
        document.set_inline_style(first, "height: 80px");
        assert_eq!(
            committed_offset(&mut document, scroller),
            Vector2D::new(0.0, 220.0)
        );
    }

    /// A pending request owns the container's position until the painter
    /// has carried it out, so the commit moves neither the offset nor the
    /// target the slot carries.
    #[test]
    fn a_container_with_a_pending_request_is_left_to_it() {
        let (mut document, scroller) =
            snapping_page("scroll-snap-type: y mandatory;", "", &[100.0; 4]);
        document.commit();
        document.scroll_to_with(
            scroller,
            Vector2D::new(0.0, 100.0),
            crate::scroll::ScrollBehavior::Instant,
        );
        let first = pages(&document, scroller)[0];
        document.set_inline_style(first, "height: 300px");
        let frame = document.commit();
        let slot = &frame.scroll_slots()[frame.slot_of(scroller).expect("a slot") as usize];
        assert_eq!(slot.offset, Vector2D::new(0.0, 100.0));
        assert_eq!(
            slot.request.map(|request| request.target),
            Some(Vector2D::new(0.0, 100.0))
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
