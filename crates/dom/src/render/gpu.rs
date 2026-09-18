//! The wgpu side: adapter/device management and rendering built scenes,
//! including a headless render-to-texture path with pixel readback (the
//! test and screenshot surface — embedders with a window drive
//! `vello::util::RenderContext`/`RenderSurface` themselves through the
//! [`crate::vello`] re-export).
//!
//! There is one render policy in this crate: area-only antialiasing.
//! [`renderer_options`] and [`render_params`] are its single definition, and
//! every target rendered through this crate — the headless one here and an
//! embedder's windowed one — must be constructed from them, or a windowed
//! frame will not match a headless screenshot of the same scene.
//!
//! [`AtlasResidency`] is the second thing every target owes each frame, and
//! every [`vello::Renderer`] rendered through this crate is paired with one.
//! [`super::blur::FilterTextures`] is the third, and unlike the other two it
//! is optional per frame: a frame with no `filter: blur()` group asks nothing
//! of it.

use std::fmt;

use euclid::default::Vector2D;
use rustc_hash::FxHashSet;
use vello::peniko::ImageData;
use vello::util::RenderContext;
use vello::wgpu;

use super::blur::FilterTextures;
use crate::visual::{CommittedFrame, ScrollSlot};

/// Headless GPU renderer for tests, benchmarks, and windowless embedders.
pub struct Headless {
    context: RenderContext,
    device_index: usize,
    renderer: vello::Renderer,
    atlas: AtlasResidency,
    filters: FilterTextures,
    target: Option<RenderTarget>,
    readback: Option<ReadbackBuffer>,
}

/// Which bitmaps one [`vello::Renderer`] has re-uploaded since its image
/// atlas was last freed.
///
/// vello frees the atlas whenever a scene with no patch at all renders — no
/// image, no gradient ramp, no glyph run: `Resolver::resolve` returns
/// `Images::default()`, which resizes the persistent proxy to 1x1 and drops
/// the texture — while `ImageCache` still counts every resident image clean,
/// so nothing re-uploads afterwards and later draws sample the freed slot.
/// The repair is a dirty mark, which re-uploads the whole bitmap on its next
/// use; doing that unconditionally would put an image-bytes-linear upload on
/// every frame, so this records what has already been repaired and marks each
/// image at most once per loss.
///
/// Nothing is owed for an image vello has evicted and will reinsert, or has
/// never seen: both enter the cache dirty by construction, so a skipped mark
/// cannot cost pixels.
#[derive(Debug, Default)]
pub struct AtlasResidency {
    /// Blob ids ([`vello::peniko::Blob::id`]) marked since the last loss.
    reuploaded: FxHashSet<u64>,
}

impl AtlasResidency {
    /// Brings `renderer` up to a render of `scene` drawing `images`. Call
    /// immediately before that render.
    pub fn prepare(
        &mut self,
        renderer: &mut vello::Renderer,
        scene: &vello::Scene,
        images: &[Option<ImageData>],
    ) {
        self.prepare_all(renderer, scene, images, &[]);
    }

    /// [`Self::prepare`] over two tables: the bitmaps a frame's image draws
    /// resolved, and the textures its `filter: blur()` groups baked.
    ///
    /// The filter textures are override images like any other as far as the
    /// atlas is concerned, so they are owed the same repair after a loss —
    /// and a bake of a solid-color group is precisely the patch-free render
    /// that causes one.
    pub fn prepare_all(
        &mut self,
        renderer: &mut vello::Renderer,
        scene: &vello::Scene,
        images: &[Option<ImageData>],
        filtered: &[Option<ImageData>],
    ) {
        self.prepare_with(
            scene.encoding().resources.patches.is_empty(),
            images,
            filtered,
            |image| renderer.mark_override_image_dirty(image),
        );
    }

    /// The decision itself, with the marking supplied, so it is checkable
    /// without a GPU.
    ///
    /// The marks are emitted before the loss is recorded: a patch-free scene
    /// that still names images must leave nothing behind, since the render
    /// that draws them is the one that frees the atlas.
    fn prepare_with(
        &mut self,
        loses_atlas: bool,
        images: &[Option<ImageData>],
        filtered: &[Option<ImageData>],
        mut mark: impl FnMut(&ImageData),
    ) {
        for image in images.iter().chain(filtered).flatten() {
            if self.reuploaded.insert(image.data.id()) {
                mark(image);
            }
        }
        if loses_atlas {
            self.reuploaded.clear();
        }
    }
}

#[derive(Debug)]
struct RenderTarget {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

#[derive(Debug)]
struct ReadbackBuffer {
    padded_bytes_per_row: u32,
    height: u32,
    buffer: wgpu::Buffer,
}

impl fmt::Debug for Headless {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Headless").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum GpuError {
    NoAdapter,
    Render(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter => f.write_str("no usable GPU adapter"),
            Self::Render(message) => write!(f, "GPU rendering failed: {message}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Renderer construction options for this crate's one render policy:
/// area-only antialiasing. Windowed embedders building their own
/// [`vello::Renderer`] must construct it with these options to match the
/// headless path.
#[must_use]
pub fn renderer_options() -> vello::RendererOptions {
    vello::RendererOptions {
        antialiasing_support: vello::AaSupport::area_only(),
        ..vello::RendererOptions::default()
    }
}

/// Per-frame render parameters for that same policy over `base_color`.
#[must_use]
pub fn render_params(
    base_color: vello::peniko::Color,
    width: u32,
    height: u32,
) -> vello::RenderParams {
    vello::RenderParams {
        base_color,
        width,
        height,
        antialiasing_method: vello::AaConfig::Area,
    }
}

impl Headless {
    /// Creates a headless renderer on the platform's default adapter.
    ///
    /// Returns [`GpuError::NoAdapter`] when the platform has no usable adapter
    /// rather than panicking: an embedder can surface that and fall back, while
    /// tests treat it as a hard environment failure.
    pub fn new() -> Result<Self, GpuError> {
        let mut context = RenderContext::new();
        let device_index = pollster::block_on(context.device(None)).ok_or(GpuError::NoAdapter)?;
        let handle = &context.devices[device_index];
        let renderer = vello::Renderer::new(&handle.device, renderer_options())
            .map_err(|error| GpuError::Render(error.to_string()))?;
        Ok(Self {
            context,
            device_index,
            renderer,
            atlas: AtlasResidency::default(),
            filters: FilterTextures::default(),
            target: None,
            readback: None,
        })
    }

    /// Drops the render target this renderer retained, and the readback
    /// buffer sized for it.
    ///
    /// For a target that changes documents. The target is given up rather than
    /// kept because it *is* the last frame here — there is no surface in front
    /// of it, so a reader would otherwise be handed the previous document's
    /// pixels as this one's. [`Self::render_frame`] builds a new one, and
    /// [`Self::read_pixels`] has nothing to read until it does.
    ///
    /// The atlas residency is untouched: it mirrors this renderer's image
    /// cache, which a change of document does not disturb. The filter bakes
    /// keep their textures but forget which frame they were baked for, since
    /// commit ids restart at one per document.
    pub fn forget(&mut self) {
        self.target = None;
        self.readback = None;
        self.filters.forget();
    }

    /// Forgets which frame the `filter: blur()` bakes belong to, keeping the
    /// render target.
    ///
    /// Every consumer that points this renderer at a *second document* owes
    /// this call: the bake cache is keyed by commit id, and commit ids
    /// restart at one per document. [`Self::forget`] includes it.
    pub fn forget_filters(&mut self) {
        self.filters.forget();
    }

    /// Bakes `frame`'s `filter: blur()` groups, if it has any, and answers
    /// the table [`crate::CommittedFrame::compose_into`] takes.
    ///
    /// Call before composing. A frame with no filter group answers an empty
    /// slice and touches no GPU resource.
    ///
    /// # Errors
    ///
    /// [`GpuError::Render`] if a bake render fails.
    pub fn prepare_filters(
        &mut self,
        frame: &CommittedFrame,
        images: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
    ) -> Result<&[Option<ImageData>], GpuError> {
        let Self {
            context,
            device_index,
            renderer,
            atlas,
            filters,
            ..
        } = self;
        let handle = &context.devices[*device_index];
        filters.prepare(
            renderer,
            &handle.device,
            &handle.queue,
            atlas,
            frame,
            images,
            offset_of,
            scroll_generation,
        )
    }

    /// Renders a scene drawing `images` into the retained headless texture.
    pub fn render_frame(
        &mut self,
        scene: &vello::Scene,
        images: &[Option<ImageData>],
        width: u32,
        height: u32,
        base_color: vello::peniko::Color,
    ) -> Result<(), GpuError> {
        if width == 0 || height == 0 {
            return Err(GpuError::Render(format!(
                "render target must be non-empty, got {width}\u{d7}{height}"
            )));
        }

        self.ensure_target(width, height);
        let Self {
            context,
            device_index,
            renderer,
            atlas,
            filters,
            target,
            ..
        } = self;
        // The bakes this renderer holds are override images the composite
        // render may draw, so they are named here too — the residency has to
        // see every image a render touches or a post-loss repair is missed.
        atlas.prepare_all(renderer, scene, images, filters.images());
        let handle = &context.devices[*device_index];
        let view = &target
            .as_ref()
            .expect("ensure_target installs a render target")
            .view;
        if let Err(error) = renderer.render_to_texture(
            &handle.device,
            &handle.queue,
            scene,
            view,
            &render_params(base_color, width, height),
        ) {
            *target = None;
            return Err(GpuError::Render(error.to_string()));
        }
        Ok(())
    }

    /// Waits for all submitted GPU work.
    pub fn wait_idle(&self) -> Result<(), GpuError> {
        let handle = &self.context.devices[self.device_index];
        handle
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| GpuError::Render(error.to_string()))?;
        Ok(())
    }

    /// Reads the last frame as tightly packed row-major RGBA8 pixels.
    pub fn read_pixels(&mut self) -> Result<Vec<u8>, GpuError> {
        let (width, height) = self
            .target
            .as_ref()
            .map(|target| (target.width, target.height))
            .ok_or_else(|| GpuError::Render("no headless frame has been rendered".to_owned()))?;
        self.ensure_readback(width, height);

        let Self {
            context,
            device_index,
            target,
            readback,
            ..
        } = self;
        let handle = &context.devices[*device_index];
        let device = &handle.device;
        let queue = &handle.queue;
        let target = target
            .as_ref()
            .expect("a target was checked before allocating readback");
        let readback = readback
            .as_ref()
            .expect("ensure_readback installs a staging buffer");

        let tight_bytes_per_row = width * 4;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("dom headless readback copy"),
        });
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(readback.padded_bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let error = {
            let slice = readback.buffer.slice(..);
            let (sender, receiver) = flume::bounded(1);
            slice.map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
            let waited = device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|error| GpuError::Render(error.to_string()))
                .and_then(|_| {
                    receiver
                        .recv()
                        .map_err(|_| GpuError::Render("readback map callback dropped".to_owned()))?
                        .map_err(|error| GpuError::Render(error.to_string()))
                });
            match waited {
                Ok(()) => {
                    let mapped = slice.get_mapped_range();
                    let mut pixels =
                        Vec::with_capacity(tight_bytes_per_row as usize * height as usize);
                    for row in mapped.chunks_exact(readback.padded_bytes_per_row as usize) {
                        pixels.extend_from_slice(&row[..tight_bytes_per_row as usize]);
                    }
                    drop(mapped);
                    readback.buffer.unmap();
                    return Ok(pixels);
                }
                Err(error) => error,
            }
        };
        self.readback = None;
        Err(error)
    }

    /// Renders a frame and reads it back as tightly-packed RGBA8 pixels.
    pub fn render(
        &mut self,
        scene: &vello::Scene,
        images: &[Option<ImageData>],
        width: u32,
        height: u32,
        base_color: vello::peniko::Color,
    ) -> Result<Vec<u8>, GpuError> {
        self.render_frame(scene, images, width, height, base_color)?;
        self.read_pixels()
    }

    fn ensure_target(&mut self, width: u32, height: u32) {
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.width == width && target.height == height)
        {
            return;
        }

        let device = &self.context.devices[self.device_index].device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dom headless target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.target = Some(RenderTarget {
            width,
            height,
            texture,
            view,
        });
    }

    fn ensure_readback(&mut self, width: u32, height: u32) {
        let padded_bytes_per_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        if self.readback.as_ref().is_some_and(|readback| {
            readback.padded_bytes_per_row == padded_bytes_per_row && readback.height == height
        }) {
            return;
        }

        let device = &self.context.devices[self.device_index].device;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dom headless readback"),
            size: u64::from(padded_bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.readback = Some(ReadbackBuffer {
            padded_bytes_per_row,
            height,
            buffer,
        });
    }
}

/// Reads an RGBA8 texture into tightly packed row-major pixels.
pub fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, GpuError> {
    if width == 0 || height == 0 {
        return Err(GpuError::Render(format!(
            "readback source must be non-empty, got {width}\u{d7}{height}"
        )));
    }

    let tight_bytes_per_row = width * 4;
    let padded_bytes_per_row =
        tight_bytes_per_row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dom texture readback"),
        size: u64::from(padded_bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("dom texture readback copy"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = staging.slice(..);
    let (sender, receiver) = flume::bounded(1);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| GpuError::Render(error.to_string()))?;
    receiver
        .recv()
        .map_err(|_| GpuError::Render("readback map callback dropped".to_owned()))?
        .map_err(|error| GpuError::Render(error.to_string()))?;

    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity(tight_bytes_per_row as usize * height as usize);
    for row in mapped.chunks_exact(padded_bytes_per_row as usize) {
        pixels.extend_from_slice(&row[..tight_bytes_per_row as usize]);
    }
    Ok(pixels)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

    use super::AtlasResidency;

    /// A 1x1 bitmap with an identity of its own — `Blob::new` takes a fresh
    /// id per call, which is what the residency keys on.
    fn image() -> ImageData {
        ImageData {
            data: Blob::new(Arc::new([255_u8, 0, 0, 255])),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 1,
            height: 1,
        }
    }

    /// How many marks a render of `images` emits, given whether that render
    /// frees the atlas.
    fn marks(
        residency: &mut AtlasResidency,
        loses_atlas: bool,
        images: &[Option<ImageData>],
    ) -> usize {
        let mut marked = 0;
        residency.prepare_with(loses_atlas, images, &[], |_| marked += 1);
        marked
    }

    /// The steady state: no patch-free render, so a bitmap is marked once
    /// however many frames draw it, and re-uploads nothing afterwards.
    #[test]
    fn a_resident_bitmap_is_marked_once_until_the_atlas_is_lost() {
        let mut residency = AtlasResidency::default();
        let drawn = [Some(image())];
        assert_eq!(marks(&mut residency, false, &drawn), 1, "the first sight");
        for frame in 0..8 {
            assert_eq!(
                marks(&mut residency, false, &drawn),
                0,
                "frame {frame} re-uploaded a bitmap vello still holds"
            );
        }
        assert_eq!(
            marks(&mut residency, true, &[]),
            0,
            "a solid-paths frame draws no bitmap to mark"
        );
        assert_eq!(
            marks(&mut residency, false, &drawn),
            1,
            "the loss must be repaired at the bitmap's next use"
        );
    }

    /// The repair is owed per bitmap, not per frame: a frame after the loss
    /// that draws only one of two bitmaps must not settle the other's debt.
    #[test]
    fn a_frame_after_the_loss_repairs_only_the_bitmaps_it_draws() {
        let mut residency = AtlasResidency::default();
        let (first, second) = (image(), image());
        let both = [Some(first.clone()), Some(second)];
        assert_eq!(marks(&mut residency, false, &both), 2);
        assert_eq!(marks(&mut residency, true, &[]), 0);
        assert_eq!(marks(&mut residency, false, &[Some(first)]), 1);
        assert_eq!(
            marks(&mut residency, false, &both),
            1,
            "the bitmap the intervening frame did not draw is still owed"
        );
    }

    /// A patch-free render that still names bitmaps is the render that frees
    /// the atlas, so it must leave nothing recorded.
    #[test]
    fn a_patch_free_render_naming_bitmaps_records_none_of_them() {
        let mut residency = AtlasResidency::default();
        let drawn = [Some(image())];
        assert_eq!(marks(&mut residency, true, &drawn), 1);
        assert_eq!(
            marks(&mut residency, false, &drawn),
            1,
            "the atlas went with that render, so the mark is owed again"
        );
    }
}
