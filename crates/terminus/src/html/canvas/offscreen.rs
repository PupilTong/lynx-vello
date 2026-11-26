//! [`OffscreenCanvas`]: HTML's `OffscreenCanvas` ("The `OffscreenCanvas`
//! interface") with a 2D context: its size, its bitmap, the commit to its
//! placeholder, and `transferToImageBitmap()`. The context itself is
//! [`Context2d`], borrowed through [`OffscreenCanvas::context_2d`].

use super::context2d::{Context2d, Context2dData, Raster, Source};
use super::{CanvasFrames, Committed, Image};

#[derive(Debug)]
pub struct OffscreenCanvas {
    width: u32,
    height: u32,
    raster: Option<Raster>,
    changed: bool,
    context: Context2dData,
}

impl OffscreenCanvas {
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

    pub fn image(&mut self) -> Image {
        match &mut self.raster {
            Some(raster) => {
                raster.flush();
                Image(Source::Live(raster.surface.texture()))
            }
            None => Image::empty(),
        }
    }

    #[must_use = "a wake or a resize the commit owes is lost"]
    pub fn commit(&mut self, frames: &CanvasFrames) -> Committed {
        self.changed = false;
        let Some(raster) = &mut self.raster else {
            return Committed::default();
        };
        raster.flush();
        raster.surface.commit(frames)
    }

    pub fn flush(&mut self) {
        if let Some(raster) = &mut self.raster {
            raster.flush();
        }
    }

    #[must_use]
    pub fn has_pending_draws(&self) -> bool {
        self.raster.as_ref().is_some_and(Raster::pending)
    }

    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.changed
    }

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
