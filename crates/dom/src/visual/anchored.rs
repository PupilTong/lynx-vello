//! css-anchor-position-1's compose half: the default scroll shift (§3.3)
//! and `position-visibility` (§6.6), sampled at the offsets a frame composes
//! at.
//!
//! Layout places an anchor-positioned box against its anchors' *remembered*
//! scroll offsets (`layout::anchors`), so between two anchor recalculation
//! points nothing re-lays it out when a scroller between its default anchor
//! and its containing block moves. What follows the anchor instead is one
//! node of the frame's space tree, [`SpaceKind::Anchored`], that every record
//! of the box — its own box, clip, group layer and content — composes
//! through, outside its own sticky, animation and scroll nodes: "as if
//! affected by a transform (before any other transforms)".
//!
//! The node's translation is the box's **default scroll shift**. For the
//! default anchor, the host remembers a *displacement* — Σ sticky shifts −
//! Σ scroll offsets over the anchor's scroll-adjustment ancestors below the
//! box's containing block ([`RememberedScroll`]). At compose time
//!
//! ```text
//! live  = Σ own(sticky) − Σ offset(scroller) + fixed
//! shift = mask(snap(live) − snap(remembered))
//! delta = L · shift
//! ```
//!
//! with `offset(scroller)` the live offset the compose uses for the same
//! scroll slot, `own(sticky)` the sticky node's own layout-space shift at
//! those offsets, `fixed` the displacement of the scrollers and sticky boxes
//! the frame carries no slot for (their offsets cannot move without a
//! commit), `snap` the device-pixel snap a scroll node applies to its
//! offset, `mask` zeroing the axes the box does not compensate in, and `L`
//! the linear part of the box's containing block's world, which maps the
//! layout-space shift into viewport CSS px. Both displacements are snapped
//! the same way, so a box whose scrollers have not moved since its
//! recalculation point has a shift of exactly zero, at any fractional
//! offset; with one scroller between the anchor and the containing block
//! the box moves exactly as far as the anchor's pixels do (`round` is odd),
//! with more it may differ from them by one device pixel. The main thread's
//! fit test ([`Document::default_scroll_shift`]) snaps the same two terms.
//!
//! `position-visibility` rides the same node: a hidden box's node is the
//! zero map, so the box and everything composed through it — its descendants
//! on its containing-block chain — draw nothing and hit nothing. A
//! descendant that escapes that chain (a `position: fixed` box whose
//! containing block is outside the anchored box) composes through the slot's
//! second node, [`SpaceKind::AnchoredVisibility`], which the build opens on
//! the fixed-containing-block context it hands the box's descendants: the
//! same zero map while the box is hidden, the identity otherwise. Draw and
//! hit testing read records through their spaces alike, so every
//! descendant — escaping or not — hides and stops hitting with the box. Its
//! computed `visibility` is untouched (Blink's model; the spec's
//! `force-hidden` is not a computed value here). Each predicate is evaluated
//! per sample, on the painter per composed frame and on the main thread per
//! hit test, from the frame alone:
//!
//! - `anchor-valid`: the chosen option references the default anchor and there is none — decided at
//!   commit.
//! - `anchor-visible`: the default anchor's computed `visibility` is not `visible`, or its border
//!   box (ink overflow ≈ border box here) mapped through its live space is fully clipped by one of
//!   the clips between it and the box's containing block, each mapped through its own live space. A
//!   hidden anchored box composes through the zero map, so a box anchored to it — or to anything
//!   inside it — finds its anchor degenerate and hides too (the spec's chained case): slots are
//!   sampled in an order that puts every anchored node on an anchor's path first.
//! - `no-overflow`: the margin box shifted by the default scroll shift is not inside the
//!   inset-modified containing block, whose carried edges (`hughie`'s
//!   [`hughie::tree::AnchorOutcome::carried_edges`]: a non-`auto` inset's, a `position-area` line
//!   that is the anchor's own edge) shift with it ([`fits_shifted`]).
//!
//! Nothing here is linear in page content: a frame samples its anchored
//! slots and, for each, the few scroll slots, sticky slots and clips between
//! its anchor and its containing block.

use std::ops::Range;

use euclid::default::{Point2D, Rect, Size2D, Transform3D, Vector2D};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use stylo::values::specified::position::PositionVisibility;

use super::space::{self, SpaceKind, SpaceSamples};
use super::{PaintItemKind, PaintOrder};
use crate::NodeId;
use crate::layout::anchors::{containing_block_generator, fits_shifted};
use crate::paint::compose::snap_offset;
use crate::tree::document::Document;
use crate::vello::kurbo::{self, Affine};

/// One anchor-positioned box's compose-time state: its space node's inputs.
#[derive(Debug, Clone)]
pub(crate) struct AnchoredSlot {
    /// The anchor-positioned box.
    pub(crate) node: NodeId,
    /// The clip chain the box itself composes in: its containing block's.
    clip: Option<usize>,
    /// The containing block's world, linear part: layout-space vectors to
    /// viewport CSS px, as a sticky slot's `parent_transform`.
    linear: [Vector2D<f32>; 2],
    /// §3.3 per physical axis.
    compensates: [bool; 2],
    /// The box's `position-visibility`.
    visibility: PositionVisibility,
    /// The scroll and sticky slots between the default anchor and the
    /// box's containing block, as a range of [`PaintOrder::anchor_links`].
    links: Range<u32>,
    /// The displacement of the ancestors with no slot: `fixed` in the
    /// module formula.
    fixed: Vector2D<f32>,
    /// The default anchor's remembered displacement, unsnapped.
    remembered: Vector2D<f32>,
    /// `anchor-valid` failed, or `anchor-visible` found the anchor not
    /// `visible`: hidden whatever the offsets.
    hidden: bool,
    /// `anchor-visible`'s live test, where one is due.
    probe: Option<AnchorProbe>,
    /// `no-overflow`'s live test, where one is due.
    overflow: Option<OverflowTest>,
    /// Whether [`Self::probe`] reads a space a transform curve moves, or one
    /// an anchored slot that does hides: the hidden state changes with the
    /// timeline reading as well as with the offsets.
    reads_timeline: bool,
}

/// One input of a default scroll shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnchorLink {
    /// A scroll container between the anchor and the containing block.
    Scroll(u32),
    /// A sticky box on that chain, the anchor included.
    Sticky(u32),
}

/// The default anchor's border box and the clips between it and the
/// anchored box's containing block.
#[derive(Debug, Clone)]
struct AnchorProbe {
    transform: Transform3D<f32>,
    size: Size2D<f32>,
    space: Option<u32>,
    /// A range of [`PaintOrder::anchor_clips`].
    clips: Range<u32>,
}

/// What `no-overflow` compares, in the containing block's layout space:
/// the outcome's inset-modified containing block, margin box and carried
/// edges, and whether the box overflowed as laid out, after position
/// fallback — the answer at a zero shift, which also carries `hughie`'s
/// negative-size correction.
#[derive(Debug, Clone, Copy)]
struct OverflowTest {
    outcome: hughie::tree::AnchorOutcome,
}

/// One anchored slot's sampled values.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct AnchoredSample {
    /// The default scroll shift in viewport CSS px.
    pub(crate) delta: Vector2D<f32>,
    /// `position-visibility` hides the box.
    pub(crate) hidden: bool,
}

/// Every anchored slot's sample, by slot.
pub(crate) type AnchoredSamples = SmallVec<[AnchoredSample; 4]>;

impl AnchoredSample {
    /// The node's map: the shift, or the zero map for a hidden box.
    pub(crate) fn affine(self) -> Affine {
        if self.hidden {
            Affine::scale(0.0)
        } else {
            Affine::translate((f64::from(self.delta.x), f64::from(self.delta.y)))
        }
    }

    /// The visibility node's map: the identity, or the zero map for a
    /// hidden box.
    pub(crate) fn visibility_affine(self) -> Affine {
        if self.hidden {
            Affine::scale(0.0)
        } else {
            Affine::IDENTITY
        }
    }
}

/// Whether a box with `outcome` and `visibility` needs an anchored node:
/// it compensates on some axis, or a `position-visibility` predicate can
/// hide it.
fn needs_slot(
    outcome: &hughie::tree::AnchorOutcome,
    visibility: PositionVisibility,
    has_default: bool,
) -> bool {
    let compensates = outcome.compensates.width || outcome.compensates.height;
    (compensates && has_default)
        || (visibility.contains(PositionVisibility::ANCHORS_VALID)
            && outcome.default_anchor_missing)
        || (visibility.contains(PositionVisibility::ANCHORS_VISIBLE) && has_default)
        || visibility.contains(PositionVisibility::NO_OVERFLOW)
}

impl AnchoredSlot {
    /// The slot a build allocates for `node` when the host reported it
    /// anchor-positioned and it needs one; the rest is bound after the walk
    /// ([`PaintOrder::bind_anchored`]).
    pub(crate) fn allocate<T>(
        document: &Document<T>,
        node: NodeId,
        style: &stylo::properties::ComputedValues,
        parent_world: &Transform3D<f32>,
        clip: Option<usize>,
    ) -> Option<Self> {
        let outcome = document.anchor_outcome(node)?;
        let visibility = style.clone_position_visibility();
        let has_default = document
            .remembered_scroll(node)
            .is_some_and(|remembered| remembered.default.is_some());
        if !needs_slot(outcome, visibility, has_default) {
            return None;
        }
        Some(Self {
            node,
            clip,
            linear: [
                Vector2D::new(parent_world.m11, parent_world.m12),
                Vector2D::new(parent_world.m21, parent_world.m22),
            ],
            compensates: [outcome.compensates.width, outcome.compensates.height],
            visibility,
            links: 0..0,
            fixed: Vector2D::zero(),
            remembered: Vector2D::zero(),
            hidden: false,
            probe: None,
            overflow: None,
            reads_timeline: false,
        })
    }

    /// Whether the slot's hidden state depends on the timeline reading, so
    /// a filter bake holding a record it hides samples the instant
    /// ([`super::space::sampled_against`]).
    pub(crate) fn reads_timeline(&self) -> bool {
        self.reads_timeline
    }

    /// Whether some `position-visibility` predicate can hide the box: what
    /// earns its escaping descendants a visibility node.
    pub(crate) fn can_hide(&self) -> bool {
        !self.visibility.is_empty()
    }

    /// `v` with the axes the box does not compensate in zeroed.
    fn mask(&self, v: Vector2D<f32>) -> Vector2D<f32> {
        Vector2D::new(
            if self.compensates[0] { v.x } else { 0.0 },
            if self.compensates[1] { v.y } else { 0.0 },
        )
    }

    fn map(&self, v: Vector2D<f32>) -> Vector2D<f32> {
        self.linear[0] * v.x + self.linear[1] * v.y
    }

    /// The layout-space default scroll shift at `samples`' offsets.
    fn shift(&self, frame: &PaintOrder, samples: &SpaceSamples<'_>) -> Vector2D<f32> {
        let mut live = self.fixed;
        for link in &frame.anchor_links[self.links.start as usize..self.links.end as usize] {
            match *link {
                AnchorLink::Scroll(slot) => {
                    let slot = &samples.slots[slot as usize];
                    live -= (samples.offset_of)(slot).unwrap_or(slot.offset);
                }
                AnchorLink::Sticky(slot) => live += samples.stickies.get(slot).own,
            }
        }
        self.mask(snap_offset(live, samples.ratio) - snap_offset(self.remembered, samples.ratio))
    }

    /// This slot's values at `samples`, whose anchored table holds every
    /// slot this one's anchor rides.
    fn sample(&self, frame: &PaintOrder, samples: &SpaceSamples<'_>) -> AnchoredSample {
        let shift = self.shift(frame, samples);
        let hidden = self.hidden
            || self
                .probe
                .as_ref()
                .is_some_and(|probe| probe.clipped(frame, samples))
            || self.overflow.is_some_and(|test| test.overflows_at(shift));
        AnchoredSample {
            delta: self.map(shift),
            hidden,
        }
    }

    /// The range of this slot's viewport delta over the scroll windows
    /// `windows` and every sticky shift the frame permits, widened by the
    /// one CSS px the two snaps can add.
    fn range(
        &self,
        frame: &PaintOrder,
        windows: &[(Vector2D<f32>, Vector2D<f32>)],
    ) -> (Vector2D<f32>, Vector2D<f32>) {
        let base = self.fixed - self.remembered;
        let mut low = base - Vector2D::splat(1.0);
        let mut high = base + Vector2D::splat(1.0);
        for link in &frame.anchor_links[self.links.start as usize..self.links.end as usize] {
            match *link {
                AnchorLink::Scroll(slot) => {
                    let (window_low, window_high) = windows[slot as usize];
                    low -= window_high;
                    high -= window_low;
                }
                AnchorLink::Sticky(slot) => {
                    let axes = &frame.stickies[slot as usize].axes;
                    let (x0, x1) = axes[0].offset_bounds();
                    let (y0, y1) = axes[1].offset_bounds();
                    low += Vector2D::new(x0, y0);
                    high += Vector2D::new(x1, y1);
                }
            }
        }
        let (low, high) = (self.mask(low), self.mask(high));
        let x0 = self.linear[0] * low.x;
        let x1 = self.linear[0] * high.x;
        let y0 = self.linear[1] * low.y;
        let y1 = self.linear[1] * high.y;
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
}

impl OverflowTest {
    fn overflows_at(self, shift: Vector2D<f32>) -> bool {
        if shift == Vector2D::zero() {
            return self.outcome.overflows;
        }
        !fits_shifted(&self.outcome, shift)
    }
}

impl AnchorProbe {
    /// §6.6: the anchor's border box is "fully clipped by a box which is an
    /// ancestor of anchor but a descendant of abspos's containing block",
    /// or its space is degenerate — a hidden anchored box's, or one inside
    /// it (the chained case).
    fn clipped(&self, frame: &PaintOrder, samples: &SpaceSamples<'_>) -> bool {
        let Some(anchor) = bounds(
            samples.css(self.space),
            &self.transform,
            Rect::from_size(self.size),
        ) else {
            return true;
        };
        frame.anchor_clips[self.clips.start as usize..self.clips.end as usize]
            .iter()
            .any(|&clip| {
                let node = &frame.clips[clip as usize];
                // A degenerate clip space collapses whatever it clips.
                bounds(samples.css(node.space), &node.transform, node.rect)
                    .is_none_or(|clip| fully_clipped(anchor, clip))
            })
    }
}

/// The viewport bounds of `rect` under `transform` and then `map`, or `None`
/// when either is degenerate or projects a corner away.
fn bounds(map: Affine, transform: &Transform3D<f32>, rect: Rect<f32>) -> Option<kurbo::Rect> {
    if map.determinant().abs() < f64::EPSILON {
        return None;
    }
    let corners = [
        rect.origin,
        Point2D::new(rect.max_x(), rect.min_y()),
        Point2D::new(rect.max_x(), rect.max_y()),
        Point2D::new(rect.min_x(), rect.max_y()),
    ];
    let mut out: Option<kurbo::Rect> = None;
    for corner in corners {
        let point = transform.transform_point2d(corner)?;
        let point = map * kurbo::Point::new(f64::from(point.x), f64::from(point.y));
        let rect = kurbo::Rect::from_points(point, point);
        out = Some(out.map_or(rect, |out| out.union(rect)));
    }
    out
}

/// §6.6: "If anchor has non-zero area, it must also have a non-zero
/// intersection area to be considered not fully clipped."
fn fully_clipped(anchor: kurbo::Rect, clip: kurbo::Rect) -> bool {
    let overlap_x = anchor.x1.min(clip.x1) - anchor.x0.max(clip.x0);
    let overlap_y = anchor.y1.min(clip.y1) - anchor.y0.max(clip.y0);
    if anchor.area() > 0.0 {
        overlap_x <= 0.0 || overlap_y <= 0.0
    } else {
        overlap_x < 0.0 || overlap_y < 0.0
    }
}

impl PaintOrder {
    /// Every anchored slot's sample at one instant, in the order that puts
    /// each slot after every anchored node its anchor rides.
    pub(crate) fn sample_anchored(
        &self,
        animations: &super::AnimationSamples,
        stickies: &super::StickySamples,
        ratio: f32,
        offset_of: &dyn Fn(&super::ScrollSlot) -> Option<Vector2D<f32>>,
    ) -> AnchoredSamples {
        let mut table: AnchoredSamples =
            smallvec::smallvec![AnchoredSample::default(); self.anchored.len()];
        for &index in &self.anchored_order {
            let sample = {
                let samples = self.space_samples(animations, stickies, &table, ratio, offset_of);
                self.anchored[index as usize].sample(self, &samples)
            };
            table[index as usize] = sample;
        }
        table
    }

    /// The `(low, high)` range of anchored node `slot`'s delta, in viewport
    /// CSS px, over the scroll windows `windows`.
    pub(crate) fn anchored_slot_range(
        &self,
        slot: u32,
        windows: &[(Vector2D<f32>, Vector2D<f32>)],
    ) -> (Vector2D<f32>, Vector2D<f32>) {
        self.anchored[slot as usize].range(self, windows)
    }

    /// A conservative range for the movement anchored nodes give content in
    /// `content` relative to a frame in `frame`: those on `content`'s path
    /// below the two spaces' common ancestor, less those on `frame`'s.
    pub(crate) fn anchored_range(
        &self,
        windows: &[(Vector2D<f32>, Vector2D<f32>)],
        content: Option<u32>,
        frame: Option<u32>,
    ) -> (Vector2D<f32>, Vector2D<f32>) {
        let mut low = Vector2D::zero();
        let mut high = Vector2D::zero();
        if self.anchored.is_empty() {
            return (low, high);
        }
        let common = space::common_ancestor(&self.spaces, content, frame);
        for node in space::path_below(&self.spaces, content, common) {
            if let SpaceKind::Anchored(slot) = node.kind {
                let (slot_low, slot_high) = self.anchored_slot_range(slot, windows);
                low += slot_low;
                high += slot_high;
            }
        }
        for node in space::path_below(&self.spaces, frame, common) {
            if let SpaceKind::Anchored(slot) = node.kind {
                let (slot_low, slot_high) = self.anchored_slot_range(slot, windows);
                low -= slot_high;
                high -= slot_low;
            }
        }
        (low, high)
    }

    /// Marks, in `spaces` and `stickies`, what sampling the anchored slots
    /// reads besides their own nodes: the anchors' spaces and their clips'
    /// spaces, and the sticky slots of the shifts.
    pub(crate) fn mark_anchored_inputs(&self, spaces: &mut [bool], stickies: &mut [bool]) {
        for slot in &self.anchored {
            for link in &self.anchor_links[slot.links.start as usize..slot.links.end as usize] {
                if let AnchorLink::Sticky(sticky) = *link {
                    stickies[sticky as usize] = true;
                }
            }
            let Some(probe) = &slot.probe else {
                continue;
            };
            let clip_spaces = self.anchor_clips
                [probe.clips.start as usize..probe.clips.end as usize]
                .iter()
                .map(|&clip| self.clips[clip as usize].space);
            for space in std::iter::once(probe.space).chain(clip_spaces).flatten() {
                spaces[space as usize] = true;
            }
        }
    }

    /// Binds every anchored slot the walk allocated, now that every scroll
    /// slot, sticky slot, clip and item exists: the links of its default
    /// scroll shift, its `position-visibility` probes, and the order the
    /// slots are sampled in.
    pub(crate) fn bind_anchored<T>(&mut self, document: &Document<T>) {
        if self.anchored.is_empty() {
            return;
        }
        let sticky_index: FxHashMap<NodeId, u32> = (0_u32..)
            .zip(&self.stickies)
            .map(|(index, slot)| (slot.node, index))
            .collect();
        // Each default anchor's own box item.
        let mut anchors: FxHashMap<NodeId, Option<usize>> = FxHashMap::default();
        for slot in &self.anchored {
            if let Some(default) = document
                .remembered_scroll(slot.node)
                .and_then(|remembered| remembered.default)
            {
                anchors.insert(default, None);
            }
        }
        if !anchors.is_empty() {
            for (index, item) in self.items.iter().enumerate() {
                if item.kind == PaintItemKind::ElementBox
                    && let Some(entry) = anchors.get_mut(&item.node)
                    && entry.is_none()
                {
                    *entry = Some(index);
                }
            }
        }
        for index in 0..self.anchored.len() {
            self.bind_slot(document, index, &sticky_index, &anchors);
        }
        self.order_anchored();
        // In sample order, so a dependency's answer is known first.
        for position in 0..self.anchored_order.len() {
            let index = self.anchored_order[position] as usize;
            self.anchored[index].reads_timeline = self.probe_reads_timeline(index);
        }
    }

    /// The anchored slots, by slot.
    pub(crate) fn anchored(&self) -> &[AnchoredSlot] {
        &self.anchored
    }

    /// Whether slot `index`'s `anchor-visible` probe reads a space a
    /// transform curve moves, or one an anchored slot whose own hidden
    /// state reads the timeline hides.
    fn probe_reads_timeline(&self, index: usize) -> bool {
        let Some(probe) = &self.anchored[index].probe else {
            return false;
        };
        let clip_spaces = self.anchor_clips[probe.clips.start as usize..probe.clips.end as usize]
            .iter()
            .map(|&clip| self.clips[clip as usize].space);
        std::iter::once(probe.space)
            .chain(clip_spaces)
            .any(|space| {
                space::path(&self.spaces, space).any(|kind| match kind {
                    SpaceKind::Animation(slot) => {
                        self.animations[slot as usize].curve.transform.is_some()
                    }
                    SpaceKind::Anchored(slot) | SpaceKind::AnchoredVisibility(slot) => {
                        slot as usize != index && self.anchored[slot as usize].reads_timeline
                    }
                    SpaceKind::Scroll(_) | SpaceKind::Sticky(_) => false,
                })
            })
    }

    fn bind_slot<T>(
        &mut self,
        document: &Document<T>,
        index: usize,
        sticky_index: &FxHashMap<NodeId, u32>,
        anchors: &FxHashMap<NodeId, Option<usize>>,
    ) {
        let node = self.anchored[index].node;
        let visibility = self.anchored[index].visibility;
        let Some(outcome) = document.anchor_outcome(node).copied() else {
            return;
        };
        let remembered = document.remembered_scroll(node);
        let default = remembered.and_then(|remembered| remembered.default);
        let containing_block = document.anchor_containing_block(node);

        let compensating = outcome.compensates.width || outcome.compensates.height;
        let (links, fixed) = self.bind_links(
            document,
            default.filter(|_| compensating),
            containing_block,
            sticky_index,
        );
        let remembered_default = default
            .and_then(|anchor| remembered?.displacement_of(anchor))
            .unwrap_or_default();

        // §6.6 `anchor-valid`.
        let mut hidden = visibility.contains(PositionVisibility::ANCHORS_VALID)
            && outcome.default_anchor_missing;

        // §6.6 `anchor-visible`.
        let mut probe = None;
        if visibility.contains(PositionVisibility::ANCHORS_VISIBLE)
            && let Some(anchor) = default
        {
            let invisible = document
                .get(anchor)
                .and_then(crate::tree::node::Node::layout_computed_style)
                .is_none_or(|style| {
                    style.clone_visibility() != hughie::style::visibility::T::Visible
                });
            if invisible {
                hidden = true;
            } else if let Some(&Some(item)) = anchors.get(&anchor) {
                let item = &self.items[item];
                let (transform, size, space, anchor_clip) =
                    (item.transform, item.size, item.space, item.clip);
                let box_chain: SmallVec<[usize; 8]> =
                    std::iter::successors(self.anchored[index].clip, |&clip| {
                        self.clips[clip].parent
                    })
                    .collect();
                let start = u32::try_from(self.anchor_clips.len()).expect("bounded by the frame");
                let mut clip = anchor_clip;
                while let Some(at) = clip {
                    if box_chain.contains(&at) {
                        break;
                    }
                    self.anchor_clips
                        .push(u32::try_from(at).expect("bounded by the frame"));
                    clip = self.clips[at].parent;
                }
                let end = u32::try_from(self.anchor_clips.len()).expect("bounded by the frame");
                probe = Some(AnchorProbe {
                    transform,
                    size,
                    space,
                    clips: start..end,
                });
            }
            // A visible anchor with no box item of its own — an inline span
            // painted by its paragraph — is never found clipped.
        }

        let overflow = visibility
            .contains(PositionVisibility::NO_OVERFLOW)
            .then_some(OverflowTest { outcome });

        let slot = &mut self.anchored[index];
        slot.links = links;
        slot.fixed = fixed;
        slot.remembered = remembered_default;
        slot.hidden = hidden;
        slot.probe = probe;
        slot.overflow = overflow;
    }

    /// §3.3: appends the links of `anchor`'s scroll-adjustment ancestors
    /// below `containing_block` — the chain `Document::anchor_displacement`
    /// walks — and answers their range with the displacement of those the
    /// frame carries no slot for. No anchor, no links.
    fn bind_links<T>(
        &mut self,
        document: &Document<T>,
        anchor: Option<NodeId>,
        containing_block: Option<NodeId>,
        sticky_index: &FxHashMap<NodeId, u32>,
    ) -> (Range<u32>, Vector2D<f32>) {
        let start = u32::try_from(self.anchor_links.len()).expect("bounded by the frame");
        let mut fixed = Vector2D::zero();
        let mut sampled = Vec::new();
        let mut current = anchor.and_then(|anchor| document.get(anchor));
        while let Some(step) = current {
            if Some(step.id()) == containing_block {
                break;
            }
            if let Some(style) = step.layout_computed_style() {
                if Some(step.id()) != anchor && crate::scroll::is_scroll_container(style) {
                    match self.slot_index.get(&step.id()) {
                        Some(&slot) => self.anchor_links.push(AnchorLink::Scroll(slot)),
                        None => fixed -= document.scroll_offset(step.id()),
                    }
                }
                if style.clone_position() == hughie::style::PositionProperty::Sticky {
                    match sticky_index.get(&step.id()) {
                        Some(&slot) => self.anchor_links.push(AnchorLink::Sticky(slot)),
                        None => {
                            fixed += super::sticky::live_offset(document, step.id(), &mut sampled);
                        }
                    }
                }
            }
            current = containing_block_generator(step);
        }
        let end = u32::try_from(self.anchor_links.len()).expect("bounded by the frame");
        (start..end, fixed)
    }

    /// Orders the slots so each is sampled after every anchored node on its
    /// anchor's path and on its clips' paths.
    fn order_anchored(&mut self) {
        let count = self.anchored.len();
        let mut state = vec![0_u8; count];
        let mut order = std::mem::take(&mut self.anchored_order);
        order.clear();
        for index in 0..count {
            self.visit_anchored(index, &mut state, &mut order);
        }
        self.anchored_order = order;
    }

    fn visit_anchored(&self, index: usize, state: &mut [u8], order: &mut Vec<u32>) {
        // 0 unvisited, 1 on the stack, 2 placed. The acceptability rule
        // leaves no cycle; one would only sample a dependency late.
        if state[index] != 0 {
            return;
        }
        state[index] = 1;
        if let Some(probe) = &self.anchored[index].probe {
            let clip_spaces = self.anchor_clips
                [probe.clips.start as usize..probe.clips.end as usize]
                .iter()
                .map(|&clip| self.clips[clip as usize].space);
            for space in std::iter::once(probe.space).chain(clip_spaces) {
                for kind in space::path(&self.spaces, space) {
                    if let SpaceKind::Anchored(dependency)
                    | SpaceKind::AnchoredVisibility(dependency) = kind
                        && dependency as usize != index
                    {
                        self.visit_anchored(dependency as usize, state, order);
                    }
                }
            }
        }
        state[index] = 2;
        order.push(u32::try_from(index).expect("bounded by the frame"));
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use euclid::default::Vector2D;

    use crate::test_common::Doc;
    use crate::visual::SpaceKind;
    use crate::{NodeId, Point2D};

    const BASE: &str = "
        page { display: flex; flex-direction: column; width: 800px; height: 600px; }
        view { display: flex; flex-direction: column; flex-shrink: 0; }
        .cb { position: relative; width: 400px; height: 400px; }
        .scroller { overflow: scroll; width: 200px; height: 100px; }
        .content { width: 500px; height: 500px; }
        .anchor { anchor-name: --a; width: 40px; height: 30px;
                  margin-top: 50px; margin-left: 50px; }
        .anchored { position: absolute; position-anchor: --a; width: 10px; height: 10px; }";

    /// A 400px containing block holding a 200×100 scroller whose content
    /// carries the anchor at (50, 50), 40×30, and, outside the scroller,
    /// the anchored box styled `anchored`.
    struct Page {
        doc: Doc,
        scroller: NodeId,
        anchor: NodeId,
        anchored: NodeId,
    }

    fn page(anchored: &str) -> Page {
        let mut doc = Doc::with_css(&format!("{BASE}\n.anchored {{ {anchored} }}"));
        let root = doc.root;
        let cb = doc.el(root, "view.cb");
        let scroller = doc.el(cb, "view.scroller");
        let content = doc.el(scroller, "view.content");
        let anchor = doc.el(content, "view.anchor");
        let anchored = doc.el(cb, "view.anchored");
        doc.dom.render();
        Page {
            doc,
            scroller,
            anchor,
            anchored,
        }
    }

    /// [`page`], with the anchored box added only once the scroller sits
    /// at `(x, y)`: its recalculation point is there.
    fn page_scrolled(anchored: &str, x: f32, y: f32) -> Page {
        let mut doc = Doc::with_css(&format!("{BASE}\n.anchored {{ {anchored} }}"));
        let root = doc.root;
        let cb = doc.el(root, "view.cb");
        let scroller = doc.el(cb, "view.scroller");
        let content = doc.el(scroller, "view.content");
        let anchor = doc.el(content, "view.anchor");
        doc.dom.render();
        doc.dom.scroll_to(scroller, Vector2D::new(x, y));
        doc.dom.render();
        let anchored = doc.el(cb, "view.anchored");
        doc.dom.render();
        Page {
            doc,
            scroller,
            anchor,
            anchored,
        }
    }

    impl Page {
        fn hit(&mut self, x: f32, y: f32) -> Option<NodeId> {
            self.doc.dom.render();
            self.doc
                .dom
                .elements_from_point(Point2D::new(x, y))
                .first()
                .copied()
        }

        /// Scrolls without rendering: inside the encode window the retained
        /// frame composes the new offsets.
        fn scroll(&mut self, x: f32, y: f32) {
            self.doc.dom.scroll_to(self.scroller, Vector2D::new(x, y));
        }

        fn commit_id(&self) -> u64 {
            self.doc
                .dom
                .committed_frame()
                .expect("rendered")
                .commit_id()
        }
    }

    /// WPT `anchor-scroll-001`, adapted: a box placed below its anchor by
    /// `position-area` follows the anchor on both axes as the scroller
    /// between them moves, with no commit.
    #[test]
    fn a_box_follows_its_scrolled_anchor_on_both_axes() {
        let mut page = page("position-area: bottom center;");
        // Centered under the anchor: (65, 80), 10×10.
        assert_eq!(page.hit(70.0, 85.0), Some(page.anchored));
        let before = page.commit_id();
        let frame = page.doc.dom.committed_frame().expect("rendered");
        assert!(
            frame
                .order
                .spaces()
                .iter()
                .any(|space| matches!(space.kind, SpaceKind::Anchored(0)))
        );
        drop(frame);

        page.scroll(20.0, 30.0);
        assert_eq!(
            page.hit(50.0, 55.0),
            Some(page.anchored),
            "moved by (−20, −30)"
        );
        assert_ne!(page.hit(70.0, 85.0), Some(page.anchored));
        assert_eq!(page.commit_id(), before, "composed, not committed");
    }

    /// §3.3's compensation is per axis: a box whose only anchor reference is
    /// a vertical `anchor()` compensates vertically and stays put
    /// horizontally.
    #[test]
    fn only_a_compensating_axis_follows() {
        let mut page = page("top: anchor(bottom); left: 150px;");
        let outcome = *page
            .doc
            .dom
            .anchor_outcome(page.anchored)
            .expect("reported");
        assert!(outcome.compensates.height && !outcome.compensates.width);
        assert_eq!(page.hit(155.0, 85.0), Some(page.anchored));
        page.scroll(20.0, 30.0);
        assert_eq!(page.hit(155.0, 55.0), Some(page.anchored), "up by 30 only");
        assert_ne!(page.hit(135.0, 55.0), Some(page.anchored), "not left by 20");
    }

    /// §6.6 `anchor-visible` (the initial value): the box hides once its
    /// anchor scrolls wholly out of the scroller's clip, and not before;
    /// `always` keeps it.
    #[test]
    fn anchor_visible_hides_the_box_once_the_anchor_is_clipped_away() {
        let mut page = page("position-area: bottom center;");
        // The anchor at y −20..10 still shows its last 10px.
        page.scroll(0.0, 70.0);
        assert_eq!(page.hit(70.0, 15.0), Some(page.anchored));
        // At y −35..−5 it is gone: the box at y −5..5 hides.
        page.scroll(0.0, 85.0);
        assert_ne!(page.hit(70.0, 2.0), Some(page.anchored));
        page.scroll(0.0, 0.0);
        assert_eq!(page.hit(70.0, 85.0), Some(page.anchored), "and comes back");

        let mut page =
            super::tests::page("position-area: bottom center; position-visibility: always;");
        page.scroll(0.0, 85.0);
        assert_eq!(page.hit(70.0, 2.0), Some(page.anchored));
    }

    /// `force-hidden` hides the box's descendants too, a `position: fixed`
    /// one whose containing block is outside the box included: it hides and
    /// stops hitting with the box — with and without a group layer on the
    /// box — and does not follow the box's default scroll shift.
    #[test]
    fn an_escaping_fixed_descendant_hides_with_its_anchored_box() {
        for group in ["", "opacity: 0.5;"] {
            let mut page = page(&format!("position-area: bottom center; {group}"));
            page.doc.add_css(
                ".fixed { position: fixed; left: 300px; top: 300px; width: 20px; height: 20px; }",
            );
            let fixed = page.doc.el(page.anchored, "view.fixed");
            assert_eq!(page.hit(310.0, 310.0), Some(fixed), "{group}");
            {
                let frame = page.doc.dom.committed_frame().expect("rendered");
                let order = &frame.order;
                let item = order
                    .items
                    .iter()
                    .find(|item| item.node == fixed)
                    .expect("painted");
                let path: Vec<_> = super::space::path(order.spaces(), item.space).collect();
                assert!(
                    path.contains(&SpaceKind::AnchoredVisibility(0))
                        && !path.contains(&SpaceKind::Anchored(0)),
                    "{group}: {path:?}"
                );
            }
            page.scroll(20.0, 30.0);
            assert_eq!(page.hit(310.0, 310.0), Some(fixed), "{group}: not shifted");
            page.scroll(0.0, 85.0);
            assert_ne!(page.hit(310.0, 310.0), Some(fixed), "{group}: hidden");
            page.scroll(0.0, 0.0);
            assert_eq!(page.hit(310.0, 310.0), Some(fixed), "{group}: back");
        }
    }

    /// An `anchor-visible` probe whose anchor a transform curve moves makes
    /// the slot's hidden state read the timeline, so a filter bake holding
    /// a record the slot hides samples the instant.
    #[test]
    fn a_probe_on_an_animated_anchor_reads_the_timeline() {
        let mut page = page("position-area: bottom center;");
        assert!(
            !page
                .doc
                .dom
                .committed_frame()
                .expect("rendered")
                .order
                .anchored()[0]
                .reads_timeline()
        );
        page.doc.add_css(
            "@keyframes slide { from { transform: translateX(0px); }
                                to { transform: translateX(300px); } }",
        );
        page.doc
            .set_inline(page.anchor, "animation: slide 1s linear infinite");
        page.doc.dom.render();
        page.doc.dom.advance_animations(0.0);
        page.doc.dom.advance_animations(0.1);
        page.doc.dom.render();
        let frame = page.doc.dom.committed_frame().expect("rendered");
        let order = &frame.order;
        assert_eq!(order.animations().len(), 1, "the slide exports");
        let slot = &order.anchored()[0];
        assert!(slot.reads_timeline());
        let anchored = order
            .spaces()
            .iter()
            .position(|space| space.kind == SpaceKind::Anchored(0))
            .and_then(|index| u32::try_from(index).ok());
        assert!(super::space::sampled_against(
            order.spaces(),
            order.animations(),
            order.anchored(),
            anchored,
            None,
        ));
        assert!(!super::space::sampled_against(
            order.spaces(),
            order.animations(),
            &[],
            anchored,
            None,
        ));
    }

    /// A box anchored to a hidden anchored box hides with it.
    #[test]
    fn a_box_anchored_to_a_hidden_box_hides_too() {
        let mut page = page("position-area: bottom center; anchor-name: --b;");
        let cb = page
            .doc
            .dom
            .get(page.anchored)
            .and_then(crate::tree::node::Node::flat_parent_id)
            .expect("in the cb");
        let chained = page.doc.el(cb, "view.chained");
        page.doc.add_css(
            ".chained { position: absolute; position-anchor: --b; position-area: right;
                        width: 10px; height: 10px; }",
        );
        // Right of the box at (65, 80): (75, 80).
        assert_eq!(page.hit(80.0, 85.0), Some(chained));
        page.scroll(0.0, 85.0);
        assert_ne!(page.hit(80.0, 2.0), Some(chained), "its anchor is hidden");
        assert_ne!(page.hit(70.0, 2.0), Some(page.anchored));
    }

    /// §6.6 `no-overflow`: the shifted box leaving its inset-modified
    /// containing block hides it.
    #[test]
    fn no_overflow_hides_a_box_shifted_out_of_its_containing_block() {
        // Above the anchor, bottom edge on the anchor's top, in the
        // containing block's top 50px.
        let mut page = page(
            "bottom: anchor(top); left: anchor(left); height: 40px;
                             position-visibility: no-overflow;",
        );
        assert_eq!(page.hit(55.0, 20.0), Some(page.anchored));
        page.scroll(0.0, 5.0);
        assert_eq!(
            page.hit(55.0, 10.0),
            Some(page.anchored),
            "5px of room left"
        );
        page.scroll(0.0, 11.0);
        assert_ne!(
            page.hit(55.0, 10.0),
            Some(page.anchored),
            "past the top edge"
        );
        let _ = page.anchor;
    }

    /// The two displacements are snapped alike: a box remembered at a
    /// fractional offset has a zero shift at rest, so a `no-overflow` box
    /// flush with its containing block's top edge stays visible there, on
    /// the painter and on the main thread, and hides one pixel further.
    #[test]
    fn a_box_remembered_at_a_fractional_offset_is_unshifted_at_rest() {
        // The anchor's top at 50 − 0.75 = 49.25; the box above it, as tall,
        // flush with the containing block's top.
        let mut page = page_scrolled(
            "bottom: anchor(top); left: anchor(left); height: 49.25px;
             position-visibility: no-overflow;",
            0.0,
            0.75,
        );
        let shift = page
            .doc
            .dom
            .default_scroll_shift(page.anchored, Some(1.0))
            .expect("compensating");
        assert_eq!(shift, Vector2D::zero(), "main: snap(now) − snap(then)");
        let frame = page.doc.dom.committed_frame().expect("rendered");
        let sampled = frame.order.sample_anchored(
            &crate::visual::AnimationSamples::default(),
            &crate::visual::StickySamples::default(),
            1.0,
            &|_| None,
        );
        assert_eq!(sampled[0], super::AnchoredSample::default(), "painter");
        drop(frame);
        assert_eq!(page.hit(55.0, 20.0), Some(page.anchored));
        page.scroll(0.0, 1.75);
        assert_ne!(page.hit(55.0, 20.0), Some(page.anchored), "1px up: out");
    }

    /// Blink's fit test after a scroll: a length inset leaves its side
    /// unconstrained, so a box stretched from its anchor to the containing
    /// block's edge is not re-determined on every adopted offset.
    #[test]
    fn a_stretched_box_is_not_redetermined_on_every_offset() {
        let mut page = page_scrolled(
            "left: anchor(right); right: 0; top: anchor(bottom); height: 10px;
             position-try-fallbacks: flip-block;",
            40.0,
            0.0,
        );
        let outcome = *page
            .doc
            .dom
            .anchor_outcome(page.anchored)
            .expect("reported");
        assert!(outcome.carried_edges.left && outcome.carried_edges.right);
        assert!(outcome.carried_edges.top && !outcome.carried_edges.bottom);
        for x in (0..40_u8).rev() {
            page.scroll(f32::from(x), 0.0);
            assert!(
                !page.doc.dom.redetermine_scrolled_fallbacks(),
                "no re-determination at {x}"
            );
        }
    }

    /// `hughie` reports which `position-area` lines the anchor carries: a
    /// line that is the anchor's edge is carried even where it lands on the
    /// containing block's edge.
    #[test]
    fn a_position_area_line_on_the_containing_block_edge_is_still_carried() {
        // The anchor's bottom (80) at the containing block's top at 80.
        let mut page = page_scrolled(
            "position-area: bottom center; position-visibility: no-overflow;",
            0.0,
            80.0,
        );
        let outcome = *page
            .doc
            .dom
            .anchor_outcome(page.anchored)
            .expect("reported");
        assert!(outcome.carried_edges.top && !outcome.carried_edges.bottom);
        assert_eq!(page.hit(70.0, 5.0), Some(page.anchored));
        page.scroll(0.0, 81.0);
        assert_eq!(page.hit(70.0, 4.0), Some(page.anchored), "1px up, carried");
    }

    /// §6.5 on scroll: a box pushed out of its containing block by its
    /// default scroll shift determines its fallback at the next render, and
    /// determines back once the fallback overflows in turn.
    #[test]
    fn a_scrolled_box_switches_to_its_fallback_and_back() {
        let mut doc = Doc::with_css(
            "page { display: flex; flex-direction: column; width: 800px; height: 600px; }
             view { display: flex; flex-direction: column; flex-shrink: 0; }
             .cb { position: relative; width: 400px; height: 400px; }
             .scroller { overflow: scroll; width: 400px; height: 400px; }
             .content { width: 400px; height: 1000px; }
             .anchor { anchor-name: --a; width: 40px; height: 30px; margin-top: 250px; }
             .anchored { position: absolute; position-anchor: --a; position-area: top;
                         position-try-fallbacks: flip-block; width: 10px; height: 150px; }",
        );
        let root = doc.root;
        let cb = doc.el(root, "view.cb");
        let scroller = doc.el(cb, "view.scroller");
        let content = doc.el(scroller, "view.content");
        doc.el(content, "view.anchor");
        let anchored = doc.el(cb, "view.anchored");
        doc.dom.render();
        let chosen = |doc: &Doc| doc.dom.anchor_outcome(anchored).expect("reported").chosen;
        assert_eq!(chosen(&doc), 0, "above the anchor: 100..250");

        doc.dom.scroll_to(scroller, Vector2D::new(0.0, 100.0));
        assert!(
            !doc.dom.redetermine_scrolled_fallbacks(),
            "still fits: 0..150"
        );
        doc.dom.scroll_to(scroller, Vector2D::new(0.0, 200.0));
        assert!(doc.dom.render(), "a re-determination commits");
        assert_eq!(chosen(&doc), 1, "below the anchor: 80..230");
        assert!(!doc.dom.render(), "once");

        doc.dom.scroll_to(scroller, Vector2D::new(0.0, 0.0));
        assert!(doc.dom.render());
        assert_eq!(chosen(&doc), 0, "280..430 overflows, the base fits again");
    }
}
