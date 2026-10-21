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
//! - The paint build copies a container's pending request into its [`ScrollSlot`], re-admitted
//!   against the built geometry (clamped to the built range, or on a circular axis normalized into
//!   the built period); a request whose container has no slot in the frame the render publishes is
//!   dropped.
//! - The painter handles each serial once, and every offset it posts for the container afterwards
//!   names the newest serial it has handled there. The runtime drops a post naming an older serial
//!   than the container's pending request — it was made before the painter saw the request — and
//!   acknowledges the request with one that names it ([`Document::acknowledge_scroll_request`]).
//!
//! One request the painter may decline: [`Document::scroll_to_yielding`]
//! records a scroll it carries out only while no pointer holds the
//! container, for a turn a timer asked for rather than a script. The
//! document moves no offset for it; the painter's post names it either way.
//!
//! # Auto-scroll
//!
//! Lynx's `autoScroll` is the other thing a script asks of a scroll
//! container: move along one axis at a constant rate until told to stop, or
//! until the edge. It names no position, so it is no [`ScrollRequest`]; it
//! is a state of the container the painter keeps, which a scroll request
//! neither starts nor ends. [`Document::auto_scroll`] records the change —
//! start at a rate, or stop — as an [`AutoScrollRequest`] with a serial from
//! the same counter, and the frame carries it beside its slots
//! ([`CommittedFrame::auto_scroll_requests`]) rather than in one, so a
//! scroll request and an auto-scroll change made in one turn both arrive.
//! The painter handles each serial once and names it in its posts as it does
//! a scroll request's, and [`Document::acknowledge_auto_scroll`] ends the
//! pending change. The document moves no offset for one and drops no post
//! on its account: the offsets an auto-scroll produces come back through
//! the painter's posts like a fling's.
//!
//! [`ScrollSlot`]: crate::visual::ScrollSlot
//! [`CommittedFrame::auto_scroll_requests`]: crate::visual::CommittedFrame::auto_scroll_requests

use std::num::NonZeroU64;

use euclid::default::Vector2D;
use smallvec::SmallVec;

use crate::tree::document::{Document, NodeId};

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
    /// Where the container is to end, as its scroll box admits it
    /// ([`ScrollBox::admit`](super::ScrollBox::admit)): clamped to its
    /// scroll range, or on a circular axis normalized into `[0, period)`.
    pub target: Vector2D<f32>,
    pub behavior: ScrollBehavior,
    /// Whether the painter is to leave the request undone while a pointer
    /// holds the container ([`Document::scroll_to_yielding`]).
    pub yields_to_pointer: bool,
}

/// One pending change of one container's auto-scroll: Lynx's
/// `autoScroll({rate, start, autoStop})`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoScrollRequest {
    /// Orders this change against every scroll request of the document and
    /// against the serial a painter's post names; drawn from the counter
    /// [`ScrollRequest::serial`] is.
    pub serial: NonZeroU64,
    /// How fast the axis's offset is to change, in CSS px per second: toward
    /// the end edge when positive, toward the start edge when negative, and
    /// zero to end the container's auto-scroll where it stands.
    pub rate: f32,
    /// Whether the axis is x rather than y.
    pub horizontal: bool,
    /// Whether the auto-scroll ends for good at the edge it moves toward.
    /// Otherwise it stays at that edge and goes on when the scroll range
    /// grows.
    pub stop_at_edge: bool,
}

impl<T> Document<T> {
    /// Changes `id`'s auto-scroll: at `rate` CSS px per second along the x
    /// axis when `horizontal`, else y, or none at all when `rate` is zero;
    /// see the module documentation and [`AutoScrollRequest`].
    ///
    /// A node that is no scroll container records nothing. A second change
    /// for the same container replaces the first: the newest one says what
    /// the container's auto-scroll is. The document needs a commit, because
    /// the change travels in the frame.
    pub fn auto_scroll(&mut self, id: NodeId, rate: f32, horizontal: bool, stop_at_edge: bool) {
        debug_assert!(rate.is_finite(), "an auto-scroll rate must be finite");
        if self.scroll_box(id).is_none() {
            return;
        }
        let state = self.layout_state_mut();
        let serial = state.next_scroll_request;
        state.next_scroll_request = serial
            .checked_add(1)
            .expect("a document issues fewer than 2^64 scroll requests");
        let request = AutoScrollRequest {
            serial,
            rate,
            horizontal,
            stop_at_edge,
        };
        match state
            .auto_scroll_requests
            .iter_mut()
            .find(|(node, _)| *node == id)
        {
            Some((_, pending)) => *pending = request,
            None => state.auto_scroll_requests.push((id, request)),
        }
        self.note_visual_mutation();
    }

    /// `id`'s pending auto-scroll change, if it has one.
    #[must_use]
    pub fn pending_auto_scroll(&self, id: NodeId) -> Option<AutoScrollRequest> {
        self.layout_state()
            .auto_scroll_requests
            .iter()
            .find(|(node, _)| *node == id)
            .map(|(_, request)| *request)
    }

    /// Records that the painter has handled `id`'s requests up to `serial`:
    /// the pending auto-scroll change ends when its serial is not newer.
    /// Commits nothing, as [`Self::acknowledge_scroll_request`] commits
    /// nothing. One test for a document with none pending.
    pub fn acknowledge_auto_scroll(&mut self, id: NodeId, serial: NonZeroU64) {
        let requests = &mut self.layout_state_mut().auto_scroll_requests;
        if !requests.is_empty() {
            requests.retain(|(node, request)| *node != id || request.serial > serial);
        }
    }

    /// Scrolls `id` to `offset` as a script-facing request with `behavior`,
    /// and returns the target as the scroll box admits it
    /// ([`ScrollBox::admit`](super::ScrollBox::admit)); see the module
    /// documentation.
    ///
    /// A node that is no scroll container records nothing and answers zero,
    /// as [`Self::scroll_to`] does. A second request for the same container
    /// replaces the first, which is CSSOM-View §3.1's abort of the ongoing
    /// smooth scroll. Either way the document needs a commit, because the
    /// request travels in the frame.
    ///
    /// An instant request that moves the container records it for
    /// [`Self::take_instant_scrolls`], so the host can fire CSSOM-View's
    /// `scroll` and `scrollend` for a scroll the document performed itself.
    pub fn scroll_to_with(
        &mut self,
        id: NodeId,
        offset: Vector2D<f32>,
        behavior: ScrollBehavior,
    ) -> Vector2D<f32> {
        self.request_scroll(id, offset, behavior, false)
            .unwrap_or_default()
    }

    /// Asks the painter to scroll `id` to `offset` with `behavior` unless a
    /// pointer holds the container when the request reaches it, in which
    /// case nothing moves: a scroll no script asked for at that moment — a
    /// timer's, such as an `x-swiper`'s autoplay turn — must not move
    /// content under a finger. Answers whether a request was recorded.
    ///
    /// Only the painter knows whether it carried the request out, so the
    /// document moves no offset of its own for one, whatever its behavior:
    /// the offset comes back through the painter's posts, as a smooth
    /// scroll's does. It replaces a pending request of its own kind and
    /// gives way to a pending [`Self::scroll_to_with`] one, which a script
    /// did ask for: then nothing is recorded.
    pub fn scroll_to_yielding(
        &mut self,
        id: NodeId,
        offset: Vector2D<f32>,
        behavior: ScrollBehavior,
    ) -> bool {
        self.request_scroll(id, offset, behavior, true).is_some()
    }

    /// The one request path behind [`Self::scroll_to_with`],
    /// [`Self::scroll_to_yielding`] and the render's own positioning
    /// (`scroll-initial-target`). `yields_to_pointer` is the yielding
    /// request's rule. Every instant move performed here is recorded for the
    /// host's scroll events ([`Self::take_instant_scrolls`]), whoever asked
    /// for it; a yielding request never performs here.
    pub(crate) fn request_scroll(
        &mut self,
        id: NodeId,
        offset: Vector2D<f32>,
        behavior: ScrollBehavior,
        yields_to_pointer: bool,
    ) -> Option<Vector2D<f32>> {
        debug_assert!(
            offset.x.is_finite() && offset.y.is_finite(),
            "scroll offsets must be finite, got {offset:?}"
        );
        let scroll_box = self.scroll_box(id)?;
        let target = scroll_box.admit(offset);
        if yields_to_pointer {
            let pending = self.layout_state().scroll_requests.get(&id);
            if pending.is_some_and(|pending| !pending.yields_to_pointer) {
                return None;
            }
        } else if behavior == ScrollBehavior::Instant {
            let previous = scroll_box.offset;
            let moved = self.scroll_to(id, target) != previous;
            let recorded = &mut self.layout_state_mut().instant_scrolls;
            if moved && !recorded.iter().any(|(held, _)| *held == id) {
                recorded.push((id, previous));
            }
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
                yields_to_pointer,
            },
        );
        self.note_visual_mutation();
        Some(target)
    }

    /// The containers an instant scroll request moved since the last call —
    /// a [`Self::scroll_to_with`], or the render's own move to a
    /// `scroll-initial-target` — each once with the offset it had before the
    /// first of those moves (so the host can report the change), in the
    /// order they first moved, and forgets them. The host fires `scroll` at
    /// each, in the task that scrolled; its `scrollend` follows when the
    /// painter, which carries the request out, posts the container at rest. A smooth one is the
    /// painter's to move, and the host learns its offsets, and its end, from
    /// the painter's posts alone.
    ///
    /// No allocation while a container or two moved, and none at all when
    /// nothing did.
    pub fn take_instant_scrolls(&mut self) -> SmallVec<[(NodeId, Vector2D<f32>); 2]> {
        std::mem::take(&mut self.layout_state_mut().instant_scrolls)
    }

    /// Whether [`Self::take_instant_scrolls`] has anything to answer: one
    /// length check.
    #[must_use]
    pub fn has_instant_scrolls(&self) -> bool {
        !self.layout_state().instant_scrolls.is_empty()
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

    /// Drops every pending request and auto-scroll change whose container
    /// `keep` rejects: the paint build found no scroll container there to
    /// carry it.
    pub(crate) fn retain_scroll_requests(&mut self, keep: impl Fn(NodeId) -> bool) {
        let state = self.layout_state_mut();
        if !state.scroll_requests.is_empty() {
            state.scroll_requests.retain(|node, _| keep(*node));
        }
        if !state.auto_scroll_requests.is_empty() {
            state.auto_scroll_requests.retain(|(node, _)| keep(*node));
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::style::engine::StylesheetOrigin;
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

    /// On a circular axis the target stands anywhere in the period: the
    /// 100px scroller over 400px of content (`max` 300, period 400) keeps a
    /// target of 350, whose scrollport straddles the seam, and normalizes
    /// one before the start. The build re-admits it against the period it
    /// built: content shrunk to 200px (`max` 100, period 200) moves 350 to
    /// 150, past the new `max` and still inside the new period.
    #[test]
    fn a_circular_target_stands_anywhere_in_the_period_and_the_build_keeps_it_there() {
        let circular = || {
            let (mut document, node) = scroller();
            document.set_inline_style(node, "overscroll-behavior-y: circular");
            document.commit();
            (document, node)
        };
        let (mut document, node) = circular();
        assert_eq!(
            document.scroll_to_with(node, Vector2D::new(0.0, 350.0), ScrollBehavior::Smooth),
            Vector2D::new(0.0, 350.0),
        );
        let request = slot_request(&mut document, node).expect("the slot carries it");
        assert_eq!(request.target, Vector2D::new(0.0, 350.0), "past max, kept");
        assert_eq!(
            document.scroll_to_with(node, Vector2D::new(0.0, -30.0), ScrollBehavior::Instant),
            Vector2D::new(0.0, 370.0),
        );
        assert_eq!(document.scroll_offset(node), Vector2D::new(0.0, 370.0));

        let (mut document, node) = circular();
        document.scroll_to_with(node, Vector2D::new(0.0, 350.0), ScrollBehavior::Smooth);
        document.add_stylesheet(".content { height: 200px; }", StylesheetOrigin::Author);
        let request = slot_request(&mut document, node).expect("the slot carries it");
        assert_eq!(request.target, Vector2D::new(0.0, 150.0));
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

    /// A request that yields to a pointer is the painter's to carry out or
    /// not, so the document moves no offset for it, an instant one included;
    /// the frame carries it marked. It replaces one of its own kind and
    /// gives way to a request a script made, in either order.
    #[test]
    fn a_yielding_request_moves_no_offset_and_gives_way_to_a_scripts() {
        let (mut document, scroller) = scroller();
        assert!(document.scroll_to_yielding(
            scroller,
            Vector2D::new(0.0, 900.0),
            ScrollBehavior::Instant
        ));
        assert_eq!(document.scroll_offset(scroller), Vector2D::zero());
        let first = slot_request(&mut document, scroller).expect("the slot carries it");
        assert_eq!(first.target, Vector2D::new(0.0, 300.0), "clamped as any");
        assert_eq!(first.behavior, ScrollBehavior::Instant);
        assert!(first.yields_to_pointer);

        assert!(document.scroll_to_yielding(
            scroller,
            Vector2D::new(0.0, 100.0),
            ScrollBehavior::Smooth
        ));
        let second = slot_request(&mut document, scroller).expect("the slot carries it");
        assert!(second.serial > first.serial, "its own kind is replaced");
        assert!(second.yields_to_pointer);

        // A script's request replaces it, and is not replaced by the next.
        document.scroll_to_with(scroller, Vector2D::new(0.0, 200.0), ScrollBehavior::Smooth);
        assert!(!document.scroll_to_yielding(
            scroller,
            Vector2D::new(0.0, 50.0),
            ScrollBehavior::Smooth
        ));
        let third = slot_request(&mut document, scroller).expect("the slot carries it");
        assert_eq!(third.target, Vector2D::new(0.0, 200.0));
        assert!(!third.yields_to_pointer);

        let root = document.document_element().id();
        assert!(
            !document.scroll_to_yielding(root, Vector2D::zero(), ScrollBehavior::Smooth),
            "no scroll container, no request"
        );
    }

    /// An auto-scroll change moves no offset, needs a commit, and rides in
    /// the frame beside the slots, where the slot's own request stays free
    /// for a scroll made in the same turn.
    #[test]
    fn an_auto_scroll_change_travels_beside_the_slots_with_a_scroll_request() {
        let (mut document, scroller) = scroller();
        assert!(document.commit().auto_scroll_requests().is_empty());
        document.scroll_to_with(scroller, Vector2D::new(0.0, 40.0), ScrollBehavior::Instant);
        document.auto_scroll(scroller, 60.0, false, true);
        assert_eq!(
            document.scroll_offset(scroller),
            Vector2D::new(0.0, 40.0),
            "only the scroll request moved the document"
        );
        assert!(document.needs_render(), "the change needs a commit");
        let moved = document.pending_scroll_request(scroller).expect("kept");
        let frame = document.commit();
        let index = frame.slot_of(scroller).expect("the scroller has a slot");
        assert_eq!(
            frame.scroll_slots()[index as usize]
                .request
                .map(|request| request.serial),
            Some(moved),
            "the scroll request is still carried"
        );
        let [(node, request)] = frame.auto_scroll_requests() else {
            panic!("one change is carried");
        };
        assert_eq!(*node, scroller);
        assert!(request.serial > moved, "one counter orders both");
        assert_eq!(
            (request.rate, request.horizontal, request.stop_at_edge),
            (60.0, false, true)
        );
        assert_eq!(document.pending_auto_scroll(scroller), Some(*request));
    }

    #[test]
    fn the_latest_auto_scroll_change_wins_and_an_acknowledgement_ends_it() {
        let (mut document, scroller) = scroller();
        document.auto_scroll(scroller, 60.0, false, true);
        let first = document.pending_auto_scroll(scroller).expect("recorded");
        document.auto_scroll(scroller, 0.0, false, true);
        let second = document.pending_auto_scroll(scroller).expect("recorded");
        assert!(second.serial > first.serial, "serials are ordered");
        assert!(
            second.rate.abs() < f32::EPSILON,
            "the stop replaced the start"
        );
        assert_eq!(document.commit().auto_scroll_requests().len(), 1);

        document.acknowledge_auto_scroll(scroller, first.serial);
        assert_eq!(
            document.pending_auto_scroll(scroller),
            Some(second),
            "an older serial acknowledges nothing"
        );
        document.acknowledge_auto_scroll(scroller, second.serial);
        assert_eq!(document.pending_auto_scroll(scroller), None);
        document.set_inline_style(scroller, "opacity: 0.5");
        assert!(
            document.commit().auto_scroll_requests().is_empty(),
            "the next frame carries nothing"
        );
    }

    #[test]
    fn an_auto_scroll_change_with_no_scroll_container_goes_nowhere() {
        let (mut document, scroller) = scroller();
        let root = document.document_element().id();
        document.auto_scroll(root, 60.0, false, true);
        assert_eq!(document.pending_auto_scroll(root), None);
        assert!(!document.needs_render());

        // Dropped at the build once its container stops scrolling.
        document.auto_scroll(scroller, 60.0, false, true);
        document.set_inline_style(scroller, "overflow: visible");
        let frame = document.commit();
        assert!(frame.auto_scroll_requests().is_empty());
        assert_eq!(document.pending_auto_scroll(scroller), None);
    }

    #[test]
    fn an_auto_scroll_change_dies_with_its_node() {
        let (mut document, scroller) = scroller();
        document.auto_scroll(scroller, 60.0, false, true);
        assert_eq!(document.layout_state().auto_scroll_requests.len(), 1);
        document.drop_subtree(scroller);
        assert!(document.layout_state().auto_scroll_requests.is_empty());
    }
}
