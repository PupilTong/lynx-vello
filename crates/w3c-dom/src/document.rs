//! The [`Document`] — the one tree.
//!
//! # ONE TREE policy
//!
//! A [`Document`] is the **single** owner of every node it contains: node
//! storage (a slab with versioned handles), the optional document element, the
//! per-node pre-mutation snapshots for stylo's invalidation-set restyle,
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
//!
//! # Storage
//!
//! Nodes live in a [`slab::Slab`], which owns vacant-slot tracking and index
//! reuse. [`NodeId`] adds a document-local allocation generation so a stale
//! id cannot resolve to a later node that reuses the same slab index.

use std::fmt;
use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use slab::Slab;
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

/// Mint a process-unique document identity token. `0` is never minted, so
/// tokens fit `NonZeroU64`.
pub(crate) fn mint_identity() -> NonZeroU64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NonZeroU64::new(NEXT.fetch_add(1, Ordering::Relaxed)).expect("identity counter starts at 1")
}

/// A stable, generation-checked handle to a node in a [`Document`].
///
/// Cheap to copy and hash. A handle stays valid until its node is removed;
/// afterwards it becomes stale (document lookups return `None`), even if the
/// slab index is later reused by a different node.
///
/// # Why the generation exists
///
/// The slab recycles indices, so a bare index is ambiguous: after
/// `remove_subtree`, the next node factory call may place a new node at the same
/// index. A document-local allocation generation turns that ABA-shaped reuse
/// into detectable staleness: queries return `None`, while mutation methods
/// panic according to the crate's let-it-crash contract.
///
/// The generation also anchors stylo integration: the [`OpaqueNode`]
/// identity is derived from **(generation, index)**, so a freed-and-reused
/// slot yields a different `OpaqueNode` and a dead node's pending snapshot
/// or restyle bookkeeping can never be attributed to its slot's successor
/// (an address-derived identity would additionally break whenever slab
/// storage growth moves nodes). Because the generation is `NonZeroU32`,
/// `Option<NodeId>` costs no extra space.
///
/// # Document identity
///
/// An id also carries the **token of the document that minted it**
/// ([`document_token`](Self::document_token)): two documents mint identical
/// `(index, generation)` sequences, so without the token an id from tree A
/// would pass tree B's liveness check and silently alias whatever occupies
/// the same slot there — cross-tree data corruption, not a detectable error.
/// With the token in the id (and in every node's own id), a foreign id
/// simply never compares equal: queries return `None`, mutations crash per
/// the let-it-crash contract, and embedder layers can reject it with a
/// typed error before that.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId {
    doc: NonZeroU64,
    index: u32,
    generation: NonZeroU32,
}

impl NodeId {
    /// The identity token of the document this id was minted by (see
    /// [`Document::token`]).
    #[must_use]
    pub const fn document_token(self) -> NonZeroU64 {
        self.doc
    }

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
    /// stable across slab-storage growth, which can move every node.
    #[must_use]
    pub(crate) const fn opaque(self) -> OpaqueNode {
        // Packs (generation, index) into the usize. 64-bit targets only —
        // on 32-bit this would truncate the generation. The document token
        // is deliberately excluded: stylo's snapshot map and traversal roots
        // are per-document, so (generation, index) is unique within the one
        // map an OpaqueNode is ever used in.
        const {
            assert!(
                size_of::<usize>() >= 8,
                "NodeId::opaque requires a 64-bit target"
            );
        }
        OpaqueNode(((self.generation.get() as usize) << 32) | self.index as usize)
    }
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
/// Everything a node's backpointer must reach lives here: the slab storage
/// (id → node resolution for tree navigation) and the shared style lock
/// (stylo's `TDocument::shared_lock`). The document-element link rides along
/// so the whole document state is one allocation; each pending snapshot lives
/// on its node.
pub(crate) struct Core<T> {
    /// This document's process-unique identity, stamped into every [`NodeId`]
    /// it mints.
    token: NonZeroU64,
    /// Node storage. `nodes[id.index]` holds the live node for that slab key;
    /// `Slab` owns vacant-slot tracking and reuse.
    nodes: Slab<Node<T>>,
    /// Generation assigned to the next allocation. This is independent of
    /// slab vacancy management and prevents a reused key from reviving an old
    /// [`NodeId`]. The wider counter lets creation fail instead of wrapping
    /// the public 32-bit generation.
    next_generation: u64,
    /// The document's element child, if one has been appended. `Document` is
    /// the actual DOM root; this link is its at-most-one element child, not a
    /// separately designated tree root. Text nodes live below elements.
    document_element: Option<NodeId>,
    /// The shared lock guarding this document's inline style blocks.
    pub(crate) lock: Arc<SharedRwLock>,
    /// The base URL data used when parsing this document's inline styles.
    pub(crate) url_data: UrlExtraData,
    /// Debug-only: set for the duration of a style flush, so per-node style
    /// readers can assert traversal-phase discipline (see
    /// [`crate::node::slot_guard`]).
    #[cfg(debug_assertions)]
    pub(crate) in_flush: std::sync::atomic::AtomicBool,
}

impl<T> Core<T> {
    /// Borrow a node if the handle is live (the occupant's own id carries
    /// the generation to check against).
    pub(crate) fn node(&self, id: NodeId) -> Option<&Node<T>> {
        let node = self.nodes.get(id.index as usize)?;
        (node.id() == id).then_some(node)
    }

    /// Mutably borrow a node if the handle is live.
    pub(crate) fn node_mut(&mut self, id: NodeId) -> Option<&mut Node<T>> {
        let node = self.nodes.get_mut(id.index as usize)?;
        (node.id() == id).then_some(node)
    }

    /// The element child of the document node, if present.
    pub(crate) fn document_element(&self) -> Option<NodeId> {
        self.document_element
    }

    /// Whether `id` is connected beneath the document node.
    pub(crate) fn is_connected(&self, id: NodeId) -> bool {
        let mut current = id;
        loop {
            let Some(node) = self.node(current) else {
                return false;
            };
            let Some(parent) = node.parent_id() else {
                return self.document_element == Some(current);
            };
            current = parent;
        }
    }

    /// Debug-only: whether a style flush is currently traversing this tree.
    #[cfg(debug_assertions)]
    pub(crate) fn in_flush(&self) -> bool {
        self.in_flush.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Resolve an internal tree link (a parent/child id stored inside a
    /// node). Links are maintained by [`Document`]'s mutation methods and
    /// always point at live nodes; a dangling link is a crate bug.
    pub(crate) fn link(&self, id: NodeId) -> &Node<T> {
        self.node(id)
            .expect("internal tree link must resolve to a live node")
    }
}

/// The one tree: a document node, a slab of element/text [`Node`]s (including
/// their pending snapshots), and the private style context.
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
        f.debug_struct("Document")
            .field("document_element", &core.document_element)
            .field("nodes", &core.nodes)
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
        Self::with_style_context(Arc::new(SharedRwLock::new()), about_blank_url_data())
    }

    /// Create a document with the style context owned by this crate.
    pub(crate) fn with_style_context(lock: Arc<SharedRwLock>, url_data: UrlExtraData) -> Self {
        let core = Box::new(Core {
            token: mint_identity(),
            nodes: Slab::new(),
            next_generation: 1,
            document_element: None,
            lock,
            url_data,
            #[cfg(debug_assertions)]
            in_flush: std::sync::atomic::AtomicBool::new(false),
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

    /// Debug-only: mark the document as inside a style flush for the
    /// returned token's lifetime (RAII, cleared on unwind). The token holds
    /// the heap-pinned core's address rather than a borrow, so the flush can
    /// still take `&mut Document` for `complete_flush` while the phase is
    /// marked.
    #[cfg(debug_assertions)]
    pub(crate) fn begin_flush_phase(&self) -> FlushPhaseToken<T> {
        use std::sync::atomic::Ordering;
        let was = self.core().in_flush.swap(true, Ordering::AcqRel);
        assert!(!was, "flush re-entered on a document already being flushed");
        FlushPhaseToken { core: self.core }
    }

    // --- node factory ------------------------------------------------------

    /// Create a detached element and return its handle.
    ///
    /// Elements are born inside this document, carrying the backpointer to
    /// it, and never move to another one.
    ///
    /// # Panics
    ///
    /// Panics if the document would need to grow past `u32::MAX` slots.
    pub fn create_element(&mut self, tag: &str, ext: T) -> NodeId {
        self.allocate_node(|core, id| Node::new_element(core, id, tag, ext))
    }

    /// Backwards-compatible name for [`create_element`](Self::create_element).
    ///
    /// New code should prefer the kind-specific factory so element creation is
    /// visibly distinct from [`create_text_node`](Self::create_text_node).
    ///
    /// # Panics
    ///
    /// Panics if the slab key or allocation generation no longer fits in the
    /// corresponding 32-bit [`NodeId`] field.
    pub fn create_node(&mut self, tag: &str, ext: T) -> NodeId {
        self.create_element(tag, ext)
    }

    /// Create a detached text node containing `text` and return its handle.
    ///
    /// Text nodes share the document's storage, tree links, identity, and
    /// embedder payload type, but they have no tag or CSS element state and
    /// are not selector subjects.
    ///
    /// # Panics
    ///
    /// Panics if the document would need to grow past `u32::MAX` slots.
    pub fn create_text_node(&mut self, text: impl Into<String>, ext: T) -> NodeId {
        let text = text.into();
        self.allocate_node(|core, id| Node::new_text(core, id, text, ext))
    }

    /// Allocate one generational slot and construct its occupant.
    fn allocate_node(&mut self, make: impl FnOnce(CorePtr<T>, NodeId) -> Node<T>) -> NodeId {
        let core_ptr = self.core;
        let core = self.core_mut();
        let index =
            u32::try_from(core.nodes.vacant_key()).expect("document slab key exceeds u32::MAX");
        let generation = NonZeroU32::new(
            u32::try_from(core.next_generation)
                .expect("document exhausted its 32-bit node generations"),
        )
        .expect("node generations start at one");
        core.next_generation += 1;

        let id = NodeId {
            doc: core.token,
            index,
            generation,
        };
        let inserted = core.nodes.insert(make(core_ptr, id));
        debug_assert_eq!(inserted, index as usize, "vacant slab key changed");
        id
    }

    // --- document node -----------------------------------------------------

    /// Append `child` to the document node.
    ///
    /// This DOM subset permits one child on the document node: its element
    /// `documentElement`. Text nodes live below elements. The child may be
    /// detached or linked under another element; DOM pre-insertion detaches
    /// it first. Attaching it schedules an initial style pass for the whole
    /// subtree.
    ///
    /// # Panics
    ///
    /// Panics when `child` is stale, is a text node, or the document already
    /// has a different element child.
    pub fn append_child(&mut self, child: NodeId) {
        assert!(
            self.get(child).is_some_and(Node::is_element),
            "Document::append_child requires a live element"
        );
        if self.core().document_element == Some(child) {
            return;
        }
        assert!(
            self.core().document_element.is_none(),
            "Document::append_child: a document may have only one element child"
        );
        self.detach(child);
        self.core_mut().document_element = Some(child);
        self.mark_subtree_dirty(child);
    }

    /// The document's element child, if one has been appended.
    #[must_use]
    pub fn document_element(&self) -> Option<NodeId> {
        self.core().document_element
    }

    /// This document's process-unique identity token.
    ///
    /// Every [`NodeId`] this document mints carries it
    /// ([`NodeId::document_token`]); embedder layers use it to give their own
    /// handles an unforgeable tree identity.
    #[must_use]
    pub fn token(&self) -> NonZeroU64 {
        self.core().token
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

    /// Whether `id` is connected beneath this document node.
    #[must_use]
    pub fn is_connected(&self, id: NodeId) -> bool {
        self.core().is_connected(id)
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
            .expect("stale or foreign NodeId passed to Document::ext_mut")
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
    /// Panics when `parent`, `child`, or `before` is stale, when `parent` is a
    /// text node, when `before` is not a child of `parent`, or — in debug
    /// builds — when the link would create a cycle or `before == child` (the
    /// let-it-crash mutation contract; see the crate docs).
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: Option<NodeId>) {
        debug_assert!(self.contains(parent), "insert_before: stale parent");
        debug_assert!(self.contains(child), "insert_before: stale child");
        assert!(
            self.get(parent).is_some_and(Node::is_element),
            "insert_before: parent must be a live element"
        );
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
                .expect("stale or foreign NodeId passed to Document::insert_before")
                .child_ids()
                .len(),
            Some(reference) => self
                .child_position(parent, reference)
                .expect("insert_before reference must be a child of parent"),
        };

        let core = self.core_mut();
        core.node_mut(parent)
            .expect("stale or foreign NodeId passed to Document::insert_before")
            .children
            .insert(index, child);
        core.node_mut(child)
            .expect("stale or foreign NodeId passed to Document::insert_before")
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
    /// A no-op on an already-detached `child`. If `child` is the document
    /// element, this removes it from the document node.
    ///
    /// # Panics
    ///
    /// Panics when `child` is stale (the let-it-crash mutation contract).
    pub fn detach(&mut self, child: NodeId) {
        let old_parent = self
            .get(child)
            .expect("stale or foreign NodeId passed to Document::detach")
            .parent_id();
        let Some(parent) = old_parent else {
            if self.core().document_element == Some(child) {
                self.core_mut().document_element = None;
            }
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
            .expect("stale or foreign NodeId passed to Document::detach")
            .parent = None;
        self.note_child_list_change(parent, removed_index);
    }

    /// Remove `id` and its entire subtree from the document, detaching it
    /// from a parent first if attached, and return the external-state payload
    /// of every node freed (in no particular order).
    ///
    /// The caller harvests whatever it indexed from the returned payloads.
    /// All handles into the subtree become stale. Removing the document
    /// element leaves the document node without an element child.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs).
    pub fn remove_subtree(&mut self, id: NodeId) -> Vec<T> {
        self.detach(id);
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

    /// Remove one node from the slab and return it. A future occupant of the
    /// same slab key receives a fresh allocation generation, so handles to
    /// this node remain stale.
    fn remove_node(&mut self, id: NodeId) -> Option<Node<T>> {
        let core = self.core_mut();
        let index = id.index as usize;
        if core.nodes.get(index)?.id() != id {
            return None;
        }
        let node = core
            .nodes
            .try_remove(index)
            .expect("validated slab entry must still be occupied");
        Some(node)
    }

    // --- flush bookkeeping ---------------------------------------------------

    /// Move the pending snapshots reachable along `root`'s dirty spine into
    /// the map expected by stylo's traversal API. Snapshots on detached trees
    /// stay on their nodes until those trees are attached and flushed (or
    /// dropped). Presence/handled flags remain set until
    /// [`complete_flush`](Self::complete_flush).
    pub(crate) fn take_snapshot_map(&mut self, root: NodeId) -> SnapshotMap {
        let mut snapshots = SnapshotMap::new();
        let core = self.core_mut();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(node) = core.node_mut(id) else {
                continue;
            };
            debug_assert_eq!(
                node.snapshot.is_some(),
                node.snapshot_present(),
                "snapshot slot and lifecycle flag diverged before flush"
            );
            if let Some(snapshot) = node.snapshot.take() {
                snapshots.insert(node.id().opaque(), snapshot);
            }
            if node.has_dirty_descendants() {
                stack.extend_from_slice(&node.children);
            }
        }
        snapshots
    }

    /// Whether the connected document element has pending style work.
    ///
    /// `false` when the document has no element child.
    #[must_use]
    pub fn needs_flush(&self) -> bool {
        self.document_element().is_some_and(|root| {
            self.get(root)
                .is_some_and(|node| node.is_style_dirty() || node.has_dirty_descendants())
        })
    }

    /// Clear every node's dirty bits and pending snapshot.
    ///
    /// Establishes a clean baseline (tests, or an embedder resetting a tree).
    /// The flush path uses the cheaper targeted `complete_flush` instead.
    pub fn clear_dirty(&mut self) {
        for (_, node) in &mut self.core_mut().nodes {
            node.set_style_dirty(false);
            node.set_dirty_descendants_bit(false);
            node.snapshot = None;
            node.clear_snapshot_flags();
        }
    }

    /// Clear the flush-scheduling state after a style traversal: clears the
    /// consumed snapshot flags and walks only the dirty spine under `root`
    /// clearing the dirty bits. `snapshots` itself is local to the flush and
    /// is dropped by its caller.
    ///
    /// The spine walk cannot see below a node whose `dirty_descendants`
    /// stylo already cleared (it does so when a subtree computes to
    /// `display: none`), so `style_dirty` breadcrumbs inside such a subtree
    /// may survive — see [`Node::is_style_dirty`].
    pub(crate) fn complete_flush(&mut self, root: NodeId, snapshots: &SnapshotMap) {
        let core = self.core_mut();
        for opaque in snapshots.keys() {
            // `NodeId::opaque` packs the slab key into the low 32 bits. Check
            // the full opaque identity as well in case the key was reused.
            let index = opaque.0 & 0xffff_ffff;
            if let Some(node) = core.nodes.get(index)
                && node.id().opaque() == *opaque
            {
                node.clear_snapshot_flags();
            }
        }

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

/// Debug-only RAII marker for the style-flush phase (see
/// [`Document::begin_flush_phase`]).
#[cfg(debug_assertions)]
pub(crate) struct FlushPhaseToken<T> {
    core: CorePtr<T>,
}

#[cfg(debug_assertions)]
impl<T> Drop for FlushPhaseToken<T> {
    fn drop(&mut self) {
        // SAFETY: the token is created from a live document and dropped
        // inside the flush call that created it; the heap-pinned core
        // outlives both. Storing the address (not a borrow) is what lets the
        // flush take `&mut Document` while the phase is marked — the flag is
        // atomic, so this store cannot race with anything the `&mut` guards.
        #[expect(unsafe_code, reason = "atomic store through the heap-pinned core")]
        let core = unsafe { self.core.0.as_ref() };
        core.in_flush
            .store(false, std::sync::atomic::Ordering::Release);
    }
}
