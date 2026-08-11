//! View metrics: the [`Viewport`] a Lynx view renders into.
//!
//! The stylo-facing device profile (touch pointers, light scheme, standards
//! mode, fallback font metrics) lives in `dom::Device`; this layer only
//! carries the metrics that vary per view and hands them across.

use dom::Device;

/// A viewport measured in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Viewport width in CSS pixels.
    pub width: f32,
    /// Viewport height in CSS pixels.
    pub height: f32,
    /// Physical pixels per CSS pixel.
    pub device_pixel_ratio: f32,
}

impl Viewport {
    /// Creates a viewport with a device-pixel ratio of 1.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio: 1.0,
        }
    }

    #[must_use]
    /// Returns this viewport with a new device-pixel ratio.
    pub const fn with_device_pixel_ratio(mut self, device_pixel_ratio: f32) -> Self {
        self.device_pixel_ratio = device_pixel_ratio;
        self
    }

    #[must_use]
    pub(crate) fn device(self) -> Device {
        Device::new(self.width, self.height, self.device_pixel_ratio)
    }
}
