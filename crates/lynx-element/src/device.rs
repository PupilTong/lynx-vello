//! View metrics and stylo [`Device`] construction.
//!
//! `docs/style-architecture.md`'s ownership table assigns "view metrics and
//! device construction" to the runtime adapter, so the `Device` a
//! [`crate::ElementTree`] hands to `dom` is built here rather than by
//! embedders or by the DOM core.

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

/// The viewport a Lynx view renders into.
///
/// Sizes are CSS pixels; `device_pixel_ratio` scales them to physical pixels.
/// Lynx's `rpx`/`ppx` view units are not derived from this yet (recorded limit
/// in the crate docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    pub device_pixel_ratio: f32,
}

impl Viewport {
    /// A viewport of `width` × `height` CSS pixels at a 1.0 device-pixel ratio.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio: 1.0,
        }
    }

    #[must_use]
    pub const fn with_device_pixel_ratio(mut self, device_pixel_ratio: f32) -> Self {
        self.device_pixel_ratio = device_pixel_ratio;
        self
    }

    /// Builds the stylo device this viewport describes.
    ///
    /// Lynx targets touch devices, so the pointer media features report a
    /// coarse primary pointer with no hover — the `@media (hover)` /
    /// `(pointer)` answers a Lynx app should see.
    #[must_use]
    pub fn device(self) -> Device {
        Device::new(
            MediaType::screen(),
            QuirksMode::NoQuirks,
            Size2D::<f32, CSSPixel>::new(self.width, self.height),
            Size2D::<f32, DevicePixel>::new(
                self.width * self.device_pixel_ratio,
                self.height * self.device_pixel_ratio,
            ),
            Scale::<f32, CSSPixel, DevicePixel>::new(self.device_pixel_ratio),
            Box::new(LynxFontMetricsProvider),
            ComputedValues::initial_values_with_font_override(Font::initial_values()),
            PrefersColorScheme::Light,
            PointerCapabilities::COARSE,
            PointerCapabilities::COARSE,
        )
    }
}

/// Font metrics for the CSS font-relative units the cascade resolves before
/// any text is shaped (`ex`, `ch`, `cap`, `ic`).
///
/// Stylo asks for these during cascade, long before parley has picked a face,
/// so this provider answers with the conventional ratios browsers fall back to
/// when a face reports no metrics of its own rather than loading a font. Text
/// itself is measured by parley through `hughie`, not by these numbers — only
/// font-relative *length units* read them.
#[derive(Debug)]
pub struct LynxFontMetricsProvider;

/// Fallback ratios, relative to the font size, for faces that report no
/// metrics. These match the CSS-values-4 defaults for `ex` (0.5em) and `ch`
/// (0.5em), plus the usual 0.8em ascent / 0.2em descent split.
const X_HEIGHT_RATIO: f32 = 0.5;
const ZERO_ADVANCE_RATIO: f32 = 0.5;
const CAP_HEIGHT_RATIO: f32 = 0.7;
const ASCENT_RATIO: f32 = 0.8;

impl FontMetricsProvider for LynxFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        base_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        let em = base_size.px();
        FontMetrics {
            x_height: Some(Length::new(em * X_HEIGHT_RATIO)),
            zero_advance_measure: Some(Length::new(em * ZERO_ADVANCE_RATIO)),
            cap_height: Some(Length::new(em * CAP_HEIGHT_RATIO)),
            ic_width: Some(Length::new(em)),
            ascent: Length::new(em * ASCENT_RATIO),
            script_percent_scale_down: None,
            script_script_percent_scale_down: None,
        }
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        Length::new(FONT_MEDIUM_PX)
    }
}
