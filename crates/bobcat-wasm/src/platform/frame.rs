//! [`BrowserFrame`]: the browser's platform frame, an `ImageBitmap` the
//! Render Worker decoded, and its writer, which copies it into the texture
//! the painter chose with WebGPU's `copyExternalImageToTexture`.

use std::rc::Rc;

use bobcat_core::{FrameTarget, FrameWriter, wgpu};

/// An `ImageBitmap` decoded in the Render Worker, as the engine's platform
/// frame: its pixels stay where the browser keeps them until its writer
/// copies them into a texture, and never pass through Wasm memory.
///
/// Cloning shares the bitmap. It is closed when the last clone goes — when
/// the resource system has let go of it, the painter holding its upload —
/// so the browser frees it then rather than at its next garbage collection.
/// It never leaves the Render Worker: an `ImageBitmap` or a `VideoFrame`
/// produced on another thread would be transferred to the Render Worker
/// with `postMessage` and wrapped here.
#[derive(Clone, Debug)]
pub struct BrowserFrame(Rc<Bitmap>);

#[derive(Debug)]
struct Bitmap(web_sys::ImageBitmap);

impl Drop for Bitmap {
    fn drop(&mut self) {
        self.0.close();
    }
}

impl BrowserFrame {
    /// Takes `bitmap` over: the frame closes it when its last clone goes.
    pub(crate) fn new(bitmap: web_sys::ImageBitmap) -> Self {
        Self(Rc::new(Bitmap(bitmap)))
    }

    /// The bitmap itself.
    pub(crate) fn bitmap(&self) -> &web_sys::ImageBitmap {
        &self.0.0
    }

    /// The bitmap's size in pixels; zero once it has been closed.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.0.0.width(), self.0.0.height())
    }
}

impl FrameWriter for BrowserFrame {
    /// Queues `copyExternalImageToTexture` from the bitmap into the texture,
    /// premultiplied, in sRGB: what `vello_gpu` samples. The bitmap was
    /// decoded with the browser's colour management into sRGB
    /// (`colorSpaceConversion: "default"`), and the copy premultiplies
    /// whatever alpha state the bitmap holds. Refuses a bitmap that is not
    /// the texture's size, which a closed one is not.
    fn write(&self, target: &FrameTarget<'_>) -> bool {
        let (width, height) = target.size;
        if self.size() != (width, height) {
            return false;
        }
        target.queue.copy_external_image_to_texture(
            &wgpu::CopyExternalImageSourceInfo {
                source: wgpu::ExternalImageSource::ImageBitmap(self.0.0.clone()),
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
