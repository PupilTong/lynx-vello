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

/// The rendering environment a [`Document`](crate::Document) is created for.
///
/// The constructor exposes exactly the inputs that vary between views —
/// viewport CSS size and device-pixel ratio — and locks everything else to
/// the one profile every current embedder wants: `screen` media type,
/// standards (no-quirks) mode, light color scheme, a coarse primary pointer
/// with no hover (the `@media (hover)` / `(pointer)` answers a touch-device
/// app should see), and CSS-values-4 fallback font metrics. Widen this type
/// with new constructors when a second profile genuinely appears; the quirks
/// lock stays regardless.
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

/// Test-harness seam: full-parameter construction for suites that probe
/// media features or supply their own font metrics. Quirks mode is locked to
/// no-quirks — selector matching (`TDocument::quirks_mode`) and the `Stylist`
/// are already hard-wired to it inside this crate, so a quirks-mode device
/// would silently diverge the cascade from matching; the knob does not exist
/// above this function.
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

/// Font metrics for the CSS font-relative units the cascade resolves before
/// any text is shaped (`ex`, `ch`, `cap`, `ic`).
///
/// Stylo asks for these during cascade, long before parley has picked a face,
/// so this provider answers with the conventional ratios browsers fall back
/// to when a face reports no metrics of its own rather than loading a font.
/// Text itself is measured by parley through `hughie`, not by these numbers —
/// only font-relative *length units* read them.
#[derive(Debug)]
struct FallbackFontMetrics;

/// Fallback ratios, relative to the font size, for faces that report no
/// metrics. These match the CSS-values-4 defaults for `ex` (0.5em) and `ch`
/// (0.5em), plus the usual 0.8em ascent / 0.2em descent split.
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
