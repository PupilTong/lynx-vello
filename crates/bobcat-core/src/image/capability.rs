//! What one injected decoder claims: which containers, at which provenance
//! tier.
//!
//! Backend selection is no longer this module's business — the embedder
//! constructs one [`Decoder`](crate::image::Decoder) (typically
//! `image-decoders::platform_decoder()`) and injects it. What remains here is
//! the vocabulary both sides of that boundary speak.

use crate::image::format::ImageFormat;

/// Codec **provenance**, not a promise about silicon.
///
/// No still-image API on any supported platform exposes an acceleration query,
/// and none of them reaches a dedicated decode ASIC: `ImageIO`'s JPEG path
/// imports `vImage` and no `IOKit` symbols, WIC's only GPU-adjacent surface hands
/// YCbCr planes to Direct2D, and `AImageDecoder` is a shim over the platform
/// Skia codecs writing into caller CPU memory. [`Self::DedicatedHardware`]
/// therefore exists as a reserved tier that no current decoder ever reports — it
/// is the honest answer to "is this hardware accelerated?", which is *no, and
/// here is the ladder we can actually observe*.
///
/// Ordered worst to best, so `max` picks the preferred provenance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Acceleration {
    /// A bundled pure-Rust codec (the Linux reference decoder).
    #[default]
    Software,
    /// The operating system's own vendor-tuned CPU codec.
    PlatformSoftware,
    /// Reserved for a codec running on dedicated decode silicon. No decoder
    /// reports this today; it exists so adding one is not a breaking change.
    DedicatedHardware,
}

impl Acceleration {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::PlatformSoftware => "platform-software",
            Self::DedicatedHardware => "dedicated-hardware",
        }
    }
}

/// Per-format tiers for one decoder, `None` where it cannot decode that format.
///
/// Consulted before every probe and decode: a sniffed container the injected
/// decoder does not claim is refused as
/// [`Unsupported`](crate::image::ImageError::Unsupported) rather than handed to a
/// decoder that would fail it less legibly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    tiers: [Option<Acceleration>; ImageFormat::ALL.len()],
}

impl Capabilities {
    /// Nothing supported.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            tiers: [None; ImageFormat::ALL.len()],
        }
    }

    #[must_use]
    pub const fn with(mut self, format: ImageFormat, tier: Acceleration) -> Self {
        self.tiers[format.index()] = Some(tier);
        self
    }

    /// The tier this decoder decodes `format` at, or `None` if it cannot.
    #[must_use]
    pub const fn tier(&self, format: ImageFormat) -> Option<Acceleration> {
        self.tiers[format.index()]
    }

    #[must_use]
    pub const fn supports(&self, format: ImageFormat) -> bool {
        self.tier(format).is_some()
    }

    /// The formats this decoder claims, in [`ImageFormat::ALL`]'s order.
    #[must_use]
    pub fn supported_formats(&self) -> Vec<ImageFormat> {
        ImageFormat::ALL
            .into_iter()
            .filter(|format| self.supports(*format))
            .collect()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{Acceleration, Capabilities};
    use crate::image::format::ImageFormat;

    #[test]
    fn none_supports_nothing_and_with_adds_one_format_at_a_time() {
        let none = Capabilities::none();
        for format in ImageFormat::ALL {
            assert_eq!(none.tier(format), None);
            assert!(!none.supports(format));
        }
        assert!(none.supported_formats().is_empty());

        let some = Capabilities::none()
            .with(ImageFormat::Jpeg, Acceleration::PlatformSoftware)
            .with(ImageFormat::Png, Acceleration::Software);
        assert_eq!(some.tier(ImageFormat::Png), Some(Acceleration::Software));
        assert_eq!(
            some.tier(ImageFormat::Jpeg),
            Some(Acceleration::PlatformSoftware)
        );
        assert_eq!(some.tier(ImageFormat::Heic), None);
        assert_eq!(
            some.supported_formats(),
            vec![ImageFormat::Png, ImageFormat::Jpeg],
            "reported in ImageFormat::ALL's order, not insertion order"
        );
    }

    #[test]
    fn acceleration_orders_worst_to_best() {
        assert!(Acceleration::Software < Acceleration::PlatformSoftware);
        assert!(Acceleration::PlatformSoftware < Acceleration::DedicatedHardware);
        assert_eq!(Acceleration::default(), Acceleration::Software);
    }
}
