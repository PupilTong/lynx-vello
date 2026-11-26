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
//!
//! Each target has one; every one on a device draws through the
//! renderer and the bitmaps of that device's [`SharedGpu`], and keeps its
//! own scene and bound textures.

use std::sync::Arc;

use super::gpu::{GpuError, SharedGpu};
use crate::paint::display_list::{DisplayList, Texture};
use crate::peniko::Color;

pub(crate) struct Raster {
    sparse: super::sparse::SparseBackend,
}

impl std::fmt::Debug for Raster {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Raster").finish_non_exhaustive()
    }
}

impl Raster {
    pub(crate) const TARGET_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT;

    pub(crate) const BOUND_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING;

    pub(crate) fn new(gpu: &Arc<SharedGpu>) -> Self {
        Self {
            sparse: super::sparse::SparseBackend::new(gpu),
        }
    }

    pub(crate) fn render_to_texture(
        &mut self,
        scene: &DisplayList,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        base: Color,
    ) -> Result<(), GpuError> {
        self.sparse
            .render(scene, view, width, height, base)
            .map_err(GpuError::Render)
    }

    pub(crate) fn bind_texture(&mut self, texture: &Texture, bound: Option<wgpu::Texture>) {
        self.sparse.bind(texture, bound);
    }

    pub(crate) fn lost_pixels(&self) -> u64 {
        self.sparse.lost()
    }
}
