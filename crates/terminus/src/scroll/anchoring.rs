//! css-scroll-anchoring-1: a layout change above what a scroll container
//! shows does not move what it shows.
//!
//! Before a layout, every scroll container that is scrolled away from its
//! block start, and whose `overflow-anchor` is `auto`, has an **anchor
//! node** (§2.1): the first box in tree order that the scrollport shows, or,
//! where that box is only partly shown, the deepest such box inside it.
//! After the layout, the container's block offset moves by however far the
//! anchor's block-start edge moved (§2.2), so the anchor stays where it was
//! on screen. This is what keeps a `<list>` still while its cells trade their
//! `contain-intrinsic-size` placeholder for their real size
//! ([`crate::visual::relevance`]).
//!
//! # What follows Blink
//!
//! The draft leaves several choices to the engine, and these follow Blink
//! (`scroll_anchor.cc`), which is what web-core's lists get from a browser:
//!
//! - Only the block axis is adjusted — `y`, as this engine lays out horizontal-tb only.
//! - The anchor is selected at the first layout that needs one and kept until a scroll the
//!   anchoring did not make — a painter post, a script's scroll, a snap — clears it
//!   ([`Document::clear_scroll_anchor`]); it is also cleared when it loses its box or leaves the
//!   container, and when a suppression trigger fires.
//! - An inline box is never the anchor; the walk goes through it. A box with no area, a box the
//!   visible rect misses, a `fixed` or `sticky` box, an `absolute` box whose containing block is
//!   outside the container, and an `overflow-anchor: none` subtree are skipped. A nested scroll
//!   container or a Lynx paragraph may be the anchor, but the walk does not enter it.
//! - The visible rect is the scrollport inset by `scroll-padding`; a box's bounds are its border
//!   box.
//! - §2.1 lets an engine decline to anchor a container, and two are left alone: one that snaps on
//!   the block axis, which css-scroll-snap-1 §4.1.3 re-snapping keeps on its area
//!   ([`crate::scroll::snap`]), and one with a pending programmatic scroll, whose target owns its
//!   position until the painter has carried it out ([`crate::scroll::ScrollRequest`]).
//! - **Not Blink, and not the draft:** a `content-visibility: auto` box that skips its contents is
//!   no candidate, nor is anything under it, and a kept anchor that has come to skip is dropped.
//!   Its size is its `contain-intrinsic-size` or its last remembered size, never a layout of what
//!   it holds, and it shows nothing, so keeping its block-start in place keeps nothing the user
//!   saw. Anchored to one, a page that builds content where its placeholders turn relevant chases
//!   the next placeholder down for as long as it builds: Chrome does, Firefox and Safari select the
//!   same anchors, and css-scroll-anchoring-1 §2.1's excluded subtrees do not include it. The
//!   choice puts filling the screen after a long scroll ahead of where that screen lands. Recorded
//!   in `docs/tracking/deviations.md`.
//! - An anchor that is a `content-visibility: auto` box whose size this layout changed was still
//!   standing at its placeholder size when it was chosen. Keeping its block-start edge in place is
//!   then no promise about its contents: if the adjustment would leave it wholly above the
//!   scrollport, the adjustment stops where its block-end edge meets the scrollport's top, and the
//!   anchor is cleared so the next layout chooses one that has been laid out.
//!
//! # Suppression
//!
//! §2.2.2's triggers on the path from the anchor up to the container: a
//! computed change of `position`, an inset, a margin, a padding, a size or
//! `transform` since the anchor's position was read clears the anchor
//! instead of adjusting. The trigger "an element in the container became or
//! stopped being absolutely positioned" is not implemented; the path
//! triggers cover the anchor's own ancestry.
//!
//! Nor is §2.2's last step: an adjustment fires no `scroll` event. The
//! runtime fires `scroll` from the painter's posts, and a post that only
//! catches up with an adjustment moves nothing the document holds.
//!
//! # The painter
//!
//! The painter owns the offset it is scrolling, so an adjustment is not a
//! position but a distance: each container keeps the sum of every adjustment
//! anchoring made to it, and the committed frame carries that sum in its
//! scroll slot ([`crate::visual::ScrollSlot::anchor_shift`]). The painter moves
//! its own offset by whatever part of the sum it has not applied yet, and
//! names the sum it had applied in every offset it posts back, so the
//! document can add what a post made before the painter saw an adjustment is
//! missing ([`Document::scroll_anchor_shift`]).

use euclid::default::{Rect, Vector2D};
use seldon::style::PositionProperty;
use smallvec::SmallVec;
use stylo::computed_values::overflow_anchor::T as OverflowAnchor;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;
use stylo::values::computed::{ContentVisibility, ScrollSnapAxis, ScrollSnapStrictness};

use super::snap::scroll_padding;
use super::{ScrollBox, is_scroll_container};
use crate::layout::{DisplayMode, StyleView, box_parent};
use crate::tree::document::{Document, NodeId};

/// One scroll container's anchoring: the side table entry
/// `scroll_anchors` keeps for every container a layout has anchored.
#[derive(Debug, Default)]
pub(crate) struct ScrollAnchor {
    /// The anchor node, while one is selected.
    node: Option<NodeId>,
    /// The sum of every block-axis adjustment anchoring has made to the
    /// container, in CSS px.
    shift: f32,
}

impl ScrollAnchor {
    /// The sum of the container's adjustments, as a scroll slot carries it.
    pub(crate) fn shift(&self) -> Vector2D<f32> {
        Vector2D::new(0.0, self.shift)
    }
}

/// The computed styles from an anchor up to its container inclusive.
type AnchorPath = SmallVec<[(NodeId, Arc<ComputedValues>); 4]>;

/// What a layout reads of one anchor before it runs.
pub(crate) struct AnchorBefore {
    scroller: NodeId,
    anchor: NodeId,
    /// The container's admitted block offset.
    offset: f32,
    /// The anchor's border box in the container's scrolled content.
    rect: Rect<f32>,
    /// The computed styles from the anchor up to the container inclusive,
    /// for the suppression triggers.
    path: AnchorPath,
}

/// What [`Document::examine_anchor_candidate`] says of one box.
enum Candidate {
    /// Neither the box nor anything under it can be the anchor.
    Skip,
    /// The box cannot be the anchor, but a box under it may be.
    Descend,
    /// The scrollport shows the whole box: it is the anchor.
    Select,
    /// The scrollport shows part of the box: it is the anchor unless a box
    /// under it qualifies.
    Constrain,
}

impl<T: Sync> Document<T> {
    /// §2.1 before a layout: selects an anchor for every scrolled container
    /// that has none and reads where each anchor stands, for
    /// [`Self::adjust_scroll_anchors`] to compare against after the layout.
    /// Empty when no container is scrolled, or when the layout about to run
    /// has nothing to lay out.
    pub(crate) fn select_scroll_anchors(&mut self) -> SmallVec<[AnchorBefore; 1]> {
        let mut before = SmallVec::new();
        if self.layout_state().scroll_offsets.is_empty()
            || !self.has_laid_out()
            || !self.layout_owed()
        {
            return before;
        }
        let scrollers: SmallVec<[NodeId; 4]> = self
            .layout_state()
            .scroll_offsets
            .iter()
            .filter(|(_, offset)| offset.y != 0.0)
            .map(|(&scroller, _)| scroller)
            .collect();
        for scroller in scrollers {
            match self.read_scroll_anchor(scroller) {
                Some(read) => before.push(read),
                None => self.clear_scroll_anchor(scroller),
            }
        }
        before
    }

    /// Selects `scroller`'s anchor if it has none and reads it, or `None`
    /// when the container is not to be anchored or shows nothing to anchor.
    fn read_scroll_anchor(&mut self, scroller: NodeId) -> Option<AnchorBefore> {
        let style = self.paint_style(scroller)?;
        if style.get_box().overflow_anchor == OverflowAnchor::None {
            return None;
        }
        let scroll_box = self.scroll_box(scroller)?;
        if scroll_box.offset.y == 0.0
            || scroll_box.wrap_period().1.is_some()
            || snaps_on_block_axis(style)
            || self.pending_scroll_request(scroller).is_some()
        {
            return None;
        }
        let kept = self
            .layout_state()
            .scroll_anchors
            .get(&scroller)
            .and_then(|entry| entry.node)
            .filter(|&anchor| {
                self.anchors_within(anchor, scroller) && !self.skips_auto_contents(anchor)
            });
        let anchor = match kept {
            Some(anchor) => anchor,
            None => self.find_scroll_anchor(scroller, style, &scroll_box)?,
        };
        let rect = self.rect_in_scroll_container(anchor, scroller)?;
        let path = self.anchor_path(anchor, scroller)?;
        self.layout_state_mut()
            .scroll_anchors
            .entry(scroller)
            .or_default()
            .node = Some(anchor);
        Some(AnchorBefore {
            scroller,
            anchor,
            offset: scroll_box.offset.y,
            rect,
            path,
        })
    }

    /// §2.2 after a layout: moves each container read by
    /// [`Self::select_scroll_anchors`] by the distance its anchor moved,
    /// unless a suppression trigger fired.
    pub(crate) fn adjust_scroll_anchors(&mut self, before: SmallVec<[AnchorBefore; 1]>) {
        for read in before {
            let AnchorBefore {
                scroller,
                anchor,
                offset,
                rect,
                path,
            } = read;
            let still = self
                .layout_state()
                .scroll_anchors
                .get(&scroller)
                .is_some_and(|entry| entry.node == Some(anchor));
            if !still {
                continue;
            }
            let after = self.rect_in_scroll_container(anchor, scroller);
            let (Some(after), Some(scroll_box)) = (after, self.scroll_box(scroller)) else {
                self.clear_scroll_anchor(scroller);
                continue;
            };
            if self.anchor_path_changed(&path) {
                self.clear_scroll_anchor(scroller);
                continue;
            }
            let mut target = offset + (after.min_y() - rect.min_y());
            let resized = after.height().to_bits() != rect.height().to_bits();
            if resized && self.is_auto_content_visibility(anchor) {
                // Blink's `ComputeAdjustment` for an anchor that stood at its
                // placeholder size: it may not end wholly above the
                // scrollport.
                target = target.min(after.max_y());
                self.clear_scroll_anchor(scroller);
            }
            let target = scroll_box.admit(Vector2D::new(scroll_box.offset.x, target));
            let moved = target.y - offset;
            if moved == 0.0 {
                continue;
            }
            let slot = self.slot(scroller).expect("a scroll container is live");
            let state = self.layout_state_mut();
            state.scroll_offsets.insert(slot, target);
            state.scroll_anchors.entry(scroller).or_default().shift += moved;
            // The frame carries the offset and the shift; the observers
            // read the offset.
            self.note_visual_mutation();
            self.note_intersections_stale();
        }
    }

    /// §2.1's anchor selection algorithm over `scroller`'s flat tree, at
    /// its current offset.
    fn find_scroll_anchor(
        &self,
        scroller: NodeId,
        style: &ComputedValues,
        scroll_box: &ScrollBox,
    ) -> Option<NodeId> {
        let port = scroll_box.scrollport;
        let left = scroll_padding(style.get_scroll_padding_left(), port.width);
        let right = scroll_padding(style.get_scroll_padding_right(), port.width);
        let top = scroll_padding(style.get_scroll_padding_top(), port.height);
        let bottom = scroll_padding(style.get_scroll_padding_bottom(), port.height);
        let visible = Rect::new(
            euclid::point2(scroll_box.offset.x + left, scroll_box.offset.y + top),
            euclid::size2(
                (port.width - left - right).max(0.0),
                (port.height - top - bottom).max(0.0),
            ),
        );
        let mut candidate = None;
        let mut pending: Vec<NodeId> = self
            .get(scroller)?
            .flat_children()
            .iter()
            .rev()
            .copied()
            .collect();
        while let Some(id) = pending.pop() {
            let examined = self.examine_anchor_candidate(id, scroller, &visible);
            match examined {
                Candidate::Skip => {}
                Candidate::Select => return Some(id),
                Candidate::Constrain if self.closes_anchor_walk(id) => return Some(id),
                Candidate::Constrain => {
                    // Nothing outside a partly shown box can be chosen once
                    // it has been reached: it or a box under it is.
                    candidate = Some(id);
                    pending.clear();
                    pending.extend(self.get(id)?.flat_children().iter().rev());
                }
                Candidate::Descend => {
                    pending.extend(self.get(id)?.flat_children().iter().rev());
                }
            }
        }
        candidate
    }

    /// §2.1's examination of one box against the visible rect.
    fn examine_anchor_candidate(
        &self,
        id: NodeId,
        scroller: NodeId,
        visible: &Rect<f32>,
    ) -> Candidate {
        let Some(node) = self.get(id) else {
            return Candidate::Skip;
        };
        let Some(view) = StyleView::try_of(node) else {
            return Candidate::Skip;
        };
        let values = view.values();
        match view.display_mode() {
            DisplayMode::None => return Candidate::Skip,
            DisplayMode::Contents => return Candidate::Descend,
            _ => {}
        }
        if values.get_box().overflow_anchor == OverflowAnchor::None {
            return Candidate::Skip;
        }
        if self.skips_auto_contents(id) {
            return Candidate::Skip;
        }
        match *values.get_box().get_position() {
            PositionProperty::Fixed | PositionProperty::Sticky => return Candidate::Skip,
            PositionProperty::Absolute if self.nearest_scroll_container(id) != Some(scroller) => {
                return Candidate::Skip;
            }
            _ => {}
        }
        if crate::layout::inline_flow::is_inline_box(node) {
            return Candidate::Descend;
        }
        let Some(rect) = self.rect_in_scroll_container(id, scroller) else {
            return Candidate::Skip;
        };
        if rect.is_empty() || !rect.intersects(visible) {
            return Candidate::Skip;
        }
        if visible.contains_rect(&rect) {
            Candidate::Select
        } else {
            Candidate::Constrain
        }
    }

    /// Whether the walk stops at `id` rather than entering it: a nested
    /// scroll container, whose contents move with its own offset, or a Lynx
    /// paragraph, whose contents are its line boxes.
    fn closes_anchor_walk(&self, id: NodeId) -> bool {
        self.get(id).is_some_and(|node| {
            node.is_lynx_text()
                || node
                    .layout_computed_style()
                    .is_some_and(is_scroll_container)
        })
    }

    /// Whether `anchor` can still anchor `scroller`: live, with a box, and
    /// still in the container's own scrolled content.
    fn anchors_within(&self, anchor: NodeId, scroller: NodeId) -> bool {
        self.get(anchor).is_some()
            && self.nearest_scroll_container(anchor) == Some(scroller)
            && self
                .rect_in_scroll_container(anchor, scroller)
                .is_some_and(|rect| !rect.is_empty())
    }

    /// The computed styles from `anchor` up to `scroller` inclusive.
    fn anchor_path(&self, anchor: NodeId, scroller: NodeId) -> Option<AnchorPath> {
        let mut path = SmallVec::new();
        let mut node = self.get(anchor)?;
        loop {
            path.push((node.id(), node.layout_computed_style_arc()?.clone()));
            if node.id() == scroller {
                return Some(path);
            }
            node = box_parent(node)?;
        }
    }

    /// §2.2.2: whether a suppression trigger fired on the path since it was
    /// read.
    fn anchor_path_changed(&self, path: &[(NodeId, Arc<ComputedValues>)]) -> bool {
        path.iter().any(|(id, old)| {
            let Some(new) = self
                .get(*id)
                .and_then(|node| node.layout_computed_style_arc())
            else {
                return true;
            };
            !Arc::ptr_eq(old, new) && triggers_suppression(old, new)
        })
    }

    /// Whether `id` is a `content-visibility: auto` box that skips its
    /// contents: undetermined, or determined not relevant. Its size is then
    /// its `contain-intrinsic-size` or last remembered size, never a layout
    /// of what it holds.
    fn skips_auto_contents(&self, id: NodeId) -> bool {
        self.get(id).is_some_and(|node| {
            node.layout_computed_style().is_some_and(|style| {
                *style.get_content_visibility() == ContentVisibility::Auto
                    && crate::layout::skips_contents(node, style)
            })
        })
    }

    /// Whether `id` computes `content-visibility: auto`.
    fn is_auto_content_visibility(&self, id: NodeId) -> bool {
        self.paint_style(id)
            .is_some_and(|style| *style.get_content_visibility() == ContentVisibility::Auto)
    }
}

impl<T> Document<T> {
    /// The sum of every block-axis adjustment scroll anchoring has made to
    /// `id`'s offset, as the committed frame carries it to the painter; see
    /// the module documentation. Zero for a container never adjusted.
    #[must_use]
    pub fn scroll_anchor_shift(&self, id: NodeId) -> Vector2D<f32> {
        self.slot(id)
            .and_then(|slot| self.layout_state().scroll_anchors.get(&slot))
            .map_or_else(Vector2D::zero, ScrollAnchor::shift)
    }

    /// Forgets `id`'s anchor: a scroll anchoring did not make moved the
    /// container, so the next layout selects one at the new offset. The sum
    /// of its adjustments stays.
    pub(crate) fn clear_scroll_anchor(&mut self, id: NodeId) {
        if let Some(entry) = self.layout_state_mut().scroll_anchors.get_mut(&id) {
            entry.node = None;
        }
    }
}

/// Whether `style` snaps its container on the block axis: css-scroll-snap-1
/// §4.1.3 re-snapping then keeps it on the area it stood on, which is
/// anchoring's job done by the area the author chose.
fn snaps_on_block_axis(style: &ComputedValues) -> bool {
    let snap_type = style.get_scroll_snap_type();
    snap_type.strictness() != ScrollSnapStrictness::None
        && matches!(
            snap_type.axis(),
            ScrollSnapAxis::Y | ScrollSnapAxis::Block | ScrollSnapAxis::Both
        )
}

/// §2.2.2's computed-value triggers between two styles of one element.
fn triggers_suppression(old: &ComputedValues, new: &ComputedValues) -> bool {
    let (old_box, new_box) = (old.get_box(), new.get_box());
    let (old_position, new_position) = (old.get_position(), new.get_position());
    old_box.get_position() != new_box.get_position()
        || old_box.transform != new_box.transform
        || old.get_margin() != new.get_margin()
        || old.get_padding() != new.get_padding()
        || old_position.top != new_position.top
        || old_position.right != new_position.right
        || old_position.bottom != new_position.bottom
        || old_position.left != new_position.left
        || old_position.width != new_position.width
        || old_position.height != new_position.height
        || old_position.min_width != new_position.min_width
        || old_position.min_height != new_position.min_height
        || old_position.max_width != new_position.max_width
        || old_position.max_height != new_position.max_height
}
