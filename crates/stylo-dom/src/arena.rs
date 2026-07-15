//! The generational storage backing a [`Document`].
//!
//! Elements live in a hand-rolled `Vec<Slot>` arena with a free list — no
//! `slotmap` dependency, deliberately minimal. Each element is addressed by an
//! [`ElementId`] carrying the slot index plus the slot's generation; once a
//! slot is freed its generation is bumped, so any [`ElementId`] referring to
//! the previous occupant becomes stale and resolves to `None`.
//!
//! Every live node carries an address-stable back-pointer to its document;
//! stylo's DOM traits are therefore implemented directly on `&Node<T>`.
//!
//! The document also owns the **pending snapshot set** for stylo's
//! invalidation-set restyle: before a matching-relevant mutation, the embedder
//! layer records the element's old state/attributes here (see
//! [`crate::dirty`]), and the next
//! [`Document::flush`](crate::Document::flush) consumes them.

use std::fmt;
use std::num::NonZeroU32;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use dom::ElementState;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::UrlExtraData;
use stylo::stylist::Stylist;
use stylo_atoms::Atom;

use crate::node::Node;

/// A stable, generation-checked handle to an element in a [`Document`].
///
/// Cheap to copy and hash. A handle stays valid until its element is removed;
/// afterwards the slot's generation advances and the handle becomes stale
/// (arena lookups return `None`), even if the slot is later reused by a
/// different element.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ElementId {
    index: u32,
    generation: NonZeroU32,
}

impl ElementId {
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
                "ElementId::opaque requires a 64-bit target"
            );
        }
        OpaqueNode(((self.generation.get() as usize) << 32) | self.index as usize)
    }
}

/// One arena slot: the current generation plus an optional live [`Node`].
#[derive(Debug)]
struct Slot<T> {
    generation: NonZeroU32,
    element: Option<Node<T>>,
}

/// The address-stable document allocation that live nodes point back to.
pub(crate) struct DocumentInner<T> {
    pub(crate) stylist: Stylist,
    slots: Vec<Slot<T>>,
    free_list: Vec<u32>,
    pub(crate) lock: SharedRwLock,
    pub(crate) url_data: UrlExtraData,
    snapshots: SnapshotMap,
    snapshotted: Vec<ElementId>,
    phase: AtomicU8,
    mutation_epoch: AtomicU64,
}

const PHASE_IDLE: u8 = 0;
const PHASE_TRAVERSING: u8 = 1;
const PHASE_POISONED: u8 = 2;

/// RAII exclusion guard for stylo's possibly parallel traversal.
pub(crate) struct TraversalGuard<'a> {
    phase: &'a AtomicU8,
    mutation_epoch: &'a AtomicU64,
    entered_epoch: u64,
}

impl Drop for TraversalGuard<'_> {
    fn drop(&mut self) {
        let panicking = std::thread::panicking();
        self.phase.store(
            if panicking {
                PHASE_POISONED
            } else {
                PHASE_IDLE
            },
            Ordering::Release,
        );
        // Do not risk a second panic while unwinding. A traversal panic already
        // poisons the document, so every later mutation/flush will fail fast.
        if !panicking {
            debug_assert_eq!(
                self.mutation_epoch.load(Ordering::Acquire),
                self.entered_epoch,
                "document mutation epoch changed during style traversal"
            );
        }
    }
}

impl<T> DocumentInner<T> {
    pub(crate) fn node(&self, id: ElementId) -> Option<&Node<T>> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation {
            slot.element.as_ref()
        } else {
            None
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
        self.mutation_epoch.fetch_add(1, Ordering::Relaxed);
    }

    fn begin_traversal(&self) -> TraversalGuard<'_> {
        begin_traversal(&self.phase, &self.mutation_epoch)
    }
}

fn assert_idle(phase: &AtomicU8) {
    match phase.load(Ordering::Acquire) {
        PHASE_IDLE => {}
        PHASE_TRAVERSING => panic!("document mutation attempted during style traversal"),
        PHASE_POISONED => panic!("document was poisoned by a panicking style traversal"),
        _ => unreachable!("invalid document phase"),
    }
}

fn begin_traversal<'a>(phase: &'a AtomicU8, mutation_epoch: &'a AtomicU64) -> TraversalGuard<'a> {
    phase
        .compare_exchange(
            PHASE_IDLE,
            PHASE_TRAVERSING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .unwrap_or_else(|phase| match phase {
            PHASE_TRAVERSING => panic!("nested style traversal on one document"),
            PHASE_POISONED => panic!("document was poisoned by a panicking style traversal"),
            _ => unreachable!("invalid document phase"),
        });
    TraversalGuard {
        phase,
        mutation_epoch,
        entered_epoch: mutation_epoch.load(Ordering::Acquire),
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
            .field("slots", &self.inner.slots)
            .field("free_list", &self.inner.free_list)
            .field("pending_snapshots", &self.inner.snapshotted.len())
            .finish_non_exhaustive()
    }
}

impl<T> Document<T> {
    pub(crate) fn from_parts(stylist: Stylist, lock: SharedRwLock, url_data: UrlExtraData) -> Self {
        Self {
            inner: Box::new(DocumentInner {
                stylist,
                slots: Vec::new(),
                free_list: Vec::new(),
                lock,
                url_data,
                snapshots: SnapshotMap::new(),
                snapshotted: Vec::new(),
                phase: AtomicU8::new(PHASE_IDLE),
                mutation_epoch: AtomicU64::new(0),
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
    /// Panics if the arena would need to grow past `u32::MAX` slots.
    pub fn create_element(&mut self, tag: &str, ext: T) -> ElementId {
        self.inner.note_mutation();
        let document = NonNull::from(self.inner.as_mut());
        if let Some(index) = self.inner.free_list.pop() {
            let slot = &mut self.inner.slots[index as usize];
            debug_assert!(slot.element.is_none(), "free-list slot must be vacant");
            let id = ElementId {
                index,
                generation: slot.generation,
            };
            slot.element = Some(Node::new(document, id, tag, ext));
            id
        } else {
            let index = u32::try_from(self.inner.slots.len())
                .expect("arena capacity exceeds u32::MAX slots");
            let id = ElementId {
                index,
                generation: NonZeroU32::MIN,
            };
            self.inner.slots.push(Slot {
                generation: NonZeroU32::MIN,
                element: Some(Node::new(document, id, tag, ext)),
            });
            id
        }
    }

    /// Destroy an element, returning its external-state payload if the handle
    /// is live.
    ///
    /// The slot's generation is advanced so the passed handle (and any other
    /// handle sharing the slot) becomes stale. If a slot's generation is
    /// exhausted it is retired rather than reused, preserving uniqueness.
    pub fn remove(&mut self, id: ElementId) -> Option<T> {
        self.remove_node(id).map(|node| node.ext)
    }

    pub(crate) fn remove_node(&mut self, id: ElementId) -> Option<Node<T>> {
        self.inner.note_mutation();
        let slot = self.inner.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        let element = slot.element.take()?;
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.inner.free_list.push(id.index);
        } else {
            // Generation space exhausted for this slot: retire it (never
            // reuse) so no future handle can collide with a past one.
        }
        // A dead element's pending snapshot must not survive it; the map entry
        // is keyed by the (now stale) opaque id and is dropped with the map on
        // the next `complete_flush`. Removing it eagerly keeps the map small.
        if element.snapshot_present() {
            self.inner.snapshots.remove(&id.opaque());
        }
        Some(element)
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
        let slot = self.inner.slots.get_mut(id.index as usize)?;
        if slot.generation == id.generation {
            slot.element.as_mut()
        } else {
            None
        }
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
        for slot in &mut self.inner.slots {
            if let Some(element) = &mut slot.element {
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
            let Some(element) = self.get(current) else {
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
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicU8, AtomicU64};

    use super::{PHASE_IDLE, assert_idle, begin_traversal};

    #[test]
    fn traversal_phase_rejects_mutation() {
        let phase = AtomicU8::new(PHASE_IDLE);
        let mutation_epoch = AtomicU64::new(0);
        let _guard = begin_traversal(&phase, &mutation_epoch);
        let mutation = catch_unwind(AssertUnwindSafe(|| assert_idle(&phase)));
        assert!(mutation.is_err());
    }

    #[test]
    fn panicking_traversal_poisons_document() {
        let phase = AtomicU8::new(PHASE_IDLE);
        let mutation_epoch = AtomicU64::new(0);
        let traversal = catch_unwind(AssertUnwindSafe(|| {
            let _guard = begin_traversal(&phase, &mutation_epoch);
            panic!("synthetic traversal failure");
        }));
        assert!(traversal.is_err());

        let mutation = catch_unwind(AssertUnwindSafe(|| assert_idle(&phase)));
        assert!(mutation.is_err());
    }
}
