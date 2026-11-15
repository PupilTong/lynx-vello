//! [`BrowserFrame`]: the browser's platform frame, an `ImageBitmap` the
//! Render Worker decoded or a `VideoFrame` the facade's `<video>` presented
//! and transferred here, and its writer, which copies it into the texture
//! the painter chose with WebGPU's `copyExternalImageToTexture`.

use std::rc::Rc;

use bobcat_core::{FrameTarget, FrameWriter, wgpu};

/// An `ImageBitmap` decoded in the Render Worker, or a `VideoFrame` of a
/// video ([`super::media`]), as the engine's platform frame: its pixels stay
/// where the browser keeps them until its writer copies them into a
/// texture, and never pass through Wasm memory.
///
/// Cloning shares the frame. It is closed when the last clone goes — when
/// the resource system has let go of it, the painter holding its upload or
/// its texture — so the browser frees it then rather than at its next
/// garbage collection; a `VideoFrame` held open holds a decoder buffer. It
/// never leaves the Render Worker: a frame produced on another thread is
/// transferred to the Render Worker with `postMessage` and wrapped here.
#[derive(Clone, Debug)]
pub struct BrowserFrame(Rc<Source>);

#[derive(Debug)]
enum Source {
    Bitmap(web_sys::ImageBitmap),
    Video(web_sys::VideoFrame),
}

impl Drop for Source {
    fn drop(&mut self) {
        match self {
            Self::Bitmap(bitmap) => bitmap.close(),
            Self::Video(frame) => frame.close(),
        }
    }
}

impl BrowserFrame {
    /// Takes `bitmap` over: the frame closes it when its last clone goes.
    pub(crate) fn new(bitmap: web_sys::ImageBitmap) -> Self {
        Self(Rc::new(Source::Bitmap(bitmap)))
    }

    /// Takes `frame` over: the frame closes it when its last clone goes.
    pub(crate) fn video(frame: web_sys::VideoFrame) -> Self {
        Self(Rc::new(Source::Video(frame)))
    }

    /// The bitmap itself, for a decoded image.
    pub(crate) fn bitmap(&self) -> Option<&web_sys::ImageBitmap> {
        match &*self.0 {
            Source::Bitmap(bitmap) => Some(bitmap),
            Source::Video(_) => None,
        }
    }

    /// The frame's size in pixels: a bitmap's, or a video frame's display
    /// size, which is what `copyExternalImageToTexture` copies; zero once
    /// it has been closed.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        match &*self.0 {
            Source::Bitmap(bitmap) => (bitmap.width(), bitmap.height()),
            Source::Video(frame) => (frame.display_width(), frame.display_height()),
        }
    }
}

impl FrameWriter for BrowserFrame {
    /// Queues `copyExternalImageToTexture` from the bitmap or the video
    /// frame into the texture, premultiplied, in sRGB: what `vello_gpu`
    /// samples. A bitmap was decoded with the browser's colour management
    /// into sRGB (`colorSpaceConversion: "default"`), and the copy converts a
    /// video frame from its own colour space into the destination's and
    /// premultiplies whatever alpha state the source holds. Refuses a frame
    /// that is not the texture's size, which a closed one is not.
    fn write(&self, target: &FrameTarget<'_>) -> bool {
        let (width, height) = target.size;
        if self.size() != (width, height) {
            return false;
        }
        let source = match &*self.0 {
            Source::Bitmap(bitmap) => wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()),
            // The handle, not `VideoFrame.clone()`, which would open a new
            // frame that nothing closes.
            Source::Video(frame) => {
                wgpu::ExternalImageSource::VideoFrame(<web_sys::VideoFrame as Clone>::clone(frame))
            }
        };
        target.queue.copy_external_image_to_texture(
            &wgpu::CopyExternalImageSourceInfo {
                source,
                origin: wgpu::Origin2d::ZERO,
                flip_y: false,
            },
            wgpu::CopyExternalImageDestInfo {
                texture: target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
                color_space: wgpu::PredefinedColorSpace::Srgb,
                premultiplied_alpha: true,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        true
    }
}
