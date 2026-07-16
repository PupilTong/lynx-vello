//! The [`Document`] — the one tree.
//!
//! # ONE TREE policy
//!
//! A [`Document`] is the **single** owner of every node it contains: node
//! storage (a generational slot arena), the optional document root, the
//! pending pre-mutation snapshot set for stylo's invalidation-set restyle,
//! and the private style context ([`SharedRwLock`] + base URL) guarding every
//! node's inline declarations. There is no separate arena/tree/document split
//! — every DOM operation is a method on `Document`, and nodes cannot be
//! constructed, mutated, or moved outside of one.
//!
//! # Contract: let it crash
//!
//! Queries ([`get`](Document::get), [`child_position`](Document::child_position), …)
//! return `Option` — asking about a stale [`NodeId`] is a legitimate
//! question. **Mutations are different**: passing a stale id, linking a node
//! under its own descendant, or naming an insertion reference that is not a
//! child of the given parent is a caller bug. Those methods `debug_assert!`
//! their preconditions and panic (also in release, via the internal `expect`s)
//! rather than silently ignoring the call. Embedder layers that receive
//! untrusted handles (e.g. from a scripting runtime) validate first and map
//! violations to their own error types.
//!
//! # Backpointers
//!
//! The core is heap-pinned (its address never changes once the document is
//! created, even when the `Document` value moves), and every [`Node`] carries
//! a pointer back to the core that owns it. That backpointer is what lets the
//! stylo handle be a plain `&Node` — see [`crate::node`] and [`crate::traits`].

use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::ptr::NonNull;

use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::UrlExtraData;

use crate::node::Node;

/// The placeholder base URL for parsing a standalone document's inline styles.
///
/// `about:blank` is a constant, valid URL, so this never fails.
pub(crate) fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

/// A stable, generation-checked handle to a node in a [`Document`].
///
/// Cheap to copy and hash. A handle stays valid until its node is removed;
/// afterwards the slot's generation advances and the handle becomes stale
/// (document lookups return `None`), even if the slot is later reused by a
/// different node.
///
/// # Why the generation exists
///
/// The document recycles slots through a free list, so a bare index is
/// ambiguous: after `remove_subtree`, the next `create_node` may place a
/// **new, unrelated node in the same slot** (the ABA problem). Embedders
/// retain ids across those events by design — Lynx's scripting runtime holds
/// element references over frames, and list recycling detaches, re-attaches,
/// and destroys subtrees constantly — so a dangling id *will* eventually
/// point at a reused slot. The generation is what turns that from silent
/// aliasing (reading or mutating a stranger node — the worst kind of logic
/// corruption) into a detectable staleness: `remove` bumps the slot's
/// generation, old handles stop resolving, and each layer reacts per its
/// contract (queries return `None`, PAPI maps to `WidgetError::StaleElement`,
/// DOM-core mutations crash per the let-it-crash contract — which is only
/// *possible* because staleness is detectable at all).
///
/// The generation also anchors stylo integration: the [`OpaqueNode`]
/// identity is derived from **(generation, index)**, so a freed-and-reused
/// slot yields a different `OpaqueNode` and a dead node's pending snapshot
/// or restyle bookkeeping can never be attributed to its slot's successor
/// (an address-derived identity would additionally break whenever slot
/// storage growth moves nodes). A slot whose 32-bit generation space is
/// exhausted is retired rather than reused, preserving uniqueness; and
/// because the generation is `NonZeroU32`, `Option<NodeId>` costs no extra
/// space.
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
    /// Deriving it from the id (rather than the node's address) keeps it
    /// stable across slot-storage growth, which can move every node.
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

/// One storage slot: the current generation plus an optional live [`Node`].
#[derive(Debug)]
struct Slot<T> {
    generation: NonZeroU32,
    node: Option<Node<T>>,
}

/// The owned pointer to a document's heap-pinned [`Core`].
///
/// A dedicated newtype so the `unsafe` `Send`/`Sync` assertions cover exactly
/// one fact: **the pointer itself is plain data with no thread affinity**.
/// Whether a `Document<T>` / `Node<T>` as a whole may cross or be shared
/// across threads keeps being decided by the compiler from their *other*
/// fields (the auto-trait chain through `T`, stylo's `ElementData`, …),
/// exactly as it would be without the backpointer.
pub(crate) struct CorePtr<T>(pub(crate) NonNull<Core<T>>);

impl<T> Clone for CorePtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for CorePtr<T> {}

// SAFETY: `CorePtr` is an address. It is only ever dereferenced through the
// borrow discipline documented on `Document` and `Node::tree` — sending or
// sharing the address itself is unconditionally fine.
#[expect(unsafe_code, reason = "thread-affinity-free pointer newtype")]
unsafe impl<T> Send for CorePtr<T> {}
#[expect(unsafe_code, reason = "thread-affinity-free pointer newtype")]
unsafe impl<T> Sync for CorePtr<T> {}

/// The heap-pinned single tree behind a [`Document`].
///
/// Everything a node's backpointer must reach lives here: the slot storage
/// (id → node resolution for tree navigation) and the shared style lock
/// (stylo's `TDocument::shared_lock`). The snapshot machinery and root ride
/// along so the whole document state is one allocation.
pub(crate) struct Core<T> {
    slots: Vec<Slot<T>>,
    free_list: Vec<u32>,
    /// The document root, if designated (see [`Document::set_root`]).
    root: Option<NodeId>,
    /// The shared lock guarding this document's inline style blocks.
    pub(crate) lock: SharedRwLock,
    /// The base URL data used when parsing this document's inline styles.
    pub(crate) url_data: UrlExtraData,
    /// Pre-mutation node snapshots pending the next flush, keyed by the
    /// node's stable [`OpaqueNode`] (see [`NodeId::opaque`]).
    pub(crate) snapshots: SnapshotMap,
    /// The ids behind [`Core::snapshots`], so `complete_flush` can clear the
    /// per-node snapshot bits without a way to map `OpaqueNode` back.
    pub(crate) snapshotted: Vec<NodeId>,
}

impl<T> Core<T> {
    /// Borrow a node if the handle is live.
    pub(crate) fn node(&self, id: NodeId) -> Option<&Node<T>> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation {
            slot.node.as_ref()
        } else {
            None
        }
    }

    /// Mutably borrow a node if the handle is live.
    pub(crate) fn node_mut(&mut self, id: NodeId) -> Option<&mut Node<T>> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation == id.generation {
            slot.node.as_mut()
        } else {
            None
        }
    }

    /// Resolve an internal tree link (a parent/child id stored inside a
    /// node). Links are maintained by [`Document`]'s mutation methods and
    /// always point at live nodes; a dangling link is a crate bug.
    pub(crate) fn link(&self, id: NodeId) -> &Node<T> {
        self.node(id)
            .expect("internal tree link must resolve to a live node")
    }
}

/// The one tree: a generational arena of [`Node`]s plus the document root,
/// the pending snapshot set, and the private style context.
///
/// See the crate docs for the ONE TREE policy and the mutation
/// contract. Create styled documents through
/// [`StyleEngine::new_document`](crate::StyleEngine::new_document); a
/// standalone [`Document::new`] works for DOM-only use.
pub struct Document<T> {
    core: CorePtr<T>,
    /// `Document` owns the `Core<T>` (and thus values of `T`) behind the raw
    /// pointer; this makes drop-check aware of it.
    _owns: PhantomData<Core<T>>,
}

impl<T> Drop for Document<T> {
    fn drop(&mut self) {
        // SAFETY: `core` came from `Box::into_raw` in `with_style_context`
        // and is dropped exactly once, here. Nodes do not dereference their
        // backpointers on drop.
        #[expect(unsafe_code, reason = "reclaim the heap-pinned core")]
        unsafe {
            drop(Box::from_raw(self.core.0.as_ptr()));
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Document<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let core = self.core();
        // `SnapshotMap` is not `Debug`; report its size instead.
        f.debug_struct("Document")
            .field("root", &core.root)
            .field("slots", &core.slots)
            .field("free_list", &core.free_list)
            .field("pending_snapshots", &core.snapshotted.len())
            .finish_non_exhaustive()
    }
}

impl<T> Default for Document<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Document<T> {
    /// Create an empty standalone document with a freshly minted
    /// [`SharedRwLock`] and a placeholder `about:blank` base URL.
    ///
    /// A standalone document (DOM-only, never styled) can use this. Styled
    /// documents should be created by
    /// [`StyleEngine::new_document`](crate::StyleEngine::new_document).
    #[must_use]
    pub fn new() -> Self {
        Self::with_style_context(SharedRwLock::new(), about_blank_url_data())
    }

    /// Create a document with the style context owned by this crate.
    pub(crate) fn with_style_context(lock: SharedRwLock, url_data: UrlExtraData) -> Self {
        let core = Box::new(Core {
            slots: Vec::new(),
            free_list: Vec::new(),
            root: None,
            lock,
            url_data,
            snapshots: SnapshotMap::new(),
            snapshotted: Vec::new(),
        });
        Document {
            core: CorePtr(NonNull::from(Box::leak(core))),
            _owns: PhantomData,
        }
    }

    /// Borrow the heap-pinned core.
    pub(crate) fn core(&self) -> &Core<T> {
        // SAFETY: the core is owned by `self`, allocated in
        // `with_style_context` and freed only in `Drop`; `&self` guarantees no
        // `&mut Core` coexists (all mutation paths go through `core_mut`).
        #[expect(unsafe_code, reason = "deref the owned, heap-pinned core")]
        unsafe {
            self.core.0.as_ref()
        }
    }

    /// Mutably borrow the heap-pinned core.
    ///
    /// Mutation paths must reach nodes **through the returned core only**
    /// (never through a node backpointer), so this `&mut` is the unique
    /// reference into the allocation for its lifetime.
    pub(crate) fn core_mut(&mut self) -> &mut Core<T> {
        // SAFETY: as `core`, plus `&mut self` excludes every other borrow of
        // the document, and node backpointers are only dereferenced from
        // shared-borrow contexts (`&Node` navigation, stylo traits).
        #[expect(unsafe_code, reason = "deref the owned, heap-pinned core")]
        unsafe {
            self.core.0.as_mut()
        }
    }

    // --- node factory ------------------------------------------------------

    /// Create a detached node and return its handle.
    ///
    /// This is the **only** way nodes come to exist: they are born inside
    /// this document, carrying the backpointer to it, and never move to
    /// another one.
    ///
    /// # Panics
    ///
    /// Panics if the document would need to grow past `u32::MAX` slots.
    pub fn create_node(&mut self, tag: &str, ext: T) -> NodeId {
        let core_ptr = self.core;
        let core = self.core_mut();
        if let Some(index) = core.free_list.pop() {
            let slot = &mut core.slots[index as usize];
            debug_assert!(slot.node.is_none(), "free-list slot must be vacant");
            let id = NodeId {
                index,
                generation: slot.generation,
            };
            slot.node = Some(Node::new(core_ptr, id, tag, ext));
            id
        } else {
            let index =
                u32::try_from(core.slots.len()).expect("document capacity exceeds u32::MAX slots");
            let id = NodeId {
                index,
                generation: NonZeroU32::MIN,
            };
            core.slots.push(Slot {
                generation: NonZeroU32::MIN,
                node: Some(Node::new(core_ptr, id, tag, ext)),
            });
            id
        }
    }

    // --- root --------------------------------------------------------------

    /// Designate `id` as the document root and schedule its initial style
    /// pass.
    ///
    /// The root is where [`StyleEngine::flush_document`](crate::StyleEngine::flush_document)
    /// starts and what [`needs_flush`](Self::needs_flush) inspects.
    ///
    /// Contract: `id` is live and parentless (`debug_assert`ed).
    pub fn set_root(&mut self, id: NodeId) {
        debug_assert!(
            self.get(id).is_some_and(|node| node.parent_id().is_none()),
            "document root must be a live, parentless node"
        );
        self.core_mut().root = Some(id);
        self.mark_subtree_dirty(id);
    }

    /// The document root, if one has been designated.
    #[must_use]
    pub fn root(&self) -> Option<NodeId> {
        self.core().root
    }

    // --- queries -----------------------------------------------------------

    /// Borrow a node if the handle is live.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node<T>> {
        self.core().node(id)
    }

    /// Whether the handle currently resolves to a live node.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.get(id).is_some()
    }

    /// The position of `child` within `parent`'s child list, if it is a child.
    #[must_use]
    pub fn child_position(&self, parent: NodeId, child: NodeId) -> Option<usize> {
        self.get(parent)?
            .child_ids()
            .iter()
            .position(|&c| c == child)
    }

    /// Whether `ancestor` is a strict ancestor of `descendant`.
    #[must_use]
    pub fn is_ancestor(&self, ancestor: NodeId, descendant: NodeId) -> bool {
        let mut next = self.get(descendant).and_then(Node::parent_id);
        while let Some(current) = next {
            if current == ancestor {
                return true;
            }
            next = self.get(current).and_then(Node::parent_id);
        }
        false
    }

    // --- embedder payload ---------------------------------------------------

    /// Mutably borrow a node's external-state payload.
    ///
    /// This is the only mutable access a document hands out into a node, and
    /// it is deliberately payload-only: the DOM fields (links, attributes,
    /// state, …) change exclusively through `Document` methods so their
    /// style invalidation cannot be skipped. When the payload change affects
    /// a synthetic attribute served by the
    /// [`ExternalState`](crate::ExternalState) hooks, call
    /// [`note_external_attribute_change`](Self::note_external_attribute_change)
    /// **before** mutating.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs).
    pub fn ext_mut(&mut self, id: NodeId) -> &mut T {
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::ext_mut")
            .ext_mut()
    }

    // --- structure ---------------------------------------------------------

    /// Insert `child` into `parent` immediately before `before`, or append it
    /// when `before` is `None`. A `child` attached elsewhere (or to `parent`
    /// itself) is detached first, so re-inserting within one parent reorders.
    ///
    /// Applies the selector-flag-scoped child-list invalidation at the old
    /// and new locations, and schedules a subtree restyle on a
    /// previously-styled `child` (its matching context — ancestors, siblings
    /// — changed with the move).
    ///
    /// # Panics
    ///
    /// Panics when `parent`, `child`, or `before` is stale, when `before` is
    /// not a child of `parent`, or — in debug builds — when the link would
    /// create a cycle or `before == child` (the let-it-crash mutation
    /// contract; see the crate docs).
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: Option<NodeId>) {
        debug_assert!(self.contains(parent), "insert_before: stale parent");
        debug_assert!(self.contains(child), "insert_before: stale child");
        debug_assert!(child != parent, "insert_before: child == parent");
        debug_assert!(
            !self.is_ancestor(child, parent),
            "insert_before: linking a node under its own descendant"
        );
        debug_assert!(
            before != Some(child),
            "insert_before: reference must differ from child (a same-node \
             insert is the caller's structural no-op)"
        );

        self.detach(child);

        let index = match before {
            None => self
                .get(parent)
                .expect("stale NodeId passed to Document::insert_before")
                .child_ids()
                .len(),
            Some(reference) => self
                .child_position(parent, reference)
                .expect("insert_before reference must be a child of parent"),
        };

        let core = self.core_mut();
        core.node_mut(parent)
            .expect("stale NodeId passed to Document::insert_before")
            .children
            .insert(index, child);
        core.node_mut(child)
            .expect("stale NodeId passed to Document::insert_before")
            .parent = Some(parent);

        self.note_moved_subtree(child);
        self.note_child_list_change(parent, index);
    }

    /// Append `child` as the last child of `parent`
    /// ([`insert_before`](Self::insert_before) with no reference).
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, child, None);
    }

    /// Detach `child` from its current parent, if any, applying the
    /// selector-flag-scoped child-list invalidation at the old location (a
    /// removal can flip `:empty` / `:nth-*` / edge-child matching).
    ///
    /// A no-op on an already-parentless `child` (that includes the root).
    ///
    /// # Panics
    ///
    /// Panics when `child` is stale (the let-it-crash mutation contract).
    pub fn detach(&mut self, child: NodeId) {
        let old_parent = self
            .get(child)
            .expect("stale NodeId passed to Document::detach")
            .parent_id();
        let Some(parent) = old_parent else {
            return;
        };
        let core = self.core_mut();
        let parent_node = core
            .node_mut(parent)
            .expect("internal tree link must resolve to a live node");
        let removed_index = parent_node
            .children
            .iter()
            .position(|&c| c == child)
            .expect("child must appear in its parent's child list");
        parent_node.children.remove(removed_index);
        core.node_mut(child)
            .expect("stale NodeId passed to Document::detach")
            .parent = None;
        self.note_child_list_change(parent, removed_index);
    }

    /// Remove `id` and its entire subtree from the document, detaching it
    /// from a parent first if attached, and return the external-state payload
    /// of every node freed (in no particular order).
    ///
    /// The caller harvests whatever it indexed from the returned payloads.
    /// All handles into the subtree become stale; if the document root was in
    /// the subtree, the document no longer has a root.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs).
    pub fn remove_subtree(&mut self, id: NodeId) -> Vec<T> {
        self.detach(id);
        if self.core().root.is_some_and(|root| root == id) {
            self.core_mut().root = None;
        }
        let mut removed = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let Some(node) = self.remove_node(current) else {
                unreachable!("subtree links always resolve while removing");
            };
            stack.extend_from_slice(&node.children);
            removed.push(node.into_ext());
        }
        removed
    }

    /// Free one slot, returning its node. The slot's generation is advanced
    /// so every handle to it becomes stale; a slot whose generation space is
    /// exhausted is retired rather than reused, preserving uniqueness.
    fn remove_node(&mut self, id: NodeId) -> Option<Node<T>> {
        let core = self.core_mut();
        let slot = core.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        let node = slot.node.take()?;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            core.free_list.push(id.index);
        }
        // A dead node's pending snapshot must not survive it; the map entry
        // is keyed by the (now stale) opaque id and would be dropped with the
        // map on the next `complete_flush`. Removing it eagerly keeps the map
        // small.
        if node.snapshot_present() {
            core.snapshots.remove(&id.opaque());
        }
        Some(node)
    }

    // --- flush bookkeeping ---------------------------------------------------

    /// Whether the tree has pending style work, judged at the document root.
    ///
    /// `false` when no root has been designated.
    #[must_use]
    pub fn needs_flush(&self) -> bool {
        self.root().is_some_and(|root| {
            self.get(root)
                .is_some_and(|node| node.is_style_dirty() || node.has_dirty_descendants())
        })
    }

    /// Clear every node's dirty bits.
    ///
    /// Establishes a clean baseline (tests, or an embedder resetting a tree).
    /// The flush path uses the cheaper targeted `complete_flush` instead.
    pub fn clear_dirty(&mut self) {
        for slot in &mut self.core_mut().slots {
            if let Some(node) = &mut slot.node {
                node.set_style_dirty(false);
                node.set_dirty_descendants_bit(false);
            }
        }
    }

    /// Clear the flush-scheduling state after a style traversal: drops the
    /// consumed snapshots and walks only the dirty spine under `root`
    /// clearing the dirty bits.
    ///
    /// The spine walk cannot see below a node whose `dirty_descendants`
    /// stylo already cleared (it does so when a subtree computes to
    /// `display: none`), so `style_dirty` breadcrumbs inside such a subtree
    /// may survive — see [`Node::is_style_dirty`].
    pub(crate) fn complete_flush(&mut self, root: NodeId) {
        let core = self.core_mut();
        for id in std::mem::take(&mut core.snapshotted) {
            if let Some(node) = core.node(id) {
                node.clear_snapshot_flags();
            }
        }
        core.snapshots.clear();

        // Walk the dirty spine: clear `style_dirty` on every child of a
        // dirty-descendants node, but only descend where the bit is set.
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let Some(node) = core.node(current) else {
                continue;
            };
            node.set_style_dirty(false);
            if node.has_dirty_descendants() {
                node.set_dirty_descendants_bit(false);
                stack.extend_from_slice(&node.children);
            }
        }
    }
}
