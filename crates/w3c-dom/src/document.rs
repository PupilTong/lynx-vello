//! The [`Document`] — one fixed-address slab containing the whole DOM tree.
//!
//! `Document<T>` owns a boxed [`Slab`] whose address never changes. Every
//! [`Node`] stores a backpointer to that slab, so a plain `&Node` can navigate
//! the tree and recover its owner document without a wrapper handle or a
//! separate tree/core object.
//!
//! Slot zero is always the real DOM document node. Element and text nodes are
//! allocated in the remaining slab slots and use the raw slab index as their
//! [`NodeId`]. IDs are context-local: callers must not route an ID to another
//! `Document`, and a removed ID must not outlive the ownership layer that
//! retained its node.

use std::fmt;
use std::ptr::NonNull;
use std::sync::Arc;

use slab::Slab;
use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::UrlExtraData;

use crate::node::Node;

/// A node's raw index in its owning document's slab.
///
/// IDs carry no document token and no allocation generation. The runtime
/// context/handle layer owns routing and lifetime; `w3c-dom` only resolves the
/// index in the `Document` passed to it.
pub type NodeId = usize;

/// The fixed slab slot occupied by the DOM document node.
pub const DOCUMENT_NODE_ID: NodeId = 0;

/// The placeholder base URL for parsing a standalone document's inline styles.
pub(crate) fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

/// One DOM tree, including its actual document node at slab slot zero.
///
/// The box is load-bearing: moving `Document` never moves the `Slab` value, so
/// every node's slab backpointer remains valid until the document is dropped.
pub struct Document<T> {
    nodes: Box<Slab<Node<T>>>,
}

impl<T: fmt::Debug> fmt::Debug for Document<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("root_element", &self.root_element().map(Node::id))
            .field("nodes", &self.nodes)
            .finish()
    }
}

impl<T> Default for Document<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Document<T> {
    /// Create an empty standalone document with an `about:blank` style context.
    #[must_use]
    pub fn new() -> Self {
        Self::with_style_context(Arc::new(SharedRwLock::new()), about_blank_url_data())
    }

    /// Create a document paired with a [`StyleEngine`](crate::StyleEngine).
    pub(crate) fn with_style_context(lock: Arc<SharedRwLock>, url_data: UrlExtraData) -> Self {
        let mut nodes = Box::new(Slab::new());
        let tree = std::ptr::from_mut::<Slab<Node<T>>>(nodes.as_mut());
        let root = nodes.insert(Node::new_document(tree, lock, url_data));
        assert_eq!(
            root, DOCUMENT_NODE_ID,
            "the DOM document node must occupy slab slot zero"
        );
        Self { nodes }
    }

    /// Borrow the complete node slab.
    pub(crate) fn tree(&self) -> &Slab<Node<T>> {
        &self.nodes
    }

    /// Mutably borrow the complete node slab.
    ///
    /// Mutation is only reachable through `&mut Document`, so no shared node
    /// reference can coexist with this borrow.
    pub(crate) fn tree_mut(&mut self) -> &mut Slab<Node<T>> {
        &mut self.nodes
    }

    /// The actual DOM document node, permanently stored at slot zero.
    ///
    /// # Panics
    ///
    /// Panics only if an internal invariant was violated and slot zero was
    /// removed or replaced; the public mutation API cannot do either.
    #[must_use]
    pub fn root_node(&self) -> &Node<T> {
        self.nodes
            .get(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
    }

    /// The first element child of the document node.
    ///
    /// This is derived from the real child list rather than cached as a
    /// second root pointer.
    ///
    /// # Panics
    ///
    /// Panics if an internal document child link does not resolve. Public
    /// mutation APIs preserve this invariant.
    #[must_use]
    pub fn root_element(&self) -> Option<&Node<T>> {
        self.root_node().children().find(|node| node.is_element())
    }

    /// The document node's shared style lock.
    pub(crate) fn style_lock(&self) -> &Arc<SharedRwLock> {
        self.root_node().document_lock()
    }

    /// Debug-only: mark the document as inside a style traversal until the
    /// returned token is dropped, including while unwinding.
    #[cfg(debug_assertions)]
    pub(crate) fn begin_flush_phase(&self) -> FlushPhaseToken {
        use std::sync::atomic::Ordering;

        let flag = self.root_node().flush_flag();
        let was = flag.swap(true, Ordering::AcqRel);
        assert!(!was, "flush re-entered on a document already being flushed");
        FlushPhaseToken {
            flag: NonNull::from(flag),
        }
    }

    // --- node factories ---------------------------------------------------

    /// Create a detached element and return its raw slab index.
    pub fn create_element(&mut self, tag: &str, ext: T) -> NodeId {
        self.allocate_node(|tree, id| Node::new_element(tree, id, tag, ext))
    }

    /// Backwards-compatible name for [`create_element`](Self::create_element).
    pub fn create_node(&mut self, tag: &str, ext: T) -> NodeId {
        self.create_element(tag, ext)
    }

    /// Create a detached text node and return its raw slab index.
    pub fn create_text_node(&mut self, text: impl Into<String>, ext: T) -> NodeId {
        let text = text.into();
        self.allocate_node(|tree, id| Node::new_text(tree, id, text, ext))
    }

    fn allocate_node(
        &mut self,
        make: impl FnOnce(*mut Slab<Node<T>>, NodeId) -> Node<T>,
    ) -> NodeId {
        let tree = std::ptr::from_mut::<Slab<Node<T>>>(self.nodes.as_mut());
        let entry = self.nodes.vacant_entry();
        let id = entry.key();
        entry.insert(make(tree, id));
        id
    }

    // --- document node ----------------------------------------------------

    /// Attach one element beneath the document node.
    ///
    /// This DOM subset permits one element child. The relationship is stored
    /// in the slot-zero node's ordinary `children` list, and the element's
    /// parent is the document node just like every other DOM link.
    ///
    /// # Panics
    ///
    /// Panics if `child` is not a live element, is the document node, or the
    /// document already has a different element child.
    pub fn append_child(&mut self, child: NodeId) {
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "Document::append_child cannot append the document to itself"
        );
        assert!(
            self.get(child).is_some_and(Node::is_element),
            "Document::append_child requires a live element"
        );
        if self.root_element().map(Node::id) == Some(child) {
            return;
        }
        assert!(
            self.root_element().is_none(),
            "Document::append_child: a document may have only one element child"
        );

        self.detach(child);
        self.nodes
            .get_mut(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
            .children
            .push(child);
        self.nodes
            .get_mut(child)
            .expect("the attached child was validated as live")
            .parent = Some(DOCUMENT_NODE_ID);
        self.mark_subtree_dirty(child);
    }

    // --- queries ----------------------------------------------------------

    /// Borrow a node by its raw slab index.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node<T>> {
        self.nodes.get(id)
    }

    /// Whether the slab index is currently occupied.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains(id)
    }

    /// Whether `id` is connected beneath the slot-zero document node.
    #[must_use]
    pub fn is_connected(&self, id: NodeId) -> bool {
        let mut current = id;
        loop {
            let Some(node) = self.get(current) else {
                return false;
            };
            if current == DOCUMENT_NODE_ID {
                return true;
            }
            let Some(parent) = node.parent_id() else {
                return false;
            };
            current = parent;
        }
    }

    /// The position of `child` in `parent`'s child list.
    #[must_use]
    pub fn child_position(&self, parent: NodeId, child: NodeId) -> Option<usize> {
        self.get(parent)?
            .child_ids()
            .iter()
            .position(|&candidate| candidate == child)
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

    // --- embedder payload -------------------------------------------------

    /// Mutably borrow an element/text node's embedder payload.
    ///
    /// # Panics
    ///
    /// Panics if `id` is vacant/out of range or identifies the document node.
    pub fn ext_mut(&mut self, id: NodeId) -> &mut T {
        self.nodes
            .get_mut(id)
            .expect("stale NodeId passed to Document::ext_mut")
            .ext_mut()
    }

    // --- structure --------------------------------------------------------

    /// Insert `child` into the element `parent` before `before`, or append.
    ///
    /// # Panics
    ///
    /// Panics for vacant IDs, a non-element parent, the document as `child`,
    /// an invalid insertion reference, or a link that would violate the tree
    /// invariants.
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: Option<NodeId>) {
        debug_assert!(self.contains(parent), "insert_before: stale parent");
        debug_assert!(self.contains(child), "insert_before: stale child");
        assert!(
            self.get(parent).is_some_and(Node::is_element),
            "insert_before: parent must be a live element"
        );
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "insert_before: the document node cannot be reparented"
        );
        debug_assert!(child != parent, "insert_before: child == parent");
        debug_assert!(
            !self.is_ancestor(child, parent),
            "insert_before: linking a node under its own descendant"
        );
        debug_assert!(
            before != Some(child),
            "insert_before: reference must differ from child"
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

        self.nodes
            .get_mut(parent)
            .expect("stale NodeId passed to Document::insert_before")
            .children
            .insert(index, child);
        self.nodes
            .get_mut(child)
            .expect("stale NodeId passed to Document::insert_before")
            .parent = Some(parent);

        self.note_moved_subtree(child);
        self.note_child_list_change(parent, index);
    }

    /// Append `child` as the final child of the element `parent`.
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, child, None);
    }

    /// Detach a non-document node from its current parent.
    ///
    /// # Panics
    ///
    /// Panics if `child` is vacant/out of range or is the document node.
    pub fn detach(&mut self, child: NodeId) {
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "Document::detach cannot detach the document node"
        );
        let old_parent = self
            .get(child)
            .expect("stale NodeId passed to Document::detach")
            .parent_id();
        let Some(parent) = old_parent else {
            return;
        };

        let removed_index = {
            let parent_node = self
                .nodes
                .get_mut(parent)
                .expect("internal tree link must resolve to a live node");
            let index = parent_node
                .children
                .iter()
                .position(|&candidate| candidate == child)
                .expect("child must appear in its parent's child list");
            parent_node.children.remove(index);
            index
        };
        self.nodes
            .get_mut(child)
            .expect("stale NodeId passed to Document::detach")
            .parent = None;

        if parent != DOCUMENT_NODE_ID {
            self.note_child_list_change(parent, removed_index);
        }
    }

    /// Remove `id` and every descendant, returning their embedder payloads.
    ///
    /// # Panics
    ///
    /// Panics if `id` is vacant/out of range or is the document node.
    pub fn remove_subtree(&mut self, id: NodeId) -> Vec<T> {
        assert_ne!(
            id, DOCUMENT_NODE_ID,
            "Document::remove_subtree cannot remove the document node"
        );
        self.detach(id);
        let mut removed = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let node = self
                .nodes
                .try_remove(current)
                .expect("subtree links always resolve while removing");
            stack.extend_from_slice(&node.children);
            removed.push(node.into_ext());
        }
        removed
    }

    // --- flush bookkeeping ------------------------------------------------

    /// Move reachable pending snapshots into Stylo's temporary map.
    pub(crate) fn take_snapshot_map(&mut self, root: NodeId) -> SnapshotMap {
        let mut snapshots = SnapshotMap::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(node) = self.nodes.get_mut(id) else {
                continue;
            };
            debug_assert_eq!(
                node.snapshot.is_some(),
                node.snapshot_present(),
                "snapshot slot and lifecycle flag diverged before flush"
            );
            if let Some(snapshot) = node.snapshot.take() {
                snapshots.insert(OpaqueNode(node.id()), *snapshot);
            }
            if node.has_dirty_descendants() {
                stack.extend_from_slice(&node.children);
            }
        }
        snapshots
    }

    /// Whether the connected root element has pending style work.
    #[must_use]
    pub fn needs_flush(&self) -> bool {
        self.root_element()
            .is_some_and(|node| node.is_style_dirty() || node.has_dirty_descendants())
    }

    /// Clear every node's dirty and snapshot state.
    pub fn clear_dirty(&mut self) {
        for (_, node) in &mut *self.nodes {
            node.set_style_dirty(false);
            node.set_dirty_descendants_bit(false);
            node.snapshot = None;
            node.clear_snapshot_flags();
        }
    }

    /// Clear state consumed by one completed style traversal.
    pub(crate) fn complete_flush(&mut self, root: NodeId, snapshots: &SnapshotMap) {
        for opaque in snapshots.keys() {
            if let Some(node) = self.nodes.get(opaque.0) {
                node.clear_snapshot_flags();
            }
        }

        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let Some(node) = self.nodes.get(current) else {
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

/// Debug-only RAII marker for the style-flush phase.
#[cfg(debug_assertions)]
pub(crate) struct FlushPhaseToken {
    flag: NonNull<std::sync::atomic::AtomicBool>,
}

#[cfg(debug_assertions)]
impl Drop for FlushPhaseToken {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;

        // SAFETY: the flag is stored in the slot-zero node, which cannot be
        // removed and outlives this token. It is atomic specifically so the
        // token need not retain a Rust borrow across the mutable flush.
        #[expect(unsafe_code, reason = "clear the document-node traversal flag")]
        unsafe {
            self.flag.as_ref().store(false, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "internal tree links always resolve")]
    fn root_element_panics_on_a_dangling_document_child() {
        let mut document: Document<()> = Document::new();
        document
            .nodes
            .get_mut(DOCUMENT_NODE_ID)
            .expect("the document node is present")
            .children
            .push(usize::MAX);

        let _ = document.root_element();
    }
}
