//! Visual order over the laid-out tree: CSS stacking contexts, Appendix-E
//! paint order, transform matrices, and hit testing.
//!
//! [`Document::render`] builds the private `PaintOrder`: a flat item list in
//! back-to-front order, each item carrying its viewport-space transform and
//! innermost clip. The private painter walks it forwards, back to front,
//! then retains it beside the scene; [`Document::elements_from_point`],
//! [`Document::elements_from_points`], and [`Document::route_input`] walk
//! that retained frame backwards as pure reads — hit testing never re-runs
//! the pipeline. Neither the frame nor the painter crosses the document API
//! boundary.
//! The hit queries answer from the retained frame however stale it is,
//! skipping only the items whose node has since been freed. That is safe
//! because a [`crate::NodeId`] is retired on free and never reissued: an id
//! the frame still names resolves either to the node it was drawn for or to
//! nothing, never to a different node that took its place.
//! Painting is pinned harder — to the document's private visual-mutation
//! epoch via its freshness assertion, let-it-crash — because it resolves the
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
//!   mutation rebuilds the whole order. Invalidation is the document's private visual epoch
//!   ([`Document::needs_render`]) — one counter for every kind of change, so a one-pixel scroll and
//!   a structural edit are indistinguishable to the render path. `StyleDamage`'s repaint and
//!   stacking classes are computed by the style flush and dropped; they are what a tiered scheme
//!   would key on, but nothing on this path reads them today.

mod build;
mod geometry;
mod hit;
mod motion;
mod stacking;
#[cfg(test)]
mod tests;
mod transform;

use std::cell::Ref;

use euclid::default::{Point2D, Rect, Size2D, Transform3D};

pub(crate) use self::build::BuildScratch;
use crate::tree::document::Document;
use crate::{ImageStore, NodeId};

/// A frame's back-to-front paint order.
#[derive(Debug)]
pub(crate) struct PaintOrder {
    items: Vec<PaintItem>,
    clips: Vec<ClipNode>,
    layers: Vec<RenderLayer>,
    visual_epoch: u64,
}

/// The three growable tables a [`PaintOrder`] is made of, emptied of one
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
}

impl FrameBuffers {
    #[cfg(test)]
    pub(crate) fn capacities(&self) -> [usize; 3] {
        [
            self.items.capacity(),
            self.clips.capacity(),
            self.layers.capacity(),
        ]
    }
}

impl PaintOrder {
    /// Empties this frame and hands back its storage with capacity intact.
    ///
    /// [`PaintItem`], [`ClipNode`] and [`RenderLayer`] own no heap data, so
    /// each clear is a length write.
    pub(crate) fn into_buffers(mut self) -> FrameBuffers {
        self.items.clear();
        self.clips.clear();
        self.layers.clear();
        FrameBuffers {
            items: self.items,
            clips: self.clips,
            layers: self.layers,
        }
    }

    #[cfg(test)]
    fn capacities(&self) -> [usize; 3] {
        [
            self.items.capacity(),
            self.clips.capacity(),
            self.layers.capacity(),
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
    pub(crate) const fn visual_epoch(&self) -> u64 {
        self.visual_epoch
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

    /// Renders only when the retained scene no longer represents the current
    /// document state. Returns whether a new scene was built.
    pub fn render(&mut self) -> bool {
        if !self.needs_render() {
            return false;
        }
        // A caller may render without ever laying out; the paint walk still
        // reads layout slots wholesale, so size them first.
        let bound = self.arenas().slot_bound();
        self.layout_state_mut().ensure_covers(bound);
        let frame = self.build_paint_order();
        self.painter.borrow_mut().paint(self, frame);
        true
    }
}

impl<T> Document<T> {
    /// Returns rendered elements under a point from front to back.
    #[must_use]
    pub fn elements_from_point(&self, point: Point2D<f32>) -> Vec<NodeId> {
        self.with_rendered_frame(|frame| frame.elements_at(self, point))
            .unwrap_or_default()
    }

    /// [`Self::elements_from_point`] for a batch of points answered from one
    /// frame read; results are index-parallel with `points`. Same pure-read
    /// contract: before the first render every answer is empty.
    #[must_use]
    pub fn elements_from_points(&self, points: &[Point2D<f32>]) -> Vec<Vec<NodeId>> {
        self.with_rendered_frame(|frame| {
            points
                .iter()
                .map(|point| frame.elements_at(self, *point))
                .collect()
        })
        .unwrap_or_else(|| vec![Vec::new(); points.len()])
    }

    pub(crate) fn rendered_element_at(&self, point: Point2D<f32>) -> Option<NodeId> {
        self.with_rendered_frame(|frame| frame.first_element_at(self, point))
            .flatten()
    }

    fn with_rendered_frame<R>(&self, read: impl FnOnce(&PaintOrder) -> R) -> Option<R> {
        let painter = self.painter.borrow();
        let frame = painter.frame()?;
        Some(read(frame))
    }
}

impl<T> Document<T> {
    /// Whether a visual mutation has made the retained scene stale, or no
    /// scene has been built yet.
    #[must_use]
    pub fn needs_render(&self) -> bool {
        self.painter.borrow().needs_render(self.visual_epoch())
    }

    /// The Vello scene retained by the last successful
    /// [`Self::render`] call.
    ///
    /// Valid only within this document's own viewport — [`Self::viewport_size`]
    /// scaled by [`Self::device_pixel_ratio`], which is the region an embedder
    /// sizes its target from. The painter discards content that can put no ink
    /// there, so rendering this scene into a target covering more CSS pixels
    /// than the document's viewport leaves the excess blank rather than
    /// showing what happens to be laid out beyond it. Resize the document
    /// first: changing the viewport invalidates the retained scene, so the
    /// next render builds one for the larger region.
    #[must_use]
    pub fn scene(&self) -> Ref<'_, crate::vello::Scene> {
        Ref::map(self.painter.borrow(), crate::paint::painter::Painter::scene)
    }

    /// The capacity of every buffer the paint pipeline retains between
    /// frames: the retained frame's three tables, the spare frame's three,
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
            .map_or([0, 0, 0], PaintOrder::capacities)
            .to_vec();
        let (spare, scratch) = painter.storage_capacities();
        out.extend_from_slice(&spare);
        out.extend(scratch);
        out
    }

    /// Reads decoded images without invalidating the retained scene.
    ///
    /// Separate from [`Self::images_mut`] because that one has to invalidate:
    /// registering pixels changes what a frame draws. Reading how many bytes
    /// the registry holds, or which registrations were refused, changes
    /// nothing, and going through the mutable accessor to ask would turn a
    /// static page into one that rebuilds its scene on every poll.
    #[must_use]
    pub fn images(&self) -> Ref<'_, ImageStore> {
        Ref::map(
            self.painter.borrow(),
            crate::paint::painter::Painter::images,
        )
    }

    /// Mutably accesses decoded images and invalidates the retained scene.
    pub fn images_mut(&mut self) -> &mut ImageStore {
        self.note_visual_mutation();
        self.painter.get_mut().images_mut()
    }
}
