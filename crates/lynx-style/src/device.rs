//! The servo [`Device`] the [`StyleEngine`](crate::StyleEngine) evaluates
//! against, plus the [`EngineMetrics`] describing the Lynx view it models.
//!
//! The device carries the viewport (the Lynx view, *not* the host window), the
//! device-pixel ratio, and the Lynx-specific `screen_width` (`rpx` basis) and
//! `font_scale` (`sp` basis). All of these stay live: mutating them and
//! re-resolving is enough for `rpx`/`ppx`/`sp`/`vw`/`vh` to follow, with no
//! re-ingestion (exercised more fully from M6).

use euclid::{Scale, Size2D};
use stylo::context::QuirksMode;
use stylo::device::Device;
use stylo::device::servo::FontMetricsProvider;
use stylo::font_metrics::FontMetrics;
use stylo::media_queries::MediaType;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::queries::values::PrefersColorScheme;
use stylo::servo::media_features::PointerCapabilities;
use stylo::values::computed::font::GenericFontFamily;
use stylo::values::computed::{CSSPixelLength, Length};
use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
use stylo_traits::{CSSPixel, DevicePixel};

/// The environment metrics the style engine builds its [`Device`] from.
///
/// Lengths are in CSS pixels except `device_pixel_ratio` (a scale).
///
/// # Process-global Lynx unit bases
///
/// `screen_width`, `device_pixel_ratio`, and `font_scale` feed the stylo
/// fork's **process-global** Lynx unit metrics
/// (`stylo::values::specified::lynx_units`): `rpx`/`ppx`/`sp` in *every*
/// engine in the process resolve against whichever engine set them last.
/// Run one `StyleEngine` per process (the Lynx model — all views share one
/// physical screen), or ensure multiple engines share identical metrics.
///
/// `font_scale` currently scales **only** the `sp` unit. Native Lynx can
/// additionally scale other font-relevant lengths by `font_scale` (unless
/// its `font-scale-sp-only` mode is set); that behavior belongs to the text
/// engine and is intentionally not modeled here yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineMetrics {
    /// The Lynx view width, in CSS pixels (the basis for `vw`, `%`).
    pub viewport_width: f32,
    /// The Lynx view height, in CSS pixels (the basis for `vh`).
    pub viewport_height: f32,
    /// CSS-pixel → device-pixel ratio (the basis for `ppx` = `1/dpr`).
    pub device_pixel_ratio: f32,
    /// The screen width, in CSS pixels, that `1rpx = screen_width / 750`
    /// resolves against.
    pub screen_width: f32,
    /// The font scale that `1sp = font_scale px` resolves against.
    pub font_scale: f32,
}

impl EngineMetrics {
    /// Metrics for a `width`×`height` Lynx view at the given device-pixel ratio,
    /// with `rpx` resolving against `width` and no font scaling.
    #[must_use]
    pub fn new(width: f32, height: f32, device_pixel_ratio: f32) -> Self {
        Self {
            viewport_width: width,
            viewport_height: height,
            device_pixel_ratio,
            screen_width: width,
            font_scale: 1.0,
        }
    }
}

/// Build a servo [`Device`] from [`EngineMetrics`].
pub(crate) fn build_device(metrics: EngineMetrics) -> Device {
    let default_values = ComputedValues::initial_values_with_font_override(Font::initial_values());

    let viewport = Size2D::<f32, CSSPixel>::new(metrics.viewport_width, metrics.viewport_height);
    let device_size = Size2D::<f32, DevicePixel>::new(
        metrics.viewport_width * metrics.device_pixel_ratio,
        metrics.viewport_height * metrics.device_pixel_ratio,
    );
    let device_pixel_ratio = Scale::<f32, CSSPixel, DevicePixel>::new(metrics.device_pixel_ratio);

    // rpx/ppx/sp resolve against these process-global metrics, not the Device
    // (see `vendor/stylo` `values/specified/lynx_units.rs`): they are plain
    // length units, set at engine init and on metric change (+ restyle).
    stylo::values::specified::lynx_units::set_lynx_unit_metrics(
        metrics.screen_width,
        metrics.device_pixel_ratio,
        metrics.font_scale,
    );

    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        viewport,
        device_size,
        device_pixel_ratio,
        Box::new(LynxFontMetricsProvider),
        default_values,
        PrefersColorScheme::Light,
        // Lynx surfaces are touch-first: a coarse, non-hovering primary
        // pointer. `PointerCapabilities::default()` would reflect the *build
        // host* instead. (Lynx's own `hover`/`pointer` media features are
        // dead even natively; this only matters once @media is wired.)
        PointerCapabilities::COARSE,
        PointerCapabilities::COARSE,
    )
}

/// A minimal [`FontMetricsProvider`]: enough to compute font-relative units and
/// `line-height: normal` without a real shaper (that lives in the future
/// `lynx-text-engine`).
#[derive(Debug)]
struct LynxFontMetricsProvider;

impl FontMetricsProvider for LynxFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        base_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics {
            ascent: Length::new(base_size.px()),
            ..FontMetrics::default()
        }
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        Length::new(FONT_MEDIUM_PX)
    }
}
