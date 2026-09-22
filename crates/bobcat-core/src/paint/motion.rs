//! Scroll kinematics: the curves a scroll follows once the finger has let
//! go, and how far a `contain-bounce` boundary stretches when a scroll
//! pushes past it. Pure functions over numbers; [`super::inertia`] runs
//! them over the scroll intents on the painter's clock.
//!
//! css-overscroll-1 leaves the boundary effect to the user agent, and
//! nothing in CSS describes inertia at all, so these are the engine's own.
//! The curves and their constants are lynx-ui's `useBounce`
//! (`packages/lynx-ui-scroll-view/src/hooks/useBounce.tsx`), so a page that
//! bounces through `overscroll-behavior: contain-bounce` and one that bounces
//! through that hook's transforms move the same way:
//!
//! - **Rubber band** — a drag past a boundary. With `x` the finger's travel past the edge and `L`
//!   the scrollport's extent on that axis, `d(x) = (1 − 1 / (x·c/L + 1))·L`, `c =`
//!   [`RUBBER_BAND_COEFFICIENT`]: the stretch starts at slope `c` and never reaches `L`. The
//!   inverse, [`rubber_band_travel`], is what lets a drag that finds a container already stretched
//!   continue the same curve from there.
//! - **Fling** — the release velocity decays geometrically per millisecond, `v(t) = v₀·r^t`, so the
//!   distance covered by `t` is `v₀·(r^t − 1)/ln r` and the whole curve covers `−v₀/ln r`. In range
//!   the rate is [`FLING_DECAY_PER_MS`]; past a boundary it is the faster
//!   [`OVERSHOOT_DECAY_PER_MS`], so an overshoot is short.
//! - **Bounce back** — a critically damped spring from displacement `C₁` with no initial velocity:
//!   `x(t) = (C₁ + β·C₁·t)·e^(−β·t)`, `β =` [`BOUNCE_BACK_STIFFNESS`] per second.
//!
//! Rest is one physical pixel ([`rest_threshold`]): a curve whose remaining
//! travel is under it is over, and a stretch under it is none.
//!
//! [`resolve_elastic_step`] is the boundary rule itself — one chain step on
//! a container whose axis may stretch — and is what the intents run in
//! place of [`resolve_step`] alone.

use dom::scroll::{ScrollAxes, ScrollKind, SnapAxis, resolve_step};
use dom::{Size2D, Vector2D};

/// The rubber band's `c`: the slope a stretch starts at. lynx-ui's
/// `rubberC`, which is `UIKit`'s.
pub(crate) const RUBBER_BAND_COEFFICIENT: f32 = 0.55;

/// A fling's in-range decay per millisecond. `UIKit`'s normal deceleration
/// rate; lynx-ui leaves the in-range fling to the native scroller and names
/// no rate of its own.
pub(crate) const FLING_DECAY_PER_MS: f32 = 0.998;

/// A fling's decay per millisecond once it has stretched past a boundary:
/// lynx-ui's `flingDeceleratingRate`.
pub(crate) const OVERSHOOT_DECAY_PER_MS: f32 = 0.99;

/// The bounce back's `β`, per second: lynx-ui's `beta`.
pub(crate) const BOUNCE_BACK_STIFFNESS: f32 = 15.0;

/// What is moving a scroll container through one chain step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Motion {
    /// One step of a finger's drag: raw in range, rubber-banded past a
    /// `contain-bounce` edge.
    Drag,
    /// One step of a fling: raw in range, and past a `contain-bounce` edge
    /// raw too, capped at one scrollport — lynx-ui's overshoot.
    Fling,
    /// A wheel tick: snaps as it lands, and never stretches.
    Wheel,
}

impl Motion {
    /// The css-scroll-snap-1 kind the step is for the snap rules.
    pub(super) fn kind(self) -> ScrollKind {
        match self {
            Self::Drag | Self::Fling => ScrollKind::Gesture,
            Self::Wheel => ScrollKind::Directed,
        }
    }
}

/// One physical pixel in CSS px: the distance under which a curve is over
/// and a stretch is none, and — read as CSS px per millisecond — the
/// velocity under which an overshoot starts its bounce back.
pub(crate) fn rest_threshold(device_pixel_ratio: f32) -> f32 {
    if device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0 {
        1.0 / device_pixel_ratio
    } else {
        1.0
    }
}

/// How far a boundary stretches when the finger has travelled `travel` CSS
/// px past it, for a scrollport of `extent` on that axis. Non-negative in,
/// non-negative out; a non-positive extent stretches nothing.
pub(crate) fn rubber_band(travel: f32, extent: f32) -> f32 {
    // NaN in, nothing out: neither comparison holds.
    if travel <= 0.0 || extent <= 0.0 || travel.is_nan() || extent.is_nan() {
        return 0.0;
    }
    (1.0 - 1.0 / (travel * RUBBER_BAND_COEFFICIENT / extent + 1.0)) * extent
}

/// How fast the stretch grows per px of finger travel at `travel` past the
/// edge: the rubber band's derivative, `c / (x·c/L + 1)²`. What a finger's
/// release velocity is damped by when it lets go mid-stretch.
pub(crate) fn rubber_band_slope(travel: f32, extent: f32) -> f32 {
    if extent <= 0.0 || extent.is_nan() {
        return 0.0;
    }
    let inner = travel.max(0.0) * RUBBER_BAND_COEFFICIENT / extent + 1.0;
    RUBBER_BAND_COEFFICIENT / (inner * inner)
}

/// The finger travel that stretches a boundary by `displacement`: the
/// inverse of [`rubber_band`]. A displacement at or past the extent is
/// unreachable and answers the travel for a hair short of it.
pub(crate) fn rubber_band_travel(displacement: f32, extent: f32) -> f32 {
    if displacement <= 0.0 || extent <= 0.0 || displacement.is_nan() || extent.is_nan() {
        return 0.0;
    }
    let displacement = displacement.min(extent - extent * 1e-3);
    (extent * displacement) / ((extent - displacement) * RUBBER_BAND_COEFFICIENT)
}

/// A fling's velocity `elapsed_ms` after it started at `velocity`, decaying
/// at `rate` per millisecond.
pub(crate) fn fling_velocity(velocity: f32, rate: f32, elapsed_ms: f32) -> f32 {
    velocity * rate.powf(elapsed_ms)
}

/// The distance a fling that started at `velocity` has covered
/// `elapsed_ms` later, decaying at `rate` per millisecond.
pub(crate) fn fling_distance(velocity: f32, rate: f32, elapsed_ms: f32) -> f32 {
    velocity * (rate.powf(elapsed_ms) - 1.0) / rate.ln()
}

/// The whole distance a fling from `velocity` covers before it is spent.
pub(crate) fn fling_travel(velocity: f32, rate: f32) -> f32 {
    -velocity / rate.ln()
}

/// The velocity whose whole fling covers exactly `travel`: how a fling is
/// aimed at a snap position.
pub(crate) fn fling_velocity_for_travel(travel: f32, rate: f32) -> f32 {
    -travel * rate.ln()
}

/// Where a bounce back from `displacement` stands `elapsed_s` seconds in.
/// Critically damped with no initial velocity, so it never crosses the
/// boundary it returns to.
pub(crate) fn bounce_back(displacement: f32, elapsed_s: f32) -> f32 {
    let beta_t = BOUNCE_BACK_STIFFNESS * elapsed_s.max(0.0);
    displacement * (1.0 + beta_t) * (-beta_t).exp()
}

/// The signed stretch of `offset` past `0..=max`: negative past the start
/// edge, positive past the end edge, zero in range.
pub(crate) fn stretch_of(offset: f32, max: f32) -> f32 {
    if offset < 0.0 {
        offset
    } else if offset > max {
        offset - max
    } else {
        0.0
    }
}

/// One resolved chain step on a container that may stretch; see
/// [`resolve_elastic_step`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ElasticStep {
    /// Where the container stands after the step. Outside `0..=max` on an
    /// axis it stretched on.
    pub(super) applied: Vector2D<f32>,
    /// What the container absorbed of the admitted delta — everything, on
    /// an axis it stretched on.
    pub(super) absorbed: Vector2D<f32>,
    /// The axes on which the container is now past a boundary.
    pub(super) stretched: ScrollAxes,
}

/// Resolves one container's chain step where an axis in `bounce` may
/// stretch past its boundary, and answers like [`resolve_step`] where none
/// does.
///
/// A drag step past the boundary is rubber-banded: the finger's travel
/// past the edge — recovered from the current stretch through
/// [`rubber_band_travel`] when the container was already stretched — maps
/// through [`rubber_band`], and a drag that pulls back through the edge
/// spends the rest of its delta scrolling. A fling step stretches raw,
/// capped at one scrollport, the way lynx-ui's overshoot does. A wheel tick
/// never stretches: it first brings a stretched container back to its
/// edge, then lands as any wheel tick does.
#[allow(
    clippy::too_many_arguments,
    reason = "one chain step's whole context: the motion, the geometry, the policy and the snap data"
)]
pub(super) fn resolve_elastic_step(
    motion: Motion,
    offset: Vector2D<f32>,
    admitted: Vector2D<f32>,
    max: Vector2D<f32>,
    scrollport: Size2D<f32>,
    bounce: ScrollAxes,
    snap_x: Option<SnapAxis<'_>>,
    snap_y: Option<SnapAxis<'_>>,
) -> ElasticStep {
    if motion == Motion::Wheel || bounce == ScrollAxes::NONE {
        let settled = Vector2D::new(clamp_axis(offset.x, max.x), clamp_axis(offset.y, max.y));
        let (applied, absorbed) = resolve_step(
            motion.kind(),
            settled,
            admitted,
            max,
            scrollport,
            snap_x,
            snap_y,
        );
        return ElasticStep {
            applied,
            absorbed,
            stretched: ScrollAxes::NONE,
        };
    }
    let axis = |current: f32, delta: f32, max: f32, extent: f32, bounces: bool| {
        if delta == 0.0 {
            return (current, 0.0, current < 0.0 || current > max);
        }
        match motion {
            Motion::Drag if bounces => drag_axis(current, delta, max, extent),
            Motion::Fling if bounces => fling_axis(current, delta, max, extent),
            Motion::Drag | Motion::Fling | Motion::Wheel => {
                let natural = clamp_axis(current + delta, max);
                (natural, natural - clamp_axis(current, max), false)
            }
        }
    };
    let (x, absorbed_x, stretched_x) =
        axis(offset.x, admitted.x, max.x, scrollport.width, bounce.x);
    let (y, absorbed_y, stretched_y) =
        axis(offset.y, admitted.y, max.y, scrollport.height, bounce.y);
    ElasticStep {
        applied: Vector2D::new(x, y),
        absorbed: Vector2D::new(absorbed_x, absorbed_y),
        stretched: ScrollAxes {
            x: stretched_x,
            y: stretched_y,
        },
    }
}

/// Unwinds one stretched axis toward its edge by `delta`, absorbing no
/// more than what brings it home: `(applied, absorbed)`. A delta pushing
/// further out, or an axis in range, is left to the chain walk.
///
/// Lynx's restore-first rule: a container already stretched on the chain
/// unwinds before any delta chains inward, whatever the walk's order.
pub(super) fn unwind_stretch(
    motion: Motion,
    current: f32,
    delta: f32,
    max: f32,
    extent: f32,
) -> (f32, f32) {
    let stretch = stretch_of(current, max);
    if stretch == 0.0 || delta == 0.0 || delta.signum() == stretch.signum() {
        return (current, 0.0);
    }
    let edge = if stretch < 0.0 { 0.0 } else { max };
    match motion {
        Motion::Drag => {
            let travel = stretch.signum() * rubber_band_travel(stretch.abs(), extent) + delta;
            if travel != 0.0 && travel.signum() == stretch.signum() {
                (
                    edge + stretch.signum() * rubber_band(travel.abs(), extent),
                    delta,
                )
            } else {
                (edge, delta - travel)
            }
        }
        Motion::Fling | Motion::Wheel => {
            let natural = current + delta;
            let left = stretch_of(natural, max);
            if left != 0.0 && left.signum() == stretch.signum() {
                (natural, delta)
            } else {
                (edge, edge - current)
            }
        }
    }
}

/// One drag step on a stretching axis: `(applied, absorbed, stretched)`.
fn drag_axis(current: f32, delta: f32, max: f32, extent: f32) -> (f32, f32, bool) {
    let stretch = stretch_of(current, max);
    let mut current = current;
    let mut remaining = delta;
    if stretch != 0.0 {
        let edge = if stretch < 0.0 { 0.0 } else { max };
        // The finger travel that produced the stretch, then this step's on
        // top: still past the edge, or back through it with the rest to
        // spend on scrolling.
        let travel = stretch.signum() * rubber_band_travel(stretch.abs(), extent) + delta;
        if travel != 0.0 && travel.signum() == stretch.signum() {
            let stretched = stretch.signum() * rubber_band(travel.abs(), extent);
            return (edge + stretched, delta, true);
        }
        current = edge;
        remaining = travel;
        if remaining == 0.0 {
            return (edge, delta, false);
        }
    }
    let natural = current + remaining;
    if (0.0..=max).contains(&natural) {
        return (natural, delta, false);
    }
    let edge = if natural < 0.0 { 0.0 } else { max };
    let excess = natural - edge;
    let stretched = excess.signum() * rubber_band(excess.abs(), extent);
    (edge + stretched, delta, true)
}

/// One fling step on a stretching axis: raw, and capped one scrollport
/// past either edge.
fn fling_axis(current: f32, delta: f32, max: f32, extent: f32) -> (f32, f32, bool) {
    let natural = (current + delta).clamp(-extent, max + extent);
    (natural, delta, natural < 0.0 || natural > max)
}

fn clamp_axis(value: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, max.max(0.0))
    } else {
        0.0
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    reason = "the closed forms are checked bit for bit where they are exact, and the loop \
              counters are tiny"
)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3 * b.abs().max(1.0)
    }

    #[test]
    fn the_rubber_band_starts_at_its_coefficient_and_never_reaches_the_extent() {
        let extent = 400.0;
        assert_eq!(rubber_band(0.0, extent), 0.0);
        assert_eq!(rubber_band(-10.0, extent), 0.0);
        assert_eq!(rubber_band(10.0, 0.0), 0.0);
        // One px of travel: (1 − 1/1.001375)·400, a hair under the slope.
        let first = rubber_band(1.0, extent);
        assert!(close(first, 0.5492), "got {first}");
        assert!(first < RUBBER_BAND_COEFFICIENT);
        assert!(rubber_band(100.0, extent) < rubber_band(200.0, extent));
        assert!(rubber_band(1e9, extent) < extent);
        assert!(rubber_band(1e9, extent) > extent * 0.999);
    }

    #[test]
    fn the_rubber_band_and_its_inverse_round_trip() {
        let extent = 300.0;
        for travel in [1.0, 25.0, 80.0, 400.0, 3000.0] {
            let stretch = rubber_band(travel, extent);
            assert!(close(rubber_band_travel(stretch, extent), travel));
        }
        assert_eq!(rubber_band_travel(0.0, extent), 0.0);
        assert!(rubber_band_travel(extent, extent).is_finite());
        // The slope is the coefficient at the edge and falls off past it.
        assert_eq!(rubber_band_slope(0.0, extent), RUBBER_BAND_COEFFICIENT);
        let numeric = rubber_band(100.5, extent) - rubber_band(99.5, extent);
        assert!(close(rubber_band_slope(100.0, extent), numeric));
        assert_eq!(rubber_band_slope(10.0, 0.0), 0.0);
    }

    #[test]
    fn the_fling_curve_is_the_integral_of_its_velocity() {
        let (v0, rate) = (2.0, FLING_DECAY_PER_MS);
        assert_eq!(fling_distance(v0, rate, 0.0), 0.0);
        assert!(close(fling_velocity(v0, rate, 100.0), v0 * rate.powi(100)));
        // Riemann sum against the closed form.
        let mut sum = 0.0;
        for step in 0..1000 {
            sum += fling_velocity(v0, rate, step as f32 + 0.5);
        }
        assert!(close(fling_distance(v0, rate, 1000.0), sum));
        assert!(fling_distance(v0, rate, 2000.0) < fling_travel(v0, rate));
        assert!(close(fling_distance(v0, rate, 1e5), fling_travel(v0, rate)));
        assert!(close(
            fling_travel(fling_velocity_for_travel(250.0, rate), rate),
            250.0
        ));
        // The overshoot rate spends a fling about five times sooner.
        assert!(fling_travel(v0, OVERSHOOT_DECAY_PER_MS) * 4.0 < fling_travel(v0, rate));
    }

    #[test]
    fn the_bounce_back_starts_where_it_was_left_with_no_velocity_and_settles() {
        let c1 = 120.0;
        assert_eq!(bounce_back(c1, 0.0), c1);
        // Zero initial velocity: the first millisecond moves it by the
        // second-order term alone, (β·t)²/2 of the way.
        assert!(bounce_back(c1, 0.001) > c1 * 0.9998);
        assert!(bounce_back(c1, 0.001) < c1);
        let mut previous = c1;
        for step in 1..=100 {
            let now = bounce_back(c1, step as f32 * 0.01);
            assert!(now <= previous && now >= 0.0, "monotone back to the edge");
            previous = now;
        }
        assert!(bounce_back(c1, 1.0) < rest_threshold(3.0));
        assert_eq!(bounce_back(-c1, 0.5), -bounce_back(c1, 0.5));
    }

    #[test]
    fn rest_is_one_physical_pixel() {
        assert_eq!(rest_threshold(2.0), 0.5);
        assert_eq!(rest_threshold(0.0), 1.0);
        assert_eq!(rest_threshold(f32::NAN), 1.0);
    }

    fn step(motion: Motion, offset: f32, delta: f32, bounces: bool) -> (f32, f32, bool) {
        let resolved = resolve_elastic_step(
            motion,
            Vector2D::new(0.0, offset),
            Vector2D::new(0.0, delta),
            Vector2D::new(0.0, 500.0),
            Size2D::new(200.0, 200.0),
            ScrollAxes {
                x: false,
                y: bounces,
            },
            None,
            None,
        );
        (
            resolved.applied.y,
            resolved.absorbed.y,
            resolved.stretched.y,
        )
    }

    #[test]
    fn a_drag_past_the_start_edge_stretches_by_the_rubber_band() {
        let (applied, absorbed, stretched) = step(Motion::Drag, 0.0, -100.0, true);
        assert_eq!(applied, -rubber_band(100.0, 200.0));
        assert_eq!(absorbed, -100.0, "a stretching axis absorbs everything");
        assert!(stretched);
        // The same finger travel in two steps lands on the same stretch.
        let (half, ..) = step(Motion::Drag, 0.0, -40.0, true);
        let (whole, ..) = step(Motion::Drag, half, -60.0, true);
        assert!(close(whole, applied), "{whole} vs {applied}");
    }

    #[test]
    fn a_drag_that_crosses_the_edge_spends_the_rest_scrolling() {
        // 30px in range, then 20px past the end edge.
        let (applied, absorbed, stretched) = step(Motion::Drag, 470.0, 50.0, true);
        assert_eq!(applied, 500.0 + rubber_band(20.0, 200.0));
        assert_eq!(absorbed, 50.0);
        assert!(stretched);
        // Pulled back: the stretch unwinds first, then the container scrolls.
        let (back, absorbed, stretched) = step(Motion::Drag, applied, -50.0, true);
        assert!(close(back, 470.0), "got {back}");
        assert_eq!(absorbed, -50.0);
        assert!(!stretched);
    }

    #[test]
    fn a_fling_stretches_raw_and_no_further_than_a_scrollport() {
        let (applied, absorbed, stretched) = step(Motion::Fling, 490.0, 30.0, true);
        assert_eq!(applied, 520.0);
        assert_eq!(absorbed, 30.0);
        assert!(stretched);
        let (capped, ..) = step(Motion::Fling, 690.0, 100.0, true);
        assert_eq!(capped, 700.0);
        let (in_range, _, stretched) = step(Motion::Fling, 100.0, 30.0, true);
        assert_eq!(in_range, 130.0);
        assert!(!stretched);
    }

    #[test]
    fn a_wheel_tick_and_a_non_bouncing_axis_clamp_like_any_step() {
        let (applied, absorbed, stretched) = step(Motion::Wheel, -40.0, 10.0, true);
        assert_eq!(
            applied, 10.0,
            "the tick first lands the container on its edge"
        );
        assert_eq!(absorbed, 10.0);
        assert!(!stretched);
        let (applied, absorbed, stretched) = step(Motion::Drag, 480.0, 50.0, false);
        assert_eq!(applied, 500.0);
        assert_eq!(absorbed, 20.0);
        assert!(!stretched);
        let (applied, absorbed, _) = step(Motion::Drag, 480.0, 0.0, true);
        assert_eq!((applied, absorbed), (480.0, 0.0));
    }

    #[test]
    fn unwinding_absorbs_only_what_brings_the_stretch_home() {
        // Stretched 40px past the start edge by ~92px of finger travel.
        let stretched = -rubber_band(92.0, 200.0);
        assert_eq!(
            unwind_stretch(Motion::Drag, stretched, -10.0, 500.0, 200.0),
            (stretched, 0.0),
            "pushing further out is the walk's, not an unwind"
        );
        let (applied, absorbed) = unwind_stretch(Motion::Drag, stretched, 50.0, 500.0, 200.0);
        assert!(
            applied < 0.0 && applied > stretched,
            "part way home: {applied}"
        );
        assert_eq!(absorbed, 50.0);
        let (applied, absorbed) = unwind_stretch(Motion::Drag, stretched, 120.0, 500.0, 200.0);
        assert_eq!(applied, 0.0, "home");
        assert!(
            close(absorbed, 92.0),
            "the rest, {} px, chains on",
            120.0 - absorbed
        );
        assert_eq!(
            unwind_stretch(Motion::Fling, 530.0, -20.0, 500.0, 200.0),
            (510.0, -20.0)
        );
        assert_eq!(
            unwind_stretch(Motion::Fling, 530.0, -50.0, 500.0, 200.0),
            (500.0, -30.0)
        );
        assert_eq!(
            unwind_stretch(Motion::Drag, 100.0, -50.0, 500.0, 200.0),
            (100.0, 0.0),
            "in range there is nothing to unwind"
        );
    }
}
