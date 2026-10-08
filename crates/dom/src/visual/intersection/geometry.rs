//! The intersection of one target with one root — Intersection Observer
//! §3.2.7 "compute the intersection" with Chromium's clipping — read off the
//! last completed layout and the live scroll offsets.
//!
//! # No pass runs
//!
//! Like [`Document::bounding_client_rect`], nothing here runs style, layout
//! or paint: the answer is what the last [`Document::layout`] left behind,
//! moved by the scroll offsets as they are now. A scroll the painter posted
//! inside the encode window commits nothing, and still moves every answer
//! here.
//!
//! # The walk
//!
//! The target's border box is carried up its containing-block chain
//! ([`ContainingBlockChain`], the walk `bounding_client_rect` folds too) to
//! the viewport. Each box on the way contributes, in this order:
//!
//! 1. **Its local matrix**, into its box parent's border-box space: the painter's own
//!    [`ContextMatrix`] — the transform list, the individual transforms, the motion path,
//!    `transform-origin`, the offset in the parent and the parent's `perspective` — so the geometry
//!    is the frame's, not a second reading of the same properties. The offset is the rounded
//!    location plus the live sticky offset and css-anchor-position-1's default scroll shift,
//!    exactly where `bounding_client_rect` adds them: for the target always, for an ancestor when
//!    it is on the chain. A box with no transform factor under no perspective is a plain
//!    translation.
//! 2. **Its scroll offset**, when it is a scroll container on the chain.
//! 3. **Its clip**, when it clips on any axis and is on the chain: its padding box, an unbounded
//!    strip on an axis it does not clip — the rect the frame's own clip node carries (`contain:
//!    paint`, and so `content-visibility`, included).
//!
//! A box *off* the chain — a scroller an out-of-flow target escapes — adds
//! its location and nothing else: an out-of-flow box neither scrolls with
//! nor is clipped by the boxes between it and its containing block, the
//! escape the paint walk keys its flow contexts on. That such a box carries
//! no matrix is not a simplification. Every box with one (a `transform`, a
//! `perspective`, an `offset-path`, a running transform animation)
//! establishes a containing block for all of its descendants, so it is on
//! every chain that passes it — the individual `translate`/`rotate`/`scale`
//! would be the exception, and they are storage-only in the fork's grammar.
//! The ancestors of a top-layer element are off its chain by membership, and
//! the painter builds such an element at the identity, past every ancestor's
//! matrix, as the walk does.
//!
//! # Matrices compose, rects are projected at clips
//!
//! The carry is one matrix from the target's space into the current box's,
//! and the part of the target the clips met so far left visible. A rect is
//! projected — its four corners mapped, their bounding box taken — only where
//! a clip has to read it, as Chromium's geometry mapper does: a target under
//! nested rotations keeps its exact bounding box, and its intersection rect
//! inflates only where a clip cut it in a rotated space. A corner a
//! perspective puts behind the viewer (`w ≤ 0`) has no projection; such a
//! target is reported rendered, with zero rects, and not intersecting.
//!
//! # Intersection is edge-inclusive
//!
//! Every clip and the root intersect *inclusively*: two rects that only touch
//! still intersect, in a zero-area rect on the shared edge. So a zero-area
//! target on the root's edge is intersecting with ratio 1, and a target
//! scrolled exactly to a scrollport's edge is intersecting with ratio 0 —
//! Chromium's answers. euclid's `Rect::intersection` drops zero-area
//! overlaps, hence [`intersect_inclusive`].
//!
//! # The root
//!
//! The implicit root is the viewport: `(0, 0, viewport size)`, dilated by the
//! resolved root margin, intersected after the document element's own
//! matrix.
//!
//! An element root's rect is its padding box when it clips on any axis and
//! its border box otherwise, dilated in its own border-box space (a negative
//! margin that eats the rect clamps its size at 0). Chromium takes the
//! padding box only when *both* axes clip; the two differ only for an
//! `overflow: clip visible` root with a border. Its bounds, the entry's
//! `rootBounds`, are that rect carried to the viewport (matrices and scroll
//! offsets, no clips). The target has to reach the root **on its chain** —
//! the spec's "descendant of the intersection root in the containing block
//! chain". A target whose chain passes the root off it (a `fixed` box under
//! a scroller root that is not its containing block, a top-layer element),
//! never passes it, or *is* the root is not a descendant: rendered, zero
//! rects, the root bounds still reported. At the root the carry is cut to
//! the dilated rect — in place of the root's own clip, which the margin
//! extends — and above it nothing clips any more: the carry is only mapped,
//! so every rect comes out in viewport coordinates.
//!
//! # Not rendered
//!
//! [`IntersectionGeometry::UNRENDERED`] answers for a target that generates
//! no box: no live node, no style, `display: none` or `contents`, no rounded
//! layout, a layout slot hughie hid — every box under a `display: none`
//! ancestor, every box under one that skips its contents (css-contain-2
//! §4.5: skipped contents are never intersecting), an inline element a text
//! block absorbed into its paragraph — or a chain that ends detached or
//! under a `display: none` ancestor. `visibility: hidden` is rendered: it
//! hides the paint, not the box.
//!
//! # Out
//!
//! - **Clip shapes.** Clips are padding-box rectangles: `clip-path`, masks and rounded corners do
//!   not narrow them, nor does `overflow-clip-margin` widen them (the frame clips to the padding
//!   box too).
//! - **`scrollMargin`**, **`delay`** and **`trackVisibility`**: `isVisible` is not modelled.
//! - **A transform curve mid-flight on the compositor.** The matrix is the main thread's last
//!   cascade, as of the document's animation clock, not the painter's sample of an exported curve.
//! - **`position-visibility`.** A box it hides stays geometric, as it does for
//!   `bounding_client_rect`: the hide is a paint and hit-testing effect.

use euclid::default::{Point2D, Rect, Size2D, Transform3D, Vector2D};
use hughie::geometry::Edges;
use hughie::style::{CoreStyle, PositionProperty};
use stylo::properties::ComputedValues;
use stylo::values::computed::motion::OffsetPath;
use stylo::values::computed::transform::{Rotate, Scale, Translate};

use crate::layout::{ChainEnd, ContainingBlockChain, DisplayMode, Layout, StyleView, display_mode};
use crate::tree::document::{Document, NodeId};
use crate::visual::build::{clipped_axes, padding_box, unclipped_axes_unbounded};
use crate::visual::sticky;
use crate::visual::transform::{ContextMatrix, ParentPerspective};

/// One side of a [`RootMargin`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarginLength {
    /// CSS pixels.
    Px(f32),
    /// A percentage of the undilated root rect, as written: `20.0` is `20%`.
    /// Top and bottom take the rect's height, left and right its width.
    Percent(f32),
}

impl MarginLength {
    fn resolve(self, basis: f32) -> f32 {
        match self {
            Self::Px(px) => px,
            Self::Percent(percent) => basis * percent / 100.0,
        }
    }
}

/// An observer's `rootMargin`: how far each side of the root intersection
/// rectangle moves out (positive) or in (negative).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RootMargin {
    pub top: MarginLength,
    pub right: MarginLength,
    pub bottom: MarginLength,
    pub left: MarginLength,
}

impl RootMargin {
    /// `0px` on every side, the spec's default.
    pub const ZERO: Self = Self {
        top: MarginLength::Px(0.0),
        right: MarginLength::Px(0.0),
        bottom: MarginLength::Px(0.0),
        left: MarginLength::Px(0.0),
    };

    /// Each side in CSS pixels against the `undilated` root rect.
    ///
    /// Percentages of top and bottom take its height, of left and right its
    /// width — what Chromium, `WebKit` and Gecko do and WPT `root-margin.html`
    /// tests, where the spec's sentence says width for all four.
    #[must_use]
    pub fn resolve(&self, undilated: &Rect<f32>) -> Edges<f32> {
        let (width, height) = (undilated.size.width, undilated.size.height);
        Edges {
            left: self.left.resolve(width),
            right: self.right.resolve(width),
            top: self.top.resolve(height),
            bottom: self.bottom.resolve(height),
        }
    }

    /// `rect` grown by the resolved margins. A size the margins drive
    /// negative clamps to 0, keeping the moved origin.
    fn dilate(&self, rect: Rect<f32>) -> Rect<f32> {
        let margin = self.resolve(&rect);
        Rect::new(
            Point2D::new(rect.origin.x - margin.left, rect.origin.y - margin.top),
            Size2D::new(
                (rect.size.width + margin.left + margin.right).max(0.0),
                (rect.size.height + margin.top + margin.bottom).max(0.0),
            ),
        )
    }
}

impl Default for RootMargin {
    fn default() -> Self {
        Self::ZERO
    }
}

/// An intersection root resolved once for every target measured against it:
/// [`Document::root_geometry`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RootGeometry {
    /// `None` for the implicit root, the viewport.
    root: Option<NodeId>,
    /// The root intersection rectangle in the root's own border-box space
    /// (the viewport's, for the implicit root): the root rect, dilated.
    local: Rect<f32>,
    /// The root intersection rectangle in viewport CSS px — the entry's
    /// `rootBounds`.
    pub bounds: Rect<f32>,
}

impl RootGeometry {
    /// The root element, or `None` for the implicit root.
    #[must_use]
    pub const fn root(&self) -> Option<NodeId> {
        self.root
    }
}

/// One target's geometry against one root, in viewport CSS px: what an
/// `IntersectionObserverEntry` reports, minus its time and target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntersectionGeometry {
    /// Whether the target generates a box this frame. An unrendered target
    /// is not intersecting and reports zero rects throughout
    /// ([`Self::UNRENDERED`]).
    pub rendered: bool,
    /// Whether target and root intersect, edge-adjacency included, through
    /// every clip between them.
    pub is_intersecting: bool,
    /// The bounding box of the target's transformed border box — the
    /// entry's `boundingClientRect`. Zero when the target is not a
    /// descendant of the root.
    pub target_rect: Rect<f32>,
    /// The root intersection rectangle — the entry's `rootBounds`.
    pub root_bounds: Rect<f32>,
    /// The part of the target visible through every clip and the root —
    /// the entry's `intersectionRect`. Zero when not intersecting.
    pub intersection_rect: Rect<f32>,
}

const ZERO_RECT: Rect<f32> = Rect::new(Point2D::new(0.0, 0.0), Size2D::new(0.0, 0.0));

impl IntersectionGeometry {
    /// A target that generates no box.
    pub const UNRENDERED: Self = Self {
        rendered: false,
        is_intersecting: false,
        target_rect: ZERO_RECT,
        root_bounds: ZERO_RECT,
        intersection_rect: ZERO_RECT,
    };

    /// A rendered target that does not intersect the root and reports no
    /// rects of its own: not a descendant of the root, or projected behind
    /// the viewer.
    const fn missed(root: &RootGeometry) -> Self {
        Self {
            rendered: true,
            is_intersecting: false,
            target_rect: ZERO_RECT,
            root_bounds: root.bounds,
            intersection_rect: ZERO_RECT,
        }
    }

    /// The entry's `intersectionRatio`: the intersection's area over the
    /// target's, at most 1.
    ///
    /// A zero-area target has no ratio to take, so it is 1 when intersecting
    /// (edge-adjacency included) and 0 otherwise, as the spec says.
    #[must_use]
    pub fn intersection_ratio(&self) -> f64 {
        let area = |rect: &Rect<f32>| f64::from(rect.size.width) * f64::from(rect.size.height);
        let target = area(&self.target_rect);
        if target > 0.0 {
            (area(&self.intersection_rect) / target).min(1.0)
        } else if self.is_intersecting {
            1.0
        } else {
            0.0
        }
    }
}

impl<T> Document<T> {
    /// Resolves an intersection root and its `margin` once, for every target
    /// measured against it: `None` for the implicit root (the viewport), or
    /// an element.
    ///
    /// `None` when `root` is an element that is not rendered — see
    /// [`IntersectionGeometry::UNRENDERED`] for what that covers. Reads the
    /// last completed layout and the live scroll offsets; runs no pass.
    #[must_use]
    pub fn root_geometry(&self, root: Option<NodeId>, margin: &RootMargin) -> Option<RootGeometry> {
        let Some(element) = root else {
            let viewport = self.viewport_size();
            let local = margin.dilate(Rect::from_size(Size2D::new(
                viewport.width,
                viewport.height,
            )));
            return Some(RootGeometry {
                root: None,
                local,
                bounds: local,
            });
        };
        let carry = self.carry_to_viewport(element, None)?;
        let node = self.get(element)?;
        let style = node.layout_computed_style()?;
        let layout = self.rounded_layout(element)?;
        let axes = clipped_axes(node, style);
        let rect = if axes.x || axes.y {
            padding_box(layout)
        } else {
            Rect::from_size(Size2D::new(layout.size.width, layout.size.height))
        };
        let local = margin.dilate(rect);
        Some(RootGeometry {
            root: Some(element),
            local,
            // A walk that clips nothing projects nothing on the way, so this
            // is the one projection: a root a perspective puts behind the
            // viewer has no bounds to report.
            bounds: map_rect(&carry.world, &local).unwrap_or(ZERO_RECT),
        })
    }

    /// `target`'s geometry against a resolved `root`: Intersection Observer
    /// §3.2.7 with Chromium's clipping, through transforms. The module doc
    /// is the algorithm.
    ///
    /// Reads the last completed layout and the live scroll offsets; runs no
    /// pass.
    #[must_use]
    pub fn intersection_geometry_in(
        &self,
        target: NodeId,
        root: &RootGeometry,
    ) -> IntersectionGeometry {
        let Some(carry) = self.carry_to_viewport(target, Some(root)) else {
            return IntersectionGeometry::UNRENDERED;
        };
        if !carry.descends || carry.degenerate {
            return IntersectionGeometry::missed(root);
        }
        let Some((target_rect, visible)) = carry.rects() else {
            return IntersectionGeometry::missed(root);
        };
        IntersectionGeometry {
            rendered: true,
            is_intersecting: visible.is_some(),
            target_rect,
            root_bounds: root.bounds,
            intersection_rect: visible.unwrap_or(ZERO_RECT),
        }
    }

    /// [`Self::intersection_geometry_in`] against a root resolved for this
    /// one call. An element root that is not rendered leaves the target
    /// [`IntersectionGeometry::UNRENDERED`].
    #[must_use]
    pub fn intersection_geometry(
        &self,
        target: NodeId,
        root: Option<NodeId>,
        margin: &RootMargin,
    ) -> IntersectionGeometry {
        self.root_geometry(root, margin)
            .map_or(IntersectionGeometry::UNRENDERED, |root| {
                self.intersection_geometry_in(target, &root)
            })
    }

    /// Carries `id`'s border box to the viewport, clipping against `clip`'s
    /// root when there is one: `None` when `id` is not rendered.
    ///
    /// With no `clip`, the walk only maps — what an element root's bounds
    /// need.
    fn carry_to_viewport(&self, id: NodeId, clip: Option<&RootGeometry>) -> Option<Carry> {
        let node = self.get(id)?;
        let style = StyleView::try_of(node)?;
        if matches!(
            display_mode(style.display()),
            DisplayMode::None | DisplayMode::Contents
        ) {
            return None;
        }
        let state = self.layout_state().get(self.slot(id)?)?;
        if state.slot.is_hidden() {
            return None;
        }
        let layout = &state.slot.rounded;
        let mut offsets = Offsets {
            document: self,
            anchored: !self.layout_state().anchored.is_empty(),
            sticky: Vec::new(),
        };
        let mut carry = Carry::new(Size2D::new(layout.size.width, layout.size.height));
        let mut below = Below::of(
            style.values(),
            layout,
            offsets.of(id, style.values(), layout),
        );
        let element_root = clip.and_then(|root| root.root);
        // Whether the boxes the walk stands on still clip the carry: not
        // without a root to clip for, and not above an element root.
        let mut clipping = clip.is_some();
        let mut chain = ContainingBlockChain::new(self, node, style.values());
        for step in &mut chain {
            let values = step.style.values();
            let perspective = step
                .on_chain
                .then(|| {
                    ParentPerspective::of(
                        values,
                        Size2D::new(step.layout.size.width, step.layout.size.height),
                    )
                })
                .flatten();
            carry.apply(below.local(perspective));
            if step.on_chain && self.is_scroll_container(step.id) {
                carry.translate(-self.scroll_offset(step.id));
            }
            if element_root == Some(step.id) {
                if step.on_chain
                    && let Some(root) = clip
                {
                    carry.clip(&root.local);
                    carry.descends = true;
                }
                clipping = false;
            } else if clipping && step.on_chain {
                let axes = clipped_axes(step.node, values);
                if axes.x || axes.y {
                    carry.clip(&unclipped_axes_unbounded(padding_box(step.layout), axes));
                }
            }
            below = if step.on_chain {
                Below::of(
                    values,
                    step.layout,
                    offsets.of(step.id, values, step.layout),
                )
            } else {
                Below::located(step.layout)
            };
        }
        if chain.end() != Some(ChainEnd::DocumentElement) {
            return None;
        }
        // The document element's own matrix: its rounded location is its
        // offset, and there is no parent perspective above it.
        carry.apply(below.local(None));
        if let Some(root) = clip
            && root.root.is_none()
        {
            carry.clip(&root.local);
            carry.descends = true;
        }
        Some(carry)
    }
}

/// The offset a box adds to its location: its live sticky offset and its
/// default scroll shift, sampled the way `bounding_client_rect` samples
/// them.
struct Offsets<'a, T> {
    document: &'a Document<T>,
    /// Whether any anchor-positioned box exists, so a page without one asks
    /// for no shift.
    anchored: bool,
    /// Nested sticky samples, memoised across the walk.
    sticky: Vec<(NodeId, Vector2D<f32>)>,
}

impl<T> Offsets<'_, T> {
    fn of(&mut self, id: NodeId, style: &ComputedValues, layout: &Layout) -> Point2D<f32> {
        let mut offset = Point2D::new(layout.location.x, layout.location.y);
        if *style.get_box().get_position() == PositionProperty::Sticky {
            offset += sticky::live_offset(self.document, id, &mut self.sticky);
        }
        if self.anchored {
            offset += self
                .document
                .default_scroll_shift(id, None)
                .unwrap_or_default();
        }
        offset
    }
}

/// The box the carry is currently in, whose local matrix maps it into the
/// next box up.
struct Below<'a> {
    /// Its style, when its own matrix applies: the target, and an ancestor
    /// on the chain. `None` for an ancestor off it, which only translates.
    style: Option<&'a ComputedValues>,
    size: Size2D<f32>,
    offset: Point2D<f32>,
}

impl<'a> Below<'a> {
    fn of(style: &'a ComputedValues, layout: &Layout, offset: Point2D<f32>) -> Self {
        Self {
            style: Some(style),
            size: Size2D::new(layout.size.width, layout.size.height),
            offset,
        }
    }

    fn located(layout: &Layout) -> Self {
        Self {
            style: None,
            size: Size2D::new(layout.size.width, layout.size.height),
            offset: Point2D::new(layout.location.x, layout.location.y),
        }
    }

    /// Its local matrix under its parent's `perspective`: the painter's
    /// [`ContextMatrix`], or the plain translation it reduces to.
    fn local(&self, perspective: Option<ParentPerspective>) -> Local {
        match self.style {
            Some(style) if perspective.is_some() || has_transform_factors(style) => Local::Matrix(
                ContextMatrix::of(style, self.size, self.offset, perspective)
                    .with_list(&style.get_box().transform),
            ),
            _ => Local::Translate(self.offset.to_vector()),
        }
    }
}

/// Whether `style` has anything [`ContextMatrix`] folds besides the offset.
fn has_transform_factors(style: &ComputedValues) -> bool {
    let box_style = style.get_box();
    !box_style.transform.0.is_empty()
        || !matches!(box_style.translate, Translate::None)
        || !matches!(box_style.rotate, Rotate::None)
        || !matches!(box_style.scale, Scale::None)
        || !matches!(box_style.offset_path, OffsetPath::None)
}

/// One step's local matrix.
#[derive(Clone, Copy)]
enum Local {
    Translate(Vector2D<f32>),
    Matrix(Transform3D<f32>),
}

/// The target on its way up: one matrix from its border-box space into the
/// current box's, and what the clips met so far left of it.
struct Carry {
    size: Size2D<f32>,
    world: Transform3D<f32>,
    visible: Visible,
    /// A projection met a corner behind the viewer.
    degenerate: bool,
    /// Whether the target reached its root on its chain: always, for the
    /// implicit root; set at an element root's step when it is on the chain.
    descends: bool,
}

/// What the clips met so far left of the target.
enum Visible {
    /// No clip yet: the whole border box, under the carry's matrix.
    Whole,
    /// What the last clip left, in that clip's space; `since` maps it into
    /// the current one.
    Part {
        rect: Rect<f32>,
        since: Transform3D<f32>,
    },
    /// A clip it met was disjoint from it.
    Gone,
}

impl Carry {
    fn new(size: Size2D<f32>) -> Self {
        Self {
            size,
            world: Transform3D::identity(),
            visible: Visible::Whole,
            degenerate: false,
            descends: false,
        }
    }

    fn apply(&mut self, local: Local) {
        match local {
            Local::Translate(offset) => self.translate(offset),
            Local::Matrix(matrix) => {
                self.world = self.world.then(&matrix);
                if let Visible::Part { since, .. } = &mut self.visible {
                    *since = since.then(&matrix);
                }
            }
        }
    }

    fn translate(&mut self, offset: Vector2D<f32>) {
        let offset = euclid::default::Vector3D::new(offset.x, offset.y, 0.0);
        self.world = self.world.then_translate(offset);
        if let Visible::Part { since, .. } = &mut self.visible {
            *since = since.then_translate(offset);
        }
    }

    /// Cuts what is visible to `clip`, a rect in the current space.
    fn clip(&mut self, clip: &Rect<f32>) {
        let current = match &self.visible {
            Visible::Whole => map_rect(&self.world, &Rect::from_size(self.size)),
            Visible::Part { rect, since } => map_rect(since, rect),
            Visible::Gone => return,
        };
        let Some(current) = current else {
            self.degenerate = true;
            self.visible = Visible::Gone;
            return;
        };
        self.visible =
            intersect_inclusive(&current, clip).map_or(Visible::Gone, |rect| Visible::Part {
                rect,
                since: Transform3D::identity(),
            });
    }

    /// The target's bounding box in the current space, and what of it is
    /// visible there (`None` when a clip removed all of it); `None` when a
    /// corner has no projection.
    fn rects(&self) -> Option<(Rect<f32>, Option<Rect<f32>>)> {
        let target = map_rect(&self.world, &Rect::from_size(self.size))?;
        let visible = match &self.visible {
            Visible::Whole => Some(target),
            Visible::Part { rect, since } => Some(map_rect(since, rect)?),
            Visible::Gone => None,
        };
        Some((target, visible))
    }
}

/// The bounding box of `rect` under `matrix`, `None` when a corner lands
/// behind the viewer (`w ≤ 0`).
///
/// A pure translation moves the rect itself, so an untransformed box keeps
/// its size bit for bit instead of re-deriving it as `max - min`.
fn map_rect(matrix: &Transform3D<f32>, rect: &Rect<f32>) -> Option<Rect<f32>> {
    if is_translation(matrix) {
        return Some(rect.translate(Vector2D::new(matrix.m41, matrix.m42)));
    }
    let corners = [
        rect.min(),
        Point2D::new(rect.max_x(), rect.min_y()),
        rect.max(),
        Point2D::new(rect.min_x(), rect.max_y()),
    ];
    let mut projected = [Point2D::zero(); 4];
    for (point, corner) in projected.iter_mut().zip(corners) {
        *point = matrix.transform_point2d(corner)?;
    }
    Some(Rect::from_points(projected))
}

/// Whether `matrix` maps the plane by a translation alone: the entries
/// `transform_point2d` reads are the identity's but for `m41`/`m42`.
#[expect(
    clippy::float_cmp,
    reason = "exactly the entries a translation leaves at 0 and 1"
)]
fn is_translation(matrix: &Transform3D<f32>) -> bool {
    matrix.m11 == 1.0
        && matrix.m12 == 0.0
        && matrix.m14 == 0.0
        && matrix.m21 == 0.0
        && matrix.m22 == 1.0
        && matrix.m24 == 0.0
        && matrix.m44 == 1.0
}

/// The edge-inclusive intersection of `a` and `b`: `None` only when they are
/// disjoint, and a zero-area rect on the shared edge when they only touch.
///
/// Per axis, an extent the other rect contains is kept as it was, so a box
/// a clip does not cut keeps its size bit for bit — a fully visible target's
/// ratio is exactly 1.
fn intersect_inclusive(a: &Rect<f32>, b: &Rect<f32>) -> Option<Rect<f32>> {
    let (x, width) = overlap(a.origin.x, a.size.width, b.origin.x, b.size.width)?;
    let (y, height) = overlap(a.origin.y, a.size.height, b.origin.y, b.size.height)?;
    Some(Rect::new(Point2D::new(x, y), Size2D::new(width, height)))
}

/// One axis of [`intersect_inclusive`]: the start and length of the overlap
/// of `[a, a + a_len]` and `[b, b + b_len]`.
#[expect(
    clippy::float_cmp,
    reason = "an extent is kept exactly when the overlap is exactly it"
)]
fn overlap(a: f32, a_len: f32, b: f32, b_len: f32) -> Option<(f32, f32)> {
    let (a_end, b_end) = (a + a_len, b + b_len);
    let start = a.max(b);
    let end = a_end.min(b_end);
    if start > end {
        return None;
    }
    Some(if start == a && end == a_end {
        (a, a_len)
    } else if start == b && end == b_end {
        (b, b_len)
    } else {
        (start, end - start)
    })
}

#[cfg(test)]
#[allow(clippy::float_cmp, reason = "the ratios asserted are exact")]
mod tests {
    use super::*;
    use crate::test_common::Doc;
    use crate::visual::PaintItemKind;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect<f32> {
        Rect::new(Point2D::new(x, y), Size2D::new(width, height))
    }

    /// The geometry *is* the frame's: at zero scroll, with no sticky or
    /// anchored box (whose motion the frame composes at use, not into an
    /// item's matrix), every element box's target rect is its paint item's
    /// border box under the item's own world matrix — nested rotations,
    /// a perspective projecting a 3D child, a motion path, an out-of-flow
    /// box under a transformed containing block, an opacity context, a
    /// scroller's content, and a top-layer element past a rotated ancestor.
    #[test]
    fn every_element_box_is_where_the_painter_paints_it() {
        let mut doc = Doc::with_css(
            r#"page { display: flex; position: relative; width: 800px; height: 600px; }
               view { display: flex; flex-shrink: 0; }
               dialog { display: flex; }
               .a { position: relative; left: 13px; top: 7px; width: 300px; height: 300px;
                    transform: rotate(30deg); }
               .b { margin-left: 20px; margin-top: 15px; width: 200px; height: 150px;
                    overflow: hidden; perspective: 400px; }
               .c { width: 80px; height: 60px; transform: rotateY(40deg) translateZ(30px); }
               .e { position: relative; left: 5px; top: 9px; width: 40px; height: 30px; }
               .g { opacity: 0.5; margin-left: 7px; width: 60px; height: 60px; }
               .d { position: absolute; left: 30px; top: 40px; width: 50px; height: 50px;
                    transform: scale(1.5) rotate(-20deg); transform-origin: 10% 90%; }
               .f { position: fixed; left: 100px; top: 50px; width: 25px; height: 25px;
                    transform: skewX(10deg); }
               .p { margin-left: 30px; margin-top: 200px; width: 120px; height: 80px;
                    transform: translate(40px, 10px) rotate(10deg); }
               .q { margin-left: 11px; width: 50px; height: 20px; transform: rotate(80deg); }
               .mover { width: 30px; height: 10px;
                        offset-path: path("M 0 0 L 100 0 L 100 100"); offset-distance: 60%; }
               .scroller { flex-direction: column; overflow: scroll; margin-top: 20px;
                           width: 150px; height: 100px; transform: scale(0.75); }
               .row { width: 140px; height: 70px; }
               .tilted { width: 30px; height: 30px; margin-left: 9px;
                         transform: rotate(15deg); }
               dialog { position: static; margin-left: 3px; width: 40px; height: 40px;
                        transform: rotate(5deg); }"#,
        );
        let root = doc.root;
        let rotated = doc.el(root, "view.a");
        let clipper = doc.el(rotated, "view.b");
        let deep = doc.el(clipper, "view.c");
        doc.el(deep, "view.e");
        doc.el(clipper, "view.g");
        doc.el(rotated, "view.d");
        doc.el(rotated, "view.f");
        let dialog = doc.el(rotated, "dialog");
        let outer = doc.el(root, "view.p");
        let inner = doc.el(outer, "view.q");
        doc.el(inner, "view.mover");
        let scroller = doc.el(root, "view.scroller");
        for _ in 0..3 {
            let row = doc.el(scroller, "view.row");
            doc.el(row, "view.tilted");
        }
        doc.dom.add_to_top_layer(dialog, false);

        let order = doc.dom.build_paint_order();
        let mut checked = 0;
        for item in order.items() {
            if item.kind != PaintItemKind::ElementBox {
                continue;
            }
            let painted = map_rect(&item.transform, &Rect::from_size(item.size))
                .expect("every box here projects in front of the viewer");
            let geometry = doc
                .dom
                .intersection_geometry(item.node, None, &RootMargin::ZERO);
            assert!(geometry.rendered, "{:?}", item.node);
            let actual = geometry.target_rect;
            let close = |a: f32, b: f32| (a - b).abs() <= 1e-3;
            assert!(
                close(actual.origin.x, painted.origin.x)
                    && close(actual.origin.y, painted.origin.y)
                    && close(actual.size.width, painted.size.width)
                    && close(actual.size.height, painted.size.height),
                "{:?}: the geometry says {actual:?}, the painter {painted:?}",
                item.node,
            );
            checked += 1;
        }
        assert_eq!(checked, 19, "every element paints one box");
    }

    #[test]
    fn overlapping_rects_intersect_in_their_overlap() {
        assert_eq!(
            intersect_inclusive(
                &rect(0.0, 0.0, 100.0, 100.0),
                &rect(50.0, 25.0, 100.0, 50.0)
            ),
            Some(rect(50.0, 25.0, 50.0, 50.0))
        );
    }

    #[test]
    fn a_contained_rect_is_kept_exactly() {
        let inner = rect(0.1, 0.3, 100.7, 20.9);
        assert_eq!(
            intersect_inclusive(&inner, &rect(-5.0, -5.0, 500.0, 500.0)),
            Some(inner)
        );
        assert_eq!(
            intersect_inclusive(&rect(-5.0, -5.0, 500.0, 500.0), &inner),
            Some(inner)
        );
    }

    #[test]
    fn touching_rects_intersect_in_a_zero_area_rect_on_the_shared_edge() {
        // Side by side: a zero-width rect on x = 100.
        assert_eq!(
            intersect_inclusive(
                &rect(0.0, 0.0, 100.0, 100.0),
                &rect(100.0, 20.0, 50.0, 50.0)
            ),
            Some(rect(100.0, 20.0, 0.0, 50.0))
        );
        // Corner to corner: a point.
        assert_eq!(
            intersect_inclusive(&rect(0.0, 0.0, 10.0, 10.0), &rect(10.0, 10.0, 10.0, 10.0)),
            Some(rect(10.0, 10.0, 0.0, 0.0))
        );
        // euclid's own intersection drops both.
        assert_eq!(
            rect(0.0, 0.0, 100.0, 100.0).intersection(&rect(100.0, 20.0, 50.0, 50.0)),
            None
        );
    }

    #[test]
    fn a_zero_area_rect_on_an_edge_or_inside_intersects() {
        let root = rect(0.0, 0.0, 800.0, 600.0);
        let on_edge = rect(10.0, 600.0, 100.0, 0.0);
        assert_eq!(intersect_inclusive(&on_edge, &root), Some(on_edge));
        let point = rect(30.0, 40.0, 0.0, 0.0);
        assert_eq!(intersect_inclusive(&point, &root), Some(point));
    }

    #[test]
    fn disjoint_rects_do_not_intersect() {
        let root = rect(0.0, 0.0, 800.0, 600.0);
        assert_eq!(
            intersect_inclusive(&rect(0.0, 601.0, 100.0, 0.0), &root),
            None
        );
        assert_eq!(
            intersect_inclusive(&rect(-11.0, 0.0, 10.0, 10.0), &root),
            None
        );
        assert_eq!(
            intersect_inclusive(&rect(0.0, 0.0, 10.0, 10.0), &rect(10.5, 0.0, 10.0, 10.0)),
            None
        );
    }

    #[test]
    fn a_projection_behind_the_viewer_has_no_rect() {
        // A perspective `w` that goes non-positive across the rect.
        let mut matrix = Transform3D::identity();
        matrix.m14 = -0.01;
        assert!(map_rect(&matrix, &rect(0.0, 0.0, 50.0, 50.0)).is_some());
        assert_eq!(map_rect(&matrix, &rect(0.0, 0.0, 200.0, 50.0)), None);
    }

    #[test]
    fn a_translation_moves_the_rect_itself() {
        let matrix = Transform3D::translation(0.1, 0.2, 0.0);
        let moved = map_rect(&matrix, &rect(0.0, 0.0, 100.0, 20.0)).expect("affine");
        assert_eq!(moved.size, Size2D::new(100.0, 20.0));
        assert_eq!(moved.origin, Point2D::new(0.1, 0.2));
    }

    #[test]
    fn ratios_follow_area_and_zero_area_targets_follow_intersection() {
        let geometry = |target: Rect<f32>, visible: Option<Rect<f32>>| IntersectionGeometry {
            rendered: true,
            is_intersecting: visible.is_some(),
            target_rect: target,
            root_bounds: ZERO_RECT,
            intersection_rect: visible.unwrap_or(ZERO_RECT),
        };
        let target = rect(0.0, 0.0, 10.0, 20.0);
        assert_eq!(geometry(target, Some(target)).intersection_ratio(), 1.0);
        assert_eq!(
            geometry(target, Some(rect(0.0, 0.0, 10.0, 5.0))).intersection_ratio(),
            0.25
        );
        assert_eq!(
            geometry(target, Some(rect(0.0, 20.0, 10.0, 0.0))).intersection_ratio(),
            0.0,
            "edge-adjacent: intersecting with ratio 0",
        );
        assert_eq!(geometry(target, None).intersection_ratio(), 0.0);
        let line = rect(0.0, 0.0, 10.0, 0.0);
        assert_eq!(geometry(line, Some(line)).intersection_ratio(), 1.0);
        assert_eq!(geometry(line, None).intersection_ratio(), 0.0);
        // A projected intersection a rotated clip inflated past the target
        // still reads at most 1.
        assert_eq!(
            geometry(target, Some(rect(-1.0, -1.0, 12.0, 22.0))).intersection_ratio(),
            1.0
        );
        assert_eq!(IntersectionGeometry::UNRENDERED.intersection_ratio(), 0.0);
    }

    #[test]
    fn margins_resolve_percentages_per_axis_and_clamp_eaten_sizes() {
        let margin = RootMargin {
            top: MarginLength::Px(10.0),
            right: MarginLength::Percent(20.0),
            bottom: MarginLength::Percent(40.0),
            left: MarginLength::Px(30.0),
        };
        let viewport = rect(0.0, 0.0, 800.0, 600.0);
        assert_eq!(
            margin.resolve(&viewport),
            Edges {
                left: 30.0,
                right: 160.0,
                top: 10.0,
                bottom: 240.0,
            }
        );
        assert_eq!(margin.dilate(viewport), rect(-30.0, -10.0, 990.0, 850.0));
        let eaten = RootMargin {
            top: MarginLength::Px(-400.0),
            right: MarginLength::Px(-500.0),
            bottom: MarginLength::Px(-400.0),
            left: MarginLength::Px(-500.0),
        };
        assert_eq!(eaten.dilate(viewport), rect(500.0, 400.0, 0.0, 0.0));
        assert_eq!(RootMargin::ZERO.dilate(viewport), viewport);
    }
}
