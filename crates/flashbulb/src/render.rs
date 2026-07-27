//! Capturing a laid-out document as an image (feature `render`).
//!
//! This is the half that needs a renderer: it drives `dom`'s paint order
//! through `pulsar` onto `pulsar`'s headless wgpu surface and reads the pixels
//! back. Everything else in the crate works on pixels that are already in
//! hand.

use std::fmt;

use dom::Document;
use pulsar::gpu::{GpuError, Headless};
use pulsar::vello::peniko::Color;
use pulsar::{ImageStore, Painter};

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

/// A headless renderer, or a reason there is none.
///
/// Machines without a usable GPU adapter (CI containers, remote shells) cannot
/// run capture tests at all. Those tests then pass without having compared
/// anything, so this announces the skip on the process's real stderr.
///
/// The write deliberately bypasses `eprintln!`: libtest captures the print
/// macros per test and *discards* the capture for a test that passes, so a
/// skip notice printed that way would be invisible without `--nocapture` —
/// exactly the runs where it matters most. Writing to the inherited file
/// descriptor reaches the terminal and the CI log either way.
///
/// Set `FLASHBULB_REQUIRE_GPU=1` to turn a missing adapter into a failure
/// instead, for CI that is supposed to have one.
#[must_use]
pub fn headless_or_skip(test: &str) -> Option<Headless> {
    match Headless::new() {
        Ok(gpu) => Some(gpu),
        Err(GpuError::NoAdapter) => {
            assert!(
                std::env::var("FLASHBULB_REQUIRE_GPU").as_deref() != Ok("1"),
                "{test}: no usable GPU adapter, and FLASHBULB_REQUIRE_GPU=1"
            );
            let _ = std::io::Write::write_all(
                &mut std::io::stderr(),
                format!("SKIP {test}: no usable GPU adapter on this machine\n").as_bytes(),
            );
            None
        }
        Err(error) => panic!("{test}: GPU initialization failed: {error}"),
    }
}

/// The device-pixel extent of a document's frame — the size a full-frame
/// capture must be.
///
/// `Device::viewport_size` is already in CSS pixels, and `pulsar` scales the
/// whole scene up by the device pixel ratio (one CSS px becomes `ratio` device
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
    let frame = document.paint_order();
    let mut painter = Painter::new();
    let scene = painter.paint(document, &frame, &ImageStore::new());
    let pixels = gpu
        .render(scene, width, height, background)
        .map_err(CaptureError::Gpu)?;
    Image::from_rgba8(width, height, pixels).map_err(CaptureError::Image)
}
