//! The device-free half of a canvas surface's render and the device side
//! that finishes it ([`super::super::canvas`]): the rasterizer's pieces
//! only a canvas uses, beside the bitmap table they read.
//!
//! A canvas surface splits a render in two: the replay runs with no device
//! ([`encode`]) on the thread that draws the canvas, drawing each bitmap
//! under a texture id reserved for the scene, and the GPU half
//! ([`Submitter::submit`]) resolves those bitmaps through the bitmap table
//! and renders, on the thread that holds the device — the same thread
//! natively, the Render Worker on wasm32. The encode holds no lock; the
//! submit takes the bitmap table's lock per bitmap and then the renderer's,
//! never both together ([`SharedRaster`]).

#![allow(
    clippy::cast_possible_truncation,
    reason = "canvas sides are at most the device's longest texture side, which fits u16"
)]

use std::cell::RefCell;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use vello_gpu::{Image, ImageSource, Scene, TextureBindings, TextureId, TextureRegion};

use super::{
    BITMAP_USAGE, BitmapKey, Drawing, Missing, Pictures, Replay, SharedRaster, Stats,
    bitmap_texture, next_target, readable, replay,
};
use crate::paint::display_list::{DisplayList, ImageRef, next_texture_id};
use crate::peniko::{Color, ImageSampler};
use crate::render::gpu::SharedGpu;
use crate::render::image::ImageDescriptor;
use crate::render::picture::PictureId;

/// The next lease owner's name, from the counter a target's name in the
/// bitmaps' pins comes from: what a placeholder canvas's link holds its
/// picture's texture under ([`SharedRaster::copy_to_picture`]).
pub(crate) fn next_owner() -> u64 {
    next_target()
}

impl Replay {
    /// How many glyph outlines glifo's cache holds, for a test: read from
    /// its `Debug` output, the one place glifo shows the count.
    #[cfg(test)]
    pub(crate) fn cached_outlines(&self) -> usize {
        let debug = format!("{:?}", self.glyphs);
        let (_, count) = debug
            .split_once("cached_count: ")
            .expect("glifo's outline cache lists its count");
        count
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|digits| digits.parse().ok())
            .expect("a count")
    }
}

impl SharedRaster {
    /// Ends every lease `owner` holds.
    pub(crate) fn end_leases(&self, owner: u64) {
        self.bitmaps().end_leases(owner);
    }

    /// Copies `source`, `width` × `height`, into `picture`'s own texture as
    /// its frame `generation`, which `descriptor` names, and leases that
    /// texture to `owner`; the copy is submitted before this returns. The
    /// texture is rewritten in place under its one id when it is the size
    /// of the last frame, and replaced under the same id when it is not, as
    /// [`Self::write_picture`] does.
    pub(crate) fn copy_to_picture(
        &self,
        gpu: &SharedGpu,
        owner: u64,
        picture: PictureId,
        generation: u64,
        descriptor: ImageDescriptor,
        source: &wgpu::Texture,
    ) {
        let (width, height) = (descriptor.width, descriptor.height);
        let key = BitmapKey::Picture(picture);
        let held = self.bitmaps().entries.get(&key).map(|bitmap| {
            let texture = bitmap.view.texture();
            (
                bitmap.id,
                (texture.width(), texture.height()) == (width, height),
                texture.clone(),
            )
        });
        let copy_into = |texture: &wgpu::Texture| {
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("terminus canvas commit"),
                });
            encoder.copy_texture_to_texture(
                source.as_image_copy(),
                texture.as_image_copy(),
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            gpu.submit([encoder.finish()]);
        };
        let replaced = if let Some((_, true, texture)) = &held {
            copy_into(texture);
            None
        } else {
            let texture = bitmap_texture(&gpu.device, width, height, BITMAP_USAGE);
            copy_into(&texture);
            let id = held
                .as_ref()
                .map_or_else(|| TextureId(next_texture_id()), |(id, _, _)| *id);
            Some((
                id,
                texture.create_view(&wgpu::TextureViewDescriptor::default()),
            ))
        };
        self.store_picture(
            owner,
            picture,
            generation,
            descriptor,
            held,
            replaced,
            u64::from(width) * u64::from(height) * 4,
            false,
        );
    }
}

/// A list replayed into a scene by [`encode`], on a thread that may hold no
/// device: the CPU half of a render, which [`Submitter::submit`] finishes
/// on the device's thread.
///
/// A bitmap the list draws is drawn in the scene as an external texture
/// under an id reserved for this scene, one per upload the bitmap table
/// keys, and resolved through the bitmap table only at the submit, which
/// binds the upload's texture under that id. A texture paint is drawn under
/// its own id, which the submit's caller binds.
pub(crate) struct Encoded {
    scene: Scene,
    /// Whether the target is cleared to transparent black before the
    /// render, rather than kept and drawn over.
    clear: bool,
    /// Each bitmap the scene draws: the id it is drawn under, and the bitmap.
    bitmaps: Vec<(TextureId, ImageRef)>,
    /// The texture ids the scene draws, each once.
    textures: Vec<u64>,
}

impl std::fmt::Debug for Encoded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoded")
            .field("width", &self.scene.width())
            .field("height", &self.scene.height())
            .field("textures", &self.textures)
            .finish_non_exhaustive()
    }
}

impl Encoded {
    /// The texture ids the scene draws, each once: every one needs a
    /// binding at the submit.
    pub(crate) fn textures(&self) -> &[u64] {
        &self.textures
    }

    /// The scene, for the next [`encode`] to reuse its buffers.
    pub(crate) fn into_scene(self) -> Scene {
        self.scene
    }
}

/// The CPU half of a render, with no device: `list` replayed into `scene`,
/// reset to a `width` × `height` target that is cleared to transparent
/// black first when `clear` is set, or kept and drawn over, as
/// [`super::SparseBackend::render`] replays it. A texture paint whose id
/// `drawable` refuses draws nothing, as one with no texture bound does
/// there. Neither needs the base colour fill a page target's render draws
/// under a blend at the root, which only a visible clear colour needs.
///
/// What a render decides about a bitmap without a device is decided here:
/// one with a side longer than any texture, or whose pixels are fewer bytes
/// than it needs, draws nothing, as it does there. Whether an upload is
/// resident, and whether lent pixels are still alive, is decided at the
/// submit.
pub(crate) fn encode(
    mut scene: Scene,
    kept: &mut Replay,
    list: &DisplayList,
    (width, height): (u32, u32),
    clear: bool,
    drawable: &dyn Fn(u64) -> bool,
) -> Encoded {
    scene.reset_and_resize(width as u16, height as u16);
    let longest = SharedGpu::LIMITS.max_texture_dimension_2d;
    let mut bitmaps: Vec<(TextureId, ImageRef)> = Vec::new();
    let mut reserved: FxHashMap<BitmapKey, TextureId> = FxHashMap::default();
    let textures: RefCell<Vec<u64>> = RefCell::new(Vec::new());
    replay(
        &mut scene,
        list,
        kept,
        &mut Pictures {
            bitmap: &mut |paint| {
                let image = &paint.image;
                if image.picture.is_none() {
                    // No upload keyed on these pixels can exist: none is
                    // made of a bitmap that cannot be converted.
                    let descriptor = &image.descriptor;
                    if descriptor.width > longest || descriptor.height > longest {
                        return Err(Missing::Unreadable);
                    }
                    if image.pixels().is_some_and(|pixels| !readable(&pixels)) {
                        return Err(Missing::Unreadable);
                    }
                }
                let id = *reserved.entry(BitmapKey::drawn(image)).or_insert_with(|| {
                    let id = TextureId(next_texture_id());
                    bitmaps.push((id, image.clone()));
                    id
                });
                // Whether the upload is opaque is not known here; the flag
                // only lets `vello_gpu` route opaque strips to its depth
                // pass, which no render here enables (`depth_view: None`).
                Ok(Image {
                    image: ImageSource::external_texture(id, TextureRegion::Full, true),
                    sampler: paint.sampler,
                })
            },
            bound: &|id| {
                let drawn = drawable(id);
                let mut textures = textures.borrow_mut();
                if drawn && !textures.contains(&id) {
                    textures.push(id);
                }
                drawn
            },
        },
    );
    kept.maintain();
    Encoded {
        scene,
        clear,
        bitmaps,
        textures: textures.into_inner(),
    }
}

/// A 1 × 1 transparent black texture: what a submit binds under an id whose
/// texture it cannot find.
///
/// In `vello_gpu` a draw composites by source-over into the layer it is in
/// (a blend mode is a layer's, never a draw's), so a draw sampling only
/// transparent black leaves that layer's pixels as they are, as no draw
/// does. What it still changes is the bounding box the scene recorded for
/// each layer around it, which was fixed at the encode either way: a
/// non-destructive blend layer then blends a transparent source over its
/// backdrop, which leaves the backdrop, and a destructive one composites
/// over its whole clip region whatever it holds
/// (`schedule_child_layer`, `vello_gpu` `schedule/mod.rs`).
pub(crate) fn transparent_view(device: &wgpu::Device) -> wgpu::TextureView {
    // wgpu zero-fills a texture before its first use.
    bitmap_texture(device, 1, 1, BITMAP_USAGE).create_view(&wgpu::TextureViewDescriptor::default())
}

/// The device side of a target whose scenes are encoded elsewhere
/// ([`encode`]): its name in the bitmaps' pins and its count of renders, as
/// a [`super::SparseBackend`] keeps them.
pub(crate) struct Submitter {
    gpu: Arc<SharedGpu>,
    target: u64,
    generation: u64,
    /// The textures [`super::Bitmaps::hit`] lists for a render: where a submit
    /// finds each upload's texture, which it binds under the id the
    /// encoded scene reserved for the bitmap.
    scratch: Vec<(TextureId, wgpu::TextureView)>,
}

impl std::fmt::Debug for Submitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Submitter")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl Submitter {
    /// A target on `gpu` that has rendered nothing yet.
    pub(crate) fn new(gpu: &Arc<SharedGpu>) -> Self {
        Self {
            gpu: Arc::clone(gpu),
            target: next_target(),
            generation: 0,
            scratch: Vec::new(),
        }
    }

    /// The stack this target renders on.
    pub(crate) fn gpu(&self) -> &Arc<SharedGpu> {
        &self.gpu
    }

    /// The GPU half of the render `encoded` began: each bitmap it draws
    /// resolved through the bitmap table — a hit, or the pixels converted
    /// and uploaded — and bound under the id the scene draws it by, with
    /// `sources` bound under theirs, then the scene rendered into `view`
    /// under the renderer lock and submitted ([`SharedRaster`]).
    ///
    /// `sources` binds every id of [`Encoded::textures`]. A bitmap with no
    /// upload whose pixels are gone or unreadable is bound as transparent
    /// black ([`transparent_view`]), which draws as nothing does.
    pub(crate) fn submit(
        &mut self,
        encoded: &Encoded,
        view: &wgpu::TextureView,
        sources: &FxHashMap<u64, wgpu::TextureView>,
    ) -> Result<(), String> {
        let Self {
            gpu,
            target,
            generation,
            scratch,
        } = self;
        let SharedGpu {
            device,
            queue,
            raster: shared,
            ..
        } = &**gpu;
        let mut bindings = TextureBindings::new();
        for (&id, view) in sources {
            bindings.insert(TextureId(id), view.clone());
        }
        let mut drawing = Drawing {
            target: *target,
            generation,
            tick: None,
            textures: scratch,
        };
        let mut transparent = None;
        for (id, image) in &encoded.bitmaps {
            // The image drawing the upload names it by the upload's own id,
            // under which the render's first draw of it lists its texture
            // in `drawing.textures`.
            let upload = shared
                .bitmap(device, queue, image, ImageSampler::new(), &mut drawing)
                .ok()
                .and_then(|drawn| match drawn.image {
                    ImageSource::ExternalTexture { id, .. } => Some(id),
                    _ => None,
                });
            let view = upload
                .and_then(|upload| drawing.textures.iter().find(|(id, _)| *id == upload))
                .map_or_else(
                    || {
                        transparent
                            .get_or_insert_with(|| transparent_view(device))
                            .clone()
                    },
                    |(_, view)| view.clone(),
                );
            bindings.insert(*id, view);
        }
        drawing.textures.clear();
        shared.render(
            gpu,
            &encoded.scene,
            view,
            &bindings,
            encoded.clear.then_some(Color::TRANSPARENT),
            &mut Stats::default(),
        )
    }
}

impl Drop for Submitter {
    /// Gives up this target's pins, as [`super::SparseBackend`]'s drop does.
    fn drop(&mut self) {
        self.gpu.raster.bitmaps().targets.remove(&self.target);
    }
}
