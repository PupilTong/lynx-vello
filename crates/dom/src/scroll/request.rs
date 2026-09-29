//! Programmatic scroll requests: CSSOM-View's `scrollTo({behavior})`
//! (§3.1 "Scrolling", §4's `ScrollBehavior`) over a document whose scroll
//! positions are also moved by a painter on another thread.
//!
//! The document is not the only writer of a scroll position. The runtime's
//! painter moves containers on its own clock — drags, flings, bounce backs,
//! glides — and posts the offsets back, and the document adopts them with
//! [`Document::scroll_to`]. A programmatic scroll that only wrote the
//! document's offset would be overwritten by the next such post, and a
//! smooth one has nobody on this side to animate it. So a script-facing
//! scroll is a **request** the committed frame carries to the painter:
//!
//! - [`Document::scroll_to_with`] records it, keyed by the container, with a serial number taken
//!   from one counter per document. An [`ScrollBehavior::Instant`] request also moves the
//!   document's own offset at once, as [`Document::scroll_to`] does; a [`ScrollBehavior::Smooth`]
//!   one leaves it where it is, because the painter animates it and the offsets come back through
//!   the painter's posts like a fling's.
//! - The paint build copies a container's pending request into its [`ScrollSlot`], re-clamped to
//!   the built range; a request whose container has no slot in the frame the render publishes is
//!   dropped.
//! - The painter handles each serial once, and every offset it posts for the container afterwards
//!   names the newest serial it has handled there. The runtime drops a post naming an older serial
//!   than the container's pending request — it was made before the painter saw the request — and
//!   acknowledges the request with one that names it ([`Document::acknowledge_scroll_request`]).
//!
//! [`ScrollSlot`]: crate::visual::ScrollSlot

use std::num::NonZeroU64;

use euclid::default::Vector2D;

use super::clamp_to;
use crate::NodeId;
use crate::tree::document::Document;

/// How a programmatic scroll moves the container: CSSOM-View's
/// `ScrollBehavior` (§4) once `auto` is resolved. There is no
/// `scroll-behavior` property here for `auto` to read, so the caller names
/// one of the two outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollBehavior {
    /// §3.1's instant scroll: the position is the target from the moment of
    /// the request.
    Instant,
    /// §3.1's smooth scroll: the painter moves the position to the target
    /// over time, on a curve of its own choosing.
    Smooth,
}

/// One pending programmatic scroll of one container.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollRequest {
    /// Orders this request against every other one of the document, and so
    /// against the serial a painter's post names.
    pub serial: NonZeroU64,
    /// Where the container is to end, clamped to its scroll range.
    pub target: Vector2D<f32>,
    pub behavior: ScrollBehavior,
}

impl<T> Document<T> {
    /// Scrolls `id` to `offset` as a script-facing request with `behavior`,
    /// and returns the clamped target; see the module documentation.
    ///
    /// A node that is no scroll container records nothing and answers zero,
    /// as [`Self::scroll_to`] does. A second request for the same container
    /// replaces the first, which is CSSOM-View §3.1's abort of the ongoing
    /// smooth scroll. Either way the document needs a commit, because the
    /// request travels in the frame.
    pub fn scroll_to_with(
        &mut self,
        id: NodeId,
        offset: Vector2D<f32>,
        behavior: ScrollBehavior,
    ) -> Vector2D<f32> {
        debug_assert!(
            offset.x.is_finite() && offset.y.is_finite(),
            "scroll offsets must be finite, got {offset:?}"
        );
        let Some(scroll_box) = self.scroll_box(id) else {
            return Vector2D::zero();
        };
        let target = clamp_to(offset, scroll_box.max_offset());
        if behavior == ScrollBehavior::Instant {
            self.scroll_to(id, target);
        }
        let state = self.layout_state_mut();
        let serial = state.next_scroll_request;
        state.next_scroll_request = serial
            .checked_add(1)
            .expect("a document issues fewer than 2^64 scroll requests");
        state.scroll_requests.insert(
            id,
            ScrollRequest {
                serial,
                target,
                behavior,
            },
        );
        self.note_visual_mutation();
        target
    }

    /// The serial of `id`'s pending request, if it has one: the least serial
    /// a painter's post for `id` must name to be adopted.
    #[must_use]
    pub fn pending_scroll_request(&self, id: NodeId) -> Option<NonZeroU64> {
        self.layout_state()
            .scroll_requests
            .get(&id)
            .map(|request| request.serial)
    }

    /// Records that the painter has handled `id`'s requests up to `serial`:
    /// the pending one ends when its serial is not newer. Changes no offset
    /// and commits nothing — the frame that stops carrying the request is
    /// whichever commit comes next for other reasons, and the painter, which
    /// handles a serial once, ignores it until then.
    pub fn acknowledge_scroll_request(&mut self, id: NodeId, serial: NonZeroU64) {
        let requests = &mut self.layout_state_mut().scroll_requests;
        if requests
            .get(&id)
            .is_some_and(|request| request.serial <= serial)
        {
            requests.remove(&id);
        }
    }

    /// Drops every pending request whose container `keep` rejects: the paint
    /// build found no scroll container there to carry it.
    pub(crate) fn retain_scroll_requests(&mut self, keep: impl Fn(NodeId) -> bool) {
        let requests = &mut self.layout_state_mut().scroll_requests;
        if !requests.is_empty() {
            requests.retain(|node, _| keep(*node));
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::StylesheetOrigin;
    use crate::tree::document::tests::device;

    /// A 100px column scroller (max offset 0,300) in a page, and the scroller.
    fn scroller() -> (Document<()>, NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .scroller { display: flex; flex-direction: column; overflow: scroll;
                         width: 100px; height: 100px; }
             .content { flex-shrink: 0; width: 100px; height: 400px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let scroller = document.create_element("view", ());
        document.add_class(scroller, "scroller");
        document.append_child(root, scroller);
        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(scroller, content);
        document.commit();
        (document, scroller)
    }

    fn slot_request(document: &mut Document<()>, id: NodeId) -> Option<ScrollRequest> {
        let frame = document.commit();
        let index = frame.slot_of(id).expect("the scroller has a slot");
        frame.scroll_slots()[index as usize].request
    }

    #[test]
    fn an_instant_request_moves_the_document_and_travels_clamped_in_the_frame() {
        let (mut document, scroller) = scroller();
        assert!(!document.needs_render());
        let target = document.scroll_to_with(
            scroller,
            Vector2D::new(50.0, 900.0),
            ScrollBehavior::Instant,
        );
        assert_eq!(target, Vector2D::new(0.0, 300.0), "clamped to the range");
        assert_eq!(document.scroll_offset(scroller), target);
        assert!(document.needs_render(), "the request needs a commit");
        let request = slot_request(&mut document, scroller).expect("the slot carries it");
        assert_eq!(request.target, target);
        assert_eq!(request.behavior, ScrollBehavior::Instant);
        assert_eq!(
            document.pending_scroll_request(scroller),
            Some(request.serial)
        );
    }

    #[test]
    fn a_smooth_request_leaves_the_document_offset_to_the_painter() {
        let (mut document, scroller) = scroller();
        document.scroll_to_with(scroller, Vector2D::new(0.0, 120.0), ScrollBehavior::Smooth);
        assert_eq!(document.scroll_offset(scroller), Vector2D::zero());
        let request = slot_request(&mut document, scroller).expect("the slot carries it");
        assert_eq!(request.target, Vector2D::new(0.0, 120.0));
        assert_eq!(request.behavior, ScrollBehavior::Smooth);
    }

    #[test]
    fn the_build_reclamps_the_target_to_the_range_it_built() {
        let (mut document, scroller) = scroller();
        document.scroll_to_with(scroller, Vector2D::new(0.0, 250.0), ScrollBehavior::Smooth);
        document.add_stylesheet(".content { height: 200px; }", StylesheetOrigin::Author);
        let request = slot_request(&mut document, scroller).expect("the slot carries it");
        assert_eq!(request.target, Vector2D::new(0.0, 100.0));
    }

    #[test]
    fn the_latest_request_wins_and_an_acknowledgement_ends_it() {
        let (mut document, scroller) = scroller();
        document.scroll_to_with(scroller, Vector2D::new(0.0, 100.0), ScrollBehavior::Smooth);
        let first = document.pending_scroll_request(scroller).expect("recorded");
        document.scroll_to_with(scroller, Vector2D::new(0.0, 200.0), ScrollBehavior::Instant);
        let second = document.pending_scroll_request(scroller).expect("recorded");
        assert!(second > first, "serials are ordered");
        let request = slot_request(&mut document, scroller).expect("the slot carries it");
        assert_eq!(
            (request.serial, request.target),
            (second, Vector2D::new(0.0, 200.0))
        );

        document.acknowledge_scroll_request(scroller, first);
        assert_eq!(
            document.pending_scroll_request(scroller),
            Some(second),
            "an older serial acknowledges nothing"
        );
        document.acknowledge_scroll_request(scroller, second);
        assert_eq!(document.pending_scroll_request(scroller), None);
        document.set_inline_style(scroller, "opacity: 0.5");
        assert_eq!(
            slot_request(&mut document, scroller),
            None,
            "the next frame carries nothing"
        );
    }

    #[test]
    fn a_node_that_is_no_scroll_container_records_nothing() {
        let (mut document, scroller) = scroller();
        let root = document.document_element().id();
        assert_eq!(
            document.scroll_to_with(root, Vector2D::new(0.0, 10.0), ScrollBehavior::Smooth),
            Vector2D::zero()
        );
        assert_eq!(document.pending_scroll_request(root), None);
        assert!(!document.needs_render());
        assert_eq!(document.pending_scroll_request(scroller), None);
    }

    #[test]
    fn a_request_whose_container_stops_scrolling_is_dropped_at_the_build() {
        let (mut document, scroller) = scroller();
        document.scroll_to_with(scroller, Vector2D::new(0.0, 100.0), ScrollBehavior::Smooth);
        document.set_inline_style(scroller, "overflow: visible");
        let frame = document.commit();
        assert_eq!(frame.slot_of(scroller), None);
        assert_eq!(document.pending_scroll_request(scroller), None);
    }

    #[test]
    fn a_request_dies_with_its_node() {
        let (mut document, scroller) = scroller();
        document.scroll_to_with(scroller, Vector2D::new(0.0, 100.0), ScrollBehavior::Smooth);
        assert_eq!(document.layout_state().scroll_requests.len(), 1);
        document.drop_subtree(scroller);
        assert!(document.layout_state().scroll_requests.is_empty());
        assert_eq!(document.pending_scroll_request(scroller), None);
    }
}
