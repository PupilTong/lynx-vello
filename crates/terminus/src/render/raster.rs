//! The one rasterizer a target renders every scene through: its frames and
//! every bake.
//!
//! The paint and compose layers build a [`DisplayList`]; [`Raster`] turns
//! it into pixels with `vello_gpu`'s sparse strips over its wgpu backend
//! ([`super::sparse`]): the list's items are replayed into a
//! `vello_gpu::Scene` per render, and the bake textures are bound by handle
//! and sampled in place as external textures.
//!
//! It answers two calls — render a list into a texture view, and bind or
//! unbind the texture a [`Texture`] names — so the targets
//! ([`super::gpu::Headless`], the painter's window) and the bakes
//! ([`super::blur`], [`super::vector_textures`]) are written once.

use super::gpu::GpuError;
use crate::paint::display_list::{DisplayList, Texture};
use crate::peniko::Color;

/// See the [module documentation](self).
pub(crate) struct Raster {
    sparse: super::sparse::SparseBackend,
}

impl std::fmt::Debug for Raster {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Raster").finish_non_exhaustive()
    }
}

impl Raster {
    /// The usage a texture this rasterizer renders into needs: it draws
    /// through a render pass.
    pub(crate) const TARGET_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT;

    /// The usage a texture bound through [`Self::bind_texture`] needs: it
    /// is sampled in place.
    pub(crate) const BOUND_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING;

    /// Builds the rasterizer on `device`.
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            sparse: super::sparse::SparseBackend::new(device),
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
                  and the base colour"
    )]
    pub(crate) fn render_to_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &DisplayList,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        base: Color,
    ) -> Result<(), GpuError> {
        self.sparse
            .render(device, queue, scene, view, width, height, base)
            .map_err(GpuError::Render)
    }

    /// Binds `bound` to every draw of `texture`, or with `None` unbinds it.
    ///
    /// `bound` is the size [`Texture`] names as its texture size, was
    /// created with [`Self::BOUND_USAGE`], and holds premultiplied colour —
    /// what the rasterizer writes into every target, since it blends into
    /// it premultiplied. A draw samples the texture as it is at that render,
    /// so redrawing into a bound texture needs no further call.
    pub(crate) fn bind_texture(&mut self, texture: &Texture, bound: Option<wgpu::Texture>) {
        self.sparse.bind(texture, bound);
    }
}
