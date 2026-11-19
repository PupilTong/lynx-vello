//! Canvas surfaces: a `<canvas>` or `OffscreenCanvas` bitmap as a GPU
//! texture its owner draws into by replaying [`DisplayList`]s, and the
//! textures canvas images hold.
//!
//! The owner records draws into a list on its own thread and hands the list
//! over at each flush ([`Surface::draw`]); nothing is drawn while it
//! records. Every GPU step is one [`Op`] applied by [`apply`] to the
//! textures of [`SharedRaster`]'s canvas table, keyed by a texture id that
//! is also the id a draw names the texture by ([`Texture`]):
//!
//! - **Natively** the owner's thread applies each op as it is issued, with the surface's own
//!   [`SparseBackend`] for its replays: the list is replayed with no lock held, the shared renderer
//!   is locked for the render and submit, and a read blocks on the readback.
//! - **On wasm32** the device lives on the Render Worker alone. An op is appended to one queue in
//!   memory every Worker shares and the Render Worker is woken ([`set_waker`]); it applies the
//!   queue in order at the start of each turn ([`drain`]), with one [`SparseBackend`] for every
//!   canvas. A read's answer is sent from the readback's completion on that Worker, and the caller
//!   blocks on it.
//!
//! A surface holds one texture, created at the first op that needs it and
//! zero-filled, and a second one once a draw has read its own pixels
//! ([`Init::Backdrop`]): `vello_gpu` composites a layer that blends by
//! anything but source-over into a transparent intermediate when it sits at
//! the root (`vello_common::record::CommandRecorder::root_is_blend_target`,
//! `vello_gpu` `schedule/mod.rs` `schedule_root`), and refuses to sample
//! the texture it renders into (`RenderError::TextureFeedbackLoop`). Such a
//! batch is rendered into the second texture over a clear, with the first
//! sampled under the surface's id, and the two change places.

use std::sync::Arc;

use rustc_hash::FxHashMap;

use super::gpu::SharedGpu;
use super::sparse::{CanvasTexture, SparseBackend};
use crate::html::canvas::{CanvasFrames, Committed};
use crate::paint::display_list::{DisplayList, Item, Paint, Texture, next_texture_id};
use crate::peniko::Color;
use crate::render::image::ImageDescriptor;
use crate::render::picture::PictureId;

/// The longest side a canvas texture has: the device's
/// `max_texture_dimension_2d` under the limits every device is created
/// with ([`SharedGpu::LIMITS`]).
pub(crate) const MAX_SURFACE_SIDE: u32 = SharedGpu::LIMITS.max_texture_dimension_2d;

/// How a draw treats what the surface holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Init {
    /// Drawn over the content.
    Load,
    /// The content is cleared to transparent black first.
    Clear,
    /// Drawn into the surface's second texture over a clear, with the
    /// content bound under the surface's own id, which the list samples:
    /// the list draws the old content itself.
    Backdrop,
}

/// Why a canvas operation produced nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceError(pub(crate) String);

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SurfaceError {}

/// One GPU step on the canvas textures.
enum Op {
    /// Natively a surface replays its list in place instead
    /// ([`Surface::draw`]), keeping the list's capacity.
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(dead_code, reason = "queued on wasm32 only")
    )]
    Draw {
        target: u64,
        size: (u32, u32),
        list: Box<DisplayList>,
        init: Init,
        /// Images the list draws, kept until it has been replayed.
        _keep: Vec<SurfaceImage>,
    },
    Write {
        target: u64,
        size: (u32, u32),
        origin: (u32, u32),
        extent: (u32, u32),
        texels: Vec<u8>,
    },
    Read {
        target: u64,
        size: (u32, u32),
        origin: (u32, u32),
        extent: (u32, u32),
        reply: flume::Sender<Result<Vec<u8>, String>>,
    },
    /// `source`'s pixels at `from` copied to `to` in a new texture `copy`.
    Copy {
        source: u64,
        source_size: (u32, u32),
        from: (u32, u32),
        to: (u32, u32),
        extent: (u32, u32),
        copy: u64,
        copy_size: (u32, u32),
    },
    /// `from`'s texture becomes `to`'s; `from` has none until its next op.
    Move {
        from: u64,
        to: u64,
        size: (u32, u32),
    },
    Commit {
        target: u64,
        size: (u32, u32),
        picture: PictureId,
        owner: u64,
        generation: u64,
        descriptor: ImageDescriptor,
    },
    Drop(u64),
    EndLease(u64),
}

/// A canvas bitmap on the GPU: a texture of `width` × `height` premultiplied
/// RGBA8, transparent black until drawn. See the [module documentation](self).
///
/// `Send`: the owner may move it between threads, and it is used from one
/// at a time.
pub(crate) struct Surface {
    id: u64,
    width: u32,
    height: u32,
    /// The replay state natively, made at the first op that needs one.
    #[cfg(not(target_arch = "wasm32"))]
    backend: Option<SparseBackend>,
}

impl std::fmt::Debug for Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surface")
            .field("id", &self.id)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// An image a canvas holds on the GPU: what `transferToImageBitmap` hands
/// out and a copy of a canvas is. Cloning shares the texture, which is
/// released with the last clone.
#[derive(Clone, Debug)]
pub(crate) struct SurfaceImage(Arc<ImageTexture>);

#[derive(Debug)]
struct ImageTexture {
    id: u64,
    width: u32,
    height: u32,
    /// The stack the texture is on, kept alive with it natively.
    #[cfg(not(target_arch = "wasm32"))]
    gpu: Option<Arc<SharedGpu>>,
}

impl ImageTexture {
    #[cfg_attr(
        target_arch = "wasm32",
        expect(clippy::unused_self, reason = "natively the image holds its stack")
    )]
    fn gpu(&self) -> Gpu<'_> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.gpu.as_ref()
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
    }
}

impl Drop for ImageTexture {
    fn drop(&mut self) {
        issue(self.gpu(), None, Op::Drop(self.id));
    }
}

/// The stack an op is applied on natively; always `None` on wasm32, where
/// ops are queued for the Render Worker's.
type Gpu<'a> = Option<&'a Arc<SharedGpu>>;

impl SurfaceImage {
    #[must_use]
    pub(crate) fn width(&self) -> u32 {
        self.0.width
    }

    #[must_use]
    pub(crate) fn height(&self) -> u32 {
        self.0.height
    }

    /// What a draw of this image names it by.
    #[must_use]
    pub(crate) fn texture(&self) -> Texture {
        Texture::with_id(self.0.id, self.0.width, self.0.height)
    }

    /// A new image `width` × `height`, transparent black but for the
    /// pixels of this one from `(x, y)`, placed at the origin: the part of
    /// that rectangle inside this image.
    #[must_use]
    pub(crate) fn crop(&self, x: i64, y: i64, width: u32, height: u32) -> Self {
        copy(
            self.0.gpu(),
            None,
            self.0.id,
            (self.0.width, self.0.height),
            (x, y),
            (width, height),
        )
    }

    /// A new image `width` × `height`, transparent black but for the
    /// pixels `source` names from `(x, y)`, the part of that rectangle inside
    /// it, as of every op issued so far: what a canvas's current pixels
    /// ([`Surface::texture`]) are kept as.
    #[must_use]
    pub(crate) fn copy(source: Texture, x: i64, y: i64, width: u32, height: u32) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let gpu = super::gpu::live();
        #[cfg(target_arch = "wasm32")]
        let gpu: Option<Arc<SharedGpu>> = None;
        copy(
            gpu.as_ref(),
            None,
            source.id(),
            source.size(),
            (x, y),
            (width, height),
        )
    }

    /// Reads the image's pixels, premultiplied RGBA8, row-major, tightly
    /// packed, waiting for them.
    ///
    /// # Errors
    ///
    /// [`SurfaceError`] when the readback failed or no device answered.
    pub(crate) fn read(&self) -> Result<Vec<u8>, SurfaceError> {
        let size = (self.0.width, self.0.height);
        read(self.0.gpu(), None, self.0.id, size, (0, 0), size).wait()
    }
}

/// A pending readback: the pixels a read asked for, once the device has
/// answered. On wasm32 the Render Worker answers it in a later turn of its
/// own.
#[derive(Debug)]
pub(crate) struct PendingRead(flume::Receiver<Result<Vec<u8>, String>>);

impl PendingRead {
    /// Waits for the pixels: premultiplied RGBA8, row-major, tightly packed.
    ///
    /// # Errors
    ///
    /// [`SurfaceError`] when the readback failed, or the op was dropped
    /// unapplied (no device, or the device's queue was closed).
    pub(crate) fn wait(self) -> Result<Vec<u8>, SurfaceError> {
        self.0
            .recv()
            .map_err(|_| SurfaceError("no GPU device answered the readback".to_owned()))?
            .map_err(SurfaceError)
    }

    /// [`Self::wait`] for a caller that waits asynchronously.
    ///
    /// # Errors
    ///
    /// As [`Self::wait`].
    pub(crate) async fn wait_async(self) -> Result<Vec<u8>, SurfaceError> {
        self.0
            .recv_async()
            .await
            .map_err(|_| SurfaceError("no GPU device answered the readback".to_owned()))?
            .map_err(SurfaceError)
    }
}

impl Surface {
    /// A `width` × `height` surface; `None` for a zero side or one longer
    /// than [`MAX_SURFACE_SIDE`]. Nothing is allocated until the first op.
    #[must_use]
    pub(crate) fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 || width > MAX_SURFACE_SIDE || height > MAX_SURFACE_SIDE {
            return None;
        }
        Some(Self {
            id: next_texture_id(),
            width,
            height,
            #[cfg(not(target_arch = "wasm32"))]
            backend: None,
        })
    }

    /// What a draw of this surface's current pixels names it by: as they are
    /// when a list drawing it is replayed, so a caller that wants them as of
    /// now replays that list before it draws into this surface again.
    #[must_use]
    pub(crate) fn texture(&self) -> Texture {
        Texture::with_id(self.id, self.width, self.height)
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The stack and replay state ops are applied with natively, made at
    /// the first call; `(None, None)` on wasm32, and natively with no
    /// usable adapter.
    #[cfg(not(target_arch = "wasm32"))]
    fn port(&mut self) -> (Option<Arc<SharedGpu>>, Option<&mut SparseBackend>) {
        if self.backend.is_none()
            && let Ok(gpu) = SharedGpu::offscreen()
        {
            self.backend = Some(SparseBackend::new(&gpu));
        }
        let gpu = self
            .backend
            .as_ref()
            .map(|backend| Arc::clone(backend.gpu()));
        (gpu, self.backend.as_mut())
    }

    #[cfg(target_arch = "wasm32")]
    #[expect(clippy::unused_self, reason = "natively it makes the surface's state")]
    fn port(&mut self) -> (Option<Arc<SharedGpu>>, Option<&mut SparseBackend>) {
        (None, None)
    }

    /// Applies or queues `op`.
    fn issue(&mut self, op: Op) {
        let (gpu, backend) = self.port();
        issue(gpu.as_ref(), backend, op);
    }

    /// Replays `list`, which leaves no clip or layer open, onto the surface
    /// as `init` says, and empties it; `keep` holds the images it draws,
    /// released once it has been replayed, and is emptied too.
    pub(crate) fn draw(
        &mut self,
        list: &mut DisplayList,
        init: Init,
        keep: &mut Vec<SurfaceImage>,
    ) {
        let (target, size) = (self.id, self.size());
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Replayed in place, so the list keeps its capacity.
            if let (Some(gpu), Some(backend)) = self.port() {
                replay(&gpu, backend, target, size, list, init);
            }
            list.reset();
            keep.clear();
        }
        #[cfg(target_arch = "wasm32")]
        self.issue(Op::Draw {
            target,
            size,
            list: Box::new(std::mem::take(list)),
            init,
            _keep: std::mem::take(keep),
        });
    }

    /// Writes `texels`, `extent` premultiplied RGBA8 pixels, tightly packed,
    /// at `origin`; the rectangle lies inside the surface.
    pub(crate) fn write(&mut self, origin: (u32, u32), extent: (u32, u32), texels: Vec<u8>) {
        self.issue(Op::Write {
            target: self.id,
            size: self.size(),
            origin,
            extent,
            texels,
        });
    }

    /// Reads the `extent` pixels at `origin`, a rectangle inside the
    /// surface, as of every op issued before this one.
    #[must_use]
    pub(crate) fn read(&mut self, origin: (u32, u32), extent: (u32, u32)) -> PendingRead {
        let (target, size) = (self.id, self.size());
        let (gpu, backend) = self.port();
        read(gpu.as_ref(), backend, target, size, origin, extent)
    }

    /// A copy of the surface's pixels as an image, for this module's tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn snapshot(&mut self) -> SurfaceImage {
        let (source, size) = (self.id, self.size());
        let (gpu, backend) = self.port();
        copy(gpu.as_ref(), backend, source, size, (0, 0), size)
    }

    /// Hands the surface's texture to a new image and leaves the surface
    /// transparent black: `transferToImageBitmap`.
    #[must_use]
    pub(crate) fn take(&mut self) -> SurfaceImage {
        let (from, size) = (self.id, self.size());
        let (gpu, backend) = self.port();
        let image = SurfaceImage(Arc::new(ImageTexture {
            id: next_texture_id(),
            width: size.0,
            height: size.1,
            #[cfg(not(target_arch = "wasm32"))]
            gpu: gpu.clone(),
        }));
        let op = Op::Move {
            from,
            to: image.0.id,
            size,
        };
        issue(gpu.as_ref(), backend, op);
        image
    }

    /// Makes the surface's pixels, as of every op issued before this one,
    /// what the placeholder `<canvas>` of `frames` shows: HTML's update of an
    /// `OffscreenCanvas`'s placeholder. They are copied into the texture the
    /// placeholder's picture draws from, which the link owns for the
    /// element's life, and only then is the new generation published, so a
    /// painter never draws a frame half copied. Answers what the commit owes;
    /// nothing once the element is freed.
    #[must_use = "a wake or a resize the commit owes is lost"]
    pub(crate) fn commit(&mut self, frames: &CanvasFrames) -> Committed {
        let Some(link) = frames.link() else {
            return Committed::default();
        };
        let (width, height) = self.size();
        let descriptor = ImageDescriptor::platform(width, height);
        let generation = link.next_generation();
        self.issue(Op::Commit {
            target: self.id,
            size: self.size(),
            picture: frames.picture(),
            owner: link.owner(),
            generation,
            descriptor,
        });
        link.publish(generation, descriptor)
    }
}

impl Drop for Surface {
    /// Releases the texture, which exists only if an op made it: a surface
    /// that never drew asks for no device here.
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(backend) = &self.backend {
            backend.gpu().raster.canvases().remove(&self.id);
        }
        #[cfg(target_arch = "wasm32")]
        issue(None, None, Op::Drop(self.id));
    }
}

/// Ends `owner`'s leases on the textures it holds: a placeholder's link
/// lets go of its picture's texture.
pub(crate) fn end_lease(owner: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    let gpu = super::gpu::live();
    #[cfg(target_arch = "wasm32")]
    let gpu: Option<Arc<SharedGpu>> = None;
    issue(gpu.as_ref(), None, Op::EndLease(owner));
}

fn read(
    gpu: Gpu<'_>,
    backend: Option<&mut SparseBackend>,
    target: u64,
    size: (u32, u32),
    origin: (u32, u32),
    extent: (u32, u32),
) -> PendingRead {
    let (reply, answer) = flume::bounded(1);
    issue(
        gpu,
        backend,
        Op::Read {
            target,
            size,
            origin,
            extent,
            reply,
        },
    );
    PendingRead(answer)
}

/// A new image `extent`, transparent black but for `source`'s pixels from
/// `at`, the part of that rectangle inside `source`.
fn copy(
    gpu: Gpu<'_>,
    backend: Option<&mut SparseBackend>,
    source: u64,
    source_size: (u32, u32),
    at: (i64, i64),
    extent: (u32, u32),
) -> SurfaceImage {
    let image = SurfaceImage(Arc::new(ImageTexture {
        id: next_texture_id(),
        width: extent.0,
        height: extent.1,
        #[cfg(not(target_arch = "wasm32"))]
        gpu: gpu.map(Arc::clone),
    }));
    let span = |at: i64, extent: u32, size: u32| {
        let start = at.clamp(0, i64::from(size));
        let end = (at + i64::from(extent)).clamp(0, i64::from(size));
        let from = u32::try_from(start).unwrap_or(0);
        let to = u32::try_from(start - at).unwrap_or(0);
        (from, to, u32::try_from(end - start).unwrap_or(0))
    };
    let (fx, tx, w) = span(at.0, extent.0, source_size.0);
    let (fy, ty, h) = span(at.1, extent.1, source_size.1);
    issue(
        gpu,
        backend,
        Op::Copy {
            source,
            source_size,
            from: (fx, fy),
            to: (tx, ty),
            extent: (w, h),
            copy: image.0.id,
            copy_size: extent,
        },
    );
    image
}

/// Applies `op` on `gpu` now, natively, or queues it for the Render Worker
/// on wasm32. Natively, an op with no stack to apply it on has nothing to
/// change: no texture of its was ever made.
fn issue(gpu: Gpu<'_>, backend: Option<&mut SparseBackend>, op: Op) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(gpu) = gpu {
        apply(gpu, backend, op);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (gpu, backend);
        queue::push(op);
    }
}

/// The texture under `id`, made `size` and zero-filled if there is none.
fn texture(gpu: &SharedGpu, id: u64, size: (u32, u32)) -> wgpu::Texture {
    gpu.raster
        .canvases()
        .entry(id)
        .or_insert_with(|| CanvasTexture {
            texture: canvas_texture(gpu, size),
            spare: None,
        })
        .texture
        .clone()
}

/// What a canvas texture is used as: rendered into, sampled, copied from
/// and into.
const SURFACE_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::COPY_DST);

fn canvas_texture(gpu: &SharedGpu, (width, height): (u32, u32)) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terminus canvas"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: SURFACE_USAGE,
        view_formats: &[],
    })
}

fn view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The canvas textures `list` draws, by id, except `target`'s.
fn sources(gpu: &SharedGpu, list: &DisplayList, target: u64) -> FxHashMap<u64, wgpu::TextureView> {
    let mut sources = FxHashMap::default();
    let paints = list
        .items()
        .iter()
        .filter_map(|item| match item {
            Item::Fill(draw) | Item::Stroke(draw, _) => Some(&draw.paint),
            _ => None,
        })
        .chain(list.runs().iter().map(|run| &run.paint));
    let canvases = gpu.raster.canvases();
    for paint in paints {
        if let Paint::Texture(paint) = paint {
            let id = paint.texture.id();
            if id != target
                && let Some(entry) = canvases.get(&id)
            {
                sources.entry(id).or_insert_with(|| view(&entry.texture));
            }
        }
    }
    sources
}

fn replay(
    gpu: &SharedGpu,
    backend: &mut SparseBackend,
    target: u64,
    size: (u32, u32),
    list: &mut DisplayList,
    init: Init,
) {
    let current = texture(gpu, target, size);
    let mut sources = sources(gpu, list, target);
    let result = if init == Init::Backdrop {
        let spare = gpu
            .raster
            .canvases()
            .get_mut(&target)
            .and_then(|entry| entry.spare.take())
            .unwrap_or_else(|| canvas_texture(gpu, size));
        sources.insert(target, view(&current));
        let result = backend.render_onto(
            list,
            &view(&spare),
            size,
            Some(Color::TRANSPARENT),
            &sources,
        );
        if let Some(entry) = gpu.raster.canvases().get_mut(&target) {
            entry.texture = spare;
            entry.spare = Some(current);
        }
        result
    } else {
        let clear = (init == Init::Clear).then_some(Color::TRANSPARENT);
        backend.render_onto(list, &view(&current), size, clear, &sources)
    };
    list.reset();
    // A failed render (an intermediate past the device's limits) leaves the
    // batch undrawn; a canvas has nobody to report it to.
    let _ = result;
}

/// Applies `op` to `gpu`'s canvas textures, replaying with `backend`.
fn apply(gpu: &SharedGpu, backend: Option<&mut SparseBackend>, op: Op) {
    match op {
        Op::Draw {
            target,
            size,
            mut list,
            init,
            _keep,
        } => {
            if let Some(backend) = backend {
                replay(gpu, backend, target, size, &mut list, init);
            }
        }
        Op::Write {
            target,
            size,
            origin,
            extent,
            texels,
        } => {
            let texture = texture(gpu, target, size);
            gpu.queue.write_texture(
                at(&texture, origin),
                &texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * extent.0),
                    rows_per_image: Some(extent.1),
                },
                extent3d(extent),
            );
        }
        Op::Read {
            target,
            size,
            origin,
            extent,
            reply,
        } => {
            let texture = texture(gpu, target, size);
            read_region(gpu, &texture, origin, extent, reply);
        }
        Op::Copy {
            source,
            source_size,
            from,
            to,
            extent,
            copy,
            copy_size,
        } => {
            let source = texture(gpu, source, source_size);
            let copied = texture(gpu, copy, copy_size);
            if extent.0 > 0 && extent.1 > 0 {
                let mut encoder = gpu
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                encoder.copy_texture_to_texture(
                    at(&source, from),
                    at(&copied, to),
                    extent3d(extent),
                );
                gpu.submit([encoder.finish()]);
            }
        }
        Op::Move { from, to, size } => {
            let mut canvases = gpu.raster.canvases();
            let moved = canvases
                .remove(&from)
                .map_or_else(|| canvas_texture(gpu, size), |entry| entry.texture);
            canvases.insert(
                to,
                CanvasTexture {
                    texture: moved,
                    spare: None,
                },
            );
        }
        Op::Commit {
            target,
            size,
            picture,
            owner,
            generation,
            descriptor,
        } => {
            let source = texture(gpu, target, size);
            gpu.raster
                .copy_to_picture(gpu, owner, picture, generation, descriptor, &source);
        }
        Op::Drop(id) => {
            gpu.raster.canvases().remove(&id);
        }
        Op::EndLease(owner) => gpu.raster.end_leases(owner),
    }
}

fn extent3d((width, height): (u32, u32)) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

fn at(texture: &wgpu::Texture, (x, y): (u32, u32)) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin: wgpu::Origin3d { x, y, z: 0 },
        aspect: wgpu::TextureAspect::All,
    }
}

/// Copies the `extent` pixels of `texture` at `origin` into a staging
/// buffer and sends them, tightly packed, on `reply` once mapped: natively
/// before this returns, on wasm32 from the map's completion on this Worker.
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        clippy::needless_pass_by_value,
        reason = "on wasm32 the reply moves into the map's completion"
    )
)]
fn read_region(
    gpu: &SharedGpu,
    texture: &wgpu::Texture,
    origin: (u32, u32),
    extent: (u32, u32),
    reply: flume::Sender<Result<Vec<u8>, String>>,
) {
    let (width, height) = extent;
    let tight = width as usize * 4;
    let padded = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("terminus canvas readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terminus canvas readback copy"),
        });
    encoder.copy_texture_to_buffer(
        at(texture, origin),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: None,
            },
        },
        extent3d(extent),
    );
    gpu.submit([encoder.finish()]);
    let mapped = staging.clone();
    let rows = move || -> Result<Vec<u8>, String> {
        let view = mapped
            .slice(..)
            .get_mapped_range()
            .map_err(|error| error.to_string())?;
        let mut pixels = Vec::with_capacity(tight * height as usize);
        for row in view.chunks_exact(padded as usize) {
            pixels.extend_from_slice(&row[..tight]);
        }
        Ok(pixels)
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (mapped_tx, mapped_rx) = flume::bounded(1);
        staging.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = mapped_tx.send(result);
        });
        let answer = gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())
            .and_then(|_| {
                mapped_rx
                    .recv()
                    .map_err(|_| "readback map callback dropped".to_owned())?
                    .map_err(|error| error.to_string())
            })
            .and_then(|()| rows());
        let _ = reply.send(answer);
    }
    #[cfg(target_arch = "wasm32")]
    staging.map_async(wgpu::MapMode::Read, .., move |result| {
        let answer = result
            .map_err(|error| error.to_string())
            .and_then(|()| rows());
        let _ = reply.send(answer);
    });
}

/// Sets what wakes the Render Worker when an op is queued for it: the
/// thread whose turns call [`drain`].
#[cfg(target_arch = "wasm32")]
pub fn set_waker(waker: Arc<dyn Fn() + Send + Sync>) {
    queue::set_waker(waker);
}

/// Applies every op queued so far, in order, on this thread's device; the
/// Render Worker calls it at the start of every turn. With no live device
/// here the ops are dropped unapplied, which fails the reads among them.
#[cfg(target_arch = "wasm32")]
pub fn drain() {
    queue::drain();
}

#[cfg(target_arch = "wasm32")]
mod queue {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex, PoisonError};

    use super::{Op, SparseBackend, apply};

    struct Queue {
        ops: Vec<Op>,
        waker: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    /// Shared by every Worker: wasm32 statics live in the one shared
    /// memory.
    static QUEUE: Mutex<Queue> = Mutex::new(Queue {
        ops: Vec::new(),
        waker: None,
    });

    thread_local! {
        /// The Render Worker's replay state for every canvas.
        static BACKEND: RefCell<Option<SparseBackend>> = const { RefCell::new(None) };
    }

    fn lock() -> std::sync::MutexGuard<'static, Queue> {
        QUEUE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn set_waker(waker: Arc<dyn Fn() + Send + Sync>) {
        lock().waker = Some(waker);
    }

    /// Queues `op`, waking the Render Worker when the queue was empty.
    pub(super) fn push(op: Op) {
        let waker = {
            let mut queue = lock();
            let first = queue.ops.is_empty();
            queue.ops.push(op);
            first.then(|| queue.waker.clone()).flatten()
        };
        if let Some(waker) = waker {
            waker();
        }
    }

    pub(super) fn drain() {
        loop {
            let ops = std::mem::take(&mut lock().ops);
            if ops.is_empty() {
                return;
            }
            let Some(gpu) = super::super::gpu::live() else {
                // Dropping the ops drops the reads' reply senders.
                continue;
            };
            BACKEND.with(|backend| {
                let mut backend = backend.borrow_mut();
                if !backend
                    .as_ref()
                    .is_some_and(|backend| Arc::ptr_eq(backend.gpu(), &gpu))
                {
                    *backend = Some(SparseBackend::new(&gpu));
                }
                for op in ops {
                    apply(&gpu, backend.as_mut(), op);
                }
            });
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{Init, Surface};
    use crate::kurbo::{Affine, Rect};
    use crate::paint::display_list::{DisplayList, ListPaint};
    use crate::peniko::{Color, Extend, Fill, ImageQuality, ImageSampler};

    const RED: [u8; 4] = [255, 0, 0, 255];
    const CLEAR: [u8; 4] = [0; 4];

    fn pixel(surface: &mut Surface, x: u32, y: u32) -> [u8; 4] {
        let read = surface.read((x, y), (1, 1)).wait().expect("the read");
        [read[0], read[1], read[2], read[3]]
    }

    fn fill(list: &mut DisplayList, rect: Rect, paint: &ListPaint) {
        list.fill_with(Fill::NonZero, Affine::IDENTITY, paint, None, &rect);
    }

    /// A draw lands on what the surface holds; a backdrop draw samples the
    /// surface itself; writes, copies and a take keep their order.
    #[test]
    fn a_surface_replays_writes_reads_and_hands_over_its_texture() {
        let mut surface = Surface::new(8, 4).expect("a side the device takes");
        assert_eq!(pixel(&mut surface, 0, 0), CLEAR, "zero-filled");
        let mut list = DisplayList::new();
        let red = ListPaint::from(Color::from_rgba8(255, 0, 0, 255));
        fill(&mut list, Rect::new(0.0, 0.0, 4.0, 4.0), &red);
        surface.draw(&mut list, Init::Load, &mut Vec::new());
        assert!(list.is_empty(), "the flush empties the list");
        assert_eq!(pixel(&mut surface, 1, 1), RED);
        assert_eq!(pixel(&mut surface, 5, 1), CLEAR);

        // The left half drawn again into the right half, from the surface.
        let own = ListPaint::texture(
            surface.texture(),
            ImageSampler::new()
                .with_extend(Extend::Pad)
                .with_quality(ImageQuality::Low),
        );
        fill(&mut list, Rect::new(0.0, 0.0, 8.0, 4.0), &own);
        list.fill_with(
            Fill::NonZero,
            Affine::IDENTITY,
            &own,
            Some(Affine::translate((4.0, 0.0))),
            &Rect::new(4.0, 0.0, 8.0, 4.0),
        );
        surface.draw(&mut list, Init::Backdrop, &mut Vec::new());
        assert_eq!(pixel(&mut surface, 1, 1), RED);
        assert_eq!(pixel(&mut surface, 5, 1), RED);

        surface.write((0, 0), (1, 1), vec![0, 0, 128, 128]);
        assert_eq!(pixel(&mut surface, 0, 0), [0, 0, 128, 128]);
        let crop = surface.snapshot().crop(-1, 0, 2, 1);
        assert_eq!(
            crop.read().expect("the read"),
            [CLEAR, [0, 0, 128, 128]].concat()
        );

        let taken = surface.take();
        assert_eq!(pixel(&mut surface, 5, 1), CLEAR, "a fresh texture");
        assert_eq!(taken.read().expect("the read")[5 * 4..6 * 4], RED);

        fill(&mut list, Rect::new(0.0, 0.0, 1.0, 1.0), &red);
        surface.draw(&mut list, Init::Clear, &mut Vec::new());
        assert_eq!(pixel(&mut surface, 0, 0), RED);
    }
}
