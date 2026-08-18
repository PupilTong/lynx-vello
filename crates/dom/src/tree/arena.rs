//! The node arenas: identity, storage, and the table that maps one onto the
//! other.
//!
//! This is its own module so the raw slabs are unreachable from the rest of
//! the tree — a [`NodeId`] cannot be used as an arena key anywhere outside
//! this file, because the only things that can index a slab are the private
//! fields here. Everything else goes through [`TreeArenas::slot`], which is
//! what makes "an id names one node forever" an enforced property instead of
//! a convention.

use std::num::NonZeroU32;

use hughie::text::{TextContext, TextLayoutStore};
use hughie::tree::LayoutSlot;
use slab::Slab;

use crate::tree::node::Node;

/// A node's permanent identity: the Lynx element `unique_id`, which is also
/// the index of its entry in [`TreeArenas::slots`].
///
/// Handed out by a counter that only ever increases — the index of a freed
/// node is never given to another node, for the lifetime of the document.
/// An id therefore names one node forever, so a stale id can only ever
/// resolve to nothing; it can never come back attached to a stranger. That
/// is what lets the whole tree drop the aliasing defenses a recycling arena
/// needs (generation counters, epoch gates on retained frames), and it is
/// why a `NodeId` can be handed to script as Lynx's `unique_id` unchanged.
///
/// The price is a deliberate, bounded leak: one [`NodeSlot`]-sized table
/// entry per node ever created, four bytes, never reclaimed. The node itself
/// is not leaked — its arena storage is freed and reused immediately.
pub type NodeId = usize;

/// Where a live node's state actually sits in the three arenas.
///
/// This is the recycled half of the split: the arenas hand a freed slot to
/// the next node, while its [`NodeId`] retires. It is private on purpose —
/// nothing outside this crate may hold one, and no `NodeId` may be used as
/// one without going through [`TreeArenas::slot`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct NodeSlot(NonZeroU32);

impl NodeSlot {
    /// Stored biased by one so `Option<NodeSlot>` fits in four bytes — the
    /// table has one entry per node ever created, so its width is the whole
    /// cost of never reusing an id.
    #[inline]
    fn from_arena_key(key: usize) -> Self {
        let biased = u32::try_from(key + 1).expect("a document holds fewer than u32::MAX nodes");
        Self(NonZeroU32::new(biased).expect("the bias makes the key non-zero"))
    }

    #[inline]
    const fn arena_key(self) -> usize {
        self.0.get() as usize - 1
    }
}

pub(crate) const DOCUMENT_NODE_ID: NodeId = 0;
pub(crate) const DOCUMENT_ELEMENT_NODE_ID: NodeId = 1;
const INITIAL_NODE_CAPACITY: usize = 8;

pub(crate) enum PayloadSlot<T> {
    Document,
    ShadowRoot,
    Node(T),
}

/// The fixed-address, document-owned arena set.
///
/// `slots` maps the public [`NodeId`] onto the arena slot the node's state
/// occupies; `nodes` and `payloads` are both keyed by that slot and
/// insert/remove in exactly the same order, each asserting that its own free
/// list returns the same key. `DocumentLayoutState::nodes` is the third
/// arena in that lockstep — it lives beside the tree rather than in it, and
/// takes an already-resolved [`NodeSlot`] for the same reason.
pub(crate) struct TreeArenas<T> {
    nodes: Slab<Node<T>>,
    payloads: Slab<PayloadSlot<T>>,
    slots: Vec<Option<NodeSlot>>,
}

impl<T> TreeArenas<T> {
    pub(crate) fn new() -> Self {
        Self {
            nodes: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            payloads: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            slots: Vec::with_capacity(INITIAL_NODE_CAPACITY),
        }
    }

    /// The arena slot a live node occupies, or `None` for an id that was
    /// freed or never issued. Out-of-range ids answer `None` too: a script
    /// can name any integer, and naming a stranger is a script error rather
    /// than a crash.
    #[inline]
    pub(crate) fn slot(&self, id: NodeId) -> Option<NodeSlot> {
        self.slots.get(id).copied().flatten()
    }

    #[inline]
    pub(crate) fn live_slot(&self, id: NodeId) -> NodeSlot {
        self.slot(id)
            .expect("stale NodeId passed to a live-node read")
    }

    #[inline]
    pub(crate) fn get(&self, id: NodeId) -> Option<&Node<T>> {
        let slot = self.slot(id)?;
        Some(self.at(slot))
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, id: NodeId) -> Option<&mut Node<T>> {
        let slot = self.slot(id)?;
        Some(self.at_mut(slot))
    }

    #[inline]
    pub(crate) fn live(&self, id: NodeId) -> &Node<T> {
        self.get(id)
            .expect("stale NodeId passed to a live-node read")
    }

    #[inline]
    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.slot(id).is_some()
    }

    #[inline]
    pub(crate) fn at(&self, slot: NodeSlot) -> &Node<T> {
        self.nodes
            .get(slot.arena_key())
            .expect("a live slot always selects its node")
    }

    #[inline]
    fn at_mut(&mut self, slot: NodeSlot) -> &mut Node<T> {
        self.nodes
            .get_mut(slot.arena_key())
            .expect("a live slot always selects its node")
    }

    #[inline]
    pub(crate) fn payload_at(&self, slot: NodeSlot) -> &PayloadSlot<T> {
        self.payloads
            .get(slot.arena_key())
            .expect("live primary node must have matching payload-arena state")
    }

    /// The node arena itself, for `Debug` only — its keys are arena slots.
    pub(crate) const fn nodes(&self) -> &Slab<Node<T>> {
        &self.nodes
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(id, slot)| slot.map(|_| id))
    }

    /// Retires an id. The entry is emptied in place and never handed out
    /// again — this is the deliberate leak.
    fn retire_id(&mut self, id: NodeId) {
        let entry = self
            .slots
            .get_mut(id)
            .expect("a freed node always has a table entry");
        *entry = None;
    }

    /// Files a node in both tree arenas under one freshly issued id, and
    /// reports the slot the caller must file the layout arena under.
    ///
    /// The slot is whatever the free list offers — recycled the instant its
    /// previous node was freed — while the id counts upward and is never
    /// reused. The two are related only through `slots`.
    #[expect(
        clippy::inline_always,
        reason = "keep the synchronized slab inserts in the node-allocation hot path"
    )]
    #[inline(always)]
    pub(crate) fn insert_node(
        &mut self,
        payload: PayloadSlot<T>,
        make: impl FnOnce(*mut Self, NodeId) -> Node<T>,
    ) -> (NodeId, NodeSlot) {
        let owner = std::ptr::from_mut::<Self>(self);
        let slot = NodeSlot::from_arena_key(self.nodes.vacant_key());
        // The id is claimed here but published last: until its table entry
        // exists the id resolves to nothing, so an unwind out of `make` or
        // out of either insert leaves an unissued id rather than a live one
        // pointing at an empty slot.
        let id = self.slots.len();
        let node = make(owner, id);
        assert_eq!(self.nodes.insert(node), slot.arena_key());
        assert_eq!(self.payloads.vacant_key(), slot.arena_key());
        assert_eq!(self.payloads.insert(payload), slot.arena_key());
        self.slots.push(Some(slot));
        (id, slot)
    }

    /// Empties both tree arenas of one node and retires its id.
    ///
    /// The slot goes back on the free list; the id does not. Anything still
    /// holding it — a retained frame, a script handle whose finalizer has not
    /// run yet — resolves to nothing from here on.
    pub(crate) fn remove_node(&mut self, id: NodeId) -> (NodeSlot, Node<T>, PayloadSlot<T>) {
        let slot = self
            .slot(id)
            .expect("subtree links always resolve while removing");
        let node = self
            .nodes
            .try_remove(slot.arena_key())
            .expect("a live slot always selects its node");
        let payload = self
            .payloads
            .try_remove(slot.arena_key())
            .expect("removed element/text node must have payload-arena state");
        self.retire_id(id);
        (slot, node, payload)
    }
}

#[derive(Default)]
pub(crate) struct NodeLayoutState {
    pub(crate) slot: LayoutSlot,
    pub(crate) text: Option<Box<TextLayoutStore>>,
    pub(crate) scroll_offset: euclid::default::Vector2D<f32>,
}

pub(crate) struct DocumentLayoutState {
    nodes: Slab<NodeLayoutState>,
    pub(crate) text_context: Option<Box<TextContext>>,
}

impl DocumentLayoutState {
    pub(crate) fn new() -> Self {
        Self {
            nodes: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            text_context: None,
        }
    }

    pub(crate) fn insert(&mut self, slot: NodeSlot) {
        assert_eq!(self.nodes.vacant_key(), slot.arena_key());
        assert_eq!(
            self.nodes.insert(NodeLayoutState::default()),
            slot.arena_key()
        );
    }

    pub(crate) fn remove(&mut self, slot: NodeSlot) {
        self.nodes
            .try_remove(slot.arena_key())
            .expect("removed node must have layout-arena state");
    }

    #[inline]
    pub(crate) fn at(&self, slot: NodeSlot) -> &NodeLayoutState {
        self.nodes
            .get(slot.arena_key())
            .expect("live primary node must have matching layout-arena state")
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (usize, &mut NodeLayoutState)> {
        self.nodes.iter_mut()
    }

    #[inline]
    pub(crate) fn at_mut(&mut self, slot: NodeSlot) -> &mut NodeLayoutState {
        self.nodes
            .get_mut(slot.arena_key())
            .expect("live primary node must have matching layout-arena state")
    }

    pub(crate) fn text_parts(
        &mut self,
        slot: NodeSlot,
    ) -> (&mut TextContext, &mut TextLayoutStore) {
        let Self {
            nodes,
            text_context,
        } = self;
        let context = text_context
            .get_or_insert_with(|| Box::new(TextContext::new()))
            .as_mut();
        let artifacts = nodes
            .get_mut(slot.arena_key())
            .expect("live node must have layout-arena state")
            .text
            .get_or_insert_with(|| Box::new(TextLayoutStore::default()))
            .as_mut();
        (context, artifacts)
    }

    pub(crate) fn clear_layout_cache(&mut self, slot: NodeSlot) {
        let node = self
            .nodes
            .get_mut(slot.arena_key())
            .expect("live node must have layout-arena state");
        node.slot.clear_layout_cache();
        if let Some(artifacts) = node.text.as_deref_mut() {
            artifacts.invalidate();
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// The one property no caller can check for itself: the two tree arenas
    /// hand out, and give back, the same key for the same node. The layout
    /// arena is the third partner in that lockstep and asserts it in
    /// [`DocumentLayoutState::insert`].
    #[test]
    fn the_tree_arenas_stay_key_aligned_across_reuse() {
        let mut arenas: TreeArenas<u32> = TreeArenas::new();
        let mut layout = DocumentLayoutState::new();
        let mut issued = Vec::new();
        for payload in 0..4 {
            let (id, slot) = arenas.insert_node(PayloadSlot::Node(payload), |owner, id| {
                Node::new_text(owner, id, String::new())
            });
            layout.insert(slot);
            assert_eq!(arenas.nodes.len(), arenas.payloads.len());
            issued.push((id, slot));
        }

        let (freed_id, freed_slot) = issued[1];
        let (slot, _, payload) = arenas.remove_node(freed_id);
        layout.remove(slot);
        assert_eq!(slot, freed_slot);
        assert!(matches!(payload, PayloadSlot::Node(1)));
        assert_eq!(arenas.nodes.len(), arenas.payloads.len());
        assert_eq!(arenas.slot(freed_id), None);

        let (next_id, next_slot) = arenas.insert_node(PayloadSlot::Node(9), |owner, id| {
            Node::new_text(owner, id, String::new())
        });
        layout.insert(next_slot);
        assert_eq!(next_slot, freed_slot, "the freed storage comes back");
        assert_eq!(
            next_id,
            issued.last().expect("ids were issued").0 + 1,
            "the id after it does not"
        );
    }

    /// An id nobody ever issued resolves to nothing rather than indexing
    /// past the table. Script can name any integer, and the host boundary
    /// turns this `None` into a JavaScript error instead of a crash.
    #[test]
    fn an_id_that_was_never_issued_resolves_to_nothing() {
        let mut arenas: TreeArenas<()> = TreeArenas::new();
        let (id, _) = arenas.insert_node(PayloadSlot::Node(()), |owner, id| {
            Node::new_text(owner, id, String::new())
        });

        for never_issued in [id + 1, id + 2, 999_999, usize::MAX] {
            assert_eq!(arenas.slot(never_issued), None);
            assert!(arenas.get(never_issued).is_none());
            assert!(!arenas.contains(never_issued));
        }
    }

    /// The table entry is the whole per-node cost of never reusing an id.
    #[test]
    fn the_id_table_entry_is_four_bytes() {
        assert_eq!(size_of::<Option<NodeSlot>>(), 4);
    }
}
