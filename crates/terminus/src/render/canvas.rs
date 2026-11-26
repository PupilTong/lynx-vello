//! Canvas surfaces: a `<canvas>` or `OffscreenCanvas` bitmap as a GPU
//! texture its owner draws into by replaying [`DisplayList`]s, and the
//! textures canvas images hold.
//!
//! The owner records draws into a list on its own thread and hands the list
//! over at each flush ([`Surface::draw`]); nothing is drawn while it
//! records. The flush replays the list into a `vello_gpu` scene on the
//! owner's thread, with no device ([`encode`], the CPU half of a render:
//! every path flattened and turned into strips). A bitmap the list draws is
//! drawn in the scene under a texture id reserved for it, and a texture by
//! its own id. Every GPU step is one [`Op`] applied by [`apply`] to the
//! device's canvas texture table ([`Textures`], held by the [`SharedGpu`]),
//! keyed by a texture id that is also the id a draw names the texture by
//! ([`Texture`]); a draw's op carries the encoded scene, and its GPU half
//! ([`Submitter::submit`]) resolves each bitmap through the bitmap table,
//! uploading it on a miss, binds it and the canvas textures the scene
//! names, and renders. The scene goes back to its surface through the op's
//! channel, and the surface's next draw reuses its buffers; a draw issued
//! while it is away makes another.
//!
//! The replay's state — glifo's glyph outline cache and the replay's
//! scratch buffers — is the encoding thread's, not a surface's: every
//! surface that encodes on a thread uses that thread's one [`Replay`], so
//! two canvases drawing the same text keep its outlines once. It is
//! maintained once per encode, so a glyph is dropped after 64 encodes on
//! the thread that did not draw it, whichever surfaces they were for. The
//! state is kept while a [`ReplayUser`] is alive on the thread and dropped
//! with the last one; an encode while none is alive keeps nothing. It is
//! never the page painter's: each [`super::sparse::SparseBackend`] keeps
//! its own.
//!
//! - **Natively** the owner's thread applies each op as it is issued, with the surface's own
//!   [`Submitter`], so a draw's two halves run back to back there: the shared renderer is locked
//!   for the render and submit, and a read blocks on the readback.
//! - **On wasm32** the device lives on the Render Worker alone. An op is appended to one queue in
//!   memory every Worker shares and the Render Worker is woken ([`set_waker`]); it applies the
//!   queue in order at the start of each turn ([`drain`]), with one [`Submitter`] for every canvas,
//!   so it runs a draw's GPU half only. A read's answer is sent from the readback's completion on
//!   that Worker, and the caller blocks on it.
//!
//! A paint whose pixels only the device side can find missing — a canvas
//! texture id with no texture (a surface that never issued an op), a bitmap
//! with no upload whose lent pixels are gone — is bound as a 1 × 1
//! transparent black texture ([`transparent_view`]), which composites as
//! the draw left out of the scene does under every composite operation.
//!
//! A surface holds one texture, created at the first op that needs it and
//! zero-filled, and a second one once a draw has read its own pixels
//! ([`Init::Backdrop`]): `vello_gpu` composites a layer that blends by
//! anything but source-over into a transparent intermediate when it sits at
//! the root (`vello_common::record::CommandRecorder::root_is_blend_target`,
//! `vello_gpu` `schedule/mod.rs` `schedule_root`), and refuses to sample
//! the texture it renders into (`RenderError::TextureFeedbackLoop`). Such a
//! batch is rendered into the second texture over a clear, with the first
//! sampled under the surface's id, and the two change places once the
//! render has succeeded.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rustc_hash::FxHashMap;
use vello_gpu::Scene;

use super::gpu::SharedGpu;
use super::sparse::Replay;
pub(crate) use super::sparse::canvas::next_owner;
use super::sparse::canvas::{Encoded, Submitter, encode, transparent_view};
use crate::html::canvas::{CanvasFrames, Committed};
use crate::paint::display_list::{DisplayList, Texture, next_texture_id};
use crate::render::image::ImageDescriptor;
use crate::render::picture::PictureId;

pub(crate) const MAX_SURFACE_SIDE: u32 = SharedGpu::LIMITS.max_texture_dimension_2d;

#[derive(Default)]
pub(crate) struct Textures(Mutex<FxHashMap<u64, CanvasTexture>>);

impl Textures {
    fn lock(&self) -> MutexGuard<'_, FxHashMap<u64, CanvasTexture>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct CanvasTexture {
    texture: wgpu::Texture,
    spare: Option<wgpu::Texture>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Init {
    Load,
    Clear,
    Backdrop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceError(pub(crate) String);

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SurfaceError {}

enum Op {
    Draw(Box<Draw>),
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
    Copy {
        source: u64,
        from: (u32, u32),
        to: (u32, u32),
        extent: (u32, u32),
        copy: u64,
        copy_size: (u32, u32),
    },
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

struct Draw {
    target: u64,
    size: (u32, u32),
    init: Init,
    encoded: Encoded,
    _keep: Vec<SurfaceImage>,
    back: flume::Sender<Scene>,
}

const _: () = {
    const fn send<T: Send>() {}
    send::<Scene>();
    send::<Op>();
};

struct ThreadReplay {
    users: usize,
    replay: Option<Replay>,
}

thread_local! {
    static REPLAY: RefCell<ThreadReplay> = const {
        RefCell::new(ThreadReplay {
            users: 0,
            replay: None,
        })
    };
}

#[derive(Debug)]
pub struct ReplayUser(PhantomData<*const ()>);

impl ReplayUser {
    #[must_use]
    pub fn new() -> Self {
        REPLAY.with_borrow_mut(|thread| thread.users += 1);
        Self(PhantomData)
    }
}

impl Default for ReplayUser {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ReplayUser {
    fn drop(&mut self) {
        let _ = REPLAY.try_with(|thread| {
            let mut thread = thread.borrow_mut();
            thread.users -= 1;
            if thread.users == 0 {
                thread.replay = None;
            }
        });
    }
}

struct Encoder {
    back: flume::Sender<Scene>,
    returned: flume::Receiver<Scene>,
}

impl Encoder {
    fn new() -> Self {
        let (back, returned) = flume::unbounded();
        Self { back, returned }
    }

    fn scene(&self) -> Scene {
        let mut back = self.returned.try_iter();
        let scene = back.next().unwrap_or_else(|| Scene::new(1, 1));
        back.for_each(drop);
        scene
    }
}

pub(crate) struct Surface {
    id: u64,
    width: u32,
    height: u32,
    encoder: Option<Encoder>,
    #[cfg(not(target_arch = "wasm32"))]
    port: Option<Submitter>,
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

#[derive(Clone, Debug)]
pub(crate) struct SurfaceImage(Arc<ImageTexture>);

#[derive(Debug)]
struct ImageTexture {
    id: u64,
    width: u32,
    height: u32,
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

    #[must_use]
    pub(crate) fn texture(&self) -> Texture {
        Texture::with_id(self.0.id, self.0.width, self.0.height)
    }

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

    pub(crate) fn read(&self) -> Result<Vec<u8>, SurfaceError> {
        let size = (self.0.width, self.0.height);
        read(self.0.gpu(), None, self.0.id, size, (0, 0), size).wait()
    }
}

#[derive(Debug)]
pub(crate) struct PendingRead(flume::Receiver<Result<Vec<u8>, String>>);

impl PendingRead {
    pub(crate) fn wait(self) -> Result<Vec<u8>, SurfaceError> {
        self.0
            .recv()
            .map_err(|_| SurfaceError("no GPU device answered the readback".to_owned()))?
            .map_err(SurfaceError)
    }

    pub(crate) async fn wait_async(self) -> Result<Vec<u8>, SurfaceError> {
        self.0
            .recv_async()
            .await
            .map_err(|_| SurfaceError("no GPU device answered the readback".to_owned()))?
            .map_err(SurfaceError)
    }
}

impl Surface {
    #[must_use]
    pub(crate) fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 || width > MAX_SURFACE_SIDE || height > MAX_SURFACE_SIDE {
            return None;
        }
        Some(Self {
            id: next_texture_id(),
            width,
            height,
            encoder: None,
            #[cfg(not(target_arch = "wasm32"))]
            port: None,
        })
    }

    #[cfg(test)]
    fn on(gpu: &Arc<SharedGpu>, width: u32, height: u32) -> Self {
        let mut surface = Self::new(width, height).expect("a side the device takes");
        surface.port = Some(Submitter::new(gpu));
        surface
    }

    #[must_use]
    pub(crate) fn texture(&self) -> Texture {
        Texture::with_id(self.id, self.width, self.height)
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn port(&mut self) -> (Option<Arc<SharedGpu>>, Option<&mut Submitter>) {
        if self.port.is_none()
            && let Ok(gpu) = SharedGpu::offscreen()
        {
            self.port = Some(Submitter::new(&gpu));
        }
        let gpu = self.port.as_ref().map(|port| Arc::clone(port.gpu()));
        (gpu, self.port.as_mut())
    }

    #[cfg(target_arch = "wasm32")]
    #[expect(clippy::unused_self, reason = "natively it makes the surface's state")]
    fn port(&mut self) -> (Option<Arc<SharedGpu>>, Option<&mut Submitter>) {
        (None, None)
    }

    fn issue(&mut self, op: Op) {
        let (gpu, port) = self.port();
        issue(gpu.as_ref(), port, op);
    }

    pub(crate) fn draw(
        &mut self,
        list: &mut DisplayList,
        init: Init,
        keep: &mut Vec<SurfaceImage>,
    ) {
        let draw = self.encode(list, init, keep);
        self.issue(Op::Draw(Box::new(draw)));
    }

    fn encode(&mut self, list: &mut DisplayList, init: Init, keep: &mut Vec<SurfaceImage>) -> Draw {
        let (target, size) = (self.id, self.size());
        let state = self.encoder.get_or_insert_with(Encoder::new);
        let clear = init != Init::Load;
        let encoded = REPLAY.with_borrow_mut(|thread| {
            let replay = thread.replay.get_or_insert_with(Replay::default);
            let encoded = encode(state.scene(), replay, list, size, clear, &|id| {
                id != target || init == Init::Backdrop
            });
            if thread.users == 0 {
                thread.replay = None;
            }
            encoded
        });
        list.reset();
        Draw {
            target,
            size,
            init,
            encoded,
            _keep: std::mem::take(keep),
            back: state.back.clone(),
        }
    }

    pub(crate) fn write(&mut self, origin: (u32, u32), extent: (u32, u32), texels: Vec<u8>) {
        self.issue(Op::Write {
            target: self.id,
            size: self.size(),
            origin,
            extent,
            texels,
        });
    }

    #[must_use]
    pub(crate) fn read(&mut self, origin: (u32, u32), extent: (u32, u32)) -> PendingRead {
        let (target, size) = (self.id, self.size());
        let (gpu, port) = self.port();
        read(gpu.as_ref(), port, target, size, origin, extent)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn snapshot(&mut self) -> SurfaceImage {
        let (source, size) = (self.id, self.size());
        let (gpu, port) = self.port();
        copy(gpu.as_ref(), port, source, size, (0, 0), size)
    }

    #[must_use]
    pub(crate) fn take(&mut self) -> SurfaceImage {
        let (from, size) = (self.id, self.size());
        let (gpu, port) = self.port();
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
        issue(gpu.as_ref(), port, op);
        image
    }

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
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(port) = &self.port {
            port.gpu().canvases.lock().remove(&self.id);
        }
        #[cfg(target_arch = "wasm32")]
        issue(None, None, Op::Drop(self.id));
    }
}

pub(crate) fn end_lease(owner: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    let gpu = super::gpu::live();
    #[cfg(target_arch = "wasm32")]
    let gpu: Option<Arc<SharedGpu>> = None;
    issue(gpu.as_ref(), None, Op::EndLease(owner));
}

fn read(
    gpu: Gpu<'_>,
    port: Option<&mut Submitter>,
    target: u64,
    size: (u32, u32),
    origin: (u32, u32),
    extent: (u32, u32),
) -> PendingRead {
    let (reply, answer) = flume::bounded(1);
    issue(
        gpu,
        port,
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

fn copy(
    gpu: Gpu<'_>,
    port: Option<&mut Submitter>,
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
        let end = at
            .saturating_add(i64::from(extent))
            .clamp(0, i64::from(size));
        let from = u32::try_from(start).unwrap_or(0);
        let to = u32::try_from(start.saturating_sub(at)).unwrap_or(0);
        (from, to, u32::try_from(end - start).unwrap_or(0))
    };
    let (fx, tx, w) = span(at.0, extent.0, source_size.0);
    let (fy, ty, h) = span(at.1, extent.1, source_size.1);
    issue(
        gpu,
        port,
        Op::Copy {
            source,
            from: (fx, fy),
            to: (tx, ty),
            extent: (w, h),
            copy: image.0.id,
            copy_size: extent,
        },
    );
    image
}

fn issue(gpu: Gpu<'_>, port: Option<&mut Submitter>, op: Op) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(gpu) = gpu {
        apply(gpu, port, op);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (gpu, port);
        queue::push(op);
    }
}

fn texture(gpu: &SharedGpu, id: u64, size: (u32, u32)) -> wgpu::Texture {
    gpu.canvases
        .lock()
        .entry(id)
        .or_insert_with(|| CanvasTexture {
            texture: canvas_texture(gpu, size),
            spare: None,
        })
        .texture
        .clone()
}

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

fn sources(gpu: &SharedGpu, encoded: &Encoded, target: u64) -> FxHashMap<u64, wgpu::TextureView> {
    let mut sources = FxHashMap::default();
    let mut transparent = None;
    let canvases = gpu.canvases.lock();
    for &id in encoded.textures() {
        if id == target {
            continue;
        }
        let view = canvases.get(&id).map_or_else(
            || {
                transparent
                    .get_or_insert_with(|| transparent_view(&gpu.device))
                    .clone()
            },
            |entry| view(&entry.texture),
        );
        sources.insert(id, view);
    }
    sources
}

fn render(gpu: &SharedGpu, port: &mut Submitter, draw: &Draw) {
    let Draw {
        target,
        size,
        init,
        encoded,
        ..
    } = draw;
    let (target, size) = (*target, *size);
    let current = texture(gpu, target, size);
    let mut sources = sources(gpu, encoded, target);
    let result = if *init == Init::Backdrop {
        let spare = gpu
            .canvases
            .lock()
            .get_mut(&target)
            .and_then(|entry| entry.spare.take())
            .unwrap_or_else(|| canvas_texture(gpu, size));
        sources.insert(target, view(&current));
        let result = port.submit(encoded, &view(&spare), &sources);
        if let Some(entry) = gpu.canvases.lock().get_mut(&target) {
            if result.is_ok() {
                entry.texture = spare;
                entry.spare = Some(current);
            } else {
                entry.spare = Some(spare);
            }
        }
        result
    } else {
        port.submit(encoded, &view(&current), &sources)
    };
    let _ = result;
}

fn apply(gpu: &SharedGpu, port: Option<&mut Submitter>, op: Op) {
    match op {
        Op::Draw(draw) => {
            if let Some(port) = port {
                render(gpu, port, &draw);
            }
            let Draw { encoded, back, .. } = *draw;
            let _ = back.send(encoded.into_scene());
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
            from,
            to,
            extent,
            copy,
            copy_size,
        } => {
            let source = gpu
                .canvases
                .lock()
                .get(&source)
                .map(|entry| entry.texture.clone());
            let copied = texture(gpu, copy, copy_size);
            if let Some(source) = source
                && extent.0 > 0
                && extent.1 > 0
            {
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
            let mut canvases = gpu.canvases.lock();
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
            gpu.canvases.lock().remove(&id);
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

#[cfg(target_arch = "wasm32")]
pub fn set_waker(waker: Arc<dyn Fn() + Send + Sync>) {
    queue::set_waker(waker);
}

#[cfg(target_arch = "wasm32")]
pub fn drain() {
    queue::drain();
}

#[cfg(target_arch = "wasm32")]
mod queue {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex, PoisonError};

    use super::{Op, Submitter, apply};

    struct Queue {
        ops: Vec<Op>,
        waker: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    static QUEUE: Mutex<Queue> = Mutex::new(Queue {
        ops: Vec::new(),
        waker: None,
    });

    thread_local! {
        static PORT: RefCell<Option<Submitter>> = const { RefCell::new(None) };
    }

    fn lock() -> std::sync::MutexGuard<'static, Queue> {
        QUEUE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn set_waker(waker: Arc<dyn Fn() + Send + Sync>) {
        lock().waker = Some(waker);
    }

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
                continue;
            };
            PORT.with(|port| {
                let mut port = port.borrow_mut();
                if !port
                    .as_ref()
                    .is_some_and(|port| Arc::ptr_eq(port.gpu(), &gpu))
                {
                    *port = Some(Submitter::new(&gpu));
                }
                for op in ops {
                    apply(&gpu, port.as_mut(), op);
                }
            });
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use seldon::text::{FontBlob, TextContext};

    use super::{Init, Op, Replay, ReplayUser, Surface, SurfaceImage};
    use crate::html::canvas::{
        CompositeOp, OffscreenCanvas, ResolvedFamily, ResolvedFont, ResolvedFontStyle,
    };
    use crate::kurbo::{Affine, Rect};
    use crate::paint::display_list::{DisplayList, ImagePaint, ImageRef, Paint};
    use crate::peniko::{
        Blob, Brush, Color, Extend, Fill, Gradient, ImageAlphaType, ImageData, ImageFormat,
        ImageQuality, ImageSampler,
    };
    use crate::render::gpu::SharedGpu;
    use crate::render::sparse::{BitmapLeases, SharedRaster};

    const RED: [u8; 4] = [255, 0, 0, 255];
    const CLEAR: [u8; 4] = [0; 4];

    fn pixel(surface: &mut Surface, x: u32, y: u32) -> [u8; 4] {
        let read = surface.read((x, y), (1, 1)).wait().expect("the read");
        [read[0], read[1], read[2], read[3]]
    }

    fn fill(list: &mut DisplayList, rect: Rect, paint: &Paint) {
        list.fill_paint(Fill::NonZero, Affine::IDENTITY, paint.clone(), None, &rect);
    }

    fn image_paint(image: ImageRef, sampler: ImageSampler) -> Paint {
        Paint::Image(Arc::new(ImagePaint { image, sampler }))
    }

    #[test]
    fn a_surface_replays_writes_reads_and_hands_over_its_texture() {
        let mut surface = Surface::new(8, 4).expect("a side the device takes");
        assert_eq!(pixel(&mut surface, 0, 0), CLEAR, "zero-filled");
        let mut list = DisplayList::new();
        let red = Paint::Solid(Color::from_rgba8(255, 0, 0, 255));
        fill(&mut list, Rect::new(0.0, 0.0, 4.0, 4.0), &red);
        surface.draw(&mut list, Init::Load, &mut Vec::new());
        assert!(list.is_empty(), "the flush empties the list");
        assert_eq!(pixel(&mut surface, 1, 1), RED);
        assert_eq!(pixel(&mut surface, 5, 1), CLEAR);

        let own = Paint::texture(
            surface.texture(),
            ImageSampler::new()
                .with_extend(Extend::Pad)
                .with_quality(ImageQuality::Low),
        );
        fill(&mut list, Rect::new(0.0, 0.0, 8.0, 4.0), &own);
        list.fill_paint(
            Fill::NonZero,
            Affine::IDENTITY,
            own.clone(),
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

    #[test]
    fn a_copy_of_a_surface_that_never_drew_leaves_no_texture() {
        let gpu = SharedGpu::offscreen().expect("a GPU adapter");
        let fresh = Surface::new(8, 4).expect("a side the device takes");
        let (id, size) = (fresh.texture().id(), fresh.size());
        let copy = SurfaceImage::copy(fresh.texture(), 0, 0, size.0, size.1);
        let copy_id = copy.texture().id();
        assert_eq!(copy.read().expect("the read"), vec![0; 8 * 4 * 4]);
        drop(copy);
        drop(fresh);
        let canvases = gpu.canvases.lock();
        assert!(!canvases.contains_key(&id), "the source's texture");
        assert!(!canvases.contains_key(&copy_id), "the copy's texture");
    }

    #[test]
    fn a_failed_backdrop_render_keeps_the_content() {
        let mut surface = Surface::new(8, 4).expect("a side the device takes");
        let mut list = DisplayList::new();
        let red = Paint::Solid(Color::from_rgba8(255, 0, 0, 255));
        fill(&mut list, Rect::new(0.0, 0.0, 8.0, 4.0), &red);
        surface.draw(&mut list, Init::Load, &mut Vec::new());
        let own = Paint::texture(
            surface.texture(),
            ImageSampler::new()
                .with_extend(Extend::Pad)
                .with_quality(ImageQuality::Low),
        );
        fill(&mut list, Rect::new(0.0, 0.0, 8.0, 4.0), &own);
        list.push_drop_shadow(0.0, 0.0, 10_000.0, Color::from_rgba8(0, 0, 255, 255));
        fill(&mut list, Rect::new(2.0, 1.0, 4.0, 3.0), &red);
        list.pop_layer();
        surface.draw(&mut list, Init::Backdrop, &mut Vec::new());
        assert_eq!(pixel(&mut surface, 1, 1), RED);
        assert_eq!(pixel(&mut surface, 7, 3), RED);
    }

    const W: u32 = 16;
    const H: u32 = 8;

    fn pixels(surface: &mut Surface) -> Vec<u8> {
        let size = surface.size();
        surface.read((0, 0), size).wait().expect("the read")
    }

    fn bitmap(width: u32, texels: &[[u8; 4]]) -> ImageData {
        let bytes: Vec<u8> = texels.iter().flatten().copied().collect();
        ImageData {
            data: Blob::new(Arc::new(bytes)),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height: u32::try_from(texels.len()).expect("a small bitmap") / width,
        }
    }

    fn nearest() -> ImageSampler {
        ImageSampler::new()
            .with_extend(Extend::Pad)
            .with_quality(ImageQuality::Low)
    }

    fn fill_image(list: &mut DisplayList, rect: Rect, image: ImageRef, size: (u32, u32)) {
        let scale = Affine::scale_non_uniform(
            rect.width() / f64::from(size.0),
            rect.height() / f64::from(size.1),
        );
        list.fill_paint(
            Fill::NonZero,
            Affine::IDENTITY,
            image_paint(image, nearest()),
            Some(Affine::translate(rect.origin().to_vec2()) * scale),
            &rect,
        );
    }

    fn backdrop(list: &mut DisplayList) {
        let canvas = Rect::new(0.0, 0.0, f64::from(W), f64::from(H));
        let across = Gradient::new_linear((0.0, 0.0), (f64::from(W), 0.0)).with_stops([
            Color::from_rgba8(255, 40, 0, 30),
            Color::from_rgba8(0, 90, 255, 250),
        ]);
        let down = Gradient::new_linear((0.0, 0.0), (0.0, f64::from(H))).with_stops([
            Color::from_rgba8(20, 200, 60, 200),
            Color::from_rgba8(200, 20, 160, 10),
        ]);
        fill(list, canvas, &Paint::from(&Brush::Gradient(across)));
        fill(list, canvas, &Paint::from(&Brush::Gradient(down)));
    }

    #[test]
    fn a_draw_encoded_on_one_thread_renders_from_another() {
        let mut source = Surface::new(4, 4).expect("a side the device takes");
        let mut list = DisplayList::new();
        fill(
            &mut list,
            Rect::new(0.0, 0.0, 4.0, 2.0),
            &Paint::Solid(Color::from_rgba8(0, 128, 0, 255)),
        );
        source.draw(&mut list, Init::Load, &mut Vec::new());
        let image = bitmap(2, &[RED, [0, 0, 255, 128], [0, 255, 0, 255], [9, 9, 9, 9]]);
        let record = |list: &mut DisplayList| {
            backdrop(list);
            fill_image(
                list,
                Rect::new(1.0, 1.0, 9.0, 7.0),
                ImageRef::held(&image),
                (2, 2),
            );
            fill(
                list,
                Rect::new(10.0, 2.0, 14.0, 6.0),
                &Paint::texture(source.texture(), nearest()),
            );
        };

        let mut here = Surface::new(W, H).expect("a side the device takes");
        record(&mut list);
        here.draw(&mut list, Init::Clear, &mut Vec::new());
        let expected = pixels(&mut here);
        assert_ne!(expected, vec![0; expected.len()], "the list draws");

        record(&mut list);
        let surface = Surface::new(W, H).expect("a side the device takes");
        let (mut surface, draw) = std::thread::spawn(move || {
            let mut surface = surface;
            let draw = surface.encode(&mut list, Init::Clear, &mut Vec::new());
            (surface, draw)
        })
        .join()
        .expect("the encoding thread");
        surface.issue(Op::Draw(Box::new(draw)));
        assert_eq!(pixels(&mut surface), expected);
    }

    fn thread_outlines() -> Option<usize> {
        super::REPLAY.with_borrow(|thread| thread.replay.as_ref().map(Replay::cached_outlines))
    }

    fn draw_text(fonts: &mut TextContext, text: &str) -> OffscreenCanvas {
        let mut canvas = OffscreenCanvas::new(W, H);
        let mut context = canvas.context_2d();
        context.set_font(ResolvedFont {
            families: vec![ResolvedFamily::Named("Ahem".to_owned())],
            size: 8.0,
            weight: 400.0,
            style: ResolvedFontStyle::Normal,
            stretch: 1.0,
        });
        context.fill_text(fonts, text, 0.0, 6.0, None);
        canvas.flush();
        canvas
    }

    fn ahem() -> TextContext {
        const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");
        let mut fonts = TextContext::new();
        assert_eq!(fonts.register_fonts(FontBlob::from_static(AHEM)), 1);
        fonts
    }

    #[test]
    fn two_canvases_on_one_thread_share_the_glyph_outlines() {
        let mut fonts = ahem();
        let _user = ReplayUser::new();
        let _first = draw_text(&mut fonts, "X");
        let one = thread_outlines().expect("the thread's replay state");
        assert!(one > 0, "the text drew an outline");
        let _second = draw_text(&mut fonts, "X");
        assert_eq!(thread_outlines(), Some(one), "the outline is found again");
        let _third = draw_text(&mut fonts, "Xp");
        assert!(
            thread_outlines().is_some_and(|outlines| outlines > one),
            "a new glyph's outline joins the same cache"
        );
    }

    #[test]
    fn the_last_replay_user_drops_the_thread_replay_state() {
        let mut fonts = ahem();
        let _canvas = draw_text(&mut fonts, "X");
        assert_eq!(thread_outlines(), None, "no user: nothing kept");

        let first = ReplayUser::new();
        let second = ReplayUser::new();
        let _canvas = draw_text(&mut fonts, "X");
        assert!(thread_outlines().is_some_and(|outlines| outlines > 0));
        drop(first);
        assert!(
            thread_outlines().is_some_and(|outlines| outlines > 0),
            "a user is left"
        );
        drop(second);
        assert_eq!(thread_outlines(), None, "the last user is gone");
    }

    #[test]
    fn a_bitmap_is_bound_under_its_reserved_id_on_a_miss_and_on_a_hit() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let uploads = BitmapLeases::new(&gpu);
        let mut surface = Surface::on(&gpu, W, H);
        let image = bitmap(2, &[RED, [0, 0, 255, 255], [0, 255, 0, 255], [255; 4]]);
        let mut list = DisplayList::new();

        fill_image(
            &mut list,
            Rect::new(0.0, 0.0, 4.0, 4.0),
            ImageRef::held(&image),
            (2, 2),
        );
        surface.draw(&mut list, Init::Load, &mut Vec::new());
        assert_eq!(uploads.uploads(), 1, "a miss uploads");
        assert_eq!(pixel(&mut surface, 0, 0), RED);
        assert_eq!(pixel(&mut surface, 3, 3), [255; 4]);
        assert_eq!(pixel(&mut surface, 5, 1), CLEAR);

        fill_image(
            &mut list,
            Rect::new(4.0, 0.0, 8.0, 4.0),
            ImageRef::held(&image),
            (2, 2),
        );
        fill_image(
            &mut list,
            Rect::new(8.0, 4.0, 12.0, 8.0),
            ImageRef::held(&image),
            (2, 2),
        );
        surface.draw(&mut list, Init::Load, &mut Vec::new());
        assert_eq!(uploads.uploads(), 1, "a hit uploads nothing");
        assert_eq!(pixel(&mut surface, 4, 0), RED);
        assert_eq!(pixel(&mut surface, 6, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&mut surface, 8, 6), [0, 255, 0, 255]);
        assert_eq!(pixel(&mut surface, 11, 7), [255; 4]);
        assert_eq!(pixel(&mut surface, 0, 0), RED, "the first batch stays");
    }

    #[test]
    fn a_paint_with_no_pixels_composites_as_no_draw() {
        let never_drew = Surface::new(4, 4).expect("a side the device takes");
        let short = ImageData {
            data: Blob::new(Arc::new(vec![255; 8])),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 4,
            height: 4,
        };
        let missing: [(&str, Paint); 3] = [
            (
                "a canvas with no texture",
                Paint::texture(never_drew.texture(), nearest()),
            ),
            ("lent pixels that are gone", {
                let gone = bitmap(1, &[RED]);
                image_paint(ImageRef::lent(&gone), nearest())
            }),
            (
                "too few bytes",
                image_paint(ImageRef::held(&short), nearest()),
            ),
        ];
        let ops = [
            "source-over",
            "source-in",
            "source-out",
            "source-atop",
            "destination-over",
            "destination-in",
            "destination-out",
            "destination-atop",
            "lighter",
            "copy",
            "xor",
            "multiply",
            "screen",
            "overlay",
            "darken",
            "lighten",
            "color-dodge",
            "color-burn",
            "hard-light",
            "soft-light",
            "difference",
            "exclusion",
            "hue",
            "saturation",
            "color",
            "luminosity",
        ];
        let canvas = Rect::new(0.0, 0.0, f64::from(W), f64::from(H));
        let shape = Rect::new(3.0, 2.0, 11.0, 6.0);
        let mut list = DisplayList::new();
        for name in ops {
            let blend = CompositeOp::from_name(name).expect("an operation").blend();
            for shadow in [false, true] {
                let mut render = |draw: Option<&Paint>| {
                    let mut surface = Surface::new(W, H).expect("a side the device takes");
                    backdrop(&mut list);
                    surface.draw(&mut list, Init::Clear, &mut Vec::new());
                    fill(
                        &mut list,
                        canvas,
                        &Paint::texture(surface.texture(), nearest()),
                    );
                    list.push_layer(Fill::NonZero, blend, 1.0, Affine::IDENTITY, &canvas);
                    if shadow {
                        list.push_drop_shadow(2.0, 1.0, 1.5, Color::from_rgba8(0, 0, 0, 200));
                    }
                    if let Some(paint) = draw {
                        fill(&mut list, shape, paint);
                    }
                    if shadow {
                        list.pop_layer();
                    }
                    list.pop_layer();
                    surface.draw(&mut list, Init::Backdrop, &mut Vec::new());
                    pixels(&mut surface)
                };
                let expected = render(None);
                for (case, paint) in &missing {
                    assert_eq!(
                        render(Some(paint)),
                        expected,
                        "{case} under {name}, shadowed: {shadow}"
                    );
                }
            }
        }
    }
}
