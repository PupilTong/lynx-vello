//! Scroll kinematics: the curves a scroll follows once the finger has let
//! go, and how far a `contain-bounce` boundary stretches when a scroll
//! pushes past it. Pure functions over numbers; [`super::inertia`] runs
//! them over the scroll intents on the painter's clock.
//!
//! css-overscroll-1 leaves the boundary effect to the user agent, and
//! nothing in CSS describes inertia at all, so these are the engine's own:
//! a rubber band with `UIKit`'s coefficient, flings that decay at `UIKit`'s
//! deceleration rates, and a critically damped spring.
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
//! - **Glide** — the same spring toward a target inside the range, from displacement `C` with an
//!   initial velocity `v` of the displacement: `x(t) = (C + (v + β·C)·t)·e^(−β·t)`. With `v = 0` it
//!   is the bounce back. What moves a container to a snap position after a release, and to the
//!   target of a smooth programmatic scroll. `v` is limited ([`glide_velocity`]) so the curve never
//!   crosses its target.
//!
//! Rest is one physical pixel ([`rest_threshold`]): a curve whose remaining
//! travel is under it is over, and a stretch under it is none.
//!
//! [`resolve_elastic_step`] is the boundary rule itself — one chain step on
//! a container whose axis may stretch — and is what the intents run in
//! place of [`resolve_step`] alone.
//!
//! An `overscroll-behavior: circular` axis has no boundary at all, so none
//! of the curves above ever meets an edge there: a drag, a fling or a wheel
//! tick on it moves the offset along an unbounded line and absorbs its
//! whole delta, so nothing stretches and nothing chains outward. The line
//! repeats the scrolling area every [`crate::visual::ScrollSlot::wrap_period`]; the
//! intents normalize the offset into one period when a commit is adopted,
//! and this crate normalizes it wherever it composes, hit-tests or samples, so
//! these functions never wrap anything themselves.

use euclid::default::{Size2D, Vector2D};

use crate::scroll::{ScrollAxes, ScrollKind, SnapAxis, resolve_step};

pub(super) const RUBBER_BAND_COEFFICIENT: f32 = 0.55;

pub(super) const FLING_DECAY_PER_MS: f32 = 0.998;

pub(super) const OVERSHOOT_DECAY_PER_MS: f32 = 0.99;

pub(super) const BOUNCE_BACK_STIFFNESS: f32 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Motion {
    Drag,
    Fling,
    Wheel,
}

impl Motion {
    pub(super) fn kind(self) -> ScrollKind {
        match self {
            Self::Drag | Self::Fling => ScrollKind::Gesture,
            Self::Wheel => ScrollKind::Directed,
        }
    }
}

pub(super) fn rest_threshold(device_pixel_ratio: f32) -> f32 {
    if device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0 {
        1.0 / device_pixel_ratio
    } else {
        1.0
    }
}

pub(super) fn rubber_band(travel: f32, extent: f32) -> f32 {
    if travel <= 0.0 || extent <= 0.0 || travel.is_nan() || extent.is_nan() {
        return 0.0;
    }
    (1.0 - 1.0 / (travel * RUBBER_BAND_COEFFICIENT / extent + 1.0)) * extent
}

pub(super) fn rubber_band_slope(travel: f32, extent: f32) -> f32 {
    if extent <= 0.0 || extent.is_nan() {
        return 0.0;
    }
    let inner = travel.max(0.0) * RUBBER_BAND_COEFFICIENT / extent + 1.0;
    RUBBER_BAND_COEFFICIENT / (inner * inner)
}

pub(super) fn rubber_band_travel(displacement: f32, extent: f32) -> f32 {
    if displacement <= 0.0 || extent <= 0.0 || displacement.is_nan() || extent.is_nan() {
        return 0.0;
    }
    let displacement = displacement.min(extent - extent * 1e-3);
    (extent * displacement) / ((extent - displacement) * RUBBER_BAND_COEFFICIENT)
}

pub(super) fn fling_velocity(velocity: f32, rate: f32, elapsed_ms: f32) -> f32 {
    velocity * rate.powf(elapsed_ms)
}

pub(super) fn fling_distance(velocity: f32, rate: f32, elapsed_ms: f32) -> f32 {
    velocity * (rate.powf(elapsed_ms) - 1.0) / rate.ln()
}

pub(super) fn fling_travel(velocity: f32, rate: f32) -> f32 {
    -velocity / rate.ln()
}

pub(super) fn fling_velocity_for_travel(travel: f32, rate: f32) -> f32 {
    -travel * rate.ln()
}

pub(super) fn bounce_back(displacement: f32, elapsed_s: f32) -> f32 {
    glide(displacement, 0.0, elapsed_s)
}

pub(super) fn glide(displacement: f32, velocity: f32, elapsed_s: f32) -> f32 {
    let t = elapsed_s.max(0.0);
    let beta_t = BOUNCE_BACK_STIFFNESS * t;
    (displacement + (velocity + BOUNCE_BACK_STIFFNESS * displacement) * t) * (-beta_t).exp()
}

pub(super) fn glide_velocity(displacement: f32, velocity: f32) -> f32 {
    if !velocity.is_finite() || velocity * displacement >= 0.0 {
        return 0.0;
    }
    let limit = BOUNCE_BACK_STIFFNESS * displacement.abs();
    velocity.clamp(-limit, limit)
}

pub(super) fn stretch_of(offset: f32, max: f32) -> f32 {
    if offset < 0.0 {
        offset
    } else if offset > max {
        offset - max
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ElasticStep {
    pub(super) applied: Vector2D<f32>,
    pub(super) absorbed: Vector2D<f32>,
    pub(super) stretched: ScrollAxes,
}

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
    wrap: ScrollAxes,
    snap_x: Option<SnapAxis<'_>>,
    snap_y: Option<SnapAxis<'_>>,
) -> ElasticStep {
    if motion == Motion::Wheel || bounce == ScrollAxes::NONE {
        let settled = Vector2D::new(
            settle_axis(offset.x, max.x, wrap.x),
            settle_axis(offset.y, max.y, wrap.y),
        );
        let (applied, absorbed) = resolve_step(
            motion.kind(),
            settled,
            admitted,
            max,
            scrollport,
            snap_x,
            snap_y,
            wrap,
        );
        return ElasticStep {
            applied,
            absorbed,
            stretched: ScrollAxes::NONE,
        };
    }
    let axis = |current: f32, delta: f32, max: f32, extent: f32, bounces: bool, wraps: bool| {
        if wraps {
            return (current + delta, delta, false);
        }
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
    let (x, absorbed_x, stretched_x) = axis(
        offset.x,
        admitted.x,
        max.x,
        scrollport.width,
        bounce.x,
        wrap.x,
    );
    let (y, absorbed_y, stretched_y) = axis(
        offset.y,
        admitted.y,
        max.y,
        scrollport.height,
        bounce.y,
        wrap.y,
    );
    ElasticStep {
        applied: Vector2D::new(x, y),
        absorbed: Vector2D::new(absorbed_x, absorbed_y),
        stretched: ScrollAxes {
            x: stretched_x,
            y: stretched_y,
        },
    }
}

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

fn drag_axis(current: f32, delta: f32, max: f32, extent: f32) -> (f32, f32, bool) {
    let stretch = stretch_of(current, max);
    let mut current = current;
    let mut remaining = delta;
    if stretch != 0.0 {
        let edge = if stretch < 0.0 { 0.0 } else { max };
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

fn settle_axis(value: f32, max: f32, wraps: bool) -> f32 {
    match (wraps, value.is_finite()) {
        (true, true) => value,
        (true, false) => 0.0,
        (false, _) => clamp_axis(value, max),
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
        assert!(fling_travel(v0, OVERSHOOT_DECAY_PER_MS) * 4.0 < fling_travel(v0, rate));
    }

    #[test]
    fn the_bounce_back_starts_where_it_was_left_with_no_velocity_and_settles() {
        let c1 = 120.0;
        assert_eq!(bounce_back(c1, 0.0), c1);
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
    fn a_glide_from_rest_is_the_bounce_back() {
        for t in [0.0, 0.01, 0.1, 0.3, 1.0] {
            assert_eq!(glide(80.0, 0.0, t), bounce_back(80.0, t));
        }
    }

    #[test]
    fn a_glide_keeps_its_velocity_toward_the_target_and_never_crosses_it() {
        let c = 120.0;
        assert_eq!(glide_velocity(c, -500.0), -500.0);
        assert_eq!(glide_velocity(c, -1e6), -BOUNCE_BACK_STIFFNESS * c);
        assert_eq!(glide_velocity(-c, 1e6), BOUNCE_BACK_STIFFNESS * c);
        assert_eq!(glide_velocity(c, 500.0), 0.0);
        assert_eq!(glide_velocity(0.0, -500.0), 0.0);
        assert_eq!(glide_velocity(c, f32::NAN), 0.0);
        for v in [0.0, -500.0, -BOUNCE_BACK_STIFFNESS * c] {
            let v = glide_velocity(c, v);
            assert_eq!(glide(c, v, 0.0), c);
            let slope = (glide(c, v, 1e-4) - c) / 1e-4;
            assert!((slope - v).abs() < 5.0, "slope {slope} for {v}");
            let mut previous = c;
            for step in 1..=200 {
                let now = glide(c, v, step as f32 * 0.005);
                assert!(now <= previous && now >= 0.0, "monotone onto the target");
                previous = now;
            }
            assert!(glide(c, v, 1.0) < rest_threshold(3.0));
        }
        let v = -BOUNCE_BACK_STIFFNESS * c;
        assert!(close(
            glide(c, v, 0.2),
            c * (-BOUNCE_BACK_STIFFNESS * 0.2).exp()
        ));
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
            ScrollAxes::NONE,
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
        let (half, ..) = step(Motion::Drag, 0.0, -40.0, true);
        let (whole, ..) = step(Motion::Drag, half, -60.0, true);
        assert!(close(whole, applied), "{whole} vs {applied}");
    }

    #[test]
    fn a_drag_that_crosses_the_edge_spends_the_rest_scrolling() {
        let (applied, absorbed, stretched) = step(Motion::Drag, 470.0, 50.0, true);
        assert_eq!(applied, 500.0 + rubber_band(20.0, 200.0));
        assert_eq!(absorbed, 50.0);
        assert!(stretched);
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

    fn wrapped_step(
        motion: Motion,
        offset: f32,
        delta: f32,
        snap_y: Option<SnapAxis<'_>>,
    ) -> ElasticStep {
        resolve_elastic_step(
            motion,
            Vector2D::new(0.0, offset),
            Vector2D::new(0.0, delta),
            Vector2D::new(0.0, 500.0),
            Size2D::new(200.0, 200.0),
            ScrollAxes { x: true, y: false },
            ScrollAxes { x: false, y: true },
            None,
            snap_y,
        )
    }

    #[test]
    fn a_drag_or_fling_on_a_wrapped_axis_runs_past_either_end_and_absorbs_everything() {
        for motion in [Motion::Drag, Motion::Fling] {
            let step = wrapped_step(motion, 480.0, 50.0, None);
            assert_eq!(step.applied.y, 530.0, "{motion:?}");
            assert_eq!(step.absorbed.y, 50.0, "{motion:?}");
            assert_eq!(step.stretched, ScrollAxes::NONE, "{motion:?}");
            let step = wrapped_step(motion, 20.0, -260.0, None);
            assert_eq!(step.applied.y, -240.0, "{motion:?}");
            assert_eq!(step.absorbed.y, -260.0, "{motion:?}");
            assert!(!step.stretched.y, "{motion:?}");
        }
        let plain = resolve_elastic_step(
            Motion::Drag,
            Vector2D::new(0.0, 900.0),
            Vector2D::new(0.0, 150.0),
            Vector2D::new(0.0, 500.0),
            Size2D::new(200.0, 200.0),
            ScrollAxes::NONE,
            ScrollAxes { x: false, y: true },
            None,
            None,
        );
        assert_eq!(plain.applied.y, 1050.0, "an offset past the end is kept");
        assert_eq!(plain.absorbed.y, 150.0);
    }

    #[test]
    fn a_wheel_tick_on_a_wrapped_axis_moves_on_to_the_next_periodic_snap_position() {
        let points = [0.0, 200.0, 400.0].map(|at| crate::scroll::SnapPoint {
            min: at,
            max: at,
            stop: false,
            area: crate::tree::document::NodeId::from_bits(1).expect("a well-formed handle"),
            order: 0,
        });
        let snap = SnapAxis {
            strictness: crate::scroll::SnapStrictness::Mandatory,
            points: &points,
            period: Some(700.0),
        };
        let step = wrapped_step(Motion::Wheel, 400.0, 30.0, Some(snap));
        assert_eq!(step.applied.y, 700.0);
        assert_eq!(step.absorbed.y, 30.0);
        let step = wrapped_step(Motion::Wheel, 0.0, -30.0, Some(snap));
        assert_eq!(step.applied.y, -300.0);
        let step = wrapped_step(Motion::Wheel, 490.0, 30.0, None);
        assert_eq!(step.applied.y, 520.0);
        assert_eq!(step.absorbed.y, 30.0);
        assert!(!step.stretched.y);
    }

    #[test]
    fn unwinding_absorbs_only_what_brings_the_stretch_home() {
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
