//! The one rasterizer a target renders every scene through: its frames and
//! every bake.
//!
//! The paint and compose layers build a [`vello::Scene`] whichever
//! rasterizer is compiled in; what differs is what turns it into pixels.
//!
//! - By default, vello's compute pipeline: a [`vello::Renderer`] paired with its
//!   [`AtlasResidency`], which every render through it owes a pass.
//! - With the `vello_gpu` feature, `vello_gpu`'s sparse strips over its wgpu backend
//!   ([`super::sparse`]): the scene's encoding is replayed into a `vello_gpu::Scene` per render,
//!   and the bake textures are bound as external textures rather than copied into an atlas.
//!
//! Both answer the same three calls — render a scene into a texture view,
//! bind or unbind a texture an [`ImageData`] handle names, and say a bound
//! texture's contents changed — so the targets ([`super::gpu::Headless`],
//! the painter's window) and the bakes ([`super::blur`],
//! [`super::vector_textures`]) are written once.

use vello::peniko::{Color, ImageAlphaType, ImageData};
use vello::wgpu;

use super::gpu::GpuError;
#[cfg(not(feature = "vello_gpu"))]
use super::gpu::{AtlasResidency, render_params, renderer_options};

/// The tables of images a render may draw, beside the scene's own patches:
/// the bitmaps the frame resolved, its `filter: blur()` bakes, and its vector
/// bakes. The default rasterizer's atlas residency has to see every one of
/// them; `vello_gpu` reads none of them, because a bake is bound by handle.
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(
    feature = "vello_gpu",
    allow(dead_code, reason = "read by the default rasterizer")
)]
pub(crate) struct Drawn<'a> {
    pub(crate) images: &'a [Option<ImageData>],
    pub(crate) filtered: &'a [Option<ImageData>],
    pub(crate) vectors: &'a [Option<ImageData>],
}

/// The rasterizer this build renders through, named for a golden file: `None`
/// for vello's compute pipeline (the default), `Some("vello_gpu")` under the
/// `vello_gpu` feature.
///
/// A screenshot store keys its backend override goldens on it
/// (`<name>.vello_gpu.png` beside the shared `<name>.png`), so a test reads
/// the rasterizer it was compiled against from here rather than from a flag
/// of its own.
pub const RASTER_VARIANT: Option<&str> = if cfg!(feature = "vello_gpu") {
    Some("vello_gpu")
} else {
    None
};

/// See the [module documentation](self).
pub(crate) struct Raster {
    #[cfg(not(feature = "vello_gpu"))]
    renderer: vello::Renderer,
    #[cfg(not(feature = "vello_gpu"))]
    atlas: AtlasResidency,
    #[cfg(feature = "vello_gpu")]
    sparse: super::sparse::SparseBackend,
}

impl std::fmt::Debug for Raster {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Raster").finish_non_exhaustive()
    }
}

impl Raster {
    /// The usage a texture this rasterizer renders into needs: vello writes
    /// through a storage binding, `vello_gpu` through a render pass.
    pub(crate) const TARGET_USAGE: wgpu::TextureUsages = if cfg!(feature = "vello_gpu") {
        wgpu::TextureUsages::RENDER_ATTACHMENT
    } else {
        wgpu::TextureUsages::STORAGE_BINDING
    };

    /// The usage a texture bound through [`Self::override_image`] needs:
    /// vello copies it into its atlas, `vello_gpu` samples it in place.
    pub(crate) const BOUND_USAGE: wgpu::TextureUsages = if cfg!(feature = "vello_gpu") {
        wgpu::TextureUsages::TEXTURE_BINDING
    } else {
        wgpu::TextureUsages::COPY_SRC
    };

    /// The alpha a texture [`Self::render_to_texture`] filled holds: vello's
    /// fine stage stores straight colour, `vello_gpu` blends premultiplied.
    pub(crate) const OUTPUT_ALPHA: ImageAlphaType = if cfg!(feature = "vello_gpu") {
        ImageAlphaType::AlphaPremultiplied
    } else {
        ImageAlphaType::Alpha
    };

    /// Builds the rasterizer on `device`.
    ///
    /// # Errors
    ///
    /// [`GpuError::Render`] if vello refuses to build its pipelines.
    #[cfg_attr(
        feature = "vello_gpu",
        expect(
            clippy::unnecessary_wraps,
            reason = "only the default rasterizer's pipelines can fail to build"
        )
    )]
    pub(crate) fn new(device: &wgpu::Device) -> Result<Self, GpuError> {
        #[cfg(not(feature = "vello_gpu"))]
        {
            let renderer = vello::Renderer::new(device, renderer_options())
                .map_err(|error| GpuError::Render(error.to_string()))?;
            Ok(Self {
                renderer,
                atlas: AtlasResidency::default(),
            })
        }
        #[cfg(feature = "vello_gpu")]
        {
            Ok(Self {
                sparse: super::sparse::SparseBackend::new(device),
            })
        }
    }

    /// Renders `scene` into `view`, a `width` × `height` RGBA8 texture
    /// created with [`Self::TARGET_USAGE`], over `base`.
    ///
    /// # Errors
    ///
    /// [`GpuError::Render`] if the render fails.
    #[expect(
        clippy::too_many_arguments,
        reason = "one render's full inputs: the device pair, the scene, the target and its size, \
                  the base colour and the images the scene may draw"
    )]
    pub(crate) fn render_to_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &vello::Scene,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        base: Color,
        drawn: Drawn<'_>,
    ) -> Result<(), GpuError> {
        #[cfg(not(feature = "vello_gpu"))]
        {
            self.atlas.prepare_all(
                &mut self.renderer,
                scene,
                drawn.images,
                drawn.filtered,
                drawn.vectors,
            );
            self.renderer
                .render_to_texture(
                    device,
                    queue,
                    scene,
                    view,
                    &render_params(base, width, height),
                )
                .map_err(|error| GpuError::Render(error.to_string()))
        }
        #[cfg(feature = "vello_gpu")]
        {
            let _ = drawn;
            self.sparse
                .render(device, queue, scene, view, width, height, base)
                .map_err(GpuError::Render)
        }
    }

    /// Binds `texture` to every draw of `handle`, or with `None` unbinds it.
    ///
    /// `texture` matches `handle`'s dimensions and was created with
    /// [`Self::BOUND_USAGE`]; its alpha is `handle`'s alpha type.
    pub(crate) fn override_image(&mut self, handle: &ImageData, texture: Option<wgpu::Texture>) {
        #[cfg(not(feature = "vello_gpu"))]
        self.renderer.override_image(
            handle,
            texture.map(|texture| wgpu::TexelCopyTextureInfoBase {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            }),
        );
        #[cfg(feature = "vello_gpu")]
        self.sparse.bind(handle, texture);
    }

    /// Says the texture bound to `handle` was drawn into since the last
    /// render. vello re-copies it into its atlas; `vello_gpu` samples the
    /// texture itself and owes nothing.
    #[cfg_attr(
        feature = "vello_gpu",
        expect(
            clippy::unused_self,
            reason = "only the default rasterizer has an atlas to re-copy into"
        )
    )]
    pub(crate) fn mark_override_image_dirty(&mut self, handle: &ImageData) {
        #[cfg(not(feature = "vello_gpu"))]
        self.renderer.mark_override_image_dirty(handle);
        #[cfg(feature = "vello_gpu")]
        let _ = handle;
    }
}
