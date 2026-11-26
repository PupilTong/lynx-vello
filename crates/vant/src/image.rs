//! Image sizing and decoded pixels: the size hint a frame reports for each
//! image, the one downsampling rule every host shares, the shapes a
//! platform decoder answers in ([`Bitmap`] for CPU pixels,
//! [`PlatformBitmap`] for a frame of the platform's own type, either as a
//! [`DecodedImage`]), when a resident bitmap is worth decoding again at
//! another size, the residency policy built on them
//! ([`residency::Residency`]), and the playback policy of an animated image
//! ([`animation`]).
//!
//! # Platform frames
//!
//! A decoder may answer a frame that is not CPU pixels: the browser's
//! `createImageBitmap` answers an `ImageBitmap`, whose pixels live where the
//! browser keeps them and are copied straight into a texture. This crate
//! never looks inside one. It is a value of the decoder's own type
//! ([`ImageDecoder::Frame`](crate::ImageDecoder::Frame)), carried as
//! [`PictureFrame::Platform`] beside the [`ImageDescriptor`] minted for it
//! ([`ImageDescriptor::platform`]), and its bytes are counted from its size.
//! A decoder that answers CPU pixels only names an uninhabited frame type
//! ([`std::convert::Infallible`]), and every generic here defaults to it.

use std::convert::Infallible;

use vello_common::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat as PixelFormat};

pub mod animation;
pub mod residency;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageSizeHint {
    pub width: u32,
    pub height: u32,
}

impl ImageSizeHint {
    pub const UNBOUNDED: Self = Self {
        width: u32::MAX,
        height: u32::MAX,
    };

    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub const fn is_unbounded(self) -> bool {
        self.width == u32::MAX && self.height == u32::MAX
    }

    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            width: self.width.max(other.width),
            height: self.height.max(other.height),
        }
    }

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

#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub premultiplied: bool,
    pub rgba: Vec<u8>,
}

impl Bitmap {
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }

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

#[derive(Clone, Debug)]
pub struct PlatformBitmap<F> {
    pub frame: F,
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
}

#[derive(Clone, Debug)]
pub enum DecodedImage<F = Infallible> {
    Pixels(Bitmap),
    Platform(PlatformBitmap<F>),
}

impl<F> From<Bitmap> for DecodedImage<F> {
    fn from(bitmap: Bitmap) -> Self {
        Self::Pixels(bitmap)
    }
}

impl<F> DecodedImage<F> {
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        match self {
            Self::Pixels(bitmap) => (bitmap.width, bitmap.height),
            Self::Platform(bitmap) => (bitmap.width, bitmap.height),
        }
    }

    #[must_use]
    pub fn source_size(&self) -> (u32, u32) {
        match self {
            Self::Pixels(bitmap) => (bitmap.source_width, bitmap.source_height),
            Self::Platform(bitmap) => (bitmap.source_width, bitmap.source_height),
        }
    }

    #[must_use]
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Pixels(bitmap) => bitmap.byte_len(),
            Self::Platform(bitmap) => frame_bytes(bitmap.width, bitmap.height),
        }
    }

    #[must_use]
    pub fn into_frame(self) -> (PictureFrame<F>, ImageDescriptor) {
        match self {
            Self::Pixels(bitmap) => {
                let image = bitmap.into_image_data();
                let descriptor = ImageDescriptor::of(&image);
                (PictureFrame::Pixels(image), descriptor)
            }
            Self::Platform(bitmap) => (
                PictureFrame::Platform(bitmap.frame),
                ImageDescriptor::platform(bitmap.width, bitmap.height),
            ),
        }
    }
}

#[must_use]
pub fn frame_bytes(width: u32, height: u32) -> usize {
    (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4)
}

#[derive(Clone, Debug, PartialEq)]
pub enum PictureFrame<P = Infallible> {
    Pixels(ImageData),
    Platform(P),
}

impl<P> PictureFrame<P> {
    #[must_use]
    pub const fn pixels(&self) -> Option<&ImageData> {
        match self {
            Self::Pixels(pixels) => Some(pixels),
            Self::Platform(_) => None,
        }
    }
}

impl PictureFrame {
    #[must_use]
    pub fn into_pixels(self) -> ImageData {
        match self {
            Self::Pixels(pixels) => pixels,
            Self::Platform(never) => match never {},
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageDescriptor {
    pub blob: u64,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub alpha_type: ImageAlphaType,
}

impl std::hash::Hash for ImageDescriptor {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
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

    #[must_use]
    pub fn platform(width: u32, height: u32) -> Self {
        Self {
            blob: Blob::<u8>::from(Vec::new()).id(),
            width,
            height,
            format: PixelFormat::Rgba8,
            alpha_type: ImageAlphaType::AlphaPremultiplied,
        }
    }

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
    #[error("no platform image decoder is available: {0}")]
    Unavailable(String),
    #[error("the platform decoder does not support this image: {0}")]
    Unsupported(String),
    #[error("the image could not be decoded: {0}")]
    Malformed(String),
}

#[must_use]
pub fn target_size(source_width: u32, source_height: u32, max: (u32, u32)) -> (u32, u32) {
    ImageSizeHint::new(max.0, max.1).fit(source_width, source_height)
}

#[must_use]
pub fn bounded(target: (u32, u32), max_renderable: u32) -> (u32, u32) {
    ImageSizeHint::new(max_renderable, max_renderable).fit(target.0, target.1)
}

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
    fn a_platform_frame_gets_a_descriptor_of_its_own_and_counts_its_size() {
        let decoded: DecodedImage<u8> = DecodedImage::Platform(PlatformBitmap {
            frame: 7,
            width: 3,
            height: 2,
            source_width: 30,
            source_height: 20,
        });
        assert_eq!(decoded.size(), (3, 2));
        assert_eq!(decoded.source_size(), (30, 20));
        assert_eq!(decoded.byte_len(), 3 * 2 * 4);
        let (frame, descriptor) = decoded.clone().into_frame();
        assert_eq!(frame, PictureFrame::Platform(7));
        assert_eq!(frame.pixels(), None);
        assert_eq!(
            descriptor.alpha_type,
            ImageAlphaType::AlphaPremultiplied,
            "what the frame's writer produces"
        );
        let (_, other) = decoded.into_frame();
        assert_ne!(
            descriptor.blob, other.blob,
            "each frame is a bitmap of its own"
        );
        let pixels = Bitmap {
            width: 1,
            height: 1,
            source_width: 1,
            source_height: 1,
            premultiplied: false,
            rgba: vec![0; 4],
        }
        .into_image_data();
        assert_ne!(descriptor.blob, pixels.data.id());
    }

    #[test]
    fn the_target_size_fits_inside_the_bound_without_upsampling() {
        assert_eq!(target_size(4000, 3000, (200, 200)), (200, 150));
        assert_eq!(target_size(100, 50, (200, 200)), (100, 50));
        assert_eq!(target_size(0, 0, (200, 200)), (1, 1));
    }

    #[test]
    fn refinement_triggers_on_a_bitmap_far_too_large_or_too_small() {
        assert!(wants_refinement(
            (2048, 1536),
            (200, 150),
            (4000, 3000),
            2.0
        ));
        assert!(!wants_refinement(
            (2048, 1536),
            (1100, 825),
            (4000, 3000),
            2.0
        ));
        assert!(wants_refinement((200, 150), (400, 300), (4000, 3000), 2.0));
        assert!(!wants_refinement((200, 150), (400, 300), (200, 150), 2.0));
        assert!(!wants_refinement((200, 150), (200, 150), (4000, 3000), 2.0));
    }
}
