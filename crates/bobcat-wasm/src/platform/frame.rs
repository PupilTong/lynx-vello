//! [`BrowserFrame`]: the browser's platform frame, an `ImageBitmap` the
//! Render Worker decoded or a `VideoFrame` the facade's `<video>` presented
//! and transferred here, and its writer, which copies it into the texture
//! the painter chose with WebGPU's `copyExternalImageToTexture`.

use std::rc::Rc;

use bobcat_core::{FrameTarget, FrameWriter, wgpu};

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
    pub(crate) fn new(bitmap: web_sys::ImageBitmap) -> Self {
        Self(Rc::new(Source::Bitmap(bitmap)))
    }

    pub(crate) fn video(frame: web_sys::VideoFrame) -> Self {
        Self(Rc::new(Source::Video(frame)))
    }

    pub(crate) fn bitmap(&self) -> Option<&web_sys::ImageBitmap> {
        match &*self.0 {
            Source::Bitmap(bitmap) => Some(bitmap),
            Source::Video(_) => None,
        }
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        match &*self.0 {
            Source::Bitmap(bitmap) => (bitmap.width(), bitmap.height()),
            Source::Video(frame) => (frame.display_width(), frame.display_height()),
        }
    }
}

impl FrameWriter for BrowserFrame {
    fn write(&self, target: &FrameTarget<'_>) -> bool {
        let (width, height) = target.size;
        if self.size() != (width, height) {
            return false;
        }
        let source = match &*self.0 {
            Source::Bitmap(bitmap) => wgpu::ExternalImageSource::ImageBitmap(bitmap.clone()),
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
