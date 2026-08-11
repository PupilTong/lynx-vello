//! CSS `filter` approximations (filter-effects-1) as blend-mode composites
//! drawn over the group's content inside its isolated layer.
//!
//! The walker calls [`apply`] after a group's content items, inside the
//! innermost effect layer, so each adjustment composites against exactly
//! the group's own pixels. vello blends are per-layer, so one adjustment =
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
//! - `blur(…)`: needs an offscreen texture pass — ignored (recorded v1 limit; the fork parses no
//!   other functions).
//!
//! Filter *chains* parse in the fork (filter-effects-1 order); each
//! function applies in list order — successive `SrcAtop` draws compose
//! naturally.

use stylo::properties::ComputedValues;

use crate::vello::Scene;
use crate::vello::kurbo::{Affine, Rect};
use crate::vello::peniko::{BlendMode, Color, Compose, Fill, Mix};

type Filter = stylo::values::computed::effects::Filter;

pub(crate) fn apply(scene: &mut Scene, style: &ComputedValues, bounds: Rect, transform: Affine) {
    for filter in style.get_effects().filter.0.iter() {
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
    fn blur_is_ignored() {
        assert!(adjustment_pass(&Filter::Blur(NonNegative(Length::new(4.0)))).is_none());
    }
}
