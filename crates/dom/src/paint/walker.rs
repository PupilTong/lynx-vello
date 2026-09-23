#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    reason = "CSS/style geometry is f32 while Vello/Kurbo geometry is f64"
)]

//! The scene walker: flat paint-order items → vello layer stack.
//!
//! Three stack disciplines interleave on vello's single layer stack:
//!
//! 1. **Item clip chains** ([`crate::visual::ClipNode`]) — pushed lazily
//!    per item by diffing the item's chain against what is on the stack,
//!    so runs of items sharing clips pay nothing. Chains restart inside
//!    every group scope: an item's full chain is (re-)pushed inside its
//!    innermost group, which keeps escape semantics (a fixed descendant of
//!    an opacity group is grouped but not clipped by the group's ancestors)
//!    and keeps group blend layers from opening inside a clip layer
//!    (vello [#1198](https://github.com/linebender/vello/issues/1198) —
//!    re-pushing intersecting clips is idempotent, so correctness is
//!    unaffected). The precise #1198 invariant maintained crate-wide: a
//!    blend layer's *immediate* enclosing layer is always a real
//!    (isolating) layer, never a clip layer — clip layers share their
//!    parent's buffer, so a blend directly inside one reads pixels outside
//!    the clip. Fragment painters that need a blend under an item clip
//!    (inset shadows) interpose their own full `SrcOver` layer first.
//! 2. **Group scopes** ([`crate::visual::RenderLayer`]) — a stacking context with group effects
//!    pushes, outermost to innermost: the effect layer (blend mode + `opacity` alpha, clipped to
//!    the group's prepass-computed content bounds), a `clip-path` layer (a full `push_layer`, not a
//!    clip layer, per the #1198 rule above), and for `mask-image` the alpha-mask sandwich — mask
//!    pattern drawn first, then a `Compose::SrcIn` layer holding the content, and innermost of all
//!    a `filter: blur()` group's bracket. Filter adjustments draw at scope close, inside the
//!    innermost layer, after in-scope clips pop: the ones before the list's first `blur()` inside
//!    the blur bracket, the ones after it outside it. Composite order on pop is therefore pre-blur
//!    adjustments → blur → post-blur adjustments → mask → clip-path → opacity/blend — clip and mask
//!    are both intersective alpha ops, so the swap versus the spec's filter → clip → mask is
//!    unobservable.
//! 3. **Fragments** — per element box: outset shadows, background, inset shadows, replaced content
//!    (above the inset shadows — css-backgrounds-3 §7.4.1 paints inner shadows immediately above
//!    the background, below content, which is why an inset shadow on an `<img>` is invisible in
//!    browsers), border, outline (outline painting with its element rather than Appendix E step 10
//!    is a recorded v1 limit); per text run: the retained Parley layout under the parent element's
//!    style.
//!
//! # Culling
//!
//! [`plan_frame`] decides, once per frame, which items can put ink where the
//! scene will be looked at, and [`walk`] skips the fragment encode for the
//! rest. The retained scene is only ever rendered into a target covering the
//! document's own viewport, so an item that reaches neither the viewport nor
//! anything its clip chain admits contributes nothing at any device scale.
//!
//! Five disciplines keep it sound.
//!
//! - **Only the encode is skipped.** Scope open and close are driven by item index, and the group
//!   bounds `plan_frame` produces are computed from every item, culled or not. Narrowing a group's
//!   bounds by the cull decision would move the `push_layer` rect and change the encoding of
//!   content nothing is culling.
//! - **Culling needs a proof, uncertainty paints.** An item is discarded only when its box,
//!   inflated by a reach that bounds every fragment painter, maps entirely outside the admitted
//!   region under the exact matrix the painter would have used — `plan_frame` hands that matrix to
//!   [`paint_item`], so painter and culler cannot disagree about geometry. A non-finite bound, or a
//!   reach that cannot be established, paints.
//! - **A blur admits its own reach.** The bake of a `filter: blur()` group is 3σ larger than the
//!   group's content on every side, so every item inside one is tested against a region grown by
//!   the sum of 3σ over each enclosing filtered layer, and the group's own layer bounds are grown
//!   the same way *before* the viewport intersection — an element straddling the viewport edge
//!   therefore still bakes the margin its visible pixels read from.
//! - **Moving content is bounded only by what moves with it.** Below an animation node with a
//!   transform track the viewport bounds nothing, since the sampled delta can carry content
//!   anywhere. Clips and encode windows inside the moving subtree move with it and still bound, so
//!   a list inside an animated card encodes only its own window, and a moving group's bounds are
//!   held to the same clips. The export's extent budget
//!   ([`crate::visual::frame::MAX_MOVING_EXTENT_VIEWPORTS`]) bounds each moving element's own
//!   extent, not how many there are: every exported row of a long list encodes and composes,
//!   whatever part of the list is on screen.
//! - **Text runs are never culled by geometry.** [`text::extent`] bounds the authored reaches
//!   (`text-shadow` offset, half the `-webkit-text-stroke` width) exactly, but a run's `size` is
//!   its line box, and glyph ink leaves that box by font ascent and descent, synthetic oblique
//!   shear, synthetic emboldening, side bearings, and metric-positioned decoration bands. None of
//!   those is bounded by anything this crate controls, since the face is author-supplied. A run is
//!   therefore discarded only when its clip chain admits no pixels at all, which is a fact about
//!   the frame rather than about the font.
//!
//! Hit testing is unaffected: it reads [`PaintOrder`], and the cull decision
//! lives in [`Scratch`], which never leaves the painter. A point outside the
//! viewport still answers with the element drawn there.

use euclid::default::{Size2D, Vector2D};

use crate::Document;
use crate::paint::compose::{CapturedShape, ComposeAssembly, ComposeOp, FilterGroup};
use crate::paint::shape::{BoxShape, with_shape};
use crate::paint::{
    BoxFragment, PathScratch, background, border, convert, filters, mask, shadow, text,
};
use crate::render::image::ImageRegistry;
use crate::vello::Scene;
use crate::vello::kurbo::{Affine, Point, Rect};
use crate::vello::peniko::{BlendMode, Compose, Fill, Mix};
use crate::visual::space::{self, SpaceKind, nearest_scroll, nearest_sticky};
use crate::visual::{AutoBox, ClipNode, PaintItem, PaintItemKind, PaintOrder, RenderLayer, Space};

/// Where one walk's output goes.
///
/// `Monolithic` is the pre-compose shape — one scene, everything inline —
/// kept for the equivalence tests; production encodes through `Compose`, where
/// walker-level pushes become program ops and content between them lands in
/// per-space fragments.
pub(crate) enum WalkSink<'s> {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "constructed only by the equivalence tests' monolithic walk"
        )
    )]
    Monolithic(&'s mut Scene, &'s dyn crate::render::image::FrameImages),
    Compose(&'s mut ComposeAssembly),
}

impl WalkSink<'_> {
    /// The scene content in `space` encodes into.
    pub(super) fn scene_for(&mut self, space: Option<u32>) -> &mut Scene {
        match self {
            Self::Monolithic(scene, _) => scene,
            Self::Compose(assembly) => assembly.fragment_for(space),
        }
    }

    /// One image draw.
    ///
    /// In compose mode it is a program op the painter resolves and encodes.
    /// In the monolithic mode the equivalence tests use it is encoded inline
    /// against that walk's own pixel source, so a culling regression in image
    /// draws stays observable to the culling oracle.
    pub(super) fn image(&mut self, space: Option<u32>, draw: crate::paint::compose::ImageDraw) {
        match self {
            Self::Monolithic(scene, pixels) => {
                if let Some(data) = pixels.read(&draw.image, draw.size_hint()) {
                    crate::paint::compose::encode_image(scene, &draw, Affine::IDENTITY, &data);
                }
            }
            Self::Compose(assembly) => assembly.push_image(space, draw),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors vello's push_layer plus the compose tags"
    )]
    pub(super) fn push_layer_rect(
        &mut self,
        space: Option<u32>,
        alpha_animation: Option<u32>,
        fill: Fill,
        blend: BlendMode,
        alpha: f32,
        transform: Affine,
        rect: Rect,
    ) {
        match self {
            Self::Monolithic(scene, _) => scene.push_layer(fill, blend, alpha, transform, &rect),
            Self::Compose(assembly) => assembly.push_op(ComposeOp::Push {
                clip_only: false,
                fill,
                blend,
                alpha,
                transform,
                shape: CapturedShape::Rect(rect),
                space,
                alpha_animation,
            }),
        }
    }

    pub(super) fn push_layer_box(
        &mut self,
        space: Option<u32>,
        fill: Fill,
        blend: BlendMode,
        alpha: f32,
        transform: Affine,
        shape: BoxShape,
    ) {
        match self {
            Self::Monolithic(scene, _) => {
                with_shape!(&shape, |s| scene
                    .push_layer(fill, blend, alpha, transform, s));
            }
            Self::Compose(assembly) => assembly.push_op(ComposeOp::Push {
                clip_only: false,
                fill,
                blend,
                alpha,
                transform,
                shape: CapturedShape::Box(shape),
                space,
                alpha_animation: None,
            }),
        }
    }

    pub(super) fn push_clip_box(
        &mut self,
        space: Option<u32>,
        fill: Fill,
        transform: Affine,
        shape: BoxShape,
    ) {
        match self {
            Self::Monolithic(scene, _) => {
                with_shape!(&shape, |s| scene.push_clip_layer(fill, transform, s));
            }
            Self::Compose(assembly) => assembly.push_op(ComposeOp::Push {
                clip_only: true,
                fill,
                blend: BlendMode::new(Mix::Normal, Compose::SrcOver),
                alpha: 1.0,
                transform,
                shape: CapturedShape::Box(shape),
                space,
                alpha_animation: None,
            }),
        }
    }

    fn push_clip_empty(&mut self, space: Option<u32>) {
        match self {
            Self::Monolithic(scene, _) => {
                scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::ZERO);
            }
            Self::Compose(assembly) => assembly.push_op(ComposeOp::Push {
                clip_only: true,
                fill: Fill::NonZero,
                blend: BlendMode::new(Mix::Normal, Compose::SrcOver),
                alpha: 1.0,
                transform: Affine::IDENTITY,
                shape: CapturedShape::Rect(Rect::ZERO),
                space,
                alpha_animation: None,
            }),
        }
    }

    pub(super) fn pop(&mut self) {
        match self {
            Self::Monolithic(scene, _) => scene.pop_layer(),
            Self::Compose(assembly) => assembly.push_op(ComposeOp::Pop),
        }
    }

    /// Opens a `filter: blur()` group, answering whether one was recorded.
    ///
    /// The monolithic sink records none: it has no side table and no
    /// composer, so its walk encodes the group's content inline and the
    /// frame is simply unblurred — which is what makes it still a usable
    /// culling oracle for a filtered page.
    fn push_filter(&mut self, group: FilterGroup) -> bool {
        match self {
            Self::Monolithic(..) => false,
            Self::Compose(assembly) => {
                assembly.push_filter(group);
                true
            }
        }
    }

    fn pop_filter(&mut self, spaces: &[Space]) {
        match self {
            Self::Monolithic(..) => {}
            Self::Compose(assembly) => assembly.pop_filter(spaces),
        }
    }

    /// Seals any open fragment and answers the program length — the boundary
    /// a `backdrop-filter` range starts or ends at. Zero for the monolithic
    /// sink, which has no program; its ranges are therefore always empty and
    /// it records no backdrop at all.
    fn content_boundary(&mut self) -> u32 {
        match self {
            Self::Monolithic(..) => 0,
            Self::Compose(assembly) => assembly.content_boundary(),
        }
    }

    /// Records a `backdrop-filter` entry over the already-emitted `ops`,
    /// answering whether one was recorded.
    ///
    /// The monolithic sink records none, for the same reason it records no
    /// filter group: with no side table and no composer its walk simply
    /// leaves the backdrop unfiltered, which keeps it a usable culling
    /// oracle for a page that uses the property.
    fn push_backdrop(
        &mut self,
        entry: FilterGroup,
        ops: std::ops::Range<u32>,
        spaces: &[Space],
    ) -> bool {
        match self {
            Self::Monolithic(..) => false,
            Self::Compose(assembly) => assembly.push_backdrop(entry, ops, spaces),
        }
    }
}

/// Reused per-frame buffers.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    clip_stack: Vec<usize>,
    chain: Vec<usize>,
    scopes: Vec<Scope>,
    layer_bounds: Vec<Rect>,
    open_layers: Vec<usize>,
    bounds_acc: Vec<Option<Rect>>,
    /// Per layer, whether a transform curve on its space's path moves it:
    /// the viewport bounds nothing of such a group, so its bounds are held
    /// to [`Self::clip_extent`] instead. Index-parallel with
    /// [`PaintOrder::layers`].
    layer_moves: Vec<bool>,
    /// Per clip node, what its chain alone admits in its own space, with no
    /// viewport — filled only when some layer moves. It is independent of
    /// the cull rect, so a moving group's bounds stay the same whether or not
    /// culling is on. Index-parallel with [`PaintOrder::clips`].
    clip_extent: Vec<Admitted>,
    /// Per layer, the blur sigma of its `filter` in *viewport* CSS px — the
    /// element's own sigma scaled by its local-to-viewport map. Zero for
    /// every layer with no `blur()`. Index-parallel with
    /// [`PaintOrder::layers`].
    layer_sigma: Vec<f64>,
    /// Per item, the item-local to viewport-CSS-px map [`paint_item`] paints
    /// with, or `None` when the item encodes nothing — a non-invertible
    /// transform, or no reachable ink. Index-parallel with
    /// [`PaintOrder::items`].
    item_plan: Vec<Option<Affine>>,
    /// The per-frame cull geometry, shared with the relevance pass.
    plan: CullPlan,
    paths: PathScratch,
}

/// The frame-wide geometry the cull test is decided against: the region every
/// clip chain admits and how far every scroll slot's committed encode window
/// reaches.
///
/// It is a type of its own, and `pub(crate)`, because two consumers must
/// agree exactly: [`plan_frame`], which decides what the paint walk encodes,
/// and [`crate::visual::relevance`], which decides which
/// `content-visibility: auto` boxes are relevant. Relevance is defined *as*
/// the admitted region, so deriving it a second way would make "an element is
/// relevant wherever its contents could paint" an invariant to be tested
/// rather than one that holds by construction.
#[derive(Debug, Default)]
pub(crate) struct CullPlan {
    /// The admitted base region: the viewport plus its slack, or `None` with
    /// culling switched off entirely.
    cull: Option<Rect>,
    /// Per clip node, what its whole chain admits in the clip's **own**
    /// space, already intersected with `cull` pulled back into that space.
    /// [`Admitted::Nothing`] means the chain admits nothing: it leaves the
    /// cull rect, or one of its links has a non-invertible transform, which
    /// [`push_clip`] encodes as an empty clip. [`Admitted::Everything`] means
    /// a transform curve moves the chain and no committed geometry bounds it.
    /// Index-parallel with [`PaintOrder::clips`].
    clip_bounds: Vec<Admitted>,
    /// Per scroll slot, the committed encode window `(low, high)` — the
    /// offset range the culled encode must stay valid for. Index-parallel
    /// with [`PaintOrder::slots`].
    slot_windows: Vec<(Vector2D<f32>, Vector2D<f32>)>,
    /// Per group layer, the summed 3-sigma ink reach of that layer and every
    /// filtered layer outside it. Content inside a filtered group can put ink
    /// that far past its own box, so the region admitted for it grows by the
    /// sum — the reaches of nested blurs compose. Index-parallel with
    /// [`PaintOrder::layers`], resolved by one forward pass because a layer's
    /// parent is always an earlier entry.
    layer_inflate: Vec<f64>,
}

impl CullPlan {
    /// Resolves the whole plan for one frame against an admitted region, in
    /// viewport CSS px, or `None` to admit everything.
    fn resolve<T>(&mut self, document: &Document<T>, frame: &PaintOrder, cull: Option<Rect>) {
        self.cull = cull;
        self.slot_windows.clear();
        self.slot_windows.extend(
            frame
                .slots()
                .iter()
                .map(crate::visual::ScrollSlot::encode_window),
        );
        self.layer_inflate.clear();
        for (index, layer) in frame.layers().iter().enumerate() {
            debug_assert!(
                layer.parent.is_none_or(|parent| parent < index),
                "a group layer nests inside an earlier group layer",
            );
            let outer = layer
                .parent
                .map_or(0.0, |parent| self.layer_inflate[parent]);
            self.layer_inflate
                .push(outer + BLUR_INK_SIGMAS * layer_blur_sigma(document, layer));
        }
        let mut bounds = std::mem::take(&mut self.clip_bounds);
        match self.cull {
            Some(cull) => resolve_clips(
                &self.slot_windows,
                frame,
                Admitted::Region(cull),
                &mut bounds,
            ),
            None => bounds.clear(),
        }
        self.clip_bounds = bounds;
    }

    /// The same plan, resolved from a document's own device metrics — the
    /// cull rect the production walk uses.
    pub(crate) fn resolve_for<T>(&mut self, document: &Document<T>, frame: &PaintOrder) {
        let device = document.device();
        let ratio = f64::from(device.device_pixel_ratio().get());
        self.resolve(
            document,
            frame,
            Some(cull_rect(device.viewport_size(), ratio)),
        );
    }

    /// The region this frame's culling admits for content in `space` inside
    /// `clip` and inside group `layer`.
    fn admitted_for(
        &self,
        frame: &PaintOrder,
        clip: Option<usize>,
        space: Option<u32>,
        layer: Option<usize>,
    ) -> Admitted {
        if self.cull.is_none() {
            return Admitted::Everything;
        }
        // Every enclosing blur carries this content's ink 3 sigma further
        // out, so the region it may reach grows by their sum. That
        // over-admits a little near an inner clip, which is the safe
        // direction: culling needs a proof, uncertainty paints.
        let inflate = layer.map_or(0.0, |layer| self.layer_inflate[layer]);
        match admitted_region(self, frame, space, clip) {
            Admitted::Region(region) => Admitted::Region(inflate_rect(region, inflate)),
            other => other,
        }
    }

    /// Whether this `content-visibility: auto` box can put ink in that
    /// region — the whole of the relevance test.
    ///
    /// It is [`plan_frame`]'s own first cull test, reached through the same
    /// [`Self::admitted_for`] and the same [`box_bounds`]: the one that lets
    /// a box already reaching the admitted region paint without computing any
    /// fragment reach. Everything uncertain (a singular transform, a
    /// non-finite bound, a moving animation node, culling switched off)
    /// answers `true`, because a cull needs a proof and relevance needs none.
    pub(crate) fn admits_auto_box(&self, frame: &PaintOrder, auto: &AutoBox) -> bool {
        let Some(local) = convert::item_affine(&auto.transform, auto.size) else {
            return true;
        };
        self.admitted_for(frame, auto.clip, auto.space, auto.layer)
            .reached_by(box_bounds(local, auto.size, 0.0))
    }
}

/// What a frame's culling admits for one piece of content, in the CSS px of
/// the space that content rides.
#[derive(Clone, Copy, Debug)]
enum Admitted {
    /// No proof is possible at all: culling is switched off, or a transform
    /// curve on the path can carry the content anywhere. Everything paints.
    Everything,
    /// The region content in this space may put ink in.
    Region(Rect),
    /// The clip chain admits nothing whatever.
    Nothing,
}

impl Admitted {
    /// `bounds` cut down to what this admits, or `None` when nothing of it
    /// is. Non-finite bounds are undecidable and pass whole.
    fn cut(self, bounds: Rect) -> Option<Rect> {
        match self {
            Self::Region(region) if is_finite(bounds) => {
                let both = bounds.intersect(region);
                (both.width() > 0.0 && both.height() > 0.0).then_some(both)
            }
            Self::Nothing => None,
            Self::Region(_) | Self::Everything => Some(bounds),
        }
    }

    /// Whether `bounds` reaches it. Undecidable bounds reach everything but
    /// [`Self::Nothing`]; see [`can_reach`], which this defers to so the
    /// non-finite rule has one definition.
    fn reached_by(self, bounds: Rect) -> bool {
        match self {
            Self::Everything => true,
            Self::Region(region) => can_reach(bounds, Some(region)),
            Self::Nothing => can_reach(bounds, None),
        }
    }
}

/// Device pixels of slack around the viewport before an item may be culled.
///
/// Three things add up to well under this. An embedder sizes its target
/// `round(css * device_pixel_ratio)` device px, up to half a device pixel past
/// the CSS viewport. Vello's area antialiasing samples across a whole pixel.
/// And the frame carries f32 geometry that is converted to f64 here.
const VIEWPORT_SLACK_DEVICE_PX: f64 = 2.0;

/// The two reaches around one item's box, in CSS px.
///
/// `layer` is what the group bounds are accumulated from. `cull` is never
/// smaller, and is infinite when the reach cannot be established at all.
#[derive(Clone, Copy, Debug)]
struct Extents {
    layer: f64,
    cull: f64,
}

/// The read-only context every step of one walk shares.
struct Painting<'a, T> {
    document: &'a Document<T>,
    frame: &'a PaintOrder,
    images: &'a ImageRegistry,
    /// The document's device scale, applied once at the root: the paint order
    /// is in viewport CSS px and the scene is in device px.
    scale: Affine,
    /// That scale as a scalar, for the device-pixel geometry a filter bake
    /// is recorded in.
    ratio: f64,
}

// Derived `Copy` would demand `T: Copy`, and the document's payload type has
// nothing to do with whether four references can be copied.
impl<T> Clone for Painting<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Painting<'_, T> {}

#[derive(Debug)]
struct Scope {
    layer: usize,
    base: usize,
    pushed: u32,
    filtered: bool,
    /// Whether [`open_scope`] recorded a filter group for this scope, which
    /// [`close_scope`] then has to close.
    blurred: bool,
    /// The program length right after this scope's own pushes and its
    /// `PushFilter`, and right *before* its own `PushBackdrop` — where a
    /// descendant's Backdrop Root Image begins when this scope is that
    /// descendant's Backdrop Root. Including this scope's own
    /// `PushBackdrop` is deliberate: filter-effects-2 draws the filtered
    /// backdrop inside the element's group, so it is part of what a nested
    /// element sees behind itself.
    content_start: u32,
    /// Whether this scope is a Backdrop Root (filter-effects-2 §2.2); see
    /// [`is_backdrop_root`].
    backdrop_root: bool,
}

/// One culled monolithic walk — the pre-compose shape, kept beside
/// [`walk_compose`] for the equivalence tests.
#[cfg(test)]
pub(crate) fn walk<T>(
    scene: &mut Scene,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    images: &ImageRegistry,
    pixels: &dyn crate::render::image::FrameImages,
) {
    let device = document.device();
    let ratio = f64::from(device.device_pixel_ratio().get());
    let cull = cull_rect(device.viewport_size(), ratio);
    let mut sink = WalkSink::Monolithic(scene, pixels);
    walk_within(
        &mut sink,
        scratch,
        document,
        frame,
        images,
        ratio,
        Some(cull),
    );
}

/// The production walk: encodes the frame as per-space fragments plus the
/// compose program over them.
pub(crate) fn walk_compose<T>(
    assembly: &mut ComposeAssembly,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    images: &ImageRegistry,
) {
    let device = document.device();
    let ratio = f64::from(device.device_pixel_ratio().get());
    let cull = cull_rect(device.viewport_size(), ratio);
    let mut sink = WalkSink::Compose(assembly);
    walk_within(
        &mut sink,
        scratch,
        document,
        frame,
        images,
        ratio,
        Some(cull),
    );
}

/// [`walk`] with culling switched off entirely.
///
/// The tests encode one frame both ways and compare, which is the statement
/// that culling changes nothing an observer of the viewport can see.
#[cfg(test)]
pub(crate) fn walk_uncultured<T>(
    scene: &mut Scene,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    images: &ImageRegistry,
    pixels: &dyn crate::render::image::FrameImages,
) {
    let ratio = f64::from(document.device().device_pixel_ratio().get());
    let mut sink = WalkSink::Monolithic(scene, pixels);
    walk_within(&mut sink, scratch, document, frame, images, ratio, None);
}

/// [`walk`] against an explicit admitted region, in viewport CSS px, or
/// `None` to encode every item.
fn walk_within<T>(
    sink: &mut WalkSink<'_>,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    images: &ImageRegistry,
    ratio: f64,
    cull: Option<Rect>,
) {
    frame.assert_visually_fresh(document);
    scratch.clip_stack.clear();
    scratch.chain.clear();
    scratch.scopes.clear();
    scratch.plan.resolve(document, frame, cull);
    plan_frame(scratch, document, frame);

    let painting = Painting {
        document,
        frame,
        images,
        scale: Affine::scale(ratio),
        ratio,
    };
    let items = frame.items();
    let layers = frame.layers();
    let mut next_open = 0_usize;

    for (index, item) in items.iter().enumerate() {
        // Both of these are driven by item index and must run for every item,
        // painted or not: a scope opens and closes where the paint order says,
        // never where the encode happens to land.
        while scratch
            .scopes
            .last()
            .is_some_and(|scope| layers[scope.layer].items.end == index)
        {
            close_scope(sink, scratch, painting);
        }
        while next_open < layers.len() && layers[next_open].items.start == index {
            open_scope(sink, scratch, painting, next_open);
            next_open += 1;
        }
        if let Some(local) = scratch.item_plan[index] {
            paint_item(sink, scratch, painting, item, local);
        }
    }
    while !scratch.scopes.is_empty() {
        close_scope(sink, scratch, painting);
    }
    pop_clips_to(sink, scratch, 0);
}

/// The viewport, in CSS px, grown by the slack every consumer of the retained
/// scene is allowed to render past it.
///
/// A device pixel ratio that is not a usable scale yields an unbounded rect.
/// That disables the viewport half of the cull test and leaves the clip half,
/// which is sound: a clip chain's bound is frame geometry and does not depend
/// on the ratio.
fn cull_rect(viewport: euclid::Size2D<f32, stylo_traits::CSSPixel>, ratio: f64) -> Rect {
    let slack = if ratio.is_finite() && ratio > 0.0 {
        VIEWPORT_SLACK_DEVICE_PX / ratio
    } else {
        f64::INFINITY
    };
    Rect::new(
        -slack,
        -slack,
        f64::from(viewport.width) + slack,
        f64::from(viewport.height) + slack,
    )
}

/// Resolves every clip chain in the frame to what it admits into `out`, each
/// in its own clip's space, intersected with `root` pulled back into that
/// space.
///
/// One forward pass suffices because a clip node's parent is always an earlier
/// entry: [`crate::visual`]'s builder pushes a clip only after the clip it
/// nests inside.
fn resolve_clips(
    windows: &[(Vector2D<f32>, Vector2D<f32>)],
    frame: &PaintOrder,
    root: Admitted,
    out: &mut Vec<Admitted>,
) {
    out.clear();
    for (index, clip) in frame.clips().iter().enumerate() {
        debug_assert!(
            clip.parent.is_none_or(|parent| parent < index),
            "a clip node nests inside an earlier clip node",
        );
        // The inherited region arrives pulled back into this clip's own
        // space, where the clip's rect is expressed.
        let inherited = admitted_under(windows, frame, out, root, clip.space, clip.parent);
        // `push_clip` pushes an empty clip for a singular transform, so
        // nothing under this chain reaches the scene at all.
        let resolved = match (clip_bounds(clip), inherited) {
            (None, _) | (_, Admitted::Nothing) => Admitted::Nothing,
            (Some(own), _) if !is_finite(own) => inherited,
            (Some(own), Admitted::Everything) => Admitted::Region(own),
            (Some(own), Admitted::Region(inherited)) => {
                let both = own.intersect(inherited);
                if both.width() > 0.0 && both.height() > 0.0 {
                    Admitted::Region(both)
                } else {
                    Admitted::Nothing
                }
            }
        };
        out.push(resolved);
    }
}

/// One clip node's rounded rect as an axis-aligned viewport-CSS-px bound.
///
/// Mirrors [`push_clip`], including its `Size2D`-only perspective fit, so this
/// is never tighter than the clip the painter pushes; taking the bounding box
/// of a rounded rect only widens it, which is the safe direction.
fn clip_bounds(clip: &ClipNode) -> Option<Rect> {
    let size = crate::Size2D::new(clip.rect.size.width, clip.rect.size.height);
    let affine = convert::item_affine(&clip.transform, size)?;
    Some(affine_rect(
        affine,
        Rect::new(
            clip.rect.origin.x as f64,
            clip.rect.origin.y as f64,
            (clip.rect.origin.x + clip.rect.size.width) as f64,
            (clip.rect.origin.y + clip.rect.size.height) as f64,
        ),
    ))
}

/// Whether `bounds` can put ink inside `admitted`, where `None` means the
/// item's clip chain admits nothing at all.
///
/// Answers `true` for anything it cannot decide. A rect with a non-finite
/// coordinate compares false against everything, so [`Rect::overlaps`] alone
/// would discard it, and culling is only ever safe on a proof of no ink.
fn can_reach(bounds: Rect, admitted: Option<Rect>) -> bool {
    let Some(admitted) = admitted else {
        return false;
    };
    if !is_finite(bounds) {
        return true;
    }
    bounds.overlaps(admitted)
}

fn is_finite(rect: Rect) -> bool {
    rect.x0.is_finite() && rect.y0.is_finite() && rect.x1.is_finite() && rect.y1.is_finite()
}

/// The `(low, high)` interval of `o_content − o_frame` over the two chains'
/// encode windows, in CSS px.
///
/// Content and regions are both baked unscrolled; at compose, content on
/// chain `c` appears translated by `−o_c` and a region on chain `f` by
/// `−o_f`, so comparing them means expanding the region by every value
/// `o_c − o_f` may take while both stay inside their windows. The common
/// prefix of the two chains cancels and is skipped.
fn relative_offset_range(
    slots: &[crate::visual::ScrollSlot],
    windows: &[(Vector2D<f32>, Vector2D<f32>)],
    content: Option<u32>,
    frame_of: Option<u32>,
) -> (Vector2D<f32>, Vector2D<f32>) {
    let is_ancestor = |candidate: Option<u32>, mut of: Option<u32>| loop {
        if of == candidate {
            return true;
        }
        let Some(index) = of else {
            return false;
        };
        of = slots[index as usize].parent;
    };
    let mut lca = content;
    loop {
        if is_ancestor(lca, frame_of) {
            break;
        }
        let Some(index) = lca else { break };
        lca = slots[index as usize].parent;
    }
    let mut low = Vector2D::zero();
    let mut high = Vector2D::zero();
    let mut chain = content;
    while chain != lca {
        let Some(index) = chain else { break };
        let (window_low, window_high) =
            viewport_window(&slots[index as usize], windows[index as usize]);
        low += window_low;
        high += window_high;
        chain = slots[index as usize].parent;
    }
    let mut chain = frame_of;
    while chain != lca {
        let Some(index) = chain else { break };
        let (window_low, window_high) =
            viewport_window(&slots[index as usize], windows[index as usize]);
        low -= window_high;
        high -= window_low;
        chain = slots[index as usize].parent;
    }
    (low, high)
}

/// An offset rectangle mapped through a scrollport's linear transform.
fn viewport_window(
    slot: &crate::visual::ScrollSlot,
    (low, high): (Vector2D<f32>, Vector2D<f32>),
) -> (Vector2D<f32>, Vector2D<f32>) {
    let x0 = slot.viewport_axes[0] * low.x;
    let x1 = slot.viewport_axes[0] * high.x;
    let y0 = slot.viewport_axes[1] * low.y;
    let y1 = slot.viewport_axes[1] * high.y;
    (
        Vector2D::new(
            x0.x.min(x1.x) + y0.x.min(y1.x),
            x0.y.min(x1.y) + y0.y.min(y1.y),
        ),
        Vector2D::new(
            x0.x.max(x1.x) + y0.x.max(y1.x),
            x0.y.max(x1.y) + y0.y.max(y1.y),
        ),
    )
}

/// A region on the frame chain, expanded to admit content whose relative
/// translation ranges over `[low, high]`: content `p` overlaps the composed
/// region iff `p` overlaps `region ⊕ [low, high]`.
fn expand_region(region: Rect, low: Vector2D<f32>, high: Vector2D<f32>) -> Rect {
    Rect::new(
        region.x0 + f64::from(low.x),
        region.y0 + f64::from(low.y),
        region.x1 + f64::from(high.x),
        region.y1 + f64::from(high.y),
    )
}

/// Content bounds carried into the frame chain's coordinates: the union of
/// `bounds − t` over `t ∈ [low, high]` — what a group layer's clip must
/// cover for content that composes inside it.
fn expand_cover(bounds: Rect, low: Vector2D<f32>, high: Vector2D<f32>) -> Rect {
    Rect::new(
        bounds.x0 - f64::from(high.x),
        bounds.y0 - f64::from(high.y),
        bounds.x1 - f64::from(low.x),
        bounds.y1 - f64::from(low.y),
    )
}

fn open_scope<T>(
    sink: &mut WalkSink<'_>,
    scratch: &mut Scratch,
    painting: Painting<'_, T>,
    layer_index: usize,
) {
    let Painting {
        document,
        frame,
        images,
        scale,
        ratio,
    } = painting;
    let layer = &frame.layers()[layer_index];
    let space = layer.space;
    let base = scratch.scopes.last().map_or(0, |scope| scope.base);
    pop_clips_to(sink, scratch, base);

    let style = document
        .paint_style(layer.node)
        .expect("a group-effect stacking context keeps its style for the frame");
    // filter-effects-2 §2.2: the Backdrop Root Image is everything painted
    // before *this element*, so the range ends here — before this scope's own
    // effect layer, its `clip-path` layer and, decisively, the mask pattern
    // `mask::paint` would otherwise draw into it. The ops from here to the
    // `PushBackdrop` recorded below are this element's own painting.
    let backdrop_end = (!style.get_effects().backdrop_filter.0.is_empty())
        .then(|| (nearest_backdrop_root(scratch), sink.content_boundary()));
    let bounds = scratch.layer_bounds[layer_index];
    let effects = style.get_effects();
    let blend = blend_mode(style);
    let mut pushed = 1_u32;
    // The effect layer's alpha is replaced at compose time when this group's
    // own element exports an opacity curve.
    let alpha_animation = space::nearest_animation(frame.spaces(), space)
        .filter(|&slot| frame.animations()[slot as usize].node == layer.node);
    sink.push_layer_rect(
        space,
        alpha_animation,
        Fill::NonZero,
        blend,
        effects.opacity.clamp(0.0, 1.0),
        scale,
        bounds,
    );

    let local = convert::item_affine(&layer.transform, layer.size);
    let fragment = document.rounded_layout(layer.node).map(|layout| {
        BoxFragment::new(
            scale * local.unwrap_or_default(),
            layer.size,
            layer.radii,
            layout,
        )
    });

    if push_clip_path(sink, space, style, fragment.as_ref(), local, scale) {
        pushed += 1;
    }

    if mask::has_mask(style) {
        if let Some(fragment) = fragment.as_ref() {
            mask::paint(sink, space, style, fragment, images);
        }
        sink.push_layer_rect(
            space,
            None,
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::SrcIn),
            1.0,
            scale,
            bounds,
        );
        pushed += 1;
    }

    // The blur scope opens *inside* the scope's own layers, so the group it
    // bakes is exactly the pixels the effect/clip-path/mask stack will then
    // clip, mask and fade — filter-effects-1's order, with clip and mask
    // swapped (both intersective, so unobservable; see the module doc).
    let blurred = filter_group(scratch, layer_index, space, ratio)
        .is_some_and(|group| sink.push_filter(group));

    // Innermost of all, and before any item: the filtered backdrop is the
    // first thing painted inside this element's own group, so this scope's
    // `opacity`, `clip-path`, `mask-image` and `filter` apply to the backdrop
    // and to the element together.
    let content_start = sink.content_boundary();
    if let Some((root_start, end)) = backdrop_end
        && let Some(entry) = backdrop_entry(style, layer, space, scale, ratio)
    {
        sink.push_backdrop(entry, root_start..end, frame.spaces());
    }

    scratch.scopes.push(Scope {
        layer: layer_index,
        base,
        pushed,
        filtered: !effects.filter.0.is_empty(),
        blurred,
        content_start,
        backdrop_root: is_backdrop_root(style),
    });
}

/// Pushes this scope's `clip-path` layer, answering whether it pushed one.
///
/// A full `push_layer` rather than a clip layer, per the module doc's #1198
/// rule. A singular map has no shape to clip with, so it pushes an empty rect
/// instead — which encodes the same "nothing gets through".
fn push_clip_path(
    sink: &mut WalkSink<'_>,
    space: Option<u32>,
    style: &stylo::properties::ComputedValues,
    fragment: Option<&BoxFragment>,
    local: Option<Affine>,
    scale: Affine,
) -> bool {
    let Some(fragment) = fragment else {
        return false;
    };
    let Some((clip_shape, fill)) =
        crate::paint::shape::clip_path_shape(style, &fragment.reference_boxes())
    else {
        return false;
    };
    match local {
        Some(local) => sink.push_layer_box(
            space,
            fill,
            BlendMode::new(Mix::Normal, Compose::SrcOver),
            1.0,
            scale * local,
            clip_shape,
        ),
        None => sink.push_layer_rect(
            space,
            None,
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            Rect::ZERO,
        ),
    }
    true
}

/// Where the Backdrop Root Image of a scope about to open begins: the
/// content start of the nearest enclosing Backdrop Root, or the start of the
/// program when there is none.
///
/// Falling back to zero *is* the spec's root-element rule: the document root
/// element is always a Backdrop Root, and nothing is painted before program
/// index zero.
fn nearest_backdrop_root(scratch: &Scratch) -> u32 {
    scratch
        .scopes
        .iter()
        .rev()
        .find(|scope| scope.backdrop_root)
        .map_or(0, |scope| scope.content_start)
}

/// Whether this element is a Backdrop Root (filter-effects-2 §2.2).
///
/// The spec's list, and the reason this predicate exists at all rather than
/// reusing [`crate::visual::stacking::needs_group_rendering`]: that one also
/// answers `true` for `isolation: isolate`, which the spec's Backdrop Root
/// list does not contain. `isolation` is not in the fork's author grammar
/// either, so that exclusion is unobservable here.
///
/// The one **observable** omission is `will-change`: a
/// `will-change: opacity` (or `filter`, `backdrop-filter`, `mask`,
/// `clip-path`) element is a Backdrop Root per the spec, and this engine
/// opens no group layer for one, so a `backdrop-filter` element inside such a
/// wrapper sees through it to the content behind. That is a ruled deviation —
/// the decision is not to open a layer per `will-change` element — recorded
/// in `docs/tracking/deviations.md`.
fn is_backdrop_root(style: &stylo::properties::ComputedValues) -> bool {
    use stylo::computed_values::mix_blend_mode::T as MixBlendMode;
    use stylo::values::computed::basic_shape::ClipPath;

    let effects = style.get_effects();
    effects.opacity < 1.0
        || !effects.filter.0.is_empty()
        || !effects.backdrop_filter.0.is_empty()
        || effects.mix_blend_mode != MixBlendMode::Normal
        || style.get_svg().clip_path != ClipPath::None
        || mask::has_mask(style)
}

/// The `backdrop-filter` entry this layer records, if it can bake one.
///
/// `None` for an element with no `backdrop-filter`, a singular or degenerate
/// map (nothing of the backdrop would be visible through it), and a border
/// box whose device rect is empty or not finite.
///
/// The rect is exactly the element's transformed border box, rounded out —
/// `backdrop-filter` enlarges no ink overflow, so unlike a `filter: blur()`
/// group there is no 3σ margin. It is also the crop filter-effects-2 §2.2
/// applies *before* filtering, and the edge the bake's mirror sampler
/// reflects at.
fn backdrop_entry(
    style: &stylo::properties::ComputedValues,
    layer: &RenderLayer,
    space: Option<u32>,
    scale: Affine,
    ratio: f64,
) -> Option<FilterGroup> {
    let list = &style.get_effects().backdrop_filter.0;
    if list.is_empty() {
        return None;
    }
    let local = convert::item_affine(&layer.transform, layer.size)?;
    let transform = scale * local;
    if !(transform.determinant().is_finite() && transform.determinant() != 0.0) {
        return None;
    }
    let border_box = Rect::new(0.0, 0.0, layer.size.width as f64, layer.size.height as f64);
    let device = affine_rect(transform, border_box);
    if !is_finite(device) {
        return None;
    }
    let rect = Rect::new(
        device.x0.floor(),
        device.y0.floor(),
        device.x1.ceil(),
        device.y1.ceil(),
    );
    if !(rect.width() >= 1.0 && rect.height() >= 1.0) {
        return None;
    }
    let plan = filters::plan(list);
    // A colour-only list is σ = 0, which bakes and filters as usual; only a
    // list with no `blur()` at all skips the gaussian.
    let sigma = plan
        .sigma
        .map_or(0.0, |sigma| f64::from(sigma) * mean_scale(local) * ratio);
    let backdrop = crate::paint::compose::Backdrop {
        shape: BoxShape::new(border_box, &layer.radii),
        transform,
        before: filters::passes(list, plan.before.clone()),
        after: filters::passes(list, plan.after.clone()),
        open_pushes: 0,
    };
    Some(FilterGroup::with_backdrop(
        if sigma.is_finite() && sigma > 0.0 {
            sigma as f32
        } else {
            0.0
        },
        rect,
        space,
        backdrop,
    ))
}

/// The filter group this layer bakes, if it blurs at all.
///
/// `None` for a layer with no `blur()`, one whose device rect is empty, and
/// one whose geometry is not finite: each of those blurs nothing, and a group
/// that blurs nothing must leave no op behind — a frame with no filter op is
/// the frame the whole bake pre-step is skipped for.
fn filter_group(
    scratch: &Scratch,
    layer_index: usize,
    space: Option<u32>,
    ratio: f64,
) -> Option<FilterGroup> {
    let sigma = scratch.layer_sigma[layer_index] * ratio;
    if !(sigma.is_finite() && sigma > 0.0) {
        return None;
    }
    let bounds = scratch.layer_bounds[layer_index];
    if !is_finite(bounds) {
        return None;
    }
    // Rounded out, so the bake covers every device pixel the group's rect
    // touches and the texture composes at an integer offset.
    let rect = Rect::new(
        (bounds.x0 * ratio).floor(),
        (bounds.y0 * ratio).floor(),
        (bounds.x1 * ratio).ceil(),
        (bounds.y1 * ratio).ceil(),
    );
    if !(is_finite(rect) && rect.width() >= 1.0 && rect.height() >= 1.0) {
        return None;
    }
    Some(FilterGroup::new(sigma as f32, rect, space))
}

fn close_scope<T>(sink: &mut WalkSink<'_>, scratch: &mut Scratch, painting: Painting<'_, T>) {
    let Painting {
        document,
        frame,
        scale,
        ..
    } = painting;
    let scope = scratch
        .scopes
        .pop()
        .expect("close_scope is only called with an open scope");
    pop_clips_to(sink, scratch, scope.base);
    let layer = &frame.layers()[scope.layer];
    let space = layer.space;
    let bounds = scratch.layer_bounds[scope.layer];
    // The list splits at its first `blur()`: what precedes it composites
    // against the group's own pixels *inside* the bake, what follows it
    // against the blurred result.
    let plan = scope
        .filtered
        .then(|| document.paint_style(layer.node))
        .flatten()
        .map(|style| {
            let list = &style.get_effects().filter.0;
            (list, filters::plan(list))
        });
    if let Some((list, plan)) = &plan {
        filters::apply(
            sink.scene_for(space),
            list,
            plan.before.clone(),
            bounds,
            scale,
        );
    }
    if scope.blurred {
        sink.pop_filter(frame.spaces());
    }
    if let Some((list, plan)) = &plan {
        filters::apply(
            sink.scene_for(space),
            list,
            plan.after.clone(),
            bounds,
            scale,
        );
    }
    for _ in 0..scope.pushed {
        sink.pop();
    }
}

/// Paints one item with the matrix [`plan_frame`] resolved for it.
fn paint_item<T>(
    sink: &mut WalkSink<'_>,
    scratch: &mut Scratch,
    painting: Painting<'_, T>,
    item: &PaintItem,
    local: Affine,
) {
    let Painting {
        document,
        frame,
        images,
        scale,
        ..
    } = painting;
    sync_clips(sink, scratch, frame, item, scale);
    let space = item.space;
    let transform = scale * local;

    match item.kind {
        PaintItemKind::ElementBox => {
            let Some(style) = document.paint_style(item.node) else {
                return;
            };
            let Some(layout) = document.rounded_layout(item.node) else {
                return;
            };
            let fragment = BoxFragment::new(transform, item.size, item.radii, layout);
            let text_clip =
                background::needs_text_clip(style).then(|| collect_text_clip(document, item.node));
            // Each painter re-acquires the fragment rather than sharing one
            // borrow across the item: an image draw between them is a
            // program op, which cuts the open fragment. `fragment_for` cuts
            // only when the space actually changed, so re-acquiring where
            // nothing was emitted costs a comparison.
            shadow::paint_outset(sink.scene_for(space), &mut scratch.paths, style, &fragment);
            background::paint(sink, space, style, &fragment, images, text_clip.as_ref());
            shadow::paint_inset(sink.scene_for(space), &mut scratch.paths, style, &fragment);
            // Of a replaced element's two sources, the registry picks the one
            // whose bitmap the node's natural size was recomputed from, so
            // `object-fit` fits the bitmap drawn here.
            let (source, placeholder) = document.image_sources(item.node);
            if let Some((image, _)) = images.resolve_presented(source, placeholder) {
                background::paint_replaced_content(
                    sink,
                    space,
                    style,
                    &fragment,
                    image,
                    document.natural_size(item.node),
                );
            }
            border::paint(sink.scene_for(space), &mut scratch.paths, style, &fragment);
            border::paint_outline(sink.scene_for(space), &mut scratch.paths, style, &fragment);
        }
        PaintItemKind::TextRun { element } => {
            let Some(style) = document.paint_style(element) else {
                return;
            };
            let Some(block) = document.text_block(item.node) else {
                return;
            };
            let layout = block.display();
            let gradient_box = text::needs_gradient_box(style)
                .then(|| color_gradient_box(document, item, element));
            // One entry per parley style index, so a glyph run resolves its own
            // element's paint in O(1). A nested scope carries its own colour,
            // shadow, stroke and decorations; the establishing element answers
            // for the synthesized ellipsis and for anything unresolvable.
            let runs = text::RunPaints::resolve(document, element, block, gradient_box);
            // A nested scope has no box of its own, so its background is an
            // inline box's: one fragment per line, under every shadow and
            // every glyph in the paragraph. Only paragraphs that have one pay.
            if runs.has_inline_backgrounds() {
                paint_inline_backgrounds(sink, space, document, block, &runs, transform, images);
            }
            text::paint(sink.scene_for(space), layout, transform, &runs);
        }
    }
}

/// Paints one fragment per line per backgrounded nested scope, outermost scope
/// first, through the same `background::paint` an element box uses.
///
/// The geometry is [`text::inline_background_fragments`]'s; what is decided
/// here is the box each fragment presents to the background painter. A
/// fragment has no border and no padding, so its border, padding and content
/// boxes coincide, and `border-radius` resolves against the fragment's own
/// size — `box-decoration-break: clone` where the web default is `slice`.
/// `background-clip: text` on an inline scope is treated as `border-box`: the
/// glyph silhouette a text clip needs is the paragraph's, which is the box
/// being painted into, so clipping to it is a no-op the `None` below spells
/// directly.
fn paint_inline_backgrounds<T>(
    sink: &mut WalkSink<'_>,
    space: Option<u32>,
    document: &Document<T>,
    block: &hughie::text::block::TextBlock,
    runs: &text::RunPaints<'_>,
    transform: Affine,
    images: &ImageRegistry,
) {
    let mut fragments = Vec::new();
    text::inline_background_fragments(block.display(), block, runs, &mut fragments);
    for (node, rect) in fragments {
        let size = crate::Size2D::new(rect.width() as f32, rect.height() as f32);
        if size.width <= 0.0 || size.height <= 0.0 {
            continue;
        }
        let Some(style) = document.paint_style(node) else {
            continue;
        };
        let box_rect = Rect::from_origin_size((0.0, 0.0), rect.size());
        let fragment = crate::paint::BoxFragment {
            transform: transform * Affine::translate(rect.origin().to_vec2()),
            border_box: box_rect,
            padding_box: box_rect,
            content_box: box_rect,
            radii: crate::visual::geometry::resolve_corner_radii(style, size),
            border_widths: crate::layout::Edges::uniform(0.0),
            padding_widths: crate::layout::Edges::uniform(0.0),
        };
        background::paint(sink, space, style, &fragment, images, None);
    }
}

fn color_gradient_box<T>(document: &Document<T>, item: &PaintItem, element: crate::NodeId) -> Rect {
    let own_box = Rect::new(
        0.0,
        0.0,
        f64::from(item.size.width),
        f64::from(item.size.height),
    );
    let Some(element_layout) = document.rounded_layout(element) else {
        return own_box;
    };
    let size = element_layout.size;
    let border = element_layout.border;
    let padding_box = Rect::new(
        f64::from(border.left),
        f64::from(border.top),
        f64::from((size.width - border.right).max(border.left)),
        f64::from((size.height - border.bottom).max(border.top)),
    );
    if padding_box.width() <= 0.0 || padding_box.height() <= 0.0 {
        return own_box;
    }
    // Into the paragraph's own space. A block paints its glyphs from its
    // content box, so that origin — border plus padding — is the offset
    // between the two, and the paint item is the establishing element itself
    // rather than a run inside it.
    let content_origin = crate::vello::kurbo::Vec2::new(
        f64::from(element_layout.border.left + element_layout.padding.left),
        f64::from(element_layout.border.top + element_layout.padding.top),
    );
    padding_box - content_origin
}

fn collect_text_clip<T>(
    document: &Document<T>,
    element: crate::NodeId,
) -> crate::paint::TextClip<'_> {
    let mut clip = crate::paint::TextClip::default();
    collect_text_clip_under(
        document,
        element,
        crate::vello::kurbo::Vec2::ZERO,
        &mut clip,
    );
    clip
}

fn collect_text_clip_under<'doc, T>(
    document: &'doc Document<T>,
    node: crate::NodeId,
    offset: crate::vello::kurbo::Vec2,
    clip: &mut crate::paint::TextClip<'doc>,
) {
    use crate::vello::kurbo::Vec2;
    let Some(node_ref) = document.get(node) else {
        return;
    };
    // A paragraph belongs to the element that establishes it, so the node
    // being walked can be the one holding the silhouette. Text nodes hold
    // none — they are content of the block above them.
    let visible = document.paint_style(node).is_none_or(|style| {
        matches!(
            style.clone_visibility(),
            stylo::computed_values::visibility::T::Visible
        )
    });
    if visible
        && let Some(block) = document.text_block(node)
        && !block.lines().is_empty()
    {
        // An element with no runs still establishes a paragraph; it just has
        // no ink, and a silhouette of nothing would clip everything away.
        clip.runs.push((offset, block.display()));
    }

    for child in node_ref.flat_children_iter() {
        if child.is_element() {
            let child_offset = document
                .rounded_layout(child.id())
                .map_or(offset, |layout| {
                    offset + Vec2::new(f64::from(layout.location.x), f64::from(layout.location.y))
                });
            collect_text_clip_under(document, child.id(), child_offset, clip);
        }
    }
}

fn sync_clips(
    sink: &mut WalkSink<'_>,
    scratch: &mut Scratch,
    frame: &PaintOrder,
    item: &PaintItem,
    scale: Affine,
) {
    let base = scratch.scopes.last().map_or(0, |scope| scope.base);
    scratch.chain.clear();
    let mut next = item.clip;
    while let Some(index) = next {
        scratch.chain.push(index);
        next = frame.clips()[index].parent;
    }
    scratch.chain.reverse();

    let common = scratch.clip_stack[base..]
        .iter()
        .zip(scratch.chain.iter())
        .take_while(|(pushed, wanted)| pushed == wanted)
        .count();
    pop_clips_to(sink, scratch, base + common);
    for position in common..scratch.chain.len() {
        let index = scratch.chain[position];
        push_clip(sink, &frame.clips()[index], scale);
        scratch.clip_stack.push(index);
    }
}

fn push_clip(sink: &mut WalkSink<'_>, clip: &ClipNode, scale: Affine) {
    let size = crate::Size2D::new(clip.rect.size.width, clip.rect.size.height);
    let Some(local) = convert::item_affine(&clip.transform, size) else {
        sink.push_clip_empty(clip.space);
        return;
    };
    let rect = Rect::new(
        clip.rect.origin.x as f64,
        clip.rect.origin.y as f64,
        (clip.rect.origin.x + clip.rect.size.width) as f64,
        (clip.rect.origin.y + clip.rect.size.height) as f64,
    );
    let shape = BoxShape::new(rect, &clip.radii);
    sink.push_clip_box(clip.space, Fill::NonZero, scale * local, shape);
}

fn pop_clips_to(sink: &mut WalkSink<'_>, scratch: &mut Scratch, len: usize) {
    while scratch.clip_stack.len() > len {
        scratch.clip_stack.pop();
        sink.pop();
    }
}

/// Per-frame prepass: the bounds every group's effect layer is pushed with,
/// and one plan entry per item saying whether it paints and with what matrix.
///
/// The admitted region every item is tested against comes from `scratch.plan`,
/// which [`CullPlan::resolve`] filled — including whether culling is on at
/// all.
fn plan_frame<T>(scratch: &mut Scratch, document: &Document<T>, frame: &PaintOrder) {
    let layers = frame.layers();
    let items = frame.items();
    scratch.layer_bounds.clear();
    scratch.open_layers.clear();
    scratch.item_plan.clear();
    scratch.item_plan.resize(items.len(), None);
    scratch.layer_bounds.resize(layers.len(), Rect::ZERO);
    scratch.layer_sigma.clear();
    scratch
        .layer_sigma
        .extend(layers.iter().map(|layer| layer_blur_sigma(document, layer)));
    // CSS px, not device px: the paint order this is intersected against carries CSS-px
    // transforms — the device scale is applied once, separately, as the root `scale` affine.
    let viewport_size = document.device().viewport_size();
    let viewport = Rect::new(
        0.0,
        0.0,
        f64::from(viewport_size.width),
        f64::from(viewport_size.height),
    );
    scratch.bounds_acc.clear();
    scratch.bounds_acc.resize(layers.len(), None);
    scratch.layer_moves.clear();
    scratch
        .layer_moves
        .extend(layers.iter().map(|layer| moving(frame, layer.space)));
    if scratch.layer_moves.contains(&true) {
        resolve_clips(
            &scratch.plan.slot_windows,
            frame,
            Admitted::Everything,
            &mut scratch.clip_extent,
        );
    }
    let slots = frame.slots();
    let spaces = frame.spaces();
    let mut next_open = 0_usize;
    let close = |scratch: &mut Scratch| close_layer(scratch, frame, viewport);

    for (index, item) in items.iter().enumerate() {
        while scratch
            .open_layers
            .last()
            .is_some_and(|&top| layers[top].items.end == index)
        {
            close(scratch);
        }
        while next_open < layers.len() && layers[next_open].items.start == index {
            let layer = &layers[next_open];
            scratch.bounds_acc[next_open] = layer_root_rect(layer)
                .and_then(|rect| held(scratch, frame, next_open, layer.clip, layer.space, rect));
            scratch.open_layers.push(next_open);
            next_open += 1;
        }
        // No `continue` on "outside every layer" here: those are precisely
        // the items no group has already clipped to the viewport, which are
        // the ones culling exists for.
        let Some(local) = convert::item_affine(&item.transform, item.size) else {
            // A singular transform encodes nothing.
            continue;
        };
        let top = scratch.open_layers.last().copied();
        let admitted = scratch.plan.admitted_for(frame, item.clip, item.space, top);

        // An item whose plain border box already reaches the admitted region
        // paints whatever its fragments reach, because every reach only grows
        // that box. Items inside a group are excluded: their reach is needed
        // for the group's bounds regardless of what the cull test decides.
        // This test is exactly `CullPlan::admits_auto_box`'s, which is what
        // the `content-visibility: auto` relevance pass asks with.
        if top.is_none() && admitted.reached_by(box_bounds(local, item.size, 0.0)) {
            scratch.item_plan[index] = Some(local);
            continue;
        }

        let reach = extents(document, item);
        let bounds = item_bounds(item, local, reach.layer);
        if let Some(top) = top
            && let Some(bounds) = held(scratch, frame, top, item.clip, item.space, bounds)
        {
            let (low, high) = relative_offset_range(
                slots,
                &scratch.plan.slot_windows,
                nearest_scroll(spaces, item.space),
                nearest_scroll(spaces, layers[top].space),
            );
            let bounds = expand_cover(bounds, low, high);
            let (sticky_low, sticky_high) = frame.sticky_range(
                nearest_sticky(spaces, item.space),
                nearest_sticky(spaces, layers[top].space),
            );
            let bounds = expand_region(bounds, sticky_low, sticky_high);
            scratch.bounds_acc[top] =
                Some(scratch.bounds_acc[top].map_or(bounds, |united| united.union(bounds)));
        }
        let reachable = match admitted {
            Admitted::Everything => true,
            _ if reach.cull.is_finite() => {
                let inflated = if reach.cull > reach.layer {
                    item_bounds(item, local, reach.cull)
                } else {
                    bounds
                };
                admitted.reached_by(inflated)
            }
            // An unbounded reach can only be discarded by a clip chain that
            // admits nothing at all.
            Admitted::Region(_) => true,
            Admitted::Nothing => false,
        };
        if reachable {
            scratch.item_plan[index] = Some(local);
        }
    }
    while !scratch.open_layers.is_empty() {
        close(scratch);
    }
}

/// Closes the topmost open layer: its accumulated bounds become its pushed
/// rect — intersected with the viewport expanded into the layer's space,
/// since the compose window may carry the layer's content across it, unless
/// the layer moves (see [`held`]) — and fold into the parent layer still
/// open, expanded into that parent's space.
fn close_layer(scratch: &mut Scratch, frame: &PaintOrder, viewport: Rect) {
    let layers = frame.layers();
    let slots = frame.slots();
    let spaces = frame.spaces();
    let closed = scratch
        .open_layers
        .pop()
        .expect("close is only called with an open layer");
    let own = layers[closed].space;
    // A blur puts ink 3 sigma past the group's own content, so the group's
    // pushed rect — which is also the bake's rect — has to carry that margin.
    // Inflating *before* the viewport intersection is what makes the margin
    // transparent rather than clipped: the bake's edges read transparent
    // black, which is filter-effects-1's edge mode.
    let reach = BLUR_INK_SIGMAS * scratch.layer_sigma[closed];
    let accumulated = scratch.bounds_acc[closed];
    scratch.layer_bounds[closed] = accumulated.map_or(Rect::ZERO, |rect| {
        let rect = inflate_rect(rect, reach);
        // The group's rect and content translate together under a sampled
        // delta, but the *viewport* does not: a moving group's content was
        // held to the clips moving with it instead, since clipping to the
        // viewport would cut content the delta moves into view.
        match pull_back(
            &scratch.plan.slot_windows,
            frame,
            Admitted::Region(viewport),
            None,
            own,
        ) {
            Admitted::Region(region) => rect.intersect(inflate_rect(region, reach)),
            Admitted::Everything | Admitted::Nothing => rect,
        }
    });
    if let (Some(bounds), Some(&parent)) = (scratch.bounds_acc[closed], scratch.open_layers.last())
    {
        let outer = layers[parent].space;
        let (low, high) = relative_offset_range(
            slots,
            &scratch.plan.slot_windows,
            nearest_scroll(spaces, own),
            nearest_scroll(spaces, outer),
        );
        let bounds = expand_cover(inflate_rect(bounds, reach), low, high);
        let (sticky_low, sticky_high) =
            frame.sticky_range(nearest_sticky(spaces, own), nearest_sticky(spaces, outer));
        let bounds = expand_region(bounds, sticky_low, sticky_high);
        scratch.bounds_acc[parent] =
            Some(scratch.bounds_acc[parent].map_or(bounds, |united| united.union(bounds)));
    }
}

/// How many standard deviations of blur ink a group's bounds are grown by.
///
/// A gaussian past 3 sigma carries under 0.3% of its mass, which is below one
/// 8-bit level; filter-effects-1 names the same number for the filter region
/// of a `blur()`.
const BLUR_INK_SIGMAS: f64 = 3.0;

/// One layer's blur sigma in *viewport* CSS px: the element's own sigma under
/// the mean scale of its local-to-viewport linear map.
///
/// The mean scale is the arithmetic mean of the map's two singular values,
/// read off `Affine::nuclear_norm_squared` — the nuclear norm *is* their sum.
/// Exact for a rotation or a uniform scale, and one isotropic number where a
/// non-uniform scale or a skew would make the spec's filter region
/// anisotropic (recorded limit).
fn layer_blur_sigma<T>(document: &Document<T>, layer: &RenderLayer) -> f64 {
    let Some(style) = document.paint_style(layer.node) else {
        return 0.0;
    };
    let Some(sigma) = filters::plan(&style.get_effects().filter.0).sigma else {
        return 0.0;
    };
    let Some(affine) = convert::item_affine(&layer.transform, layer.size) else {
        // A singular map encodes nothing, so there is nothing to blur.
        return 0.0;
    };
    let scaled = f64::from(sigma) * mean_scale(affine);
    if scaled.is_finite() && scaled > 0.0 {
        scaled
    } else {
        0.0
    }
}

/// The arithmetic mean of a map's two singular values, read off
/// `Affine::nuclear_norm_squared` — the nuclear norm *is* their sum. Shared
/// by `filter` and `backdrop-filter`, which scale σ into device space the
/// same way.
fn mean_scale(affine: Affine) -> f64 {
    affine.nuclear_norm_squared().sqrt() / 2.0
}

/// `rect` grown by `reach` on every side. A zero reach is the rect itself, so
/// an unfiltered layer pays one comparison.
fn inflate_rect(rect: Rect, reach: f64) -> Rect {
    if reach <= 0.0 {
        return rect;
    }
    Rect::new(
        rect.x0 - reach,
        rect.y0 - reach,
        rect.x1 + reach,
        rect.y1 + reach,
    )
}

/// What is admitted for content in `space`: the innermost enclosing `clip`'s
/// resolved bounds, or the cull rect when there is no clip, pulled back from
/// the space it is expressed in into `space`.
fn admitted_region(
    plan: &CullPlan,
    frame: &PaintOrder,
    space: Option<u32>,
    clip: Option<usize>,
) -> Admitted {
    let root = plan.cull.map_or(Admitted::Everything, Admitted::Region);
    admitted_under(
        &plan.slot_windows,
        frame,
        &plan.clip_bounds,
        root,
        space,
        clip,
    )
}

/// [`admitted_region`] over any resolved clip table and root region.
fn admitted_under(
    windows: &[(Vector2D<f32>, Vector2D<f32>)],
    frame: &PaintOrder,
    clips: &[Admitted],
    root: Admitted,
    space: Option<u32>,
    clip: Option<usize>,
) -> Admitted {
    let (base, outer) = match clip {
        Some(clip) => (clips[clip], frame.clips()[clip].space),
        None => (root, None),
    };
    pull_back(windows, frame, base, outer, space)
}

/// The part of `bounds` — content of layer `layer`, in `space` under `clip` —
/// that the layer's own bounds accumulate.
///
/// A moving layer's content is held to what the clips moving with it admit:
/// the viewport cannot bound it, and neither can a clip outside the moving
/// subtree, but a clip or encode window inside it can — so a list inside a
/// moving blurred card bakes its window, not its whole content. A still
/// layer's content passes whole; its rect meets the viewport at close.
fn held(
    scratch: &Scratch,
    frame: &PaintOrder,
    layer: usize,
    clip: Option<usize>,
    space: Option<u32>,
    bounds: Rect,
) -> Option<Rect> {
    if !scratch.layer_moves[layer] {
        return Some(bounds);
    }
    admitted_under(
        &scratch.plan.slot_windows,
        frame,
        &scratch.clip_extent,
        Admitted::Everything,
        space,
        clip,
    )
    .cut(bounds)
}

/// `region`, in `outer`'s coordinates, pulled back into `space`'s, `outer`
/// being an ancestor-or-self of `space`.
///
/// Content is baked unscrolled, unstuck and at committed transforms, so each
/// node between the two carries it by its whole committed range — a scroll
/// node by its encode window, a sticky node by its offset bounds — and the
/// ranges add. An animation node with a transform track makes the region
/// [`Admitted::Everything`]: nothing committed bounds its delta. A region
/// expressed at or below that node (a clip inside the moving subtree) never
/// walks through it, which is why such clips still bound.
fn pull_back(
    windows: &[(Vector2D<f32>, Vector2D<f32>)],
    frame: &PaintOrder,
    region: Admitted,
    outer: Option<u32>,
    space: Option<u32>,
) -> Admitted {
    let Admitted::Region(region) = region else {
        return region;
    };
    let spaces = frame.spaces();
    let mut low = Vector2D::zero();
    let mut high = Vector2D::zero();
    let mut current = space;
    while current != outer {
        let Some(index) = current else {
            // Uncertainty paints.
            debug_assert!(
                false,
                "a region's space encloses the space it is pulled into"
            );
            return Admitted::Everything;
        };
        let node = spaces[index as usize];
        match node.kind {
            SpaceKind::Scroll(slot) => {
                let slot = slot as usize;
                let (window_low, window_high) =
                    viewport_window(&frame.slots()[slot], windows[slot]);
                low += window_low;
                high += window_high;
            }
            SpaceKind::Sticky(slot) => {
                let (sticky_low, sticky_high) = frame.sticky_slot_range(slot);
                low -= sticky_high;
                high -= sticky_low;
            }
            SpaceKind::Animation(slot) => {
                if moves(frame, slot) {
                    return Admitted::Everything;
                }
            }
        }
        current = node.parent;
    }
    Admitted::Region(expand_region(region, low, high))
}

/// Whether animation slot `slot` carries a transform track; an opacity-only
/// curve moves nothing.
fn moves(frame: &PaintOrder, slot: u32) -> bool {
    frame.animations()[slot as usize].curve.transform.is_some()
}

/// Whether a transform curve on `space`'s path moves it.
fn moving(frame: &PaintOrder, space: Option<u32>) -> bool {
    space::path(frame.spaces(), space)
        .any(|kind| matches!(kind, SpaceKind::Animation(slot) if moves(frame, slot)))
}

fn layer_root_rect(layer: &RenderLayer) -> Option<Rect> {
    let affine = convert::item_affine(&layer.transform, layer.size)?;
    Some(affine_rect(
        affine,
        Rect::new(0.0, 0.0, layer.size.width as f64, layer.size.height as f64),
    ))
}

/// How far past its box each of the two reaches carries, for one item.
///
/// Element boxes are exact rather than estimated. `shadow::extent` is the
/// outset shadows' offset plus spread plus the blur cutoff, `outline_extent`
/// is the outline width and the fork has no `outline-offset`, and every other
/// element fragment is clipped to a border, padding, or content box. So the
/// same number serves both jobs and the cull bound needs no margin of its own.
///
/// A text run's is an estimate, which is why its cull reach is infinite. Half
/// an em is fine as a *layer* bound, where too small only trims an effect
/// layer over content that is not there; as a cull bound too small means the
/// run vanishes, and nothing here bounds an author-supplied face's ink.
fn extents<T>(document: &Document<T>, item: &PaintItem) -> Extents {
    match item.kind {
        PaintItemKind::ElementBox => {
            let reach = document.paint_style(item.node).map_or(0.0, |style| {
                shadow::extent(style).max(border::outline_extent(style))
            });
            Extents {
                layer: reach,
                cull: reach,
            }
        }
        PaintItemKind::TextRun { element } => {
            let layer = document.paint_style(element).map_or(4.0, |style| {
                0.5 * f64::from(style.get_font().clone_font_size().computed_size().px())
                    + text::extent(style)
            });
            Extents {
                layer,
                cull: f64::INFINITY,
            }
        }
    }
}

/// The rect an item may put ink in, in viewport CSS px.
///
/// `affine` is the map [`paint_item`] paints with, so this is the exact
/// bounding box of the painted box under the exact matrix the painter applies:
/// rotation, skew, and the perspective corner fit are all handled by
/// construction rather than approximated again here.
fn item_bounds(item: &PaintItem, affine: Affine, extent: f64) -> Rect {
    box_bounds(affine, item.size, extent)
}

/// [`item_bounds`] for a box that is not an item — the
/// `content-visibility: auto` relevance test's, so that the two ask about a
/// border box the same way.
fn box_bounds(affine: Affine, size: Size2D<f32>, extent: f64) -> Rect {
    affine_rect(
        affine,
        Rect::new(
            -extent,
            -extent,
            size.width as f64 + extent,
            size.height as f64 + extent,
        ),
    )
}

fn affine_rect(affine: Affine, rect: Rect) -> Rect {
    let corners = [
        affine * Point::new(rect.x0, rect.y0),
        affine * Point::new(rect.x1, rect.y0),
        affine * Point::new(rect.x0, rect.y1),
        affine * Point::new(rect.x1, rect.y1),
    ];
    let mut mapped = Rect::from_points(corners[0], corners[0]);
    for corner in &corners[1..] {
        mapped = mapped.union_pt(*corner);
    }
    mapped
}

fn blend_mode(style: &stylo::properties::ComputedValues) -> BlendMode {
    use stylo::computed_values::mix_blend_mode::T as MixBlendMode;
    let mix = match style.get_effects().mix_blend_mode {
        MixBlendMode::Normal => Mix::Normal,
        MixBlendMode::Multiply => Mix::Multiply,
        MixBlendMode::Screen => Mix::Screen,
        MixBlendMode::Overlay => Mix::Overlay,
        MixBlendMode::Darken => Mix::Darken,
        MixBlendMode::Lighten => Mix::Lighten,
        MixBlendMode::ColorDodge => Mix::ColorDodge,
        MixBlendMode::ColorBurn => Mix::ColorBurn,
        MixBlendMode::HardLight => Mix::HardLight,
        MixBlendMode::SoftLight => Mix::SoftLight,
        MixBlendMode::Difference => Mix::Difference,
        MixBlendMode::Exclusion => Mix::Exclusion,
        MixBlendMode::Hue => Mix::Hue,
        MixBlendMode::Saturation => Mix::Saturation,
        MixBlendMode::Color => Mix::Color,
        MixBlendMode::Luminosity => Mix::Luminosity,
        MixBlendMode::PlusLighter => {
            return BlendMode::new(Mix::Normal, Compose::Plus);
        }
    };
    BlendMode::new(mix, Compose::SrcOver)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use euclid::default::Transform3D;
    use vello::kurbo::Affine;

    use super::{
        ComposeAssembly, ComposeOp, CullPlan, PaintItem, PaintItemKind, Rect, Scene, Scratch,
        can_reach, cull_rect, item_bounds, walk, walk_compose, walk_uncultured,
    };
    use crate::Size2D;
    use crate::paint::equivalence::assert_scenes_identical;
    use crate::test_common::Doc;
    use crate::visual::CornerRadii;

    const VIEWPORT: (f32, f32) = (800.0, 600.0);

    /// A page sized to the viewport, laid out as a positioned canvas so a test
    /// can put a box anywhere, on screen or off.
    const PAGE: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
         .box { display: flex; position: absolute; width: 100px; height: 100px;
                background-color: teal; }";

    /// Both encodings of one frame: the one the painter produces, and the one
    /// it would produce if nothing were culled.
    struct Frames {
        cultured: Scene,
        uncultured: Scene,
        cultured_bounds: Vec<Rect>,
        uncultured_bounds: Vec<Rect>,
        painted: usize,
    }

    fn walk_twice(doc: &mut Doc) -> Frames {
        let frame = doc.dom.build_paint_order();
        let images = crate::render::image::ImageRegistry::default();
        let mut cultured = Scene::default();
        let mut cultured_scratch = Scratch::default();
        walk(
            &mut cultured,
            &mut cultured_scratch,
            &doc.dom,
            &frame,
            &images,
            &crate::NoImages,
        );
        let mut uncultured = Scene::default();
        let mut uncultured_scratch = Scratch::default();
        walk_uncultured(
            &mut uncultured,
            &mut uncultured_scratch,
            &doc.dom,
            &frame,
            &images,
            &crate::NoImages,
        );
        Frames {
            cultured,
            uncultured,
            cultured_bounds: cultured_scratch.layer_bounds.clone(),
            uncultured_bounds: uncultured_scratch.layer_bounds.clone(),
            painted: cultured_scratch
                .item_plan
                .iter()
                .filter(|plan| plan.is_some())
                .count(),
        }
    }

    fn draws(scene: &Scene) -> usize {
        scene.encoding().draw_tags.len()
    }

    fn item(x: f32, y: f32, size: f32) -> PaintItem {
        PaintItem {
            node: crate::tree::document::DOCUMENT_ELEMENT_NODE_ID,
            kind: PaintItemKind::ElementBox,
            transform: Transform3D::translation(x, y, 0.0),
            clip: None,
            size: Size2D::new(size, size),
            radii: CornerRadii::ZERO,
            hit_testable: true,
            slot: None,
            space: None,
        }
    }

    #[test]
    fn the_cull_rect_covers_every_device_pixel_the_target_can_hold() {
        // A target is `round(css * ratio)` device px, so it can reach half a
        // device pixel past the CSS viewport. A box ending inside that sliver
        // must survive at every scale.
        for ratio in [1.0_f64, 2.0, 3.0] {
            let rect = cull_rect(euclid::Size2D::new(VIEWPORT.0, VIEWPORT.1), ratio);
            let sliver = 0.5 / ratio;
            assert!(
                rect.x1 > f64::from(VIEWPORT.0) + sliver && rect.x0 < -sliver,
                "ratio {ratio} leaves the target's last device pixel outside the cull rect",
            );
        }
    }

    #[test]
    fn a_device_pixel_ratio_that_is_not_a_scale_leaves_the_cull_rect_unbounded() {
        for ratio in [0.0_f64, -1.0, f64::NAN, f64::INFINITY] {
            let rect = cull_rect(euclid::Size2D::new(VIEWPORT.0, VIEWPORT.1), ratio);
            assert!(
                rect.x0.is_infinite() && rect.x1.is_infinite(),
                "ratio {ratio} must disable the viewport half of the test",
            );
        }
    }

    #[test]
    fn nothing_undecidable_is_ever_culled() {
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        assert!(can_reach(Rect::new(10.0, 10.0, 20.0, 20.0), Some(viewport)));
        assert!(!can_reach(
            Rect::new(900.0, 10.0, 1000.0, 20.0),
            Some(viewport)
        ));
        assert!(
            !can_reach(Rect::new(10.0, 10.0, 20.0, 20.0), None),
            "a clip chain that admits nothing discards everything under it",
        );
        for edge in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                can_reach(Rect::new(900.0, edge, 1000.0, 20.0), Some(viewport)),
                "a bound containing {edge} is undecidable and must paint",
            );
        }
    }

    #[test]
    fn a_rotated_box_is_judged_by_its_rotated_bound() {
        // Rotating about the box's own centre moves its corners outward, so
        // the mapped bound is wider than the box. The cull test uses the
        // painter's own matrix, so it sees exactly that.
        let mut rotated = item(0.0, 0.0, 100.0);
        rotated.transform = Transform3D::rotation(0.0, 0.0, 1.0, euclid::Angle::degrees(45.0));
        let upright = item_bounds(&item(0.0, 0.0, 100.0), Affine::IDENTITY, 0.0);
        let affine = crate::paint::convert::item_affine(&rotated.transform, rotated.size)
            .expect("a rotation is invertible");
        let turned = item_bounds(&rotated, affine, 0.0);
        assert!(turned.width() > upright.width());
    }

    #[test]
    fn a_box_outside_the_viewport_whose_ink_reaches_it_still_encodes() {
        // The box-shadow blur and the outline both put ink beyond the border
        // box. A box parked just outside the viewport with one of them reaches
        // back in, and the box-only test that lets an obviously visible item
        // skip its style lookup must not be the one that decides this.
        for (extra, offset) in [
            (
                ".shadow { box-shadow: 0px 0px 40px 20px rgba(0,0,0,0.9); }",
                -130.0,
            ),
            (".ring { outline: 20px solid orange; }", -110.0),
        ] {
            let class = if extra.starts_with(".shadow") {
                "shadow"
            } else {
                "ring"
            };
            let mut doc = Doc::with_css(&format!("{PAGE} {extra}"));
            let root = doc.root;
            let reaching = doc.el(root, &format!("view.box.{class}"));
            doc.set_inline(reaching, &format!("left: {offset}px; top: 20px"));
            let frames = walk_twice(&mut doc);
            assert_eq!(frames.painted, 2, "{class}: its ink reaches the viewport");
            assert_scenes_identical(&frames.cultured, &frames.uncultured);

            let mut doc = Doc::with_css(&format!("{PAGE} {extra}"));
            let root = doc.root;
            let far = doc.el(root, &format!("view.box.{class}"));
            doc.set_inline(far, "left: -900px; top: 20px");
            let frames = walk_twice(&mut doc);
            assert_eq!(frames.painted, 1, "{class}: nothing of it reaches");
        }
    }

    #[test]
    fn culling_does_not_change_the_encoding_of_on_viewport_content() {
        let mut doc = Doc::with_css(&format!(
            "{PAGE}
             .fade {{ opacity: 0.6; }}
             .clip {{ overflow: hidden; }}
             .shadow {{ box-shadow: 0px 4px 12px rgba(0,0,0,0.4); }}
             .ring {{ outline: 3px solid orange; }}
             .round {{ border-radius: 12px; border: 2px solid navy; }}"
        ));
        let root = doc.root;
        for (index, extra) in ["fade", "clip", "shadow", "ring", "round"]
            .into_iter()
            .enumerate()
        {
            let box_id = doc.el(root, &format!("view.box.{extra}"));
            doc.set_inline(box_id, &format!("left: {}px; top: 20px", index * 120));
            doc.el(box_id, "view.box");
        }

        let frames = walk_twice(&mut doc);
        assert!(draws(&frames.cultured) > 0);
        assert_scenes_identical(&frames.cultured, &frames.uncultured);
        assert_eq!(
            frames.cultured_bounds, frames.uncultured_bounds,
            "group bounds must be computed from every item, culled or not",
        );
    }

    #[test]
    fn a_box_straddling_the_viewport_edge_still_encodes() {
        let mut doc = Doc::with_css(PAGE);
        let root = doc.root;
        for (left, top) in [(-40.0, 20.0), (760.0, 20.0), (20.0, -40.0), (20.0, 560.0)] {
            let box_id = doc.el(root, "view.box");
            doc.set_inline(box_id, &format!("left: {left}px; top: {top}px"));
        }
        let frames = walk_twice(&mut doc);
        assert_eq!(frames.painted, 5, "the page and all four straddlers paint");
        assert_scenes_identical(&frames.cultured, &frames.uncultured);
    }

    #[test]
    fn content_wholly_outside_the_viewport_encodes_nothing() {
        let mut doc = Doc::with_css(PAGE);
        let root = doc.root;
        let on_screen = doc.el(root, "view.box");
        doc.set_inline(on_screen, "left: 20px; top: 20px");
        let visible = walk_twice(&mut doc);

        for index in 0..40 {
            let far = doc.el(root, "view.box");
            doc.set_inline(far, &format!("left: 2000px; top: {}px", index * 120));
        }
        let with_far = walk_twice(&mut doc);

        assert_eq!(
            draws(&visible.cultured),
            draws(&with_far.cultured),
            "boxes outside the viewport must not reach the encoding",
        );
        assert!(
            draws(&with_far.uncultured) > draws(&with_far.cultured),
            "the uncultured walk must encode them, or the fixture proves nothing",
        );
        assert_eq!(visible.painted, with_far.painted);
    }

    #[test]
    fn rows_clipped_out_of_their_scroll_container_encode_nothing() {
        // The container sits inside the viewport, so viewport culling alone
        // would keep every row. What discards them is the clip-chain bound.
        let mut doc = Doc::with_css(
            "page { display: flex; position: relative; width: 800px; height: 600px; }
             .list { display: flex; flex-direction: column; overflow: scroll;
                     width: 300px; height: 120px; }
             .row { display: flex; flex-shrink: 0; width: 300px; height: 40px;
                    background-color: teal; }",
        );
        let root = doc.root;
        let list = doc.el(root, "view.list");
        for _ in 0..8 {
            doc.el(list, "view.row");
        }
        let short = walk_twice(&mut doc);

        for _ in 0..200 {
            doc.el(list, "view.row");
        }
        let long = walk_twice(&mut doc);

        assert_eq!(
            draws(&short.cultured),
            draws(&long.cultured),
            "rows below the scrollport must not reach the encoding",
        );
        assert!(draws(&long.uncultured) > draws(&long.cultured));
    }

    /// The same bound for rows a `z-index` sorts out of their scroller into
    /// the page's stacking context: they paint there, but still carry the
    /// scroller's clip, so the rows below its scrollport stay unencoded.
    #[test]
    fn hoisted_rows_clipped_out_of_their_scroll_container_encode_nothing() {
        let mut doc = Doc::with_css(
            "page { display: flex; position: relative; width: 800px; height: 600px; }
             .list { display: flex; flex-direction: column; overflow: scroll;
                     width: 300px; height: 120px; }
             .row { display: flex; flex-shrink: 0; position: relative; z-index: 1;
                    width: 300px; height: 40px; background-color: teal; }",
        );
        let root = doc.root;
        let list = doc.el(root, "view.list");
        for _ in 0..8 {
            doc.el(list, "view.row");
        }
        let short = walk_twice(&mut doc);

        for _ in 0..200 {
            doc.el(list, "view.row");
        }
        let long = walk_twice(&mut doc);

        assert_eq!(
            draws(&short.cultured),
            draws(&long.cultured),
            "hoisted rows below the scrollport must not reach the encoding",
        );
        assert!(draws(&long.uncultured) > draws(&long.cultured));
    }

    /// A list inside a card sliding by an exported transform curve encodes
    /// what the same list in a still card does: the viewport no longer
    /// bounds the card's subtree, but the list's clip and encode window move
    /// with it and still do.
    #[test]
    fn a_list_inside_an_animated_card_still_encodes_only_its_window() {
        let painted = |animation: &str| {
            let mut doc = Doc::with_css(&format!(
                "page {{ display: flex; position: relative; width: 800px; height: 600px; }}
                 .card {{ display: flex; padding: 10px; background-color: navy; {animation} }}
                 .list {{ display: flex; flex-direction: column; overflow: scroll;
                          width: 300px; height: 120px; }}
                 .row {{ display: flex; flex-shrink: 0; width: 300px; height: 40px;
                         background-color: teal; }}
                 @keyframes slide {{ from {{ transform: translateX(0px); }}
                                     to {{ transform: translateX(200px); }} }}"
            ));
            let root = doc.root;
            let card = doc.el(root, "view.card");
            let list = doc.el(card, "view.list");
            for _ in 0..200 {
                doc.el(list, "view.row");
            }
            doc.dom.render();
            doc.dom.advance_animations(0.0);
            doc.dom.advance_animations(0.25);
            let exported = doc.dom.build_paint_order().animations().len();
            (exported, walk_twice(&mut doc).painted)
        };
        let (still_curves, still) = painted("");
        let (moving_curves, moving) = painted("animation: slide 1s linear infinite;");
        assert_eq!((still_curves, moving_curves), (0, 1), "the slide exports");
        assert_eq!(moving, still, "the moving list encodes its window alone");
        assert!(
            still < 20,
            "the window holds a handful of 200 rows, got {still}"
        );
    }

    /// Starts every animation on the timeline's origin and advances a
    /// quarter second in, the state the exporter reads.
    fn run_animations(doc: &mut Doc) {
        doc.dom.render();
        doc.dom.advance_animations(0.0);
        doc.dom.advance_animations(0.25);
    }

    const SLIDE: &str = "@keyframes slide { from { transform: translateX(0px); }
                                             to { transform: translateX(600px); } }";

    /// A box a transform curve moves is never culled by the viewport, which
    /// cannot bound where the sampled delta carries it.
    #[test]
    fn an_off_viewport_box_a_transform_curve_moves_still_encodes() {
        let painted = |animation: &str| {
            let mut doc = Doc::with_css(&format!("{PAGE} .mover {{ {animation} }} {SLIDE}"));
            let root = doc.root;
            let mover = doc.el(root, "view.box.mover");
            doc.set_inline(mover, "left: -300px; top: 20px");
            run_animations(&mut doc);
            let exported = doc.dom.build_paint_order().animations().len();
            (exported, walk_twice(&mut doc).painted)
        };
        assert_eq!(
            painted(""),
            (0, 1),
            "a still box off the viewport is culled"
        );
        assert_eq!(
            painted("animation: slide 1s linear infinite;"),
            (1, 2),
            "the slide exports and the box encodes, off the viewport at commit",
        );
    }

    /// The bounds of a group a transform curve moves: the only layer of a
    /// page built from `css` by `build`, which must export one curve.
    fn moving_layer_bounds(css: &str, build: impl FnOnce(&mut Doc)) -> Rect {
        let mut doc = Doc::with_css(&format!(
            "page {{ display: flex; position: relative; width: 800px; height: 600px; }}
             .list {{ display: flex; flex-direction: column; overflow: scroll;
                      width: 300px; height: 200px; }}
             .row {{ display: flex; flex-shrink: 0; width: 300px; height: 40px;
                     background-color: teal; }}
             {SLIDE} {css}"
        ));
        build(&mut doc);
        run_animations(&mut doc);
        assert_eq!(doc.dom.build_paint_order().animations().len(), 1);
        let frames = walk_twice(&mut doc);
        assert_eq!(
            frames.cultured_bounds, frames.uncultured_bounds,
            "a moving group's bounds do not depend on culling either",
        );
        let [bounds] = frames.cultured_bounds[..] else {
            panic!("one group layer, got {:?}", frames.cultured_bounds);
        };
        bounds
    }

    /// A blurred card sliding by an exported curve holds its bounds — its
    /// bake rect — to what its list's clip and encode window admit: the
    /// viewport cannot bound a moving group, but the clips moving with it
    /// can, and the list's 12000 px of rows are not in the card's extent.
    #[test]
    fn a_moving_blurred_card_bakes_its_lists_window_not_its_content() {
        let bounds = moving_layer_bounds(
            ".card { display: flex; padding: 10px; filter: blur(2px);
                     animation: slide 1s linear infinite; }",
            |doc| {
                let card = doc.el(doc.root, "view.card");
                let list = doc.el(card, "view.list");
                for _ in 0..300 {
                    doc.el(list, "view.row");
                }
            },
        );
        assert!(
            bounds.height() < 1000.0,
            "the list's window, not its rows, got {bounds:?}"
        );
    }

    /// A blurred panel inside a list inside a sliding card: the panel's
    /// group moves, and the list's clip moving with it bounds the panel's own
    /// 5000 px box as well as its rows.
    #[test]
    fn a_blurred_panel_in_a_list_in_a_moving_card_bakes_the_lists_window() {
        let bounds = moving_layer_bounds(
            ".card { display: flex; animation: slide 1s linear infinite; }
             .panel { display: flex; flex-direction: column; flex-shrink: 0;
                      filter: blur(2px); }",
            |doc| {
                let card = doc.el(doc.root, "view.card");
                let list = doc.el(card, "view.list");
                let panel = doc.el(list, "view.panel");
                for _ in 0..125 {
                    doc.el(panel, "view.row");
                }
            },
        );
        assert!(
            bounds.height() < 1000.0,
            "the list's window, not the panel, got {bounds:?}"
        );
    }

    /// Relevance follows the cull rule: under a sliding card a clip-free
    /// `content-visibility: auto` box is relevant wherever it sits, and one
    /// inside the card's list only inside the list's encode window.
    #[test]
    fn relevance_under_a_moving_card_is_bounded_by_the_clips_moving_with_it() {
        let mut doc = Doc::with_css(&format!(
            "page {{ display: flex; position: relative; width: 800px; height: 600px; }}
             .card {{ display: flex; position: relative; animation: slide 1s linear infinite; }}
             .list {{ display: flex; flex-direction: column; overflow: scroll;
                      width: 300px; height: 120px; }}
             .row {{ display: flex; flex-shrink: 0; width: 300px; height: 40px;
                     content-visibility: auto; contain-intrinsic-size: 300px 40px; }}
             .free {{ display: flex; position: absolute; left: -500px; top: 0px;
                      width: 100px; height: 40px; content-visibility: auto;
                      contain-intrinsic-size: 100px 40px; }}
             {SLIDE}"
        ));
        let card = doc.el(doc.root, "view.card");
        let list = doc.el(card, "view.list");
        let rows: Vec<_> = (0..40).map(|_| doc.el(list, "view.row")).collect();
        let free = doc.el(card, "view.free");
        run_animations(&mut doc);
        let frame = doc.dom.build_paint_order();
        assert_eq!(frame.animations().len(), 1, "the slide exports");
        let mut plan = CullPlan::default();
        plan.resolve_for(&doc.dom, &frame);
        let relevant = |node| {
            let auto = frame
                .auto_boxes()
                .iter()
                .find(|auto| auto.node == node)
                .expect("every auto box is recorded");
            plan.admits_auto_box(&frame, auto)
        };
        assert!(relevant(free), "the slide can carry it into view");
        assert!(relevant(rows[0]), "inside the list's window");
        assert!(!relevant(rows[39]), "past the list's window");
    }

    #[test]
    fn a_wholly_culled_group_stays_layer_balanced() {
        let mut doc = Doc::with_css(&format!(
            "{PAGE}
             .fade {{ opacity: 0.5; }}
             .blend {{ mix-blend-mode: multiply; }}"
        ));
        let root = doc.root;
        for extra in ["fade", "blend"] {
            let group = doc.el(root, &format!("view.box.{extra}"));
            doc.set_inline(group, "left: 5000px; top: 5000px");
            doc.el(group, "view.box");
        }
        let frames = walk_twice(&mut doc);
        assert_eq!(
            frames.painted, 1,
            "only the page item is left: both groups and their members go",
        );
        assert!(
            draws(&frames.uncultured) > draws(&frames.cultured),
            "the uncultured walk must encode them, or the fixture proves nothing",
        );
        assert_eq!(
            frames.cultured.encoding().n_open_clips,
            0,
            "every pushed layer must still be popped",
        );
        assert_eq!(
            frames.cultured_bounds, frames.uncultured_bounds,
            "a culled member still contributes to its group's bounds",
        );
    }

    /// A blurred group's bounds carry the 3 sigma ink margin, and the cull
    /// region admits everything that can reach it.
    #[test]
    fn a_blurred_group_keeps_the_ink_margin_in_its_bounds() {
        let mut doc = Doc::with_css(&format!("{PAGE} .blurred {{ filter: blur(4px); }}"));
        let root = doc.root;
        let group = doc.el(root, "view.box.blurred");
        doc.set_inline(group, "left: 100px; top: 100px");
        let frames = walk_twice(&mut doc);
        let bounds = frames.cultured_bounds[0];
        // The box is 100x100 at (100, 100), so 3 sigma = 12 px of margin
        // lands its bounds on (88, 88)-(212, 212).
        assert!(
            (bounds.x0 - 88.0).abs() < 0.5
                && (bounds.y0 - 88.0).abs() < 0.5
                && (bounds.x1 - 212.0).abs() < 0.5
                && (bounds.y1 - 212.0).abs() < 0.5,
            "a blurred group's bounds carry 3 sigma on every side ({bounds:?})",
        );
        assert_eq!(
            frames.cultured_bounds, frames.uncultured_bounds,
            "the margin is not a cull decision",
        );

        // A box whose own border box is off screen, but whose blur reaches
        // back onto it, must still paint.
        let mut doc = Doc::with_css(&format!("{PAGE} .blurred {{ filter: blur(40px); }}"));
        let root = doc.root;
        let group = doc.el(root, "view.box.blurred");
        doc.set_inline(group, "left: 860px; top: 100px");
        let frames = walk_twice(&mut doc);
        assert_eq!(
            frames.painted, 2,
            "the page and the box whose blur reaches the viewport",
        );
    }

    // -----------------------------------------------------------------
    // `backdrop-filter`
    // -----------------------------------------------------------------

    /// One compose walk's program and the layer bounds it pushed with.
    fn compose(doc: &mut Doc) -> (crate::paint::compose::Finished, Vec<Rect>) {
        let frame = doc.dom.build_paint_order();
        let images = crate::render::image::ImageRegistry::default();
        let mut assembly = ComposeAssembly::default();
        let mut scratch = Scratch::default();
        walk_compose(&mut assembly, &mut scratch, &doc.dom, &frame, &images);
        let bounds = scratch.layer_bounds.clone();
        (assembly.finish(), bounds)
    }

    /// A marker box, a wrapper carrying `wrapper`, and inside it a second
    /// marker and a `backdrop-filter` box.
    ///
    /// The wrapper is always a group scope, so the two markers always land in
    /// two fragments; what varies is whether the wrapper is a *Backdrop Root*,
    /// and therefore whether the outer marker is in the box's range.
    const BACKDROP_PAGE: &str =
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .mark { display: flex; position: absolute; left: 0px; top: 0px;
                 width: 50px; height: 50px; background-color: teal; }
         .wrap { display: flex; position: absolute; left: 0px; top: 0px;
                 width: 200px; height: 200px; }
         .box { display: flex; position: absolute; left: 10px; top: 10px;
                width: 100px; height: 100px; backdrop-filter: blur(4px); }";

    /// How many fragments a `backdrop-filter` box's range holds, on a page
    /// whose wrapper carries `wrapper`.
    fn fragments_behind(wrapper: &str) -> (usize, u32) {
        let mut doc = Doc::with_css(BACKDROP_PAGE);
        let root = doc.root;
        doc.el(root, "view.mark");
        let wrap = doc.el(root, "view.wrap");
        doc.set_inline(wrap, wrapper);
        doc.el(wrap, "view.mark");
        doc.el(wrap, "view.box");
        let (finished, _) = compose(&mut doc);
        assert_eq!(finished.filter_groups.len(), 1, "one backdrop entry");
        let entry = &finished.filter_groups[0];
        assert!(entry.is_backdrop());
        let count = finished.program[entry.ops.start as usize..entry.ops.end as usize]
            .iter()
            .filter(|op| matches!(op, ComposeOp::Fragment { .. }))
            .count();
        (count, entry.ops.start)
    }

    /// The range begins at the nearest Backdrop Root's content start.
    ///
    /// The second loop pins the two deliberate non-roots beside the two
    /// stacking contexts that were never roots to begin with.
    /// `isolation: isolate` is not in the spec's Backdrop Root list (and is
    /// not in the fork's author grammar either, so its exclusion is
    /// unobservable), while `will-change: opacity` **is** one per the spec:
    /// this engine opens no group layer for it, so the element inside sees
    /// through it. Ruled, and recorded in `docs/tracking/deviations.md`.
    #[test]
    fn a_backdrops_range_begins_at_its_backdrop_root() {
        for wrapper in [
            "opacity: 0.5",
            "filter: grayscale(1)",
            "clip-path: inset(0px)",
            "mask-image: linear-gradient(black, transparent)",
        ] {
            let (count, start) = fragments_behind(wrapper);
            assert_eq!(count, 1, "{wrapper}: only what it painted itself");
            assert!(start > 0, "{wrapper}: and not from the frame's start");
        }
        // A stacking context with no group effect roots nothing, so the range
        // runs from the frame's own start — which is the document root
        // element's, the spec's root Backdrop Root. `will-change: opacity` is
        // the deviation: the spec makes it a Backdrop Root and we do not.
        for wrapper in [
            "transform: translate(0px)",
            "z-index: 1",
            "will-change: opacity",
        ] {
            let (count, start) = fragments_behind(wrapper);
            assert_eq!(start, 0, "{wrapper} is no Backdrop Root");
            assert!(count >= 1, "{wrapper}: and the whole prefix is in range");
        }
    }

    /// An element with both properties records its blur bracket first, so the
    /// filtered backdrop is baked *inside* the element's own blur group — and
    /// the backdrop's own range ends before either, at the element's first
    /// layer push.
    #[test]
    fn a_filter_bracket_precedes_the_backdrop_it_encloses() {
        let mut doc = Doc::with_css(BACKDROP_PAGE);
        let root = doc.root;
        doc.el(root, "view.mark");
        let boxed = doc.el(root, "view.box");
        doc.set_inline(boxed, "filter: blur(4px)");
        let (finished, _) = compose(&mut doc);

        let bracket = finished
            .program
            .iter()
            .position(|op| matches!(op, ComposeOp::PushFilter { .. }))
            .expect("the blur group's bracket");
        let backdrop = finished
            .program
            .iter()
            .position(|op| matches!(op, ComposeOp::PushBackdrop { .. }))
            .expect("the backdrop op");
        assert_eq!(
            backdrop,
            bracket + 1,
            "the backdrop is innermost, inside the blur bracket",
        );
        let entry = finished
            .filter_groups
            .iter()
            .find(|entry| entry.is_backdrop())
            .expect("the backdrop entry");
        assert!(
            entry.ops.end < bracket as u32,
            "and its own range stops before the element's own painting",
        );
        assert!(
            matches!(
                finished.program[entry.ops.end as usize],
                ComposeOp::Push { .. }
            ),
            "which is exactly the element's first layer push",
        );
    }

    /// `backdrop-filter` enlarges no ink overflow: the group's pushed rect is
    /// the element's own box, where the same σ as a `filter` grows it by 3σ.
    #[test]
    fn a_backdrop_inflates_no_bounds() {
        let css = "page { display: flex; position: relative; width: 800px; height: 600px; }
             .box { display: flex; position: absolute; left: 200px; top: 200px;
                    width: 100px; height: 100px; background-color: teal; }";
        let mut bounds = Vec::new();
        for extra in [
            "opacity: 0.99",
            "opacity: 0.99; backdrop-filter: blur(20px)",
            "opacity: 0.99; filter: blur(20px)",
        ] {
            let mut doc = Doc::with_css(css);
            let root = doc.root;
            let boxed = doc.el(root, "view.box");
            doc.set_inline(boxed, extra);
            let (_, layer_bounds) = compose(&mut doc);
            assert_eq!(layer_bounds.len(), 1, "{extra}: one group layer");
            bounds.push(layer_bounds[0]);
        }
        assert_eq!(bounds[0], bounds[1], "a backdrop inflates nothing");
        assert!(
            (bounds[2].x0 - bounds[0].x0 + 60.0).abs() < 1e-6,
            "while a filter grows the same box by 3 sigma ({:?} vs {:?})",
            bounds[2],
            bounds[0],
        );
    }
}
