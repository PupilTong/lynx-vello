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
//!   children, skipped `content-visibility` contents) are exactly the subtrees this module skips.
//!   That is a two-way street for `content-visibility: auto`: the skipping is decided *here*, by
//!   [`relevance`], out of the frame this module built — so the frame a render publishes is always
//!   the one built under the bits the layout it was built on used. Note that the decision reads the
//!   build's [`AutoBox`] records, not its items: relevance is geometric, and a `visibility: hidden`
//!   `auto` element has no item yet still decides whether its contents lay out.
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
//!   fixed/absolute containing-block rule; the backdrop bake itself is not painted yet.
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

mod build;
pub(crate) mod curves;
pub(crate) mod frame;
pub(crate) mod geometry;
mod hit;
mod motion;
#[cfg(test)]
mod oracle_tests;
pub(crate) mod relevance;
pub(crate) mod space;
mod stacking;
pub(crate) mod sticky;
mod sticky_frame;
#[cfg(test)]
mod tests;
mod transform;

use std::sync::Arc;

use euclid::default::{Point2D, Rect, Size2D, Transform3D};

pub(crate) use self::build::BuildScratch;
pub use self::frame::{
    AnimationSlot, CommittedFrame, HitTarget, ScrollSlot, SnapSlot, SnapSlotAxis,
};
pub use self::relevance::ContentVisibilityChange;
pub(crate) use self::space::{Space, SpaceKind, SpaceSamples};
#[cfg(test)]
pub(crate) use self::sticky_frame::StickySample;
pub(crate) use self::sticky_frame::{StickySamples, StickySlot};
use crate::render::image::{ImageEvent, ImageOutcome, ImageRole};
use crate::scroll::SnapPoint;
use crate::scroll::initial_target::InitialTarget;
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
    stickies: Vec<StickySlot>,
    /// The scroll, sticky and animation nodes, in allocation order; see
    /// [`space`].
    spaces: Vec<Space>,
    /// Every `content-visibility: auto` box this build reached, in build
    /// order. See [`AutoBox`].
    auto_boxes: Vec<AutoBox>,
    /// Every scroll slot's snap positions, in slot order; see
    /// [`ScrollSlot::snap`].
    snap_points: Vec<SnapPoint>,
    /// Every `scroll-initial-target: nearest` element the build reached,
    /// with its slot; see `scroll::initial_target`. Rarely non-empty, so
    /// not a recycled buffer.
    initial_targets: Vec<InitialTarget>,
    commit_id: u64,
}

/// One instant's samples of a frame's slots, ascending by slot: every slot
/// for a hit test, which answers over items no program encoded, and only the
/// program's own for a compose, so a composed frame costs what it encoded
/// rather than what the page animates. Inline for the few a frame composes.
#[derive(Debug, Clone)]
pub(crate) struct SlotSamples<S>(smallvec::SmallVec<[(u32, S); 4]>);

impl<S> Default for SlotSamples<S> {
    fn default() -> Self {
        Self(smallvec::SmallVec::new())
    }
}

impl<S> SlotSamples<S> {
    /// Slot `slot`'s sample. A compose reads only the slots its program
    /// names, which are exactly the ones it sampled.
    pub(crate) fn get(&self, slot: u32) -> &S {
        let entries = &self.0;
        // A whole-frame table holds slot `i` at `i`.
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

/// The sampled values of a frame's animation slots.
pub(crate) type AnimationSamples = SlotSamples<AnimationSample>;

/// The painter's scratch for [`PaintOrder::mark_composed_spaces`]: one flag
/// per space node and one per sticky slot, reused across commits.
#[derive(Debug, Default)]
pub(crate) struct ComposedMarks {
    spaces: Vec<bool>,
    stickies: Vec<bool>,
}

/// The slots one compose program reads, derived from the program by
/// [`PaintOrder::mark_composed_spaces`] and carried beside it, so a
/// composition samples only what it draws.
#[derive(Debug, Default)]
pub(crate) struct ComposedSlots {
    /// The animation slots on the path of a space some op names, ascending.
    pub(crate) animations: Vec<u32>,
    /// The sticky slots on those paths plus the boxes they solve against,
    /// ascending.
    pub(crate) stickies: Vec<u32>,
}

impl ComposedSlots {
    /// Empties both lists, keeping their capacity.
    pub(crate) fn clear(&mut self) {
        self.animations.clear();
        self.stickies.clear();
    }
}

/// One animation slot's compose-time values, sampled at one instant: the
/// CSS-px delta from the committed geometry, and the opacity replacing the
/// committed one on the element's effect layer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AnimationSample {
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
    stickies: Vec<StickySlot>,
    spaces: Vec<Space>,
    auto_boxes: Vec<AutoBox>,
    snap_points: Vec<SnapPoint>,
}

impl FrameBuffers {
    #[cfg(test)]
    pub(crate) fn capacities(&self) -> [usize; 7] {
        [
            self.items.capacity(),
            self.clips.capacity(),
            self.layers.capacity(),
            self.slots.capacity(),
            self.animations.capacity(),
            self.auto_boxes.capacity(),
            self.snap_points.capacity(),
        ]
    }
}

impl PaintOrder {
    /// Empties this frame and hands back its storage with capacity intact.
    ///
    /// [`PaintItem`], [`ClipNode`], [`RenderLayer`], [`ScrollSlot`],
    /// [`Space`] and [`AutoBox`] own no heap data, so each clear is a length
    /// write.
    pub(crate) fn into_buffers(mut self) -> FrameBuffers {
        self.items.clear();
        self.clips.clear();
        self.layers.clear();
        self.slots.clear();
        self.animations.clear();
        self.stickies.clear();
        self.spaces.clear();
        self.auto_boxes.clear();
        self.snap_points.clear();
        FrameBuffers {
            items: self.items,
            clips: self.clips,
            layers: self.layers,
            slots: self.slots,
            animations: self.animations,
            stickies: self.stickies,
            spaces: self.spaces,
            auto_boxes: self.auto_boxes,
            snap_points: self.snap_points,
        }
    }

    /// A frame with no storage at all — the placeholder a discarded pass
    /// leaves behind while its buffers go back to the painter. It allocates
    /// nothing and is never built, walked or published.
    pub(crate) const fn empty() -> Self {
        Self {
            items: Vec::new(),
            clips: Vec::new(),
            layers: Vec::new(),
            slots: Vec::new(),
            animations: Vec::new(),
            stickies: Vec::new(),
            spaces: Vec::new(),
            auto_boxes: Vec::new(),
            snap_points: Vec::new(),
            initial_targets: Vec::new(),
            commit_id: 0,
        }
    }

    #[cfg(test)]
    fn capacities(&self) -> [usize; 7] {
        [
            self.items.capacity(),
            self.clips.capacity(),
            self.layers.capacity(),
            self.slots.capacity(),
            self.animations.capacity(),
            self.auto_boxes.capacity(),
            self.snap_points.capacity(),
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

    /// Every slot's snap positions, sliced by [`ScrollSlot::snap`].
    #[must_use]
    pub(crate) fn snap_points(&self) -> &[SnapPoint] {
        &self.snap_points
    }

    /// Every `scroll-initial-target: nearest` element this build reached.
    #[must_use]
    pub(crate) fn initial_targets(&self) -> &[InitialTarget] {
        &self.initial_targets
    }

    #[must_use]
    pub(crate) fn animations(&self) -> &[AnimationSlot] {
        &self.animations
    }

    /// The frame's space tree; see [`space`].
    #[must_use]
    pub(crate) fn spaces(&self) -> &[Space] {
        &self.spaces
    }

    /// Every `content-visibility: auto` box this frame's build reached.
    #[must_use]
    pub(crate) fn auto_boxes(&self) -> &[AutoBox] {
        &self.auto_boxes
    }

    /// Every animation slot's compose values sampled at `now` — the
    /// committed values (identity delta, committed opacity) when `now` is
    /// `None`.
    ///
    /// Every slot, because hit testing answers over every item the frame
    /// carries, including the ones culling left unencoded.
    pub(crate) fn sample_animations(&self, now: Option<f64>) -> AnimationSamples {
        (0_u32..)
            .zip(&self.animations)
            .map(|(index, slot)| (index, slot.sample(now)))
            .collect()
    }

    /// [`Self::sample_animations`] over only `composed`, the slots a
    /// program composes; see [`Self::mark_composed_spaces`].
    pub(crate) fn sample_composed_animations(
        &self,
        composed: &[u32],
        now: Option<f64>,
    ) -> AnimationSamples {
        composed
            .iter()
            .map(|&index| (index, self.animations[index as usize].sample(now)))
            .collect()
    }

    /// The earliest instant an exported curve leaves its domain at, over
    /// every slot; `None` when nothing exported ever ends.
    pub(crate) fn earliest_expiry(&self) -> Option<f64> {
        self.animations
            .iter()
            .filter_map(|slot| slot.curve.expires_at)
            .reduce(f64::min)
    }

    /// Writes into `composed` the animation and sticky slots `program`
    /// composes: those on the path of a space some op names, plus the sticky
    /// boxes their constraints solve against. Composition samples only these,
    /// which keeps a composed frame screen-bounded however much of the page
    /// animates.
    ///
    /// Hit testing reads every slot instead: it answers over items this
    /// commit may never have encoded.
    pub(crate) fn mark_composed_spaces(
        &self,
        program: &[crate::paint::compose::ComposeOp],
        groups: &[crate::FilterGroup],
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
        composed.clear();
        // A node is pushed after its parent, so one reverse pass closes the
        // set under `Space::parent`.
        for index in (0..spaces.len()).rev() {
            if !spaces[index] {
                continue;
            }
            let node = self.spaces[index];
            if let Some(parent) = node.parent {
                spaces[parent as usize] = true;
            }
            match node.kind {
                SpaceKind::Scroll(_) => {}
                SpaceKind::Sticky(slot) => stickies[slot as usize] = true,
                SpaceKind::Animation(slot) => composed.animations.push(slot),
            }
        }
        // Space order is slot order within each kind.
        composed.animations.reverse();
        // A sticky box solves against its parent box's cumulative shift and
        // the one its scrollport shares, both allocated before it.
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

    /// This frame's space inputs at one instant, over the tables they index.
    pub(crate) fn space_samples<'a>(
        &'a self,
        animations: &'a AnimationSamples,
        stickies: &'a StickySamples,
        ratio: f32,
        offset_of: &'a dyn Fn(&ScrollSlot) -> Option<euclid::default::Vector2D<f32>>,
    ) -> SpaceSamples<'a> {
        SpaceSamples {
            spaces: &self.spaces,
            slots: &self.slots,
            animations,
            stickies,
            ratio,
            offset_of,
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
    /// slot (the box is a scroll target), while what moves the item is
    /// [`Self::space`], which for a container's own box stops outside its own
    /// scroll node.
    pub(crate) slot: Option<u32>,
    /// The space this item composes in: the element's box space for its own
    /// box, its content space for a text run. See [`space`].
    pub(crate) space: Option<u32>,
}

/// A stacking context rendered as a composited group.
///
/// Only contexts with group effects (`opacity`, `filter`, `backdrop-filter`,
/// `mix-blend-mode`, `clip-path`, `mask-image`, `isolation`) get one. A plain transform or
/// `z-index` context has no [`RenderLayer`] at all, so this table is not an
/// index of stacking contexts and cannot be used as one.
#[derive(Debug, Clone)]
pub(crate) struct RenderLayer {
    pub(crate) parent: Option<usize>,
    pub(crate) node: NodeId,
    pub(crate) transform: Transform3D<f32>,
    pub(crate) size: Size2D<f32>,
    pub(crate) radii: CornerRadii,
    /// The root element's box space: the group moves with the root's own
    /// sticky and animation nodes and the scrollers *around* it, never with
    /// the root's own content.
    pub(crate) space: Option<u32>,
    /// The clip enclosing the root element, which its own box item carries.
    pub(crate) clip: Option<usize>,
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
    /// The establishing element's box space: the clip moves with the
    /// element's own sticky and animation nodes, never with its own scroll
    /// node — a scroller's clip does not move with its own content.
    pub(crate) space: Option<u32>,
}

/// One `content-visibility: auto` box the build reached, with exactly the
/// geometry the relevance test asks about.
///
/// **Not an item.** A `visibility: hidden` element paints nothing and so has
/// no [`PaintItem`], but relevance is a geometric fact about its border box:
/// it decides whether the element's *contents* — which may be
/// `visibility: visible` and must then lay out and paint — are skipped. Tying
/// the record to the item list would leave such an element undetermined and
/// skipping forever. So the builder records this the moment it reaches the
/// element, before it decides whether to emit an item at all, and the
/// relevance pass reads the frame through it: one pass over the `auto` boxes
/// rather than one over every item, and a page with none pays one `is_empty`
/// test per render.
///
/// `space` is the element's box space, the one its own box item composes in.
#[derive(Debug, Clone)]
pub(crate) struct AutoBox {
    pub(crate) node: NodeId,
    pub(crate) transform: Transform3D<f32>,
    pub(crate) size: Size2D<f32>,
    pub(crate) clip: Option<usize>,
    pub(crate) space: Option<u32>,
    /// The innermost enclosing group layer, including one this element opened
    /// for itself: its blur carries the element's contents' ink outward, so
    /// the region admitted for them grows with it.
    pub(crate) layer: Option<usize>,
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
    /// One frame built out of band, claiming a commit id of its own — the
    /// shape the tests that inspect or deliberately strand a paint order
    /// need. The production path is [`Self::render`], which claims the id
    /// once and may build more than one frame under it.
    #[cfg(test)]
    pub(crate) fn build_paint_order(&mut self) -> PaintOrder {
        self.layout();
        // Claimed after the layout and before the build, so the built frame
        // carries it; a panicking build or walk leaves the previous frame
        // retained with its older id, so `needs_render` still reports stale.
        let _ = self.next_commit_id();
        self.build_frame()
    }

    /// One pass: style, layout, paint order, under the commit id already
    /// claimed.
    ///
    /// Split out of [`Self::build_paint_order`] because a render may take
    /// more than one — a `content-visibility: auto` relevance flip voids the
    /// frame it was decided on — while the commit id is claimed once per
    /// render, so every pass builds the *same* commit.
    fn build_frame(&mut self) -> PaintOrder {
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

    /// Builds the frame this render publishes, with
    /// `content-visibility: auto` relevance determined inside it.
    ///
    /// See [`relevance`] for what the determination is and why the reveal
    /// happens in this same commit: a frame is never published with a
    /// relevance flip pending.
    fn build_frame_with_relevance(&mut self) -> PaintOrder {
        let mut scratch = self.painter.get_mut().take_relevance_scratch();
        let mut frame = self.build_frame();
        if self.scroll_to_initial_targets(&frame) {
            // css-scroll-snap-2's initial scroll position is set from the
            // built frame and must be in the frame this commit publishes,
            // so the frame it was decided on is void — same commit, as
            // below.
            let stale = std::mem::replace(&mut frame, PaintOrder::empty());
            self.painter.get_mut().restore_spare_buffers(stale);
            frame = self.build_frame();
            self.clear_visual_dirty();
        }
        let mut pass = 1;
        while pass < relevance::RELEVANCE_PASSES && self.determine_relevance(&mut scratch, &frame) {
            // The frame the flips were decided on is void; its storage is
            // not, so it goes back to the painter for the pass that replaces
            // it.
            let stale = std::mem::replace(&mut frame, PaintOrder::empty());
            self.painter.get_mut().restore_spare_buffers(stale);
            frame = self.build_frame();
            // The flips invalidated layout, which dirtied the frame they
            // were decided on. This render owns that dirt and the rebuild
            // just above is its answer, so it is spent — no second commit id
            // is claimed, because this is still the same commit.
            self.clear_visual_dirty();
            pass += 1;
        }
        self.settle_relevance();
        self.painter.get_mut().restore_relevance_scratch(scratch);
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
        // Style and layout run before the claim: the flush's damage harvest
        // invalidates layout, which notes a visual mutation, so an id
        // claimed ahead of it would be stale the instant it was claimed.
        self.layout();
        let _ = self.next_commit_id();
        let frame = self.build_frame_with_relevance();
        // Asked again now the frame is final: the relevance pass inside the
        // build reveals and skips subtrees with no style change to notice it,
        // and css-contain-2 §4 freezes the animations inside a skipped one.
        // So the flag this frame carries is decided after the last pass, and
        // a reveal starts its animations in the very commit that revealed
        // them.
        let animations_active = self.refresh_animation_activity();
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
    pub fn scene<P: FrameImages + ?Sized>(&self, pixels: &P) -> SceneRef {
        let frame = self
            .committed_frame()
            .expect("Document::scene reads the committed frame: render first");
        let composed = frame.scene().is_none().then(|| {
            let (mut images, mut sources) = (Vec::new(), Vec::new());
            frame.resolve_images(pixels, &mut images, &mut sources);
            let mut scene = crate::vello::Scene::new();
            // No GPU here, so no bake: a `filter: blur()` group replays raw
            // and this scene is the unblurred one (documented fallback).
            frame.compose_into(&mut scene, &images, &[], &|_| None, None);
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
            .map_or([0, 0, 0, 0, 0, 0, 0], |frame| frame.order.capacities())
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
    /// A report that lands on replaced nodes recomputes their natural size in
    /// the same call, so an element resizes in the commit that first draws
    /// what it reports on.
    ///
    /// The [`ImageOutcome`]s are the elements whose *own* source settled, for
    /// the embedder to turn into `load` and `error` events. A placeholder is
    /// nobody's event, and a source reported twice is nobody's either: one URL
    /// has one content, so only the report that moves it is carried.
    pub fn apply_image_events(&mut self, events: &[ImageEvent]) -> Vec<ImageOutcome> {
        let mut outcomes = Vec::new();
        for event in events {
            // `None` is a source reported twice, which one URL with one
            // content makes a no-op: nothing moved, so nothing is dirtied.
            let Some(applied) = self.images.apply(event) else {
                continue;
            };
            for (node, role) in applied.nodes {
                if role == ImageRole::Source {
                    outcomes.push(match applied.loaded {
                        Some((width, height)) => ImageOutcome::Loaded {
                            node,
                            width,
                            height,
                        },
                        None => ImageOutcome::Failed { node },
                    });
                }
                // Which bitmap the node draws may have changed, and with it
                // the natural size that bitmap is fitted against.
                self.refresh_natural_size(node);
            }
            self.note_visual_mutation();
        }
        outcomes
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
