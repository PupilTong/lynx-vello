//! [`Node`] — the unit the tree is composed of — and its `&Node` read/
//! navigation handle.
//!
//! A node models a strict subset of the W3C DOM. Slot zero is the document
//! node and owns the shared style context. Element nodes carry a tag, id,
//! classes, attributes, dynamic pseudo-class state, an inline style block,
//! and the per-element style bookkeeping Stylo needs; text nodes carry
//! character data. Element/text variants share tree links and an
//! embedder-owned [`ext`](Node::ext) payload.
//!
//! Nodes are created by
//! [`Document::create_element`](crate::Document::create_element) or
//! [`Document::create_text_node`](crate::Document::create_text_node) — never
//! directly — and every DOM mutation goes through `Document` methods, so
//! pre-mutation snapshots and restyle hints can never be skipped. Shared
//! accessors on `Node` are the read surface.
//!
//! # The backpointer, and why `&Node` is the handle
//!
//! Each node carries a pointer back to the fixed-address [`Slab`](slab::Slab)
//! owned by its [`Document`](crate::Document). Tree navigation therefore
//! needs nothing but the node itself,
//! and stylo's element traits are implemented **directly on `&'a Node<T>`**
//! (see the crate-private `traits` module) — no wrapper handle exists. This
//! is load-bearing beyond convenience — stylo's style-sharing cache sizes its
//! thread-local storage for a word-sized `TElement` handle (see
//! `style/sharing/mod.rs`, `FakeCandidate`), and a shared reference is
//! exactly one word and `Copy` by nature — and it is what lets the restyle
//! traversal run over the one tree in place, with no mirror tree built for
//! styling.
//!
//! # Thread-safety
//!
//! stylo's restyle traversal may run **in parallel** (rayon workers sharing
//! the tree), so every piece of node state that stylo touches during a
//! traversal is either
//!
//! - atomic ([`selector_flags`](Node::selector_flags), the dirty-descendants bit, the snapshot
//!   bits, the traversal counter), or
//! - owned by exactly one worker at a time under stylo's traversal discipline (`stylo_data`, an
//!   [`UnsafeCell`]; see [`crate::traits`] for the per-access safety arguments).
//!
//! Everything else (tag/classes/attrs/text/`ext`) is **immutable during a
//! flush**: mutation requires `&mut Document`, which
//! [`StyleEngine::flush_document`](crate::StyleEngine::flush_document) holds
//! exclusively for the whole traversal.

use std::cell::UnsafeCell;
use std::fmt;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, AtomicUsize, Ordering};

use atomic_refcell::AtomicRefCell;
use dom::ElementState;
use neutron_star::tree::Layout;
use rustc_hash::FxHashMap;
use selectors::matching::ElementSelectorFlags;
use slab::Slab;
use smallvec::SmallVec;
use stylo::LocalName;
use stylo::data::ElementDataWrapper;
use stylo::properties::{ComputedValues, PropertyDeclarationBlock};
use stylo::selector_parser::Snapshot;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{Locked, SharedRwLock};
use stylo::stylesheets::UrlExtraData;
use stylo_atoms::Atom;

use crate::document::{DOCUMENT_NODE_ID, NodeId};
use crate::layout::LayoutData;

/// Debug-only instrumentation for the `stylo_data` slot (finding: a bare
/// `UnsafeCell` makes contract violations undefined behavior instead of a
/// loud failure). The inner `ElementData` borrows are already checked by
/// stylo's `ElementDataWrapper` (debug borrow tracking); this guard covers
/// the layer that wrapper cannot see — the **`Option` slot itself**
/// (`ensure_data`/`clear_data` writing the slot while another worker reads
/// its discriminant), plus traversal-phase discipline and unwind poisoning.
/// Release builds compile all of it away.
#[cfg(debug_assertions)]
pub(crate) mod slot_guard {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    /// A process-unique token for the current thread
    /// (`ThreadId::as_u64` is unstable; this is its moral equivalent).
    fn current_thread_token() -> u64 {
        use std::cell::Cell;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        thread_local! {
            static TOKEN: Cell<u64> = const { Cell::new(0) };
        }
        TOKEN.with(|token| {
            if token.get() == 0 {
                token.set(NEXT.fetch_add(1, Ordering::Relaxed));
            }
            token.get()
        })
    }

    const FREE: u32 = 0;
    const WRITER: u32 = u32::MAX;

    /// Per-node access guard for the `stylo_data` slot: `FREE`, a reader
    /// count, or `WRITER`; the owning thread of a writer; and a poison flag
    /// set when a panic unwinds through an active access.
    pub(crate) struct SlotGuard {
        state: AtomicU32,
        owner: AtomicU64,
        poisoned: AtomicBool,
    }

    impl SlotGuard {
        pub(crate) const fn new() -> Self {
            Self {
                state: AtomicU32::new(FREE),
                owner: AtomicU64::new(0),
                poisoned: AtomicBool::new(false),
            }
        }

        fn check_poison(&self) {
            assert!(
                !self.poisoned.load(Ordering::Acquire),
                "stylo_data slot poisoned: a panic unwound through an earlier access; the tree's style state is unspecified — discard or rebuild it (see the flush docs)"
            );
        }

        /// Begin an exclusive (slot-mutating) access.
        pub(crate) fn begin_write(&self) -> WriteToken<'_> {
            self.check_poison();
            let prev = self.state.swap(WRITER, Ordering::AcqRel);
            if prev != FREE {
                self.poisoned.store(true, Ordering::Release);
                panic!(
                    "stylo_data slot written while {} — stylo's traversal ownership contract (one worker per element) was violated (writer thread {})",
                    if prev == WRITER {
                        "another worker holds it for writing".to_owned()
                    } else {
                        format!("{prev} reader(s) hold it")
                    },
                    self.owner.load(Ordering::Relaxed),
                );
            }
            self.owner.store(current_thread_token(), Ordering::Relaxed);
            WriteToken { guard: self }
        }

        /// Begin a shared (discriminant/`as_ref`) access.
        pub(crate) fn begin_read(&self) -> ReadToken<'_> {
            self.check_poison();
            let prev = self.state.fetch_add(1, Ordering::AcqRel);
            if prev >= WRITER - 1 {
                self.poisoned.store(true, Ordering::Release);
                panic!(
                    "stylo_data slot read while a writer (thread {}) holds it — stylo's traversal ownership contract was violated",
                    self.owner.load(Ordering::Relaxed),
                );
            }
            ReadToken { guard: self }
        }
    }

    pub(crate) struct WriteToken<'a> {
        guard: &'a SlotGuard,
    }

    impl Drop for WriteToken<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.guard.poisoned.store(true, Ordering::Release);
            }
            self.guard.owner.store(0, Ordering::Relaxed);
            self.guard.state.store(FREE, Ordering::Release);
        }
    }

    pub(crate) struct ReadToken<'a> {
        guard: &'a SlotGuard,
    }

    impl Drop for ReadToken<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.guard.poisoned.store(true, Ordering::Release);
            }
            self.guard.state.fetch_sub(1, Ordering::AcqRel);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn concurrent_writers_panic_and_poison() {
            let guard = SlotGuard::new();
            let _held = guard.begin_write();
            let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _t = guard.begin_write();
            }));
            assert!(second.is_err(), "second writer must panic");
            // The violation poisons the guard for every later access.
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _t = guard.begin_read();
                }))
                .is_err(),
                "post-violation access must report poisoning"
            );
        }

        #[test]
        fn reader_during_writer_panics() {
            let guard = SlotGuard::new();
            let _held = guard.begin_write();
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _t = guard.begin_read();
                }))
                .is_err()
            );
        }

        #[test]
        fn readers_share_and_release() {
            let guard = SlotGuard::new();
            {
                let _a = guard.begin_read();
                let _b = guard.begin_read();
            }
            let _w = guard.begin_write();
        }

        #[test]
        fn unwind_through_access_poisons() {
            let guard = SlotGuard::new();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _t = guard.begin_read();
                panic!("mid-access unwind");
            }));
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _t = guard.begin_read();
                }))
                .is_err(),
                "an unwound access must poison the slot"
            );
        }
    }
}

/// Bit set in [`Node::snapshot_flags`] when this node has a pre-mutation
/// snapshot pending in its [`Node::snapshot`] slot or being consumed by a
/// style flush.
pub(crate) const SNAPSHOT_PRESENT: u8 = 1 << 0;
/// Bit set once stylo's invalidation pass has consumed the snapshot.
pub(crate) const SNAPSHOT_HANDLED: u8 = 1 << 1;

/// The kind of a DOM [`Node`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeType {
    /// The real DOM document node, permanently stored at slab slot zero.
    Document,
    /// An element node with a local tag name and CSS style state.
    Element,
    /// A text node containing character data and no element identity.
    Text,
}

/// Kind-specific node data.
///
/// The document variant owns the context that every node can reach through
/// its slab backpointer. Element/text variants own the embedder payload; the
/// document node deliberately has no `T`, so creating `Document<T>` never
/// requires `T: Default` or a sentinel payload.
pub(crate) enum NodeData<T> {
    Document {
        lock: StdArc<SharedRwLock>,
        url_data: UrlExtraData,
        #[cfg(debug_assertions)]
        in_flush: AtomicBool,
    },
    Element(T),
    Text(T),
}

/// A single node in a [`Document`](crate::Document) tree.
///
/// See the crate docs for the model, the backpointer, and the
/// thread-safety story. All fields are crate-private: reads go through the
/// accessors below, writes through `Document` methods.
pub struct Node<T> {
    /// Backpointer to the owning fixed-address slab.
    tree: AtomicPtr<Slab<Node<T>>>,
    /// This node's raw slab index and stable Stylo `OpaqueNode` identity.
    id: NodeId,
    /// Whether this is the document, an element, or text, plus its payload.
    data: NodeData<T>,

    /// The parent node. The connected root element points to slot zero.
    pub(crate) parent: Option<NodeId>,
    /// Child nodes, in document order.
    pub(crate) children: Vec<NodeId>,
    /// The element's local name, interned as a stylo [`LocalName`] atom so
    /// `selectors::Element::has_local_name` is a cheap atom comparison.
    /// `None` for document and text nodes.
    pub(crate) local_name: Option<LocalName>,
    /// The node's classes, interned as atoms.
    pub(crate) classes: SmallVec<[Atom; 4]>,
    /// The node's `id` selector value, distinct from a plain `id` attribute
    /// (the embedder decides whether/how the two are linked).
    pub(crate) id_attr: Option<Atom>,
    /// Plain attributes, keyed by their interned local names. Synthetic /
    /// reflected attributes beyond this map are served by the
    /// [`ext`](Node::ext) payload's
    /// [`extra_attr_value`](crate::ExternalState::extra_attr_value) hook.
    pub(crate) attrs: FxHashMap<LocalName, String>,
    /// Active dynamic pseudo-classes (`:hover` / `:active` / `:focus`) as
    /// stylo state bits.
    pub(crate) element_state: ElementState,

    /// The node's parsed inline style block (the `style` attribute), locked
    /// under the document's [`SharedRwLock`](stylo::shared_lock::SharedRwLock).
    /// `None` when no inline style is set.
    pub(crate) inline_block: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    /// This node's matching-relevant state before its first mutation since
    /// the last style flush. Boxed so the common no-snapshot case costs one
    /// word in every node; drained into stylo's temporary `SnapshotMap` at
    /// the start of a flush.
    pub(crate) snapshot: Option<Box<Snapshot>>,

    /// stylo's per-element style data (`ElementData`), created lazily via
    /// `TElement::ensure_data`. The resolved computed style lives here (see
    /// [`computed_style`](Node::computed_style)). It remains empty for text
    /// nodes and is only touched through the [`traits`](crate::traits) impls
    /// under stylo's traversal discipline.
    pub(crate) stylo_data: UnsafeCell<Option<ElementDataWrapper>>,

    /// Selector flags accumulated by stylo during matching (e.g. "has a
    /// child-position-dependent rule"), stored as the raw
    /// [`ElementSelectorFlags`] bits. Atomic because parallel workers matching
    /// sibling nodes may both push `for_parent()` flags onto the shared
    /// parent.
    pub(crate) selector_flags: AtomicUsize,

    /// Whether some descendant of this node has pending style work. This is
    /// the bit stylo's traversal walks down
    /// ([`TElement::has_dirty_descendants`](stylo::dom::TElement::has_dirty_descendants)).
    /// The node's *own* dirtiness carries no separate flag — see
    /// [`is_style_dirty`](Node::is_style_dirty), which derives it from stylo's
    /// scheduling state.
    dirty_descendants: AtomicBool,

    /// Snapshot lifecycle bits ([`SNAPSHOT_PRESENT`] / [`SNAPSHOT_HANDLED`]),
    /// mirroring `TElement::{has_snapshot, handled_snapshot}`.
    snapshot_flags: AtomicU8,

    /// Bottom-up traversal bookkeeping
    /// (`TElement::{store_children_to_process, did_process_child}`). Unused
    /// while the style traversal has no postorder pass, but kept sound for
    /// when one appears.
    pub(crate) children_to_process: AtomicIsize,

    /// Debug-only access guard for the `stylo_data` slot (see
    /// [`slot_guard`]).
    #[cfg(debug_assertions)]
    pub(crate) slot_guard: slot_guard::SlotGuard,

    /// Literal character data. Always `Some` for a text node. Element nodes
    /// may also carry data for embedder-defined element-backed text carriers
    /// (Lynx's `<raw-text>` is one); ordinary W3C text uses a child text node.
    pub(crate) text: Option<String>,

    /// This node's layout state (measurement cache, unrounded and
    /// device-snapped layouts, out-of-flow bookkeeping) — created and dropped
    /// with the node, so tree mutation can never leave layout state to
    /// synchronize (see [`crate::layout`]).
    ///
    /// An `AtomicRefCell` (the Servo per-node layout-data shape): keeps the
    /// node shareable for stylo's parallel restyle traversal while the
    /// single-threaded, post-style layout pass writes through `&Node` handles
    /// in short scoped borrows.
    pub(crate) layout_data: AtomicRefCell<LayoutData>,
}

impl<T> Node<T> {
    /// Create the slot-zero document node.
    pub(crate) fn new_document(
        tree: *mut Slab<Node<T>>,
        lock: StdArc<SharedRwLock>,
        url_data: UrlExtraData,
    ) -> Self {
        Self::new(
            tree,
            DOCUMENT_NODE_ID,
            NodeData::Document {
                lock,
                url_data,
                #[cfg(debug_assertions)]
                in_flush: AtomicBool::new(false),
            },
            None,
            None,
        )
    }

    /// Create a detached element bound to its owning document slab.
    /// Crate-only: embedders go through
    /// [`Document::create_element`](crate::Document::create_element).
    pub(crate) fn new_element(
        tree: *mut Slab<Node<T>>,
        id: NodeId,
        local_name: LocalName,
        ext: T,
    ) -> Self {
        Self::new(tree, id, NodeData::Element(ext), Some(local_name), None)
    }

    /// Create a detached text node bound to its owning document slab.
    /// Crate-only: embedders go through
    /// [`Document::create_text_node`](crate::Document::create_text_node).
    pub(crate) fn new_text(tree: *mut Slab<Node<T>>, id: NodeId, text: String, ext: T) -> Self {
        Self::new(tree, id, NodeData::Text(ext), None, Some(text))
    }

    fn new(
        tree: *mut Slab<Node<T>>,
        id: NodeId,
        data: NodeData<T>,
        local_name: Option<LocalName>,
        text: Option<String>,
    ) -> Self {
        Self {
            tree: AtomicPtr::new(tree),
            id,
            data,
            parent: None,
            children: Vec::new(),
            local_name,
            classes: SmallVec::new(),
            id_attr: None,
            attrs: FxHashMap::default(),
            element_state: ElementState::empty(),
            inline_block: None,
            snapshot: None,
            stylo_data: UnsafeCell::new(None),
            selector_flags: AtomicUsize::new(0),
            dirty_descendants: AtomicBool::new(false),
            snapshot_flags: AtomicU8::new(0),
            children_to_process: AtomicIsize::new(0),
            #[cfg(debug_assertions)]
            slot_guard: slot_guard::SlotGuard::new(),
            text,
            layout_data: AtomicRefCell::new(LayoutData::default()),
        }
    }

    /// Borrow the owning document's fixed-address slab through the backpointer.
    ///
    /// # Safety discipline (crate-internal)
    ///
    /// Callable only from shared-borrow contexts (`&Node` navigation and
    /// the stylo trait impls), where the `&self` was itself derived from the
    /// slab. The boxed slab outlives every node and mutation requires
    /// `&mut Document`, so no mutable slab borrow can coexist.
    pub(crate) fn tree(&self) -> &Slab<Node<T>> {
        // SAFETY: the private `Document` field keeps the boxed slab at this
        // address until after every node is dropped; see the method contract.
        #[expect(unsafe_code, reason = "deref the owning slab backpointer")]
        unsafe {
            &*self.tree.load(Ordering::Relaxed)
        }
    }

    /// The owner document node at the slab's fixed slot zero.
    pub(crate) fn owner_document(&self) -> &Node<T> {
        self.tree()
            .get(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
    }

    /// The owner document's style lock.
    pub(crate) fn document_lock(&self) -> &StdArc<SharedRwLock> {
        match &self.owner_document().data {
            NodeData::Document { lock, .. } => lock,
            _ => unreachable!("slot zero must contain the document node"),
        }
    }

    /// The owner document's base URL data.
    pub(crate) fn document_url_data(&self) -> &UrlExtraData {
        match &self.owner_document().data {
            NodeData::Document { url_data, .. } => url_data,
            _ => unreachable!("slot zero must contain the document node"),
        }
    }

    /// The owner document's traversal-phase flag.
    #[cfg(debug_assertions)]
    pub(crate) fn flush_flag(&self) -> &AtomicBool {
        match &self.owner_document().data {
            NodeData::Document { in_flush, .. } => in_flush,
            _ => unreachable!("slot zero must contain the document node"),
        }
    }

    /// Whether the owner document is inside a style traversal.
    #[cfg(debug_assertions)]
    pub(crate) fn in_flush(&self) -> bool {
        self.flush_flag().load(Ordering::Relaxed)
    }

    // --- identity & DOM reads ------------------------------------------------

    /// This node's handle in its document.
    #[must_use]
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// This node's DOM kind.
    #[must_use]
    pub fn node_type(&self) -> NodeType {
        match &self.data {
            NodeData::Document { .. } => NodeType::Document,
            NodeData::Element(_) => NodeType::Element,
            NodeData::Text(_) => NodeType::Text,
        }
    }

    /// Whether this is the document node.
    #[must_use]
    pub fn is_document(&self) -> bool {
        matches!(&self.data, NodeData::Document { .. })
    }

    /// Whether this is an element node.
    #[must_use]
    pub fn is_element(&self) -> bool {
        matches!(&self.data, NodeData::Element(_))
    }

    /// Whether this is a text node.
    #[must_use]
    pub fn is_text_node(&self) -> bool {
        matches!(&self.data, NodeData::Text(_))
    }

    /// The parent node's handle, or `None` for the document/detached nodes.
    #[must_use]
    pub fn parent_id(&self) -> Option<NodeId> {
        self.parent
    }

    /// The children's handles, in document order.
    #[must_use]
    pub fn child_ids(&self) -> &[NodeId] {
        &self.children
    }

    /// The element's interned local name, or `None` for a non-element node.
    #[must_use]
    pub fn local_name(&self) -> Option<&LocalName> {
        self.local_name.as_ref()
    }

    /// The element's tag name as a string, or `None` for a non-element node.
    #[must_use]
    pub fn tag(&self) -> Option<&str> {
        self.local_name().map(|name| name.0.as_ref())
    }

    /// The node's `id` selector value.
    #[must_use]
    pub fn id_attr(&self) -> Option<&str> {
        self.id_attr.as_deref()
    }

    /// Whether `class` is in the node's class list (case-sensitive).
    #[must_use]
    pub fn has_class(&self, class: &str) -> bool {
        self.classes
            .iter()
            .any(|existing| existing.as_ref() == class)
    }

    /// The node's classes, in authored order.
    pub fn classes(&self) -> impl ExactSizeIterator<Item = &str> {
        self.classes.iter().map(AsRef::as_ref)
    }

    /// A plain attribute's value, addressed by its local name.
    #[must_use]
    pub fn attr(&self, name: &LocalName) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }

    /// The plain attribute map.
    #[must_use]
    pub fn attrs(&self) -> &FxHashMap<LocalName, String> {
        &self.attrs
    }

    /// The active dynamic pseudo-class state bits.
    #[must_use]
    pub fn element_state(&self) -> ElementState {
        self.element_state
    }

    /// The literal character data, if this is a text node or an
    /// embedder-defined element-backed text carrier.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// The embedder's external-state payload.
    ///
    /// # Panics
    ///
    /// Panics when called on the document node, which deliberately has no
    /// embedder payload.
    #[must_use]
    pub fn ext(&self) -> &T {
        match &self.data {
            NodeData::Element(ext) | NodeData::Text(ext) => ext,
            NodeData::Document { .. } => panic!("the document node has no external payload"),
        }
    }

    /// Payload mutation is `Document`-mediated
    /// ([`Document::ext_mut`](crate::Document::ext_mut)) so synthetic-attribute
    /// snapshots can be demanded contractually alongside it.
    pub(crate) fn ext_mut(&mut self) -> &mut T {
        match &mut self.data {
            NodeData::Element(ext) | NodeData::Text(ext) => ext,
            NodeData::Document { .. } => panic!("the document node has no external payload"),
        }
    }

    // --- style reads ----------------------------------------------------------

    /// Whether stylo has ever created element style data here.
    ///
    /// Always `false` for text nodes, which do not enter the cascade.
    ///
    /// Must not be called while a style flush is running on the node's
    /// document (impossible through the public API: a flush holds
    /// `&mut Document`).
    #[must_use]
    pub fn has_style_data(&self) -> bool {
        // SAFETY: reads only the `Option` discriminant; no flush is running
        // (flushes require `&mut Document`, we hold `&self` from it).
        #[expect(unsafe_code, reason = "UnsafeCell discriminant read outside any flush")]
        unsafe {
            (*self.stylo_data.get()).is_some()
        }
    }

    /// The resolved computed style, if this element has been styled.
    ///
    /// Text nodes return `None`. The style lives in stylo's per-element
    /// `ElementData`; this clones the `Arc` out of it. Must not be called while
    /// a style flush is running on the node's document (impossible through the
    /// public API: a flush holds `&mut Document`).
    #[must_use]
    pub fn computed_style(&self) -> Option<Arc<ComputedValues>> {
        // SAFETY: no flush is running (flushes require `&mut Document`, and
        // we hold `&self` borrowed from it), so reading the slot and taking a
        // shared borrow of the wrapper cannot race.
        #[expect(unsafe_code, reason = "UnsafeCell read outside any flush")]
        let slot = unsafe { (*self.stylo_data.get()).as_ref() };
        slot.and_then(|wrapper| wrapper.borrow().styles.primary.clone())
    }

    /// Store a resolved computed style, creating the stylo `ElementData` slot
    /// if needed. Used by the standalone
    /// [`StyleEngine::resolve`](crate::StyleEngine::resolve) path; the flush
    /// traversal writes styles through stylo itself.
    pub(crate) fn set_computed_style(&mut self, style: Arc<ComputedValues>) {
        let slot = self.stylo_data.get_mut();
        let wrapper = slot.get_or_insert_with(ElementDataWrapper::default);
        wrapper.borrow_mut().styles.primary = Some(style);
    }

    // --- layout reads ---------------------------------------------------------

    /// The device-pixel-snapped [`Layout`] from the last layout pass
    /// ([`StyleEngine::layout_document`](crate::StyleEngine::layout_document)):
    /// the border box painting consumes, `location` relative to the parent's
    /// border box. All-zero when the node has never been laid out or is
    /// inside a `display: none` subtree.
    ///
    /// Must not be called while a layout pass is running on the document
    /// (impossible through the public API: layout holds `&mut Document`).
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout_data.borrow().rounded
    }

    /// The unrounded CSS-pixel [`Layout`] from the last layout pass — the
    /// values relayout derives from (rounded output is presentation, this is
    /// truth).
    #[must_use]
    pub fn unrounded_layout(&self) -> Layout {
        self.layout_data.borrow().unrounded
    }

    /// Whether this node's layout **measurement cache** currently holds no
    /// memoized answer — i.e. the next layout pass must recompute it rather
    /// than answer from cache.
    ///
    /// Observes the incremental-relayout invalidation state: after a
    /// [`Document::invalidate_layout`](crate::Document::invalidate_layout) the
    /// dirty spine reports `true` up to (and including) the nearest relayout
    /// boundary, while the boundary's ancestors and every clean subtree keep
    /// their caches and report `false`. A freshly laid-out (or never laid-out)
    /// node reports `false` / `true` respectively.
    ///
    /// Must not be called while a layout pass is running on the document
    /// (impossible through the public API: layout holds `&mut Document`).
    #[must_use]
    pub fn layout_cache_is_empty(&self) -> bool {
        self.layout_data.borrow().measure_cache.is_empty()
    }

    /// The accumulated stylo selector flags.
    #[must_use]
    pub fn selector_flags(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::from_bits_retain(self.selector_flags.load(Ordering::Relaxed))
    }

    /// Whether this node itself has pending style work, **derived from
    /// stylo's own scheduling state** rather than a separate embedder flag:
    ///
    /// - a node stylo has never styled (no [`ElementData`] slot) is always dirty — it needs a full
    ///   style computation the moment the traversal reaches it;
    /// - a styled node is dirty iff a pre-mutation snapshot is pending (`snapshot_present`) or
    ///   stylo has a queued
    ///   [`RestyleHint`](stylo::invalidation::element::restyle_hints::RestyleHint) for it
    ///   (`ElementData::hint` is non-empty).
    ///
    /// Dirtiness is an **element** concept: only element nodes enter the
    /// cascade. The document node at slot zero and text nodes never receive
    /// `ElementData`, so the "no data ⇒ dirty" rule above would perpetually
    /// mislabel them dirty; they are reported clean instead. That preserves the
    /// pre-derived contract (the document node's dirty bit defaulted clear) and
    /// scopes the derived rule to the only nodes the scheduler ever consults:
    /// [`Document::needs_flush`](crate::Document::needs_flush) reads the
    /// document element ([`root_element`](crate::Document::root_element)), and
    /// the flush roots the traversal there.
    ///
    /// This is a scheduling signal, not ground truth about the tree: the
    /// authoritative "does the tree need a flush" query is the document
    /// element's bits ([`Document::needs_flush`](crate::Document::needs_flush)
    /// — this OR [`has_dirty_descendants`](Self::has_dirty_descendants)).
    ///
    /// Must not be called while a style flush is running on the node's
    /// document (impossible through the public API: a flush holds
    /// `&mut Document`, this borrows `&self` from it) — the same
    /// no-concurrent-flush contract as [`computed_style`](Self::computed_style).
    ///
    /// [`ElementData`]: stylo::data::ElementData
    #[must_use]
    pub fn is_style_dirty(&self) -> bool {
        // Dirtiness is an element concept (see the method docs): the document
        // node and text nodes never enter the cascade, so they never carry
        // element style data and must not be reported dirty for lacking it.
        if !self.is_element() {
            return false;
        }
        // SAFETY: no flush is running (flushes require `&mut Document`, and
        // we hold `&self` borrowed from it), so reading the slot and taking a
        // shared borrow of the wrapper cannot race.
        #[expect(unsafe_code, reason = "UnsafeCell read outside any flush")]
        let slot = unsafe { (*self.stylo_data.get()).as_ref() };
        match slot {
            // Never styled: dirty until it is reached and styled.
            None => true,
            // Styled: dirty iff a snapshot is pending or a restyle hint is
            // queued.
            Some(wrapper) => self.snapshot_present() || !wrapper.borrow().hint.is_empty(),
        }
    }

    /// Whether a descendant has pending style work.
    #[must_use]
    pub fn has_dirty_descendants(&self) -> bool {
        self.dirty_descendants.load(Ordering::Relaxed)
    }

    // --- crate-internal bookkeeping -------------------------------------------

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
    /// Safe because it goes through `&mut self`: exclusive access to the node
    /// means no traversal is concurrently touching the `UnsafeCell`.
    pub(crate) fn stylo_data_mut(&mut self) -> Option<&mut ElementDataWrapper> {
        self.stylo_data.get_mut().as_mut()
    }

    /// Take ownership of the payload when the node is freed.
    pub(crate) fn into_ext(self) -> T {
        match self.data {
            NodeData::Element(ext) | NodeData::Text(ext) => ext,
            NodeData::Document { .. } => unreachable!("the document node is never removed"),
        }
    }

    /// Whether this element has no element children, non-empty text-node
    /// children, or element-backed character data.
    ///
    /// Empty text nodes do not affect `:empty`; non-empty text nodes (including
    /// whitespace-only data) do. Text nodes themselves are never selector
    /// subjects, so callers only use this for elements.
    pub(crate) fn is_empty_element(&self) -> bool {
        debug_assert!(self.is_element(), "`:empty` is only defined for elements");
        self.text.as_ref().is_none_or(String::is_empty)
            && self.children.iter().all(|&id| {
                let child = self
                    .tree()
                    .get(id)
                    .expect("internal tree links always resolve");
                !child.is_element()
                    && (!child.is_text_node() || child.text.as_ref().is_none_or(String::is_empty))
            })
    }
}

// --- tree navigation ----------------------------------------------------------
//
// Navigation lives directly on `Node` (resolving ids through the document
// backpointer), so a plain `&'a Node<T>` is the crate's only read handle —
// the same one-word value stylo's element traits are implemented on (see
// the crate-private `traits` module; stylo's style-sharing cache sizes its
// TLS for a word-sized `TElement` handle). References being `Copy` is what
// the traversal relies on; no wrapper type is needed.
//
// The trait impls on `&Node` reuse these method names (`first_child`,
// `next_sibling`, …) and delegate to them **fully qualified**
// (`Node::first_child(node)`): with a stylo trait in scope, method-call
// syntax on a `&Node` receiver would resolve to the trait impl first.
impl<T> Node<T> {
    /// The parent node, if any.
    ///
    /// # Panics
    ///
    /// Panics only if an internal parent link is dangling.
    #[must_use]
    pub fn parent(&self) -> Option<&Node<T>> {
        self.parent.map(|id| {
            self.tree()
                .get(id)
                .expect("internal tree links always resolve")
        })
    }

    /// The first child node, if any.
    ///
    /// # Panics
    ///
    /// Panics only if an internal child link is dangling.
    #[must_use]
    pub fn first_child(&self) -> Option<&Node<T>> {
        self.children.first().map(|&id| {
            self.tree()
                .get(id)
                .expect("internal tree links always resolve")
        })
    }

    /// The last child node, if any.
    ///
    /// # Panics
    ///
    /// Panics only if an internal child link is dangling.
    #[must_use]
    pub fn last_child(&self) -> Option<&Node<T>> {
        self.children.last().map(|&id| {
            self.tree()
                .get(id)
                .expect("internal tree links always resolve")
        })
    }

    /// The next sibling node, if any.
    #[must_use]
    pub fn next_sibling(&self) -> Option<&Node<T>> {
        self.sibling_at(1)
    }

    /// The previous sibling node, if any.
    #[must_use]
    pub fn prev_sibling(&self) -> Option<&Node<T>> {
        self.sibling_at(-1)
    }

    fn sibling_at(&self, offset: isize) -> Option<&Node<T>> {
        let tree = self.tree();
        let siblings = &tree
            .get(self.parent?)
            .expect("internal tree links always resolve")
            .children;
        let pos = siblings
            .iter()
            .position(|&c| c == self.id)
            .expect("node must appear in its parent's child list");
        let sibling = *siblings.get(pos.checked_add_signed(offset)?)?;
        Some(
            tree.get(sibling)
                .expect("internal tree links always resolve"),
        )
    }

    /// Iterate over the node's children in document order.
    #[must_use]
    pub fn children(&self) -> ChildrenIter<'_, T> {
        ChildrenIter {
            tree: self.tree(),
            children: &self.children,
            index: 0,
        }
    }
}

// `stylo_data` (an `UnsafeCell`) and the `ext` payload are deliberately
// omitted: the former is not `Debug` (and reading it would need the
// no-concurrent-flush invariant — stylo debug-prints nodes *during* the
// traversal, which also rules out the derived `is_style_dirty` here), and
// printing the latter would demand a `T: Debug` bound this impl cannot carry
// — stylo's `TNode`/`TElement` require `&Node<T>: Debug` for every payload
// type.
impl<T> fmt::Debug for Node<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id)
            .field("node_type", &self.node_type())
            .field("tag", &self.tag())
            .field("text", &self.text)
            .field("classes", &self.classes)
            .field("id_attr", &self.id_attr)
            .field("element_state", &self.element_state)
            .field("has_inline_block", &self.inline_block.is_some())
            .field("dirty_descendants", &self.has_dirty_descendants())
            .field("children", &self.children)
            .finish_non_exhaustive()
    }
}

/// Node equality is **identity**: two nodes are equal exactly when they are
/// the same node (compared by address, which is stable for the lifetime of
/// any borrow — mutation, and thus node motion, needs `&mut Document`).
///
/// A DOM node is an entity, not a value; this is also what stylo's
/// `TElement`/`TNode` bounds (`Eq`/`Hash` on the `&Node` handle) require of
/// element identity. `Hash` matches: it hashes the address.
impl<T> PartialEq for Node<T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl<T> Eq for Node<T> {}

impl<T> std::hash::Hash for Node<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::from_ref(self).hash(state);
    }
}

/// The children iterator ([`Node::children`]); also what stylo's restyle
/// traversal walks.
pub struct ChildrenIter<'a, T> {
    tree: &'a Slab<Node<T>>,
    children: &'a [NodeId],
    index: usize,
}

impl<T> fmt::Debug for ChildrenIter<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildrenIter")
            .field("children", &self.children)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

impl<'a, T> Iterator for ChildrenIter<'a, T> {
    type Item = &'a Node<T>;

    fn next(&mut self) -> Option<&'a Node<T>> {
        let id = *self.children.get(self.index)?;
        self.index += 1;
        Some(
            self.tree
                .get(id)
                .expect("internal tree links always resolve"),
        )
    }
}
