//! [`Image`]: what a draw samples — premultiplied pixels not uploaded yet,
//! a texture of its own, or a canvas's current texture.

use std::sync::Arc;

use super::CanvasError;
use crate::paint::display_list::{ImagePaint, ImageRef, Paint, Texture};
use crate::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat, ImageSampler};
use crate::render::canvas::{MAX_SURFACE_SIDE, SurfaceImage};

/// An image a draw samples: what `drawImage` and a pattern draw, what
/// `transferToImageBitmap` hands out, and what an `ImageBitmap` holds.
/// Cloning shares it. `Send` and `Sync` natively.
///
/// Either side is at most [`MAX_SURFACE_SIDE`] pixels, the longest texture
/// the device makes.
#[derive(Clone, Debug)]
pub struct Image(pub(crate) Source);

#[derive(Clone, Debug)]
pub(crate) enum Source {
    /// Pixels, uploaded at the replay of the first draw that samples
    /// them.
    Pixels(ImageData),
    /// A texture of its own, which lives as long as a clone does.
    Texture(SurfaceImage),
    /// A canvas's texture as it is when the list drawing it is replayed
    /// ([`OffscreenCanvas::image`](crate::html::canvas::OffscreenCanvas::image)): drawn at once,
    /// never kept.
    Live(Texture),
    /// No pixels: a canvas with no bitmap.
    Empty,
}

impl Image {
    /// An image of `pixels`, already premultiplied: every color channel at
    /// most its alpha.
    ///
    /// `InvalidState` when `pixels` is not `width * height * 4` bytes or a
    /// side exceeds [`MAX_SURFACE_SIDE`].
    pub fn from_premultiplied(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<Self, CanvasError> {
        Self::from_rgba8(width, height, pixels, ImageAlphaType::AlphaPremultiplied)
    }

    /// An image of straight-alpha `pixels`, premultiplied when uploaded.
    ///
    /// `InvalidState` when `pixels` is not `width * height * 4` bytes or a
    /// side exceeds [`MAX_SURFACE_SIDE`].
    pub fn from_unpremultiplied(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<Self, CanvasError> {
        Self::from_rgba8(width, height, pixels, ImageAlphaType::Alpha)
    }

    /// An image of `data`'s pixels, shared.
    ///
    /// `InvalidState` for a size `from_premultiplied` refuses or a pixel
    /// format other than RGBA8 and BGRA8.
    pub fn from_image_data(data: &ImageData) -> Result<Self, CanvasError> {
        if !matches!(data.format, ImageFormat::Rgba8 | ImageFormat::Bgra8)
            || !fits(data.width, data.height)
            || data.data.data().len() != data.width as usize * data.height as usize * 4
        {
            return Err(CanvasError::InvalidState);
        }
        Ok(Self(Source::Pixels(data.clone())))
    }

    fn from_rgba8(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        alpha_type: ImageAlphaType,
    ) -> Result<Self, CanvasError> {
        if !fits(width, height) || pixels.len() != width as usize * height as usize * 4 {
            return Err(CanvasError::InvalidState);
        }
        Ok(Self(Source::Pixels(ImageData {
            data: Blob::new(Arc::new(pixels)),
            format: ImageFormat::Rgba8,
            alpha_type,
            width,
            height,
        })))
    }

    pub(crate) fn empty() -> Self {
        Self(Source::Empty)
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        match &self.0 {
            Source::Pixels(data) => data.width,
            Source::Texture(image) => image.width(),
            Source::Live(texture) => texture.size().0,
            Source::Empty => 0,
        }
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        match &self.0 {
            Source::Pixels(data) => data.height,
            Source::Texture(image) => image.height(),
            Source::Live(texture) => texture.size().1,
            Source::Empty => 0,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.width() == 0 || self.height() == 0
    }

    /// What `createImageBitmap` makes of this image: the image itself, or
    /// with `crop` the `width` × `height` rectangle at `(x, y)` in it, the
    /// part outside it transparent black. A canvas's current texture is
    /// copied, as of every draw issued so far.
    ///
    /// # Errors
    ///
    /// For any image, before anything is allocated: `Range` for a crop
    /// with a zero width or height (HTML `createImageBitmap`: an `sw` or
    /// `sh` of 0 is a `RangeError`), then `InvalidState` for a crop with a
    /// side over [`MAX_SURFACE_SIDE`], as for an image made from pixels
    /// that do not fit.
    pub fn crop(&self, crop: Option<(i64, i64, u32, u32)>) -> Result<Self, CanvasError> {
        if let Some((_, _, width, height)) = crop {
            if width == 0 || height == 0 {
                return Err(CanvasError::Range);
            }
            if !fits(width, height) {
                return Err(CanvasError::InvalidState);
            }
        }
        let (x, y, width, height) = match (crop, &self.0) {
            (None, Source::Live(_)) => (0, 0, self.width(), self.height()),
            (None, _) => return Ok(self.clone()),
            (Some(crop), _) => crop,
        };
        Ok(Self(match &self.0 {
            Source::Pixels(data) => return crop_pixels(data, x, y, width, height),
            Source::Texture(image) => Source::Texture(image.crop(x, y, width, height)),
            Source::Live(texture) => {
                Source::Texture(SurfaceImage::copy(*texture, x, y, width, height))
            }
            Source::Empty => return Ok(Self::empty()),
        }))
    }

    /// The image as one that stays as it is: a canvas's current texture is
    /// copied.
    pub(crate) fn stable(self) -> Self {
        match self.0 {
            Source::Live(texture) => {
                let (width, height) = texture.size();
                Self(Source::Texture(SurfaceImage::copy(
                    texture, 0, 0, width, height,
                )))
            }
            source => Self(source),
        }
    }

    /// Reads the pixels, premultiplied RGBA8, row-major, tightly packed,
    /// waiting for a texture's. `InvalidState` when the readback failed.
    pub fn read(&self) -> Result<Vec<u8>, CanvasError> {
        match &self.clone().stable().0 {
            Source::Pixels(data) => Ok(premultiplied(data)),
            Source::Texture(image) => image.read().map_err(|_| CanvasError::InvalidState),
            Source::Live(_) | Source::Empty => Ok(Vec::new()),
        }
    }

    /// The paint drawing this image through `sampler`, the texture a draw of
    /// it keeps alive until it is replayed, and the canvas texture it reads
    /// as it is at replay.
    pub(crate) fn paint(
        &self,
        sampler: ImageSampler,
    ) -> (Paint, Option<SurfaceImage>, Option<Texture>) {
        match &self.0 {
            Source::Pixels(data) => {
                let image = ImageRef::held(data);
                (
                    Paint::Image(Arc::new(ImagePaint { image, sampler })),
                    None,
                    None,
                )
            }
            Source::Texture(image) => (
                Paint::texture(image.texture(), sampler),
                Some(image.clone()),
                None,
            ),
            Source::Live(texture) => (Paint::texture(*texture, sampler), None, Some(*texture)),
            Source::Empty => (Paint::Solid(crate::peniko::Color::TRANSPARENT), None, None),
        }
    }
}

fn fits(width: u32, height: u32) -> bool {
    width <= MAX_SURFACE_SIDE && height <= MAX_SURFACE_SIDE
}

/// `data`'s pixels as premultiplied RGBA8.
fn premultiplied(data: &ImageData) -> Vec<u8> {
    let mut pixels = data.data.data().to_vec();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        if data.format == ImageFormat::Bgra8 {
            pixel.swap(0, 2);
        }
        if data.alpha_type == ImageAlphaType::Alpha {
            let a = u32::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = u8::try_from((u32::from(*channel) * a + 127) / 255).unwrap_or(255);
            }
        }
    }
    pixels
}

/// The `width` × `height` rectangle at `(x, y)` in `data`, premultiplied,
/// transparent black outside it; both sides are at most
/// [`MAX_SURFACE_SIDE`].
fn crop_pixels(
    data: &ImageData,
    x: i64,
    y: i64,
    width: u32,
    height: u32,
) -> Result<Image, CanvasError> {
    let (w, h) = (i64::from(width), i64::from(height));
    let len = usize::try_from(w * h * 4).map_err(|_| CanvasError::Range)?;
    let mut pixels = vec![0; len];
    let source = premultiplied(data);
    let (iw, ih) = (i64::from(data.width), i64::from(data.height));
    // Rows and columns are copied only where the crop overlaps the image,
    // so `x` and `y` are then within one crop side of it.
    let (cx0, cx1) = (x.max(0), x.saturating_add(w).min(iw));
    if cx0 < cx1 {
        let span = usize::try_from((cx1 - cx0) * 4).map_err(|_| CanvasError::Range)?;
        for row in y.max(0)..y.saturating_add(h).min(ih) {
            let from = usize::try_from((row * iw + cx0) * 4).map_err(|_| CanvasError::Range)?;
            let to =
                usize::try_from(((row - y) * w + cx0 - x) * 4).map_err(|_| CanvasError::Range)?;
            pixels[to..to + span].copy_from_slice(&source[from..from + span]);
        }
    }
    Image::from_premultiplied(width, height, pixels)
}
