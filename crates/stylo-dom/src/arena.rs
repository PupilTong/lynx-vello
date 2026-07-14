//! The generational arena backing the DOM node tree.
//!
//! Element and Text nodes live in a hand-rolled `Vec<Slot>` arena with a free
//! list — no `slotmap` dependency, deliberately minimal. Each node is
//! addressed by a [`NodeId`] carrying the slot index plus the slot's
//! generation; once a slot is freed its generation is bumped, so any id referring to
//! the previous occupant becomes stale and resolves to `None`.
//!
//! [`NodeRef`] exposes raw DOM navigation and implements Stylo's `TNode`;
//! [`ElementRef`] is only constructed for Element nodes and implements
//! Stylo's element traits (see [`crate::traits`]).
//!
//! The arena also owns the **pending snapshot set** for stylo's
//! invalidation-set restyle: before a matching-relevant mutation, the embedder
//! layer records the element's old state/attributes here (see
//! [`crate::dirty`]), and the next
//! [`StyleEngine::flush_tree`](crate::StyleEngine::flush_tree) consumes them.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::atomic::{AtomicU64, Ordering};

use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::UrlExtraData;

use crate::element::Element;
use crate::ext::ExternalState;
use crate::node::{Node, TextNode};

/// The placeholder base URL for parsing a standalone arena's inline styles.
///
/// `about:blank` is a constant, valid URL, so this never fails.
fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

static NEXT_LAYOUT_IDENTITY: AtomicU64 = AtomicU64::new(1);

fn next_layout_identity() -> NonZeroU64 {
    let identity = NEXT_LAYOUT_IDENTITY
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .expect("DOM arena layout identity space exhausted");
    NonZeroU64::new(identity).expect("DOM arena layout identities start at one")
}

/// A stable, generation-checked handle to a DOM node in an [`Arena`].
///
/// Cheap to copy and hash. A handle stays valid until its node is removed;
/// afterwards the slot's generation advances and the handle becomes stale
/// (arena lookups return `None`), even if the slot is later reused by a
/// different node.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId {
    index: u32,
    generation: NonZeroU32,
}

impl NodeId {
    /// The slot index this handle refers to.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// The generation this handle was minted with.
    #[must_use]
    pub const fn generation(self) -> NonZeroU32 {
        self.generation
    }

    /// The stable stylo [`OpaqueNode`] identity for this handle.
    ///
    /// stylo keys its [`SnapshotMap`] and traversal roots by `OpaqueNode`.
    /// Deriving it from the id (rather than the element's address) keeps it
    /// stable across arena growth, which can reallocate and move every
    /// element.
    #[must_use]
    pub(crate) const fn opaque(self) -> OpaqueNode {
        // Packs (generation, index) into the usize. 64-bit targets only —
        // on 32-bit this would truncate the generation.
        const {
            assert!(
                size_of::<usize>() >= 8,
                "NodeId::opaque requires a 64-bit target"
            );
        }
        OpaqueNode(((self.generation.get() as usize) << 32) | self.index as usize)
    }
}

/// Backwards-compatible name for an id known by the caller to identify an
/// [`Element`]. Text-node APIs use [`NodeId`] directly.
pub type ElementId = NodeId;

/// One arena slot: the current generation plus an optional live DOM node.
#[derive(Debug)]
struct Slot<T> {
    generation: NonZeroU32,
    node: Option<Node<T>>,
}

/// A generational arena of [`Node`]s.
///
/// The arena owns the [`SharedRwLock`] and [`UrlExtraData`] used to parse and
/// guard every element's inline style block. [`StyleEngine`](crate::StyleEngine)
/// creates styled arenas with the matching private context; embedders do not
/// pass locks across crate boundaries.
pub struct Arena<T> {
    /// Process-unique identity used to separate retained layout sessions.
    layout_identity: NonZeroU64,
    slots: Vec<Slot<T>>,
    free_list: Vec<u32>,
    lock: SharedRwLock,
    url_data: UrlExtraData,
    /// Pre-mutation element snapshots pending the next flush, keyed by the
    /// element's stable [`OpaqueNode`] (see [`ElementId::opaque`]).
    snapshots: SnapshotMap,
    /// The ids behind [`Arena::snapshots`], so `complete_flush` can clear the
    /// per-element snapshot bits without a way to map `OpaqueNode` back.
    snapshotted: Vec<ElementId>,
    /// Monotonic epoch for layout-observable DOM/style changes.
    layout_revision: u64,
}

impl<T: fmt::Debug> fmt::Debug for Arena<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `SnapshotMap` is not `Debug`; report its size instead.
        f.debug_struct("Arena")
            .field("layout_identity", &self.layout_identity)
            .field("slots", &self.slots)
            .field("free_list", &self.free_list)
            .field("pending_snapshots", &self.snapshotted.len())
            .finish_non_exhaustive()
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Arena<T> {
    /// Create an empty arena with a freshly minted [`SharedRwLock`] and a
    /// placeholder `about:blank` [`UrlExtraData`].
    ///
    /// A standalone arena (DOM-only, never styled) can use this. Styled trees
    /// should be created by [`StyleEngine::new_arena`](crate::StyleEngine::new_arena).
    #[must_use]
    pub fn new() -> Self {
        Self::with_style_context(SharedRwLock::new(), about_blank_url_data())
    }

    /// Create an arena with the style context owned by this crate.
    pub(crate) fn with_style_context(lock: SharedRwLock, url_data: UrlExtraData) -> Self {
        Self {
            layout_identity: next_layout_identity(),
            slots: Vec::new(),
            free_list: Vec::new(),
            lock,
            url_data,
            snapshots: SnapshotMap::new(),
            snapshotted: Vec::new(),
            layout_revision: 0,
        }
    }

    /// The shared lock guarding this arena's inline style blocks.
    #[must_use]
    pub(crate) fn shared_lock(&self) -> &SharedRwLock {
        &self.lock
    }

    /// The base URL data used when parsing this arena's inline styles.
    #[must_use]
    pub(crate) fn url_data(&self) -> &UrlExtraData {
        &self.url_data
    }

    /// The pending pre-mutation snapshots, consumed by the flush traversal.
    #[must_use]
    pub(crate) fn snapshot_map(&self) -> &SnapshotMap {
        &self.snapshots
    }

    pub(crate) fn snapshot_map_mut(&mut self) -> (&mut SnapshotMap, &mut Vec<ElementId>) {
        (&mut self.snapshots, &mut self.snapshotted)
    }

    /// The current monotonic layout-observable mutation epoch.
    ///
    /// Layout sessions can conservatively invalidate retained measurements
    /// whenever this changes instead of fingerprinting a copied host tree.
    #[must_use]
    pub const fn layout_revision(&self) -> u64 {
        self.layout_revision
    }

    /// Process-unique identity for retained layout-session epochs.
    #[must_use]
    pub(crate) const fn layout_identity(&self) -> NonZeroU64 {
        self.layout_identity
    }

    /// Advance the layout mutation epoch.
    pub(crate) fn bump_layout_revision(&mut self) {
        self.layout_revision = self.layout_revision.saturating_add(1);
    }

    /// Insert an element and return its handle.
    ///
    /// # Panics
    ///
    /// Panics if the arena would need to grow past `u32::MAX` slots.
    pub fn insert(&mut self, element: Element<T>) -> ElementId {
        self.insert_node(Node::Element(element))
    }

    /// Insert a detached Text node and return its handle.
    ///
    /// Text nodes have no external-state payload and no per-element Stylo
    /// data. Attach the returned id with [`Arena::attach_at`].
    pub fn insert_text(&mut self, data: impl Into<String>) -> NodeId {
        self.insert_node(Node::Text(TextNode::new(data)))
    }

    /// Insert a detached DOM node and return its handle.
    ///
    /// # Panics
    ///
    /// Panics if the arena would need to grow past `u32::MAX` slots.
    pub fn insert_node(&mut self, node: Node<T>) -> NodeId {
        let id = if let Some(index) = self.free_list.pop() {
            let slot = &mut self.slots[index as usize];
            debug_assert!(slot.node.is_none(), "free-list slot must be vacant");
            slot.node = Some(node);
            NodeId {
                index,
                generation: slot.generation,
            }
        } else {
            let index =
                u32::try_from(self.slots.len()).expect("arena capacity exceeds u32::MAX slots");
            self.slots.push(Slot {
                generation: NonZeroU32::MIN,
                node: Some(node),
            });
            NodeId {
                index,
                generation: NonZeroU32::MIN,
            }
        };
        self.bump_layout_revision();
        id
    }

    /// Remove an element, returning it if the handle identifies a live
    /// Element node.
    ///
    /// This compatibility API leaves Text nodes untouched. Use
    /// [`Arena::remove_node`] when the node kind is not statically known.
    ///
    /// The slot's generation is advanced so the passed handle (and any other
    /// handle sharing the slot) becomes stale. If a slot's generation is
    /// exhausted it is retired rather than reused, preserving uniqueness.
    pub fn remove(&mut self, id: ElementId) -> Option<Element<T>> {
        self.get(id)?;
        match self.remove_node(id)? {
            Node::Element(element) => Some(element),
            Node::Text(_) => unreachable!("get() only resolves Element nodes"),
        }
    }

    /// Remove any live DOM node and return it.
    ///
    /// The slot's generation is advanced so the passed handle (and any other
    /// handle sharing the slot) becomes stale. If a slot's generation is
    /// exhausted it is retired rather than reused, preserving uniqueness.
    pub fn remove_node(&mut self, id: NodeId) -> Option<Node<T>> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        let node = slot.node.take()?;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free_list.push(id.index);
        } else {
            // Generation space exhausted for this slot: retire it (never
            // reuse) so no future handle can collide with a past one.
        }
        // A dead element's pending snapshot must not survive it; the map entry
        // is keyed by the (now stale) opaque id and is dropped with the map on
        // the next `complete_flush`. Removing it eagerly keeps the map small.
        if let Node::Element(element) = &node
            && element.snapshot_present()
        {
            self.snapshots.remove(&id.opaque());
        }
        self.bump_layout_revision();
        Some(node)
    }

    /// Borrow an element if the handle is live.
    #[must_use]
    pub fn get(&self, id: ElementId) -> Option<&Element<T>> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation {
            slot.node.as_ref()?.as_element()
        } else {
            None
        }
    }

    /// Mutably borrow an element if the handle is live.
    ///
    /// Obtaining this borrow conservatively advances the layout revision,
    /// because the public fields include topology and embedder policy that a
    /// formatting source can observe. Call the arena's dedicated mutation
    /// helpers when selector/style invalidation is also required.
    pub fn get_mut(&mut self, id: ElementId) -> Option<&mut Element<T>> {
        let index = id.index as usize;
        let is_live_element = self.slots.get(index).is_some_and(|slot| {
            slot.generation == id.generation
                && slot
                    .node
                    .as_ref()
                    .is_some_and(|node| node.as_element().is_some())
        });
        if !is_live_element {
            return None;
        }
        self.bump_layout_revision();
        self.slots[index].node.as_mut()?.as_element_mut()
    }

    /// Borrow any node if the handle is live.
    #[must_use]
    pub fn get_node(&self, id: NodeId) -> Option<&Node<T>> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation {
            slot.node.as_ref()
        } else {
            None
        }
    }

    /// Mutably borrow any node if the handle is live.
    ///
    /// Obtaining this borrow conservatively advances the layout revision.
    /// Structural/style-aware callers should still prefer the dedicated arena
    /// mutation helpers so selector invalidation is scheduled as well.
    pub fn get_node_mut(&mut self, id: NodeId) -> Option<&mut Node<T>> {
        let index = id.index as usize;
        let is_live = self
            .slots
            .get(index)
            .is_some_and(|slot| slot.generation == id.generation && slot.node.is_some());
        if !is_live {
            return None;
        }
        self.bump_layout_revision();
        self.slots[index].node.as_mut()
    }

    /// Borrow a Text node if the handle identifies one.
    #[must_use]
    pub fn text(&self, id: NodeId) -> Option<&TextNode> {
        self.get_node(id)?.as_text()
    }

    /// Mutably borrow a Text node if the handle identifies one.
    ///
    /// Direct mutation advances the layout revision but bypasses `:empty`
    /// invalidation; prefer [`Arena::set_text`] for attached nodes.
    pub fn text_mut(&mut self, id: NodeId) -> Option<&mut TextNode> {
        self.get_node_mut(id)?.as_text_mut()
    }

    /// Replace a Text node's character data.
    ///
    /// This schedules the parent for selector invalidation because changing
    /// between empty and non-empty data can change whether the parent matches
    /// `:empty`. Returns `false` for a stale id or an Element node.
    pub fn set_text(&mut self, id: NodeId, data: impl Into<String>) -> bool {
        let data = data.into();
        let (parent, changed) = match self.text(id) {
            Some(text) => (text.parent, text.data() != data),
            None => return false,
        };
        if !changed {
            return true;
        }
        let Some(text) = self.text_mut(id) else {
            return false;
        };
        text.set_data(data);
        if let Some(parent) = parent {
            let index = self.child_position(parent, id).unwrap_or_default();
            self.note_child_list_change(parent, index);
        }
        true
    }

    /// Whether the handle currently resolves to any live DOM node.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.get_node(id).is_some()
    }

    /// A read-only navigation handle for the element, if live.
    #[must_use]
    pub fn element_ref(&self, id: ElementId) -> Option<ElementRef<'_, T>> {
        if self.get(id).is_some() {
            Some(ElementRef { arena: self, id })
        } else {
            None
        }
    }

    /// A read-only navigation handle for any live DOM node.
    #[must_use]
    pub fn node_ref(&self, id: NodeId) -> Option<NodeRef<'_, T>> {
        if self.contains(id) {
            Some(NodeRef { arena: self, id })
        } else {
            None
        }
    }

    /// The Element that stands in for this arena's Document, when one exists.
    ///
    /// A detached Text node has no Element ancestor, but it still belongs to
    /// the arena that created it. Stylo's DOM protocol requires `owner_doc()`
    /// to remain available in that state, so node traversal falls back to the
    /// arena's distinguished root Element.
    pub(crate) fn document_element_ref(&self) -> Option<ElementRef<'_, T>>
    where
        T: ExternalState,
    {
        self.slots.iter().enumerate().find_map(|(index, slot)| {
            let Node::Element(element) = slot.node.as_ref()? else {
                return None;
            };
            if element.parent.is_some() || !element.ext.is_root() {
                return None;
            }
            Some(ElementRef {
                arena: self,
                id: NodeId {
                    index: u32::try_from(index).expect("arena index exceeds u32::MAX"),
                    generation: slot.generation,
                },
            })
        })
    }

    /// Clear every element's dirty bits.
    ///
    /// Establishes a clean baseline (tests, or an embedder resetting a tree).
    /// The flush path uses the cheaper targeted
    /// [`complete_flush`](Self::complete_flush) instead.
    pub fn clear_dirty(&mut self) {
        for slot in &mut self.slots {
            if let Some(Node::Element(element)) = &mut slot.node {
                element.set_style_dirty(false);
                element.set_dirty_descendants_bit(false);
            }
        }
    }

    /// Clear the flush-scheduling state after a style traversal: drops the
    /// consumed snapshots and walks only the dirty spine under `root` clearing
    /// the dirty bits.
    ///
    /// The spine walk cannot see below an element whose `dirty_descendants`
    /// stylo already cleared (it does so when a subtree computes to
    /// `display: none`), so `style_dirty` breadcrumbs inside such a subtree
    /// may survive — see [`Element::is_style_dirty`](crate::Element::is_style_dirty).
    pub(crate) fn complete_flush(&mut self, root: ElementId) {
        for id in std::mem::take(&mut self.snapshotted) {
            if let Some(element) = self.get(id) {
                element.clear_snapshot_flags();
            }
        }
        self.snapshots.clear();

        // Walk the dirty spine: clear `style_dirty` on every child of a
        // dirty-descendants node, but only descend where the bit is set.
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let Some(element) = self.get(current) else {
                continue;
            };
            element.set_style_dirty(false);
            if element.has_dirty_descendants() {
                element.set_dirty_descendants_bit(false);
                stack.extend(
                    element
                        .children
                        .iter()
                        .copied()
                        .filter(|&id| self.get(id).is_some()),
                );
            }
        }
    }
}

/// A `Copy` read-only handle over any live DOM node and its arena.
///
/// This is the type Stylo's [`TNode`](stylo::dom::TNode) implementation uses.
/// Only constructible via [`Arena::node_ref`], so the node is guaranteed live
/// for the handle's immutable arena borrow.
pub struct NodeRef<'a, T> {
    pub(crate) arena: &'a Arena<T>,
    pub(crate) id: NodeId,
}

// Hand-written (rather than derived) so `T` needs no `Clone`/`Copy` bound: the
// handle only holds a shared reference plus an id.
impl<T> Clone for NodeRef<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for NodeRef<'_, T> {}

impl<T> std::fmt::Debug for NodeRef<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("NodeRef");
        debug.field("id", &self.id);
        match self.node() {
            Node::Element(element) => debug.field("element", &element.tag_str()),
            Node::Text(text) => debug.field("text", &text.data()),
        };
        debug.finish()
    }
}

impl<T> PartialEq for NodeRef<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.arena, other.arena) && self.id == other.id
    }
}

impl<T> Eq for NodeRef<'_, T> {}

impl<T> std::hash::Hash for NodeRef<'_, T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (std::ptr::from_ref(self.arena) as usize).hash(state);
        self.id.hash(state);
    }
}

impl<'a, T> NodeRef<'a, T> {
    /// Borrow the underlying DOM node.
    pub(crate) fn node(self) -> &'a Node<T> {
        self.arena
            .get_node(self.id)
            .expect("NodeRef always references a live node")
    }

    /// The id for this node.
    #[must_use]
    pub const fn id(self) -> NodeId {
        self.id
    }

    /// Whether this is an Element node.
    #[must_use]
    pub fn is_element(self) -> bool {
        matches!(self.node(), Node::Element(_))
    }

    /// Whether this is a Text node.
    #[must_use]
    pub fn is_text(self) -> bool {
        matches!(self.node(), Node::Text(_))
    }

    /// Convert this node handle to an element handle when it is an Element.
    #[must_use]
    pub fn as_element(self) -> Option<ElementRef<'a, T>> {
        self.arena.element_ref(self.id)
    }

    /// Borrow the underlying Text node when this is Text.
    #[must_use]
    pub fn as_text(self) -> Option<&'a TextNode> {
        self.arena.text(self.id)
    }

    /// The character data when this is Text.
    #[must_use]
    pub fn text(self) -> Option<&'a str> {
        self.as_text().map(TextNode::data)
    }

    /// The parent node, if any.
    #[must_use]
    pub fn parent(self) -> Option<NodeRef<'a, T>> {
        self.node().parent().and_then(|id| self.arena.node_ref(id))
    }

    /// The first child node, if any. Text nodes have no children.
    #[must_use]
    pub fn first_child(self) -> Option<NodeRef<'a, T>> {
        self.node()
            .children()
            .first()
            .and_then(|&id| self.arena.node_ref(id))
    }

    /// The last child node, if any. Text nodes have no children.
    #[must_use]
    pub fn last_child(self) -> Option<NodeRef<'a, T>> {
        self.node()
            .children()
            .last()
            .and_then(|&id| self.arena.node_ref(id))
    }

    /// The next sibling node, if any.
    #[must_use]
    pub fn next_sibling(self) -> Option<NodeRef<'a, T>> {
        let parent = self.node().parent()?;
        let siblings = self.arena.get_node(parent)?.children();
        let pos = siblings.iter().position(|&id| id == self.id)?;
        siblings
            .get(pos + 1)
            .and_then(|&id| self.arena.node_ref(id))
    }

    /// The previous sibling node, if any.
    #[must_use]
    pub fn prev_sibling(self) -> Option<NodeRef<'a, T>> {
        let parent = self.node().parent()?;
        let siblings = self.arena.get_node(parent)?.children();
        let pos = siblings.iter().position(|&id| id == self.id)?;
        siblings
            .get(pos.checked_sub(1)?)
            .and_then(|&id| self.arena.node_ref(id))
    }

    /// Iterate over all child nodes in document order.
    #[must_use]
    pub fn children(self) -> impl DoubleEndedIterator<Item = NodeRef<'a, T>> + 'a {
        let arena = self.arena;
        self.node()
            .children()
            .iter()
            .filter_map(move |&id| arena.node_ref(id))
    }
}

/// A `Copy` read-only handle over an Element node and its arena.
///
/// This is the type Stylo's element and selector traits are implemented on.
/// It cannot be constructed for Text nodes.
pub struct ElementRef<'a, T> {
    pub(crate) arena: &'a Arena<T>,
    pub(crate) id: ElementId,
}

impl<T> Clone for ElementRef<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ElementRef<'_, T> {}

impl<T> std::fmt::Debug for ElementRef<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElementRef")
            .field("id", &self.id)
            .field("tag", &self.element().tag_str())
            .finish()
    }
}

impl<T> PartialEq for ElementRef<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.arena, other.arena) && self.id == other.id
    }
}

impl<T> Eq for ElementRef<'_, T> {}

impl<T> std::hash::Hash for ElementRef<'_, T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (std::ptr::from_ref(self.arena) as usize).hash(state);
        self.id.hash(state);
    }
}

impl<'a, T> ElementRef<'a, T> {
    /// Borrow the underlying element.
    pub(crate) fn element(self) -> &'a Element<T> {
        self.arena
            .get(self.id)
            .expect("ElementRef always references a live element")
    }

    /// Convert to the corresponding generic node handle.
    #[must_use]
    pub fn as_node(self) -> NodeRef<'a, T> {
        NodeRef {
            arena: self.arena,
            id: self.id,
        }
    }

    /// The handle for this element.
    #[must_use]
    pub const fn id(self) -> ElementId {
        self.id
    }

    /// The element's tag name.
    #[must_use]
    pub fn tag(self) -> &'a str {
        self.element().tag_str()
    }

    /// The element's external-state payload.
    #[must_use]
    pub fn ext(self) -> &'a T {
        &self.element().ext
    }

    /// The parent element, if any.
    #[must_use]
    pub fn parent(self) -> Option<ElementRef<'a, T>> {
        self.as_node().parent()?.as_element()
    }

    /// The first Element child, skipping Text nodes.
    #[must_use]
    pub fn first_child(self) -> Option<ElementRef<'a, T>> {
        self.children().next()
    }

    /// The last Element child, skipping Text nodes.
    #[must_use]
    pub fn last_child(self) -> Option<ElementRef<'a, T>> {
        self.children().next_back()
    }

    /// The next Element sibling, skipping Text nodes.
    #[must_use]
    pub fn next_sibling(self) -> Option<ElementRef<'a, T>> {
        let mut sibling = self.as_node().next_sibling();
        while let Some(node) = sibling {
            if let Some(element) = node.as_element() {
                return Some(element);
            }
            sibling = node.next_sibling();
        }
        None
    }

    /// The previous Element sibling, skipping Text nodes.
    #[must_use]
    pub fn prev_sibling(self) -> Option<ElementRef<'a, T>> {
        let mut sibling = self.as_node().prev_sibling();
        while let Some(node) = sibling {
            if let Some(element) = node.as_element() {
                return Some(element);
            }
            sibling = node.prev_sibling();
        }
        None
    }

    /// Iterate over all direct child nodes in document order.
    #[must_use]
    pub fn child_nodes(self) -> impl DoubleEndedIterator<Item = NodeRef<'a, T>> + 'a {
        self.as_node().children()
    }

    /// Iterate over direct Element children in document order, skipping Text.
    pub fn children(self) -> impl DoubleEndedIterator<Item = ElementRef<'a, T>> + 'a {
        self.child_nodes().filter_map(NodeRef::as_element)
    }
}

#[cfg(test)]
mod tests {
    use super::Arena;

    #[test]
    fn recreated_arenas_receive_distinct_layout_identities() {
        let first = Arena::<()>::new();
        let first_identity = first.layout_identity();
        drop(first);

        let second = Arena::<()>::new();
        assert_ne!(first_identity, second.layout_identity());
    }
}
