//! Capturing a laid-out document as an image (feature `render`).
//!
//! This is the half that needs the render stack: it asks `dom` to render through its
//! private paint pipeline, submits the retained scene to `pulsar`'s headless
//! wgpu surface, and reads the pixels back. Everything else in the crate works
//! on pixels that are already in hand.

use std::fmt;

use dom::Document;
use pulsar::gpu::{GpuError, Headless};
use pulsar::vello::Scene;
use pulsar::vello::peniko::Color;

use crate::image::{Image, ImageError};

/// Why a capture failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum CaptureError {
    /// The GPU could not render the scene.
    Gpu(GpuError),
    /// The readback did not describe a valid image.
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

/// Creates the mandatory headless renderer for a GPU-backed test.
///
/// A missing adapter is a test-environment failure, including in CI. Capture
/// tests must never report success without rendering and comparing pixels.
#[must_use]
#[track_caller]
pub fn headless(test: &str) -> Headless {
    Headless::new().unwrap_or_else(|error| panic!("{test}: GPU initialization failed: {error}"))
}

/// The device-pixel extent of a document's frame — the size a full-frame
/// capture must be.
///
/// `Device::viewport_size` is already in CSS pixels, and DOM's private painter
/// scales the whole scene up by the device pixel ratio (one CSS px becomes `ratio` device
/// px), so the painted frame spans `viewport * ratio` device pixels. Capturing
/// at anything smaller returns a top-left crop.
///
/// Playwright captures at `scale: 'css'` — one image pixel per CSS pixel —
/// which it can do because it downsamples. We have no resampler, so goldens
/// are device-pixel sized. They coincide with Playwright's only at a device
/// pixel ratio of 1, which is exactly what lynx-stack pins for determinism:
/// its Chromium project passes `--force-device-scale-factor=1`, and its
/// Firefox project overrides `deviceScaleFactor: 1`. Every viewport in this
/// repo uses 1.0 for the same reason.
#[must_use]
pub fn frame_size<T>(document: &Document<T>) -> (u32, u32) {
    let viewport = document.device().viewport_size();
    let ratio = document.device().device_pixel_ratio().get();
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

/// Lays out, paints, and reads back `document`'s whole frame.
///
/// Replaced-content resources must be registered through
/// [`Document::images_mut`] before this call. The private painter necessarily
/// consumes that same document-owned store, so capture cannot accidentally
/// paint with a second empty registry.
pub fn capture_document<T: Sync>(
    gpu: &mut Headless,
    document: &mut Document<T>,
    background: Color,
) -> Result<Image, CaptureError> {
    let (width, height) = frame_size(document);
    capture_document_sized(gpu, document, background, width, height)
}

/// [`capture_document`] at an explicit pixel size.
pub fn capture_document_sized<T: Sync>(
    gpu: &mut Headless,
    document: &mut Document<T>,
    background: Color,
    width: u32,
    height: u32,
) -> Result<Image, CaptureError> {
    document.render();
    capture_scene_sized(gpu, &document.scene(), background, width, height)
}

/// Captures a scene retained by a document's private painter.
///
/// The painter has already consumed its document-owned image registry while
/// building the scene, so this entry point deliberately accepts only the
/// finished scene. Runtime adapters use this after their own render boundary.
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
