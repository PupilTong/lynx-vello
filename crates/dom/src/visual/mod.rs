//! Visual order over the laid-out tree: CSS stacking contexts, Appendix-E
//! paint order, transform matrices, and hit testing.
//!
//! [`Document::render`], [`Document::hit_test`], and
//! [`Document::handle_input`] internally build a
//! private `PaintOrder`: a flat item list in back-to-front order, each item carrying
//! its viewport-space transform and innermost clip. The private painter walks
//! it frontwards; hit testing walks it backwards. Neither the frame nor the
//! painter crosses the document API boundary.
//! A frame is pinned to the document's node-removal epoch: after any
//! `remove_subtree` a freed id can be recycled by a later creation, so
//! querying a stale frame panics (let-it-crash) rather than returning a
//! recycled node for old geometry. Painting is pinned harder — to the
//! document's private visual-mutation epoch via its freshness assertion —
//! because it resolves the frame against live styles/layouts/text, which
//! any visual mutation desynchronizes; hit testing's snapshot is
//! self-contained and tolerates non-structural mutations.
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
//! - No retained/incremental structure: `StyleDamage::needs_stacking_context_rebuild` is the
//!   designated invalidation hook for a future retained mode, but no cache exists today.

mod build;
mod geometry;
mod hit;
mod motion;
mod stacking;
#[cfg(test)]
mod tests;
mod transform;

// The crate's whole geometry vocabulary, re-exported so an embedder can name
// every type that appears in a public signature here — `Vector2D` included:
// scroll offsets and deltas are displacements, not positions, and they surface
// on `Document::scroll_by`, `ScrollBox`, and `DefaultAction::Scroll`.
use std::cell::Ref;

pub use euclid::default::{Point2D, Rect, Size2D, Transform3D, Vector2D};

use crate::document::Document;
use crate::{ImageStore, NodeId};

/// The document's current frame in paint order: `items[0]` paints first
/// (bottom), `items[len - 1]` paints last (top).
///
/// A frame is a snapshot. It stays queryable while the document's node set
/// is unchanged; once nodes are removed (`Document::remove_subtree`), freed
/// ids can be recycled and [`Self::hit_test`] fails fast instead of
/// answering with a recycled node — rebuild the frame after structural
/// mutations.
#[derive(Debug)]
pub(crate) struct PaintOrder {
    pub(crate) items: Vec<PaintItem>,
    pub(crate) clips: Vec<ClipNode>,
    pub(crate) layers: Vec<RenderLayer>,
    /// [`Document::node_removal_epoch`] at build time.
    pub(crate) epoch: u64,
    /// The document's private visual-mutation epoch at build time.
    pub(crate) visual_epoch: u64,
}

impl PaintOrder {
    /// Back-to-front paint items.
    #[must_use]
    pub(crate) fn items(&self) -> &[PaintItem] {
        &self.items
    }

    /// Clip arena referenced by [`PaintItem::clip`] and [`ClipNode::parent`].
    #[must_use]
    pub(crate) fn clips(&self) -> &[ClipNode] {
        &self.clips
    }

    /// Render layers, in preorder (a layer precedes every layer nested in
    /// it, and its [`RenderLayer::items`] range contains theirs).
    #[must_use]
    pub(crate) fn layers(&self) -> &[RenderLayer] {
        &self.layers
    }
}

/// Where a hit landed inside the item it hit, from
/// [`PaintOrder::hit_test_local`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LocalHit {
    /// Index into [`PaintOrder::items`].
    pub(crate) item: usize,
    /// The point in that item's border-box space.
    pub(crate) position: Point2D<f32>,
}

/// What one paint item draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaintItemKind {
    /// An element's own box: background, borders, decorations.
    ElementBox,
    /// A text leaf's glyph runs; `element` is the styled parent element hit
    /// testing resolves to.
    TextRun { element: NodeId },
}

/// One node's slot in the paint order.
#[derive(Debug, Clone)]
pub(crate) struct PaintItem {
    pub(crate) node: NodeId,
    pub(crate) kind: PaintItemKind,
    /// Item-local border-box coordinates → viewport CSS px. Flattened
    /// 2D-projective (z decoupled); a singular matrix means the element is
    /// not rendered (css-transforms-1) and hit testing skips it.
    pub(crate) transform: Transform3D<f32>,
    /// Innermost applicable clip, an index into [`PaintOrder::clips`].
    pub(crate) clip: Option<usize>,
    /// Rounded border-box size.
    pub(crate) size: Size2D<f32>,
    /// Resolved, overlap-normalized border radii (zero for text runs).
    pub(crate) radii: CornerRadii,
    /// `visibility: visible` and `pointer-events` other than `none`.
    pub(crate) hit_testable: bool,
}

/// A stacking context whose subtree composites as a group — `opacity` below
/// one, `filter`, `clip-path`, `mask`, and the storage-only blend/isolation
/// triggers once a grammar rebase makes them authorable. The private painter must
/// paint the enclosed items into an offscreen group and apply the effects on
/// composite; contexts without group effects (plain `z-index`, transforms)
/// deliberately get no layer.
///
/// Effect *parameters* are not duplicated here: the establishing element's
/// computed style (via [`Document::paint_style`]) is the single source the
/// painter resolves `opacity`/`filter`/`clip-path`/`mask` values from, with
/// `clip-path` and `mask` geometry resolved against [`Self::size`].
///
/// Group effects still do not affect [`PaintOrder::hit_test`] (a `clip-path`
/// that clips painting away does not yet clip the hit region — recorded v1
/// limit).
#[derive(Debug, Clone)]
pub(crate) struct RenderLayer {
    /// Next-outer render layer, an index into [`PaintOrder::layers`].
    pub(crate) parent: Option<usize>,
    /// The establishing element.
    pub(crate) node: NodeId,
    /// Layer-local (border-box) coordinates → viewport CSS px. Present even
    /// when the establishing box itself paints no item (`visibility:
    /// hidden` root with visible descendants).
    pub(crate) transform: Transform3D<f32>,
    /// Rounded border-box size of the establishing element.
    pub(crate) size: Size2D<f32>,
    /// The establishing element's resolved, overlap-normalized border
    /// radii — carried here (not scavenged from the root's item) so
    /// `clip-path`/`mask` geometry keeps its rounding even when the root
    /// box paints no item (`visibility: hidden`).
    pub(crate) radii: CornerRadii,
    /// Half-open range of [`PaintOrder::items`] the group encloses.
    /// Stacking contexts paint atomically, so the range is contiguous and
    /// nested layers' ranges nest; empty groups are dropped at build time,
    /// so the range is never empty.
    pub(crate) items: std::ops::Range<usize>,
}

/// One overflow/`contain: paint` clip: a rounded padding-box rect in the
/// establishing element's local space.
#[derive(Debug, Clone)]
pub(crate) struct ClipNode {
    /// Next-outer clip in this node's containing-block chain.
    pub(crate) parent: Option<usize>,
    /// The establishing element, retained only for invariant tests; painting
    /// and hit testing consume the clip's resolved geometry directly.
    #[cfg(test)]
    pub(crate) node: NodeId,
    /// Clip-local coordinates → viewport CSS px.
    pub(crate) transform: Transform3D<f32>,
    /// Padding box in clip-local coordinates.
    pub(crate) rect: Rect<f32>,
    /// Inner border radii (outer radii inset by border widths, clamped ≥ 0).
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
    /// Flushes styles and layout, then builds the private frame description
    /// shared by painting, hit testing, and input routing.
    pub(crate) fn build_paint_order(&mut self) -> PaintOrder {
        self.layout();
        build::build(self)
    }

    /// Returns the topmost hit-testable element at `point` in viewport CSS px.
    pub fn hit_test(&mut self, point: Point2D<f32>) -> Option<NodeId> {
        let frame = self.build_paint_order();
        frame.hit_test(self, point)
    }

    /// Lays out and renders the current document through its private painter.
    pub fn render(&mut self) {
        let frame = self.build_paint_order();
        self.painter.borrow_mut().paint(self, &frame);
    }

    /// Renders only when the retained scene no longer represents the current
    /// document state. Returns whether a new scene was built.
    pub fn render_if_needed(&mut self) -> bool {
        if !self.needs_render() {
            return false;
        }
        self.render();
        true
    }
}

impl<T> Document<T> {
    /// Whether a visual mutation has made the retained scene stale, or no
    /// scene has been built yet.
    #[must_use]
    pub fn needs_render(&self) -> bool {
        self.painter.borrow().needs_render(self.visual_epoch())
    }

    /// The Vello scene retained by the last [`Self::render`] call.
    #[must_use]
    pub fn scene(&self) -> Ref<'_, crate::vello::Scene> {
        Ref::map(self.painter.borrow(), crate::painter::Painter::scene)
    }

    /// Registers or updates decoded images without exposing the painter.
    ///
    /// Access conservatively advances the visual epoch so a retained scene is
    /// never reused after its resource set may have changed.
    pub fn images_mut(&mut self) -> &mut ImageStore {
        self.note_visual_mutation();
        self.painter.get_mut().images_mut()
    }
}
