//! The servo [`Device`] the [`StyleEngine`](crate::StyleEngine) evaluates
//! against, plus the [`EngineMetrics`] describing the Lynx view it models.
//!
//! The device carries the viewport (the Lynx view, *not* the host window) and
//! the device-pixel ratio. The Lynx `rpx` unit is viewport-relative (`1rpx =
//! viewport width / 750`), so it resolves off the same viewport as `vw`/`vh`:
//! mutating the viewport and re-resolving is enough for all of them to follow,
//! with no re-ingestion (exercised more fully from M6).

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
/// Lengths are in CSS pixels except `device_pixel_ratio` (a scale). Every
/// metric lives on the [`Device`], so engines are independent — there is no
/// process-global unit state, and multiple `StyleEngine`s can coexist with
/// different metrics.
///
/// The Lynx `rpx` unit is viewport-relative (`1rpx = viewport width / 750`,
/// i.e. `N rpx == N/7.5 vw`), so it resolves off `viewport_width` — the same
/// basis as `vw`, with no separate screen dimension. (Lynx's `ppx`/`sp` units
/// are deliberately unsupported by the fork.)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineMetrics {
    /// The Lynx view width, in CSS pixels (the basis for `vw`, `%`, and the
    /// viewport-relative `rpx`: `1rpx = viewport_width / 750`).
    pub viewport_width: f32,
    /// The Lynx view height, in CSS pixels (the basis for `vh`).
    pub viewport_height: f32,
    /// CSS-pixel → device-pixel ratio.
    pub device_pixel_ratio: f32,
}

impl EngineMetrics {
    /// Metrics for a `width`×`height` Lynx view at the given device-pixel ratio
    /// (`rpx` resolves against `width`).
    #[must_use]
    pub fn new(width: f32, height: f32, device_pixel_ratio: f32) -> Self {
        Self {
            viewport_width: width,
            viewport_height: height,
            device_pixel_ratio,
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

    // `rpx` resolves off the Device viewport (`1rpx = viewport width / 750`,
    // like `vw`), so it needs no extra setup here.
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
