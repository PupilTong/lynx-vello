//! The node arenas: identity, storage, and the table that maps one onto the
//! other.
//!
//! This is its own module so the raw slabs are unreachable from the rest of
//! the tree — a [`NodeId`] cannot be used as an arena key anywhere outside
//! this file, because the only things that can index a slab are the private
//! fields here. Everything else goes through [`TreeArenas::slot`], which is
//! what makes "an id names one node forever" an enforced property instead of
//! a convention.

use core::fmt;
use std::hint::likely;
use std::num::{NonZeroU32, NonZeroU64};

use hughie::text::{TextContext, TextLayoutStore};
use hughie::tree::LayoutSlot;
use slab::Slab;

use crate::tree::node::Node;

/// A node's identity *and* the position its state occupies: the arena key it
/// lives at, plus the generation that key was at when the handle was made.
///
/// There is no second id space. An earlier design gave every node a
/// monotonically increasing id and mapped it onto the arena key through a side
/// table, so that a freed id could never come back attached to a stranger.
/// That bought the guarantee with a *dependent* load on every tree-walk step —
/// follow a link, index the table, index the arena — and measured expensive
/// enough across the bench set that the table is gone. The generation buys the
/// same guarantee for the price of a compare: the arena bumps a key's
/// generation on every free, so a handle taken before that free no longer
/// matches and every getter answers `None` instead of a stranger.
///
/// Key zero is permanently occupied by [`TreeArenas::reserve_zero`], so no
/// live node sits there and [`NonZeroU32`] is the key's own type rather than an
/// encoding — which is what lets `Option<NodeId>` stay four bytes wide for the
/// tree's own links.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(NonZeroU64);

/// A handle *is* its storage position now, so the two names are one type.
/// Kept so the tree can still say "slot" where it means "where this lives".
pub(crate) type NodeSlot = NodeId;

/// Bits the generation gets when a handle is packed into one integer. The
/// script boundary is an `f64`, which represents every integer below 2^53
/// exactly, and the key already claims 32 of those bits.
const GENERATION_BITS: u32 = 21;

/// Where the key stops and the generation starts inside the handle.
const KEY_BITS: u32 = 32;
const KEY_MASK: u64 = (1 << KEY_BITS) - 1;

impl NodeId {
    /// The arena index, with the generation masked off. Only for asking "is
    /// this the same physical slot" and for indexing inside this module —
    /// nothing accepts a bare key back as a handle.
    // Neither cast below can lose anything, and clippy can see it: the key is
    // masked to `KEY_BITS`, and the generation is what is left after shifting
    // that many bits off a `u64`. No `expect` — it would go unfulfilled.
    #[inline]
    pub(crate) const fn arena_key(self) -> usize {
        (self.0.get() & KEY_MASK) as usize
    }

    #[inline]
    const fn generation(self) -> u32 {
        (self.0.get() >> KEY_BITS) as u32
    }

    /// The handle as one integer, for the places that can only carry a number:
    /// script's `unique_id`, Stylo's `OpaqueNode`, the image registry's owner
    /// key. Round-trips through [`NodeId::from_bits`].
    ///
    /// Free: the handle *is* that integer.
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        self.0.get()
    }

    /// The handle an integer names, or `None` if it names no handle shape at
    /// all. A live-node check still has to go through the arena: this only
    /// rejects integers that could never have been a handle.
    #[must_use]
    pub const fn from_bits(bits: u64) -> Option<Self> {
        if bits & KEY_MASK == 0 || (bits >> KEY_BITS) >> GENERATION_BITS != 0 {
            return None;
        }
        match NonZeroU64::new(bits) {
            Some(bits) => Some(Self(bits)),
            None => None,
        }
    }

    #[inline]
    const fn at_key(key: NonZeroU32, generation: u32) -> Self {
        let bits = ((generation as u64) << KEY_BITS) | key.get() as u64;
        match NonZeroU64::new(bits) {
            Some(bits) => Self(bits),
            None => unreachable!(),
        }
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_bits())
    }
}

/// The reserved arena key. Nothing lives here: it keeps zero out of the key
/// space so it can mean "no node".
const RESERVED: usize = 0;

/// The document node and the permanent document element are the first two
/// nodes filed and can never be freed, so their keys never recycle and their
/// generation stays zero for the life of the document.
pub(crate) const DOCUMENT_NODE_ID: NodeId = NodeId::at_key(NonZeroU32::new(1).unwrap(), 0);
pub(crate) const DOCUMENT_ELEMENT_NODE_ID: NodeId = NodeId::at_key(NonZeroU32::new(2).unwrap(), 0);
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
/// `nodes` and `payloads` are keyed by a [`NodeId`]'s arena key and
/// insert/remove in exactly the same order, each asserting that its own free
/// list returns the same key. `DocumentLayoutState::nodes` is the third arena
/// in that lockstep — it lives beside the tree rather than in it.
///
/// `generations` is what makes a key checkable. It is indexed by key, so it is
/// bounded by the peak number of live nodes rather than by how many nodes the
/// document has ever created, and it survives a key going vacant, which is the
/// whole point: it is what tells a handle taken before a free from one taken
/// after.
pub(crate) struct TreeArenas<T> {
    nodes: Slab<Node<T>>,
    payloads: Slab<PayloadSlot<T>>,
    generations: Vec<u32>,
}

impl<T> TreeArenas<T> {
    pub(crate) fn new() -> Self {
        Self {
            nodes: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            payloads: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            generations: Vec::with_capacity(INITIAL_NODE_CAPACITY),
        }
    }

    /// Takes arena key zero out of circulation, before any node is filed.
    ///
    /// Called once, on the boxed arena set, so the placeholder's owner
    /// backpointer is the address every later node will also carry. No handle
    /// can name it — [`NodeId`]'s key is [`NonZeroU32`] — so it exists only to
    /// keep the key off the slab's free list, which is what makes that
    /// non-zero-ness true in the first place.
    pub(crate) fn reserve_zero(&mut self) {
        debug_assert!(
            self.generations.is_empty(),
            "zero is reserved before any node"
        );
        let owner = std::ptr::from_mut::<Self>(self);
        let placeholder = NodeId::at_key(NonZeroU32::new(1).expect("one is non-zero"), 0);
        assert_eq!(
            self.nodes
                .insert(Node::new_text(owner, placeholder, String::new())),
            RESERVED
        );
        assert_eq!(self.payloads.insert(PayloadSlot::Reserved), RESERVED);
        self.generations.push(0);
    }

    /// Whether a handle still names the node it was made for.
    ///
    /// False for one held across the free of its node, whether or not the key
    /// has since been handed to someone else — which is the case a bare arena
    /// key cannot distinguish. Out-of-range keys answer false too: a script can
    /// name any integer, and naming a stranger is a script error rather than a
    /// crash.
    #[inline]
    pub(crate) fn slot_is_current(&self, slot: NodeId) -> bool {
        self.generations
            .get(slot.arena_key())
            .is_some_and(|generation| *generation == slot.generation())
    }

    /// The handle, if it still names a live node. Identity plus a liveness
    /// check — there is nothing left to resolve.
    #[inline]
    pub(crate) fn slot(&self, id: NodeId) -> Option<NodeId> {
        self.contains(id).then_some(id)
    }

    #[inline]
    pub(crate) fn live_slot(&self, id: NodeId) -> NodeId {
        assert!(self.contains(id), "stale NodeId passed to a live-node read");
        id
    }

    #[inline]
    pub(crate) fn get(&self, id: NodeId) -> Option<&Node<T>> {
        self.try_at(id)
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, id: NodeId) -> Option<&mut Node<T>> {
        let node = self.nodes.get_mut(id.arena_key())?;
        likely(node.id() == id).then_some(node)
    }

    #[inline]
    pub(crate) fn live(&self, id: NodeId) -> &Node<T> {
        self.get(id)
            .expect("stale NodeId passed to a live-node read")
    }

    #[inline]
    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.try_at(id).is_some()
    }

    /// The node a handle names, or `None` if the handle is stale.
    ///
    /// The generation is read off the node itself, so the live path is a
    /// single load: the node has to be fetched regardless, and its handle
    /// rides in the same record. Checking a side table first instead costs a
    /// second array on every walk step — and buys nothing, because the answer
    /// is only interesting once the node is in hand anyway.
    #[inline]
    pub(crate) fn try_at(&self, slot: NodeId) -> Option<&Node<T>> {
        let node = self.nodes.get(slot.arena_key())?;
        // A walk follows links the tree itself owns, so all but a vanishing
        // fraction of the handles reaching here are live; a stale one comes
        // from script or a retained frame. Saying so keeps the live path
        // fall-through.
        likely(node.id() == slot).then_some(node)
    }

    #[inline]
    pub(crate) fn at(&self, slot: NodeId) -> &Node<T> {
        self.try_at(slot)
            .expect("stale NodeId: its node was freed after the handle was taken")
    }

    #[inline]
    pub(crate) fn payload_at(&self, slot: NodeId) -> &PayloadSlot<T> {
        assert!(
            self.slot_is_current(slot),
            "stale NodeId: its node was freed after the handle was taken"
        );
        self.payloads
            .get(slot.arena_key())
            .expect("live primary node must have matching payload-arena state")
    }

    /// The node arena itself, for `Debug` only — its keys are arena keys.
    pub(crate) const fn nodes(&self) -> &Slab<Node<T>> {
        &self.nodes
    }

    /// The handle of whatever lives at an arena key, if anything does.
    ///
    /// Stylo's `OpaqueNode` is a bare `usize` — 32 bits on wasm32 — so it
    /// carries the key alone rather than a packed handle. That loses nothing:
    /// two live nodes never share a key, and the snapshot map it keys is
    /// per-flush and purged on free, so every key in it names a live node.
    #[inline]
    pub(crate) fn id_at_arena_key(&self, key: usize) -> Option<NodeId> {
        self.nodes.get(key).map(Node::id)
    }

    /// Every live node's handle, in arena order.
    pub(crate) fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.iter().skip(1).map(|(_, node)| node.id())
    }

    /// Files a node in both tree arenas under a fresh handle.
    ///
    /// The key is whatever the free list offers — recycled the instant its
    /// previous node was freed — and the generation is what distinguishes this
    /// occupant from that one.
    #[expect(
        clippy::inline_always,
        reason = "keep the synchronized slab inserts in the node-allocation hot path"
    )]
    #[inline(always)]
    pub(crate) fn insert_node(
        &mut self,
        payload: PayloadSlot<T>,
        make: impl FnOnce(*mut Self, NodeId) -> Node<T>,
    ) -> NodeId {
        let owner = std::ptr::from_mut::<Self>(self);
        let arena_key = self.nodes.vacant_key();
        let key = u32::try_from(arena_key).expect("a document holds fewer than u32::MAX nodes");
        let key = NonZeroU32::new(key).expect("arena key zero is reserved and never allocated");
        // A key the slab has never handed out yet starts at generation zero;
        // a recycled one carries whatever its last free bumped it to.
        if arena_key == self.generations.len() {
            self.generations.push(0);
        }
        let id = NodeId::at_key(key, self.generations[arena_key]);
        let node = make(owner, id);
        assert_eq!(self.nodes.insert(node), arena_key);
        assert_eq!(self.payloads.vacant_key(), arena_key);
        assert_eq!(self.payloads.insert(payload), arena_key);
        id
    }

    /// Empties both tree arenas of one node and invalidates every handle to it.
    ///
    /// The key goes back on the free list, but its generation moves on, so
    /// anything still holding the old handle — a retained frame, a script
    /// handle whose finalizer has not run yet — resolves to nothing from here
    /// on, even once the key has a new occupant.
    pub(crate) fn remove_node(&mut self, id: NodeId) -> (Node<T>, PayloadSlot<T>) {
        assert!(
            self.slot_is_current(id),
            "subtree links always resolve while removing"
        );
        let node = self
            .nodes
            .try_remove(id.arena_key())
            .expect("a current handle always selects its node");
        let payload = self
            .payloads
            .try_remove(id.arena_key())
            .expect("removed element/text node must have payload-arena state");
        self.generations[id.arena_key()] = self.generations[id.arena_key()].wrapping_add(1);
        (node, payload)
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

    pub(crate) fn insert(&mut self, slot: NodeId) {
        assert_eq!(self.nodes.vacant_key(), slot.arena_key());
        assert_eq!(
            self.nodes.insert(NodeLayoutState::default()),
            slot.arena_key()
        );
    }

    pub(crate) fn remove(&mut self, slot: NodeId) {
        self.nodes
            .try_remove(slot.arena_key())
            .expect("removed node must have layout-arena state");
    }

    #[inline]
    pub(crate) fn at(&self, slot: NodeId) -> &NodeLayoutState {
        self.nodes
            .get(slot.arena_key())
            .expect("live primary node must have matching layout-arena state")
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (usize, &mut NodeLayoutState)> {
        self.nodes.iter_mut()
    }

    #[inline]
    pub(crate) fn at_mut(&mut self, slot: NodeId) -> &mut NodeLayoutState {
        self.nodes
            .get_mut(slot.arena_key())
            .expect("live primary node must have matching layout-arena state")
    }

    pub(crate) fn text_parts(&mut self, slot: NodeId) -> (&mut TextContext, &mut TextLayoutStore) {
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

    pub(crate) fn clear_layout_cache(&mut self, slot: NodeId) {
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

    fn text(owner: *mut TreeArenas<u32>, id: NodeId) -> Node<u32> {
        let _ = owner;
        Node::new_text(owner, id, String::new())
    }

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
            let id = arenas.insert_node(PayloadSlot::Node(payload), text);
            layout.insert(id);
            assert_eq!(arenas.nodes.len(), arenas.payloads.len());
            issued.push(id);
        }

        let freed = issued[1];
        let (_, payload) = arenas.remove_node(freed);
        layout.remove(freed);
        assert!(matches!(payload, PayloadSlot::Node(1)));
        assert_eq!(arenas.nodes.len(), arenas.payloads.len());
        assert_eq!(arenas.slot(freed), None);

        let next = arenas.insert_node(PayloadSlot::Node(9), text);
        layout.insert(next);
        assert_eq!(
            next.arena_key(),
            freed.arena_key(),
            "the freed storage comes back"
        );
        assert_ne!(next, freed, "but the handle to it does not");
    }

    /// A handle nobody ever made resolves to nothing rather than indexing past
    /// the arena. Script can name any integer, and the host boundary turns this
    /// `None` into a JavaScript error instead of a crash.
    #[test]
    fn a_handle_that_was_never_issued_resolves_to_nothing() {
        let mut arenas: TreeArenas<u32> = TreeArenas::new();
        arenas.reserve_zero();
        let id = arenas.insert_node(PayloadSlot::Node(0), text);
        assert_eq!(id.arena_key(), 1, "key zero is reserved");

        let never_issued = [
            NodeId::at_key(NonZeroU32::new(2).unwrap(), 0),
            NodeId::at_key(NonZeroU32::new(999).unwrap(), 0),
            NodeId::at_key(NonZeroU32::new(1).unwrap(), 7),
        ];
        for handle in never_issued {
            assert_eq!(arenas.slot(handle), None);
            assert!(arenas.get(handle).is_none());
            assert!(!arenas.contains(handle));
        }
    }

    /// The tree's own links are `Option<NodeId>`, one per parent and one per
    /// child, so the handle's width is the tree's per-link cost.
    #[test]
    fn a_handle_is_eight_bytes_and_its_option_adds_nothing() {
        assert_eq!(size_of::<NodeId>(), 8);
        assert_eq!(size_of::<Option<NodeId>>(), 8);
    }

    /// The packed form is what crosses every boundary that can only carry a
    /// number, and it has to come back unchanged.
    #[test]
    fn a_handle_round_trips_through_its_packed_form() {
        for (key, generation) in [(1u32, 0u32), (2, 1), (7, 3), (u32::MAX, (1 << 21) - 1)] {
            let id = NodeId::at_key(NonZeroU32::new(key).unwrap(), generation);
            assert_eq!(NodeId::from_bits(id.to_bits()), Some(id));
            let packed = id.to_bits();
            assert!(
                packed < (1_u64 << 53),
                "the packed form must stay inside the integers an f64 represents exactly"
            );
        }
        assert_eq!(NodeId::from_bits(0), None, "key zero is not a handle");
        assert_eq!(
            NodeId::from_bits(1 << 53),
            None,
            "a generation past the carried bits is not a handle"
        );
    }

    /// The generation is the whole reason a recycling arena can hand out
    /// handles at all: it is what tells a handle made before a free from one
    /// made after, once the key has moved on to a new occupant.
    #[test]
    fn a_handle_held_across_its_nodes_free_is_refused_even_once_reused() {
        let mut arenas: TreeArenas<u32> = TreeArenas::new();
        arenas.reserve_zero();
        let held = arenas.insert_node(PayloadSlot::Node(1), text);
        assert!(arenas.slot_is_current(held));
        assert!(arenas.try_at(held).is_some());

        arenas.remove_node(held);
        assert!(
            !arenas.slot_is_current(held),
            "freeing invalidates every handle to that node"
        );
        assert!(arenas.try_at(held).is_none());

        let reused = arenas.insert_node(PayloadSlot::Node(2), text);
        assert_eq!(
            reused.arena_key(),
            held.arena_key(),
            "the storage really did come back"
        );
        assert_ne!(reused, held, "but the old handle does not name it");
        assert!(
            arenas.try_at(held).is_none(),
            "a stale handle must not follow the key to its new occupant"
        );
        assert!(arenas.try_at(reused).is_some());
    }
}
