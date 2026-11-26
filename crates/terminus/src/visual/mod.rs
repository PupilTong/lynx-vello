//! Visual order over the laid-out tree: CSS stacking contexts, Appendix-E
//! paint order, transform matrices, and hit testing.
//!
//! [`Document::render`] builds the private `PaintOrder`: a flat item list in
//! back-to-front order, each item carrying its viewport-space transform and
//! innermost clip. It may build it more than once — see
//! [`relevance`] — but claims one commit id and publishes one frame either
//! way. The private painter walks it forwards, back to front,
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
//! because a [`crate::tree::document::NodeId`] is retired on free and never reissued: an id
//! the frame still names resolves either to the node it was drawn for or to
//! nothing, never to a different node that took its place.
//! Painting is pinned harder — to the current commit id with a clean dirty
//! bit via its freshness assertion, let-it-crash — because it resolves the
//! frame against live styles/layouts/text, which any visual mutation
//! desynchronizes; hit testing's snapshot is self-contained and tolerates
//! non-structural mutations, which is exactly what lets an event landing
//! between a scroll and its repaint hit the content the user was shown.
//!
//! The frame is built unscrolled, unstuck and at committed transforms. Scroll
//! offsets ([`crate::scroll`]), sticky shifts, anchor-positioned boxes'
//! default scroll shifts and `position-visibility` ([`anchored`]) and
//! exported animation curves are nodes of its compose space tree ([`space`]),
//! applied at composition
//! and inverted by hit testing, so a scroll inside its encode window or a
//! frame of an exported animation recomposes the retained frame instead of
//! rebuilding it.
//!
//! Invariants this module relies on (verified against the layout host):
//! - `Layout.location` is border-box-relative to the **box parent**'s border box for every box —
//!   the container whose formatting context laid the box out, which is the DOM parent except across
//!   dissolved `display: contents` levels — including hoisted absolute/fixed boxes (the layout
//!   host's `hoisted` module rewrites their location back into parent-relative terms). The build
//!   walks the same flattened box-tree, so plain offset accumulation along it is sound.
//! - Rounded layouts stay in CSS px with parent-relative locations that telescope exactly to
//!   snapped absolute positions at any device scale.
//! - Subtrees the layout host zeroes (display:none, unstyled descendants, `DisplayMode::Leaf`
//!   children, skipped `content-visibility` contents) are exactly the subtrees this module skips.
//!   The descendants of an outermost `svg` element are among them: what they draw reaches the order
//!   as the element's recording, appended after its box item ([`svg`]). That is a two-way street
//!   for `content-visibility: auto`: the skipping is decided *here*, by [`relevance`], out of the
//!   frame this module built — so the frame a render publishes is always the one built under the
//!   bits the layout it was built on used. Note that the decision reads the build's [`AutoBox`]
//!   records, not its items: relevance is geometric, and a `visibility: hidden` `auto` element has
//!   no item yet still decides whether its contents lay out.
//! - `display: contents` (css-display-3 §2.5) dissolves in lockstep with layout: this module walks
//!   the engine's own `LayoutTree::flattened_children`, so dissolved grandchildren paint, stack,
//!   and hit as members of the box parent's context; the boxless element itself paints nothing,
//!   triggers nothing (no stacking context, clip, or transform), and relays only inherited values
//!   (visibility, pointer-events) into its children. A text run whose DOM parent is boxless still
//!   hit-targets that parent — the singular text-hit rule, matching Chrome's `elementFromPoint`.
//!
//! Deliberate v1 limits (see docs/tracking/css-layout.md for status):
//! - Group effects (`opacity`, `filter`, `backdrop-filter`, `clip-path`, `mask`) surface as private
//!   `RenderLayer` boundaries for the private painter to composite; they still do not affect hit
//!   testing (a `clip-path` that clips painting away does not clip the hit region yet).
//!   `backdrop-filter` reaches the cascade, the stacking-context predicate, the group layer and the
//!   fixed/absolute containing-block rule; its backdrop is baked by the painter
//!   ([`crate::paint::compose`]).
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
//! - `position: sticky` establishes a stacking context and retains its normal-flow layout. Private
//!   sticky constraints clamp its visual offset to the scrollport and containing block at
//!   composition time; descendants and clips inherit that offset through their containing blocks,
//!   and hit testing samples the same geometry.
//! - `transform-style: preserve-3d`, `backface-visibility`, and `perspective-origin` are not
//!   authorable (the latter two are not even compiled) — everything flattens and perspective
//!   projects about the border-box center. Scroll and sticky displacements use the ancestor's
//!   affine axes; projective ancestors do not reproject these displacements as the offset changes.
//! - `content-visibility: auto` relevance is decided against the frame's own culling region, so an
//!   `auto` box whose estimate (`contain-intrinsic-size`) was wrong may move other boxes into or
//!   out of that region when it reveals. Those are re-determined by the next commit, not this one:
//!   a box determined in an earlier pass of a commit is deliberately not re-asked in a later one,
//!   which is what bounds the pass loop. Browsers have the same one-update lag.
//! - No incremental visual-order structure. The last `PaintOrder` is retained beside the scene, but
//!   only as the hit-test snapshot: it is never an input to the next build, and every visual
//!   mutation rebuilds the whole order. Invalidation is one private dirty bit
//!   ([`Document::needs_render`]) — every kind of change sets the same bit, so any two changes are
//!   indistinguishable to the render path; a frame's identity is its commit id, which orders
//!   commits and carries no damage information. `StyleDamage`'s repaint and stacking classes are
//!   computed by the style flush and dropped; they are what a tiered scheme would key on, but
//!   nothing on this path reads them today.

pub(crate) mod anchored;
mod build;
pub(crate) mod container_timing;
pub(crate) mod curves;
pub(crate) mod frame;
pub(crate) mod geometry;
mod hit;
pub(crate) mod intersection;
mod motion;
#[cfg(test)]
mod oracle_tests;
pub(crate) mod reach;
pub(crate) mod relevance;
pub(crate) mod resize;
pub(crate) mod space;
mod stacking;
pub(crate) mod sticky;
mod sticky_frame;
mod svg;
#[cfg(test)]
mod tests;
pub(crate) mod timing;
pub(crate) mod transform;

use std::sync::Arc;

use euclid::default::{Point2D, Rect, Size2D, Transform3D, Vector2D};
use rustc_hash::{FxHashMap, FxHashSet};
use stylo::properties::animated_properties::AnimationValueMap;

pub(crate) use self::build::BuildScratch;
pub(crate) use self::frame::ScrollOffsets;
pub use self::frame::{
    AnimationSlot, CommittedFrame, HitTarget, ScrollSlot, SnapSlot, SnapSlotAxis,
};
pub use self::relevance::ContentVisibilityChange;
pub(crate) use self::space::{Space, SpaceKind, SpaceSamples};
#[cfg(test)]
pub(crate) use self::sticky_frame::StickySample;
pub(crate) use self::sticky_frame::{StickySamples, StickySlot};
use crate::paint::display_list::DisplayList;
use crate::render::image::{DocumentRequest, FrameImages, ImageEvent, ImageStatus};
use crate::scroll::initial_target::InitialTarget;
use crate::scroll::{AutoScrollRequest, SnapAxis, SnapPoint};
use crate::tree::document::{Document, NodeId};

#[derive(Debug)]
pub(crate) struct PaintOrder {
    items: Vec<PaintItem>,
    clips: Vec<ClipNode>,
    layers: Vec<RenderLayer>,
    slots: Vec<ScrollSlot>,
    slot_index: FxHashMap<NodeId, u32>,
    animations: Vec<AnimationSlot>,
    stickies: Vec<StickySlot>,
    spaces: Vec<Space>,
    anchored: Vec<anchored::AnchoredSlot>,
    anchor_links: Vec<anchored::AnchorLink>,
    anchor_clips: Vec<u32>,
    anchored_order: Vec<u32>,
    auto_boxes: Vec<AutoBox>,
    snap_points: Vec<SnapPoint>,
    matrices: Vec<Transform3D<f32>>,
    initial_targets: Vec<InitialTarget>,
    auto_scroll_requests: Vec<(NodeId, AutoScrollRequest)>,
    inert_floor: Option<usize>,
    commit_id: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SlotSamples<S>(smallvec::SmallVec<[(u32, S); 4]>);

impl<S> Default for SlotSamples<S> {
    fn default() -> Self {
        Self(smallvec::SmallVec::new())
    }
}

impl<S> SlotSamples<S> {
    pub(crate) fn get(&self, slot: u32) -> &S {
        let entries = &self.0;
        let index = match entries.get(slot as usize) {
            Some((at, _)) if *at == slot => slot as usize,
            _ => entries
                .binary_search_by_key(&slot, |(at, _)| *at)
                .expect("a composed space's slots are sampled"),
        };
        &entries[index].1
    }

    fn push(&mut self, slot: u32, sample: S) {
        debug_assert!(
            self.0.last().is_none_or(|(last, _)| *last < slot),
            "slots are sampled in ascending order",
        );
        self.0.push((slot, sample));
    }

    pub(crate) fn entries_mut(&mut self) -> impl Iterator<Item = (u32, &mut S)> {
        self.0.iter_mut().map(|(slot, sample)| (*slot, sample))
    }
}

impl<S> FromIterator<(u32, S)> for SlotSamples<S> {
    fn from_iter<I: IntoIterator<Item = (u32, S)>>(entries: I) -> Self {
        let mut samples = Self::default();
        for (slot, sample) in entries {
            samples.push(slot, sample);
        }
        samples
    }
}

pub(crate) type AnimationSamples = SlotSamples<AnimationSample>;

#[derive(Debug, Default)]
pub(crate) struct ComposedMarks {
    spaces: Vec<bool>,
    stickies: Vec<bool>,
}

#[derive(Debug, Default)]
pub(crate) struct ComposedSlots {
    pub(crate) animations: Vec<u32>,
    pub(crate) stickies: Vec<u32>,
    pub(crate) clock: bool,
    pub(crate) scroll: bool,
}

impl ComposedSlots {
    pub(crate) fn clear(&mut self) {
        self.animations.clear();
        self.stickies.clear();
        self.clock = false;
        self.scroll = false;
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AnimationSample {
    pub(crate) delta: crate::kurbo::Affine,
    pub(crate) alpha: Option<f32>,
}

#[derive(Debug, Default)]
pub(crate) struct FrameBuffers {
    items: Vec<PaintItem>,
    clips: Vec<ClipNode>,
    layers: Vec<RenderLayer>,
    slots: Vec<ScrollSlot>,
    slot_index: FxHashMap<NodeId, u32>,
    animations: Vec<AnimationSlot>,
    stickies: Vec<StickySlot>,
    spaces: Vec<Space>,
    anchored: Vec<anchored::AnchoredSlot>,
    anchor_links: Vec<anchored::AnchorLink>,
    anchor_clips: Vec<u32>,
    anchored_order: Vec<u32>,
    auto_boxes: Vec<AutoBox>,
    snap_points: Vec<SnapPoint>,
    matrices: Vec<Transform3D<f32>>,
}

impl FrameBuffers {
    #[cfg(test)]
    pub(crate) fn capacities(&self) -> [usize; 8] {
        [
            self.items.capacity(),
            self.clips.capacity(),
            self.layers.capacity(),
            self.slots.capacity(),
            self.animations.capacity(),
            self.auto_boxes.capacity(),
            self.snap_points.capacity(),
            self.matrices.capacity(),
        ]
    }
}

impl PaintOrder {
    pub(crate) fn into_buffers(mut self) -> FrameBuffers {
        self.items.clear();
        self.clips.clear();
        self.layers.clear();
        self.slots.clear();
        self.slot_index.clear();
        self.animations.clear();
        self.stickies.clear();
        self.spaces.clear();
        self.anchored.clear();
        self.anchor_links.clear();
        self.anchor_clips.clear();
        self.anchored_order.clear();
        self.auto_boxes.clear();
        self.snap_points.clear();
        self.matrices.clear();
        FrameBuffers {
            items: self.items,
            clips: self.clips,
            layers: self.layers,
            slots: self.slots,
            slot_index: self.slot_index,
            animations: self.animations,
            stickies: self.stickies,
            spaces: self.spaces,
            anchored: self.anchored,
            anchor_links: self.anchor_links,
            anchor_clips: self.anchor_clips,
            anchored_order: self.anchored_order,
            auto_boxes: self.auto_boxes,
            snap_points: self.snap_points,
            matrices: self.matrices,
        }
    }

    pub(crate) const fn empty() -> Self {
        Self {
            items: Vec::new(),
            clips: Vec::new(),
            layers: Vec::new(),
            slots: Vec::new(),
            slot_index: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            animations: Vec::new(),
            stickies: Vec::new(),
            spaces: Vec::new(),
            anchored: Vec::new(),
            anchor_links: Vec::new(),
            anchor_clips: Vec::new(),
            anchored_order: Vec::new(),
            auto_boxes: Vec::new(),
            snap_points: Vec::new(),
            matrices: Vec::new(),
            initial_targets: Vec::new(),
            auto_scroll_requests: Vec::new(),
            inert_floor: None,
            commit_id: 0,
        }
    }

    #[cfg(test)]
    fn capacities(&self) -> [usize; 8] {
        [
            self.items.capacity(),
            self.clips.capacity(),
            self.layers.capacity(),
            self.slots.capacity(),
            self.animations.capacity(),
            self.auto_boxes.capacity(),
            self.snap_points.capacity(),
            self.matrices.capacity(),
        ]
    }

    #[must_use]
    pub(crate) fn matrix(&self, transform: ItemTransform) -> Transform3D<f32> {
        transform.resolve(&self.matrices)
    }

    #[must_use]
    pub(crate) fn items(&self) -> &[PaintItem] {
        &self.items
    }

    #[must_use]
    pub(crate) fn hit_testable_items(&self) -> &[PaintItem] {
        &self.items[self.inert_floor.unwrap_or(0)..]
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) const fn inert_floor(&self) -> Option<usize> {
        self.inert_floor
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
    pub(crate) fn slot_of(&self, node: NodeId) -> Option<u32> {
        self.slot_index.get(&node).copied()
    }

    #[must_use]
    pub(crate) fn snap_points(&self) -> &[SnapPoint] {
        &self.snap_points
    }

    #[must_use]
    pub(crate) fn snap_axes(
        &self,
        slot: &ScrollSlot,
    ) -> (Option<SnapAxis<'_>>, Option<SnapAxis<'_>>) {
        let (period_x, period_y) = slot.wrap_period();
        (
            slot.snap
                .x
                .as_ref()
                .map(|axis| axis.axis(&self.snap_points, period_x)),
            slot.snap
                .y
                .as_ref()
                .map(|axis| axis.axis(&self.snap_points, period_y)),
        )
    }

    #[must_use]
    pub(crate) fn initial_targets(&self) -> &[InitialTarget] {
        &self.initial_targets
    }

    #[must_use]
    pub(crate) fn auto_scroll_requests(&self) -> &[(NodeId, AutoScrollRequest)] {
        &self.auto_scroll_requests
    }

    #[must_use]
    pub(crate) fn animations(&self) -> &[AnimationSlot] {
        &self.animations
    }

    #[must_use]
    pub(crate) fn spaces(&self) -> &[Space] {
        &self.spaces
    }

    #[must_use]
    pub(crate) fn auto_boxes(&self) -> &[AutoBox] {
        &self.auto_boxes
    }

    pub(crate) fn sample_animations(
        &self,
        now: Option<f64>,
        offset_of: &dyn Fn(&ScrollSlot) -> Option<euclid::default::Vector2D<f32>>,
    ) -> AnimationSamples {
        let offsets = self.scroll_offsets(offset_of);
        let mut values = AnimationValueMap::default();
        (0_u32..)
            .zip(&self.animations)
            .map(|(index, slot)| (index, slot.sample(now, &offsets, &mut values)))
            .collect()
    }

    pub(crate) fn sample_composed_animations(
        &self,
        composed: &[u32],
        now: Option<f64>,
        offset_of: &dyn Fn(&ScrollSlot) -> Option<euclid::default::Vector2D<f32>>,
    ) -> AnimationSamples {
        let offsets = self.scroll_offsets(offset_of);
        let mut values = AnimationValueMap::default();
        composed
            .iter()
            .map(|&index| {
                (
                    index,
                    self.animations[index as usize].sample(now, &offsets, &mut values),
                )
            })
            .collect()
    }

    pub(crate) fn committed_animations(composed: &[u32]) -> AnimationSamples {
        composed
            .iter()
            .map(|&index| {
                (
                    index,
                    AnimationSample {
                        delta: crate::kurbo::Affine::IDENTITY,
                        alpha: None,
                    },
                )
            })
            .collect()
    }

    fn scroll_offsets<'a>(
        &'a self,
        offset_of: &'a dyn Fn(&ScrollSlot) -> Option<euclid::default::Vector2D<f32>>,
    ) -> ScrollOffsets<'a> {
        ScrollOffsets {
            slots: &self.slots,
            offset_of,
        }
    }

    fn bind_scroll_timelines(&mut self, painted: &mut FxHashSet<NodeId>) {
        painted.clear();
        for slot in &mut self.animations {
            for (_, timeline) in &mut slot.curve.animations {
                if let curves::Timeline::Scroll(scroll) = timeline {
                    scroll.slot = self.slot_index.get(&scroll.source).copied();
                    if scroll.slot.is_some() {
                        painted.insert(slot.node);
                    }
                }
            }
        }
    }

    pub(crate) fn earliest_expiry(&self) -> Option<f64> {
        self.animations
            .iter()
            .filter_map(|slot| slot.curve.expires_at)
            .reduce(f64::min)
    }

    pub(crate) fn mark_composed_spaces(
        &self,
        program: &[crate::paint::compose::ComposeOp],
        groups: &[crate::paint::compose::FilterGroup],
        marks: &mut ComposedMarks,
        composed: &mut ComposedSlots,
    ) {
        let spaces = &mut marks.spaces;
        spaces.clear();
        spaces.resize(self.spaces.len(), false);
        for op in program {
            if let Some(Some(space)) = op.space(groups) {
                spaces[space as usize] = true;
            }
        }
        let stickies = &mut marks.stickies;
        stickies.clear();
        stickies.resize(self.stickies.len(), false);
        self.mark_anchored_inputs(spaces, stickies);
        composed.clear();
        for index in (0..spaces.len()).rev() {
            if !spaces[index] {
                continue;
            }
            let node = self.spaces[index];
            if let Some(parent) = node.parent {
                spaces[parent as usize] = true;
            }
            match node.kind {
                SpaceKind::Scroll(_)
                | SpaceKind::Anchored(_)
                | SpaceKind::AnchoredVisibility(_) => {}
                SpaceKind::Sticky(slot) => stickies[slot as usize] = true,
                SpaceKind::Animation(slot) => composed.animations.push(slot),
            }
        }
        composed.animations.reverse();
        for &slot in &composed.animations {
            let curve = &self.animations[slot as usize].curve;
            composed.clock |= curve.reads_clock();
            composed.scroll |= curve.reads_scroll();
        }
        for index in (0..stickies.len()).rev() {
            if !stickies[index] {
                continue;
            }
            let slot = &self.stickies[index];
            for input in [slot.parent, slot.scroll_sticky[0], slot.scroll_sticky[1]]
                .into_iter()
                .flatten()
            {
                debug_assert!(
                    (input as usize) < index,
                    "a sticky box solves against earlier boxes"
                );
                stickies[input as usize] = true;
            }
        }
        composed.stickies.extend(
            (0_u32..)
                .zip(stickies.iter())
                .filter(|(_, marked)| **marked)
                .map(|(index, _)| index),
        );
    }

    #[must_use]
    pub(crate) const fn commit_id(&self) -> u64 {
        self.commit_id
    }

    pub(crate) fn space_samples<'a>(
        &'a self,
        animations: &'a AnimationSamples,
        stickies: &'a StickySamples,
        anchored: &'a anchored::AnchoredSamples,
        ratio: f32,
        offset_of: &'a dyn Fn(&ScrollSlot) -> Option<euclid::default::Vector2D<f32>>,
    ) -> SpaceSamples<'a> {
        SpaceSamples {
            spaces: &self.spaces,
            slots: &self.slots,
            animations,
            stickies,
            anchored,
            ratio,
            offset_of,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaintItemKind {
    ElementBox,
    TextRun { element: NodeId },
    Backdrop { element: NodeId },
    InlineFragment { paragraph: NodeId },
    Svg { root: NodeId, index: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ItemTransform {
    Translate(Vector2D<f32>),
    Matrix(u32),
}

impl ItemTransform {
    pub(crate) fn resolve(self, matrices: &[Transform3D<f32>]) -> Transform3D<f32> {
        match self {
            Self::Translate(offset) => Transform3D::translation(offset.x, offset.y, 0.0),
            Self::Matrix(index) => matrices[index as usize],
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PaintItem {
    pub(crate) node: NodeId,
    pub(crate) kind: PaintItemKind,
    pub(crate) transform: ItemTransform,
    pub(crate) clip: Option<usize>,
    pub(crate) size: Size2D<f32>,
    pub(crate) radii: CornerRadii,
    pub(crate) hit_testable: bool,
    pub(crate) slot: Option<u32>,
    pub(crate) space: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct RenderLayer {
    pub(crate) parent: Option<usize>,
    pub(crate) node: NodeId,
    pub(crate) transform: ItemTransform,
    pub(crate) size: Size2D<f32>,
    pub(crate) radii: CornerRadii,
    pub(crate) space: Option<u32>,
    pub(crate) clip: Option<usize>,
    pub(crate) items: std::ops::Range<usize>,
    pub(crate) kind: LayerKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayerKind {
    Element,
    Svg { root: NodeId, index: u32 },
}

#[derive(Debug, Clone)]
pub(crate) struct ClipNode {
    pub(crate) parent: Option<usize>,
    #[cfg(test)]
    pub(crate) node: NodeId,
    pub(crate) transform: ItemTransform,
    pub(crate) rect: Rect<f32>,
    pub(crate) radii: CornerRadii,
    pub(crate) space: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct AutoBox {
    pub(crate) node: NodeId,
    pub(crate) transform: ItemTransform,
    pub(crate) size: Size2D<f32>,
    pub(crate) clip: Option<usize>,
    pub(crate) space: Option<u32>,
    pub(crate) layer: Option<usize>,
}

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
    #[cfg(test)]
    pub(crate) fn build_paint_order(&mut self) -> PaintOrder {
        self.layout();
        let _ = self.next_commit_id();
        self.finish_layout();
        self.build_frame()
    }

    fn build_frame(&mut self) -> PaintOrder {
        self.record_svg_roots();
        let (scratch, buffers) = {
            let painter = self.painter.get_mut();
            (painter.take_build_scratch(), painter.take_spare_buffers())
        };
        let (mut frame, scratch) = build::build(self, scratch, buffers);
        self.painter.get_mut().restore_build_scratch(scratch);
        frame.bind_scroll_timelines(&mut self.animations_mut().timelines.painted);
        frame
    }

    fn finish_layout(&mut self) {
        if self.layout_owed() {
            self.layout();
        }
    }

    fn build_frame_with_relevance(&mut self) -> PaintOrder {
        let mut scratch = self.painter.get_mut().take_relevance_scratch();
        self.finish_layout();
        let mut frame = self.build_frame();
        let resnapped = self.resnap(&frame);
        if self.scroll_to_initial_targets(&frame) || resnapped {
            let stale = std::mem::replace(&mut frame, PaintOrder::empty());
            self.painter.get_mut().restore_spare_buffers(stale);
            self.layout();
            frame = self.build_frame();
            self.clear_visual_dirty();
        }
        let mut builds = 1;
        let mut owed = false;
        while self.determine_relevance(&mut scratch, &frame) {
            let stale = std::mem::replace(&mut frame, PaintOrder::empty());
            self.painter.get_mut().restore_spare_buffers(stale);
            self.layout();
            frame = self.build_frame();
            self.clear_visual_dirty();
            builds += 1;
            if builds == relevance::RELEVANCE_PASSES {
                owed = self.relevance_pending(&mut scratch, &frame);
                break;
            }
        }
        self.settle_relevance();
        self.arenas_mut().set_relevance_owed(owed);
        self.painter.get_mut().restore_relevance_scratch(scratch);
        frame
    }

    pub fn render(&mut self) -> bool {
        self.redetermine_scrolled_fallbacks();
        if !self.needs_render() {
            return false;
        }
        let (bound, elements) = (self.arenas().slot_bound(), self.arenas().live_elements());
        self.layout_state_mut().ensure_covers(bound, elements);
        self.commit_timer.open();
        self.layout();
        if self.forget_fallback_sensitive_options() {
            self.layout();
        }
        self.record_last_successful_options();
        let commit_id = self.next_commit_id();
        let frame_build_start = self.commit_timer.close();
        let frame = self.build_frame_with_relevance();
        self.retain_scroll_requests(|node| frame.slot_of(node).is_some());
        let (animations_active, needs_main_ticks) = self.refresh_animation_activity(Some(&frame));
        let viewport = self.viewport_size();
        let device_pixel_ratio = self.device_pixel_ratio();
        let needs_main_ticks = needs_main_ticks || self.arenas().relevance_owed();
        self.painter.borrow_mut().paint(
            self,
            frame,
            animations_active,
            needs_main_ticks,
            viewport,
            device_pixel_ratio,
        );
        if let Some(start) = frame_build_start {
            self.commit_timer.finish(commit_id, start);
        }
        self.note_intersections_stale();
        true
    }

    pub fn commit(&mut self) -> Arc<CommittedFrame> {
        self.render();
        self.committed_frame()
            .expect("render always leaves a committed frame retained")
    }
}

pub struct SceneRef {
    frame: Arc<CommittedFrame>,
    composed: Option<Box<DisplayList>>,
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
    type Target = DisplayList;

    fn deref(&self) -> &DisplayList {
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
    #[must_use]
    pub fn committed_frame(&self) -> Option<Arc<CommittedFrame>> {
        self.painter.borrow().frame().cloned()
    }

    #[must_use]
    pub fn scene<P: FrameImages<PlatformFrame = std::convert::Infallible> + ?Sized>(
        &self,
        pixels: &P,
    ) -> SceneRef {
        let frame = self
            .committed_frame()
            .expect("Document::scene reads the committed frame: render first");
        let composed = frame.scene().is_none().then(|| {
            let (mut images, mut sources) = (Vec::new(), Vec::new());
            let _wanted = frame.resolve_images(pixels, &mut images, &mut sources);
            let mut scene = DisplayList::new();
            frame.compose_into(&mut scene, &images, &[], &[], &|_| None, None);
            Box::new(scene)
        });
        SceneRef { frame, composed }
    }

    #[must_use]
    pub fn elements_from_point(&self, point: Point2D<f32>) -> Vec<NodeId> {
        let offsets = |slot: &ScrollSlot| Some(self.scroll_offset(slot.node));
        let ratio = self.device_pixel_ratio();
        self.with_rendered_frame(|frame| frame.elements_at(self, point, &offsets, ratio))
            .unwrap_or_default()
    }

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
    #[must_use]
    pub fn needs_render(&self) -> bool {
        self.visual_dirty()
            || self.painter.borrow().frame().map(|frame| frame.commit_id())
                != Some(self.commit_id())
    }

    #[cfg(test)]
    pub(crate) fn paint_storage_capacities(&self) -> Vec<usize> {
        let painter = self.painter.borrow();
        let mut out = painter
            .frame()
            .map_or([0; 8], |frame| frame.order.capacities())
            .to_vec();
        let (spare, scratch) = painter.storage_capacities();
        out.extend_from_slice(&spare);
        out.extend(scratch);
        out
    }

    pub(crate) fn images(&self) -> &crate::render::image::ImageRegistry {
        &self.images
    }

    pub fn take_wanted_images(&mut self) -> Vec<Arc<str>> {
        self.images.take_wanted()
    }

    pub fn take_document_requests(&mut self) -> Vec<DocumentRequest> {
        self.images.take_document_requests()
    }

    pub fn apply_image_events(&mut self, events: &[ImageEvent]) -> Vec<(Arc<str>, ImageStatus)> {
        let mut settled = Vec::new();
        for event in events {
            let applied = self.apply_image_event(event);
            let (Some(status), Some(source)) = (applied, event.source()) else {
                continue;
            };
            settled.push((Arc::clone(source), status));
            self.note_visual_mutation();
        }
        settled
    }

    pub fn resume_relevance(&mut self) {
        if self.arenas_mut().take_relevance_owed() {
            self.note_visual_mutation();
        }
    }

    pub fn note_scroll_windows_stale(&mut self) {
        self.note_visual_mutation();
    }
}
