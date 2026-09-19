//! CSS `filter` (filter-effects-1): the color adjustments as blend-mode
//! composites, and `blur()` as an offscreen bake the compose program carries.
//!
//! One element's `filter` list splits at its **first** `blur()`, which is
//! what [`plan`] answers. The walker opens the group's blur scope inside the
//! scope's own layers, so at scope close the composite order is: the passes
//! *before* the blur (drawn inside the bake), then the blur itself, then the
//! passes *after* it (drawn over the blurred result), then the scope's mask /
//! clip-path / opacity pops.
//!
//! # `blur()`
//!
//! `blur(<length>)`'s length is σ, a standard deviation in the element's own
//! CSS px. The element and its descendants — its background and border
//! included — are one group, and the filter applies to that group's composed
//! pixels. Edge mode is transparent black, and ink overflows the group's
//! bounds by 3σ, which is what the walker inflates the group's layer bounds
//! and the cull region by.
//!
//! Two recorded approximations:
//!
//! - **Chain folding.** A second `blur()` in one list folds into the first by variance addition (σ²
//!   sum): `blur(3px) blur(4px)` bakes one σ = 5 pass. That is exact for two *consecutive* blurs (a
//!   gaussian convolved with a gaussian is a gaussian), and an approximation when a color pass sits
//!   between them — the passes between two blurs are applied inside the one bake instead of between
//!   the two.
//! - **Isotropic σ under a transform.** The bake happens in device space, so σ is scaled by the
//!   arithmetic mean of the two singular values of the group's local→viewport linear map (see
//!   `walker::layer_blur_sigma`). Exact under rotation and uniform scale; a non-uniform scale or a
//!   skew gets one isotropic σ where the spec's filter region would be anisotropic.
//!
//! # Color adjustments
//!
//! The walker calls [`apply`] at a group's scope close, inside the innermost
//! effect layer, so each adjustment composites against exactly the group's
//! own pixels. vello blends are per-layer, so one adjustment =
//! `push_layer(BlendMode::new(mix, Compose::SrcAtop), alpha, bounds)` +
//! flat gray fill of `bounds` + `pop_layer` (`SrcAtop`: modify color where
//! content exists, leave alpha untouched; with layer alpha `a` the covered
//! result is `mix(c, gray)·a + c·(1−a)`). The walker guarantees no clip
//! layer is open here, so these blend layers are safe (vello #1198).
//!
//! Mapping (recorded approximations — behavioral, not colorimetric):
//! - `grayscale(f)`: gray fill with `Mix::Saturation`, alpha `f` (`grayscale(1)` ≡ `saturate(0)`;
//!   HSL-saturation removal, not the spec's luminance-weighted matrix).
//! - `saturate(f)`, `f < 1`: same as grayscale with alpha `1 − f`; `f ≥ 1`: `Mix::Saturation`
//!   cannot oversaturate — skipped.
//! - `brightness(f)`, `f < 1`: gray(`f`) fill with `Mix::Multiply` (`c·f` — exact for opaque
//!   content; the opaque fill's `SrcAtop` composite leaks toward gray inside semi-transparent
//!   pixels, where the spec would scale the premultiplied color). `f > 1`: gray(`1 − 1/f`) fill
//!   with `Mix::Screen` (`c + (1−c)·(1−1/f)` — correct endpoints, compressed midtones).
//! - `contrast(f)`, `f < 1`: gray(0.5) fill with `Mix::Normal` at alpha `1 − f` — the linear pivot
//!   identity `c′ = c·f + 0.5·(1−f)` exactly (one normal pass at alpha `1 − f` *is* that affine
//!   mix; it subsumes the `Mix::Multiply`-then-`Compose::Plus` two-pass derivation of the same
//!   identity, whose `Plus` fill would also bleed gray over the zero-alpha parts of `bounds`). `f >
//!   1` needs slope > 1 around the pivot, which no flat blend expresses — every coefficient of the
//!   identity clamps out of range (multiply ≤ 1, additive ≥ 0, alpha ≥ 0) and the pass degenerates
//!   to a no-op, so it is skipped (recorded clamp error).
//!
//! Filter *chains* parse in the fork (filter-effects-1 order); each
//! function applies in list order — successive `SrcAtop` draws compose
//! naturally.

use std::ops::Range;

use stylo::properties::ComputedValues;

use crate::vello::Scene;
use crate::vello::kurbo::{Affine, Rect};
use crate::vello::peniko::{BlendMode, Color, Compose, Fill, Mix};

type Filter = stylo::values::computed::effects::Filter;

/// One element's `filter` list, split at its first `blur()`.
///
/// `before` and `after` are index ranges into the computed list, and are
/// exactly the two halves [`apply`] is called with. With no `blur()` the list
/// is entirely `after`, which reproduces the pre-blur behavior: every
/// adjustment draws at scope close, over the group's own pixels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FilterPlan {
    /// Adjustments preceding the first `blur()` — drawn inside the bake.
    pub(crate) before: Range<usize>,
    /// Adjustments following it — drawn over the blurred result.
    pub(crate) after: Range<usize>,
    /// The folded blur σ in the element's own CSS px: `sqrt(Σ σᵢ²)` over
    /// every `blur()` in the list. `None` when the list holds none, and
    /// never zero or non-finite — a σ that would blur nothing is no blur.
    pub(crate) sigma: Option<f32>,
}

/// Splits `style`'s `filter` list at its first `blur()` and folds every
/// `blur()` σ into one.
pub(crate) fn plan(style: &ComputedValues) -> FilterPlan {
    split(&style.get_effects().filter.0)
}

/// [`plan`] over the list alone, so the split is checkable without a cascade.
fn split(filters: &[Filter]) -> FilterPlan {
    let len = filters.len();
    let mut first_blur = None;
    let mut variance = 0.0_f32;
    for (index, filter) in filters.iter().enumerate() {
        if let Filter::Blur(radius) = filter {
            let sigma = radius.0.px();
            if sigma > 0.0 && sigma.is_finite() {
                variance += sigma * sigma;
                if first_blur.is_none() {
                    first_blur = Some(index);
                }
            }
        }
    }
    let Some(split) = first_blur else {
        return FilterPlan {
            before: 0..0,
            after: 0..len,
            sigma: None,
        };
    };
    FilterPlan {
        before: 0..split,
        after: (split + 1)..len,
        sigma: Some(variance.sqrt()),
    }
}

/// Draws the adjustment passes of `range`, in list order.
pub(crate) fn apply(
    scene: &mut Scene,
    style: &ComputedValues,
    range: Range<usize>,
    bounds: Rect,
    transform: Affine,
) {
    let filters = &style.get_effects().filter.0;
    let range = range.start.min(filters.len())..range.end.min(filters.len());
    for filter in &filters[range] {
        let Some(pass) = adjustment_pass(filter) else {
            continue;
        };
        scene.push_layer(
            Fill::NonZero,
            BlendMode::new(pass.mix, Compose::SrcAtop),
            pass.alpha,
            transform,
            &bounds,
        );
        scene.fill(Fill::NonZero, transform, gray(pass.level), None, &bounds);
        scene.pop_layer();
    }
}

/// One filter function's blend pass: a flat achromatic fill composited
/// `SrcAtop` under `mix` at the layer's `alpha`.
#[derive(Debug, PartialEq)]
struct Pass {
    mix: Mix,
    alpha: f32,
    level: f32,
}

fn adjustment_pass(filter: &Filter) -> Option<Pass> {
    match filter {
        Filter::Grayscale(amount) => {
            let f = amount.0.clamp(0.0, 1.0);
            (f > 0.0).then_some(Pass {
                mix: Mix::Saturation,
                alpha: f,
                level: 0.5,
            })
        }
        Filter::Saturate(amount) => {
            let f = amount.0.max(0.0);
            (f < 1.0).then_some(Pass {
                mix: Mix::Saturation,
                alpha: 1.0 - f,
                level: 0.5,
            })
        }
        Filter::Brightness(amount) => {
            let f = amount.0.max(0.0);
            if f < 1.0 {
                Some(Pass {
                    mix: Mix::Multiply,
                    alpha: 1.0,
                    level: f,
                })
            } else if f > 1.0 {
                Some(Pass {
                    mix: Mix::Screen,
                    alpha: 1.0,
                    level: 1.0 - 1.0 / f,
                })
            } else {
                None
            }
        }
        Filter::Contrast(amount) => {
            let f = amount.0.max(0.0);
            (f < 1.0).then_some(Pass {
                mix: Mix::Normal,
                alpha: 1.0 - f,
                level: 0.5,
            })
        }
        Filter::Blur(_)
        | Filter::HueRotate(_)
        | Filter::Invert(_)
        | Filter::Opacity(_)
        | Filter::Sepia(_)
        | Filter::DropShadow(_) => None,
        Filter::Url(url) => match *url {},
    }
}

fn gray(level: f32) -> Color {
    Color::new([level, level, level, 1.0])
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::values::computed::Length;
    use stylo::values::generics::{NonNegative, ZeroToOne};

    use super::*;

    #[test]
    fn grayscale_is_saturation_removal_at_alpha_f() {
        let pass = adjustment_pass(&Filter::Grayscale(ZeroToOne(0.6))).unwrap();
        assert_eq!(pass.mix, Mix::Saturation);
        assert!((pass.alpha - 0.6).abs() < 1e-6);
        assert!(adjustment_pass(&Filter::Grayscale(ZeroToOne(0.0))).is_none());
    }

    #[test]
    fn saturate_below_one_desaturates_and_above_skips() {
        let pass = adjustment_pass(&Filter::Saturate(NonNegative(0.25))).unwrap();
        assert_eq!(pass.mix, Mix::Saturation);
        assert!((pass.alpha - 0.75).abs() < 1e-6);
        assert!(adjustment_pass(&Filter::Saturate(NonNegative(1.0))).is_none());
        assert!(adjustment_pass(&Filter::Saturate(NonNegative(2.0))).is_none());
    }

    #[test]
    fn brightness_multiplies_down_and_screens_up() {
        let dim = adjustment_pass(&Filter::Brightness(NonNegative(0.5))).unwrap();
        assert_eq!(
            dim,
            Pass {
                mix: Mix::Multiply,
                alpha: 1.0,
                level: 0.5
            }
        );
        let boost = adjustment_pass(&Filter::Brightness(NonNegative(2.0))).unwrap();
        assert_eq!(boost.mix, Mix::Screen);
        assert!((boost.level - 0.5).abs() < 1e-6);
        assert!(adjustment_pass(&Filter::Brightness(NonNegative(1.0))).is_none());
    }

    #[test]
    fn contrast_below_one_mixes_toward_mid_gray() {
        let pass = adjustment_pass(&Filter::Contrast(NonNegative(0.25))).unwrap();
        assert_eq!(pass.mix, Mix::Normal);
        assert!((pass.alpha - 0.75).abs() < 1e-6);
        assert!((pass.level - 0.5).abs() < 1e-6);
        let mixed = pass.level * pass.alpha + 1.0 * (1.0 - pass.alpha);
        assert!((mixed - (1.0 * 0.25 + 0.5 * 0.75)).abs() < 1e-6);
    }

    #[test]
    fn contrast_at_or_above_one_is_skipped() {
        assert!(adjustment_pass(&Filter::Contrast(NonNegative(1.0))).is_none());
        assert!(adjustment_pass(&Filter::Contrast(NonNegative(1.5))).is_none());
    }

    #[test]
    fn blur_draws_no_adjustment_pass_of_its_own() {
        assert!(adjustment_pass(&Filter::Blur(NonNegative(Length::new(4.0)))).is_none());
    }

    fn blur(sigma: f32) -> Filter {
        Filter::Blur(NonNegative(Length::new(sigma)))
    }

    #[test]
    fn a_list_with_no_blur_applies_whole_at_scope_close() {
        let plan = split(&[
            Filter::Grayscale(ZeroToOne(1.0)),
            Filter::Contrast(NonNegative(0.5)),
        ]);
        assert_eq!(plan.sigma, None);
        assert_eq!(plan.before, 0..0);
        assert_eq!(plan.after, 0..2);
    }

    #[test]
    fn the_split_puts_earlier_passes_inside_the_bake() {
        let plan = split(&[
            Filter::Grayscale(ZeroToOne(1.0)),
            blur(4.0),
            Filter::Contrast(NonNegative(0.5)),
        ]);
        assert_eq!(plan.sigma, Some(4.0));
        assert_eq!(plan.before, 0..1, "grayscale is baked before the blur");
        assert_eq!(plan.after, 2..3, "contrast lands on the blurred result");
    }

    #[test]
    fn several_blurs_fold_by_variance_at_the_first_one() {
        let plan = split(&[blur(3.0), Filter::Saturate(NonNegative(0.5)), blur(4.0)]);
        assert_eq!(plan.before, 0..0);
        assert_eq!(plan.after, 1..3);
        let sigma = plan.sigma.expect("the list holds two blurs");
        assert!((sigma - 5.0).abs() < 1e-5, "3² + 4² = 5² ({sigma})");
    }

    #[test]
    fn a_zero_or_non_finite_blur_is_no_blur_at_all() {
        assert_eq!(split(&[blur(0.0)]).sigma, None);
        assert_eq!(split(&[blur(f32::INFINITY)]).sigma, None);
        // The list still splits at the first blur that carries a σ.
        let plan = split(&[blur(0.0), blur(2.0)]);
        assert_eq!(plan.sigma, Some(2.0));
        assert_eq!(plan.before, 0..1);
    }
}
