//! CSSOM-View scrolling: the scroll box model over the laid-out tree.
//!
//! A **scroll container** is a box whose computed `overflow` is scrollable on
//! either axis (css-overflow-3 §3). Its *scrollport* is its padding box; its
//! *scrolling area* is the union of that scrollport with the scrollable
//! overflow the layout engine accumulated ([`hughie::tree::Layout::content_size`]).
//! The difference between the two is how far the box can scroll.
//!
//! `hidden` is a scroll container but is **not user-scrollable**: it clips, and
//! it scrolls only when something asks it to programmatically. Only `scroll`
//! and `auto` respond to gestures. That distinction carries real weight in this
//! engine, because Lynx's UA cascade puts `overflow: hidden` on *every*
//! element — conflating the two would make every box drag-scrollable.
//!
//! Offsets are stored per node in the layout arena and clamped against live
//! geometry on every read, so a relayout that shrinks the scrolling area, or a
//! restyle that stops the box being a scroll container, corrects the
//! observable offset without an invalidation hook of its own. Like every other
//! post-layout query here, the clamp reads the **last committed** layout: call
//! [`Document::layout`] first if the tree has been mutated since.
//!
//! Nothing in this module knows about input devices. [`crate::input`] drives
//! it from pointer and wheel events; an embedder, or a runtime layer's
//! `scrollTo`-style API, drives it directly.
//!
//! Deliberate limits:
//! - Only element boxes scroll. There is no document/viewport scrolling area: the root box is sized
//!   to the viewport, and page scrolling is a runtime-policy concern the embedder resolves by
//!   making its own root element a scroll container.
//! - The scrolling area does not extend past the last box by the scroll container's own end-side
//!   padding (css-overflow-3 §2.2). That end padding is missing from the layout engine's
//!   accumulated content size, not discarded here.
//! - `scroll-behavior`, scroll snapping, `overscroll-behavior`, and rubber-band overscroll are
//!   absent: scrolling is instantaneous and clamps hard at the boundary, and chaining always
//!   chains.

use euclid::default::{Size2D, Vector2D};
use stylo::properties::ComputedValues;

use crate::NodeId;
use crate::document::Document;
use crate::layout::box_parent;
use crate::node::Node;

/// Which axes of a box the user may scroll directly.
///
/// Post-cascade the two axes always agree on *whether* they are scrollable —
/// the style adjuster pairs them (css-overflow-3 §3) — but not on *how*:
/// `overflow-x: hidden; overflow-y: scroll` survives the adjuster and is
/// user-scrollable vertically only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollAxes {
    pub x: bool,
    pub y: bool,
}

impl ScrollAxes {
    pub const NONE: Self = Self { x: false, y: false };
    pub const BOTH: Self = Self { x: true, y: true };

    /// The part of `delta` these axes admit.
    #[must_use]
    fn mask(self, delta: Vector2D<f32>) -> Vector2D<f32> {
        Vector2D::new(
            if self.x { delta.x } else { 0.0 },
            if self.y { delta.y } else { 0.0 },
        )
    }
}

/// One scroll container's geometry, in CSS px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollBox {
    /// The visible area — the padding box (CSSOM-View `clientWidth`/`clientHeight`).
    pub scrollport: Size2D<f32>,
    /// The scrolling area (CSSOM-View `scrollWidth`/`scrollHeight`), never
    /// smaller than [`Self::scrollport`].
    pub scroll_size: Size2D<f32>,
    /// The current, already-clamped offset (CSSOM-View `scrollLeft`/`scrollTop`).
    pub offset: Vector2D<f32>,
    /// The axes the user may scroll directly.
    pub user_scrollable: ScrollAxes,
}

impl ScrollBox {
    /// The largest offset this box admits: how far its scrolling area
    /// overhangs its scrollport, never negative.
    #[must_use]
    pub fn max_offset(&self) -> Vector2D<f32> {
        Vector2D::new(
            (self.scroll_size.width - self.scrollport.width).max(0.0),
            (self.scroll_size.height - self.scrollport.height).max(0.0),
        )
    }
}

/// Whether this computed style makes a box a scroll container at all —
/// including `overflow: hidden`, which scrolls only programmatically.
#[must_use]
pub fn is_scroll_container(style: &ComputedValues) -> bool {
    style.clone_overflow_x().is_scrollable() || style.clone_overflow_y().is_scrollable()
}

/// The axes this computed style lets the user scroll: `scroll` and `auto`,
/// never `hidden` (css-overflow-3 §3).
#[must_use]
pub fn user_scrollable_axes(style: &ComputedValues) -> ScrollAxes {
    ScrollAxes {
        x: style.clone_overflow_x().is_user_scrollable(),
        y: style.clone_overflow_y().is_user_scrollable(),
    }
}

fn clamp_to(offset: Vector2D<f32>, max: Vector2D<f32>) -> Vector2D<f32> {
    Vector2D::new(offset.x.clamp(0.0, max.x), offset.y.clamp(0.0, max.y))
}

/// The whole scroll model of one box, from its computed style, its committed
/// layout, and its stored offset. `None` when the box is not a scroll
/// container.
///
/// Single-sourced here so the public query and the paint-order build cannot
/// disagree about how far a box has scrolled.
pub(crate) fn resolve(
    style: &ComputedValues,
    layout: &hughie::tree::Layout,
    stored: Vector2D<f32>,
) -> Option<ScrollBox> {
    if !is_scroll_container(style) {
        return None;
    }
    // `content_size` is the scrollable overflow extent in the box's own
    // border-box coordinates; the scrollport starts one border in.
    let scrollport = Size2D::new(
        (layout.size.width - layout.border.left - layout.border.right).max(0.0),
        (layout.size.height - layout.border.top - layout.border.bottom).max(0.0),
    );
    let scroll_size = Size2D::new(
        (layout.content_size.width - layout.border.left).max(scrollport.width),
        (layout.content_size.height - layout.border.top).max(scrollport.height),
    );
    let mut scroll_box = ScrollBox {
        scrollport,
        scroll_size,
        offset: Vector2D::zero(),
        user_scrollable: user_scrollable_axes(style),
    };
    scroll_box.offset = clamp_to(stored, scroll_box.max_offset());
    Some(scroll_box)
}

impl<T> Document<T> {
    /// Whether this node is a scroll container — a box whose computed
    /// `overflow` is scrollable on either axis. `overflow: hidden` qualifies;
    /// it just is not user-scrollable.
    ///
    /// # Panics
    ///
    /// As [`Document::paint_style`], when styles are not ready.
    #[must_use]
    pub fn is_scroll_container(&self, id: NodeId) -> bool {
        self.paint_style(id).is_some_and(is_scroll_container)
    }

    /// This node's scroll geometry, or `None` when it is not a scroll
    /// container.
    ///
    /// Resolved against the last committed layout — call [`Document::layout`]
    /// first if the tree has been mutated since.
    ///
    /// # Panics
    ///
    /// As [`Document::paint_style`], when styles are not ready.
    #[must_use]
    pub fn scroll_box(&self, id: NodeId) -> Option<ScrollBox> {
        resolve(
            self.paint_style(id)?,
            self.rounded_layout(id)?,
            self.stored_scroll_offset(id),
        )
    }

    /// This node's scroll position, clamped to what its current geometry
    /// admits. Zero for every node that is not a scroll container.
    ///
    /// # Panics
    ///
    /// As [`Document::paint_style`], when styles are not ready.
    #[must_use]
    pub fn scroll_offset(&self, id: NodeId) -> Vector2D<f32> {
        self.scroll_box(id)
            .map_or_else(Vector2D::zero, |scroll_box| scroll_box.offset)
    }

    fn stored_scroll_offset(&self, id: NodeId) -> Vector2D<f32> {
        self.layout_state()
            .nodes
            .get(id)
            .map_or_else(Vector2D::zero, |state| state.scroll_offset)
    }

    /// Scrolls `id` to `offset`, clamping it into range. Returns the offset
    /// actually applied.
    ///
    /// Scrolling moves painted content but neither restyles nor relayouts, so
    /// this invalidates the visual frame only: rebuild the
    /// [`crate::visual::PaintOrder`] afterwards, but expect no style or layout
    /// work.
    ///
    /// # Panics
    ///
    /// As [`Document::paint_style`], when styles are not ready.
    pub fn scroll_to(&mut self, id: NodeId, offset: Vector2D<f32>) -> Vector2D<f32> {
        let Some(scroll_box) = self.scroll_box(id) else {
            return Vector2D::zero();
        };
        let clamped = clamp_to(offset, scroll_box.max_offset());
        if clamped != scroll_box.offset {
            self.note_visual_mutation();
        }
        self.layout_state_mut()
            .nodes
            .get_mut(id)
            .expect("a scroll container is a live node with layout-arena state")
            .scroll_offset = clamped;
        clamped
    }

    /// Scrolls `id` by `delta`, returning the part of `delta` it could **not**
    /// consume — zero when the box absorbed the whole delta, the untouched
    /// delta when it is not a scroll container or is already at that boundary.
    ///
    /// The remainder is the primitive scroll chaining is built from, here and
    /// in whatever nested-scroll policy a runtime layer grows later.
    ///
    /// # Panics
    ///
    /// As [`Document::paint_style`], when styles are not ready.
    pub fn scroll_by(&mut self, id: NodeId, delta: Vector2D<f32>) -> Vector2D<f32> {
        let Some(scroll_box) = self.scroll_box(id) else {
            return delta;
        };
        let applied = self.scroll_to(id, scroll_box.offset + delta);
        scroll_box.offset + delta - applied
    }

    /// The nearest box at or above `id` that the user may scroll along at
    /// least one of `axes`, walking the box tree (`display: contents` levels
    /// generate no box and cannot scroll).
    ///
    /// A box that is user-scrollable but already pinned at both boundaries
    /// still qualifies: whether it *can absorb a particular delta* is
    /// [`Self::scroll_by`]'s answer, not this one's, so a gesture stays
    /// latched to the scroller it started on.
    ///
    /// # Panics
    ///
    /// As [`Document::paint_style`], when styles are not ready.
    #[must_use]
    pub fn nearest_user_scrollable(&self, id: NodeId, axes: ScrollAxes) -> Option<NodeId> {
        let mut current = self.get(id);
        while let Some(node) = current {
            if node.is_element()
                && let Some(style) = node.layout_computed_style()
            {
                let scrollable = user_scrollable_axes(style);
                if (scrollable.x && axes.x) || (scrollable.y && axes.y) {
                    return Some(node.id());
                }
            }
            current = if node.is_element() {
                box_parent(node)
            } else {
                // A text leaf's own parent is its styled element, box or not.
                node.parent()
            };
        }
        None
    }

    /// Applies `delta` at `from`, chaining whatever the innermost scroller
    /// cannot absorb outward through its box-tree ancestors, and returns the
    /// innermost box that consumed anything together with the total consumed.
    ///
    /// This is the CSS default (`overscroll-behavior: auto`): a scroller at its
    /// boundary hands the rest to its parent. The node reported is the
    /// innermost consumer, which is what an event target wants to name even
    /// when an ancestor absorbed the rest.
    ///
    /// # Panics
    ///
    /// As [`Document::paint_style`], when styles are not ready.
    pub fn scroll_chain(
        &mut self,
        from: NodeId,
        delta: Vector2D<f32>,
    ) -> Option<(NodeId, Vector2D<f32>)> {
        let mut remaining = delta;
        let mut innermost = None;
        let mut consumed = Vector2D::zero();
        let mut next = Some(from);

        while let Some(start) = next {
            let axes = ScrollAxes {
                x: remaining.x != 0.0,
                y: remaining.y != 0.0,
            };
            let Some(scroller) = self.nearest_user_scrollable(start, axes) else {
                break;
            };
            let admitted = self
                .scroll_box(scroller)
                .map_or(ScrollAxes::NONE, |scroll_box| scroll_box.user_scrollable)
                .mask(remaining);
            let unconsumed = self.scroll_by(scroller, admitted);
            let step = admitted - unconsumed;
            if step != Vector2D::zero() {
                innermost.get_or_insert(scroller);
                consumed += step;
                remaining -= step;
            }
            if remaining == Vector2D::zero() {
                break;
            }
            next = self.get(scroller).and_then(Node::parent_id);
        }

        innermost.map(|node| (node, consumed))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::StylesheetOrigin;
    use crate::document::tests::device;

    /// A 100×100 `overflow: scroll` box holding a 300×400 child, nested inside
    /// an outer 200×200 scroller so chaining has somewhere to go.
    fn nested_scrollers() -> (Document<()>, NodeId, NodeId) {
        let mut document: Document<()> = Document::new(device());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .outer { display: flex; overflow: scroll; width: 200px; height: 200px; }
             .inner { display: flex; overflow: scroll; width: 100px; height: 100px; }
             .content { flex-shrink: 0; width: 300px; height: 400px; }
             .tall { flex-shrink: 0; width: 100px; height: 1000px; }",
            StylesheetOrigin::Author,
        );
        let root = document.create_element("page", ());
        document.append_document_element(root);

        let outer = document.create_element("view", ());
        document.add_class(outer, "outer");
        document.append_child(root, outer);

        let inner = document.create_element("view", ());
        document.add_class(inner, "inner");
        document.append_child(outer, inner);

        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(inner, content);

        let filler = document.create_element("view", ());
        document.add_class(filler, "tall");
        document.append_child(outer, filler);

        document.layout();
        (document, outer, inner)
    }

    #[test]
    fn scroll_geometry_comes_from_the_scrollable_overflow_the_layout_left() {
        let (document, _outer, inner) = nested_scrollers();
        let scroll_box = document.scroll_box(inner).expect("inner is a scroll box");
        assert_eq!(scroll_box.scrollport, Size2D::new(100.0, 100.0));
        assert_eq!(scroll_box.scroll_size, Size2D::new(300.0, 400.0));
        assert_eq!(scroll_box.max_offset(), Vector2D::new(200.0, 300.0));
        assert_eq!(scroll_box.user_scrollable, ScrollAxes::BOTH);
    }

    #[test]
    fn overflow_hidden_is_a_scroll_container_but_not_user_scrollable() {
        let mut document: Document<()> = Document::new(device());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .clip { display: flex; overflow: hidden; width: 100px; height: 100px; }
             .content { flex-shrink: 0; width: 300px; height: 400px; }",
            StylesheetOrigin::Author,
        );
        let root = document.create_element("page", ());
        document.append_document_element(root);
        let clip = document.create_element("view", ());
        document.add_class(clip, "clip");
        document.append_child(root, clip);
        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(clip, content);
        document.layout();

        assert!(document.is_scroll_container(clip));
        let scroll_box = document.scroll_box(clip).expect("hidden still scrolls");
        assert_eq!(scroll_box.user_scrollable, ScrollAxes::NONE);
        assert_eq!(scroll_box.max_offset(), Vector2D::new(200.0, 300.0));

        // Programmatic scrolling reaches it; a gesture never would, because
        // `nearest_user_scrollable` refuses to name it.
        assert_eq!(
            document.scroll_to(clip, Vector2D::new(50.0, 60.0)),
            Vector2D::new(50.0, 60.0)
        );
        assert_eq!(
            document.nearest_user_scrollable(content, ScrollAxes::BOTH),
            None,
        );
    }

    #[test]
    fn offsets_clamp_into_range_and_report_the_unconsumed_remainder() {
        let (mut document, _outer, inner) = nested_scrollers();

        assert_eq!(
            document.scroll_to(inner, Vector2D::new(-40.0, 9_000.0)),
            Vector2D::new(0.0, 300.0),
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));

        // Already pinned at the bottom: the whole downward delta comes back.
        assert_eq!(
            document.scroll_by(inner, Vector2D::new(50.0, 25.0)),
            Vector2D::new(0.0, 25.0),
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(50.0, 300.0));
    }

    #[test]
    fn a_shrinking_relayout_reclamps_the_offset_without_an_invalidation_hook() {
        let (mut document, _outer, inner) = nested_scrollers();
        document.scroll_to(inner, Vector2D::new(0.0, 300.0));

        document.add_stylesheet(".content { height: 120px; }", StylesheetOrigin::Author);
        document.layout();

        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 20.0));
    }

    #[test]
    fn a_box_that_stops_scrolling_reports_no_offset() {
        let (mut document, _outer, inner) = nested_scrollers();
        document.scroll_to(inner, Vector2D::new(0.0, 100.0));

        document.add_stylesheet(".inner { overflow: visible; }", StylesheetOrigin::Author);
        document.layout();

        assert!(!document.is_scroll_container(inner));
        assert_eq!(document.scroll_box(inner), None);
        assert_eq!(document.scroll_offset(inner), Vector2D::zero());
    }

    #[test]
    fn chaining_hands_the_remainder_to_the_next_scroller_out() {
        let (mut document, outer, inner) = nested_scrollers();

        // 400 down at the innermost box: 300 fits, 100 chains to the outer one.
        let (named, total) = document
            .scroll_chain(inner, Vector2D::new(0.0, 400.0))
            .expect("something scrolled");
        assert_eq!(named, inner, "the innermost consumer names the event");
        assert_eq!(total, Vector2D::new(0.0, 400.0));
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 100.0));

        // Both pinned downward now, so a further push moves nothing at all.
        document.scroll_to(outer, Vector2D::new(0.0, 1_000.0));
        let pinned_at = document.scroll_offset(outer);
        assert_eq!(document.scroll_chain(inner, Vector2D::new(0.0, 50.0)), None);
        assert_eq!(document.scroll_offset(outer), pinned_at);
    }

    #[test]
    fn the_chain_starts_at_the_nearest_scroller_above_the_target() {
        let (mut document, _outer, inner) = nested_scrollers();
        let content = document
            .get(inner)
            .and_then(|node| node.child_ids().first().copied())
            .expect("inner has its content child");

        let (consumer, _) = document
            .scroll_chain(content, Vector2D::new(0.0, 30.0))
            .expect("the ancestor scroller consumes it");
        assert_eq!(consumer, inner);
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 30.0));
    }
}
