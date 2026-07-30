//! Visual order over the laid-out tree: CSS stacking contexts, Appendix-E
//! paint order, transform matrices, and hit testing.
//!
//! [`Document::paint_order`] flushes styles and layout, then builds a
//! [`PaintOrder`]: a flat item list in back-to-front paint order, each item
//! carrying its viewport-space transform and innermost clip. The future
//! render crate paints the items in list order (back to front); event
//! dispatch consumes [`PaintOrder::hit_test`], which walks the same list in
//! reverse (topmost first). Hit-test consumers should hold one `PaintOrder`
//! per frame and query it repeatedly rather than calling
//! [`Document::hit_test`] per pointer event — every call rebuilds the world.
//! A frame is pinned to the document's node-removal epoch: after any
//! `remove_subtree` a freed id can be recycled by a later creation, so
//! querying a stale frame panics (let-it-crash) rather than returning a
//! recycled node for old geometry. Painting is pinned harder — to
//! [`Document::visual_epoch`] via [`PaintOrder::assert_visually_fresh`] —
//! because it resolves the frame against live styles/layouts/text, which
//! any visual mutation desynchronizes; hit testing's snapshot is
//! self-contained and tolerates non-structural mutations.
//!
//! Scroll offsets ([`crate::scroll`]) are baked into the frame rather than
//! surfaced beside it: a scroll container's contents are translated as they
//! are collected, so painting and hit testing both see scrolled geometry with
//! no knowledge of scrolling at all, and the render crate needs none either.
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
//! - Group effects (`opacity`, `filter`, `clip-path`, `mask`) surface as [`RenderLayer`] boundaries
//!   for the render crate to composite; they still do not affect hit testing (a `clip-path` that
//!   clips painting away does not clip the hit region yet). `backdrop-filter` is not compiled in
//!   the fork at all, so its stacking-context trigger is structurally deferred.
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
mod transform;

// The crate's whole geometry vocabulary, re-exported so an embedder can name
// every type that appears in a public signature here — `Vector2D` included:
// scroll offsets and deltas are displacements, not positions, and they surface
// on `Document::scroll_by`, `ScrollBox`, and `DefaultAction::Scroll`.
pub use euclid::default::{Point2D, Rect, Size2D, Transform3D, Vector2D};

use crate::NodeId;
use crate::document::Document;

/// The document's current frame in paint order: `items[0]` paints first
/// (bottom), `items[len - 1]` paints last (top).
///
/// A frame is a snapshot. It stays queryable while the document's node set
/// is unchanged; once nodes are removed (`Document::remove_subtree`), freed
/// ids can be recycled and [`Self::hit_test`] fails fast instead of
/// answering with a recycled node — rebuild the frame after structural
/// mutations.
#[derive(Debug)]
pub struct PaintOrder {
    pub(crate) items: Vec<PaintItem>,
    pub(crate) clips: Vec<ClipNode>,
    pub(crate) layers: Vec<RenderLayer>,
    /// [`Document::node_removal_epoch`] at build time.
    pub(crate) epoch: u64,
    /// [`Document::visual_epoch`] at build time.
    pub(crate) visual_epoch: u64,
}

impl PaintOrder {
    /// Back-to-front paint items.
    #[must_use]
    pub fn items(&self) -> &[PaintItem] {
        &self.items
    }

    /// Clip arena referenced by [`PaintItem::clip`] and [`ClipNode::parent`].
    #[must_use]
    pub fn clips(&self) -> &[ClipNode] {
        &self.clips
    }

    /// Render layers, in preorder (a layer precedes every layer nested in
    /// it, and its [`RenderLayer::items`] range contains theirs).
    #[must_use]
    pub fn layers(&self) -> &[RenderLayer] {
        &self.layers
    }
}

/// Where a hit landed inside the item it hit, from
/// [`PaintOrder::hit_test_local`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalHit {
    /// Index into [`PaintOrder::items`].
    pub item: usize,
    /// The point in that item's border-box space.
    pub position: Point2D<f32>,
}

/// What one paint item draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintItemKind {
    /// An element's own box: background, borders, decorations.
    ElementBox,
    /// A text leaf's glyph runs; `element` is the styled parent element hit
    /// testing resolves to.
    TextRun { element: NodeId },
}

/// One node's slot in the paint order.
#[derive(Debug, Clone)]
pub struct PaintItem {
    pub node: NodeId,
    pub kind: PaintItemKind,
    /// Item-local border-box coordinates → viewport CSS px. Flattened
    /// 2D-projective (z decoupled); a singular matrix means the element is
    /// not rendered (css-transforms-1) and hit testing skips it.
    pub transform: Transform3D<f32>,
    /// Innermost applicable clip, an index into [`PaintOrder::clips`].
    pub clip: Option<usize>,
    /// Rounded border-box size.
    pub size: Size2D<f32>,
    /// Resolved, overlap-normalized border radii (zero for text runs).
    pub radii: CornerRadii,
    /// `visibility: visible` and `pointer-events` other than `none`.
    pub hit_testable: bool,
}

/// A stacking context whose subtree composites as a group — `opacity` below
/// one, `filter`, `clip-path`, `mask`, and the storage-only blend/isolation
/// triggers once a grammar rebase makes them authorable. The renderer must
/// paint the enclosed items into an offscreen group and apply the effects on
/// composite; contexts without group effects (plain `z-index`, transforms)
/// deliberately get no layer.
///
/// Effect *parameters* are not duplicated here: the establishing element's
/// computed style (via [`Document::paint_style`]) is the single source the
/// renderer resolves `opacity`/`filter`/`clip-path`/`mask` values from, with
/// `clip-path` and `mask` geometry resolved against [`Self::size`].
///
/// Group effects still do not affect [`PaintOrder::hit_test`] (a `clip-path`
/// that clips painting away does not yet clip the hit region — recorded v1
/// limit).
#[derive(Debug, Clone)]
pub struct RenderLayer {
    /// Next-outer render layer, an index into [`PaintOrder::layers`].
    pub parent: Option<usize>,
    /// The establishing element.
    pub node: NodeId,
    /// Layer-local (border-box) coordinates → viewport CSS px. Present even
    /// when the establishing box itself paints no item (`visibility:
    /// hidden` root with visible descendants).
    pub transform: Transform3D<f32>,
    /// Rounded border-box size of the establishing element.
    pub size: Size2D<f32>,
    /// The establishing element's resolved, overlap-normalized border
    /// radii — carried here (not scavenged from the root's item) so
    /// `clip-path`/`mask` geometry keeps its rounding even when the root
    /// box paints no item (`visibility: hidden`).
    pub radii: CornerRadii,
    /// Half-open range of [`PaintOrder::items`] the group encloses.
    /// Stacking contexts paint atomically, so the range is contiguous and
    /// nested layers' ranges nest; empty groups are dropped at build time,
    /// so the range is never empty.
    pub items: std::ops::Range<usize>,
}

/// One overflow/`contain: paint` clip: a rounded padding-box rect in the
/// establishing element's local space.
#[derive(Debug, Clone)]
pub struct ClipNode {
    /// Next-outer clip in this node's containing-block chain.
    pub parent: Option<usize>,
    /// The establishing element.
    pub node: NodeId,
    /// Clip-local coordinates → viewport CSS px.
    pub transform: Transform3D<f32>,
    /// Padding box in clip-local coordinates.
    pub rect: Rect<f32>,
    /// Inner border radii (outer radii inset by border widths, clamped ≥ 0).
    pub radii: CornerRadii,
}

/// Per-corner elliptical radii, in CSS px: `width` is the horizontal radius,
/// `height` the vertical.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CornerRadii {
    pub top_left: Size2D<f32>,
    pub top_right: Size2D<f32>,
    pub bottom_right: Size2D<f32>,
    pub bottom_left: Size2D<f32>,
}

impl CornerRadii {
    pub const ZERO: Self = Self {
        top_left: Size2D::new(0.0, 0.0),
        top_right: Size2D::new(0.0, 0.0),
        bottom_right: Size2D::new(0.0, 0.0),
        bottom_left: Size2D::new(0.0, 0.0),
    };

    #[must_use]
    pub fn is_zero(&self) -> bool {
        *self == Self::ZERO
    }
}

impl<T: Sync> Document<T> {
    /// Flushes styles and layout, then builds the frame's paint order.
    pub fn paint_order(&mut self) -> PaintOrder {
        self.layout();
        build::build(self)
    }

    /// Convenience for one-off queries: [`Self::paint_order`] plus
    /// [`PaintOrder::hit_test`]. `point` is in viewport CSS px.
    pub fn hit_test(&mut self, point: Point2D<f32>) -> Option<NodeId> {
        let frame = self.paint_order();
        frame.hit_test(self, point)
    }
}
