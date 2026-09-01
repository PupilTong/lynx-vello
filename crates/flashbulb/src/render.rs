//! Capturing a laid-out document as an image (feature `render`).
//!
//! This is the half that needs the render stack: it asks `dom` to render through its
//! private paint pipeline, submits the retained scene to `dom`'s headless
//! wgpu surface, and reads the pixels back. Everything else in the crate works
//! on pixels that are already in hand.

use std::fmt;

use dom::Document;
use dom::render::gpu::{GpuError, Headless};
use dom::vello::Scene;
use dom::vello::peniko::Color;

use crate::image::{Image, ImageError};

#[derive(Debug)]
#[non_exhaustive]
pub enum CaptureError {
    Gpu(GpuError),
    Image(ImageError),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gpu(error) => write!(formatter, "{error}"),
            Self::Image(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Creates the required headless renderer for a GPU-backed test.
#[must_use]
#[track_caller]
pub fn headless(test: &str) -> Headless {
    Headless::new().unwrap_or_else(|error| panic!("{test}: GPU initialization failed: {error}"))
}

/// Returns the document frame's device-pixel dimensions.
#[must_use]
pub fn frame_size<T>(document: &Document<T>) -> (u32, u32) {
    let viewport = document.viewport_size();
    let ratio = document.device_pixel_ratio();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a viewport is a small, non-negative pixel count"
    )]
    {
        (
            (viewport.width * ratio).round() as u32,
            (viewport.height * ratio).round() as u32,
        )
    }
}

/// Lays out, paints, and reads back the document's whole frame.
pub fn capture_document<T: Sync>(
    gpu: &mut Headless,
    document: &mut Document<T>,
    background: Color,
    pixels: &dyn dom::FrameImages,
) -> Result<Image, CaptureError> {
    let (width, height) = frame_size(document);
    capture_document_sized(gpu, document, background, pixels, width, height)
}

/// [`capture_document`] at an explicit pixel size.
pub fn capture_document_sized<T: Sync>(
    gpu: &mut Headless,
    document: &mut Document<T>,
    background: Color,
    pixels: &dyn dom::FrameImages,
    width: u32,
    height: u32,
) -> Result<Image, CaptureError> {
    document.render();
    capture_scene_sized(gpu, &document.scene(pixels), background, width, height)
}

/// Captures a scene retained by a document's painter.
pub fn capture_scene<T>(
    gpu: &mut Headless,
    document: &Document<T>,
    scene: &Scene,
    background: Color,
) -> Result<Image, CaptureError> {
    let (width, height) = frame_size(document);
    capture_scene_sized(gpu, scene, background, width, height)
}

/// [`capture_scene`] at an explicit pixel size.
pub fn capture_scene_sized(
    gpu: &mut Headless,
    scene: &Scene,
    background: Color,
    width: u32,
    height: u32,
) -> Result<Image, CaptureError> {
    let pixels = gpu
        .render(scene, width, height, background)
        .map_err(CaptureError::Gpu)?;
    Image::from_rgba8(width, height, pixels).map_err(CaptureError::Image)
}
