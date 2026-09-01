//! Visual order over the laid-out tree: CSS stacking contexts, Appendix-E
//! paint order, transform matrices, and hit testing.
//!
//! [`Document::render`] builds the private `PaintOrder`: a flat item list in
//! back-to-front order, each item carrying its viewport-space transform and
//! innermost clip. The private painter walks it forwards, back to front,
//! then retains it — together with the encoded scene and the scroll-slot
//! table — as one immutable [`CommittedFrame`] behind an `Arc`.
//! [`Document::elements_from_point`], [`Document::elements_from_points`], and
//! [`Document::route_input`] walk that retained frame backwards as pure reads
//! — hit testing never re-runs the pipeline. [`Document::commit`] publishes
//! the same `Arc` across the document API boundary: a thread that does not
//! hold the document composites, hit-tests, and recognizes scroll gestures
//! against the published frame alone.
//! The hit queries answer from the retained frame however stale it is,
//! skipping only the items whose node has since been freed. That is safe
//! because a [`crate::NodeId`] is retired on free and never reissued: an id
//! the frame still names resolves either to the node it was drawn for or to
//! nothing, never to a different node that took its place.
//! Painting is pinned harder — to the current commit id with a clean dirty
//! bit via its freshness assertion, let-it-crash — because it resolves the
//! frame against live styles/layouts/text, which any visual mutation
//! desynchronizes; hit testing's snapshot is self-contained and tolerates
//! non-structural mutations, which is exactly what lets an event landing
//! between a scroll and its repaint hit the content the user was shown.
//!
//! Scroll offsets ([`crate::scroll`]) are baked into the frame rather than
//! surfaced beside it: a scroll container's contents are translated as they
//! are collected, so painting and hit testing both see scrolled geometry with
//! no separate scroll state, and the private painter needs none either.
//! The price is that scrolling invalidates the frame like any other visual
//! mutation — which it must anyway, since scrolled content has to repaint.
//!
//! Invariants this module relies on (verified against the layout host):
//! - `Layout.location` is border-box-relative to the **box parent**'s border box for every box —
//!   the container whose formatting context laid the box out, which is the DOM parent except across
//!   dissolved `display: contents` levels — including hoisted absolute/fixed boxes
//!   (`position_hoisted` rewrites their location back into parent-relative terms). The build walks
//!   the same flattened box-tree, so plain offset accumulation along it is sound.
//! - Rounded layouts stay in CSS px with parent-relative locations that telescope exactly to
//!   snapped absolute positions at any device scale.
//! - Subtrees the layout host zeroes (display:none, unstyled descendants, `DisplayMode::Leaf`
//!   children, `content-visibility: hidden` contents) are exactly the subtrees this module skips.
//! - `display: contents` (css-display-3 §2.5) dissolves in lockstep with layout: this module walks
//!   the engine's own `LayoutTree::flattened_children`, so dissolved grandchildren paint, stack,
//!   and hit as members of the box parent's context; the boxless element itself paints nothing,
//!   triggers nothing (no stacking context, clip, or transform), and relays only inherited values
//!   (visibility, pointer-events) into its children. A text run whose DOM parent is boxless still
//!   hit-targets that parent — the singular text-hit rule, matching Chrome's `elementFromPoint`.
//!
//! Deliberate v1 limits (see docs/tracking/css-layout.md for status):
//! - Group effects (`opacity`, `filter`, `clip-path`, `mask`) surface as private `RenderLayer`
//!   boundaries for the private painter to composite; they still do not affect hit testing (a
//!   `clip-path` that clips painting away does not clip the hit region yet). `backdrop-filter` is
//!   not compiled in the fork at all, so its stacking-context trigger is structurally deferred.
//! - Motion paths (motion-1) are composed into the matrix between the individual transforms and the
//!   transform list: `path()`, `circle()`, `ellipse()`, and `inset()` — the shapes the fork parses,
//!   with the coord box fixed to the border box. The anchor is always `transform-origin`
//!   (`offset-anchor`/`offset-position` are not compiled), `ray()`/`url()` are gated out, and the
//!   fork's `offset-rotate` grammar is `auto | <angle 0..=360>` (no `reverse`, no `auto <angle>` —
//!   Lynx's own surface; the computed-value handling folds the general form for rebase safety).
//! - The css-display-3 replaced-element "unbox" rule is not implemented: `display: contents` on a
//!   natural-size (replaced) element with DOM children renders those children as box-parent items
//!   instead of suppressing them; childless replaced elements already match the spec outcome.
//! - Hit regions are half-open at a box's trailing (right/bottom) edges, matching browser event
//!   targeting; clip containment stays inclusive.
//! - Clipping is resolved per axis, because `overflow: clip` on one axis with `visible` on the
//!   other is a pair the style adjuster leaves alone (it only reconciles axes that disagree about
//!   being *scrollable*, and neither of those is). A one-axis clip is an infinite strip, so it
//!   carries no corner radii; every other combination clips the padding box as before.
//! - `position: sticky` establishes a stacking context and paints as normal flow, but does not
//!   stick: no offset is clamped against the scrollport (css-position-3 §6.3), so a sticky box
//!   scrolls away with its container. Recorded in `crate::scroll`'s limits and
//!   `docs/tracking/deviations.md`.
//! - `transform-style: preserve-3d`, `backface-visibility`, and `perspective-origin` are not
//!   authorable (the latter two are not even compiled) — everything flattens and perspective
//!   projects about the border-box center.
//! - No incremental visual-order structure. The last `PaintOrder` is retained beside the scene, but
//!   only as the hit-test snapshot: it is never an input to the next build, and every visual
//!   mutation rebuilds the whole order. Invalidation is one private dirty bit
//!   ([`Document::needs_render`]) — every kind of change sets the same bit, so any two changes are
//!   indistinguishable to the render path; a frame's identity is its commit id, which orders
//!   commits and carries no damage information. `StyleDamage`'s repaint and stacking classes are
//!   computed by the style flush and dropped; they are what a tiered scheme would key on, but
//!   nothing on this path reads them today.

mod build;
pub(crate) mod curves;
pub(crate) mod frame;
mod geometry;
mod hit;
mod motion;
mod stacking;
#[cfg(test)]
mod tests;
mod transform;

use std::sync::Arc;

use euclid::default::{Point2D, Rect, Size2D, Transform3D};

pub(crate) use self::build::BuildScratch;
pub use self::frame::{AnimationSlot, CommittedFrame, HitTarget, ScrollSlot};
use crate::render::image::ImageEvent;
use crate::tree::document::Document;
use crate::{FrameImages, NodeId};

/// A frame's back-to-front paint order.
#[derive(Debug)]
pub(crate) struct PaintOrder {
    items: Vec<PaintItem>,
    clips: Vec<ClipNode>,
    layers: Vec<RenderLayer>,
    slots: Vec<ScrollSlot>,
    animations: Vec<AnimationSlot>,
    commit_id: u64,
}

/// One animation slot's compose-time values, sampled at one instant: the
/// CSS-px delta from the committed geometry, and the opacity replacing the
/// committed one on the element's effect layer. `parent` mirrors the slot
/// table so a chain walk needs only this table.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AnimationSample {
    pub(crate) parent: Option<u32>,
    pub(crate) delta: crate::vello::kurbo::Affine,
    pub(crate) alpha: Option<f32>,
}

/// The four growable tables a [`PaintOrder`] is made of, emptied of one
/// frame's contents but not of their capacity.
///
/// The builder fills a set and the painter reclaims the set from the frame it
/// retires, so a document whose page shape has stopped growing allocates
/// paint-order storage zero times per frame. The set the painter reclaims is
/// never the frame it retains: hit testing reads that one between renders, so
/// it must never be the object being emptied.
#[derive(Debug, Default)]
pub(crate) struct FrameBuffers {
    items: Vec<PaintItem>,
    clips: Vec<ClipNode>,
    layers: Vec<RenderLayer>,
    slots: Vec<ScrollSlot>,
    animations: Vec<AnimationSlot>,
}

impl FrameBuffers {
    #[cfg(test)]
    pub(crate) fn capacities(&self) -> [usize; 5] {
        [
            self.items.capacity(),
            self.clips.capacity(),
            self.layers.capacity(),
            self.slots.capacity(),
            self.animations.capacity(),
        ]
    }
}

impl PaintOrder {
    /// Empties this frame and hands back its storage with capacity intact.
    ///
    /// [`PaintItem`], [`ClipNode`], [`RenderLayer`] and [`ScrollSlot`] own no
    /// heap data, so each clear is a length write.
    pub(crate) fn into_buffers(mut self) -> FrameBuffers {
        self.items.clear();
        self.clips.clear();
        self.layers.clear();
        self.slots.clear();
        self.animations.clear();
        FrameBuffers {
            items: self.items,
            clips: self.clips,
            layers: self.layers,
            slots: self.slots,
            animations: self.animations,
        }
    }

    #[cfg(test)]
    fn capacities(&self) -> [usize; 5] {
        [
            self.items.capacity(),
            self.clips.capacity(),
            self.layers.capacity(),
            self.slots.capacity(),
            self.animations.capacity(),
        ]
    }

    #[must_use]
    pub(crate) fn items(&self) -> &[PaintItem] {
        &self.items
    }

    #[must_use]
    pub(crate) fn clips(&self) -> &[ClipNode] {
        &self.clips
    }

    #[must_use]
    pub(crate) fn layers(&self) -> &[RenderLayer] {
        &self.layers
    }

    #[must_use]
    pub(crate) fn slots(&self) -> &[ScrollSlot] {
        &self.slots
    }

    #[must_use]
    pub(crate) fn animations(&self) -> &[AnimationSlot] {
        &self.animations
    }

    /// Every animation slot's compose values sampled at `now` — the
    /// committed values (identity delta, committed opacity) for a slot with
    /// no exported curve, or when `now` is `None`.
    pub(crate) fn sample_animations(&self, now: Option<f64>) -> Vec<AnimationSample> {
        self.animations
            .iter()
            .map(|slot| slot.sample(now))
            .collect()
    }

    #[must_use]
    pub(crate) const fn commit_id(&self) -> u64 {
        self.commit_id
    }

    /// The chain whose scroll translations move this item — the *content*
    /// chain, as opposed to [`PaintItem::slot`]'s recognition chain. They
    /// differ in exactly one case: a scroll container's own box carries its
    /// own slot for recognition (the box is a scroll target) but is moved
    /// only by the scrollers around it.
    /// The full compose chain moving this item's content: the scroll
    /// translation chain plus the animation chain — an element's own box
    /// moves with its own animation delta, so the animation side has no
    /// recognition split.
    #[must_use]
    pub(crate) fn item_compose_chain(
        &self,
        item: &PaintItem,
    ) -> crate::paint::compose::ComposeChain {
        crate::paint::compose::ComposeChain {
            scroll: self.item_translation_chain(item),
            animation: item.animation,
        }
    }

    #[must_use]
    pub(crate) fn item_translation_chain(&self, item: &PaintItem) -> Option<u32> {
        let slot = item.slot?;
        let entry = self.slots[slot as usize];
        if matches!(item.kind, PaintItemKind::ElementBox) && entry.node == item.node {
            entry.parent
        } else {
            Some(slot)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaintItemKind {
    ElementBox,
    TextRun { element: NodeId },
}

/// One node's entry in the paint order.
#[derive(Debug, Clone)]
pub(crate) struct PaintItem {
    pub(crate) node: NodeId,
    pub(crate) kind: PaintItemKind,
    pub(crate) transform: Transform3D<f32>,
    pub(crate) clip: Option<usize>,
    pub(crate) size: Size2D<f32>,
    pub(crate) radii: CornerRadii,
    pub(crate) hit_testable: bool,
    /// The nearest ancestor-or-self scroll container on this item's
    /// containing-block chain, as an index into [`PaintOrder::slots`]. This is
    /// the *recognition* chain: a scroll container's own item names its own
    /// slot (the box is a scroll target), while the containers that *carry*
    /// the item — whose content translation is folded into
    /// [`Self::transform`] — are this slot's parent chain for a container's
    /// own item and this very chain for everything else.
    pub(crate) slot: Option<u32>,
    /// The nearest ancestor-or-self animation slot moving this item, as an
    /// index into [`PaintOrder::animations`]. Unlike the scroll chain, an
    /// element's own box rides its own slot: the animated transform moves
    /// the element itself.
    pub(crate) animation: Option<u32>,
}

/// A stacking context rendered as a composited group.
///
/// Only contexts with group effects (`opacity`, `filter`, `mix-blend-mode`,
/// `clip-path`, `mask-image`, `isolation`) get one. A plain transform or
/// `z-index` context has no [`RenderLayer`] at all, so this table is not an
/// index of stacking contexts and cannot be used as one.
#[derive(Debug, Clone)]
pub(crate) struct RenderLayer {
    pub(crate) parent: Option<usize>,
    pub(crate) node: NodeId,
    pub(crate) transform: Transform3D<f32>,
    pub(crate) size: Size2D<f32>,
    pub(crate) radii: CornerRadii,
    /// The scroll chain this group's own frame rides — the chain of its root
    /// box, so the group and its clip move with the scrollers *around* the
    /// root, never with the root's own content.
    pub(crate) slot: Option<u32>,
    /// The nearest ancestor-or-self animation slot moving this group's own
    /// frame — ancestor-or-self, because an animated element's group moves
    /// with the element.
    pub(crate) animation: Option<u32>,
    /// The contiguous run of [`PaintOrder::items`] this group encloses. A
    /// stacking context paints atomically, so its members are always
    /// contiguous; an empty run is not recorded at all (the layer is popped).
    pub(crate) items: std::ops::Range<usize>,
}

/// One overflow/`contain: paint` clip: a rounded padding-box rect in the
/// establishing element's local space.
#[derive(Debug, Clone)]
pub(crate) struct ClipNode {
    pub(crate) parent: Option<usize>,
    #[cfg(test)]
    pub(crate) node: NodeId,
    pub(crate) transform: Transform3D<f32>,
    pub(crate) rect: Rect<f32>,
    pub(crate) radii: CornerRadii,
    /// The scroll chain the clip's own rect rides: the chain *outside* the
    /// establishing element, captured before that element's own slot enters
    /// the flow — a scroller's clip does not move with its own content.
    pub(crate) slot: Option<u32>,
}

/// Per-corner elliptical radii, in CSS px: `width` is the horizontal radius,
/// `height` the vertical.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct CornerRadii {
    pub(crate) top_left: Size2D<f32>,
    pub(crate) top_right: Size2D<f32>,
    pub(crate) bottom_right: Size2D<f32>,
    pub(crate) bottom_left: Size2D<f32>,
}

impl CornerRadii {
    pub(crate) const ZERO: Self = Self {
        top_left: Size2D::new(0.0, 0.0),
        top_right: Size2D::new(0.0, 0.0),
        bottom_right: Size2D::new(0.0, 0.0),
        bottom_left: Size2D::new(0.0, 0.0),
    };

    #[must_use]
    pub(crate) fn is_zero(&self) -> bool {
        *self == Self::ZERO
    }
}

impl<T: Sync> Document<T> {
    pub(crate) fn build_paint_order(&mut self) -> PaintOrder {
        self.layout();
        // Claimed before the build so the built frame carries it; a
        // panicking build or walk leaves the previous frame retained with
        // its older id, so `needs_render` still reports stale.
        let _ = self.next_commit_id();
        // Both takes finish before `build` reborrows the document shared. The
        // retained frame is not among them: it stays where hit testing can
        // read it for the whole build, and a build that panics loses only one
        // frame's worth of capacity.
        let (scratch, buffers) = {
            let painter = self.painter.get_mut();
            (painter.take_build_scratch(), painter.take_spare_buffers())
        };
        let (frame, scratch) = build::build(self, scratch, buffers);
        self.painter.get_mut().restore_build_scratch(scratch);
        frame
    }

    /// Renders only when the retained frame no longer represents the current
    /// document state. Returns whether a new frame was built.
    pub fn render(&mut self) -> bool {
        if !self.needs_render() {
            return false;
        }
        // A caller may render without ever laying out; the paint walk still
        // reads layout slots wholesale, so size them first.
        let bound = self.arenas().slot_bound();
        self.layout_state_mut().ensure_covers(bound);
        let frame = self.build_paint_order();
        let animations_active = self.animations().is_active();
        let needs_main_ticks = animations_active && self.animation_needs_main_ticks(&frame);
        let viewport = self.viewport_size();
        let device_pixel_ratio = self.device_pixel_ratio();
        self.painter.borrow_mut().paint(
            self,
            frame,
            animations_active,
            needs_main_ticks,
            viewport,
            device_pixel_ratio,
        );
        true
    }

    /// Runs the whole pipeline — style, layout, paint-order build, scene
    /// encode — if anything is stale, and returns the committed frame.
    ///
    /// The returned `Arc` is the same object the document retains for its own
    /// hit queries: publishing it to another thread costs one reference-count
    /// increment, and the frame stays valid however stale it later gets.
    pub fn commit(&mut self) -> Arc<CommittedFrame> {
        self.render();
        self.committed_frame()
            .expect("render always leaves a committed frame retained")
    }
}

/// A borrow-shaped handle on the committed frame's scene: it keeps the frame
/// alive and dereferences to the [`crate::vello::Scene`] inside it. A
/// layered frame retains no whole-frame composition, so the handle carries
/// one composed for it on demand.
pub struct SceneRef {
    frame: Arc<CommittedFrame>,
    composed: Option<Box<crate::vello::Scene>>,
}

impl std::fmt::Debug for SceneRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SceneRef")
            .field("frame", &self.frame)
            .finish_non_exhaustive()
    }
}

impl std::ops::Deref for SceneRef {
    type Target = crate::vello::Scene;

    fn deref(&self) -> &crate::vello::Scene {
        self.composed.as_deref().map_or_else(
            || {
                self.frame
                    .scene()
                    .expect("a layered frame's handle carries its composition")
            },
            |scene| scene,
        )
    }
}

impl<T> Document<T> {
    /// The frame the last [`Self::render`] committed, if any.
    #[must_use]
    pub fn committed_frame(&self) -> Option<Arc<CommittedFrame>> {
        self.painter.borrow().frame().cloned()
    }

    /// The Vello scene committed by the last successful [`Self::render`].
    ///
    /// Valid only within this document's own viewport — [`Self::viewport_size`]
    /// scaled by [`Self::device_pixel_ratio`], which is the region an embedder
    /// sizes its target from. The painter discards content that can put no ink
    /// there, so rendering this scene into a target covering more CSS pixels
    /// than the document's viewport leaves the excess blank rather than
    /// showing what happens to be laid out beyond it.
    ///
    /// # Panics
    ///
    /// If nothing has been rendered yet: there is no committed frame to read.
    #[must_use]
    /// A scene cannot be composed without a pixel source: after the image
    /// system moved to the painter, the frame names images rather than
    /// carrying them, so the caller supplies whatever resolves those names.
    /// Pass [`NoImages`](crate::NoImages) for a page with no images.
    pub fn scene(&self, pixels: &dyn FrameImages) -> SceneRef {
        let frame = self
            .committed_frame()
            .expect("Document::scene reads the committed frame: render first");
        let composed = frame.scene().is_none().then(|| {
            let (mut images, mut sources) = (Vec::new(), Vec::new());
            frame.resolve_images(pixels, &mut images, &mut sources);
            let mut scene = crate::vello::Scene::new();
            frame.compose_into(&mut scene, &images, &|_| None, None);
            Box::new(scene)
        });
        SceneRef { frame, composed }
    }

    /// Returns rendered elements under a point from front to back.
    ///
    /// The frame is baked unscrolled; document-side queries apply the
    /// document's *live* scroll offsets, so a scroll is observable here the
    /// moment it lands, without a rebuild.
    #[must_use]
    pub fn elements_from_point(&self, point: Point2D<f32>) -> Vec<NodeId> {
        let offsets = |slot: &ScrollSlot| Some(self.scroll_offset(slot.node));
        let ratio = self.device_pixel_ratio();
        self.with_rendered_frame(|frame| frame.elements_at(self, point, &offsets, ratio))
            .unwrap_or_default()
    }

    /// [`Self::elements_from_point`] for a batch of points answered from one
    /// frame read; results are index-parallel with `points`. Same pure-read
    /// contract: before the first render every answer is empty.
    #[must_use]
    pub fn elements_from_points(&self, points: &[Point2D<f32>]) -> Vec<Vec<NodeId>> {
        let offsets = |slot: &ScrollSlot| Some(self.scroll_offset(slot.node));
        let ratio = self.device_pixel_ratio();
        self.with_rendered_frame(|frame| {
            points
                .iter()
                .map(|point| frame.elements_at(self, *point, &offsets, ratio))
                .collect()
        })
        .unwrap_or_else(|| vec![Vec::new(); points.len()])
    }

    pub(crate) fn rendered_element_at(&self, point: Point2D<f32>) -> Option<NodeId> {
        let offsets = |slot: &ScrollSlot| Some(self.scroll_offset(slot.node));
        let ratio = self.device_pixel_ratio();
        self.with_rendered_frame(|frame| frame.first_element_at(self, point, &offsets, ratio))
            .flatten()
    }

    fn with_rendered_frame<R>(&self, read: impl FnOnce(&PaintOrder) -> R) -> Option<R> {
        let painter = self.painter.borrow();
        let frame = painter.frame()?;
        Some(read(&frame.order))
    }
}

impl<T> Document<T> {
    /// Whether a visual mutation has made the retained frame stale, or no
    /// frame has been built yet.
    #[must_use]
    pub fn needs_render(&self) -> bool {
        self.visual_dirty()
            || self.painter.borrow().frame().map(|frame| frame.commit_id())
                != Some(self.commit_id())
    }

    /// The capacity of every buffer the paint pipeline retains between
    /// frames: the retained frame's four tables, the spare frame's four,
    /// then the working scratch, ending with one entry per pooled
    /// pseudo-context buffer.
    ///
    /// Exists for the reuse tests: a settled page must reproduce this exactly
    /// from one frame to the next, because an entry that grew is a buffer that
    /// reallocated. The pool is reported per entry rather than as a total, so
    /// that a reallocation moving capacity from one pooled buffer to another
    /// is visible instead of cancelling out.
    #[cfg(test)]
    pub(crate) fn paint_storage_capacities(&self) -> Vec<usize> {
        let painter = self.painter.borrow();
        let mut out = painter
            .frame()
            .map_or([0, 0, 0, 0, 0], |frame| frame.order.capacities())
            .to_vec();
        let (spare, scratch) = painter.storage_capacities();
        out.extend_from_slice(&spare);
        out.extend(scratch);
        out
    }

    /// The document's image name table.
    pub(crate) fn images(&self) -> &crate::render::image::ImageRegistry {
        &self.images
    }

    /// The sources the last paint walk met and has not yet asked for, drained
    /// for the painter to request from its store.
    ///
    /// This is how a `url()` in a stylesheet and an element's own source both
    /// become load requests: the walk is the one place that knows which
    /// sources a frame actually needs.
    pub fn take_wanted_images(&mut self) -> Vec<Arc<str>> {
        self.images.take_wanted()
    }

    /// Applies the host's image reports: records completed loads with their
    /// intrinsic dimensions, and marks failures.
    ///
    /// A load that lands on replaced nodes sets their natural size in the
    /// same call, so the element resizes in the commit that first draws it.
    pub fn apply_image_events(&mut self, events: &[ImageEvent]) {
        let mut changed = false;
        for event in events {
            // `None` is a source reported twice, which one URL with one
            // content makes a no-op: nothing moved, so nothing is dirtied.
            let Some(nodes) = self.images.apply(event) else {
                continue;
            };
            changed = true;
            if let ImageEvent::Loaded { width, height, .. } = event {
                let natural = crate::layout::natural_size(*width, *height);
                for node in nodes {
                    self.set_natural_size(node, natural);
                }
            }
        }
        if changed {
            self.note_visual_mutation();
        }
    }

    /// Invalidates the retained frame because composition has moved a scroll
    /// slot far through its committed encode window: the next commit must
    /// repaint so the windows re-center on the current offsets.
    ///
    /// The offsets themselves are already this document's — scroll writes
    /// land here first — so only the paint is stale, never the geometry.
    pub fn note_scroll_windows_stale(&mut self) {
        self.note_visual_mutation();
    }
}
