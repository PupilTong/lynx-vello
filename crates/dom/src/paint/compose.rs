//! The compose program: one commit's scene, split where scroll chains
//! change, so offsets apply at composition instead of at encode.
//!
//! The walker's own layer discipline is preserved wholesale by construction:
//! every walker-level `push_layer`/`push_clip_layer`/`pop_layer` becomes a
//! program op carrying the chain its shape rides, and everything painted
//! *between* those pushes — item fragments, mask patterns, filter
//! adjustments — lands in the current fragment, cut whenever the content's
//! chain changes. Replaying the program with a set of chain translations
//! reproduces exactly the operation sequence the monolithic walk would have
//! encoded at those offsets: push ops re-encode their shapes under a
//! translated transform, fragments append under the same translation
//! (`Scene::append` left-multiplies the child's transform stream), and pops
//! are pops. Painter-internal pushes (background layers, text `SrcIn`
//! sandwiches, inset-shadow isolation) are balanced within one item and stay
//! inside fragments untouched.
//!
//! Translation per chain is the sum of the chain's slot offsets, each
//! snapped to the device pixel grid so composed edges stay crisp.

use euclid::default::Vector2D;

use crate::paint::shape::{BoxShape, with_shape};
use crate::render::image::{FrameImages, ImageRef};
use crate::vello::Scene;
use crate::vello::kurbo::{Affine, Point, Rect, Size};
use crate::vello::peniko::{BlendMode, BrushRef, Fill, ImageBrush, ImageSampler};
use crate::visual::{AnimationSample, ScrollSlot};

/// The compose-time coordinate context one op or fragment rides: the scroll
/// chain whose translations move it, and the animation chain whose sampled
/// deltas move it. Scroll translations always apply outside animation deltas
/// — export eligibility refuses a scroll container inside an animated
/// subtree, so the two never interleave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ComposeChain {
    pub(crate) scroll: Option<u32>,
    pub(crate) animation: Option<u32>,
}

/// A shape captured at encode time, replayable without the document.
#[derive(Debug)]
pub(crate) enum CapturedShape {
    Rect(Rect),
    Box(BoxShape),
}

/// The shape a raster fill resolves to, decided from geometry alone.
///
/// The clip-layer pair of the rounded-partial case lives *inside* this
/// variant rather than as surrounding program ops. That is not tidiness:
/// `Encoding::encode_end_clip` is a silent no-op at `n_open_clips == 0`, so a
/// fragment cut landing between a `push_clip_layer` and its `pop_layer` would
/// produce a wrong picture with no error anywhere. Keeping the pair inside one
/// op makes that cut unrepresentable.
#[derive(Debug)]
pub(crate) enum ImageArea {
    /// The draw covers the clip, or a rectangular clip reduced to the
    /// intersection: one fill, no layer.
    Fill(CapturedShape),
    /// A rounded clip the draw only partly covers: clip layer, fill, pop.
    Clipped { clip: BoxShape, draw: Rect },
}

/// One raster image fill whose only late input is the pixels.
///
/// Every CSS decision is already resolved here, on the document's thread,
/// from the intrinsic dimensions the registry holds: `object-fit`, the tile
/// grid, repeat, position, and which shape to fill. What is deliberately not
/// resolved is the brush scale, because that is the one quantity that depends
/// on the decoded bitmap.
///
/// `anchor` and `extent` are carried separately rather than pre-multiplied
/// into a brush transform so the division by the bitmap's own dimensions
/// happens at encode time. That lets a store decode at reduced scale and still
/// compose correctly, and stops a superseded generation whose dimensions
/// differ from silently drawing at the wrong size.
#[derive(Debug)]
pub(crate) struct ImageDraw {
    pub(crate) image: ImageRef,
    /// Item-local space to device px.
    pub(crate) transform: Affine,
    /// Where one copy of the source image starts, item-local.
    pub(crate) anchor: Point,
    /// How large one copy is, item-local.
    pub(crate) extent: Size,
    /// Extend modes, `image-rendering` quality, alpha. Carries no pixels.
    pub(crate) sampler: ImageSampler,
    pub(crate) area: ImageArea,
}

/// Encodes one image draw, resolving its pixels through `pixels`.
///
/// A read that misses draws nothing, which is the same one-frame gap a
/// not-yet-loaded image already produces. `outer` is the device chain
/// transform when replaying, or the plane translation when baking.
pub(crate) fn encode_image(
    scene: &mut Scene,
    draw: &ImageDraw,
    outer: Affine,
    pixels: &dyn FrameImages,
) {
    let Some(data) = pixels.read(draw.image) else {
        return;
    };
    debug_assert!(
        data.width > 0
            && data.height > 0
            && data.width <= crate::render::image::MAX_RENDERABLE_DIMENSION
            && data.height <= crate::render::image::MAX_RENDERABLE_DIMENSION,
        "a store must not hand back a bitmap vello cannot place",
    );
    let transform = outer * draw.transform;
    let brush_transform = Affine::translate(draw.anchor.to_vec2())
        * Affine::scale_non_uniform(
            draw.extent.width / f64::from(data.width),
            draw.extent.height / f64::from(data.height),
        );
    let brush = BrushRef::Image(ImageBrush {
        image: &data,
        sampler: draw.sampler,
    });
    match &draw.area {
        ImageArea::Fill(CapturedShape::Rect(rect)) => {
            scene.fill(Fill::NonZero, transform, brush, Some(brush_transform), rect);
        }
        ImageArea::Fill(CapturedShape::Box(shape)) => {
            with_shape!(shape, |s| scene.fill(
                Fill::NonZero,
                transform,
                brush,
                Some(brush_transform),
                s
            ));
        }
        ImageArea::Clipped { clip, draw: rect } => {
            with_shape!(clip, |s| scene.push_clip_layer(Fill::NonZero, transform, s));
            scene.fill(Fill::NonZero, transform, brush, Some(brush_transform), rect);
            scene.pop_layer();
        }
    }
}

/// One walker-level layer-stack operation, with the chain its shape rides.
pub(crate) enum ComposeOp {
    /// Append `fragments[index]` transformed by `chain`.
    Fragment {
        index: u32,
        chain: ComposeChain,
    },
    /// `Scene::push_layer` (or `push_clip_layer` when `clip_only`) with the
    /// recorded parameters, the transform carried by `chain`.
    Push {
        clip_only: bool,
        fill: Fill,
        blend: BlendMode,
        alpha: f32,
        transform: Affine,
        shape: CapturedShape,
        chain: ComposeChain,
        /// The animation slot whose sampled opacity replaces `alpha` — set
        /// only on the effect layer of an element exporting an opacity
        /// curve.
        alpha_animation: Option<u32>,
    },
    Pop,
    /// Draw `image_draws[index]`, whose pixels the composer supplies.
    Image {
        index: u32,
        chain: ComposeChain,
    },
}

/// The compose sink the walker fills: fragments plus the program over them.
#[derive(Default)]
pub(crate) struct ComposeAssembly {
    pub(crate) fragments: Vec<Scene>,
    pub(crate) program: Vec<ComposeOp>,
    pub(crate) image_draws: Vec<ImageDraw>,
    /// The chain of the currently open fragment, if one is open.
    current: Option<ComposeChain>,
    /// Emptied scenes to encode the next fragments into.
    pool: Vec<Scene>,
}

impl std::fmt::Debug for ComposeAssembly {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComposeAssembly")
            .field("fragments", &self.fragments.len())
            .field("program", &self.program.len())
            .finish_non_exhaustive()
    }
}

impl ComposeAssembly {
    /// An assembly over recycled storage: the emptied fragment and program
    /// containers of a retired frame, and the pool of emptied scenes its
    /// fragments encode into.
    pub(crate) fn with_storage(
        fragments: Vec<Scene>,
        program: Vec<ComposeOp>,
        image_draws: Vec<ImageDraw>,
        pool: Vec<Scene>,
    ) -> Self {
        debug_assert!(
            fragments.is_empty() && program.is_empty() && image_draws.is_empty(),
            "recycled containers are emptied before they are handed back",
        );
        Self {
            fragments,
            program,
            image_draws,
            current: None,
            pool,
        }
    }

    /// The scene content on `chain` encodes into, cutting a fragment when
    /// the chain changed.
    pub(crate) fn fragment_for(&mut self, chain: ComposeChain) -> &mut Scene {
        if self.current != Some(chain) {
            self.seal_fragment();
            let mut scene = self.pool.pop().unwrap_or_default();
            scene.reset();
            self.fragments.push(scene);
            self.current = Some(chain);
        }
        self.fragments
            .last_mut()
            .expect("an open fragment was just ensured")
    }

    /// Records a layer-stack op, sealing any open fragment first: the op
    /// must land after the content already encoded.
    pub(crate) fn push_op(&mut self, op: ComposeOp) {
        self.seal_fragment();
        self.program.push(op);
    }

    /// Records one image draw as a program op, sealing any open fragment
    /// first so the draw lands after the content already encoded.
    pub(crate) fn push_image(&mut self, chain: ComposeChain, draw: ImageDraw) {
        let index = u32::try_from(self.image_draws.len()).expect("a frame cannot hold 2^32 images");
        self.image_draws.push(draw);
        self.push_op(ComposeOp::Image { index, chain });
    }

    /// Closes the open fragment: an empty one goes back to the pool and
    /// leaves no op, everything else becomes a `Fragment` op in place.
    fn seal_fragment(&mut self) {
        let Some(chain) = self.current.take() else {
            return;
        };
        let encoding = self
            .fragments
            .last()
            .expect("an open fragment has a scene")
            .encoding();
        // A cut between a `push_clip_layer` and its `pop_layer` is silently
        // wrong rather than loud: `Encoding::encode_end_clip` does nothing at
        // zero open clips, so the pop is dropped and every later draw stays
        // clipped. Painter-internal layers must therefore close inside the
        // fragment that opened them.
        debug_assert_eq!(
            encoding.n_open_clips, 0,
            "a fragment must never be cut inside a painter-internal layer",
        );
        // Glyphs are deferred resources: a text-only fragment has an empty
        // path stream, so `Encoding::is_empty` alone would discard it.
        if encoding.is_empty() && encoding.resources.glyph_runs.is_empty() {
            let scene = self.fragments.pop().expect("the open fragment exists");
            self.pool.push(scene);
            return;
        }
        let index =
            u32::try_from(self.fragments.len() - 1).expect("a frame cannot hold 2^32 fragments");
        self.program.push(ComposeOp::Fragment { index, chain });
    }

    /// Finishes the assembly, returning fragments, program, and the unused
    /// pool.
    pub(crate) fn finish(mut self) -> (Vec<Scene>, Vec<ComposeOp>, Vec<ImageDraw>, Vec<Scene>) {
        self.seal_fragment();
        (self.fragments, self.program, self.image_draws, self.pool)
    }
}

/// One chain's full compose transform in CSS px: the animation chain's
/// sampled deltas (innermost applied first), then the scroll chain's
/// translation.
pub(crate) fn chain_transform(
    slots: &[ScrollSlot],
    samples: &[AnimationSample],
    chain: ComposeChain,
    ratio: f32,
    offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) -> Affine {
    let translation = chain_translation(slots, chain.scroll, ratio, offset_of);
    Affine::translate((-f64::from(translation.x), -f64::from(translation.y)))
        * animation_deltas(samples, chain.animation)
}

/// The ordered product of an animation chain's sampled deltas, outermost
/// first, in CSS px.
pub(crate) fn animation_deltas(samples: &[AnimationSample], chain: Option<u32>) -> Affine {
    let mut product = Affine::IDENTITY;
    let mut current = chain;
    while let Some(index) = current {
        let sample = &samples[index as usize];
        product = sample.delta * product;
        current = sample.parent;
    }
    product
}

/// One chain's compose translation in CSS px: the sum of its slots' offsets,
/// each snapped to the device pixel grid.
pub(crate) fn chain_translation(
    slots: &[ScrollSlot],
    chain: Option<u32>,
    ratio: f32,
    offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) -> Vector2D<f32> {
    let mut sum = Vector2D::zero();
    let mut current = chain;
    while let Some(index) = current {
        let slot = &slots[index as usize];
        let offset = offset_of(slot).unwrap_or(slot.offset);
        sum += snap_offset(offset, ratio);
        current = slot.parent;
    }
    sum
}

pub(crate) fn snap_offset(offset: Vector2D<f32>, ratio: f32) -> Vector2D<f32> {
    if ratio.is_finite() && ratio > 0.0 {
        Vector2D::new(
            (offset.x * ratio).round() / ratio,
            (offset.y * ratio).round() / ratio,
        )
    } else {
        offset
    }
}

/// Replays the program into `scene` with each chain translated by the
/// offsets `offset_of` reports (falling back to the committed ones).
#[expect(
    clippy::too_many_arguments,
    reason = "one replay's full inputs: the program, its two side tables, and the transforms"
)]
pub(crate) fn replay(
    scene: &mut Scene,
    fragments: &[Scene],
    program: &[ComposeOp],
    image_draws: &[ImageDraw],
    pixels: &dyn FrameImages,
    slots: &[ScrollSlot],
    samples: &[AnimationSample],
    ratio: f32,
    offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) {
    let device_transform = device_chain_transform(slots, samples, ratio, offset_of);
    replay_ops(
        scene,
        fragments,
        program,
        image_draws,
        pixels,
        samples,
        &device_transform,
    );
}

/// The device-px transform each chain composes at: the CSS-px chain
/// transform conjugated into device px, since encoded content carries the
/// device scale as its outermost factor — the chain applies inside one
/// scale and outside the other.
pub(crate) fn device_chain_transform<'a>(
    slots: &'a [ScrollSlot],
    samples: &'a [AnimationSample],
    ratio: f32,
    offset_of: &'a dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) -> impl Fn(ComposeChain) -> Affine + 'a {
    let scale = f64::from(ratio);
    move |chain: ComposeChain| {
        let css = chain_transform(slots, samples, chain, ratio, offset_of);
        if scale.is_finite() && scale > 0.0 {
            Affine::scale(scale) * css * Affine::scale(1.0 / scale)
        } else {
            css
        }
    }
}

/// Encodes one plane run into `scene`, every op placed by `translate` —
/// the run's uniform chain reduced to the plane-texture translation.
///
/// Clip-only pushes riding any chain but the run's head are the walker's
/// re-pushes of the slot's own clip chain: their shapes must not translate
/// with the plane, so the bake skips them (pops included) and the composite
/// applies the chain around the plane's draw instead.
pub(crate) fn bake_ops(
    scene: &mut Scene,
    fragments: &[Scene],
    program: &[ComposeOp],
    image_draws: &[ImageDraw],
    pixels: &dyn FrameImages,
    head: u32,
    translate: Affine,
) {
    let mut kept: Vec<bool> = Vec::new();
    for op in program {
        match op {
            ComposeOp::Fragment { index, .. } => {
                scene.append(&fragments[*index as usize], Some(translate));
            }
            ComposeOp::Push {
                clip_only,
                fill,
                blend,
                alpha,
                transform,
                shape,
                chain,
                // Absent inside a plane run by construction.
                alpha_animation: _,
            } => {
                let rides_head = chain.scroll == Some(head) && chain.animation.is_none();
                if *clip_only && !rides_head {
                    kept.push(false);
                    continue;
                }
                kept.push(true);
                let transform = translate * *transform;
                match (clip_only, shape) {
                    (true, CapturedShape::Rect(rect)) => {
                        scene.push_clip_layer(*fill, transform, rect);
                    }
                    (true, CapturedShape::Box(shape)) => {
                        with_shape!(shape, |s| scene.push_clip_layer(*fill, transform, s));
                    }
                    (false, CapturedShape::Rect(rect)) => {
                        scene.push_layer(*fill, *blend, *alpha, transform, rect);
                    }
                    (false, CapturedShape::Box(shape)) => {
                        with_shape!(shape, |s| scene
                            .push_layer(*fill, *blend, *alpha, transform, s));
                    }
                }
            }
            ComposeOp::Image { index, .. } => {
                encode_image(scene, &image_draws[*index as usize], translate, pixels);
            }
            ComposeOp::Pop => {
                if kept.pop().expect("a plane run's pushes balance its pops") {
                    scene.pop_layer();
                }
            }
        }
    }
}

/// Replays `program` with each chain placed by `device_transform`, resolving
/// image draws through `pixels`.
///
/// Besides pushes, appends and pops this also encodes raw geometry between
/// appends, for image draws. That is sound because `Encoding::append`
/// left-multiplies the child's transform stream before `encode_transform`'s
/// dedup compares against the last one, so an elided tag after an append is
/// genuinely redundant rather than wrong.
pub(crate) fn replay_ops(
    scene: &mut Scene,
    fragments: &[Scene],
    program: &[ComposeOp],
    image_draws: &[ImageDraw],
    pixels: &dyn FrameImages,
    samples: &[AnimationSample],
    device_transform: &impl Fn(ComposeChain) -> Affine,
) {
    for op in program {
        match op {
            ComposeOp::Fragment { index, chain } => {
                scene.append(&fragments[*index as usize], Some(device_transform(*chain)));
            }
            ComposeOp::Push {
                clip_only,
                fill,
                blend,
                alpha,
                transform,
                shape,
                chain,
                alpha_animation,
            } => {
                let alpha = alpha_animation
                    .and_then(|slot| samples[slot as usize].alpha)
                    .unwrap_or(*alpha);
                let transform = device_transform(*chain) * *transform;
                match (clip_only, shape) {
                    (true, CapturedShape::Rect(rect)) => {
                        scene.push_clip_layer(*fill, transform, rect);
                    }
                    (true, CapturedShape::Box(shape)) => {
                        with_shape!(shape, |s| scene.push_clip_layer(*fill, transform, s));
                    }
                    (false, CapturedShape::Rect(rect)) => {
                        scene.push_layer(*fill, *blend, alpha, transform, rect);
                    }
                    (false, CapturedShape::Box(shape)) => {
                        with_shape!(shape, |s| scene
                            .push_layer(*fill, *blend, alpha, transform, s));
                    }
                }
            }
            ComposeOp::Image { index, chain } => {
                encode_image(
                    scene,
                    &image_draws[*index as usize],
                    device_transform(*chain),
                    pixels,
                );
            }
            ComposeOp::Pop => scene.pop_layer(),
        }
    }
}
