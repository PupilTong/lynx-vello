//! The unified [`Node`] struct — the HTML-DOM-subset node.

use std::cell::UnsafeCell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use dom::ElementState;
use rustc_hash::FxHashMap;
use selectors::matching::ElementSelectorFlags;
use smallvec::SmallVec;
use stylo::LocalName;
use stylo::data::ElementDataWrapper;
use stylo::properties::{ComputedValues, PropertyDeclarationBlock};
use stylo::servo_arc::Arc;
use stylo::shared_lock::Locked;
use stylo_atoms::Atom;

use crate::arena::{DocumentInner, ElementId};

#[cfg(debug_assertions)]
static NEXT_DEBUG_STYLE_OWNER: AtomicUsize = AtomicUsize::new(1);

#[cfg(debug_assertions)]
thread_local! {
    /// A compact, process-unique token used in per-element diagnostic atomics.
    static DEBUG_STYLE_OWNER: usize = {
        let owner = NEXT_DEBUG_STYLE_OWNER.fetch_add(1, Ordering::Relaxed);
        assert_ne!(owner, 0, "debug style owner token space exhausted");
        owner
    };
}

#[cfg(debug_assertions)]
fn debug_style_owner() -> usize {
    DEBUG_STYLE_OWNER.with(|owner| *owner)
}

/// Debug-only lease proving that this thread is the current accessor of an
/// element's outer `Option<ElementDataWrapper>` slot.
#[cfg(debug_assertions)]
pub(crate) struct DebugStyleDataOwnerGuard<'a> {
    owner: &'a AtomicUsize,
    acquired: bool,
}

/// Debug-only shared lease protecting an outer style-data-slot read. The
/// returned stylo `ElementDataRef` carries its own `AtomicRefCell` borrow after
/// this short-lived outer-slot lease is released.
#[cfg(debug_assertions)]
pub(crate) struct DebugStyleDataReadGuard<'a> {
    readers: &'a AtomicUsize,
    acquired: bool,
}

#[cfg(debug_assertions)]
impl<'a> DebugStyleDataOwnerGuard<'a> {
    fn claim(
        owner: &'a AtomicUsize,
        readers: &AtomicUsize,
        element_index: u32,
        reentrant: bool,
    ) -> Self {
        let current = debug_style_owner();
        match owner.compare_exchange(0, current, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                let reader_count = readers.load(Ordering::Acquire);
                if reader_count != 0 {
                    owner.store(0, Ordering::Release);
                    panic!(
                        "mutable ElementData ownership for element slot {element_index} overlaps {reader_count} outer-slot reader(s)"
                    );
                }
                Self {
                    owner,
                    acquired: true,
                }
            }
            Err(existing) if reentrant && existing == current => Self {
                owner,
                acquired: false,
            },
            Err(existing) => panic!(
                "concurrent ElementData ownership for element slot {element_index}: owner={existing}, contender={current}"
            ),
        }
    }
}

#[cfg(debug_assertions)]
impl<'a> DebugStyleDataReadGuard<'a> {
    fn claim(owner: &AtomicUsize, readers: &'a AtomicUsize, element_index: u32) -> Self {
        let current = debug_style_owner();
        let existing = owner.load(Ordering::Acquire);
        if existing == current {
            return Self {
                readers,
                acquired: false,
            };
        }
        assert_eq!(
            existing, 0,
            "ElementData read for element slot {element_index} overlaps mutable owner {existing}"
        );

        let previous = readers.fetch_add(1, Ordering::AcqRel);
        assert_ne!(previous, usize::MAX, "debug style reader count overflow");
        let existing = owner.load(Ordering::Acquire);
        if existing != 0 {
            readers.fetch_sub(1, Ordering::AcqRel);
            panic!(
                "ElementData read for element slot {element_index} raced mutable owner {existing}"
            );
        }
        Self {
            readers,
            acquired: true,
        }
    }
}

#[cfg(debug_assertions)]
impl Drop for DebugStyleDataOwnerGuard<'_> {
    fn drop(&mut self) {
        if self.acquired {
            self.owner.store(0, Ordering::Release);
        }
    }
}

#[cfg(debug_assertions)]
impl Drop for DebugStyleDataReadGuard<'_> {
    fn drop(&mut self) {
        if self.acquired {
            self.readers.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Bit set in [`Node::snapshot_flags`] when a pre-mutation snapshot has
/// been recorded for this element in the arena's
/// [`SnapshotMap`](stylo::selector_parser::SnapshotMap).
pub(crate) const SNAPSHOT_PRESENT: u8 = 1 << 0;
/// Bit set once stylo's invalidation pass has consumed the snapshot.
pub(crate) const SNAPSHOT_HANDLED: u8 = 1 << 1;

/// A single element in the document tree.
///
/// The fields model a strict subset of the HTML DOM: tree links, tag, id,
/// classes, attributes, dynamic pseudo-class state, an inline style block,
/// character data, and the per-element style bookkeeping stylo needs. Anything
/// beyond that subset belongs to the embedder and lives in the [`ext`] payload
/// (see [`ExternalState`](crate::ExternalState)).
///
/// Nodes are created only by [`Document::create_element`](crate::Document::create_element),
/// which installs their document back-pointer and stable id before publishing
/// them. Detaching changes tree links but keeps the node in that document;
/// physical removal consumes the node and returns only its external payload.
///
/// # Thread-safety
///
/// stylo's restyle traversal may run **in parallel** (rayon workers sharing
/// `&Document`), so every piece of element state that stylo touches during a
/// traversal is either
///
/// - atomic ([`selector_flags`], [`dirty_descendants`], [`snapshot_flags`]), or
/// - owned by exactly one worker at a time under stylo's traversal discipline
///   ([`style_element_data`], an `UnsafeCell<Option<ElementDataWrapper>>`; see [`crate::traits`]
///   for the per-access safety arguments).
///
/// Everything else (tag/classes/attrs/text/`ext`) is **immutable during a
/// flush**: mutation requires `&mut Document`, which
/// [`Document::flush`](crate::Document::flush) holds
/// exclusively for the whole traversal. The flush API additionally requires
/// `T: Sync`, because selector matching may read the external payload from
/// multiple workers.
///
/// [`ext`]: Node::ext
/// [`style_element_data`]: Node::style_element_data
/// [`selector_flags`]: Node::selector_flags
/// [`style_dirty`]: Node::style_dirty
/// [`dirty_descendants`]: Node::dirty_descendants
/// [`snapshot_flags`]: Node::snapshot_flags
pub struct Node<T> {
    /// Stable back-pointer to the document allocation that owns this node.
    document: NonNull<DocumentInner<T>>,
    /// This node's internal slot identity in `document`.
    id: ElementId,
    /// The parent element, or `None` for the root / a detached element.
    pub parent: Option<ElementId>,
    /// Child elements, in document order.
    pub children: Vec<ElementId>,
    /// The tag name, interned as a stylo [`LocalName`] atom so
    /// `selectors::Element::has_local_name` is a cheap atom comparison.
    pub tag: LocalName,
    /// The element's classes, interned as atoms.
    pub classes: SmallVec<[Atom; 4]>,
    /// The element's `id` selector value, distinct from a plain `id` attribute
    /// (the embedder decides whether/how the two are linked).
    pub id_attr: Option<Atom>,
    /// Plain attributes. Synthetic / reflected attributes beyond this map are
    /// served by the [`ext`](Node::ext) payload's
    /// [`extra_attr_value`](crate::ExternalState::extra_attr_value) hook.
    pub attrs: FxHashMap<Box<str>, String>,
    /// Active dynamic pseudo-classes (`:hover` / `:active` / `:focus`) as stylo
    /// state bits, written by the embedder (see
    /// [`PseudoState`](crate::PseudoState) for the bridge type).
    pub element_state: ElementState,

    /// The element's parsed inline style block (the `style` attribute), locked
    /// under the arena's [`SharedRwLock`](stylo::shared_lock::SharedRwLock).
    /// `None` when no inline style is set.
    pub inline_block: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    /// stylo's per-element style data (`ElementData`), created lazily via
    /// `TElement::ensure_data`. The resolved computed style lives here (see
    /// [`computed_style`](Node::computed_style)). Only touched through the
    /// [`traits`](crate::traits) impls under stylo's traversal discipline.
    pub(crate) style_element_data: UnsafeCell<Option<ElementDataWrapper>>,
    /// Worker currently processing or inspecting `style_element_data`.
    /// Strict preorder claims reject duplicate processing; nested accesses by
    /// that worker are allowed. Completely absent from release builds.
    #[cfg(debug_assertions)]
    debug_style_data_owner: AtomicUsize,
    /// Number of workers currently inspecting the outer style-data slot.
    #[cfg(debug_assertions)]
    debug_style_data_readers: AtomicUsize,

    /// Selector flags accumulated by stylo during matching (e.g. "has a
    /// child-position-dependent rule"), stored as the raw
    /// [`ElementSelectorFlags`] bits. Atomic because parallel workers matching
    /// sibling elements may both push `for_parent()` flags onto the shared
    /// parent.
    pub(crate) selector_flags: AtomicUsize,

    /// Whether this element itself has pending style work (embedder-visible
    /// dirty signal; stylo's own scheduling uses `ElementData::hint`).
    pub(crate) style_dirty: bool,
    /// Whether some descendant of this element has pending style work. This is
    /// the bit stylo's traversal walks down
    /// ([`TElement::has_dirty_descendants`](stylo::dom::TElement::has_dirty_descendants)).
    pub(crate) dirty_descendants: AtomicBool,

    /// Snapshot lifecycle bits ([`SNAPSHOT_PRESENT`] / [`SNAPSHOT_HANDLED`]),
    /// mirroring `TElement::{has_snapshot, handled_snapshot}`.
    pub(crate) snapshot_flags: AtomicU8,

    /// Literal character-data content, for text leaves.
    pub text: Option<String>,

    /// The embedder's external-state payload (see
    /// [`ExternalState`](crate::ExternalState)).
    pub ext: T,
}

impl<T> Node<T> {
    pub(crate) fn new(
        document: NonNull<DocumentInner<T>>,
        id: ElementId,
        tag: &str,
        ext: T,
    ) -> Self {
        Self {
            document,
            id,
            parent: None,
            children: Vec::new(),
            tag: LocalName::from(tag),
            classes: SmallVec::new(),
            id_attr: None,
            attrs: FxHashMap::default(),
            element_state: ElementState::empty(),
            inline_block: None,
            style_element_data: UnsafeCell::new(None),
            #[cfg(debug_assertions)]
            debug_style_data_owner: AtomicUsize::new(0),
            #[cfg(debug_assertions)]
            debug_style_data_readers: AtomicUsize::new(0),
            selector_flags: AtomicUsize::new(0),
            style_dirty: false,
            dirty_descendants: AtomicBool::new(false),
            snapshot_flags: AtomicU8::new(0),
            text: None,
            ext,
        }
    }

    /// This node's internal identity within its owning document.
    #[must_use]
    pub const fn node_id(&self) -> ElementId {
        self.id
    }

    /// Compatibility spelling for [`Node::node_id`].
    #[must_use]
    pub fn id(&self) -> ElementId {
        self.node_id()
    }

    /// The element tag as a string slice.
    #[must_use]
    pub fn tag(&self) -> &str {
        self.tag_str()
    }

    /// The embedder's external-state payload.
    #[must_use]
    pub const fn ext(&self) -> &T {
        &self.ext
    }

    /// The stable document allocation that owns this live node.
    pub(crate) fn document(&self) -> &DocumentInner<T> {
        // SAFETY: `Document` owns `DocumentInner` through a private `Box`, so
        // moving the public owner never moves this allocation. Only the document can
        // construct nodes, and physical removal consumes the node without
        // exposing it, so every reachable node carries this live pointer.
        #[expect(
            unsafe_code,
            reason = "node navigation follows its stable document back-pointer"
        )]
        unsafe {
            self.document.as_ref()
        }
    }

    /// The parent node, if attached.
    #[must_use]
    pub fn parent(&self) -> Option<&Node<T>> {
        self.document().node(self.parent?)
    }

    /// The first child node, if any.
    #[must_use]
    pub fn first_child(&self) -> Option<&Node<T>> {
        self.document().node(*self.children.first()?)
    }

    /// The last child node, if any.
    #[must_use]
    pub fn last_child(&self) -> Option<&Node<T>> {
        self.document().node(*self.children.last()?)
    }

    /// The next sibling node, if any.
    #[must_use]
    pub fn next_sibling(&self) -> Option<&Node<T>> {
        let document = self.document();
        let siblings = &document.node(self.parent?)?.children;
        let position = siblings.iter().position(|&id| id == self.node_id())?;
        document.node(*siblings.get(position + 1)?)
    }

    /// The previous sibling node, if any.
    #[must_use]
    pub fn prev_sibling(&self) -> Option<&Node<T>> {
        let document = self.document();
        let siblings = &document.node(self.parent?)?.children;
        let position = siblings.iter().position(|&id| id == self.node_id())?;
        document.node(*siblings.get(position.checked_sub(1)?)?)
    }

    /// Child nodes in document order.
    pub fn children(&self) -> impl Iterator<Item = &Node<T>> {
        let document = self.document();
        self.children
            .iter()
            .filter_map(move |&id| document.node(id))
    }

    /// The element record used by stylo's trait implementations.
    pub(crate) const fn element(&self) -> &Self {
        self
    }

    /// Claim this element for one complete `process_preorder` call.
    #[cfg(debug_assertions)]
    pub(crate) fn debug_claim_style_data_for_traversal(&self) -> DebugStyleDataOwnerGuard<'_> {
        self.document().debug_assert_traversing();
        DebugStyleDataOwnerGuard::claim(
            &self.debug_style_data_owner,
            &self.debug_style_data_readers,
            self.id.index(),
            false,
        )
    }

    /// Claim mutable access to the outer style-data slot for one trait method.
    /// Accesses nested under this thread's preorder claim are reentrant.
    #[cfg(debug_assertions)]
    pub(crate) fn debug_claim_style_data_write(&self) -> DebugStyleDataOwnerGuard<'_> {
        self.document().debug_assert_style_data_access();
        DebugStyleDataOwnerGuard::claim(
            &self.debug_style_data_owner,
            &self.debug_style_data_readers,
            self.id.index(),
            true,
        )
    }

    /// Claim a shared read of the outer style-data slot. Multiple traversal
    /// workers may legitimately read a common parent at the same time.
    #[cfg(debug_assertions)]
    pub(crate) fn debug_claim_style_data_read(&self) -> DebugStyleDataReadGuard<'_> {
        self.document().debug_assert_style_data_access();
        DebugStyleDataReadGuard::claim(
            &self.debug_style_data_owner,
            &self.debug_style_data_readers,
            self.id.index(),
        )
    }

    /// The element's tag name as a string slice.
    #[must_use]
    pub fn tag_str(&self) -> &str {
        self.tag.0.as_ref()
    }

    /// Whether stylo has ever created per-element style data here (i.e. the
    /// element has been through a style pass).
    ///
    /// Must not be called while a style flush is running on the element's
    /// arena (impossible through the public API: a flush holds `&mut Document`).
    #[must_use]
    pub fn has_style_data(&self) -> bool {
        #[cfg(debug_assertions)]
        let _reader = self.debug_claim_style_data_read();
        // SAFETY: reads only the `Option` discriminant; no flush is running
        // (flushes require `&mut Document`, and this reference came from that
        // document).
        #[expect(unsafe_code, reason = "read the stylo data slot outside a flush")]
        unsafe {
            (*self.style_element_data.get()).is_some()
        }
    }

    /// The resolved computed style, if this element has been styled.
    ///
    /// The style lives in stylo's per-element `ElementData`; this clones the
    /// `Arc` out of it. Must not be called while a style flush is running on
    /// the element's arena (impossible through the public API: a flush holds
    /// `&mut Document`).
    #[must_use]
    pub fn computed_style(&self) -> Option<Arc<ComputedValues>> {
        #[cfg(debug_assertions)]
        let _reader = self.debug_claim_style_data_read();
        // SAFETY: no flush is running, so reading the slot and borrowing an
        // initialized wrapper cannot race with mutation.
        #[expect(unsafe_code, reason = "borrow the stylo data slot outside a flush")]
        let slot = unsafe { (*self.style_element_data.get()).as_ref() };
        slot.and_then(|wrapper| wrapper.borrow().styles.primary.clone())
    }

    /// Store a resolved computed style, creating the stylo `ElementData` slot
    /// if needed. Used by the standalone
    /// [`Document::resolve`](crate::Document::resolve) path; the flush
    /// traversal writes styles through stylo itself.
    pub(crate) fn set_computed_style(&mut self, style: Arc<ComputedValues>) {
        let wrapper = self
            .style_element_data
            .get_mut()
            .get_or_insert_with(ElementDataWrapper::default);
        wrapper.borrow_mut().styles.primary = Some(style);
    }

    /// The accumulated stylo selector flags.
    #[must_use]
    pub fn selector_flags(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::from_bits_retain(self.selector_flags.load(Ordering::Relaxed))
    }

    /// Whether this element itself has pending style work.
    ///
    /// A scheduling breadcrumb, not ground truth: the authoritative "does the
    /// tree need a flush" signal is the root's bits. In one corner the
    /// breadcrumb can go stale — a descendant of a subtree that became
    /// `display: none` in the same flush keeps its bit set (stylo prunes the
    /// none-subtree from traversal and drops its style data; the bit clears
    /// the next time the element is scheduled while reachable).
    #[must_use]
    pub fn is_style_dirty(&self) -> bool {
        self.style_dirty
    }

    /// Whether a descendant has pending style work.
    #[must_use]
    pub fn has_dirty_descendants(&self) -> bool {
        self.dirty_descendants.load(Ordering::Relaxed)
    }

    pub(crate) const fn set_style_dirty(&mut self, dirty: bool) {
        self.style_dirty = dirty;
    }

    pub(crate) fn set_dirty_descendants_bit(&self, dirty: bool) {
        self.dirty_descendants.store(dirty, Ordering::Relaxed);
    }

    pub(crate) fn snapshot_present(&self) -> bool {
        self.snapshot_flags.load(Ordering::Relaxed) & SNAPSHOT_PRESENT != 0
    }

    pub(crate) fn snapshot_handled(&self) -> bool {
        self.snapshot_flags.load(Ordering::Relaxed) & SNAPSHOT_HANDLED != 0
    }

    pub(crate) fn set_snapshot_present(&self) {
        self.snapshot_flags
            .fetch_or(SNAPSHOT_PRESENT, Ordering::Relaxed);
    }

    pub(crate) fn set_snapshot_handled(&self) {
        self.snapshot_flags
            .fetch_or(SNAPSHOT_HANDLED, Ordering::Relaxed);
    }

    pub(crate) fn clear_snapshot_flags(&self) {
        self.snapshot_flags.store(0, Ordering::Relaxed);
    }

    /// Mutable access to the stylo `ElementData` wrapper, if it exists.
    ///
    /// Safe because it goes through `&mut self`: exclusive access to the
    /// element means no traversal is concurrently touching the slot.
    pub(crate) fn style_element_data_mut(&mut self) -> Option<&mut ElementDataWrapper> {
        self.style_element_data.get_mut().as_mut()
    }
}

impl<T> PartialEq for Node<T> {
    fn eq(&self, other: &Self) -> bool {
        self.document == other.document && self.id == other.id
    }
}

impl<T> Eq for Node<T> {}

impl<T> Hash for Node<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.document.hash(state);
        self.id.hash(state);
    }
}

impl<T> fmt::Debug for Node<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `style_element_data` is deliberately omitted: a generic Debug call
        // cannot establish the no-concurrent-flush invariant required to read it.
        f.debug_struct("Node")
            .field("node_id", &self.id)
            .field("tag", &self.tag_str())
            .field("classes", &self.classes)
            .field("id_attr", &self.id_attr)
            .field("element_state", &self.element_state)
            .field("has_inline_block", &self.inline_block.is_some())
            .field("style_dirty", &self.is_style_dirty())
            .field("dirty_descendants", &Node::has_dirty_descendants(self))
            .field("children", &self.children)
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{DebugStyleDataOwnerGuard, DebugStyleDataReadGuard};

    #[test]
    fn style_data_owner_rejects_duplicate_preorder_claim() {
        let owner = AtomicUsize::new(0);
        let readers = AtomicUsize::new(0);
        let _first = DebugStyleDataOwnerGuard::claim(&owner, &readers, 7, false);

        let duplicate = catch_unwind(AssertUnwindSafe(|| {
            let _duplicate = DebugStyleDataOwnerGuard::claim(&owner, &readers, 7, false);
        }));

        assert!(duplicate.is_err());
    }

    #[test]
    fn style_data_owner_allows_same_worker_nested_access() {
        let owner = AtomicUsize::new(0);
        let readers = AtomicUsize::new(0);
        let first = DebugStyleDataOwnerGuard::claim(&owner, &readers, 7, false);
        let claimed = owner.load(Ordering::Acquire);
        {
            let _nested = DebugStyleDataOwnerGuard::claim(&owner, &readers, 7, true);
            assert_eq!(owner.load(Ordering::Acquire), claimed);
        }
        assert_eq!(owner.load(Ordering::Acquire), claimed);
        drop(first);
        assert_eq!(owner.load(Ordering::Acquire), 0);
    }

    #[test]
    fn style_data_owner_rejects_another_thread() {
        let owner = AtomicUsize::new(0);
        let readers = AtomicUsize::new(0);
        let _first = DebugStyleDataOwnerGuard::claim(&owner, &readers, 7, false);

        std::thread::scope(|scope| {
            let contender = scope
                .spawn(|| {
                    catch_unwind(AssertUnwindSafe(|| {
                        let _contender = DebugStyleDataOwnerGuard::claim(&owner, &readers, 7, true);
                    }))
                })
                .join()
                .expect("contender thread must return normally");
            assert!(contender.is_err());
        });
    }

    #[test]
    fn style_data_readers_can_overlap_but_exclude_a_writer() {
        let owner = AtomicUsize::new(0);
        let readers = AtomicUsize::new(0);
        let first = DebugStyleDataReadGuard::claim(&owner, &readers, 7);

        std::thread::scope(|scope| {
            let second = scope.spawn(|| {
                let second = DebugStyleDataReadGuard::claim(&owner, &readers, 7);
                assert_eq!(readers.load(Ordering::Acquire), 2);
                second
            });
            let second = second.join().expect("second reader must be accepted");
            drop(second);
        });

        let writer = catch_unwind(AssertUnwindSafe(|| {
            let _writer = DebugStyleDataOwnerGuard::claim(&owner, &readers, 7, true);
        }));
        assert!(writer.is_err());
        drop(first);
        assert_eq!(readers.load(Ordering::Acquire), 0);
    }
}
