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

use rustc_hash::{FxHashMap, FxHashSet};
use seldon::geometry::Edges;
use seldon::text::TextContext;
use seldon::tree::{LayoutInput, LayoutSlot};
use slab::Slab;
use smallvec::SmallVec;

use crate::layout::anchors::{AnchorBuckets, AnchorRegistry, AnchoredBox, PendingReads};
use crate::layout::committed_box::{CommittedBox, CommittedBoxTable};
use crate::layout::paragraph::TextStore;
use crate::layout::relevance::{Freshness, Relevance, RelevanceTable};
use crate::layout::text_block::TextBlockStore;
use crate::scroll::{AutoScrollRequest, ScrollRequest};
use crate::tree::anonymous::AnonymousBoxes;
use crate::tree::node::Node;
use crate::tree::top_layer::TopLayer;

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
/// Key zero is permanently occupied by `TreeArenas::reserve_zero`, so no
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
    /// Every document's permanent document element. It is filed second and
    /// can never be freed, so this one handle names it in any document for
    /// the document's whole life — which is what lets a layer without the
    /// document (the runtime's input router, targeting a key event before a
    /// focus model exists) name it.
    pub const DOCUMENT_ELEMENT: Self = DOCUMENT_ELEMENT_NODE_ID;

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

/// The fixed-address, document-owned arena set.
///
/// `nodes` is keyed by a [`NodeId`]'s arena key. An embedder's payload rides
/// in its node ([`Node::payload`]), and so does what tells a generated node —
/// a `::backdrop` box's, an anonymous box's — from the rest, so there is no
/// second slab to keep in lockstep with it. `DocumentLayoutState` keys its
/// entries by the same arena key, beside the tree rather than in it.
///
/// `generations` is what makes a key checkable. It is indexed by key, so it is
/// bounded by the peak number of live nodes rather than by how many nodes the
/// document has ever created, and it survives a key going vacant, which is the
/// whole point: it is what tells a handle taken before a free from one taken
/// after.
pub(crate) struct TreeArenas<T> {
    nodes: Slab<Node<T>>,
    generations: Vec<u32>,
    /// `content-visibility: auto` relevance, keyed by arena key. It lives
    /// here rather than in `DocumentLayoutState` because a
    /// [`StyleView`](crate::layout::StyleView) is built from these arenas
    /// alone and has to be able to read it; see
    /// [`crate::layout::relevance`].
    relevance: RelevanceTable,
    /// Each element's last committed content box, keyed by arena key, here
    /// for the same reason the relevance table is: its two readers are a
    /// [`StyleView`](crate::layout::StyleView), which substitutes the
    /// css-sizing-4 half into `contain-intrinsic-*`, and
    /// [`TElement::query_container_size`](stylo::dom::TElement::query_container_size)
    /// on `&Node`, which answers `cqw`/`cqh` from the css-contain-3 half.
    /// Neither can reach anything but these arenas. See
    /// [`crate::layout::committed_box`].
    committed_boxes: CommittedBoxTable,
    /// css-anchor-position-1's name index and each element's position
    /// options, here rather than in `DocumentLayoutState` because
    /// `LayoutTree::position_option_style` lends an option style out of
    /// these arenas as it lends the base style, with no state to reach. Its
    /// entries are the elements that declare an anchor name, an anchor
    /// scope or position-try fallbacks; see [`crate::layout::anchors`].
    anchors: AnchorRegistry,
    /// The document's top layer (css-position-4 §3), in insertion order.
    /// Here rather than in `Document` because its per-box readers — the
    /// position lowering a [`StyleView`](crate::layout::StyleView) answers
    /// and the containing-block walk in [`crate::layout::anchors`] — reach
    /// nothing but these arenas. A side table rather than a per-node bit,
    /// because the layer is almost always empty and every reader tests
    /// `is_empty` first; see [`crate::tree::top_layer`].
    top_layer: TopLayer,
    /// The anonymous boxes loose text generates, and the layout children
    /// they replace that text with. Here because `LayoutTree::children`, the
    /// one reader on the layout path, reaches nothing but these arenas; see
    /// [`crate::tree::anonymous`].
    anonymous: AnonymousBoxes,
    /// How many element nodes are live: the upper bound
    /// [`DocumentLayoutState::ensure_covers`] reserves its entries to, since
    /// only an element files one. Counted where [`Self::insert_node`] files a
    /// node and [`Self::remove_node`] frees one, the only two places a node
    /// enters or leaves the arena, and a node's kind never changes in
    /// between.
    live_elements: usize,
}

impl<T> TreeArenas<T> {
    pub(crate) fn new() -> Self {
        Self {
            nodes: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            generations: Vec::with_capacity(INITIAL_NODE_CAPACITY),
            relevance: RelevanceTable::default(),
            committed_boxes: CommittedBoxTable::default(),
            anchors: AnchorRegistry::default(),
            top_layer: TopLayer::default(),
            anonymous: AnonymousBoxes::default(),
            live_elements: 0,
        }
    }

    /// The anonymous boxes around loose text; see
    /// [`crate::tree::anonymous`].
    #[inline]
    pub(crate) fn anonymous(&self) -> &AnonymousBoxes {
        &self.anonymous
    }

    #[inline]
    pub(crate) fn anonymous_mut(&mut self) -> &mut AnonymousBoxes {
        &mut self.anonymous
    }

    /// Whether `id` names an anonymous block box's node.
    #[inline]
    pub(crate) fn is_anonymous_box(&self, id: NodeId) -> bool {
        self.get(id).is_some_and(Node::is_anonymous_box)
    }

    /// The container of the anonymous box `id`: its parent, or the host of
    /// the shadow root it parents to; `None` when `id` is no anonymous box.
    ///
    /// Read from the box's own link, not through the flat tree: a shadow root
    /// attached to the container since the box was derived turns its flat
    /// parent into `None` until the next flush re-derives it.
    pub(crate) fn anonymous_container(&self, id: NodeId) -> Option<NodeId> {
        if !self.is_anonymous_box(id) {
            return None;
        }
        let parent = self.get(self.get(id)?.parent_id()?)?;
        Some(parent.shadow_host_id().unwrap_or_else(|| parent.id()))
    }

    /// The document's top layer; see [`crate::tree::top_layer`].
    #[inline]
    pub(crate) fn top_layer(&self) -> &TopLayer {
        &self.top_layer
    }

    #[inline]
    pub(crate) fn top_layer_mut(&mut self) -> &mut TopLayer {
        &mut self.top_layer
    }

    /// The anchor-positioning registry; see [`crate::layout::anchors`].
    #[inline]
    pub(crate) fn anchors(&self) -> &AnchorRegistry {
        &self.anchors
    }

    #[inline]
    pub(crate) fn anchors_mut(&mut self) -> &mut AnchorRegistry {
        &mut self.anchors
    }

    /// One element's `content-visibility: auto` relevance.
    #[inline]
    pub(crate) fn relevance(&self, slot: NodeId) -> Relevance {
        self.relevance.state(slot.arena_key())
    }

    /// How far the render in flight has determined this element's
    /// relevance.
    #[inline]
    pub(crate) fn relevance_freshness(&self, slot: NodeId) -> Freshness {
        self.relevance.freshness(slot.arena_key())
    }

    /// Records one determination, answering whether the bit moved.
    pub(crate) fn determine_relevance(&mut self, slot: NodeId, state: Relevance) -> bool {
        let flipped = self.relevance.determine(slot.arena_key(), state);
        if flipped {
            self.anchors.note_relevance_flip();
        }
        flipped
    }

    /// Whether the last render left a reveal owed; see
    /// [`RelevanceTable::owed`].
    #[inline]
    pub(crate) const fn relevance_owed(&self) -> bool {
        self.relevance.owed()
    }

    /// Records whether the render in flight leaves a reveal owed.
    #[inline]
    pub(crate) fn set_relevance_owed(&mut self, owed: bool) {
        self.relevance.set_owed(owed);
    }

    /// Takes the owed reveal, answering whether there was one.
    #[inline]
    pub(crate) fn take_relevance_owed(&mut self) -> bool {
        self.relevance.take_owed()
    }

    /// Ends a render's determinations; see [`RelevanceTable::settle`].
    pub(crate) fn settle_relevance(&mut self) {
        self.relevance.settle();
    }

    /// One element's last committed content box: its css-sizing-4 §5.2.1
    /// last remembered size and its css-contain-3 §2.1 query container size.
    #[inline]
    pub(crate) fn committed_box(&self, slot: NodeId) -> CommittedBox {
        self.committed_boxes.get(slot.arena_key())
    }

    /// Records one box's content box for the layout pass in flight.
    ///
    /// Shared rather than exclusive, because the recording moment is inside
    /// the layout pass and `LayoutTree::compute_layout` holds these arenas
    /// shared; staged rather than published, because the style traversal
    /// reads the published table from several threads at once, so only
    /// [`Self::publish_committed_boxes`] — which takes `&mut self` — may
    /// write it.
    #[inline]
    pub(crate) fn note_committed_box(&self, slot: NodeId, committed: CommittedBox) {
        self.committed_boxes.note(slot, committed);
    }

    /// Publishes the layout pass's records, appending every query container
    /// whose size moved to `resized`.
    #[inline]
    pub(crate) fn publish_committed_boxes(&mut self, resized: &mut Vec<NodeId>) {
        self.committed_boxes.apply(resized);
    }

    /// Whether any style this document ever cascaded resolved a `cqw`/`cqh`.
    #[inline]
    pub(crate) fn uses_container_units(&self) -> bool {
        self.committed_boxes.uses_container_units()
    }

    /// The flag the style traversal sets when it cascades one.
    #[inline]
    pub(crate) fn container_units_flag(&self) -> &std::sync::atomic::AtomicBool {
        self.committed_boxes.units_flag()
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
                .insert(Node::new_text(owner, placeholder, Box::default())),
            RESERVED
        );
        self.generations.push(0);
    }

    /// One past the highest arena key ever occupied — the bound the
    /// slot-aligned layout state sizes itself to.
    #[inline]
    pub(crate) fn slot_bound(&self) -> usize {
        self.generations.len()
    }

    /// How many element nodes are live — the bound the layout state reserves
    /// its entries to.
    #[inline]
    pub(crate) const fn live_elements(&self) -> usize {
        self.live_elements
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

    fn live_mut(&mut self, id: NodeId) -> &mut Node<T> {
        self.get_mut(id)
            .expect("internal tree link must resolve to a live node")
    }

    /// Puts `child` at `index` in `parent`'s child list and makes `parent`
    /// its parent, keeping every child's stored position
    /// ([`Node::index_in`]) current.
    ///
    /// The children on the shorter side of `index` are the ones rewritten, so
    /// an insertion at either end of the list writes no sibling.
    pub(crate) fn insert_child(&mut self, parent: NodeId, index: usize, child: NodeId) {
        let parent_node = self.live_mut(parent);
        let mut children = std::mem::take(&mut parent_node.children);
        let mut base = parent_node.child_index_base;
        if index < children.len() - index {
            // Moving the base moves every position up by one; the children
            // before `index` move back down to keep theirs.
            base = base.wrapping_sub(1);
            self.shift_child_indexes(&children[..index], u32::MAX);
        } else {
            self.shift_child_indexes(&children[index..], 1);
        }
        children.insert(index, child);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "positions are stored modulo 2^32, like the base they are relative to"
        )]
        let position = index as u32;
        let child_node = self.live_mut(child);
        child_node.parent = Some(parent);
        child_node.child_index = base.wrapping_add(position);
        let parent_node = self.live_mut(parent);
        parent_node.children = children;
        parent_node.child_index_base = base;
    }

    /// Takes `child` out of `parent`'s child list and clears its parent,
    /// answering the position it held. The children on the shorter side of
    /// that position are the ones rewritten, as in [`Self::insert_child`].
    pub(crate) fn remove_child(&mut self, parent: NodeId, child: NodeId) -> usize {
        let parent_node = self.live(parent);
        let index = self.live(child).index_in(parent_node);
        assert_eq!(
            parent_node.children.get(index),
            Some(&child),
            "child must appear in its parent's child list"
        );
        let parent_node = self.live_mut(parent);
        let mut children = std::mem::take(&mut parent_node.children);
        let mut base = parent_node.child_index_base;
        children.remove(index);
        if index < children.len() - index {
            // Moving the base moves every position down by one; the children
            // before `index` move back up to keep theirs.
            base = base.wrapping_add(1);
            self.shift_child_indexes(&children[..index], 1);
        } else {
            self.shift_child_indexes(&children[index..], u32::MAX);
        }
        self.live_mut(child).parent = None;
        let parent_node = self.live_mut(parent);
        parent_node.children = children;
        parent_node.child_index_base = base;
        index
    }

    /// Adds `by` to each listed child's stored position, wrapping, so
    /// `u32::MAX` subtracts one.
    fn shift_child_indexes(&mut self, children: &[NodeId], by: u32) {
        for &slot in children {
            let node = self.live_mut(slot);
            node.child_index = node.child_index.wrapping_add(by);
        }
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

    /// Files a node in the node arena under a fresh handle.
    ///
    /// The key is whatever the free list offers — recycled the instant its
    /// previous node was freed — and the generation is what distinguishes this
    /// occupant from that one.
    #[expect(
        clippy::inline_always,
        reason = "keep the slab insert in the node-allocation hot path"
    )]
    #[inline(always)]
    pub(crate) fn insert_node(
        &mut self,
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
        self.live_elements += usize::from(node.is_element());
        assert_eq!(self.nodes.insert(node), arena_key);
        id
    }

    /// Empties the node arena of one node and invalidates every handle to it.
    ///
    /// The key goes back on the free list, but its generation moves on, so
    /// anything still holding the old handle — a retained frame, a script
    /// handle whose finalizer has not run yet — resolves to nothing from here
    /// on, even once the key has a new occupant.
    pub(crate) fn remove_node(&mut self, id: NodeId) -> Node<T> {
        assert!(
            self.slot_is_current(id),
            "subtree links always resolve while removing"
        );
        let node = self
            .nodes
            .try_remove(id.arena_key())
            .expect("a current handle always selects its node");
        self.live_elements -= usize::from(node.is_element());
        self.generations[id.arena_key()] = self.generations[id.arena_key()].wrapping_add(1);
        // A dropped node takes its relevance with it: the key is recycled,
        // and its next occupant must start undetermined rather than inherit
        // a stranger's answer.
        self.relevance.reset(id.arena_key());
        // Its last committed content box goes the same way, and for the same
        // reason: css-sizing-4 attaches the remembered size to the element,
        // and no element is a query container until its own `container-type`
        // says so, so the key's next occupant starts with neither.
        self.committed_boxes.reset(id.arena_key());
        // And whatever it declared for anchor positioning: a name index that
        // outlived its element would offer a dead candidate to every lookup.
        self.anchors.forget(id);
        node
    }
}

#[derive(Default)]
pub(crate) struct NodeLayoutState {
    pub(crate) slot: LayoutSlot,
    /// The paragraph a Lynx text element or a block container owns. A text node holds none: it is
    /// content of the paragraph above it, not a box.
    pub(crate) text: Option<Box<TextStore>>,
}

/// [`DocumentLayoutState::index`]'s mark for a key with no entry.
const NO_ENTRY: u32 = u32::MAX;

/// Layout/text state for the nodes that lay out, sized lazily rather than
/// kept in lockstep with the primary arena: creating a node costs nothing
/// here, and a node that is never laid out never allocates layout state.
///
/// Two levels. `index` is arena-key aligned at four bytes a key and names
/// each key's position in `entries`, which holds a [`NodeLayoutState`] only
/// for a key something wrote layout state for. The document node, a shadow
/// root, an element no pass reached and a freed key that never laid out hold
/// none, and neither does a text node: it is content of the paragraph above
/// it, never a box, and `LayoutTree::children` leaves it out, so seldon never
/// writes a box for one.
///
/// `Document::layout` sizes `index` to the primary arena's slot bound before
/// the engine reads slots wholesale; a key it covers with no entry reads as
/// [`Self::vacant`] — "never laid out", an empty cache — and a key past it
/// reads as nothing, as before the split. An entry outlives its node: a freed
/// key's entry is reset in place for the key's next occupant, never
/// compacted.
pub(crate) struct DocumentLayoutState {
    index: Vec<u32>,
    entries: Vec<NodeLayoutState>,
    /// What a covered key with no entry reads as. Never written.
    vacant: NodeLayoutState,
    pub(crate) text_context: Option<Box<TextContext>>,
    /// Text nodes whose one retained Parley layout a probe re-broke away from
    /// its committed line break during the pass in flight. The pass owes them
    /// a restore before anything outside it (painting, hit testing) reads a
    /// committed layout; see [`Self::restore_probed_text`].
    probed_text: Vec<NodeId>,
    /// Whether the run in flight may defer a size query container's contents;
    /// see [`crate::layout::committed_box`] and `layout::host::run_layout`.
    interleaves_containers: bool,
    /// The size query containers this run deferred, in the order it reached
    /// them. Empty for every page that has none, which is why the interleave
    /// costs one `is_empty` test to a page that does not use the feature.
    container_deferrals: Vec<DeferredContainer>,
    /// Each scroll container that has honoured a `scroll-initial-target`
    /// element, with that element; see `scroll::initial_target`. A side
    /// table rather than a per-node field because almost no container ever
    /// has one: a page without the property keeps it empty and pays
    /// nothing per node.
    pub(crate) initial_targets: Vec<(NodeId, NodeId)>,
    /// Each scroll container's pending programmatic scroll request, the
    /// newest only; see `scroll::request`. A side table rather than a
    /// per-node field because only a container a script has scrolled has
    /// one, and only until the painter acknowledges it: bounded by the
    /// containers with an unacknowledged request. An entry dies with its
    /// acknowledgement, with its node ([`Self::remove`]), or at the first
    /// render whose frame has no scroll slot for it.
    pub(crate) scroll_requests: FxHashMap<NodeId, ScrollRequest>,
    /// Each scroll container's stored scroll offset, as the last scroll
    /// wrote it; see `Document::scroll_to`. A side table rather than a field
    /// of every node's layout state because only scroll containers have one.
    /// An entry lives until its element is freed ([`Self::remove`]); a key
    /// with none reads as zero.
    pub(crate) scroll_offsets: FxHashMap<NodeId, euclid::default::Vector2D<f32>>,
    /// Each scroll container that scroll anchoring has anchored or adjusted,
    /// with its anchor and the sum of its adjustments; see
    /// `scroll::anchoring`. A side table for the reason `scroll_offsets` is
    /// one, and bounded by the containers that were ever scrolled away from
    /// their block start.
    pub(crate) scroll_anchors: FxHashMap<NodeId, crate::scroll::anchoring::ScrollAnchor>,
    /// Each scroll container's reach as the painter last posted it — how
    /// much farther its fling or glide will carry it — keyed as
    /// `scroll_offsets` is; see `Document::set_scroll_reach`. A side table
    /// for the reason `scroll_offsets` is one, and emptier still: a zero
    /// reach removes the entry, so it holds only the containers in motion,
    /// and a settled page holds none.
    pub(crate) scroll_reaches: FxHashMap<NodeId, euclid::default::Vector2D<f32>>,
    /// The serial the next scroll request takes: one counter per document,
    /// so serials order every request against every post that names one.
    pub(crate) next_scroll_request: NonZeroU64,
    /// Each scroll container's pending auto-scroll change, the newest only;
    /// see `scroll::request`. A list rather than a per-node field or a field
    /// of the scroll request because almost no document ever calls
    /// `autoScroll`: one that never does keeps it empty and pays nothing per
    /// node or per container. An entry dies as a scroll request's does.
    pub(crate) auto_scroll_requests: Vec<(NodeId, AutoScrollRequest)>,
    /// The containers an instant scroll request moved since the host last
    /// took them, each once with the offset it had before the first such
    /// move, in the order they moved: CSSOM-View's pending scroll event
    /// targets for the scrolls the document itself performs. Bounded by the
    /// scroll containers, and empty unless a script, a component or a
    /// `scroll-initial-target` scrolled one; see `scroll::request`.
    pub(crate) instant_scrolls: SmallVec<[(NodeId, euclid::default::Vector2D<f32>); 2]>,
    /// The grid items whose `position: sticky` insets resolve against their
    /// grid area rather than the grid container: the area's edges in the
    /// container's border-box coordinates, unrounded as the layout run
    /// committed them and device-snapped by the rounding tail. A side
    /// table rather than a field of every `Layout` because only sticky grid
    /// items have one: a page without them keeps it empty and pays nothing
    /// per node (`seldon::LayoutTree::set_sticky_containing_block`).
    pub(crate) sticky_containing_blocks: Vec<StickyContainingBlock>,
    /// css-anchor-position-1's per-box state — outcome, reads, remembered
    /// scroll offsets, last successful position option, and whether the run
    /// in flight reported it (for the settle loop's recording) — for every box
    /// `seldon` reported as anchor-positioned and nothing else: a page
    /// without anchor positioning keeps it empty. See
    /// [`crate::layout::anchors`] for what each part is for.
    pub(crate) anchored: FxHashMap<NodeId, AnchoredBox>,
    /// The reads of the boxes being laid out that `seldon` has not reported
    /// an outcome for yet. Interior-mutable because the queries arrive with
    /// the state borrowed shared; empty between runs.
    pub(crate) anchor_pending: PendingReads,
    /// Each scoped anchor name's definers partitioned by their nearest
    /// `anchor-scope`, for the §2.3 lookup; see
    /// [`crate::layout::anchors::AnchorBuckets`].
    pub(crate) anchor_buckets: AnchorBuckets,
    /// Each element that generates the containing block of an out-of-flow
    /// box whose box parent is another box (an `absolute` box under a
    /// non-positioned parent, a `fixed` one under a transformed ancestor),
    /// with those boxes: what its algorithm's absolute pass lays out besides
    /// its own children ([`seldon::tree::LayoutTree::hoisted_children`]).
    /// Written where the box's parent records its static position, the one
    /// call every committing run of that parent makes for it. A side table
    /// because only such containing blocks have an entry: the document's
    /// UA sheet makes every Lynx element `position: relative`, so a page
    /// gets one only from `fixed` boxes under transforms or authored
    /// `position: static` parents. An entry can outlive its box's escape —
    /// the box stopped being positioned, or moved — until the box escapes
    /// elsewhere or is freed; every read re-derives the containing block and
    /// skips it, so a stale entry costs a lookup, never a misplaced box.
    /// A box is listed under one block at most. There is no inverse map:
    /// moving or freeing a box scans these entries, which are few for the
    /// same reason they exist at all.
    pub(crate) hoisted_to: FxHashMap<NodeId, SmallVec<[NodeId; 2]>>,
    /// Whether the rounding tail is running, where a box placed late (its
    /// containing block did not run) is written after the walk has already
    /// rounded the boxes above it and needs no marks on them.
    pub(crate) in_rounding_tail: bool,
    /// The Lynx text elements whose committing paragraph runs are reported
    /// ([`Self::note_text_layout`]), as the host set them through
    /// `Document::set_text_layout_reporting`. A side table keyed by node
    /// rather than a field of the paragraph store, because the store is
    /// replaced on every rebuild, dropped by relayout damage
    /// (`clear_text_artifact`) and absent before the first layout, and the
    /// setting has to survive all three. An empty set holds no allocation, so
    /// a page with no reporting element pays one `is_empty` test per
    /// committing paragraph run. An entry lives until the host clears it or
    /// the element is freed ([`Self::remove`]).
    pub(crate) text_layout_reporters: FxHashSet<NodeId>,
    /// The reporting Lynx text elements whose paragraph a committing run
    /// recomputed since the host last took the list, in the order the runs
    /// reached them, one entry per run: an element can appear more than once
    /// until `Document::take_text_layout_reports` removes the repeats.
    pub(crate) text_layout_reports: Vec<NodeId>,
}

/// One entry of [`DocumentLayoutState::sticky_containing_blocks`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct StickyContainingBlock {
    pub(crate) node: NodeId,
    pub(crate) unrounded: Edges<f32>,
    /// Written by the rounding tail after the commit that recorded
    /// `unrounded`; `None` between the two.
    pub(crate) rounded: Option<Edges<f32>>,
}

/// A `container-type: size` box whose committing run laid no contents out.
///
/// The interleave found that the size the box is about to take is not the one
/// its descendants' `cqw`/`cqh` were last cascaded against, so the run
/// recorded the new size and stopped: laying the subtree out there would lay
/// it out at a size about to be restyled away. The host relays it once the
/// restyle has happened, from the input kept here.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DeferredContainer {
    pub(crate) node: NodeSlot,
    pub(crate) input: LayoutInput,
}

impl DocumentLayoutState {
    pub(crate) fn new() -> Self {
        Self {
            index: Vec::new(),
            entries: Vec::new(),
            vacant: NodeLayoutState::default(),
            text_context: None,
            probed_text: Vec::new(),
            interleaves_containers: false,
            container_deferrals: Vec::new(),
            initial_targets: Vec::new(),
            scroll_requests: FxHashMap::default(),
            scroll_offsets: FxHashMap::default(),
            scroll_anchors: FxHashMap::default(),
            scroll_reaches: FxHashMap::default(),
            next_scroll_request: NonZeroU64::MIN,
            auto_scroll_requests: Vec::new(),
            instant_scrolls: SmallVec::new(),
            sticky_containing_blocks: Vec::new(),
            anchored: FxHashMap::default(),
            anchor_pending: PendingReads::default(),
            anchor_buckets: AnchorBuckets::default(),
            hoisted_to: FxHashMap::default(),
            in_rounding_tail: false,
            text_layout_reporters: FxHashSet::default(),
            text_layout_reports: Vec::new(),
        }
    }

    /// Opens (or closes) the interleave for one run and clears whatever the
    /// last one left.
    pub(crate) fn begin_container_interleave(&mut self, enabled: bool) {
        self.interleaves_containers = enabled;
        self.container_deferrals.clear();
    }

    #[inline]
    pub(crate) const fn interleaves_containers(&self) -> bool {
        self.interleaves_containers
    }

    pub(crate) fn defer_container(&mut self, node: NodeSlot, input: LayoutInput) {
        self.container_deferrals
            .push(DeferredContainer { node, input });
    }

    /// Moves this run's deferrals into `sink`, keeping the allocation: a page
    /// whose container animates defers into the same one forever.
    pub(crate) fn take_container_deferrals(&mut self, sink: &mut Vec<DeferredContainer>) {
        sink.clear();
        sink.append(&mut self.container_deferrals);
    }

    /// Sizes the key-aligned index to cover every key below `slots`, and
    /// makes room in `entries` for `elements` — the live element count,
    /// which bounds the entries a pass can file, since a text node never
    /// files one.
    ///
    /// Exact until the first entry is filed, so a page's first layout files
    /// every entry into one allocation instead of doubling its way there
    /// through `push` (a render reserves before the flush that may add
    /// anonymous boxes, and the layout run after it reserves again, still
    /// exactly); amortized as `Vec::reserve` is after that, so a page that
    /// adds a few elements per layout is not reallocated exactly every time.
    /// It never shrinks: an entry outlives its node.
    pub(crate) fn ensure_covers(&mut self, slots: usize, elements: usize) {
        if self.index.len() < slots {
            self.index.resize(slots, NO_ENTRY);
        }
        if self.entries.is_empty() {
            self.entries.reserve_exact(elements);
        } else {
            self.entries
                .reserve(elements.saturating_sub(self.entries.len()));
        }
    }

    /// Where `slot`'s entry sits in `entries`, if it has one.
    #[inline]
    fn entry_of(&self, slot: NodeId) -> Option<usize> {
        match self.index.get(slot.arena_key()) {
            Some(&at) if at != NO_ENTRY => Some(at as usize),
            _ => None,
        }
    }

    /// Whether `slot` has an entry of its own.
    #[cfg(test)]
    pub(crate) fn has_entry(&self, slot: NodeId) -> bool {
        self.entry_of(slot).is_some()
    }

    /// Where `slot`'s entry sits in `entries`, filing it on first use.
    fn file_entry(&mut self, slot: NodeId) -> usize {
        if let Some(at) = self.entry_of(slot) {
            return at;
        }
        if self.index.len() <= slot.arena_key() {
            self.index.resize(slot.arena_key() + 1, NO_ENTRY);
        }
        let at = self.entries.len();
        self.index[slot.arena_key()] =
            u32::try_from(at).expect("a document holds fewer than u32::MAX nodes");
        self.entries.push(NodeLayoutState::default());
        at
    }

    /// `slot`'s entry, filed on first use.
    fn entry_mut(&mut self, slot: NodeId) -> &mut NodeLayoutState {
        let at = self.file_entry(slot);
        &mut self.entries[at]
    }

    /// Resets a freed slot so the key's next occupant starts clean. The entry
    /// itself stays: slot storage is recycled, never compacted.
    pub(crate) fn remove(&mut self, slot: NodeId) {
        if let Some(at) = self.entry_of(slot) {
            self.entries[at] = NodeLayoutState::default();
        }
        if !self.scroll_requests.is_empty() {
            self.scroll_requests.remove(&slot);
        }
        if !self.scroll_offsets.is_empty() {
            self.scroll_offsets.remove(&slot);
        }
        if !self.scroll_anchors.is_empty() {
            self.scroll_anchors.remove(&slot);
        }
        if !self.scroll_reaches.is_empty() {
            self.scroll_reaches.remove(&slot);
        }
        if !self.auto_scroll_requests.is_empty() {
            self.auto_scroll_requests.retain(|(node, _)| *node != slot);
        }
        if !self.anchored.is_empty() {
            self.anchored.remove(&slot);
        }
        self.register_hoisted(slot, None);
        self.hoisted_to.remove(&slot);
        if !self.text_layout_reporters.is_empty() {
            self.text_layout_reporters.remove(&slot);
            self.text_layout_reports.retain(|node| *node != slot);
        }
    }

    /// Records that a committing run recomputed `slot`'s Lynx paragraph, when
    /// `slot` is a reporting element ([`Self::text_layout_reporters`]). Called
    /// from the committing branch of
    /// [`crate::layout::text_block::compute_text_block_layout`], which seldon
    /// reaches only when its layout cache for the element missed.
    #[inline]
    pub(crate) fn note_text_layout(&mut self, slot: NodeId) {
        if !self.text_layout_reporters.is_empty() && self.text_layout_reporters.contains(&slot) {
            self.text_layout_reports.push(slot);
        }
    }

    /// Lists `node` under the containing block `block` its absolute pass
    /// lays it out from (`None`: no element's — the initial containing
    /// block's, which the run's tail places), moving it off any other.
    ///
    /// Finds the block it was listed under by scanning [`Self::hoisted_to`];
    /// see the field for why that is cheap.
    pub(crate) fn register_hoisted(&mut self, node: NodeId, block: Option<NodeId>) {
        if self.hoisted_to.is_empty() && block.is_none() {
            return;
        }
        if let Some(block) = block
            && self
                .hoisted_to
                .get(&block)
                .is_some_and(|listed| listed.contains(&node))
        {
            return;
        }
        self.hoisted_to.retain(|_, listed| {
            listed.retain(|listed| *listed != node);
            !listed.is_empty()
        });
        if let Some(block) = block {
            self.hoisted_to.entry(block).or_default().push(node);
        }
    }

    /// `slot`'s layout state: its entry, [`Self::vacant`] for a key the index
    /// covers with none, and nothing for a key past it.
    #[inline]
    pub(crate) fn get(&self, slot: NodeId) -> Option<&NodeLayoutState> {
        let &at = self.index.get(slot.arena_key())?;
        Some(if at == NO_ENTRY {
            &self.vacant
        } else {
            &self.entries[at as usize]
        })
    }

    /// `slot`'s layout slot, a never-laid-out one when it has no entry.
    #[inline]
    pub(crate) fn slot_at(&self, slot: NodeId) -> &LayoutSlot {
        match self.entry_of(slot) {
            Some(at) => &self.entries[at].slot,
            None => &self.vacant.slot,
        }
    }

    /// Every entry, in no particular order.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut NodeLayoutState> {
        self.entries.iter_mut()
    }

    #[inline]
    pub(crate) fn at_mut(&mut self, slot: NodeId) -> &mut NodeLayoutState {
        self.entry_mut(slot)
    }

    /// `slot`'s layout slot for a write: its entry, filed on first use.
    #[inline]
    pub(crate) fn slot_mut(&mut self, slot: NodeId) -> &mut LayoutSlot {
        &mut self.entry_mut(slot).slot
    }

    /// The shared shaping context beside one element's paragraph slot.
    ///
    /// Disjoint field borrows: laying a paragraph out needs the context and
    /// the block at once, and child layout needs the whole state, so the two
    /// can never be live together by construction.
    pub(crate) fn text_store_parts(
        &mut self,
        slot: NodeId,
    ) -> (&mut TextContext, &mut Option<Box<TextStore>>) {
        let at = self.file_entry(slot);
        let Self {
            index: _,
            entries,
            vacant: _,
            text_context,
            probed_text: _,
            interleaves_containers: _,
            container_deferrals: _,
            initial_targets: _,
            scroll_requests: _,
            scroll_offsets: _,
            scroll_anchors: _,
            scroll_reaches: _,
            next_scroll_request: _,
            auto_scroll_requests: _,
            instant_scrolls: _,
            sticky_containing_blocks: _,
            anchored: _,
            anchor_pending: _,
            anchor_buckets: _,
            hoisted_to: _,
            in_rounding_tail: _,
            text_layout_reporters: _,
            text_layout_reports: _,
        } = self;
        let context = text_context
            .get_or_insert_with(|| Box::new(TextContext::new()))
            .as_mut();
        (context, &mut entries[at].text)
    }

    /// The retained paragraph `slot` owns, of either kind.
    pub(crate) fn text_store(&self, slot: NodeId) -> Option<&TextStore> {
        self.get(slot).and_then(|node| node.text.as_deref())
    }

    /// The Lynx paragraph `slot` owns, if it owns one.
    pub(crate) fn text_block(&self, slot: NodeId) -> Option<&TextBlockStore> {
        self.text_store(slot).and_then(TextStore::as_block)
    }

    /// Records that `slot`'s probe left its retained text layout off the
    /// committed line break. Cheap enough to call after every probe: the check
    /// is two comparisons, and only a probe that actually re-entered Parley
    /// enqueues anything.
    pub(crate) fn note_probed_text(&mut self, slot: NodeId) {
        if self.text_store(slot).is_some_and(TextStore::is_probe_dirty) {
            self.probed_text.push(slot);
        }
    }

    /// Returns every text node this pass probed to its committed line break.
    ///
    /// Restoring is a no-op for a node whose commit already brought it back,
    /// which is the ordinary case; the queue exists for the nodes a pass
    /// measures but never commits, whose committed layout painting still
    /// reads.
    pub(crate) fn restore_probed_text(&mut self) {
        let mut probed = std::mem::take(&mut self.probed_text);
        let Self {
            index,
            entries,
            vacant: _,
            text_context,
            probed_text: _,
            interleaves_containers: _,
            container_deferrals: _,
            initial_targets: _,
            scroll_requests: _,
            scroll_offsets: _,
            scroll_anchors: _,
            scroll_reaches: _,
            next_scroll_request: _,
            auto_scroll_requests: _,
            instant_scrolls: _,
            sticky_containing_blocks: _,
            anchored: _,
            anchor_pending: _,
            anchor_buckets: _,
            hoisted_to: _,
            in_rounding_tail: _,
            text_layout_reporters: _,
            text_layout_reports: _,
        } = self;
        // Unlike the path this replaces, restoring can re-enter the shaper —
        // a truncating block rebuilds its display layout — so the context is
        // borrowed disjointly beside the slot rather than not at all.
        let context = text_context
            .get_or_insert_with(|| Box::new(TextContext::new()))
            .as_mut();
        for slot in probed.drain(..) {
            if let Some(store) = index
                .get(slot.arena_key())
                .filter(|&&at| at != NO_ENTRY)
                .and_then(|&at| entries[at as usize].text.as_deref_mut())
            {
                store.restore_committed(context);
            }
        }
        self.probed_text = probed;
        // The queue is only as good as the pushes that fill it, and a block
        // missing from it paints its probe's line breaks with no other
        // symptom. Debug builds re-derive the answer the expensive way.
        debug_assert!(
            !self
                .entries
                .iter()
                .filter_map(|node| node.text.as_deref())
                .any(TextStore::is_probe_dirty),
            "a probed text block never reached the restore queue"
        );
    }

    /// Drops a node's cached box measurements without touching its shaped
    /// text.
    ///
    /// The pair exists because the two caches answer to different inputs: the
    /// box cache is invalid the moment any geometry around a node moves, while
    /// the shaped layout only dies when the text or the style Parley shaped it
    /// from changes. Re-shaping a paragraph to lay it out one pixel narrower
    /// is the single most expensive thing this engine can do for no reason.
    ///
    /// A node with no entry has an empty cache already, and nothing above it
    /// reads the mark clearing sets before its first write files the entry.
    pub(crate) fn clear_box_cache(&mut self, slot: NodeId) {
        if let Some(at) = self.entry_of(slot) {
            self.entries[at].slot.clear_layout_cache();
        }
    }

    /// Drops a node's shaped text, keeping its box measurements.
    ///
    /// Separate from [`Self::clear_box_cache`] because the two answer
    /// different questions: a box cache dies when any geometry around a node
    /// moves, a shaped paragraph only when the text or the style it was
    /// shaped from changes. Re-shaping is the expensive half, so the eviction
    /// that costs it is never implied by the one that does not.
    /// Drops a node's shaped paragraph, keeping its box measurements.
    ///
    /// A paragraph is rebuilt rather than patched — parley's own mutability
    /// contract — so eviction is simply dropping it.
    pub(crate) fn clear_text_artifact(&mut self, slot: NodeId) {
        if let Some(at) = self.entry_of(slot) {
            self.entries[at].text = None;
        }
    }

    /// Drops both a node's cached box measurements and its shaped text.
    pub(crate) fn clear_layout_cache(&mut self, slot: NodeId) {
        self.clear_box_cache(slot);
        self.clear_text_artifact(slot);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn text(payload: u32) -> impl FnOnce(*mut TreeArenas<u32>, NodeId) -> Node<u32> {
        move |owner, id| Node::new_text(owner, id, Box::default()).with_payload(payload)
    }

    /// The one property no caller can check for itself: a freed key comes
    /// back with its payload gone and a fresh handle, and the lazily filed
    /// layout state resets a freed key's entry so its next occupant starts
    /// clean.
    #[test]
    fn a_freed_key_comes_back_clean() {
        let mut arenas: TreeArenas<u32> = TreeArenas::new();
        arenas.reserve_zero();
        let mut layout = DocumentLayoutState::new();
        let mut issued = Vec::new();
        for payload in 0..4 {
            let id = arenas.insert_node(text(payload));
            assert_eq!(arenas.get(id).map(Node::payload), Some(&payload));
            issued.push(id);
        }

        let freed = issued[1];
        layout.at_mut(freed).slot.static_position.x = 7.0;
        layout
            .scroll_offsets
            .insert(freed, euclid::default::Vector2D::new(7.0, 0.0));
        layout
            .scroll_reaches
            .insert(freed, euclid::default::Vector2D::new(0.0, 120.0));
        let node = arenas.remove_node(freed);
        layout.remove(freed);
        assert_eq!(
            node.into_payload(),
            Some(1),
            "the payload leaves with its node"
        );
        assert_eq!(arenas.slot(freed), None);

        let next = arenas.insert_node(text(9));
        assert_eq!(
            next.arena_key(),
            freed.arena_key(),
            "the freed storage comes back"
        );
        assert_ne!(next, freed, "but the handle to it does not");
        assert_eq!(arenas.get(next).map(Node::payload), Some(&9));
        assert!(
            layout.slot_at(next).static_position.x.to_bits() == 0,
            "reused storage must not inherit the freed node's layout state"
        );
        assert!(
            layout.scroll_offsets.is_empty(),
            "a freed scroll container's offset leaves with it"
        );
        assert!(layout.scroll_reaches.is_empty(), "and so does its reach");
    }

    /// Layout state is filed only for the keys something writes: a covered
    /// key with no entry reads as never laid out, a key past the index reads
    /// as nothing, and a text node's box writes file no entry at all.
    #[test]
    fn layout_state_is_filed_only_for_the_nodes_that_lay_out() {
        let mut arenas: TreeArenas<u32> = TreeArenas::new();
        arenas.reserve_zero();
        let mut layout = DocumentLayoutState::new();
        let element = arenas.insert_node(|owner, id| {
            Node::new_element(owner, id, stylo::LocalName::from("view")).with_payload(0)
        });
        let run = arenas.insert_node(text(1));
        assert!(layout.get(element).is_none(), "past the index: nothing");

        layout.ensure_covers(arenas.slot_bound(), arenas.live_elements());
        assert!(
            layout
                .get(element)
                .is_some_and(|state| state.slot.layout_cache_is_empty() && state.text.is_none()),
            "covered with no entry: never laid out"
        );
        assert!(!layout.has_entry(element) && layout.entries.is_empty());
        assert!(!layout.has_entry(run), "a text node is never written");

        layout.slot_mut(element).mark_hidden();
        assert!(layout.has_entry(element));
        assert!(layout.slot_at(element).is_hidden());
        assert!(!layout.has_entry(run));
        assert_eq!(layout.entries.len(), 1);
    }

    /// A page's first layout files its entries into the one reservation the
    /// live element count sizes, rather than doubling `entries` through
    /// `push`: only elements file an entry, so the count bounds them.
    #[test]
    fn a_first_layout_files_its_entries_into_one_reservation() {
        let mut document = crate::tree::document::Document::new(
            crate::tree::document::tests::device(),
            "page",
            (),
        );
        let root = document.document_element().id();
        for _ in 0..100 {
            let view = document.create_element("view", ());
            document.append_child(root, view);
            let text = document.create_element("text", ());
            document.append_child(view, text);
            let run = document.create_text_node("run", ());
            document.append_child(text, run);
        }
        document.layout();

        let elements = document.arenas().live_elements();
        let entries = &document.layout_state().entries;
        assert!(entries.len() > 100 && entries.len() <= elements);
        assert_eq!(entries.capacity(), elements, "one exact reservation");
    }

    /// A handle nobody ever made resolves to nothing rather than indexing past
    /// the arena. Script can name any integer, and the host boundary turns this
    /// `None` into a JavaScript error instead of a crash.
    #[test]
    fn a_handle_that_was_never_issued_resolves_to_nothing() {
        let mut arenas: TreeArenas<u32> = TreeArenas::new();
        arenas.reserve_zero();
        let id = arenas.insert_node(text(0));
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
        let held = arenas.insert_node(text(1));
        assert!(arenas.slot_is_current(held));
        assert!(arenas.try_at(held).is_some());

        arenas.remove_node(held);
        assert!(
            !arenas.slot_is_current(held),
            "freeing invalidates every handle to that node"
        );
        assert!(arenas.try_at(held).is_none());

        let reused = arenas.insert_node(text(2));
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
