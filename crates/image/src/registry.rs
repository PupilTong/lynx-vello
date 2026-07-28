//! Runtime capability detection and backend selection.
//!
//! The probe runs once per process and answers two questions: which containers
//! can be decoded at all on this machine, and by whose codec. Both are genuinely
//! runtime questions — `ImageIO` gained WebP only in macOS 11 / iOS 14, WIC's WebP
//! codec is a Store extension rather than an inbox component, and
//! `AImageDecoder` exists only from Android API 30 — so none of them can be
//! settled by a `#[cfg]` at compile time.

use std::sync::{Arc, OnceLock};

use crate::backend::software::SoftwareDecoder;
use crate::decode::Decoder;
use crate::format::ImageFormat;

/// Codec **provenance**, not a promise about silicon.
///
/// No still-image API on any of the three platforms exposes an acceleration
/// query, and none of them reaches a dedicated decode ASIC: `ImageIO`'s JPEG path
/// imports `vImage` and no `IOKit` symbols, WIC's only GPU-adjacent surface hands
/// YCbCr planes to Direct2D, and `AImageDecoder` is a shim over the platform
/// Skia codecs writing into caller CPU memory. [`Self::DedicatedHardware`]
/// therefore exists as a reserved tier that no v1 backend ever reports — it is
/// the honest answer to "is this hardware accelerated?", which is *no, and here
/// is the ladder we can actually observe*.
///
/// Ordered worst to best, so `max` picks the preferred provenance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Acceleration {
    /// This crate's bundled pure-Rust codecs.
    #[default]
    Software,
    /// The operating system's own vendor-tuned CPU codec.
    PlatformSoftware,
    /// Reserved for a codec running on dedicated decode silicon. No backend
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

/// Per-format tiers for one backend, `None` where it cannot decode that format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    png: Option<Acceleration>,
    jpeg: Option<Acceleration>,
    webp: Option<Acceleration>,
}

impl Capabilities {
    /// Nothing supported.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            png: None,
            jpeg: None,
            webp: None,
        }
    }

    /// All three formats at [`Acceleration::Software`].
    #[must_use]
    pub const fn software() -> Self {
        Self {
            png: Some(Acceleration::Software),
            jpeg: Some(Acceleration::Software),
            webp: Some(Acceleration::Software),
        }
    }

    #[must_use]
    pub const fn with(mut self, format: ImageFormat, tier: Acceleration) -> Self {
        match format {
            ImageFormat::Png => self.png = Some(tier),
            ImageFormat::Jpeg => self.jpeg = Some(tier),
            ImageFormat::WebP => self.webp = Some(tier),
        }
        self
    }

    /// The tier this backend decodes `format` at, or `None` if it cannot.
    #[must_use]
    pub const fn tier(&self, format: ImageFormat) -> Option<Acceleration> {
        match format {
            ImageFormat::Png => self.png,
            ImageFormat::Jpeg => self.jpeg,
            ImageFormat::WebP => self.webp,
        }
    }

    #[must_use]
    pub const fn supports(&self, format: ImageFormat) -> bool {
        self.tier(format).is_some()
    }

    /// Best tier per format across two capability sets.
    #[must_use]
    pub fn merged(self, other: Self) -> Self {
        Self {
            png: merge(self.png, other.png),
            jpeg: merge(self.jpeg, other.jpeg),
            webp: merge(self.webp, other.webp),
        }
    }

    /// The formats this crate decodes on this machine, in a stable order.
    #[must_use]
    pub fn supported_formats(&self) -> Vec<ImageFormat> {
        [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP]
            .into_iter()
            .filter(|format| self.supports(*format))
            .collect()
    }
}

fn merge(left: Option<Acceleration>, right: Option<Acceleration>) -> Option<Acceleration> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (found @ Some(_), None) | (None, found) => found,
    }
}

/// The always-available software backend plus whichever platform backend the
/// runtime probe accepted.
///
/// Cheap to clone — the backends sit behind `Arc`.
#[derive(Clone, Debug)]
pub struct BackendRegistry {
    /// A plain `Vec` rather than a `SmallVec`: it holds at most two entries, is
    /// built once when the registry is constructed, and is never touched in a
    /// hot path — `decoder_for` reads it, and reading is what the per-format
    /// route cache below makes O(1) anyway.
    backends: Vec<Arc<dyn Decoder>>,
    routes: Routes,
}

/// The chosen backend index per format, resolved once at construction so
/// `decoder_for` is a array index rather than a scan plus a policy decision.
#[derive(Clone, Copy, Debug)]
struct Routes {
    png: usize,
    jpeg: usize,
    webp: usize,
}

impl BackendRegistry {
    /// Probes the platform and builds the registry. The probe itself is
    /// memoised process-wide; this call is cheap to repeat.
    #[must_use]
    pub fn detect() -> Self {
        let software: Arc<dyn Decoder> = Arc::new(SoftwareDecoder::new());
        let mut backends = vec![software];
        if let Some(platform) = crate::backend::platform_decoder() {
            backends.push(platform);
        }
        Self::from_backends(backends)
    }

    /// The software backend alone. Used by tests that must not depend on what
    /// the host machine happens to provide.
    #[must_use]
    pub fn software_only() -> Self {
        Self::from_backends(vec![Arc::new(SoftwareDecoder::new())])
    }

    fn from_backends(backends: Vec<Arc<dyn Decoder>>) -> Self {
        debug_assert!(
            !backends.is_empty(),
            "the software backend is unconditional, so a registry is never empty"
        );
        let routes = Routes {
            png: route(&backends, ImageFormat::Png),
            jpeg: route(&backends, ImageFormat::Jpeg),
            webp: route(&backends, ImageFormat::WebP),
        };
        Self { backends, routes }
    }

    /// The backend that will decode `format`.
    ///
    /// Never fails: the software backend claims all three formats
    /// unconditionally, so a platform backend is always an upgrade and never a
    /// dependency.
    #[must_use]
    pub fn decoder_for(&self, format: ImageFormat) -> &dyn Decoder {
        let index = match format {
            ImageFormat::Png => self.routes.png,
            ImageFormat::Jpeg => self.routes.jpeg,
            ImageFormat::WebP => self.routes.webp,
        };
        self.backends[index].as_ref()
    }

    /// Merged best tier per format across every registered backend.
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        self.backends
            .iter()
            .fold(Capabilities::none(), |merged, backend| {
                merged.merged(backend.capabilities())
            })
    }

    /// The tier the backend that will actually run reports for `format` — which
    /// is what a caller asking "is this accelerated here?" means, and is not the
    /// same as [`Self::capabilities`]'s merged best when routing deliberately
    /// declines a platform codec.
    #[must_use]
    pub fn effective_tier(&self, format: ImageFormat) -> Acceleration {
        self.decoder_for(format)
            .capabilities()
            .tier(format)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn backend_names(&self) -> Vec<&'static str> {
        self.backends.iter().map(|backend| backend.name()).collect()
    }
}

/// Routing policy, in one place so revisiting it is a one-function change.
///
/// Prefer the highest tier that claims the format, with one deliberate
/// exception: on Apple, PNG stays on the software backend. `ImageIO` delegates PNG
/// to its own bundled `libpng` and measured slower than the `png` crate this
/// workspace already links, so taking the platform codec there would be a
/// downgrade dressed up as an upgrade. The tier ladder describes provenance;
/// routing is allowed to disagree with it when provenance and speed diverge.
fn route(backends: &[Arc<dyn Decoder>], format: ImageFormat) -> usize {
    let prefer_software =
        cfg!(any(target_os = "macos", target_os = "ios")) && format == ImageFormat::Png;

    let mut best = 0;
    let mut best_tier = None;
    for (index, backend) in backends.iter().enumerate() {
        let Some(tier) = backend.capabilities().tier(format) else {
            continue;
        };
        if prefer_software && tier != Acceleration::Software {
            continue;
        }
        if best_tier.is_none_or(|current| tier > current) {
            best = index;
            best_tier = Some(tier);
        }
    }
    best
}

/// Process-wide memoised platform probe, shared by every registry.
pub(crate) fn probe_once<T: Copy>(cell: &'static OnceLock<T>, probe: impl FnOnce() -> T) -> T {
    *cell.get_or_init(probe)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{Acceleration, BackendRegistry, Capabilities};
    use crate::format::ImageFormat;

    const ALL: [ImageFormat; 3] = [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP];

    #[test]
    fn software_capabilities_claim_every_supported_format() {
        let capabilities = Capabilities::software();
        for format in ALL {
            assert_eq!(capabilities.tier(format), Some(Acceleration::Software));
            assert!(capabilities.supports(format));
        }
        assert_eq!(capabilities.supported_formats(), ALL.to_vec());
    }

    #[test]
    fn none_supports_nothing() {
        let capabilities = Capabilities::none();
        for format in ALL {
            assert_eq!(capabilities.tier(format), None);
        }
        assert!(capabilities.supported_formats().is_empty());
    }

    #[test]
    fn merging_takes_the_better_tier_per_format_and_the_union_of_support() {
        let platform = Capabilities::none()
            .with(ImageFormat::Jpeg, Acceleration::PlatformSoftware)
            .with(ImageFormat::WebP, Acceleration::PlatformSoftware);
        let merged = Capabilities::software().merged(platform);

        assert_eq!(merged.tier(ImageFormat::Png), Some(Acceleration::Software));
        assert_eq!(
            merged.tier(ImageFormat::Jpeg),
            Some(Acceleration::PlatformSoftware)
        );
        // Union, not intersection: a format only one backend claims survives.
        let only_platform = Capabilities::none().with(ImageFormat::WebP, Acceleration::Software);
        assert!(
            Capabilities::none()
                .merged(only_platform)
                .supports(ImageFormat::WebP)
        );
    }

    #[test]
    fn acceleration_orders_worst_to_best() {
        assert!(Acceleration::Software < Acceleration::PlatformSoftware);
        assert!(Acceleration::PlatformSoftware < Acceleration::DedicatedHardware);
        assert_eq!(Acceleration::default(), Acceleration::Software);
    }

    #[test]
    fn a_software_only_registry_routes_every_format_to_software() {
        let registry = BackendRegistry::software_only();
        for format in ALL {
            assert_eq!(registry.decoder_for(format).name(), "software");
            assert_eq!(registry.effective_tier(format), Acceleration::Software);
        }
        assert_eq!(registry.backend_names(), vec!["software"]);
    }

    #[test]
    fn detect_always_decodes_every_supported_format() {
        // Whatever the host provides, the software backend guarantees coverage
        // — a platform backend is an upgrade, never a dependency.
        let registry = BackendRegistry::detect();
        let capabilities = registry.capabilities();
        for format in ALL {
            assert!(capabilities.supports(format), "{format} must be decodable");
        }
        assert!(registry.backend_names().contains(&"software"));
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    fn apple_keeps_png_on_the_software_backend() {
        // Recorded routing policy: ImageIO delegates PNG to bundled libpng and
        // measured slower than the `png` crate already in this workspace.
        let registry = BackendRegistry::detect();
        assert_eq!(registry.decoder_for(ImageFormat::Png).name(), "software");
    }
}
