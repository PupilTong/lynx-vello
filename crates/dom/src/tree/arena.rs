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
/// Zero is never issued — it is the reserved "no node" value, and index zero
/// of the table stays empty for the life of the document.
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
///
/// The key is the arena index. Key zero is permanently occupied by the
/// reservation [`TreeArenas::reserve_zero`] makes, so no live node ever sits
/// there and [`NonZeroU32`] is the key's own type rather than an encoding.
///
/// The generation is what makes a slot *checkable*. A slot is the recycled
/// half of the split, so on its own it is exactly the reusable handle this
/// whole change exists to get rid of: hold one across a free and the next
/// node lands underneath it. The arena bumps that key's generation on every
/// free, so a slot taken before the free no longer matches and every getter
/// that takes one answers `None` instead of a stranger.
///
/// The generation is **not** stored in [`TreeArenas::slots`] — that table
/// only ever maps live ids, where the generation is current by construction,
/// so it stays four bytes per entry and the deliberate leak does not grow.
/// It is rebuilt by [`TreeArenas::slot`] and carried only in locals.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct NodeSlot {
    key: NonZeroU32,
    generation: u32,
}

impl NodeSlot {
    /// The storage position, with the generation dropped. Only for asking
    /// "is this the same physical slot" — nothing accepts a bare key back.
    #[inline]
    pub(crate) const fn arena_key(self) -> usize {
        self.key.get() as usize
    }
}

/// The reserved id and arena key. Nothing lives here: it keeps zero out of
/// both spaces so it can mean "no node" in each.
const RESERVED: NodeId = 0;

pub(crate) const DOCUMENT_NODE_ID: NodeId = 1;
pub(crate) const DOCUMENT_ELEMENT_NODE_ID: NodeId = 2;
const INITIAL_NODE_CAPACITY: usize = 8;

pub(crate) enum PayloadSlot<T> {
    /// Arena key zero's placeholder. Unreachable: no id resolves to it.
    Reserved,
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
    /// `NodeId` -> the arena key its node occupies, for live ids only.
    /// Four bytes per node ever created, never reclaimed.
    slots: Vec<Option<NonZeroU32>>,
    /// Arena key -> how many times that key has been handed to a new node.
    /// Indexed by *key*, so it is bounded by the peak number of live nodes,
    /// not by how many ids the document has ever issued.
    generations: Vec<u32>,
}

impl<T> TreeArenas<T> {
    pub(crate) fn new() -> Self {
        Self {
            nodes: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            payloads: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            slots: Vec::with_capacity(INITIAL_NODE_CAPACITY),
            generations: Vec::with_capacity(INITIAL_NODE_CAPACITY),
        }
    }

    /// Takes id zero and arena key zero out of circulation, before any node
    /// is filed.
    ///
    /// Called once, on the boxed arena set, so the placeholder's owner
    /// backpointer is the address every later node will also carry. Nothing
    /// resolves to the placeholder — `slots[0]` stays `None` — so it exists
    /// only to keep the key off the slab's free list, which is what lets
    /// [`NodeSlot`] be a plain [`NonZeroU32`].
    pub(crate) fn reserve_zero(&mut self) {
        debug_assert!(self.slots.is_empty(), "zero is reserved before any node");
        let owner = std::ptr::from_mut::<Self>(self);
        let placeholder = NodeSlot {
            key: NonZeroU32::new(1).expect("one is non-zero"),
            generation: 0,
        };
        assert_eq!(
            self.nodes
                .insert(Node::new_text(owner, RESERVED, placeholder, String::new())),
            RESERVED
        );
        assert_eq!(self.payloads.insert(PayloadSlot::Reserved), RESERVED);
        self.slots.push(None);
        self.generations.push(0);
    }

    /// The arena slot a live node occupies, or `None` for an id that was
    /// freed or never issued. Out-of-range ids answer `None` too: a script
    /// can name any integer, and naming a stranger is a script error rather
    /// than a crash.
    #[inline]
    pub(crate) fn slot(&self, id: NodeId) -> Option<NodeSlot> {
        let key = self.slots.get(id).copied().flatten()?;
        Some(NodeSlot {
            key,
            generation: self.generations[key.get() as usize],
        })
    }

    /// Whether a slot still names the node it was taken for.
    ///
    /// False for a slot held across the free of its node, whether or not the
    /// key has since been handed to someone else — which is the case a bare
    /// arena key cannot distinguish.
    #[inline]
    pub(crate) fn slot_is_current(&self, slot: NodeSlot) -> bool {
        self.generations
            .get(slot.arena_key())
            .is_some_and(|generation| *generation == slot.generation)
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

    /// The node a slot names, or `None` if the slot is stale.
    ///
    /// The generation compare is the whole check. Its address depends only on
    /// the key the caller already holds, so it issues alongside the node load
    /// rather than after it — unlike the id-to-slot hop, it does not lengthen
    /// the dependency chain a tree walk is bound by.
    #[inline]
    pub(crate) fn try_at(&self, slot: NodeSlot) -> Option<&Node<T>> {
        if !self.slot_is_current(slot) {
            return None;
        }
        self.nodes.get(slot.arena_key())
    }

    #[inline]
    pub(crate) fn at(&self, slot: NodeSlot) -> &Node<T> {
        self.try_at(slot)
            .expect("stale NodeSlot: its node was freed after the slot was taken")
    }

    #[inline]
    fn at_mut(&mut self, slot: NodeSlot) -> &mut Node<T> {
        assert!(
            self.slot_is_current(slot),
            "stale NodeSlot: its node was freed after the slot was taken"
        );
        self.nodes
            .get_mut(slot.arena_key())
            .expect("a current slot always selects its node")
    }

    #[inline]
    pub(crate) fn payload_at(&self, slot: NodeSlot) -> &PayloadSlot<T> {
        assert!(
            self.slot_is_current(slot),
            "stale NodeSlot: its node was freed after the slot was taken"
        );
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
        make: impl FnOnce(*mut Self, NodeId, NodeSlot) -> Node<T>,
    ) -> (NodeId, NodeSlot) {
        let owner = std::ptr::from_mut::<Self>(self);
        let arena_key = self.nodes.vacant_key();
        let key = u32::try_from(arena_key).expect("a document holds fewer than u32::MAX nodes");
        let key = NonZeroU32::new(key).expect("arena key zero is reserved and never allocated");
        // A key the slab has never handed out yet starts at generation zero;
        // a recycled one carries whatever its last free bumped it to.
        if arena_key == self.generations.len() {
            self.generations.push(0);
        }
        let slot = NodeSlot {
            key,
            generation: self.generations[arena_key],
        };
        // The id is claimed here but published last: until its table entry
        // exists the id resolves to nothing, so an unwind out of `make` or
        // out of either insert leaves an unissued id rather than a live one
        // pointing at an empty slot.
        let id = self.slots.len();
        let node = make(owner, id, slot);
        assert_eq!(self.nodes.insert(node), arena_key);
        assert_eq!(self.payloads.vacant_key(), arena_key);
        assert_eq!(self.payloads.insert(payload), arena_key);
        self.slots.push(Some(key));
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
        // Every slot taken for this node before now stops matching, so a held
        // one cannot follow the key to whoever the slab hands it to next.
        self.generations[slot.arena_key()] = self.generations[slot.arena_key()].wrapping_add(1);
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
        let mut nodes = Slab::with_capacity(INITIAL_NODE_CAPACITY);
        // Key zero is reserved in every arena, so all three stay aligned.
        assert_eq!(nodes.insert(NodeLayoutState::default()), RESERVED);
        Self {
            nodes,
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
        arenas.reserve_zero();
        let mut layout = DocumentLayoutState::new();
        let mut issued = Vec::new();
        for payload in 0..4 {
            let (id, slot) = arenas.insert_node(PayloadSlot::Node(payload), |owner, id, slot| {
                Node::new_text(owner, id, slot, String::new())
            });
            layout.insert(slot);
            assert_eq!(arenas.nodes.len(), arenas.payloads.len());
            issued.push((id, slot));
        }

        let (freed_id, freed_slot) = issued[1];
        let (slot, _, payload) = arenas.remove_node(freed_id);
        layout.remove(slot);
        assert_eq!(slot.arena_key(), freed_slot.arena_key());
        assert!(matches!(payload, PayloadSlot::Node(1)));
        assert_eq!(arenas.nodes.len(), arenas.payloads.len());
        assert_eq!(arenas.slot(freed_id), None);

        let (next_id, next_slot) = arenas.insert_node(PayloadSlot::Node(9), |owner, id, slot| {
            Node::new_text(owner, id, slot, String::new())
        });
        layout.insert(next_slot);
        assert_eq!(
            next_slot.arena_key(),
            freed_slot.arena_key(),
            "the freed storage comes back"
        );
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
        arenas.reserve_zero();
        let (id, _) = arenas.insert_node(PayloadSlot::Node(()), |owner, id, slot| {
            Node::new_text(owner, id, slot, String::new())
        });
        assert_eq!(id, 1, "zero is reserved, so the first issued id is one");

        for never_issued in [RESERVED, id + 1, id + 2, 999_999, usize::MAX] {
            assert_eq!(arenas.slot(never_issued), None);
            assert!(arenas.get(never_issued).is_none());
            assert!(!arenas.contains(never_issued));
        }
    }

    /// The table entry is the whole per-node cost of never reusing an id, so
    /// it stores the bare key. A generation-carrying `NodeSlot` is twice the
    /// width and never goes in the table — it is rebuilt per lookup.
    #[test]
    fn the_id_table_entry_is_four_bytes() {
        assert_eq!(size_of::<Option<NonZeroU32>>(), 4);
        assert_eq!(size_of::<Option<NodeSlot>>(), 8);
    }

    /// The generation is what stops a slot from being the reusable handle
    /// this whole split exists to remove.
    #[test]
    fn a_slot_held_across_its_nodes_free_is_refused_even_once_reused() {
        let mut arenas: TreeArenas<u32> = TreeArenas::new();
        arenas.reserve_zero();
        let (doomed, held) = arenas.insert_node(PayloadSlot::Node(1), |owner, id, slot| {
            Node::new_text(owner, id, slot, String::new())
        });
        assert!(arenas.slot_is_current(held));
        assert!(arenas.try_at(held).is_some());

        arenas.remove_node(doomed);
        assert!(
            !arenas.slot_is_current(held),
            "freeing invalidates every slot taken for that node"
        );
        assert!(arenas.try_at(held).is_none());

        let (_, reused) = arenas.insert_node(PayloadSlot::Node(2), |owner, id, slot| {
            Node::new_text(owner, id, slot, String::new())
        });
        assert_eq!(
            reused.arena_key(),
            held.arena_key(),
            "the storage really did come back"
        );
        assert_ne!(reused, held, "but the old slot does not name it");
        assert!(
            arenas.try_at(held).is_none(),
            "a stale slot must not follow the key to its new occupant"
        );
        assert!(arenas.try_at(reused).is_some());
    }
}
