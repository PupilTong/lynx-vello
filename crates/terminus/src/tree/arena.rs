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
use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

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

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(NonZeroU64);

pub(crate) type NodeSlot = NodeId;

const GENERATION_BITS: u32 = 21;

const KEY_BITS: u32 = 32;
const KEY_MASK: u64 = (1 << KEY_BITS) - 1;

impl NodeId {
    pub const DOCUMENT_ELEMENT: Self = DOCUMENT_ELEMENT_NODE_ID;

    #[inline]
    pub(crate) const fn arena_key(self) -> usize {
        (self.0.get() & KEY_MASK) as usize
    }

    #[inline]
    const fn generation(self) -> u32 {
        (self.0.get() >> KEY_BITS) as u32
    }

    #[must_use]
    pub const fn to_bits(self) -> u64 {
        self.0.get()
    }

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

const RESERVED: usize = 0;

pub(crate) const DOCUMENT_NODE_ID: NodeId = NodeId::at_key(NonZeroU32::new(1).unwrap(), 0);
pub(crate) const DOCUMENT_ELEMENT_NODE_ID: NodeId = NodeId::at_key(NonZeroU32::new(2).unwrap(), 0);
const INITIAL_NODE_CAPACITY: usize = 8;

pub(crate) struct TreeArenas<T> {
    nodes: Slab<Node<T>>,
    generations: Vec<u32>,
    relevance: RelevanceTable,
    committed_boxes: CommittedBoxTable,
    anchors: AnchorRegistry,
    top_layer: TopLayer,
    anonymous: AnonymousBoxes,
    live_elements: usize,
}

pub(crate) struct ArenaPtr<T>(NonNull<TreeArenas<T>>);

impl<T> Clone for ArenaPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ArenaPtr<T> {}

impl<T> ArenaPtr<T> {
    #[inline]
    pub(crate) const fn as_ptr(self) -> *const TreeArenas<T> {
        self.0.as_ptr().cast_const()
    }
}

pub(crate) struct ArenaBox<T> {
    ptr: NonNull<TreeArenas<T>>,
    _owns: PhantomData<TreeArenas<T>>,
}

impl<T> ArenaBox<T> {
    pub(crate) fn new() -> Self {
        let mut arenas = Self {
            ptr: NonNull::from(Box::leak(Box::new(TreeArenas::new()))),
            _owns: PhantomData,
        };
        let owner = arenas.owner();
        arenas.reserve_zero(owner);
        arenas
    }

    #[inline]
    const fn owner(&self) -> ArenaPtr<T> {
        ArenaPtr(self.ptr)
    }

    #[expect(
        clippy::inline_always,
        reason = "keep the slab insert in the node-allocation hot path"
    )]
    #[inline(always)]
    pub(crate) fn insert_node(
        &mut self,
        make: impl FnOnce(ArenaPtr<T>, NodeId) -> Node<T>,
    ) -> NodeId {
        let owner = self.owner();
        self.file_node(owner, make)
    }
}

impl<T> Deref for ArenaBox<T> {
    type Target = TreeArenas<T>;

    #[inline]
    fn deref(&self) -> &TreeArenas<T> {
        #[expect(unsafe_code, reason = "lend the arena set from its owning pointer")]
        // SAFETY: `ptr` is the allocation `new` leaked, initialised there and
        // freed only by `drop`, so it is valid for as long as `self` is. The
        // shared borrow is tied to `&self`, and the only `&mut TreeArenas<T>`
        // there is comes from `deref_mut`, which needs `&mut self`, so none is
        // live beside it. The borrow is derived from `ptr` itself, so using it
        // leaves every node's copy of `ptr` usable (`Node::arenas`).
        unsafe {
            self.ptr.as_ref()
        }
    }
}

impl<T> DerefMut for ArenaBox<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut TreeArenas<T> {
        #[expect(unsafe_code, reason = "lend the arena set from its owning pointer")]
        // SAFETY: `ptr` is valid for as long as `self` is, as in `deref`. The
        // exclusive borrow is tied to `&mut self`, so no borrow lent by
        // `deref` or `deref_mut`, and no `&Node` or `&TreeArenas<T>` reached
        // through one ([`Node::arenas`] included, whose result borrows the
        // node it was called on), is live beside it. The borrow is derived
        // from `ptr`, so writing through it leaves every node's copy of `ptr`
        // usable.
        unsafe {
            self.ptr.as_mut()
        }
    }
}

impl<T> Drop for ArenaBox<T> {
    fn drop(&mut self) {
        #[expect(unsafe_code, reason = "free the arena set's allocation")]
        // SAFETY: `ptr` came from `Box::leak` of a `Box<TreeArenas<T>>` in
        // `new`, and this is the one place it is turned back into one, once.
        // `&mut self` means no borrow lent by `deref`/`deref_mut` is live.
        // Every node carrying a copy of `ptr` is either inside the arena set,
        // and dropped with it here, or was handed back by `remove_node` and
        // dropped by the document before that call returned; dropping a node
        // reads nothing through its copy.
        drop(unsafe { Box::from_raw(self.ptr.as_ptr()) });
    }
}

impl<T> TreeArenas<T> {
    fn new() -> Self {
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

    #[inline]
    pub(crate) fn anonymous(&self) -> &AnonymousBoxes {
        &self.anonymous
    }

    #[inline]
    pub(crate) fn anonymous_mut(&mut self) -> &mut AnonymousBoxes {
        &mut self.anonymous
    }

    #[inline]
    pub(crate) fn is_anonymous_box(&self, id: NodeId) -> bool {
        self.get(id).is_some_and(Node::is_anonymous_box)
    }

    pub(crate) fn anonymous_container(&self, id: NodeId) -> Option<NodeId> {
        if !self.is_anonymous_box(id) {
            return None;
        }
        let parent = self.get(self.get(id)?.parent_id()?)?;
        Some(parent.shadow_host_id().unwrap_or_else(|| parent.id()))
    }

    #[inline]
    pub(crate) fn top_layer(&self) -> &TopLayer {
        &self.top_layer
    }

    #[inline]
    pub(crate) fn top_layer_mut(&mut self) -> &mut TopLayer {
        &mut self.top_layer
    }

    #[inline]
    pub(crate) fn anchors(&self) -> &AnchorRegistry {
        &self.anchors
    }

    #[inline]
    pub(crate) fn anchors_mut(&mut self) -> &mut AnchorRegistry {
        &mut self.anchors
    }

    #[inline]
    pub(crate) fn relevance(&self, slot: NodeId) -> Relevance {
        self.relevance.state(slot.arena_key())
    }

    #[inline]
    pub(crate) fn relevance_freshness(&self, slot: NodeId) -> Freshness {
        self.relevance.freshness(slot.arena_key())
    }

    pub(crate) fn determine_relevance(&mut self, slot: NodeId, state: Relevance) -> bool {
        let flipped = self.relevance.determine(slot.arena_key(), state);
        if flipped {
            self.anchors.note_relevance_flip();
        }
        flipped
    }

    #[inline]
    pub(crate) const fn relevance_owed(&self) -> bool {
        self.relevance.owed()
    }

    #[inline]
    pub(crate) fn set_relevance_owed(&mut self, owed: bool) {
        self.relevance.set_owed(owed);
    }

    #[inline]
    pub(crate) fn take_relevance_owed(&mut self) -> bool {
        self.relevance.take_owed()
    }

    pub(crate) fn settle_relevance(&mut self) {
        self.relevance.settle();
    }

    #[inline]
    pub(crate) fn committed_box(&self, slot: NodeId) -> CommittedBox {
        self.committed_boxes.get(slot.arena_key())
    }

    #[inline]
    pub(crate) fn note_committed_box(&self, slot: NodeId, committed: CommittedBox) {
        self.committed_boxes.note(slot, committed);
    }

    #[inline]
    pub(crate) fn publish_committed_boxes(&mut self, resized: &mut Vec<NodeId>) {
        self.committed_boxes.apply(resized);
    }

    #[inline]
    pub(crate) fn uses_container_units(&self) -> bool {
        self.committed_boxes.uses_container_units()
    }

    #[inline]
    pub(crate) fn container_units_flag(&self) -> &std::sync::atomic::AtomicBool {
        self.committed_boxes.units_flag()
    }

    fn reserve_zero(&mut self, owner: ArenaPtr<T>) {
        debug_assert!(
            self.generations.is_empty(),
            "zero is reserved before any node"
        );
        let placeholder = NodeId::at_key(NonZeroU32::new(1).expect("one is non-zero"), 0);
        assert_eq!(
            self.nodes
                .insert(Node::new_text(owner, placeholder, Box::default())),
            RESERVED
        );
        self.generations.push(0);
    }

    #[inline]
    pub(crate) fn slot_bound(&self) -> usize {
        self.generations.len()
    }

    #[inline]
    pub(crate) const fn live_elements(&self) -> usize {
        self.live_elements
    }

    #[inline]
    pub(crate) fn slot_is_current(&self, slot: NodeId) -> bool {
        self.generations
            .get(slot.arena_key())
            .is_some_and(|generation| *generation == slot.generation())
    }

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

    #[inline]
    pub(crate) fn try_at(&self, slot: NodeId) -> Option<&Node<T>> {
        let node = self.nodes.get(slot.arena_key())?;
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

    pub(crate) fn insert_child(&mut self, parent: NodeId, index: usize, child: NodeId) {
        let parent_node = self.live_mut(parent);
        let mut children = std::mem::take(&mut parent_node.children);
        let mut base = parent_node.child_index_base;
        if index < children.len() - index {
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

    fn shift_child_indexes(&mut self, children: &[NodeId], by: u32) {
        for &slot in children {
            let node = self.live_mut(slot);
            node.child_index = node.child_index.wrapping_add(by);
        }
    }

    pub(crate) const fn nodes(&self) -> &Slab<Node<T>> {
        &self.nodes
    }

    #[inline]
    pub(crate) fn id_at_arena_key(&self, key: usize) -> Option<NodeId> {
        self.nodes
            .get(key)
            .map(Node::id)
            .filter(|id| id.arena_key() == key)
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes
            .iter()
            .map(|(key, node)| (key, node.id()))
            .filter(|&(key, id)| id.arena_key() == key)
            .map(|(_, id)| id)
    }

    #[expect(
        clippy::inline_always,
        reason = "keep the slab insert in the node-allocation hot path"
    )]
    #[inline(always)]
    fn file_node(
        &mut self,
        owner: ArenaPtr<T>,
        make: impl FnOnce(ArenaPtr<T>, NodeId) -> Node<T>,
    ) -> NodeId {
        let arena_key = self.nodes.vacant_key();
        let key = u32::try_from(arena_key).expect("a document holds fewer than u32::MAX nodes");
        let key = NonZeroU32::new(key).expect("arena key zero is reserved and never allocated");
        if arena_key == self.generations.len() {
            self.generations.push(0);
        }
        let id = NodeId::at_key(key, self.generations[arena_key]);
        let node = make(owner, id);
        self.live_elements += usize::from(node.is_element());
        assert_eq!(self.nodes.insert(node), arena_key);
        id
    }

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
        let key = id.arena_key();
        let generation = self.generations[key] + 1;
        self.generations[key] = generation;
        if generation == 1 << GENERATION_BITS {
            self.retire_key(key, node.owner());
        }
        self.relevance.reset(key);
        self.committed_boxes.reset(key);
        self.anchors.forget(id);
        node
    }

    #[cold]
    fn retire_key(&mut self, key: usize, owner: ArenaPtr<T>) {
        let placeholder = if key == DOCUMENT_NODE_ID.arena_key() {
            DOCUMENT_ELEMENT_NODE_ID
        } else {
            DOCUMENT_NODE_ID
        };
        assert_eq!(self.nodes.vacant_key(), key);
        assert_eq!(
            self.nodes
                .insert(Node::new_text(owner, placeholder, Box::default())),
            key
        );
    }
}

#[derive(Default)]
pub(crate) struct NodeLayoutState {
    pub(crate) slot: LayoutSlot,
    pub(crate) text: Option<Box<TextStore>>,
}

const NO_ENTRY: u32 = u32::MAX;

pub(crate) struct DocumentLayoutState {
    index: Vec<u32>,
    entries: Vec<NodeLayoutState>,
    vacant: NodeLayoutState,
    pub(crate) text_context: Option<Box<TextContext>>,
    probed_text: Vec<NodeId>,
    interleaves_containers: bool,
    container_deferrals: Vec<DeferredContainer>,
    pub(crate) initial_targets: Vec<(NodeId, NodeId)>,
    pub(crate) scroll_requests: FxHashMap<NodeId, ScrollRequest>,
    pub(crate) scroll_offsets: FxHashMap<NodeId, euclid::default::Vector2D<f32>>,
    pub(crate) scroll_anchors: FxHashMap<NodeId, crate::scroll::anchoring::ScrollAnchor>,
    pub(crate) scroll_reaches: FxHashMap<NodeId, euclid::default::Vector2D<f32>>,
    pub(crate) next_scroll_request: NonZeroU64,
    pub(crate) auto_scroll_requests: Vec<(NodeId, AutoScrollRequest)>,
    pub(crate) instant_scrolls: SmallVec<[(NodeId, euclid::default::Vector2D<f32>); 2]>,
    pub(crate) sticky_containing_blocks: Vec<StickyContainingBlock>,
    pub(crate) anchored: FxHashMap<NodeId, AnchoredBox>,
    pub(crate) anchor_pending: PendingReads,
    pub(crate) anchor_buckets: AnchorBuckets,
    pub(crate) hoisted_to: FxHashMap<NodeId, SmallVec<[NodeId; 2]>>,
    pub(crate) in_rounding_tail: bool,
    pub(crate) text_layout_reporters: FxHashSet<NodeId>,
    pub(crate) text_layout_reports: Vec<NodeId>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StickyContainingBlock {
    pub(crate) node: NodeId,
    pub(crate) unrounded: Edges<f32>,
    pub(crate) rounded: Option<Edges<f32>>,
}

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

    pub(crate) fn take_container_deferrals(&mut self, sink: &mut Vec<DeferredContainer>) {
        sink.clear();
        sink.append(&mut self.container_deferrals);
    }

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

    #[inline]
    fn entry_of(&self, slot: NodeId) -> Option<usize> {
        match self.index.get(slot.arena_key()) {
            Some(&at) if at != NO_ENTRY => Some(at as usize),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn has_entry(&self, slot: NodeId) -> bool {
        self.entry_of(slot).is_some()
    }

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

    fn entry_mut(&mut self, slot: NodeId) -> &mut NodeLayoutState {
        let at = self.file_entry(slot);
        &mut self.entries[at]
    }

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

    #[inline]
    pub(crate) fn note_text_layout(&mut self, slot: NodeId) {
        if !self.text_layout_reporters.is_empty() && self.text_layout_reporters.contains(&slot) {
            self.text_layout_reports.push(slot);
        }
    }

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

    #[inline]
    pub(crate) fn get(&self, slot: NodeId) -> Option<&NodeLayoutState> {
        let &at = self.index.get(slot.arena_key())?;
        Some(if at == NO_ENTRY {
            &self.vacant
        } else {
            &self.entries[at as usize]
        })
    }

    #[inline]
    pub(crate) fn slot_at(&self, slot: NodeId) -> &LayoutSlot {
        match self.entry_of(slot) {
            Some(at) => &self.entries[at].slot,
            None => &self.vacant.slot,
        }
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut NodeLayoutState> {
        self.entries.iter_mut()
    }

    #[inline]
    pub(crate) fn at_mut(&mut self, slot: NodeId) -> &mut NodeLayoutState {
        self.entry_mut(slot)
    }

    #[inline]
    pub(crate) fn slot_mut(&mut self, slot: NodeId) -> &mut LayoutSlot {
        &mut self.entry_mut(slot).slot
    }

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

    pub(crate) fn text_store(&self, slot: NodeId) -> Option<&TextStore> {
        self.get(slot).and_then(|node| node.text.as_deref())
    }

    pub(crate) fn text_block(&self, slot: NodeId) -> Option<&TextBlockStore> {
        self.text_store(slot).and_then(TextStore::as_block)
    }

    pub(crate) fn note_probed_text(&mut self, slot: NodeId) {
        if self.text_store(slot).is_some_and(TextStore::is_probe_dirty) {
            self.probed_text.push(slot);
        }
    }

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
        debug_assert!(
            !self
                .entries
                .iter()
                .filter_map(|node| node.text.as_deref())
                .any(TextStore::is_probe_dirty),
            "a probed text block never reached the restore queue"
        );
    }

    pub(crate) fn clear_box_cache(&mut self, slot: NodeId) {
        if let Some(at) = self.entry_of(slot) {
            self.entries[at].slot.clear_layout_cache();
        }
    }

    pub(crate) fn clear_text_artifact(&mut self, slot: NodeId) {
        if let Some(at) = self.entry_of(slot) {
            self.entries[at].text = None;
        }
    }

    pub(crate) fn clear_layout_cache(&mut self, slot: NodeId) {
        self.clear_box_cache(slot);
        self.clear_text_artifact(slot);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn text(payload: u32) -> impl FnOnce(ArenaPtr<u32>, NodeId) -> Node<u32> {
        move |owner, id| Node::new_text(owner, id, Box::default()).with_payload(payload)
    }

    #[test]
    fn a_freed_key_comes_back_clean() {
        let mut arenas: ArenaBox<u32> = ArenaBox::new();
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

    #[test]
    fn a_removed_node_frees_its_key_for_the_next_generation() {
        let mut arenas: ArenaBox<u32> = ArenaBox::new();
        arenas.insert_node(text(0));
        let first = arenas.insert_node(text(1));
        arenas.remove_node(first);
        let next = arenas.insert_node(text(2));
        assert_eq!(next.arena_key(), first.arena_key());
        assert_eq!(next.generation(), first.generation() + 1);
        assert_eq!(NodeId::from_bits(next.to_bits()), Some(next));
    }

    #[test]
    fn a_key_at_the_last_generation_is_retired_on_removal() {
        let mut arenas: ArenaBox<u32> = ArenaBox::new();
        let kept = arenas.insert_node(text(0));
        let probe = arenas.insert_node(text(1));
        let retired = probe.arena_key();
        arenas.remove_node(probe);
        arenas.generations[retired] = (1 << GENERATION_BITS) - 1;

        let last = arenas.insert_node(text(2));
        assert_eq!(last.arena_key(), retired);
        assert_eq!(NodeId::from_bits(last.to_bits()), Some(last));
        assert!(arenas.contains(last));

        arenas.remove_node(last);
        assert!(!arenas.contains(last));
        assert!(!arenas.slot_is_current(last));
        let after = arenas.insert_node(text(3));
        assert_ne!(after.arena_key(), retired);
        assert_eq!(NodeId::from_bits(after.to_bits()), Some(after));
        assert_eq!(arenas.get(after).map(Node::payload), Some(&3));
        assert!(arenas.ids().all(|id| id.arena_key() != retired));
        assert_eq!(arenas.ids().collect::<Vec<_>>(), vec![kept, after]);
        assert_eq!(arenas.id_at_arena_key(retired), None);
        assert_eq!(arenas.id_at_arena_key(RESERVED), None);
        assert_eq!(arenas.id_at_arena_key(kept.arena_key()), Some(kept));
    }

    #[test]
    fn layout_state_is_filed_only_for_the_nodes_that_lay_out() {
        let mut arenas: ArenaBox<u32> = ArenaBox::new();
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

    #[test]
    fn a_nodes_arena_address_survives_later_writes_and_moves() {
        #[inline(never)]
        fn moved<A>(arenas: A) -> A {
            arenas
        }

        let mut arenas: ArenaBox<u32> = ArenaBox::new();
        let first = arenas.insert_node(text(0));
        let freed = arenas.insert_node(text(1));
        let first_arenas = std::ptr::from_ref(arenas.at(first).arenas());
        arenas.remove_node(freed);
        let mut arenas = moved(arenas);
        let last = arenas.insert_node(text(2));
        arenas
            .get_mut(last)
            .expect("just filed")
            .set_has_line_if_empty(true);

        let address = std::ptr::from_ref::<TreeArenas<u32>>(&arenas);
        for id in [first, last] {
            let node = arenas.at(id);
            assert!(std::ptr::eq(node.arenas(), address));
            assert_eq!(
                node.arenas().get(id).map(Node::payload),
                Some(node.payload())
            );
        }
        assert!(
            std::ptr::eq(first_arenas, address),
            "one address throughout"
        );
    }

    #[test]
    fn a_handle_that_was_never_issued_resolves_to_nothing() {
        let mut arenas: ArenaBox<u32> = ArenaBox::new();
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

    #[test]
    fn a_handle_is_eight_bytes_and_its_option_adds_nothing() {
        assert_eq!(size_of::<NodeId>(), 8);
        assert_eq!(size_of::<Option<NodeId>>(), 8);
    }

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

    #[test]
    fn a_handle_held_across_its_nodes_free_is_refused_even_once_reused() {
        let mut arenas: ArenaBox<u32> = ArenaBox::new();
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
