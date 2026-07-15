//! The slot storage backing a [`Document`].
//!
//! Elements live in a [`Slab`] and are addressed by an [`ElementId`] carrying
//! a private slot index. Production correctness relies on the embedder not
//! reclaiming a slot while a strong external handle or a delayed raw id
//! exists. Debug/test builds additionally carry a document-wide allocation
//! epoch so violations fail immediately instead of silently becoming ABA.
//!
//! Every live node carries an address-stable back-pointer to its document;
//! stylo's DOM traits are therefore implemented directly on `&Node<T>`.
//!
//! The document also owns the **pending snapshot set** for stylo's
//! invalidation-set restyle: before a matching-relevant mutation, the embedder
//! layer records the element's old state/attributes here (see
//! [`crate::dirty`]), and the next
//! [`Document::flush`](crate::Document::flush) consumes them.

use std::cell::Cell;
use std::fmt;
#[cfg(debug_assertions)]
use std::num::NonZeroU32;
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicU8, Ordering};

use dom::ElementState;
use rustc_hash::FxHashMap;
use slab::Slab;
use smallvec::SmallVec;
use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::UrlExtraData;
use stylo::stylist::Stylist;
#[cfg(debug_assertions)]
use stylo::thread_state;
use stylo_atoms::Atom;

use crate::node::Node;

/// A private-to-the-embedder slot identity for an element in a [`Document`].
///
/// VM/application layers must wrap this in their own strong handle rather than
/// exposing or storing it directly. Debug/test builds attach an allocation
/// epoch that detects stale internal ids after slot reuse.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ElementId {
    index: u32,
    #[cfg(debug_assertions)]
    allocation_epoch: NonZeroU32,
}

impl ElementId {
    /// The slot index this handle refers to.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// The stable stylo [`OpaqueNode`] identity for this handle.
    ///
    /// stylo keys its [`SnapshotMap`] and traversal roots by `OpaqueNode`.
    /// Deriving it from the id (rather than the element's address) keeps it
    /// stable across arena growth, which can reallocate and move every
    /// element.
    #[must_use]
    pub(crate) const fn opaque(self) -> OpaqueNode {
        #[cfg(debug_assertions)]
        {
            // Debug identities include the allocation epoch so a stale
            // snapshot cannot collide with a replacement node.
            const {
                assert!(
                    size_of::<usize>() >= 8,
                    "ElementId::opaque requires a 64-bit target in debug builds"
                );
            }
            OpaqueNode(((self.allocation_epoch.get() as usize) << 32) | self.index as usize)
        }
        #[cfg(not(debug_assertions))]
        {
            OpaqueNode(self.index as usize)
        }
    }
}

/// The address-stable document allocation that live nodes point back to.
pub(crate) struct DocumentInner<T> {
    pub(crate) stylist: Stylist,
    nodes: Slab<Node<T>>,
    #[cfg(debug_assertions)]
    next_allocation_epoch: NonZeroU32,
    pub(crate) lock: SharedRwLock,
    pub(crate) url_data: UrlExtraData,
    snapshots: SnapshotMap,
    snapshotted: Vec<ElementId>,
    phase: Cell<DocumentPhase>,
    /// Cross-worker mirror of `phase`, used only to diagnose violations of
    /// stylo's traversal access contract. Release builds retain only the
    /// owner-thread `Cell` state above.
    #[cfg(debug_assertions)]
    debug_traversal_phase: AtomicU8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocumentPhase {
    Idle,
    Traversing,
    Poisoned,
}

#[cfg(debug_assertions)]
const DEBUG_PHASE_IDLE: u8 = 0;
#[cfg(debug_assertions)]
const DEBUG_PHASE_TRAVERSING: u8 = 1;
#[cfg(debug_assertions)]
const DEBUG_PHASE_POISONED: u8 = 2;

/// RAII exclusion guard for stylo's possibly parallel traversal.
pub(crate) struct TraversalGuard<'a> {
    phase: &'a Cell<DocumentPhase>,
    #[cfg(debug_assertions)]
    debug_phase: &'a AtomicU8,
}

impl Drop for TraversalGuard<'_> {
    fn drop(&mut self) {
        let panicking = std::thread::panicking();
        let next_phase = if panicking {
            DocumentPhase::Poisoned
        } else {
            DocumentPhase::Idle
        };
        #[cfg(debug_assertions)]
        self.debug_phase.store(
            if panicking {
                DEBUG_PHASE_POISONED
            } else {
                DEBUG_PHASE_IDLE
            },
            Ordering::Release,
        );
        self.phase.set(next_phase);
    }
}

impl<T> DocumentInner<T> {
    pub(crate) fn node(&self, id: ElementId) -> Option<&Node<T>> {
        let node = self.nodes.get(id.index as usize)?;
        #[cfg(debug_assertions)]
        if node.node_id() == id {
            Some(node)
        } else {
            None
        }
        #[cfg(not(debug_assertions))]
        {
            Some(node)
        }
    }

    pub(crate) fn node_mut(&mut self, id: ElementId) -> Option<&mut Node<T>> {
        let node = self.nodes.get_mut(id.index as usize)?;
        #[cfg(debug_assertions)]
        if node.node_id() == id {
            Some(node)
        } else {
            None
        }
        #[cfg(not(debug_assertions))]
        {
            Some(node)
        }
    }

    pub(crate) fn shared_lock(&self) -> &SharedRwLock {
        &self.lock
    }

    fn assert_idle(&self) {
        assert_idle(&self.phase);
    }

    fn note_mutation(&self) {
        self.assert_idle();
    }

    fn begin_traversal(&self) -> TraversalGuard<'_> {
        begin_traversal(
            &self.phase,
            #[cfg(debug_assertions)]
            &self.debug_traversal_phase,
        )
    }

    /// Verify an `ElementData` access is coming from a traversal participant,
    /// or from the single owner thread while the document is idle.
    #[cfg(debug_assertions)]
    pub(crate) fn debug_assert_style_data_access(&self) {
        match self.debug_traversal_phase.load(Ordering::Acquire) {
            DEBUG_PHASE_IDLE => {}
            DEBUG_PHASE_TRAVERSING => assert!(
                thread_state::get().is_layout(),
                "ElementData accessed by a non-traversal thread during style traversal"
            ),
            DEBUG_PHASE_POISONED => {
                panic!("ElementData accessed after a panicking style traversal")
            }
            phase => panic!("invalid debug traversal phase {phase}"),
        }
    }

    /// Verify entry into the one-worker-per-element portion of traversal.
    #[cfg(debug_assertions)]
    pub(crate) fn debug_assert_traversing(&self) {
        assert_eq!(
            self.debug_traversal_phase.load(Ordering::Acquire),
            DEBUG_PHASE_TRAVERSING,
            "element traversal ownership claimed outside style traversal"
        );
        assert!(
            thread_state::get().is_layout(),
            "element traversal ownership claimed by a non-traversal thread"
        );
    }
}

fn assert_idle(phase: &Cell<DocumentPhase>) {
    match phase.get() {
        DocumentPhase::Idle => {}
        DocumentPhase::Traversing => {
            panic!("document mutation attempted during style traversal")
        }
        DocumentPhase::Poisoned => {
            panic!("document was poisoned by a panicking style traversal")
        }
    }
}

fn begin_traversal<'a>(
    phase: &'a Cell<DocumentPhase>,
    #[cfg(debug_assertions)] debug_phase: &'a AtomicU8,
) -> TraversalGuard<'a> {
    match phase.get() {
        DocumentPhase::Idle => {}
        DocumentPhase::Traversing => panic!("nested style traversal on one document"),
        DocumentPhase::Poisoned => {
            panic!("document was poisoned by a panicking style traversal")
        }
    }
    #[cfg(debug_assertions)]
    debug_phase
        .compare_exchange(
            DEBUG_PHASE_IDLE,
            DEBUG_PHASE_TRAVERSING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .expect("debug traversal phase disagrees with document phase");
    phase.set(DocumentPhase::Traversing);
    TraversalGuard {
        phase,
        #[cfg(debug_assertions)]
        debug_phase,
    }
}

/// One independent DOM tree and its complete stylo style context.
///
/// Every document owns its own [`Stylist`], [`SharedRwLock`], device, node
/// storage, invalidation snapshots, and traversal phase. The private boxed
/// allocation keeps the address followed by every [`Node`] back-pointer stable
/// even when this public owner is moved.
pub struct Document<T> {
    pub(crate) inner: Box<DocumentInner<T>>,
}

impl<T: fmt::Debug> fmt::Debug for Document<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `SnapshotMap` is not `Debug`; report its size instead.
        f.debug_struct("Document")
            .field("viewport", &self.inner.stylist.device().viewport_size())
            .field("nodes", &self.inner.nodes)
            .field("pending_snapshots", &self.inner.snapshotted.len())
            .finish_non_exhaustive()
    }
}

impl<T> Document<T> {
    pub(crate) fn from_parts(stylist: Stylist, lock: SharedRwLock, url_data: UrlExtraData) -> Self {
        Self {
            inner: Box::new(DocumentInner {
                stylist,
                nodes: Slab::new(),
                #[cfg(debug_assertions)]
                next_allocation_epoch: NonZeroU32::MIN,
                lock,
                url_data,
                snapshots: SnapshotMap::new(),
                snapshotted: Vec::new(),
                phase: Cell::new(DocumentPhase::Idle),
                #[cfg(debug_assertions)]
                debug_traversal_phase: AtomicU8::new(DEBUG_PHASE_IDLE),
            }),
        }
    }

    /// The shared lock guarding this arena's inline style blocks.
    #[must_use]
    pub(crate) fn shared_lock(&self) -> &SharedRwLock {
        &self.inner.lock
    }

    /// The base URL data used when parsing this arena's inline styles.
    #[must_use]
    pub(crate) fn url_data(&self) -> &UrlExtraData {
        &self.inner.url_data
    }

    /// The pending pre-mutation snapshots, consumed by the flush traversal.
    #[must_use]
    pub(crate) fn snapshot_map(&self) -> &SnapshotMap {
        &self.inner.snapshots
    }

    pub(crate) fn snapshot_map_mut(&mut self) -> (&mut SnapshotMap, &mut Vec<ElementId>) {
        self.inner.note_mutation();
        (&mut self.inner.snapshots, &mut self.inner.snapshotted)
    }

    pub(crate) fn note_mutation(&self) {
        self.inner.note_mutation();
    }

    /// Create an element in this document and return its handle.
    ///
    /// # Panics
    ///
    /// Panics if the slab would need an index beyond `u32::MAX`. Debug/test
    /// builds also panic after exhausting the diagnostic allocation epoch.
    pub fn create_element(&mut self, tag: &str, ext: T) -> ElementId {
        self.inner.note_mutation();
        let document = NonNull::from(self.inner.as_mut());
        let index = u32::try_from(self.inner.nodes.vacant_key())
            .expect("arena capacity exceeds u32::MAX slots");
        #[cfg(debug_assertions)]
        let allocation_epoch = {
            let current = self.inner.next_allocation_epoch;
            self.inner.next_allocation_epoch = current
                .checked_add(1)
                .expect("document allocation epoch exhausted");
            current
        };
        let id = ElementId {
            index,
            #[cfg(debug_assertions)]
            allocation_epoch,
        };
        let entry = self.inner.nodes.vacant_entry();
        debug_assert_eq!(entry.key(), index as usize);
        entry.insert(Node::new(document, id, tag, ext));
        id
    }

    pub(crate) fn remove_node(&mut self, id: ElementId) -> Option<Node<T>> {
        self.inner.note_mutation();
        self.inner.node(id)?;
        let element = self.inner.nodes.try_remove(id.index as usize)?;
        // A dead element's pending snapshot must not survive it; the map entry
        // is keyed by the (now stale) opaque id and is dropped with the map on
        // the next `complete_flush`. Removing it eagerly keeps the map small.
        if element.snapshot_present() {
            self.inner.snapshots.remove(&id.opaque());
        }
        // No internal raw id may survive physical reclamation. In particular,
        // later slot reuse must not let flush cleanup mistake a new node for
        // this removed snapshot owner.
        self.inner
            .snapshotted
            .retain(|&snapshotted| snapshotted != id);
        Some(element)
    }

    pub(crate) fn live_ids(&self) -> Vec<ElementId> {
        self.inner
            .nodes
            .iter()
            .map(|(_, node)| node.node_id())
            .collect()
    }

    /// Number of live nodes retained by this document.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.nodes.len()
    }

    /// Whether this document retains no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Borrow an element if the handle is live.
    #[must_use]
    pub fn get(&self, id: ElementId) -> Option<&Node<T>> {
        self.inner.node(id)
    }

    /// Mutably borrow a whole node inside this crate.
    ///
    /// This must not be public: moving/swapping the returned `Node` could move
    /// it into a different document while leaving its back-pointer unchanged.
    pub(crate) fn node_mut(&mut self, id: ElementId) -> Option<&mut Node<T>> {
        self.inner.note_mutation();
        self.inner.node_mut(id)
    }

    /// Mutably borrow an element's class list if the handle is live.
    pub fn classes_mut(&mut self, id: ElementId) -> Option<&mut SmallVec<[Atom; 4]>> {
        self.node_mut(id).map(|node| &mut node.classes)
    }

    /// Mutably borrow an element's id-selector value if the handle is live.
    pub fn id_attr_mut(&mut self, id: ElementId) -> Option<&mut Option<Atom>> {
        self.node_mut(id).map(|node| &mut node.id_attr)
    }

    /// Mutably borrow an element's ordinary attribute map if the handle is live.
    pub fn attrs_mut(&mut self, id: ElementId) -> Option<&mut FxHashMap<Box<str>, String>> {
        self.node_mut(id).map(|node| &mut node.attrs)
    }

    /// Mutably borrow an element's dynamic pseudo-class state if the handle is live.
    pub fn element_state_mut(&mut self, id: ElementId) -> Option<&mut ElementState> {
        self.node_mut(id).map(|node| &mut node.element_state)
    }

    /// Mutably borrow an element's character data if the handle is live.
    pub fn text_mut(&mut self, id: ElementId) -> Option<&mut Option<String>> {
        self.node_mut(id).map(|node| &mut node.text)
    }

    /// Mutably borrow an element's embedder payload if the handle is live.
    pub fn ext_mut(&mut self, id: ElementId) -> Option<&mut T> {
        self.node_mut(id).map(|node| &mut node.ext)
    }

    /// Whether the handle currently resolves to a live element.
    #[must_use]
    pub fn contains(&self, id: ElementId) -> bool {
        self.get(id).is_some()
    }

    /// A read-only navigation handle for the element, if live.
    #[must_use]
    pub fn element_ref(&self, id: ElementId) -> Option<&Node<T>> {
        self.get(id)
    }

    /// Borrow a live node by id.
    #[must_use]
    pub fn node(&self, id: ElementId) -> Option<&Node<T>> {
        self.get(id)
    }

    /// Clear every element's dirty bits.
    ///
    /// Establishes a clean baseline (tests, or an embedder resetting a tree).
    /// The flush path uses the cheaper targeted
    /// [`complete_flush`](Self::complete_flush) instead.
    pub fn clear_dirty(&mut self) {
        self.inner.note_mutation();
        for (_, element) in &mut self.inner.nodes {
            element.set_style_dirty(false);
            element.set_dirty_descendants_bit(false);
        }
    }

    /// Clear the flush-scheduling state after a style traversal: drops the
    /// consumed snapshots and walks only the dirty spine under `root` clearing
    /// the dirty bits.
    ///
    /// The spine walk cannot see below an element whose `dirty_descendants`
    /// stylo already cleared (it does so when a subtree computes to
    /// `display: none`), so `style_dirty` breadcrumbs inside such a subtree
    /// may survive — see [`Node::is_style_dirty`](crate::Node::is_style_dirty).
    pub(crate) fn complete_flush(&mut self, root: ElementId) {
        self.inner.assert_idle();
        for id in std::mem::take(&mut self.inner.snapshotted) {
            if let Some(element) = self.get(id) {
                element.clear_snapshot_flags();
            }
        }
        self.inner.snapshots.clear();

        // Walk the dirty spine: clear `style_dirty` on every child of a
        // dirty-descendants node, but only descend where the bit is set.
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let Some(element) = self.inner.node_mut(current) else {
                continue;
            };
            element.set_style_dirty(false);
            if element.has_dirty_descendants() {
                element.set_dirty_descendants_bit(false);
                stack.extend_from_slice(&element.children);
            }
        }
    }

    pub(crate) fn begin_traversal(&self) -> TraversalGuard<'_> {
        self.inner.begin_traversal()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    #[cfg(debug_assertions)]
    use std::sync::atomic::{AtomicU8, Ordering};

    #[cfg(debug_assertions)]
    use super::{DEBUG_PHASE_IDLE, DEBUG_PHASE_POISONED, DEBUG_PHASE_TRAVERSING};
    use super::{DocumentPhase, assert_idle, begin_traversal};

    #[test]
    fn traversal_phase_rejects_mutation() {
        let phase = Cell::new(DocumentPhase::Idle);
        #[cfg(debug_assertions)]
        let debug_phase = AtomicU8::new(DEBUG_PHASE_IDLE);
        let _guard = begin_traversal(
            &phase,
            #[cfg(debug_assertions)]
            &debug_phase,
        );
        #[cfg(debug_assertions)]
        assert_eq!(debug_phase.load(Ordering::Acquire), DEBUG_PHASE_TRAVERSING);
        let mutation = catch_unwind(AssertUnwindSafe(|| assert_idle(&phase)));
        assert!(mutation.is_err());
    }

    #[test]
    fn panicking_traversal_poisons_document() {
        let phase = Cell::new(DocumentPhase::Idle);
        #[cfg(debug_assertions)]
        let debug_phase = AtomicU8::new(DEBUG_PHASE_IDLE);
        let traversal = catch_unwind(AssertUnwindSafe(|| {
            let _guard = begin_traversal(
                &phase,
                #[cfg(debug_assertions)]
                &debug_phase,
            );
            panic!("synthetic traversal failure");
        }));
        assert!(traversal.is_err());
        #[cfg(debug_assertions)]
        assert_eq!(debug_phase.load(Ordering::Acquire), DEBUG_PHASE_POISONED);

        let mutation = catch_unwind(AssertUnwindSafe(|| assert_idle(&phase)));
        assert!(mutation.is_err());
    }
}
