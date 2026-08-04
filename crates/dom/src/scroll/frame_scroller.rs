//! Scrolling a published [`Frame`] ahead of the document.
//!
//! [`Document::handle_input`](crate::Document) is the authoritative path: it
//! hit tests the live tree, moves the offsets this module's parent owns, and
//! everything downstream sees the result. Its cost is a full round trip —
//! restyle, relayout, rebuild the frame — before a single pixel moves, which
//! is fine in process and is not fine when the document is on another thread.
//!
//! So a renderer holding a [`Frame`] can answer the gesture itself. The frame
//! carries a [`ScrollNode`](crate::ScrollNode) arena and everything hit
//! testing reads, so [`FrameScroller`] routes the event, clamps against the
//! scrolling area the document already measured, and hands the resulting
//! offsets to the painter — all without the document. Pixels move on the
//! thread that owns them.
//!
//! **The document stays authoritative.** Every offset reached here is
//! reported back as a [`ScrollUpdate`], which the host applies with
//! [`Document::scroll_to`](crate::Document::scroll_to), so `scrollTop`, event
//! targeting, and the next relayout all agree. Handing it back is absolute
//! rather than a delta: this side owns the position it painted, so a message
//! that arrives late or twice lands on the same place instead of scrolling
//! twice as far.
//!
//! The gesture recognizer is *shared*, not reimplemented:
//! [`crate::input::Drag`] is the same value `Document::handle_input` latches,
//! spending the same slop toll. Two copies would mean the same drag travelling
//! different distances depending on which side answered it.
//!
//! What is deliberately not replicated:
//!
//! - **No layout.** Scrolling here moves content the document already laid out, clamps at the
//!   scrolling area as last measured, and cannot make `position: sticky` stick (it does not stick
//!   in this engine anyway).
//! - **No momentum, rubber-band, or snapping** — the same limits [`crate::scroll`] records, for the
//!   same reason: neither side owns a clock.

use std::collections::HashMap;

use crate::input::{Drag, InputEvent, InputKind, PointerKind, PointerPhase};
use crate::visual::frame::Frame;
use crate::{NodeId, Point2D, Vector2D};

/// One offset a renderer reached, for the document to adopt.
///
/// Absolute rather than a delta, and stamped with the frame's removal epoch:
/// a `NodeId` carries no generation, so an id resolved against a frame and
/// applied later can name a stranger. The holder of the document compares
/// [`Self::epoch`] against
/// [`Document::node_removal_epoch`](crate::Document::node_removal_epoch) and
/// drops the update when they disagree — the same rule hit testing enforces
/// synchronously.
#[derive(Debug, Clone, Copy)]
pub struct ScrollUpdate {
    /// The scroll container to move.
    pub node: NodeId,
    /// Where it landed, already clamped.
    pub offset: Vector2D<f32>,
    /// Monotone per scroller, so the document can report what it has applied.
    pub seq: u64,
    /// The frame's removal epoch when `node` was resolved.
    pub epoch: u64,
}

/// What routing one event through a frame resolved to.
#[derive(Debug, Default)]
pub struct FrameInput {
    /// Offsets this side moved to, to hand back to the document.
    pub scrolled: Vec<ScrollUpdate>,
    /// Whether this side is driving the gesture.
    ///
    /// The caller forwards the event to the document with
    /// [`InputEvent::default_prevented`] set to this, so the document
    /// resolves a target without also scrolling. A press that may yet be a
    /// tap is deliberately *not* claimed: ownership begins at the move that
    /// actually scrolls, which is early enough, because the document's own
    /// drag skips every step this side prevents.
    pub owns_gesture: bool,
}

/// One offset reached here that the document has not confirmed yet.
#[derive(Debug, Clone, Copy)]
struct Pending {
    offset: Vector2D<f32>,
    seq: u64,
    epoch: u64,
}

/// A renderer's live scroll offsets for the frame it holds.
#[derive(Debug, Default)]
pub(crate) struct FrameScroller {
    /// Live offset per scroll node, indexed to match the frame's arena.
    offsets: Vec<Vector2D<f32>>,
    /// Positions reached here the document has not confirmed, keyed by node
    /// so they survive a frame rebuild — arena *indices* do not.
    unconfirmed: HashMap<NodeId, Pending>,
    /// Gestures currently latched to a pointer.
    drags: Vec<Drag>,
    next_seq: u64,
}

impl FrameScroller {
    /// Rebases onto a newly published frame.
    ///
    /// `confirmed_seq` is the highest update the document had applied when it
    /// built this frame. A node whose pending update is covered adopts the
    /// frame's baked offset — which is how a programmatic `scrollTo` from
    /// script, or a clamp after relayout, reaches the screen. One still
    /// waiting keeps the position this side painted, so scrolling does not
    /// visibly snap back to a stale offset. One whose epoch has moved is
    /// dropped: its key may name a stranger now.
    pub(crate) fn adopt(&mut self, frame: &Frame, confirmed_seq: u64) {
        let epoch = frame.node_removal_epoch();
        self.unconfirmed
            .retain(|_, pending| pending.seq > confirmed_seq && pending.epoch == epoch);
        self.offsets.clear();
        self.offsets.extend(frame.scrolls().iter().map(|node| {
            self.unconfirmed
                .get(&node.node)
                .map_or(node.baked_offset, |pending| node.clamp(pending.offset))
        }));
    }

    /// The offsets to paint with.
    ///
    /// Empty when every one equals what the frame already baked, which is the
    /// whole of a page nobody has scrolled. Not just tidiness: the walk skips
    /// its per-item scroll-chain lookup entirely for an empty slice, and
    /// Lynx's UA cascade makes almost every element a scroll container, so
    /// those chains are long and almost always contribute nothing.
    pub(crate) fn paint_offsets(&self, frame: &Frame) -> &[Vector2D<f32>] {
        let baked = frame
            .scrolls()
            .iter()
            .zip(&self.offsets)
            .all(|(node, offset)| node.baked_offset == *offset);
        if baked { &[] } else { &self.offsets }
    }

    #[cfg(test)]
    pub(crate) fn offsets_for_testing(&self) -> &[Vector2D<f32>] {
        &self.offsets
    }

    /// Routes one event against `frame`.
    pub(crate) fn handle_input(&mut self, frame: &Frame, event: &InputEvent) -> FrameInput {
        if !event.is_finite() {
            return FrameInput::default();
        }
        match event.kind {
            InputKind::Wheel { delta, mode } => FrameInput {
                scrolled: self.wheel(frame, event.position, delta, mode),
                // Scrolling is wholly this side's job once a host routes here,
                // so the document must never run its own wheel default —
                // whether or not anything moved.
                owns_gesture: true,
            },
            InputKind::Pointer { id, device, phase } => {
                self.pointer(frame, event.position, id, device, phase)
            }
        }
    }

    fn wheel(
        &mut self,
        frame: &Frame,
        point: Point2D<f32>,
        delta: Vector2D<f32>,
        mode: crate::input::DeltaMode,
    ) -> Vec<ScrollUpdate> {
        let Some(index) = frame.scroll_target(point, &self.offsets) else {
            return Vec::new();
        };
        let delta = Self::resolve_delta(frame, index, delta, mode);
        self.apply(frame, index, delta)
    }

    /// A wheel delta in the scroller's own CSS px, resolving the same units
    /// [`Document::handle_input`](crate::Document) would.
    fn resolve_delta(
        frame: &Frame,
        index: usize,
        delta: Vector2D<f32>,
        mode: crate::input::DeltaMode,
    ) -> Vector2D<f32> {
        match mode {
            crate::input::DeltaMode::Pixel => delta,
            crate::input::DeltaMode::Line => delta * crate::input::WHEEL_LINE_PX,
            crate::input::DeltaMode::Page => {
                // A page is the scrollport, which the frame reports as the
                // span the offset may cover plus what is already visible.
                let node = &frame.scrolls()[index];
                let page = node.max_offset;
                Vector2D::new(delta.x * page.x.max(1.0), delta.y * page.y.max(1.0))
            }
        }
    }

    fn pointer(
        &mut self,
        frame: &Frame,
        point: Point2D<f32>,
        pointer: crate::input::PointerId,
        device: PointerKind,
        phase: PointerPhase,
    ) -> FrameInput {
        // Only touch and pen drag to scroll — a mouse scrolls with its wheel,
        // in every browser and in `dom::input`.
        if !matches!(device, PointerKind::Touch | PointerKind::Pen) {
            return FrameInput::default();
        }
        match phase {
            PointerPhase::Down => {
                // A second press with a live id supersedes the first.
                self.drags.retain(|drag| drag.pointer != pointer);
                if let Some(index) = frame.scroll_target(point, &self.offsets) {
                    self.drags.push(Drag::new(
                        pointer,
                        frame.scrolls()[index].node,
                        frame.node_removal_epoch(),
                        point,
                    ));
                }
                FrameInput::default()
            }
            PointerPhase::Move => {
                let scrolled = self.drag_step(frame, point, pointer);
                FrameInput {
                    scrolled,
                    owns_gesture: self.is_scrolling(pointer),
                }
            }
            // `PointerPhase` is non-exhaustive; anything that is not a press
            // or a move ends the gesture, the right default for a phase this
            // module does not recognize.
            _ => {
                let owns_gesture = self.is_scrolling(pointer);
                self.drags.retain(|drag| drag.pointer != pointer);
                FrameInput {
                    scrolled: Vec::new(),
                    owns_gesture,
                }
            }
        }
    }

    fn is_scrolling(&self, pointer: crate::input::PointerId) -> bool {
        self.drags
            .iter()
            .any(|drag| drag.pointer == pointer && drag.is_scrolling())
    }

    fn drag_step(
        &mut self,
        frame: &Frame,
        point: Point2D<f32>,
        pointer: crate::input::PointerId,
    ) -> Vec<ScrollUpdate> {
        let Some(drag) = self.drags.iter_mut().find(|drag| drag.pointer == pointer) else {
            return Vec::new();
        };
        let Some(movement) = drag.step(point) else {
            return Vec::new();
        };
        let scroller = drag.scroller;

        // The container was latched at pointer-down; find where it sits in
        // *this* frame's arena, which a republish may have renumbered.
        let Some(index) = frame
            .scrolls()
            .iter()
            .position(|node| node.node == scroller)
        else {
            // Gone from the frame (removed, or restyled out of
            // scroll-container-hood): the gesture dies with it rather than
            // scrolling a stranger.
            self.drags.retain(|drag| drag.pointer != pointer);
            return Vec::new();
        };
        // Content follows the finger, so the scroll position moves against it.
        self.apply(frame, index, -movement)
    }

    /// Scrolls `index` by `delta`, chaining the unconsumed remainder outwards.
    ///
    /// Chaining follows the containing-block chain the frame recorded, which
    /// is the chain [`crate::scroll`] walks: a box scrolls with exactly the
    /// ancestors that clip it.
    fn apply(&mut self, frame: &Frame, index: usize, delta: Vector2D<f32>) -> Vec<ScrollUpdate> {
        let mut remaining = delta;
        let mut updates = Vec::new();
        let scrolls = frame.scrolls();
        let mut next = Some(index);

        while let Some(index) = next {
            let node = &scrolls[index];
            next = node.parent;
            if remaining.x == 0.0 && remaining.y == 0.0 {
                break;
            }
            let wanted = node.user_scrollable.mask(remaining);
            if wanted.x == 0.0 && wanted.y == 0.0 {
                continue;
            }
            let before = self.offsets[index];
            let after = node.clamp(before + wanted);
            let consumed = after - before;
            if consumed.x == 0.0 && consumed.y == 0.0 {
                continue;
            }
            self.offsets[index] = after;
            remaining -= consumed;
            self.next_seq += 1;
            let epoch = frame.node_removal_epoch();
            self.unconfirmed.insert(
                node.node,
                Pending {
                    offset: after,
                    seq: self.next_seq,
                    epoch,
                },
            );
            updates.push(ScrollUpdate {
                node: node.node,
                offset: after,
                seq: self.next_seq,
                epoch,
            });
        }
        updates
    }
}
