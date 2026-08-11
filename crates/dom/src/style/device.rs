//! The document's rendering environment: the embedder-facing [`Device`]
//! profile and its stylo construction.

use euclid::{Scale, Size2D};
use stylo::context::QuirksMode;
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

/// A document's viewport and device-pixel rendering environment.
#[derive(Debug)]
pub struct Device {
    stylo: stylo::device::Device,
}

impl Device {
    /// A `width` × `height` CSS-pixel viewport at `device_pixel_ratio`.
    #[must_use]
    pub fn new(width: f32, height: f32, device_pixel_ratio: f32) -> Self {
        standards_device(
            MediaType::screen(),
            Size2D::<f32, CSSPixel>::new(width, height),
            Size2D::<f32, DevicePixel>::new(
                width * device_pixel_ratio,
                height * device_pixel_ratio,
            ),
            Scale::<f32, CSSPixel, DevicePixel>::new(device_pixel_ratio),
            Box::new(FallbackFontMetrics),
            ComputedValues::initial_values_with_font_override(Font::initial_values()),
            PrefersColorScheme::Light,
            PointerCapabilities::COARSE,
            PointerCapabilities::COARSE,
        )
    }

    pub(crate) fn into_stylo(self) -> stylo::device::Device {
        self.stylo
    }
}

#[doc(hidden)]
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors stylo's Device::new minus the locked quirks knob"
)]
pub fn standards_device(
    media_type: MediaType,
    viewport_size: Size2D<f32, CSSPixel>,
    device_size: Size2D<f32, DevicePixel>,
    device_pixel_ratio: Scale<f32, CSSPixel, DevicePixel>,
    font_metrics_provider: Box<dyn FontMetricsProvider>,
    default_values: stylo::servo_arc::Arc<ComputedValues>,
    prefers_color_scheme: PrefersColorScheme,
    primary_pointer_capabilities: PointerCapabilities,
    all_pointer_capabilities: PointerCapabilities,
) -> Device {
    Device {
        stylo: stylo::device::Device::new(
            media_type,
            QuirksMode::NoQuirks,
            viewport_size,
            device_size,
            device_pixel_ratio,
            font_metrics_provider,
            default_values,
            prefers_color_scheme,
            primary_pointer_capabilities,
            all_pointer_capabilities,
        ),
    }
}

/// CSS fallback font metrics used before text shaping.
#[derive(Debug)]
struct FallbackFontMetrics;

const X_HEIGHT_RATIO: f32 = 0.5;
const ZERO_ADVANCE_RATIO: f32 = 0.5;
const CAP_HEIGHT_RATIO: f32 = 0.7;
const ASCENT_RATIO: f32 = 0.8;

impl FontMetricsProvider for FallbackFontMetrics {
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
