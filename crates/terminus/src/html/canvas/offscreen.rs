//! [`OffscreenCanvas`]: HTML's `OffscreenCanvas` ("The `OffscreenCanvas`
//! interface") with a 2D context: its size, its bitmap, the commit to its
//! placeholder, and `transferToImageBitmap()`. The context itself is
//! [`Context2d`], borrowed through [`OffscreenCanvas::context_2d`].

use super::context2d::{Context2d, Context2dData, Raster, Source};
use super::{CanvasFrames, Committed, Image};

/// HTML's `OffscreenCanvas` with its `OffscreenCanvasRenderingContext2D`.
///
/// The bitmap is premultiplied RGBA8 in a GPU texture, initially
/// transparent black. A size of zero or a side over 8,192 px (the longest
/// texture the device makes, `MAX_SURFACE_SIDE`)
/// has no bitmap: draws do nothing and reads answer transparent black.
/// The 2D context's attributes and methods are on the [`Context2d`]
/// [`Self::context_2d`] borrows.
#[derive(Debug)]
pub struct OffscreenCanvas {
    width: u32,
    height: u32,
    raster: Option<Raster>,
    /// The bitmap may have changed since the last commit.
    changed: bool,
    /// The 2D context's drawing state, state stack and default path.
    context: Context2dData,
}

impl OffscreenCanvas {
    /// A `width` × `height` canvas, transparent black, its context in the
    /// default state.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            raster: Raster::new(width, height),
            changed: false,
            context: Context2dData::new(),
        }
    }

    /// The canvas's 2D context, drawing on this canvas's bitmap.
    pub fn context_2d(&mut self) -> Context2d<'_> {
        Context2d::new(
            &mut self.context,
            self.width,
            self.height,
            &mut self.raster,
            &mut self.changed,
        )
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Setting `width` or `height`: the bitmap takes the new size and, like
    /// `reset`, becomes transparent black with the context in its default
    /// state, even when the size is unchanged.
    pub fn set_size(&mut self, width: u32, height: u32) {
        if (width, height) == (self.width, self.height) {
            self.context_2d().reset();
            return;
        }
        self.width = width;
        self.height = height;
        self.raster = Raster::new(width, height);
        self.context_2d().reset_state();
        self.changed = true;
    }

    // The bitmap.

    /// The bitmap as an image, for `drawImage(canvas)`, `createPattern` and
    /// `createImageBitmap`: the canvas's texture as it is when a draw of it
    /// is replayed. A draw of it replays at once, a pattern
    /// ([`super::Pattern::new`]) and a bitmap ([`Image::crop`]) copy it, so
    /// each sees the pixels as of this call. Replays what is recorded first.
    pub fn image(&mut self) -> Image {
        match &mut self.raster {
            Some(raster) => {
                raster.flush();
                Image(Source::Live(raster.surface.texture()))
            }
            None => Image::empty(),
        }
    }

    /// HTML's update of an `OffscreenCanvas`'s placeholder: the bitmap, with
    /// every draw recorded so far, becomes what the placeholder `<canvas>` of
    /// `frames` shows, and the canvas is marked unchanged. Answers what the
    /// commit owes; nothing for a canvas with no bitmap or a freed element.
    #[must_use = "a wake or a resize the commit owes is lost"]
    pub fn commit(&mut self, frames: &CanvasFrames) -> Committed {
        self.changed = false;
        let Some(raster) = &mut self.raster else {
            return Committed::default();
        };
        raster.flush();
        raster.surface.commit(frames)
    }

    /// Replays the draws recorded since the last replay onto the bitmap, so
    /// the recording does not grow for a canvas nothing reads.
    pub fn flush(&mut self) {
        if let Some(raster) = &mut self.raster {
            raster.flush();
        }
    }

    /// Whether draws are recorded and not yet replayed.
    #[must_use]
    pub fn has_pending_draws(&self) -> bool {
        self.raster.as_ref().is_some_and(Raster::pending)
    }

    /// Whether anything may have changed the bitmap since the last
    /// [`Self::commit`] (or since creation).
    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.changed
    }

    /// `transferToImageBitmap()`: hands the bitmap's texture out and leaves
    /// the canvas transparent black, its context state untouched.
    pub fn take_image(&mut self) -> Image {
        self.changed = true;
        match &mut self.raster {
            Some(raster) => {
                raster.flush();
                Image(Source::Texture(raster.surface.take()))
            }
            None => Image::empty(),
        }
    }
}
