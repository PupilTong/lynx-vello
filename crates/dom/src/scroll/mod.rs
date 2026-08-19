//! CSSOM-View scrolling: the scroll box model over the laid-out tree.
//!
//! A **scroll container** is a box whose computed `overflow` is scrollable on
//! either axis (css-overflow-3 §3). Its *scrollport* is its padding box; its
//! *scrolling area* is the union of that scrollport with the scrollable
//! overflow the layout engine accumulated ([`hughie::tree::Layout::content_size`]).
//! The difference between the two is how far the box can scroll.
//!
//! The fork's keyword set is `visible | hidden | scroll | clip` — no `auto` —
//! and the three non-`visible` values are three different things:
//!
//! - `scroll` is a scroll container the user can drag and wheel.
//! - `hidden` is a scroll container that is **not user-scrollable**: it clips, and it moves only
//!   when something asks it to programmatically. That distinction carries real weight here, because
//!   Lynx's UA cascade puts `overflow: hidden` on *every* element — conflating the two would make
//!   every box drag-scrollable.
//! - `clip` is not a scroll container at all: it clips and stops, with no scrolling area and no
//!   offset, and its content does not reach into an ancestor's scrolling area either.
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
//! - **`position: sticky` does not stick.** It parses in the fork's grammar, but the paint build
//!   treats it as normal flow, so a sticky box scrolls away with its container instead of pinning
//!   to the scrollport. Before scrolling existed that was indistinguishable from `relative`; now it
//!   is observable, which is why it is written down here rather than left implied. Its scroll
//!   parent is deliberately its box parent (sticky *is* in flow — that part is right); what is
//!   missing is the offset clamp against the scrollport that css-position-3 §6.3 defines.

use euclid::default::{Size2D, Vector2D};
use hughie::style::PositionProperty;
use stylo::properties::ComputedValues;

use crate::NodeId;
use crate::layout::{
    box_parent, establishes_absolute_containing_block, establishes_fixed_containing_block,
};
use crate::tree::document::Document;
use crate::tree::node::Node;

/// Per-axis scrolling capability flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollAxes {
    pub x: bool,
    pub y: bool,
}

#[cfg(test)]
mod behavior_tests;

impl ScrollAxes {
    pub const NONE: Self = Self { x: false, y: false };
    pub const BOTH: Self = Self { x: true, y: true };

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

#[must_use]
fn is_scroll_container(style: &ComputedValues) -> bool {
    style.clone_overflow_x().is_scrollable() || style.clone_overflow_y().is_scrollable()
}

#[must_use]
fn user_scrollable_axes(style: &ComputedValues) -> ScrollAxes {
    ScrollAxes {
        x: style.clone_overflow_x().is_user_scrollable(),
        y: style.clone_overflow_y().is_user_scrollable(),
    }
}

fn clamp_axis(value: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, max)
    } else {
        0.0
    }
}

fn clamp_to(offset: Vector2D<f32>, max: Vector2D<f32>) -> Vector2D<f32> {
    Vector2D::new(clamp_axis(offset.x, max.x), clamp_axis(offset.y, max.y))
}

pub(crate) fn resolve(
    style: &ComputedValues,
    layout: &hughie::tree::Layout,
    stored: Vector2D<f32>,
) -> Option<ScrollBox> {
    if !is_scroll_container(style) {
        return None;
    }
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
    /// Whether this node has scrollable overflow.
    #[must_use]
    pub fn is_scroll_container(&self, id: NodeId) -> bool {
        self.paint_style(id).is_some_and(is_scroll_container)
    }

    /// Returns this node's scroll geometry when it is a scroll container.
    #[must_use]
    pub fn scroll_box(&self, id: NodeId) -> Option<ScrollBox> {
        resolve(
            self.paint_style(id)?,
            self.rounded_layout(id)?,
            self.stored_scroll_offset(id),
        )
    }

    /// Returns the scroll offset clamped to current geometry.
    #[must_use]
    pub fn scroll_offset(&self, id: NodeId) -> Vector2D<f32> {
        self.scroll_box(id)
            .map_or_else(Vector2D::zero, |scroll_box| scroll_box.offset)
    }

    fn stored_scroll_offset(&self, id: NodeId) -> Vector2D<f32> {
        self.slot(id).map_or_else(Vector2D::zero, |slot| {
            self.layout_state().at(slot).scroll_offset
        })
    }

    /// Scrolls to a clamped offset and returns the applied offset.
    pub fn scroll_to(&mut self, id: NodeId, offset: Vector2D<f32>) -> Vector2D<f32> {
        debug_assert!(
            offset.x.is_finite() && offset.y.is_finite(),
            "scroll offsets must be finite, got {offset:?}"
        );
        let Some(scroll_box) = self.scroll_box(id) else {
            return Vector2D::zero();
        };
        let clamped = clamp_to(offset, scroll_box.max_offset());
        if clamped != scroll_box.offset {
            self.note_visual_mutation();
        }
        let slot = self
            .slot(id)
            .expect("a scroll container is a live node with layout-arena state");
        self.layout_state_mut().at_mut(slot).scroll_offset = clamped;
        clamped
    }

    /// Scrolls by a delta and returns the unconsumed remainder.
    pub fn scroll_by(&mut self, id: NodeId, delta: Vector2D<f32>) -> Vector2D<f32> {
        let Some(scroll_box) = self.scroll_box(id) else {
            return delta;
        };
        let applied = self.scroll_to(id, scroll_box.offset + delta);
        scroll_box.offset + delta - applied
    }

    fn scroll_parent(&self, id: NodeId) -> Option<NodeId> {
        let node = self.get(id)?;
        if !node.is_element() {
            return node.flat_parent_id();
        }
        let style = node.layout_computed_style()?;
        match style.clone_position() {
            PositionProperty::Absolute => Self::containing_block(node, false),
            PositionProperty::Fixed => Self::containing_block(node, true),
            PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {
                box_parent(node).map(Node::id)
            }
        }
    }

    fn containing_block(node: &Node<T>, fixed: bool) -> Option<NodeId> {
        let mut current = box_parent(node);
        while let Some(ancestor) = current {
            let style = ancestor.layout_computed_style()?;
            let establishes = if fixed {
                establishes_fixed_containing_block(ancestor, style)
            } else {
                establishes_absolute_containing_block(ancestor, style)
            };
            if establishes {
                return Some(ancestor.id());
            }
            current = box_parent(ancestor);
        }
        None
    }

    /// Finds the nearest ancestor scrollable on every requested axis.
    #[must_use]
    pub fn nearest_user_scrollable(&self, id: NodeId, axes: ScrollAxes) -> Option<NodeId> {
        let mut current = Some(id);
        while let Some(node_id) = current {
            if let Some(node) = self.get(node_id)
                && node.is_element()
                && let Some(style) = node.layout_computed_style()
            {
                let scrollable = user_scrollable_axes(style);
                if (scrollable.x && axes.x) || (scrollable.y && axes.y) {
                    return Some(node_id);
                }
            }
            current = self.scroll_parent(node_id);
        }
        None
    }

    /// Applies a delta through the ancestor scroll chain.
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
            next = self.scroll_parent(scroller);
        }

        innermost.map(|node| (node, consumed))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::StylesheetOrigin;
    use crate::tree::document::tests::device;

    fn nested_scrollers() -> (Document<()>, NodeId, NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .outer { display: flex; overflow: scroll; width: 200px; height: 200px; }
             .inner { display: flex; overflow: scroll; width: 100px; height: 100px; }
             .content { flex-shrink: 0; width: 300px; height: 400px; }
             .tall { flex-shrink: 0; width: 100px; height: 1000px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();

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
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .clip { display: flex; overflow: hidden; width: 100px; height: 100px; }
             .content { flex-shrink: 0; width: 300px; height: 400px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
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

        let (named, total) = document
            .scroll_chain(inner, Vector2D::new(0.0, 400.0))
            .expect("something scrolled");
        assert_eq!(named, inner, "the innermost consumer names the event");
        assert_eq!(total, Vector2D::new(0.0, 400.0));
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 100.0));

        document.scroll_to(outer, Vector2D::new(0.0, 1_000.0));
        let pinned_at = document.scroll_offset(outer);
        assert_eq!(document.scroll_chain(inner, Vector2D::new(0.0, 50.0)), None);
        assert_eq!(document.scroll_offset(outer), pinned_at);
    }

    #[test]
    fn the_chain_follows_containing_blocks_not_dom_ancestry() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; position: relative; width: 800px; height: 600px; }
             .scroller { display: flex; flex-direction: column; overflow: scroll;
                         width: 100px; height: 100px; }
             .row { flex-shrink: 0; width: 100px; height: 100px; }
             .pinned { display: flex; position: absolute; left: 0; top: 0;
                       width: 50px; height: 50px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let scroller = document.create_element("view", ());
        document.add_class(scroller, "scroller");
        document.append_child(root, scroller);
        for _ in 0..3 {
            let row = document.create_element("view", ());
            document.add_class(row, "row");
            document.append_child(scroller, row);
        }
        let pinned = document.create_element("view", ());
        document.add_class(pinned, "pinned");
        document.append_child(scroller, pinned);
        document.layout();

        assert_eq!(
            document.nearest_user_scrollable(pinned, ScrollAxes::BOTH),
            None,
            "an absolute box anchored on the page has no scroller in its chain",
        );
        assert_eq!(
            document.scroll_chain(pinned, Vector2D::new(0.0, 50.0)),
            None,
        );
        assert_eq!(document.scroll_offset(scroller), Vector2D::zero());

        let row = document
            .get(scroller)
            .and_then(|node| node.child_ids().first().copied())
            .expect("the scroller has rows");
        assert_eq!(
            document.nearest_user_scrollable(row, ScrollAxes::BOTH),
            Some(scroller),
        );

        document.add_stylesheet(
            ".scroller { position: relative; }",
            StylesheetOrigin::Author,
        );
        document.layout();
        assert_eq!(
            document.nearest_user_scrollable(pinned, ScrollAxes::BOTH),
            Some(scroller),
        );
    }

    #[test]
    fn a_non_finite_offset_cannot_poison_the_stored_scroll_position() {
        let (mut document, _outer, inner) = nested_scrollers();
        document.scroll_to(inner, Vector2D::new(0.0, 100.0));

        assert_eq!(
            clamp_to(
                Vector2D::new(f32::NAN, f32::INFINITY),
                Vector2D::new(200.0, 300.0)
            ),
            Vector2D::zero(),
        );
        assert!(document.scroll_offset(inner).y.is_finite());
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
