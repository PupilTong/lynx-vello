//! Image sizing and decoded pixels: the size hint a frame reports for each
//! image, the one downsampling rule every host shares, the shape every
//! platform decoder answers in ([`Bitmap`]), when a resident bitmap is
//! worth decoding again at another size, and the residency policy built on
//! them ([`residency::Residency`]).

use vello_common::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat as PixelFormat};

pub mod residency;

/// How large a frame draws an image, in device pixels.
///
/// Per axis, the largest extent of one copy of the source across every draw
/// in the frame that names it — a tiled background counts one tile, a
/// `cover`-fitted replaced element the fitted rect — under the draw's own
/// transform, so a scaled or high-DPR draw asks for more pixels than its CSS
/// size says. It is the one fact a host needs to size a decode: a bitmap
/// larger than the draw is resampled down at composition and pays its memory
/// for nothing, while one smaller than the draw is upsampled and blurs. The
/// engine composes correctly against either, so the hint is advisory — a
/// host may decode at it, below it, or ignore it.
///
/// [`ImageSizeHint::UNBOUNDED`] names a read with no frame behind it, where
/// the only right answer is the image's own size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageSizeHint {
    pub width: u32,
    pub height: u32,
}

impl ImageSizeHint {
    /// No bound on either axis.
    pub const UNBOUNDED: Self = Self {
        width: u32::MAX,
        height: u32::MAX,
    };

    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Whether this hint bounds nothing.
    #[must_use]
    pub const fn is_unbounded(self) -> bool {
        self.width == u32::MAX && self.height == u32::MAX
    }

    /// The hint covering both this one and `other`: per-axis maximum, so a
    /// source drawn at two sizes in one frame is decoded for the larger.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            width: self.width.max(other.width),
            height: self.height.max(other.height),
        }
    }

    /// The size to decode a `width`x`height` image to under this hint: the
    /// largest size inside the hint that keeps the image's ratio, never
    /// larger than the image itself, never smaller than one pixel.
    ///
    /// This is the whole downsampling decision, kept beside the type so every
    /// host makes it the same way.
    #[must_use]
    pub fn fit(self, width: u32, height: u32) -> (u32, u32) {
        if self.is_unbounded() || (width <= self.width && height <= self.height) {
            return (width.max(1), height.max(1));
        }
        let scale = (f64::from(self.width) / f64::from(width.max(1)))
            .min(f64::from(self.height) / f64::from(height.max(1)));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the product is at most the image's own axis, a u32, and floored positive"
        )]
        let axis = |length: u32| ((f64::from(length) * scale).floor() as u32).max(1);
        (axis(width), axis(height))
    }
}

/// Decoded pixels: tightly packed RGBA8, at most the size that was asked
/// for, with the image's own size beside them.
#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// The image's intrinsic size — what layout is told — which the bitmap
    /// is a downsampled rendition of when the two differ.
    pub source_width: u32,
    pub source_height: u32,
    /// Whether the colour channels are premultiplied by alpha. `ImageIO`
    /// produces premultiplied pixels and gdk-pixbuf straight ones; the rasterizer
    /// composes either correctly as long as it is told which.
    pub premultiplied: bool,
    pub rgba: Vec<u8>,
}

impl Bitmap {
    /// The bytes the pixels occupy.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }

    /// The pixels as the image the compose path draws.
    #[must_use]
    pub fn into_image_data(self) -> ImageData {
        ImageData {
            data: Blob::from(self.rgba),
            format: PixelFormat::Rgba8,
            alpha_type: if self.premultiplied {
                ImageAlphaType::AlphaPremultiplied
            } else {
                ImageAlphaType::Alpha
            },
            width: self.width,
            height: self.height,
        }
    }
}

/// What a decoded bitmap is, without its pixels: everything a rasterizer
/// keys an upload of it on — the blob's identity, its size and its layout.
///
/// A descriptor names one bitmap for as long as its blob id is unique,
/// which is the process's life: a bitmap decoded again is a new blob and so
/// a new descriptor, even at the same size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageDescriptor {
    /// The pixels' [`Blob::id`].
    pub blob: u64,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub alpha_type: ImageAlphaType,
}

impl std::hash::Hash for ImageDescriptor {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // `peniko`'s two layout enums are fieldless and derive no `Hash`.
        (
            self.blob,
            self.width,
            self.height,
            self.format as u8,
            self.alpha_type as u8,
        )
            .hash(state);
    }
}

impl ImageDescriptor {
    /// What `image` is.
    #[must_use]
    pub fn of(image: &ImageData) -> Self {
        Self {
            blob: image.data.id(),
            width: image.width,
            height: image.height,
            format: image.format,
            alpha_type: image.alpha_type,
        }
    }

    /// The image this descriptor names, with `data` its pixels: the blob
    /// the descriptor was taken of, or one upgraded from a weak handle to
    /// it.
    #[must_use]
    pub fn with_pixels(self, data: Blob<u8>) -> ImageData {
        debug_assert_eq!(data.id(), self.blob, "the pixels this descriptor names");
        ImageData {
            data,
            format: self.format,
            alpha_type: self.alpha_type,
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum DecodeError {
    /// This platform has no decoder the host knows how to reach.
    #[error("no platform image decoder is available: {0}")]
    Unavailable(String),
    /// The platform decoder does not decode this container.
    #[error("the platform decoder does not support this image: {0}")]
    Unsupported(String),
    /// The bytes are not a decodable image.
    #[error("the image could not be decoded: {0}")]
    Malformed(String),
}

/// The size a `source_width`x`source_height` image is decoded to under
/// `max`: inside the bound, the image's own ratio kept, never upsampled.
#[must_use]
pub fn target_size(source_width: u32, source_height: u32, max: (u32, u32)) -> (u32, u32) {
    ImageSizeHint::new(max.0, max.1).fit(source_width, source_height)
}

/// Never ask a decoder for more than the rasterizer can draw:
/// `max_renderable` is the largest axis it accepts, which the caller knows
/// and this crate does not.
#[must_use]
pub fn bounded(target: (u32, u32), max_renderable: u32) -> (u32, u32) {
    ImageSizeHint::new(max_renderable, max_renderable).fit(target.0, target.1)
}

/// Whether a resident bitmap of `resident` should be replaced by one decoded
/// for `target`: it is at least `ratio` times too large on both axes, or it
/// is smaller than the draw while the image itself has more pixels to give.
#[must_use]
pub fn wants_refinement(
    resident: (u32, u32),
    target: (u32, u32),
    intrinsic: (u32, u32),
    ratio: f32,
) -> bool {
    #[expect(
        clippy::cast_precision_loss,
        reason = "pixel counts are compared approximately, by ratio"
    )]
    let too_large = resident.0 as f32 >= target.0 as f32 * ratio
        && resident.1 as f32 >= target.1 as f32 * ratio;
    let too_small = (resident.0 < target.0 || resident.1 < target.1)
        && (resident.0 < intrinsic.0 || resident.1 < intrinsic.1);
    too_large || too_small
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The downsampling decision every host shares: inside the hint, keep
    /// the ratio, never grow, never vanish.
    #[test]
    fn a_hint_fits_an_image_without_growing_it_or_breaking_its_ratio() {
        assert_eq!(
            ImageSizeHint::UNBOUNDED.fit(4000, 1000),
            (4000, 1000),
            "no bound decodes the image as it is"
        );
        assert_eq!(
            ImageSizeHint::new(8000, 8000).fit(4000, 1000),
            (4000, 1000),
            "a hint larger than the image never upsamples"
        );
        assert_eq!(
            ImageSizeHint::new(500, 500).fit(4000, 1000),
            (500, 125),
            "the binding axis lands on the hint and the other keeps the ratio"
        );
        assert_eq!(ImageSizeHint::new(500, 200).fit(1000, 4000), (50, 200));
        assert_eq!(
            ImageSizeHint::new(0, 0).fit(1000, 4000),
            (1, 1),
            "a degenerate draw still asks for a pixel, never a zero-sized bitmap"
        );
        assert_eq!(ImageSizeHint::new(300, 300).fit(0, 0), (1, 1));
    }

    #[test]
    fn a_union_of_hints_covers_the_larger_draw_on_each_axis() {
        let hint = ImageSizeHint::new(100, 900).union(ImageSizeHint::new(800, 50));
        assert_eq!(hint, ImageSizeHint::new(800, 900));
        assert!(!hint.is_unbounded());
        assert!(hint.union(ImageSizeHint::UNBOUNDED).is_unbounded());
    }

    #[test]
    fn a_bitmap_becomes_image_data_with_its_alpha_type() {
        let bitmap = Bitmap {
            width: 1,
            height: 1,
            source_width: 8,
            source_height: 8,
            premultiplied: true,
            rgba: vec![1, 2, 3, 4],
        };
        assert_eq!(bitmap.byte_len(), 4);
        let image = bitmap.into_image_data();
        assert_eq!(image.alpha_type, ImageAlphaType::AlphaPremultiplied);
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.data.as_ref(), &[1, 2, 3, 4]);
    }

    #[test]
    fn a_descriptor_names_an_image_and_rebuilds_it_from_its_pixels() {
        let image = Bitmap {
            width: 2,
            height: 1,
            source_width: 2,
            source_height: 1,
            premultiplied: false,
            rgba: vec![9; 8],
        }
        .into_image_data();
        let descriptor = ImageDescriptor::of(&image);
        assert_eq!(descriptor.blob, image.data.id());
        assert_eq!((descriptor.width, descriptor.height), (2, 1));
        let weak = image.data.downgrade();
        let rebuilt = descriptor.with_pixels(weak.upgrade().expect("still held"));
        assert_eq!(rebuilt, image);
        assert_eq!(ImageDescriptor::of(&rebuilt), descriptor);
    }

    #[test]
    fn the_target_size_fits_inside_the_bound_without_upsampling() {
        assert_eq!(target_size(4000, 3000, (200, 200)), (200, 150));
        assert_eq!(target_size(100, 50, (200, 200)), (100, 50));
        assert_eq!(target_size(0, 0, (200, 200)), (1, 1));
    }

    #[test]
    fn refinement_triggers_on_a_bitmap_far_too_large_or_too_small() {
        // A 2048-wide initial decode drawn at 200px: twice too large.
        assert!(wants_refinement(
            (2048, 1536),
            (200, 150),
            (4000, 3000),
            2.0
        ));
        // Drawn at 1100px: under the ratio, kept as is.
        assert!(!wants_refinement(
            (2048, 1536),
            (1100, 825),
            (4000, 3000),
            2.0
        ));
        // Drawn larger than the resident bitmap while the image has more.
        assert!(wants_refinement((200, 150), (400, 300), (4000, 3000), 2.0));
        // Drawn larger than the resident bitmap, which is already everything.
        assert!(!wants_refinement((200, 150), (400, 300), (200, 150), 2.0));
        // Exactly the drawn size.
        assert!(!wants_refinement((200, 150), (200, 150), (4000, 3000), 2.0));
    }
}
