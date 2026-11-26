//! [`Image`]: what a draw samples — premultiplied pixels not uploaded yet,
//! a texture of its own, or a canvas's current texture.

use std::sync::Arc;

use super::CanvasError;
use crate::paint::display_list::{ImagePaint, ImageRef, Paint, Texture};
use crate::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat, ImageSampler};
use crate::render::canvas::{MAX_SURFACE_SIDE, SurfaceImage};

#[derive(Clone, Debug)]
pub struct Image(pub(crate) Source);

#[derive(Clone, Debug)]
pub(crate) enum Source {
    Pixels(ImageData),
    Texture(SurfaceImage),
    Live(Texture),
    Empty,
}

impl Image {
    pub fn from_premultiplied(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<Self, CanvasError> {
        Self::from_rgba8(width, height, pixels, ImageAlphaType::AlphaPremultiplied)
    }

    pub fn from_unpremultiplied(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<Self, CanvasError> {
        Self::from_rgba8(width, height, pixels, ImageAlphaType::Alpha)
    }

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

    pub fn read(&self) -> Result<Vec<u8>, CanvasError> {
        match &self.clone().stable().0 {
            Source::Pixels(data) => Ok(premultiplied(data)),
            Source::Texture(image) => image.read().map_err(|_| CanvasError::InvalidState),
            Source::Live(_) | Source::Empty => Ok(Vec::new()),
        }
    }

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
