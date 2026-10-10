//! The painter's raster cache for vector images: one texture per
//! `(image, device width, device height)`, baked with vello's own renderer
//! and drawn as a bitmap.
//!
//! A commit records a vector image (an SVG document) as a
//! [`VectorDraw`]: the document's scene, its identity, and the device
//! geometry of one draw. Nothing in it is a GPU resource, so the frame stays
//! device-free and `Send + Sync`. [`VectorTextures`] is where the device
//! side lives: one per [`vello::Renderer`], owned beside that renderer's
//! [`AtlasResidency`] and [`super::blur::FilterTextures`], persisting across
//! frames and documents, and asked once per rendered frame for the table of
//! textures the compose program indexes by draw
//! ([`crate::CommittedFrame::compose_into`]'s `vectors`).
//!
//! # Key and size
//!
//! The key is `(VectorImage::key, width, height)`. The image key is
//! process-unique and the scene behind it immutable, so a key names one
//! picture for the life of the process and no document change can alias
//! it — which is why this cache, unlike the filter bakes keyed by commit id,
//! has no `forget`. The size is [`VectorDraw::device_size`]: the draw's
//! extent under the per-axis scale of its committed transform, rounded up,
//! clamped to [`crate::MAX_RENDERABLE_DIMENSION`] per axis (vello's atlas
//! can place nothing longer). Every draw of one image at one device size — every tile
//! of a repeated background, every element showing the same icon at the same
//! size — shares one texture; a draw at another size is another texture. A
//! compose-time scale (an exported `transform` curve) samples the committed
//! size's texture, the documented cost.
//!
//! # The bake
//!
//! A miss renders the scene with [`vello::Renderer::render_to_texture`]
//! into a fresh `Rgba8Unorm` texture of the key's size, over transparent,
//! under the map that places the image's viewport in the draw's unclamped
//! device extent by its `preserveAspectRatio`
//! ([`crate::paint::compose::aspect_transform`]: `none` stretches per axis;
//! `meet` and `slice` scale uniformly and align) and then scales that
//! extent per axis onto the texture ([`VectorDraw::placement`]), so a
//! clamped texture holds the same picture squeezed and the draw's stretch
//! restores it, inside one full `Normal` layer when the scene opens a blend
//! layer at its top level (vello #1198). vello writes its target with **straight**
//! alpha — `fine.wgsl` divides the colour by the coverage before the store —
//! so the texture is registered as an [`ImageAlphaType::Alpha`] override
//! image through [`vello::Renderer::override_image`], behind the deliberate
//! empty blob a registered override never reads. The compose program then
//! draws the handle as it draws any bitmap, with the brush scale
//! `extent / size`.
//!
//! A bake is a render through the frame's own renderer, so it owes the
//! [`AtlasResidency`] a pass like any other render: a scene with no patch at
//! all — a flat icon — frees the image atlas, which is exactly the loss the
//! residency repairs at each bitmap's next use.
//!
//! # Budget, pinning and eviction
//!
//! The resident textures are bounded by [`MAX_VECTOR_TEXTURE_BYTES`] —
//! 64 MiB natively, 32 MiB on wasm32; a constant, not a setting. The entries
//! the frame being prepared draws are **pinned** for that prepare. Once its
//! misses are baked, the unpinned entries are evicted least-recently-used
//! first until the total is back under the budget, each eviction
//! unregistering the override and so dropping the texture. A single draw
//! whose texture alone would exceed the budget gets none and encodes
//! nothing. A frame whose own draws together exceed the budget keeps them
//! all for that frame: the bound is on what is retained *across* frames,
//! and what one frame draws is what its own culling admitted.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use vello::kurbo::{Affine, Rect, Size};
use vello::peniko::{
    BlendMode, Blob, Color, Compose, Fill, ImageAlphaType, ImageData, ImageFormat, Mix,
};
use vello::wgpu;

use super::gpu::{AtlasResidency, GpuError, render_params};
use crate::paint::compose::VectorDraw;
use crate::visual::CommittedFrame;

/// The bytes of baked vector textures kept across frames.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_VECTOR_TEXTURE_BYTES: u64 = 64 * 1024 * 1024;
/// The bytes of baked vector textures kept across frames.
#[cfg(target_arch = "wasm32")]
pub const MAX_VECTOR_TEXTURE_BYTES: u64 = 32 * 1024 * 1024;

/// Bytes per `Rgba8Unorm` texel.
const BYTES_PER_TEXEL: u64 = 4;

/// One baked texture, by the handle the compose program draws it through.
///
/// The texture itself is held by the renderer's override table — a
/// `wgpu::Texture` is a shared handle — and goes when the override is
/// unregistered, so nothing here has to own it twice.
struct Entry {
    handle: ImageData,
    bytes: u64,
    /// The prepare that last drew it; see [`VectorTextures::tick`].
    last_used: u64,
}

/// The device side of vector images: the texture bank. See the module doc.
pub struct VectorTextures {
    entries: FxHashMap<(u64, u32, u32), Entry>,
    /// Index-parallel with the prepared frame's vector draws: the handle
    /// each draw is drawn through, or `None` for a draw with no texture.
    images: Vec<Option<ImageData>>,
    /// Bytes of every resident texture.
    bytes: u64,
    /// How many prepares this bank has run. An entry's `last_used` is the
    /// tick of the prepare that last drew it, so the entries at the current
    /// tick are the pinned ones and the smallest tick is the LRU victim.
    tick: u64,
    budget: u64,
    /// The scene one bake encodes. Reset per bake, so its capacity is
    /// reused across bakes and frames.
    bake: vello::Scene,
}

impl Default for VectorTextures {
    fn default() -> Self {
        Self::with_budget(MAX_VECTOR_TEXTURE_BYTES)
    }
}

impl std::fmt::Debug for VectorTextures {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VectorTextures")
            .field("resident", &self.entries.len())
            .field("bytes", &self.bytes)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl VectorTextures {
    /// A bank bounded by `budget` bytes. Production uses
    /// [`MAX_VECTOR_TEXTURE_BYTES`] through `Default`; a test shrinks it to
    /// watch eviction.
    pub(crate) fn with_budget(budget: u64) -> Self {
        Self {
            entries: FxHashMap::default(),
            images: Vec::new(),
            bytes: 0,
            tick: 0,
            budget,
            bake: vello::Scene::new(),
        }
    }

    /// Brings the bank up to `frame`: bakes the texture of every vector
    /// draw whose `(key, size)` is not resident, pins the ones the frame
    /// draws, evicts the rest past the budget, and answers the table the
    /// compose program indexes by draw.
    ///
    /// Cheap and allocation-free for a frame whose draws are all resident
    /// — every frame after the first that shows the same pictures at the
    /// same sizes — and for a frame with no vector draw at all, which
    /// touches no GPU resource.
    ///
    /// # Errors
    ///
    /// [`GpuError::Render`] if a bake render fails. The table is then
    /// incomplete and should not be composed from; the next prepare starts
    /// over, with every texture baked before the failure still resident.
    pub fn prepare(
        &mut self,
        frame: &CommittedFrame,
        renderer: &mut vello::Renderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &mut AtlasResidency,
    ) -> Result<&[Option<ImageData>], GpuError> {
        let draws = frame.vector_draws();
        self.images.clear();
        if draws.is_empty() {
            return Ok(&self.images);
        }
        self.tick += 1;
        let tick = self.tick;
        self.images.reserve(draws.len());
        for draw in draws {
            let (width, height) = draw.device_size();
            let bytes = u64::from(width) * u64::from(height) * BYTES_PER_TEXEL;
            if width == 0 || height == 0 || bytes > self.budget {
                self.images.push(None);
                continue;
            }
            let key = (draw.key, width, height);
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.last_used = tick;
                self.images.push(Some(entry.handle.clone()));
                continue;
            }
            let handle = bake(
                &mut self.bake,
                draw,
                width,
                height,
                renderer,
                device,
                queue,
                atlas,
            )?;
            self.entries.insert(
                key,
                Entry {
                    handle: handle.clone(),
                    bytes,
                    last_used: tick,
                },
            );
            self.bytes += bytes;
            self.images.push(Some(handle));
        }
        self.evict(renderer);
        Ok(&self.images)
    }

    /// The table [`Self::prepare`] last produced — what a composite render
    /// has to name so the atlas residency stays truthful about it.
    #[must_use]
    pub fn images(&self) -> &[Option<ImageData>] {
        &self.images
    }

    /// Bytes of texture resident right now.
    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        self.bytes
    }

    /// Textures resident right now.
    #[must_use]
    pub fn resident(&self) -> usize {
        self.entries.len()
    }

    /// Evicts unpinned entries, least recently used first, until the total
    /// is within the budget or only pinned entries remain.
    fn evict(&mut self, renderer: &mut vello::Renderer) {
        while self.bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.last_used != self.tick)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key);
            let Some(key) = victim else {
                break;
            };
            let entry = self
                .entries
                .remove(&key)
                .expect("the victim was just found in the table");
            renderer.override_image(&entry.handle, None);
            self.bytes -= entry.bytes;
        }
    }
}

/// Renders `draw`'s scene into a new `width` × `height` texture and
/// registers it as an override image, answering the handle it is drawn
/// through.
#[expect(
    clippy::too_many_arguments,
    reason = "one bake's full inputs: the scratch scene, the draw, its size, and the renderer's \
              three device handles plus the residency it shares"
)]
fn bake(
    scene: &mut vello::Scene,
    draw: &VectorDraw,
    width: u32,
    height: u32,
    renderer: &mut vello::Renderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &mut AtlasResidency,
) -> Result<ImageData, GpuError> {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("dom vector texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        // vello renders through a storage binding and copies an override
        // into its atlas with a texture-to-texture copy.
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    scene.reset();
    let bounds = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
    if draw.opens_blend {
        scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &bounds,
        );
    }
    scene.append(
        &draw.scene,
        Some(draw.placement(
            draw.device_extent(),
            Size::new(f64::from(width), f64::from(height)),
        )),
    );
    if draw.opens_blend {
        scene.pop_layer();
    }
    // Every render through this renderer owes the residency a pass,
    // including a bake: a patch-free bake frees the whole image atlas, which
    // is exactly the loss `AtlasResidency` repairs.
    atlas.prepare_all(renderer, scene, &[], &[], &[]);
    renderer
        .render_to_texture(
            device,
            queue,
            scene,
            &view,
            &render_params(Color::TRANSPARENT, width, height),
        )
        .map_err(|error| GpuError::Render(error.to_string()))?;

    // `Renderer::register_texture` builds a handle like this one; it is
    // built here so the alpha type is named where the render that fills the
    // texture is. The blob is the deliberate fake: vello never reads it,
    // because the image is registered as an override.
    let handle = ImageData {
        data: Blob::new(Arc::new([])),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    };
    renderer.override_image(
        &handle,
        Some(wgpu::TexelCopyTextureInfoBase {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        }),
    );
    Ok(handle)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use vello::peniko::{Color, ImageData};

    use crate::render::gpu::Headless;
    use crate::test_common::Doc;
    use crate::{DocumentKind, ImageEvent, ImageRole};

    /// A 64×64 document filled with `fill`.
    fn square(fill: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64">
                  <rect width="64" height="64" fill="{fill}"/></svg>"#
        )
    }

    /// The bytes of one 64×64 texture.
    const SQUARE_BYTES: u64 = 64 * 64 * 4;

    /// A page showing each `(source, document)` as a 64×64 `<image>`, the
    /// `n`th at `x = 100n`, with every document parsed into the registry,
    /// and the elements so a test can hide and show them. One document,
    /// because an image's key is per parse: the same markup parsed into a
    /// second document is another picture to the cache.
    fn page(images: &[(&str, &str)]) -> (Doc, Vec<crate::NodeId>) {
        let mut doc = Doc::with_css(
            "page { display: flex; position: relative; width: 800px; height: 600px; }
             image { display: flex; position: absolute; top: 0px; width: 64px; height: 64px; }",
        );
        let root = doc.root;
        let mut nodes = Vec::new();
        for (index, (source, _)) in images.iter().enumerate() {
            let node = doc.el(root, "image");
            doc.dom
                .set_inline_style(node, &format!("left: {}px", index * 100));
            doc.dom
                .set_image_source(node, ImageRole::Source, Some(source));
            nodes.push(node);
        }
        doc.dom.render();
        let _wanted = doc.dom.take_wanted_images();
        let events: Vec<ImageEvent> = images
            .iter()
            .map(|(source, svg)| {
                ImageEvent::parse_document(Arc::from(*source), svg.as_bytes(), DocumentKind::Svg)
            })
            .collect();
        doc.dom.apply_image_events(&events);
        doc.dom.render();
        (doc, nodes)
    }

    /// Commits a frame of `doc` showing exactly the images in `shown`.
    fn showing(
        doc: &mut Doc,
        nodes: &[crate::NodeId],
        shown: &[bool],
    ) -> Arc<crate::CommittedFrame> {
        for (index, (&node, &visible)) in nodes.iter().zip(shown).enumerate() {
            let display = if visible { "flex" } else { "none" };
            doc.dom.set_inline_style(
                node,
                &format!("left: {}px; display: {display}", index * 100),
            );
        }
        let frame = doc.dom.commit();
        assert_eq!(
            frame.vector_draws().len(),
            shown.iter().filter(|&&visible| visible).count(),
            "one draw per shown icon",
        );
        frame
    }

    fn handle_id(table: &[Option<ImageData>], index: usize) -> u64 {
        table[index]
            .as_ref()
            .expect("the draw has a texture")
            .data
            .id()
    }

    /// Two pictures under a budget that holds one texture: a frame drawing
    /// the second evicts the first's texture, a frame drawing the first
    /// again bakes it again (a fresh handle), a frame repeating a picture
    /// reuses its texture, and a frame drawing both keeps both for itself —
    /// pinned past the budget — until the next frame lets one go. And the
    /// texture holds the picture: the composed frame shows it.
    #[test]
    fn a_tiny_budget_evicts_the_unpinned_texture_and_re_bakes_it_on_return() {
        let mut gpu = Headless::new().expect("a usable GPU adapter is mandatory");
        gpu.set_vector_budget(SQUARE_BYTES + 1024);
        let red = ("app:///red.svg", square("#ff0000"));
        let blue = ("app:///blue.svg", square("#0000ff"));
        let (mut doc, nodes) = page(&[(red.0, &red.1), (blue.0, &blue.1)]);
        let red_frame = showing(&mut doc, &nodes, &[true, false]);
        let blue_frame = showing(&mut doc, &nodes, &[false, true]);
        let both_frame = showing(&mut doc, &nodes, &[true, true]);

        let first = handle_id(gpu.prepare_vectors(&red_frame).expect("bake"), 0);
        assert_eq!(gpu.vector_textures().resident(), 1);
        assert_eq!(gpu.vector_textures().resident_bytes(), SQUARE_BYTES);
        assert_eq!(
            handle_id(gpu.prepare_vectors(&red_frame).expect("hit"), 0),
            first,
            "a frame repeating the picture reuses its texture",
        );

        // The texture shows the picture where the element is.
        let vectors = gpu.prepare_vectors(&red_frame).expect("hit").to_vec();
        let mut scene = vello::Scene::new();
        red_frame.compose_into(&mut scene, &[], &[], &vectors, &|_| None, None);
        let pixels = gpu
            .render(&scene, &[], 800, 600, Color::WHITE)
            .expect("render");
        let at = (32 * 800 + 32) * 4;
        assert_eq!(&pixels[at..at + 4], &[255, 0, 0, 255], "the red icon");
        let outside = (32 * 800 + 200) * 4;
        assert_eq!(&pixels[outside..outside + 4], &[255, 255, 255, 255]);

        let second = handle_id(gpu.prepare_vectors(&blue_frame).expect("bake"), 0);
        assert_ne!(second, first);
        assert_eq!(
            gpu.vector_textures().resident(),
            1,
            "the red texture, unpinned, was evicted for the blue one",
        );
        assert_eq!(gpu.vector_textures().resident_bytes(), SQUARE_BYTES);

        let third = handle_id(gpu.prepare_vectors(&red_frame).expect("re-bake"), 0);
        assert_ne!(third, first, "the red picture was baked again");
        assert_ne!(third, second);
        assert_eq!(gpu.vector_textures().resident(), 1);

        let both = gpu.prepare_vectors(&both_frame).expect("bake").to_vec();
        assert_eq!(
            handle_id(&both, 0),
            third,
            "the red texture is a hit, the blue one a miss"
        );
        assert_eq!(
            gpu.vector_textures().resident(),
            2,
            "both are pinned by the frame that draws them, past the budget",
        );
        assert_eq!(gpu.vector_textures().resident_bytes(), 2 * SQUARE_BYTES);
        let _ = gpu.prepare_vectors(&blue_frame).expect("hit");
        assert_eq!(
            gpu.vector_textures().resident(),
            1,
            "and the next frame lets the one it does not draw go",
        );
    }

    /// A draw whose texture alone exceeds the budget gets none: the table
    /// says so, nothing is resident, and the composed frame simply lacks
    /// the picture.
    #[test]
    fn a_draw_larger_than_the_whole_budget_gets_no_texture() {
        let mut gpu = Headless::new().expect("a usable GPU adapter is mandatory");
        gpu.set_vector_budget(SQUARE_BYTES - 1);
        let red = ("app:///red.svg", square("#ff0000"));
        let (mut doc, _) = page(&[(red.0, &red.1)]);
        let frame = doc.dom.commit();
        let vectors = gpu.prepare_vectors(&frame).expect("prepare").to_vec();
        assert_eq!(vectors, [None]);
        assert_eq!(gpu.vector_textures().resident(), 0);
        let mut scene = vello::Scene::new();
        frame.compose_into(&mut scene, &[], &[], &vectors, &|_| None, None);
        assert_eq!(
            scene.encoding().resources.patches.len(),
            0,
            "no texture is drawn"
        );
    }
}
