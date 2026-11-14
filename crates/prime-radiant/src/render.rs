//! Capturing a laid-out document as an image (feature `render`).
//!
//! This is the half that needs the render stack: it asks `terminus` to render through its
//! private paint pipeline, submits the retained scene to `terminus`'s headless
//! wgpu surface, and reads the pixels back. Everything else in the crate works
//! on pixels that are already in hand.
//!
//! [`capture_document`] goes through the committed frame rather than through
//! `Document::scene`, for two reasons a screenshot cares about: the frame's
//! image draws resolve against the store the caller supplied (so a captured
//! page's bitmaps are the ones it asked for), and its `filter: blur()` groups
//! and `backdrop-filter` elements get their offscreen bake. `capture_scene`
//! keeps taking a scene somebody else composed and therefore neither.

use std::fmt;

use terminus::link::peniko::Color;
use terminus::link::render::gpu::{GpuError, Headless};
use terminus::link::{DisplayList, Document, Texture};

use crate::image::{Image, ImageError};

#[derive(Debug)]
#[non_exhaustive]
pub enum CaptureError {
    Gpu(GpuError),
    Image(ImageError),
    /// This many pictures the frame draws were answered without pixels and
    /// are on their way: the store did not restore them by the second
    /// describe. A capture never draws a frame without them.
    PicturesWanted(usize),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gpu(error) => write!(formatter, "{error}"),
            Self::Image(error) => write!(formatter, "{error}"),
            Self::PicturesWanted(count) => write!(
                formatter,
                "{count} picture(s) the frame draws are wanted and have not arrived"
            ),
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
    pixels: &impl terminus::link::FrameImages<PlatformFrame = std::convert::Infallible>,
) -> Result<Image, CaptureError> {
    let (width, height) = frame_size(document);
    capture_document_sized(gpu, document, background, pixels, width, height)
}

/// [`capture_document`] at an explicit pixel size.
pub fn capture_document_sized<T: Sync>(
    gpu: &mut Headless,
    document: &mut Document<T>,
    background: Color,
    pixels: &impl terminus::link::FrameImages<PlatformFrame = std::convert::Infallible>,
    width: u32,
    height: u32,
) -> Result<Image, CaptureError> {
    document.render();
    // One `Headless` captures many documents in these suites and commit ids
    // restart at one per document, so the bake cache's key cannot tell them
    // apart. A capture is of a document just rendered, so there is no reuse
    // worth keeping anyway.
    gpu.forget_filters();
    let frame = document
        .committed_frame()
        .expect("`Document::render` always leaves a committed frame retained");
    let (mut images, mut sources) = (Vec::new(), Vec::new());
    // A source the store answers without pixels is wanted, and a store that
    // restores inside `want`, as `TestImages` does, answers the second
    // describe with them. One that restores off this thread has no event
    // loop here to answer in, so the capture is refused rather than drawn
    // without the picture.
    if frame.resolve_images(pixels, &mut images, &mut sources) > 0 {
        let wanted = frame.resolve_images(pixels, &mut images, &mut sources);
        if wanted > 0 {
            return Err(CaptureError::PicturesWanted(wanted));
        }
    }
    // The vector textures first, since a filter bake may draw them; then,
    // with no offsets, no generation and no timeline reading, the filter
    // bakes: a capture composes the frame exactly as it was committed. They
    // take the capture's background, which is the canvas a backdrop at the
    // document's root filters.
    let vectors: Vec<Option<Texture>> = gpu
        .prepare_vectors(&frame)
        .map_err(CaptureError::Gpu)?
        .to_vec();
    let filtered: Vec<Option<Texture>> = gpu
        .prepare_filters(&frame, &images, &vectors, &|_| None, 0, None, background)
        .map_err(CaptureError::Gpu)?
        .to_vec();
    let mut composed = DisplayList::new();
    let scene = if let Some(scene) = frame.scene() {
        scene
    } else {
        frame.compose_target_into(
            &mut composed,
            (width, height),
            &images,
            &filtered,
            &vectors,
            &|_| None,
            None,
        );
        &composed
    };
    let pixels = gpu
        .render(scene, width, height, background)
        .map_err(CaptureError::Gpu)?;
    Image::from_rgba8(width, height, pixels).map_err(CaptureError::Image)
}

/// Captures a scene retained by a document's painter.
pub fn capture_scene<T>(
    gpu: &mut Headless,
    document: &Document<T>,
    scene: &DisplayList,
    background: Color,
) -> Result<Image, CaptureError> {
    let (width, height) = frame_size(document);
    capture_scene_sized(gpu, scene, background, width, height)
}

/// [`capture_scene`] at an explicit pixel size.
pub fn capture_scene_sized(
    gpu: &mut Headless,
    scene: &DisplayList,
    background: Color,
    width: u32,
    height: u32,
) -> Result<Image, CaptureError> {
    let pixels = gpu
        .render(scene, width, height, background)
        .map_err(CaptureError::Gpu)?;
    Image::from_rgba8(width, height, pixels).map_err(CaptureError::Image)
}
