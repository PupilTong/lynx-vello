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
//! Scroll translation is the sum of the chain's local slot offsets, each
//! snapped to the device pixel grid and mapped through the scrollport's
//! transform. Sticky displacement is sampled from those same offsets and
//! added to the box, its contents, and its own clips.
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
//!
//! [`ComposeOp::PushBackdrop`] is the `backdrop-filter` half of the same
//! machinery, and the one op whose range points *backwards*. Its entry sits
//! in the same side table and its range is everything already painted inside
//! the element's nearest Backdrop Root ancestor, so its bake reads program
//! ops that precede it rather than ops it brackets — there is no matching
//! pop. With a texture the op draws it, shaped by the element's own rounded
//! border box, as the first thing inside the element's group; without one it
//! draws nothing at all, and the unfiltered backdrop the op sits on top of
//! is what shows.

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
use crate::visual::{AnimationSample, ScrollSlot, StickySample};

/// The compose-time coordinate context one op or fragment rides: the scroll
/// chain whose translations move it, the sticky chain constrained by those
/// offsets, and the animation chain whose sampled deltas move it.
/// Scroll and sticky translations always apply outside animation deltas
/// — export eligibility refuses a scroll container inside an animated
/// subtree, so the two never interleave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ComposeChain {
    pub(crate) scroll: Option<u32>,
    pub(crate) animation: Option<u32>,
    pub(crate) sticky: Option<u32>,
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
    /// Draw `filter_groups[index]`'s baked backdrop, shaped by the element's
    /// rounded border box, and then that entry's post-blur passes over it.
    ///
    /// Unlike [`Self::PushFilter`] this opens nothing and skips nothing: the
    /// entry's range lies *before* this op. Without a texture it encodes
    /// nothing, which leaves the unfiltered backdrop showing through.
    PushBackdrop {
        index: u32,
    },
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
            Self::PushFilter { index } | Self::PushBackdrop { index } => {
                Some(groups[*index as usize].chain)
            }
            Self::Pop | Self::PopFilter => None,
        }
    }
}

/// What a [`FilterGroup`] needs beyond σ and a rect to be a
/// `backdrop-filter` entry rather than a `filter: blur()` group.
///
/// Its presence is what makes an entry a backdrop: the bake pops the layers
/// its backward range left open, draws the `before` passes over the whole
/// bake rect, and mirrors at the rect's edges rather than reading the
/// transparent black a `filter` group's 3σ margin holds.
#[derive(Debug)]
pub(crate) struct Backdrop {
    /// The element's border box with its radii, in the element's own CSS px.
    /// Both the shape the texture is drawn through and the bound of the
    /// `after` passes.
    pub(crate) shape: BoxShape,
    /// Element-local CSS px to device px within the entry's chain — the
    /// walker's `scale * local`.
    pub(crate) transform: Affine,
    /// The passes preceding the list's first `blur()`, drawn inside the bake.
    pub(crate) before: Vec<crate::paint::filters::Pass>,
    /// The passes following it, drawn over the composed backdrop.
    pub(crate) after: Vec<crate::paint::filters::Pass>,
    /// Whether some op in the range rides an animation chain other than the
    /// entry's — the one condition under which the bake's pixels depend on
    /// the timeline reading, and therefore the one condition under which an
    /// animation tick invalidates the bake.
    pub(crate) inner_animations: bool,
    /// Layers the range leaves open at its end: the group scopes between the
    /// Backdrop Root and this element. The bake pops exactly this many, so
    /// the `before` passes do not land inside one.
    pub(crate) open_pushes: u32,
}

/// One baked entry: the ops whose composed pixels are baked offscreen,
/// filtered, and drawn back as one image.
///
/// Two properties produce one: `filter: blur()`, whose range is the ops the
/// entry *brackets*, and `backdrop-filter`, whose range is the ops already
/// painted *before* it inside its Backdrop Root — [`Self::backdrop`] is
/// which.
///
/// Everything here is decided at commit time on the document's thread, in
/// device pixels, and carries no GPU resource: a frame stays `Send + Sync`
/// and device-free. `rect` is integer-valued, so the bake's own render
/// target size is `rect`'s and the texture composes at an integer offset —
/// which is why nearest sampling reproduces it exactly on an unanimated
/// chain. For a `filter: blur()` group it already includes the 3σ ink
/// margin; for a backdrop it is exactly the element's transformed border
/// box, because `backdrop-filter` enlarges no ink overflow.
#[derive(Debug)]
pub struct FilterGroup {
    /// The blur's standard deviation, in device px. Zero is a real value for
    /// a backdrop — a colour-only list still bakes — and never occurs on a
    /// `filter: blur()` group.
    pub sigma: f32,
    /// The device-px region baked, integer-valued.
    pub rect: Rect,
    /// The chain the *texture* composes under. Content inside the range may
    /// ride inner scroll or sticky chains; see [`Self::inner_chains`].
    pub(crate) chain: ComposeChain,
    /// For a `filter: blur()` group, the ops strictly between its
    /// `PushFilter` and its `PopFilter`. For a backdrop, the ops from its
    /// Backdrop Root's content start up to the element's own scope open.
    pub(crate) ops: Range<u32>,
    /// Whether some op in `ops` rides a scroll or sticky chain other than `chain` —
    /// the one condition under which the bake's pixels depend on a scroll
    /// offset, and therefore the one condition under which a scroll
    /// invalidates the bake.
    pub(crate) inner_chains: bool,
    /// The group this one nests inside, so the assembly needs no open-filter
    /// stack of its own.
    pub(crate) parent: Option<u32>,
    /// Set exactly for a `backdrop-filter` entry; see [`Backdrop`].
    pub(crate) backdrop: Option<Backdrop>,
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
            backdrop: None,
        }
    }

    /// A `backdrop-filter` entry over `rect` at `sigma`, before the assembly
    /// records its backward range.
    pub(crate) fn with_backdrop(
        sigma: f32,
        rect: Rect,
        chain: ComposeChain,
        backdrop: Backdrop,
    ) -> Self {
        Self {
            backdrop: Some(backdrop),
            ..Self::new(sigma, rect, chain)
        }
    }

    /// Whether this entry filters a backdrop rather than a group's own
    /// pixels — which is what decides its bake's edge mode.
    #[must_use]
    pub fn is_backdrop(&self) -> bool {
        self.backdrop.is_some()
    }

    /// Whether this entry's baked pixels depend on the timeline reading:
    /// some op in its range rides an animation chain the entry does not.
    ///
    /// Only a backdrop can answer `true`. A `filter: blur()` group's range
    /// is its own subtree, and an animated element's whole subtree rides its
    /// own slot, so a group and its content are never on different animation
    /// chains.
    #[must_use]
    pub fn samples_animations(&self) -> bool {
        self.backdrop
            .as_ref()
            .is_some_and(|backdrop| backdrop.inner_animations)
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

    /// Seals any open fragment and answers the program length: the boundary
    /// "everything painted so far" ends at.
    ///
    /// Records nothing of its own — the seal produces exactly the `Fragment`
    /// op the next [`Self::push_op`] would have produced, at the same index —
    /// so a walk that calls this leaves a byte-identical program.
    pub(crate) fn content_boundary(&mut self) -> u32 {
        self.seal_fragment();
        u32::try_from(self.program.len()).expect("a frame cannot hold 2^32 program ops")
    }

    /// Records a `backdrop-filter` entry over the already-emitted `ops`,
    /// answering whether one was recorded.
    ///
    /// An empty range records nothing and answers `false`: there is nothing
    /// behind the element to filter, so the op would bake a transparent
    /// texture and draw it over nothing.
    pub(crate) fn push_backdrop(&mut self, mut entry: FilterGroup, ops: Range<u32>) -> bool {
        self.seal_fragment();
        if ops.start >= ops.end {
            return false;
        }
        debug_assert!(
            ops.end as usize <= self.program.len(),
            "a backdrop's range ends at or before the op that draws it",
        );
        let chain = entry.chain;
        let (inner_chains, inner_animations, open_pushes) = self.scan_backdrop(&ops, chain);
        let index =
            u32::try_from(self.filter_groups.len()).expect("a frame cannot hold 2^32 filters");
        entry.parent = self.open_filter;
        entry.ops = ops;
        entry.inner_chains = inner_chains;
        if let Some(backdrop) = entry.backdrop.as_mut() {
            backdrop.inner_animations = inner_animations;
            backdrop.open_pushes = open_pushes;
        } else {
            debug_assert!(false, "push_backdrop is only called with a backdrop entry");
        }
        self.filter_groups.push(entry);
        self.program.push(ComposeOp::PushBackdrop { index });
        true
    }

    /// One pass over a backdrop's backward range: whether it holds another
    /// scroll or sticky chain, whether it holds another animation chain, and how many
    /// layers it leaves open at its end.
    fn scan_backdrop(&self, ops: &Range<u32>, chain: ComposeChain) -> (bool, bool, u32) {
        let mut scrolls = false;
        let mut animations = false;
        let mut depth = 0_i64;
        for op in &self.program[ops.start as usize..ops.end as usize] {
            match op {
                ComposeOp::Push { .. } => depth += 1,
                ComposeOp::Pop => depth -= 1,
                _ => {}
            }
            if let Some(op) = op.chain(&self.filter_groups) {
                scrolls |= op.scroll != chain.scroll || op.sticky != chain.sticky;
                animations |= op.animation != chain.animation;
            }
        }
        // The range starts and ends with an empty clip stack (every group
        // scope restarts clip chains), and no scope enclosing the range can
        // close inside it, so nothing in here pops a layer it did not push.
        debug_assert!(
            depth >= 0,
            "a backdrop's range pops a layer it never pushed"
        );
        (
            scrolls,
            animations,
            u32::try_from(depth.max(0)).expect("a frame cannot nest 2^32 layers"),
        )
    }

    /// Closes the innermost open filter group, completing its op range and
    /// deciding whether anything inside it rides another scroll or sticky chain.
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
                .is_some_and(|op| op.scroll != chain.scroll || op.sticky != chain.sticky)
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
/// sampled deltas (innermost applied first), then the scroll and sticky
/// translations.
pub(crate) fn chain_transform(
    slots: &[ScrollSlot],
    samples: &[AnimationSample],
    stickies: &[StickySample],
    chain: ComposeChain,
    ratio: f32,
    offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) -> Affine {
    let translation = chain_translation(slots, chain.scroll, ratio, offset_of)
        - sticky_translation(stickies, chain.sticky);
    Affine::translate((-f64::from(translation.x), -f64::from(translation.y)))
        * animation_deltas(samples, chain.animation)
}

/// Cumulative sticky displacement in viewport CSS pixels.
pub(crate) fn sticky_translation(samples: &[StickySample], chain: Option<u32>) -> Vector2D<f32> {
    chain.map_or_else(Vector2D::zero, |index| samples[index as usize].translation)
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

/// One chain's compose translation in viewport CSS px: the sum of its local
/// offsets, each snapped to the device pixel grid and transformed by its slot.
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
        sum += slot.viewport_translation(snap_offset(offset, ratio));
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
    stickies: &[StickySample],
    ratio: f32,
    offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) {
    let transform = device_transform(slots, samples, stickies, ratio, offset_of);
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
    stickies: &'a [StickySample],
    ratio: f32,
    offset_of: &'a dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) -> impl Fn(ComposeChain) -> Affine + 'a {
    let scale = f64::from(ratio);
    move |chain| {
        let css = chain_transform(slots, samples, stickies, chain, ratio, offset_of);
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
    /// that entry, or `None` for the unfiltered fallback.
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
/// unblurred fallback. A `PushBackdrop` with a texture draws it through the
/// element's border box; one without encodes nothing, and the unfiltered
/// backdrop underneath is what shows.
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
            ComposeOp::PushBackdrop { index: slot } => {
                let slot = *slot as usize;
                let entry = &filter_groups[slot];
                if let (Some(Some(image)), Some(backdrop)) = (filtered.get(slot), &entry.backdrop) {
                    draw_backdrop(scene, entry, backdrop, image, device_transform);
                }
            }
        }
        index += 1;
    }
}

/// Draws one baked backdrop: the texture through the element's own rounded
/// border box, then the entry's post-blur passes over it.
///
/// The texture is the backdrop already filtered, in device px at `rect`'s
/// origin, so the brush transform undoes the element's own map and puts the
/// image back where it was baked from: `transform * brush_transform` is
/// exactly `device_transform(chain) * translate(rect.origin)`. That is the
/// spec's "inverse of the element's transforms, then the element's own
/// transforms again", performed once rather than twice.
///
/// No clip layer is opened for the crop. The shape *is* the fill, which
/// keeps the op a single draw and keeps a fragment cut from ever landing
/// between a push and its pop.
fn draw_backdrop(
    scene: &mut Scene,
    entry: &FilterGroup,
    backdrop: &Backdrop,
    image: &ImageData,
    device_transform: &dyn Fn(ComposeChain) -> Affine,
) {
    let outer = device_transform(entry.chain);
    let transform = outer * backdrop.transform;
    let brush_transform =
        backdrop.transform.inverse() * Affine::translate((entry.rect.x0, entry.rect.y0));
    // Nearest reproduces the bake byte for byte wherever the composed
    // placement is a whole-pixel translation, which is every chain a scroll
    // offset snaps; a sampled animation delta can land it anywhere.
    let quality = if entry.chain.animation.is_none() && is_integer_translation(outer) {
        ImageQuality::Low
    } else {
        ImageQuality::Medium
    };
    let brush = BrushRef::Image(ImageBrush {
        image,
        sampler: ImageSampler {
            x_extend: Extend::Pad,
            y_extend: Extend::Pad,
            quality,
            alpha: 1.0,
        },
    });
    with_shape!(&backdrop.shape, |s| scene.fill(
        Fill::NonZero,
        transform,
        brush,
        Some(brush_transform),
        s
    ));
    crate::paint::filters::draw_passes_shape(scene, &backdrop.after, &backdrop.shape, transform);
}

/// Whether `affine` moves an image by a whole number of device pixels and
/// nothing else — the condition under which nearest sampling is exact.
fn is_integer_translation(affine: Affine) -> bool {
    let coefficients = affine.as_coeffs();
    // Exact comparison on purpose: the question is whether nearest sampling
    // reproduces the bake byte for byte, and anything but the identity linear
    // part and a whole-pixel offset means it does not.
    coefficients[..4] == [1.0, 0.0, 0.0, 1.0]
        && coefficients[4].fract() == 0.0
        && coefficients[5].fract() == 0.0
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
            sticky: None,
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
            sticky: None,
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

    fn backdrop_entry(chain: ComposeChain) -> FilterGroup {
        FilterGroup::with_backdrop(
            2.0,
            Rect::new(0.0, 0.0, 8.0, 8.0),
            chain,
            Backdrop {
                shape: BoxShape::Rect(Rect::new(0.0, 0.0, 8.0, 8.0)),
                transform: Affine::IDENTITY,
                before: Vec::new(),
                after: Vec::new(),
                inner_animations: false,
                open_pushes: 0,
            },
        )
    }

    /// A backdrop's range is everything already recorded from the boundary it
    /// is handed, and the flags are read off exactly that slice.
    #[test]
    fn a_backdrop_records_the_program_behind_it() {
        let mut assembly = assembly();
        let root_start = assembly.content_boundary();
        assert_eq!(root_start, 0, "nothing precedes the frame root");
        assembly.push_op(push(ComposeChain::default()));
        assembly.push_op(push(scrolled(0)));
        assembly.push_op(ComposeOp::Pop);
        let end = assembly.content_boundary();
        assert!(
            assembly.push_backdrop(backdrop_entry(ComposeChain::default()), root_start..end),
            "a non-empty range records an entry",
        );
        let finished = assembly.finish();

        let entry = &finished.filter_groups[0];
        assert_eq!(entry.ops, 0..3, "the whole prefix");
        assert!(entry.is_backdrop());
        assert!(
            entry.inner_chains,
            "the prefix holds an op on another scroll chain",
        );
        assert!(
            !entry.samples_animations(),
            "and none on another animation chain",
        );
        let backdrop = entry.backdrop.as_ref().expect("a backdrop entry");
        assert_eq!(
            backdrop.open_pushes, 1,
            "two pushes and one pop leave one layer open",
        );
        assert!(matches!(
            finished.program[3],
            ComposeOp::PushBackdrop { index: 0 }
        ));
        assert_eq!(
            finished.program.len(),
            4,
            "and the op opens nothing it has to close",
        );
    }

    /// An animation chain the entry is not on is what makes a bake depend on
    /// the timeline reading — and the only thing that does.
    #[test]
    fn inner_animations_reports_only_a_differing_animation_chain() {
        let animated = ComposeChain {
            scroll: None,
            animation: Some(0),
            sticky: None,
        };
        for (chain, op, expected) in [
            (ComposeChain::default(), animated, true),
            (animated, animated, false),
            (ComposeChain::default(), scrolled(0), false),
        ] {
            let mut assembly = assembly();
            assembly.push_op(push(op));
            let end = assembly.content_boundary();
            assembly.push_backdrop(backdrop_entry(chain), 0..end);
            assert_eq!(
                assembly.finish().filter_groups[0].samples_animations(),
                expected,
                "entry on {chain:?} over an op on {op:?}",
            );
        }
    }

    /// Nothing painted behind the element is nothing to filter: the entry is
    /// refused outright rather than recorded with an empty range, so the
    /// frame's filter table stays empty and the bake pre-step stays skipped.
    #[test]
    fn an_empty_range_records_no_backdrop_at_all() {
        let mut assembly = assembly();
        let start = assembly.content_boundary();
        assert!(!assembly.push_backdrop(backdrop_entry(ComposeChain::default()), start..start));
        let finished = assembly.finish();
        assert!(finished.filter_groups.is_empty());
        assert!(finished.program.is_empty());
    }

    /// Without a texture the op encodes nothing: the unfiltered backdrop the
    /// element sits on is what shows, byte for byte.
    #[test]
    fn a_backdrop_op_encodes_nothing_without_a_baked_texture() {
        let bare = [ComposeOp::Fragment {
            index: 0,
            chain: ComposeChain::default(),
        }];
        let with_op = [
            ComposeOp::Fragment {
                index: 0,
                chain: ComposeChain::default(),
            },
            ComposeOp::PushBackdrop { index: 0 },
        ];
        let mut entry = backdrop_entry(ComposeChain::default());
        entry.ops = 0..1;
        let entries = [entry];

        let without = replay_program(&bare, &[], &[]);
        for filtered in [&[][..], &[None][..]] {
            let with = replay_program(&with_op, &entries, filtered);
            crate::paint::equivalence::assert_scenes_identical(&with, &without);
        }
    }

    /// With a texture the op draws it once, through the element's own shape,
    /// and each post-blur pass adds one blend layer over it.
    #[test]
    fn a_baked_backdrop_draws_one_fill_and_its_after_passes() {
        let program = [
            ComposeOp::Fragment {
                index: 0,
                chain: ComposeChain::default(),
            },
            ComposeOp::PushBackdrop { index: 0 },
        ];
        let baked = ImageData {
            data: crate::vello::peniko::Blob::new(Arc::new([0_u8, 0, 0, 0])),
            format: crate::vello::peniko::ImageFormat::Rgba8,
            alpha_type: crate::vello::peniko::ImageAlphaType::AlphaPremultiplied,
            width: 8,
            height: 8,
        };

        let plain = {
            let mut entry = backdrop_entry(ComposeChain::default());
            entry.ops = 0..1;
            let entries = [entry];
            replay_program(&program, &entries, &[Some(baked.clone())])
        };
        assert_eq!(
            plain.encoding().resources.patches.len(),
            1,
            "the entry's texture, once",
        );
        assert_eq!(
            plain.encoding().n_open_clips,
            0,
            "and the op leaves the layer stack balanced",
        );

        let with_passes = {
            let mut entry = backdrop_entry(ComposeChain::default());
            entry.ops = 0..1;
            entry.backdrop.as_mut().expect("a backdrop").after = crate::paint::filters::passes(
                &[stylo::values::computed::effects::Filter::Brightness(
                    stylo::values::generics::NonNegative(0.5),
                )],
                0..1,
            );
            let entries = [entry];
            replay_program(&program, &entries, &[Some(baked)])
        };
        assert_eq!(
            with_passes.encoding().draw_tags.len(),
            plain.encoding().draw_tags.len() + 3,
            "one post-blur pass is a blend layer, its flat fill, and the pop",
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
