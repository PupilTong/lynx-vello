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
//!    pattern drawn first, then a `Compose::SrcIn` layer holding the content. Filter adjustments
//!    draw at scope close, inside the innermost layer, after in-scope clips pop. Composite order on
//!    pop is therefore filter → mask → clip-path → opacity/blend — clip and mask are both
//!    intersective alpha ops, so the swap versus the spec's filter → clip → mask is unobservable.
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
//! This extends a rule the walker already applied rather than inventing one:
//! every group's effect layer is pushed clipped to bounds already intersected
//! with the viewport, so an item inside any `opacity` or `mask` group has been
//! viewport-culled at layer granularity since group compositing landed.
//!
//! Three disciplines keep it sound.
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

use euclid::default::Vector2D;

use crate::paint::compose::{CapturedShape, ComposeAssembly, ComposeChain, ComposeOp};
use crate::paint::shape::{BoxShape, with_shape};
use crate::paint::{
    BoxFragment, PathScratch, background, border, convert, filters, mask, shadow, text,
};
use crate::vello::Scene;
use crate::vello::kurbo::{Affine, Point, Rect};
use crate::vello::peniko::{BlendMode, Compose, Fill, Mix};
use crate::visual::{ClipNode, PaintItem, PaintItemKind, PaintOrder, RenderLayer, ScrollSlot};
use crate::{Document, ImageStore};

/// Where one walk's output goes.
///
/// The walker's traversal, scope, and clip logic is identical either way;
/// only the destination of each operation differs. `Monolithic` is the
/// pre-compose shape — one scene, everything inline — kept for the
/// equivalence tests; production encodes through `Compose`, where
/// walker-level pushes become program ops and content between them lands in
/// per-chain fragments.
pub(crate) enum WalkSink<'s> {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "constructed only by the equivalence tests' monolithic walk"
        )
    )]
    Monolithic(&'s mut Scene),
    Compose(&'s mut ComposeAssembly),
}

impl WalkSink<'_> {
    /// The scene content riding `chain` encodes into.
    fn scene_for(&mut self, chain: ComposeChain) -> &mut Scene {
        match self {
            Self::Monolithic(scene) => scene,
            Self::Compose(assembly) => assembly.fragment_for(chain),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors vello's push_layer plus the compose tags"
    )]
    fn push_layer_rect(
        &mut self,
        chain: ComposeChain,
        alpha_animation: Option<u32>,
        fill: Fill,
        blend: BlendMode,
        alpha: f32,
        transform: Affine,
        rect: Rect,
    ) {
        match self {
            Self::Monolithic(scene) => scene.push_layer(fill, blend, alpha, transform, &rect),
            Self::Compose(assembly) => assembly.push_op(ComposeOp::Push {
                clip_only: false,
                fill,
                blend,
                alpha,
                transform,
                shape: CapturedShape::Rect(rect),
                chain,
                alpha_animation,
            }),
        }
    }

    fn push_layer_box(
        &mut self,
        chain: ComposeChain,
        fill: Fill,
        blend: BlendMode,
        alpha: f32,
        transform: Affine,
        shape: BoxShape,
    ) {
        match self {
            Self::Monolithic(scene) => {
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
                chain,
                alpha_animation: None,
            }),
        }
    }

    fn push_clip_box(
        &mut self,
        chain: ComposeChain,
        fill: Fill,
        transform: Affine,
        shape: BoxShape,
    ) {
        match self {
            Self::Monolithic(scene) => {
                with_shape!(&shape, |s| scene.push_clip_layer(fill, transform, s));
            }
            Self::Compose(assembly) => assembly.push_op(ComposeOp::Push {
                clip_only: true,
                fill,
                blend: BlendMode::new(Mix::Normal, Compose::SrcOver),
                alpha: 1.0,
                transform,
                shape: CapturedShape::Box(shape),
                chain,
                alpha_animation: None,
            }),
        }
    }

    fn push_clip_empty(&mut self, chain: ComposeChain) {
        match self {
            Self::Monolithic(scene) => {
                scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::ZERO);
            }
            Self::Compose(assembly) => assembly.push_op(ComposeOp::Push {
                clip_only: true,
                fill: Fill::NonZero,
                blend: BlendMode::new(Mix::Normal, Compose::SrcOver),
                alpha: 1.0,
                transform: Affine::IDENTITY,
                shape: CapturedShape::Rect(Rect::ZERO),
                chain,
                alpha_animation: None,
            }),
        }
    }

    fn pop(&mut self) {
        match self {
            Self::Monolithic(scene) => scene.pop_layer(),
            Self::Compose(assembly) => assembly.push_op(ComposeOp::Pop),
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
    /// Per clip node, the region its whole chain admits, in viewport CSS px,
    /// already intersected with the frame's cull rect. `None` means the chain
    /// admits nothing: it leaves the cull rect, or one of its links has a
    /// non-invertible transform, which [`push_clip`] encodes as an empty clip.
    /// Index-parallel with [`PaintOrder::clips`].
    clip_bounds: Vec<Option<Rect>>,
    /// Per item, the item-local to viewport-CSS-px map [`paint_item`] paints
    /// with, or `None` when the item encodes nothing — a non-invertible
    /// transform, or no reachable ink. Index-parallel with
    /// [`PaintOrder::items`].
    item_plan: Vec<Option<Affine>>,
    /// Per scroll slot, the committed encode window `(low, high)` — the
    /// offset range the culled encode must stay valid for. Index-parallel
    /// with [`PaintOrder::slots`].
    slot_windows: Vec<(Vector2D<f32>, Vector2D<f32>)>,
    /// Per animation slot, whether a live transform curve moves its chain —
    /// transitive through slot parents. Content on a moving chain is never
    /// culled and its enclosing groups keep unclipped bounds: the sampled
    /// delta can carry it anywhere.
    animation_moves: Vec<bool>,
    paths: PathScratch,
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
/// `layer` is what the group bounds are accumulated from and is exactly what
/// this pass computed before it gained a second job. `cull` is never smaller,
/// and is infinite when the reach cannot be established at all.
#[derive(Clone, Copy, Debug)]
struct Extents {
    layer: f64,
    cull: f64,
}

/// The read-only context every step of one walk shares.
struct Painting<'a, T> {
    document: &'a Document<T>,
    frame: &'a PaintOrder,
    images: &'a dyn ImageStore,
    /// The document's device scale, applied once at the root: the paint order
    /// is in viewport CSS px and the scene is in device px.
    scale: Affine,
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
}

/// One culled monolithic walk — the pre-compose shape, kept beside
/// [`walk_compose`] for the equivalence tests.
#[cfg(test)]
pub(crate) fn walk<T>(
    scene: &mut Scene,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    images: &dyn ImageStore,
) {
    let device = document.device();
    let ratio = f64::from(device.device_pixel_ratio().get());
    let cull = cull_rect(device.viewport_size(), ratio);
    let mut sink = WalkSink::Monolithic(scene);
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

/// The production walk: encodes the frame as per-chain fragments plus the
/// compose program over them.
pub(crate) fn walk_compose<T>(
    assembly: &mut ComposeAssembly,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    images: &dyn ImageStore,
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

/// [`walk`] with culling switched off entirely, which is what this walker did
/// before culling existed.
///
/// The tests encode one frame both ways and compare, which is the statement
/// that culling changes nothing an observer of the viewport can see.
#[cfg(test)]
pub(crate) fn walk_uncultured<T>(
    scene: &mut Scene,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    images: &dyn ImageStore,
) {
    let ratio = f64::from(document.device().device_pixel_ratio().get());
    let mut sink = WalkSink::Monolithic(scene);
    walk_within(&mut sink, scratch, document, frame, images, ratio, None);
}

/// [`walk`] against an explicit admitted region, in viewport CSS px, or
/// `None` to encode every item.
fn walk_within<T>(
    sink: &mut WalkSink<'_>,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    images: &dyn ImageStore,
    ratio: f64,
    cull: Option<Rect>,
) {
    frame.assert_visually_fresh(document);
    scratch.clip_stack.clear();
    scratch.chain.clear();
    scratch.scopes.clear();
    scratch.slot_windows.clear();
    scratch.slot_windows.extend(
        frame
            .slots()
            .iter()
            .map(crate::visual::ScrollSlot::encode_window),
    );
    scratch.animation_moves.clear();
    for slot in frame.animations() {
        let own = slot
            .curve
            .as_ref()
            .is_some_and(|curve| curve.transform.is_some());
        let inherited = slot
            .parent
            .is_some_and(|parent| scratch.animation_moves[parent as usize]);
        scratch.animation_moves.push(own || inherited);
    }
    plan_clips(scratch, frame, cull);
    plan_frame(scratch, document, frame, cull);

    let painting = Painting {
        document,
        frame,
        images,
        scale: Affine::scale(ratio),
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

/// Resolves every clip chain in the frame to the region it admits, in viewport
/// CSS px, intersected with `cull`.
///
/// One forward pass suffices because a clip node's parent is always an earlier
/// entry: [`crate::visual`]'s builder pushes a clip only after the clip it
/// nests inside.
fn plan_clips(scratch: &mut Scratch, frame: &PaintOrder, cull: Option<Rect>) {
    scratch.clip_bounds.clear();
    let Some(cull) = cull else {
        return;
    };
    for (index, clip) in frame.clips().iter().enumerate() {
        debug_assert!(
            clip.parent.is_none_or(|parent| parent < index),
            "a clip node nests inside an earlier clip node",
        );
        // The inherited region is expanded into this clip's chain
        // coordinates: everything here is baked unscrolled, so a region on
        // an outer chain admits content on an inner one anywhere the inner
        // slots' encode windows can carry it.
        let inherited = admitted_region(scratch, frame, cull, clip.slot, clip.parent);
        let resolved = inherited.and_then(|inherited| {
            // `push_clip` pushes an empty clip for a singular transform, so
            // nothing under this chain reaches the scene at all.
            let own = clip_bounds(clip)?;
            if !is_finite(own) {
                return Some(inherited);
            }
            let both = own.intersect(inherited);
            (both.width() > 0.0 && both.height() > 0.0).then_some(both)
        });
        scratch.clip_bounds.push(resolved);
    }
}

/// One clip node's rounded rect as an axis-aligned viewport-CSS-px bound.
///
/// Mirrors [`push_clip`], including its `Size2D`-only perspective fit, so this
/// is never tighter than the clip the painter pushes; taking the bounding box
/// of a rounded rect only widens it, which is the safe direction.
pub(crate) fn clip_bounds(clip: &ClipNode) -> Option<Rect> {
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
        let (window_low, window_high) = windows[index as usize];
        low += window_low;
        high += window_high;
        chain = slots[index as usize].parent;
    }
    let mut chain = frame_of;
    while chain != lca {
        let Some(index) = chain else { break };
        let (window_low, window_high) = windows[index as usize];
        low -= window_high;
        high -= window_low;
        chain = slots[index as usize].parent;
    }
    (low, high)
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
    } = painting;
    let layer = &frame.layers()[layer_index];
    let chain = ComposeChain {
        scroll: layer.slot,
        animation: layer.animation,
    };
    let base = scratch.scopes.last().map_or(0, |scope| scope.base);
    pop_clips_to(sink, scratch, base);

    let style = document
        .paint_style(layer.node)
        .expect("a group-effect stacking context keeps its style for the frame");
    let bounds = scratch.layer_bounds[layer_index];
    let effects = style.get_effects();
    let blend = blend_mode(style);
    let mut pushed = 1_u32;
    // The effect layer's alpha is replaced at compose time when this group's
    // own element exports an opacity curve.
    let alpha_animation = layer
        .animation
        .filter(|&slot| frame.animations()[slot as usize].node == layer.node);
    sink.push_layer_rect(
        chain,
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

    if let Some(fragment) = fragment.as_ref()
        && let Some((clip_shape, fill)) =
            crate::paint::shape::clip_path_shape(style, &fragment.reference_boxes())
    {
        match local {
            Some(local) => sink.push_layer_box(
                chain,
                fill,
                BlendMode::new(Mix::Normal, Compose::SrcOver),
                1.0,
                scale * local,
                clip_shape,
            ),
            None => sink.push_layer_rect(
                chain,
                None,
                Fill::NonZero,
                BlendMode::new(Mix::Normal, Compose::SrcOver),
                1.0,
                Affine::IDENTITY,
                Rect::ZERO,
            ),
        }
        pushed += 1;
    }

    if mask::has_mask(style) {
        if let Some(fragment) = fragment.as_ref() {
            mask::paint(sink.scene_for(chain), style, fragment, images);
        }
        sink.push_layer_rect(
            chain,
            None,
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::SrcIn),
            1.0,
            scale,
            bounds,
        );
        pushed += 1;
    }

    scratch.scopes.push(Scope {
        layer: layer_index,
        base,
        pushed,
        filtered: !effects.filter.0.is_empty(),
    });
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
    if scope.filtered {
        let layer = &frame.layers()[scope.layer];
        if let Some(style) = document.paint_style(layer.node) {
            filters::apply(
                sink.scene_for(ComposeChain {
                    scroll: layer.slot,
                    animation: layer.animation,
                }),
                style,
                scratch.layer_bounds[scope.layer],
                scale,
            );
        }
    }
    for _ in 0..scope.pushed {
        sink.pop();
    }
}

/// Paints one item with the matrix [`plan_frame`] resolved for it.
///
/// The matrix is handed in rather than recomputed so that the culler's
/// geometry and the painter's are the same value by construction, not by
/// convention.
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
    } = painting;
    sync_clips(sink, scratch, frame, item, scale);
    let scene = sink.scene_for(frame.item_compose_chain(item));
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
            shadow::paint_outset(scene, &mut scratch.paths, style, &fragment);
            background::paint(scene, style, &fragment, images, text_clip.as_ref());
            shadow::paint_inset(scene, &mut scratch.paths, style, &fragment);
            if let Some(source) = document.image_source(item.node) {
                background::paint_replaced_content(
                    scene,
                    style,
                    &fragment,
                    images,
                    source,
                    document.natural_size(item.node),
                );
            }
            border::paint(scene, &mut scratch.paths, style, &fragment);
            border::paint_outline(scene, &mut scratch.paths, style, &fragment);
        }
        PaintItemKind::TextRun { element } => {
            let Some(style) = document.paint_style(element) else {
                return;
            };
            let Some(layout) = document.text_layout(item.node) else {
                return;
            };
            let decorations = text::propagated_decorations(document, element);
            let gradient_box = text::needs_gradient_box(style)
                .then(|| color_gradient_box(document, item, element));
            text::paint(scene, style, layout, transform, &decorations, gradient_box);
        }
    }
}

fn color_gradient_box<T>(document: &Document<T>, item: &PaintItem, element: crate::NodeId) -> Rect {
    let own_box = Rect::new(
        0.0,
        0.0,
        f64::from(item.size.width),
        f64::from(item.size.height),
    );
    let (Some(element_layout), Some(run_layout)) = (
        document.rounded_layout(element),
        document.rounded_layout(item.node),
    ) else {
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
    padding_box
        - crate::vello::kurbo::Vec2::new(
            f64::from(run_layout.location.x),
            f64::from(run_layout.location.y),
        )
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
    for child in node_ref.flat_children_iter() {
        if child.is_text_node() {
            let visible = document.paint_style(node).is_none_or(|style| {
                matches!(
                    style.clone_visibility(),
                    stylo::computed_values::visibility::T::Visible
                )
            });
            if !visible {
                continue;
            }
            if let (Some(layout), Some(text)) = (
                document.rounded_layout(child.id()),
                document.text_layout(child.id()),
            ) {
                clip.runs.push((
                    offset + Vec2::new(f64::from(layout.location.x), f64::from(layout.location.y)),
                    text,
                ));
            }
        } else if child.is_element() {
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

/// A clip node's compose chain: its scroll chain, and never an animation
/// chain — export eligibility refuses a clip established inside an animated
/// subtree, so a clip's rect never moves with a sampled delta.
pub(crate) fn clip_chain(clip: &ClipNode) -> ComposeChain {
    ComposeChain {
        scroll: clip.slot,
        animation: None,
    }
}

/// Encodes one clip node as a clip layer directly on `scene`, its shape
/// under `outer * scale * local` — the same operation [`push_clip`] emits,
/// for the composite's re-application of a slot's clip chain around a
/// plane draw. The caller pops it.
pub(crate) fn encode_clip(scene: &mut Scene, clip: &ClipNode, outer: Affine, scale: Affine) {
    let size = crate::Size2D::new(clip.rect.size.width, clip.rect.size.height);
    let Some(local) = convert::item_affine(&clip.transform, size) else {
        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::ZERO);
        return;
    };
    let rect = Rect::new(
        clip.rect.origin.x as f64,
        clip.rect.origin.y as f64,
        (clip.rect.origin.x + clip.rect.size.width) as f64,
        (clip.rect.origin.y + clip.rect.size.height) as f64,
    );
    let shape = BoxShape::new(rect, &clip.radii);
    with_shape!(&shape, |s| scene.push_clip_layer(
        Fill::NonZero,
        outer * scale * local,
        s
    ));
}

fn push_clip(sink: &mut WalkSink<'_>, clip: &ClipNode, scale: Affine) {
    let size = crate::Size2D::new(clip.rect.size.width, clip.rect.size.height);
    let Some(local) = convert::item_affine(&clip.transform, size) else {
        sink.push_clip_empty(clip_chain(clip));
        return;
    };
    let rect = Rect::new(
        clip.rect.origin.x as f64,
        clip.rect.origin.y as f64,
        (clip.rect.origin.x + clip.rect.size.width) as f64,
        (clip.rect.origin.y + clip.rect.size.height) as f64,
    );
    let shape = BoxShape::new(rect, &clip.radii);
    sink.push_clip_box(clip_chain(clip), Fill::NonZero, scale * local, shape);
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
/// The group bounds this produces are exactly what they were before the pass
/// gained a second job. Narrowing them by the cull decision would move every
/// `push_layer` rect and change the encoding of content nothing is culling.
fn plan_frame<T>(
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    cull: Option<Rect>,
) {
    let layers = frame.layers();
    let items = frame.items();
    scratch.layer_bounds.clear();
    scratch.open_layers.clear();
    scratch.item_plan.clear();
    scratch.item_plan.resize(items.len(), None);
    scratch.layer_bounds.resize(layers.len(), Rect::ZERO);
    // CSS px, not device px: `Device::viewport_size` is `Size2D<f32, CSSPixel>`, and the paint
    // order this is intersected against carries CSS-px transforms — the device scale is applied
    // once, separately, as the root `scale` affine.
    let viewport_size = document.device().viewport_size();
    let viewport = Rect::new(
        0.0,
        0.0,
        f64::from(viewport_size.width),
        f64::from(viewport_size.height),
    );
    scratch.bounds_acc.clear();
    scratch.bounds_acc.resize(layers.len(), None);
    let slots = frame.slots();
    let mut next_open = 0_usize;
    let close = |scratch: &mut Scratch| close_layer(scratch, layers, slots, viewport);

    for (index, item) in items.iter().enumerate() {
        while scratch
            .open_layers
            .last()
            .is_some_and(|&top| layers[top].items.end == index)
        {
            close(scratch);
        }
        while next_open < layers.len() && layers[next_open].items.start == index {
            scratch.bounds_acc[next_open] = layer_root_rect(&layers[next_open]);
            scratch.open_layers.push(next_open);
            next_open += 1;
        }
        // No `continue` on "outside every layer" here: those are precisely
        // the items no group has already clipped to the viewport, which are
        // the ones culling exists for.
        let Some(local) = convert::item_affine(&item.transform, item.size) else {
            // `paint_item` drew nothing for a singular transform either; the
            // decision has only moved.
            continue;
        };
        let top = scratch.open_layers.last().copied();
        let content_chain = frame.item_translation_chain(item);
        let moving = item
            .animation
            .is_some_and(|slot| scratch.animation_moves[slot as usize]);
        let admitted = if moving {
            // A sampled delta can move the item anywhere; it always paints.
            None
        } else {
            cull.map(|cull| admitted_region(scratch, frame, cull, content_chain, item.clip))
        };

        // An item whose plain border box already reaches the admitted region
        // paints whatever its fragments reach, because every reach only grows
        // that box. Establishing the reach reads the item's computed style, so
        // taking the answer without it is worth a comparison — and this is the
        // case on a page whose content is on screen, which is most of them.
        // Items inside a group are excluded: their reach is needed for the
        // group's bounds regardless of what the cull test decides.
        if top.is_none()
            && admitted.is_none_or(|admitted| can_reach(item_bounds(item, local, 0.0), admitted))
        {
            scratch.item_plan[index] = Some(local);
            continue;
        }

        let reach = extents(document, item);
        let bounds = item_bounds(item, local, reach.layer);
        if let Some(top) = top {
            let (low, high) = relative_offset_range(
                slots,
                &scratch.slot_windows,
                content_chain,
                layers[top].slot,
            );
            let bounds = expand_cover(bounds, low, high);
            scratch.bounds_acc[top] =
                Some(scratch.bounds_acc[top].map_or(bounds, |united| united.union(bounds)));
        }
        let reachable = admitted.is_none_or(|admitted| {
            if reach.cull.is_finite() {
                let inflated = if reach.cull > reach.layer {
                    item_bounds(item, local, reach.cull)
                } else {
                    bounds
                };
                can_reach(inflated, admitted)
            } else {
                // An unbounded reach can only be discarded by a clip chain
                // that admits nothing at all.
                admitted.is_some()
            }
        });
        if reachable {
            scratch.item_plan[index] = Some(local);
        }
    }
    while !scratch.open_layers.is_empty() {
        close(scratch);
    }
}

/// Closes the topmost open layer: its accumulated bounds become its pushed
/// rect — intersected with the viewport expanded into the layer's chain
/// coordinates, since the compose window may carry the layer's content
/// across it — and fold into the parent layer still open, expanded into
/// that parent's chain.
fn close_layer(
    scratch: &mut Scratch,
    layers: &[RenderLayer],
    slots: &[ScrollSlot],
    viewport: Rect,
) {
    let closed = scratch
        .open_layers
        .pop()
        .expect("close is only called with an open layer");
    let moving = layers[closed]
        .animation
        .is_some_and(|slot| scratch.animation_moves[slot as usize]);
    scratch.layer_bounds[closed] = scratch.bounds_acc[closed].map_or(Rect::ZERO, |rect| {
        if moving {
            // The group's rect and content translate together under the
            // sampled delta, but the *viewport* does not: clipping to it
            // would cut content the delta moves into view.
            return rect;
        }
        let (low, high) =
            relative_offset_range(slots, &scratch.slot_windows, layers[closed].slot, None);
        rect.intersect(expand_region(viewport, low, high))
    });
    if let (Some(bounds), Some(&parent)) = (scratch.bounds_acc[closed], scratch.open_layers.last())
    {
        let (low, high) = relative_offset_range(
            slots,
            &scratch.slot_windows,
            layers[closed].slot,
            layers[parent].slot,
        );
        let bounds = expand_cover(bounds, low, high);
        scratch.bounds_acc[parent] =
            Some(scratch.bounds_acc[parent].map_or(bounds, |united| united.union(bounds)));
    }
}

/// The region admitted for content on `chain`: the innermost enclosing
/// `clip`'s resolved bounds, or the base `region` when there is no clip,
/// expanded from its own chain into `chain`'s coordinates by however far the
/// encode windows between them can carry content. `None` means the clip
/// chain admits nothing at all.
fn admitted_region(
    scratch: &Scratch,
    frame: &PaintOrder,
    region: Rect,
    chain: Option<u32>,
    clip: Option<usize>,
) -> Option<Rect> {
    let (base, outer) = match clip {
        Some(clip) => (scratch.clip_bounds[clip]?, frame.clips()[clip].slot),
        None => (region, None),
    };
    let (low, high) = relative_offset_range(frame.slots(), &scratch.slot_windows, chain, outer);
    Some(expand_region(base, low, high))
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
    affine_rect(
        affine,
        Rect::new(
            -extent,
            -extent,
            item.size.width as f64 + extent,
            item.size.height as f64 + extent,
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
        PaintItem, PaintItemKind, Rect, Scene, Scratch, can_reach, cull_rect, item_bounds, walk,
        walk_uncultured,
    };
    use crate::Size2D;
    use crate::paint::equivalence::assert_scenes_identical;
    use crate::render::image::NoImages;
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
        let images = NoImages;
        let mut cultured = Scene::default();
        let mut cultured_scratch = Scratch::default();
        walk(
            &mut cultured,
            &mut cultured_scratch,
            &doc.dom,
            &frame,
            &images,
        );
        let mut uncultured = Scene::default();
        let mut uncultured_scratch = Scratch::default();
        walk_uncultured(
            &mut uncultured,
            &mut uncultured_scratch,
            &doc.dom,
            &frame,
            &images,
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
            animation: None,
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
}
