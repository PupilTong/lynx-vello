//! View metrics: the [`Viewport`] a Lynx view renders into.
//!
//! The stylo-facing device profile (touch pointers, light scheme, standards
//! mode, fallback font metrics) lives in `dom::Device`; this layer only
//! carries the metrics that vary per view and hands them across.

use dom::Device;

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

    /// Builds the device profile this viewport describes.
    #[must_use]
    pub(crate) fn device(self) -> Device {
        Device::new(self.width, self.height, self.device_pixel_ratio)
    }
}
