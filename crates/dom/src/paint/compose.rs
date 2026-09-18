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
//!
//! One op pair is not a layer-stack operation: [`ComposeOp::PushFilter`] and
//! [`ComposeOp::PopFilter`] bracket the ops of a `filter: blur()` group. The
//! program carries both the bracket and the ops inside it, so the same
//! program serves two purposes — replayed with a baked texture for the group
//! the bracket is replaced by one `draw_image` and the range is skipped, and
//! replayed without one (no GPU, or a group over the memory budget) the range
//! plays raw and the frame simply is not blurred. The bake itself replays
//! that range into an offscreen scene; see
//! [`crate::CommittedFrame::bake_filter`].

use std::ops::Range;
use std::sync::Arc;

use euclid::default::Vector2D;

use crate::paint::shape::{BoxShape, with_shape};
use crate::render::image::{ImageSizeHint, is_renderable};
use crate::vello::Scene;
use crate::vello::kurbo::{Affine, Point, Rect, Size};
use crate::vello::peniko::{
    BlendMode, BrushRef, Extend, Fill, ImageBrush, ImageData, ImageQuality, ImageSampler,
};
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
    /// The raw source string the page wrote — the registry's own key, so
    /// every draw of one source in one frame shares one allocation.
    pub(crate) image: Arc<str>,
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

impl ImageDraw {
    /// How large this draw puts one copy of the image on the device: the
    /// extent under the per-axis scale of its transform, rounded up.
    ///
    /// Rotation and skew fold into the axis lengths, which over-estimates a
    /// rotated draw a little — the right direction for a hint that bounds a
    /// decode. A non-finite length bounds nothing.
    pub(crate) fn size_hint(&self) -> ImageSizeHint {
        let [a, b, c, d, _, _] = self.transform.as_coeffs();
        ImageSizeHint::new(
            device_length(self.extent.width * a.hypot(b)),
            device_length(self.extent.height * c.hypot(d)),
        )
    }
}

/// A device-pixel length as a hint axis: rounded up so a fractional draw is
/// never decoded a pixel short, saturating past `u32`.
fn device_length(length: f64) -> u32 {
    if !length.is_finite() {
        return u32::MAX;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the u32 range immediately before the cast"
    )]
    let length = length.ceil().clamp(0.0, f64::from(u32::MAX)) as u32;
    length
}

/// Encodes draw `index`, if its pixels resolved.
///
/// A draw whose source had no pixels is simply absent from the table's
/// answer, and draws nothing — the same one-frame gap a not-yet-loaded image
/// already produces.
fn encode_draw(
    scene: &mut Scene,
    draws: &[ImageDraw],
    images: &[Option<ImageData>],
    index: u32,
    outer: Affine,
) {
    let index = index as usize;
    if let Some(Some(data)) = images.get(index) {
        encode_image(scene, &draws[index], outer, data);
    }
}

/// Encodes one image draw against pixels already resolved for it.
///
/// A read that misses draws nothing, which is the same one-frame gap a
/// not-yet-loaded image already produces. `outer` is the device chain
/// transform the draw composes under.
pub(crate) fn encode_image(scene: &mut Scene, draw: &ImageDraw, outer: Affine, data: &ImageData) {
    // A bitmap vello cannot place draws as nothing — the same one-frame gap a
    // not-yet-loaded image already produces. This is the only place the bound
    // is enforced, because it is the only place the bitmap is known, and the
    // only place the extent is divided by it.
    if !is_renderable(data) {
        return;
    }
    let transform = outer * draw.transform;
    let brush_transform = Affine::translate(draw.anchor.to_vec2())
        * Affine::scale_non_uniform(
            draw.extent.width / f64::from(data.width),
            draw.extent.height / f64::from(data.height),
        );
    let brush = BrushRef::Image(ImageBrush {
        image: data,
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
    /// Open `filter_groups[index]`: draw its baked texture and skip to
    /// `ops.end` if the composer supplied one, otherwise replay the range.
    PushFilter {
        index: u32,
    },
    /// Close the innermost open filter group. A no-op on replay — the bracket
    /// exists so the group's op range is a fact about the program rather than
    /// something a consumer has to rediscover.
    PopFilter,
}

impl ComposeOp {
    /// The scroll/animation chain this op's own geometry rides, where it has
    /// one. `Pop` and `PopFilter` carry none: they close whatever the
    /// matching push opened.
    fn chain(&self, groups: &[FilterGroup]) -> Option<ComposeChain> {
        match self {
            Self::Fragment { chain, .. } | Self::Push { chain, .. } | Self::Image { chain, .. } => {
                Some(*chain)
            }
            Self::PushFilter { index } => Some(groups[*index as usize].chain),
            Self::Pop | Self::PopFilter => None,
        }
    }
}

/// One `filter: blur()` group: the ops whose composed pixels are baked
/// offscreen, blurred, and drawn back as one image.
///
/// Everything here is decided at commit time on the document's thread, in
/// device pixels, and carries no GPU resource: a frame stays `Send + Sync`
/// and device-free. `rect` already includes the 3σ ink margin and is
/// integer-valued, so the bake's own render target size is `rect`'s and the
/// texture composes at an integer offset — which is why nearest sampling
/// reproduces it exactly on an unanimated chain.
#[derive(Debug)]
pub struct FilterGroup {
    /// The blur's standard deviation, in device px.
    pub sigma: f32,
    /// The device-px region baked, integer-valued, 3σ larger than the
    /// group's own bounds on every side.
    pub rect: Rect,
    /// The chain the *texture* composes under. Content inside the group may
    /// ride inner scroll chains; see [`Self::inner_chains`].
    pub(crate) chain: ComposeChain,
    /// The ops strictly between this group's `PushFilter` and its
    /// `PopFilter`, as indices into the program.
    pub(crate) ops: Range<u32>,
    /// Whether some op in `ops` rides a scroll chain other than `chain` —
    /// the one condition under which the bake's pixels depend on a scroll
    /// offset, and therefore the one condition under which a scroll
    /// invalidates the bake.
    pub(crate) inner_chains: bool,
    /// The group this one nests inside, so the assembly needs no open-filter
    /// stack of its own.
    pub(crate) parent: Option<u32>,
}

impl FilterGroup {
    /// A group over `rect` at `sigma`, before the assembly brackets it.
    pub(crate) fn new(sigma: f32, rect: Rect, chain: ComposeChain) -> Self {
        Self {
            sigma,
            rect,
            chain,
            ops: 0..0,
            inner_chains: false,
            parent: None,
        }
    }

    /// The bake target's device-pixel size.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the walker records only finite, positive, integer-valued rects"
        )]
        {
            (self.rect.width() as u32, self.rect.height() as u32)
        }
    }
}

/// The compose sink the walker fills: fragments plus the program over them.
#[derive(Default)]
pub(crate) struct ComposeAssembly {
    pub(crate) fragments: Vec<Scene>,
    pub(crate) program: Vec<ComposeOp>,
    pub(crate) image_draws: Vec<ImageDraw>,
    pub(crate) filter_groups: Vec<FilterGroup>,
    /// The chain of the currently open fragment, if one is open.
    current: Option<ComposeChain>,
    /// The innermost open filter group; its own `parent` is the rest of the
    /// stack, so nesting costs no allocation.
    open_filter: Option<u32>,
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
        filter_groups: Vec<FilterGroup>,
        pool: Vec<Scene>,
    ) -> Self {
        debug_assert!(
            fragments.is_empty()
                && program.is_empty()
                && image_draws.is_empty()
                && filter_groups.is_empty(),
            "recycled containers are emptied before they are handed back",
        );
        Self {
            fragments,
            program,
            image_draws,
            filter_groups,
            current: None,
            open_filter: None,
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

    /// Opens a filter group: seals the open fragment, records the bracket,
    /// and answers the group's index in the side table.
    pub(crate) fn push_filter(&mut self, mut group: FilterGroup) -> u32 {
        self.seal_fragment();
        let index =
            u32::try_from(self.filter_groups.len()).expect("a frame cannot hold 2^32 filters");
        // The op right after the `PushFilter` this is about to record.
        let start =
            u32::try_from(self.program.len() + 1).expect("a frame cannot hold 2^32 program ops");
        group.parent = self.open_filter;
        group.ops = start..start;
        self.filter_groups.push(group);
        self.program.push(ComposeOp::PushFilter { index });
        self.open_filter = Some(index);
        index
    }

    /// Closes the innermost open filter group, completing its op range and
    /// deciding whether anything inside it rides another scroll chain.
    pub(crate) fn pop_filter(&mut self) {
        self.seal_fragment();
        let Some(index) = self.open_filter else {
            debug_assert!(false, "pop_filter is only called with an open filter group");
            return;
        };
        let end = u32::try_from(self.program.len()).expect("a frame cannot hold 2^32 program ops");
        let (chain, parent, start) = {
            let group = &mut self.filter_groups[index as usize];
            group.ops.end = end;
            (group.chain, group.parent, group.ops.start)
        };
        let inner = self.program[start as usize..end as usize].iter().any(|op| {
            op.chain(&self.filter_groups)
                .is_some_and(|op| op.scroll != chain.scroll)
        });
        self.filter_groups[index as usize].inner_chains = inner;
        self.open_filter = parent;
        self.program.push(ComposeOp::PopFilter);
    }

    /// Finishes the assembly, returning fragments, program, the two side
    /// tables, and the unused pool.
    pub(crate) fn finish(mut self) -> Finished {
        self.seal_fragment();
        debug_assert!(
            self.open_filter.is_none(),
            "every filter group the walk opened is closed before the frame is sealed",
        );
        Finished {
            fragments: self.fragments,
            program: self.program,
            image_draws: self.image_draws,
            filter_groups: self.filter_groups,
            pool: self.pool,
        }
    }
}

/// What one sealed assembly hands back.
pub(crate) struct Finished {
    pub(crate) fragments: Vec<Scene>,
    pub(crate) program: Vec<ComposeOp>,
    pub(crate) image_draws: Vec<ImageDraw>,
    pub(crate) filter_groups: Vec<FilterGroup>,
    pub(crate) pool: Vec<Scene>,
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
///
/// Each chain composes at its CSS-px transform conjugated into device px,
/// since encoded content carries the device scale as its outermost factor —
/// the chain applies inside one scale and outside the other.
///
/// Besides pushes, appends and pops this also encodes raw geometry between
/// appends, for image draws. That is sound because `Encoding::append`
/// left-multiplies the child's transform stream before `encode_transform`'s
/// dedup compares against the last one, so an elided tag after an append is
/// genuinely redundant rather than wrong.
#[expect(
    clippy::too_many_arguments,
    reason = "one replay's full inputs: the program, its three side tables, and the transforms"
)]
pub(crate) fn replay(
    scene: &mut Scene,
    fragments: &[Scene],
    program: &[ComposeOp],
    image_draws: &[ImageDraw],
    images: &[Option<ImageData>],
    filter_groups: &[FilterGroup],
    filtered: &[Option<ImageData>],
    slots: &[ScrollSlot],
    samples: &[AnimationSample],
    ratio: f32,
    offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) {
    let transform = device_transform(slots, samples, ratio, offset_of);
    replay_ops(
        scene,
        Tables {
            fragments,
            program,
            image_draws,
            images,
            filter_groups,
            filtered,
            samples,
        },
        0..program.len(),
        &transform,
    );
}

/// One chain's full compose transform in *device* px: the CSS-px chain
/// transform conjugated by the device scale.
pub(crate) fn device_transform<'a>(
    slots: &'a [ScrollSlot],
    samples: &'a [AnimationSample],
    ratio: f32,
    offset_of: &'a dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) -> impl Fn(ComposeChain) -> Affine + 'a {
    let scale = f64::from(ratio);
    move |chain| {
        let css = chain_transform(slots, samples, chain, ratio, offset_of);
        if scale.is_finite() && scale > 0.0 {
            Affine::scale(scale) * css * Affine::scale(1.0 / scale)
        } else {
            css
        }
    }
}

/// Everything one replay reads besides the transform and the op range: the
/// program and every side table it indexes.
#[derive(Clone, Copy)]
pub(crate) struct Tables<'a> {
    pub(crate) fragments: &'a [Scene],
    pub(crate) program: &'a [ComposeOp],
    pub(crate) image_draws: &'a [ImageDraw],
    pub(crate) images: &'a [Option<ImageData>],
    pub(crate) filter_groups: &'a [FilterGroup],
    /// One entry per [`Tables::filter_groups`] entry: the baked texture for
    /// that group, or `None` for the unblurred fallback.
    pub(crate) filtered: &'a [Option<ImageData>],
    pub(crate) samples: &'a [AnimationSample],
}

/// Replays one contiguous slice of the program.
///
/// `device_transform` maps a chain to the device-px transform its content
/// composes under. A whole-frame replay passes the chain transform itself; a
/// bake passes the same thing conjugated into the bake target's own origin
/// and chain, which is how one program serves both.
///
/// A `PushFilter` whose group has a baked texture draws that texture and
/// skips the group's ops; one without replays them raw — the documented
/// unblurred fallback.
pub(crate) fn replay_ops(
    scene: &mut Scene,
    tables: Tables<'_>,
    ops: Range<usize>,
    device_transform: &dyn Fn(ComposeChain) -> Affine,
) {
    let Tables {
        fragments,
        program,
        image_draws,
        images,
        filter_groups,
        filtered,
        samples,
    } = tables;
    let end = ops.end.min(program.len());
    let mut index = ops.start.min(end);
    while index < end {
        match &program[index] {
            ComposeOp::Fragment {
                index: fragment,
                chain,
            } => {
                scene.append(
                    &fragments[*fragment as usize],
                    Some(device_transform(*chain)),
                );
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
            ComposeOp::Image { index: draw, chain } => {
                encode_draw(scene, image_draws, images, *draw, device_transform(*chain));
            }
            ComposeOp::Pop => scene.pop_layer(),
            ComposeOp::PushFilter { index: group } => {
                let group_index = *group as usize;
                let group = &filter_groups[group_index];
                if let Some(Some(image)) = filtered.get(group_index) {
                    // The texture is the group's own pixels already blurred,
                    // in device px at `rect`'s origin; only the group's own
                    // chain is left to apply. `Extend::Pad` never fires — the
                    // draw covers exactly the image — and nearest sampling
                    // reproduces the texture byte for byte at the integer
                    // offsets a scroll chain snaps to. An animation chain can
                    // land it anywhere, so that case samples bilinearly.
                    let quality = if group.chain.animation.is_none() {
                        ImageQuality::Low
                    } else {
                        ImageQuality::Medium
                    };
                    scene.draw_image(
                        ImageBrush {
                            image,
                            sampler: ImageSampler {
                                x_extend: Extend::Pad,
                                y_extend: Extend::Pad,
                                quality,
                                alpha: 1.0,
                            },
                        },
                        device_transform(group.chain)
                            * Affine::translate((group.rect.x0, group.rect.y0)),
                    );
                    // Straight to the matching `PopFilter`, which is a no-op.
                    index = group.ops.end as usize;
                    continue;
                }
            }
            ComposeOp::PopFilter => {}
        }
        index += 1;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::vello::peniko::{Compose, Mix};

    fn assembly() -> ComposeAssembly {
        ComposeAssembly::with_storage(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new())
    }

    /// A clip push on `chain` — the cheapest op that carries one.
    fn push(chain: ComposeChain) -> ComposeOp {
        ComposeOp::Push {
            clip_only: true,
            fill: Fill::NonZero,
            blend: BlendMode::new(Mix::Normal, Compose::SrcOver),
            alpha: 1.0,
            transform: Affine::IDENTITY,
            shape: CapturedShape::Rect(Rect::ZERO),
            chain,
            alpha_animation: None,
        }
    }

    fn group(chain: ComposeChain) -> FilterGroup {
        FilterGroup::new(2.0, Rect::new(0.0, 0.0, 8.0, 8.0), chain)
    }

    fn scrolled(slot: u32) -> ComposeChain {
        ComposeChain {
            scroll: Some(slot),
            animation: None,
        }
    }

    /// Nested brackets pair up, and each group's range is exactly the ops
    /// strictly between its own pair.
    #[test]
    fn nested_filter_brackets_pair_and_bound_their_own_ops() {
        let mut assembly = assembly();
        let outer = assembly.push_filter(group(ComposeChain::default()));
        assembly.push_op(push(ComposeChain::default()));
        let inner = assembly.push_filter(group(ComposeChain::default()));
        assembly.push_op(push(ComposeChain::default()));
        assembly.pop_filter();
        assembly.pop_filter();
        let finished = assembly.finish();

        assert_eq!((outer, inner), (0, 1), "groups are numbered in push order");
        assert_eq!(
            finished.program.len(),
            6,
            "PushFilter, Push, PushFilter, Push, PopFilter, PopFilter",
        );
        assert!(matches!(
            finished.program[0],
            ComposeOp::PushFilter { index: 0 }
        ));
        assert!(matches!(
            finished.program[2],
            ComposeOp::PushFilter { index: 1 }
        ));
        assert!(matches!(finished.program[4], ComposeOp::PopFilter));
        assert!(matches!(finished.program[5], ComposeOp::PopFilter));
        assert_eq!(finished.filter_groups[0].ops, 1..5, "the outer range");
        assert_eq!(finished.filter_groups[1].ops, 3..4, "the inner range");
        assert_eq!(finished.filter_groups[1].parent, Some(0));
        assert_eq!(finished.filter_groups[0].parent, None);
        assert!(
            finished.filter_groups[1].ops.end < finished.filter_groups[0].ops.end,
            "bake order is increasing range end, so the inner group bakes first",
        );
    }

    /// `inner_chains` is exactly "some op in the range rides another scroll
    /// chain" — the one condition that makes a bake depend on a scroll
    /// offset.
    #[test]
    fn inner_chains_reports_only_a_differing_scroll_chain() {
        // Content on the group's own chain: the group and its content move
        // together, so the bake is offset-independent.
        let mut same = assembly();
        same.push_filter(group(scrolled(0)));
        same.push_op(push(scrolled(0)));
        same.pop_filter();
        assert!(!same.finish().filter_groups[0].inner_chains);

        // Content on a chain the group is not on: the content slides under
        // the blur.
        let mut differing = assembly();
        differing.push_filter(group(ComposeChain::default()));
        differing.push_op(push(scrolled(0)));
        differing.pop_filter();
        assert!(differing.finish().filter_groups[0].inner_chains);

        // An animation chain is not a scroll chain: a bake samples no
        // instant, so it cannot depend on one.
        let mut animated = assembly();
        animated.push_filter(group(ComposeChain::default()));
        animated.push_op(push(ComposeChain {
            scroll: None,
            animation: Some(0),
        }));
        animated.pop_filter();
        assert!(!animated.finish().filter_groups[0].inner_chains);
    }

    /// One fragment drawing a teal square, so a replay has real content to
    /// append.
    fn fragment() -> Scene {
        let mut scene = Scene::default();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            crate::vello::peniko::Color::from_rgb8(0, 128, 128),
            None,
            &Rect::new(0.0, 0.0, 8.0, 8.0),
        );
        scene
    }

    fn replay_program(
        program: &[ComposeOp],
        groups: &[FilterGroup],
        filtered: &[Option<ImageData>],
    ) -> Scene {
        let fragments = [fragment()];
        let mut scene = Scene::default();
        replay(
            &mut scene,
            &fragments,
            program,
            &[],
            &[],
            groups,
            filtered,
            &[],
            &[],
            1.0,
            &|_| None,
        );
        scene
    }

    /// The bracket ops encode nothing at all when no texture was baked: the
    /// raw fallback's encoding is byte-for-byte the encoding of the same
    /// program without the bracket.
    ///
    /// This is the property the fallback rests on. A bracket that encoded
    /// anything — even a redundant transform tag — would make "no GPU" and
    /// "over budget" observable in the drawing rather than only in the blur.
    #[test]
    fn a_filter_bracket_encodes_nothing_without_a_baked_texture() {
        let bare = [
            push(ComposeChain::default()),
            ComposeOp::Fragment {
                index: 0,
                chain: ComposeChain::default(),
            },
            ComposeOp::Pop,
        ];
        let bracketed = [
            push(ComposeChain::default()),
            ComposeOp::PushFilter { index: 0 },
            ComposeOp::Fragment {
                index: 0,
                chain: ComposeChain::default(),
            },
            ComposeOp::PopFilter,
            ComposeOp::Pop,
        ];
        let mut group = group(ComposeChain::default());
        group.ops = 2..3;
        let groups = [group];

        let without = replay_program(&bare, &[], &[]);
        for filtered in [&[][..], &[None][..]] {
            let with = replay_program(&bracketed, &groups, filtered);
            crate::paint::equivalence::assert_scenes_identical(&with, &without);
        }
    }

    /// With a texture the bracket draws it once, at the group's own device
    /// rect, and the group's ops are skipped rather than drawn underneath.
    #[test]
    fn a_baked_group_is_drawn_once_and_its_ops_skipped() {
        let program = [
            push(ComposeChain::default()),
            ComposeOp::PushFilter { index: 0 },
            ComposeOp::Fragment {
                index: 0,
                chain: ComposeChain::default(),
            },
            ComposeOp::PopFilter,
            ComposeOp::Pop,
        ];
        let mut group = group(ComposeChain::default());
        group.ops = 2..3;
        let groups = [group];
        let baked = ImageData {
            data: crate::vello::peniko::Blob::new(Arc::new([0_u8, 0, 0, 0])),
            format: crate::vello::peniko::ImageFormat::Rgba8,
            alpha_type: crate::vello::peniko::ImageAlphaType::AlphaPremultiplied,
            width: 8,
            height: 8,
        };

        let raw = replay_program(&program, &groups, &[None]);
        let composed = replay_program(&program, &groups, &[Some(baked)]);
        assert_eq!(
            composed.encoding().resources.patches.len(),
            1,
            "the group's texture, once",
        );
        assert_eq!(
            raw.encoding().resources.patches.len(),
            0,
            "and none in the fallback",
        );
        assert_eq!(
            composed.encoding().n_open_clips,
            0,
            "skipping the range leaves the layer stack balanced",
        );
        assert_eq!(
            composed.encoding().draw_tags.len(),
            raw.encoding().draw_tags.len(),
            "one image fill replaces one content fill",
        );
    }

    /// A nested group's own chain counts as an op's chain, so an inner
    /// scroller reaches the outer group through the bracket alone.
    #[test]
    fn a_nested_group_reports_its_own_chain_to_the_group_around_it() {
        let mut assembly = assembly();
        assembly.push_filter(group(ComposeChain::default()));
        assembly.push_filter(group(scrolled(0)));
        assembly.pop_filter();
        assembly.pop_filter();
        let finished = assembly.finish();
        assert!(
            finished.filter_groups[0].inner_chains,
            "the outer group's range holds a bracket on another chain",
        );
    }
}
