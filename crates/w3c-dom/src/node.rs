//! [`Node`] — the unit the tree is composed of — and its `&Node` read/
//! navigation handle.
//!
//! A node models a strict subset of the W3C DOM. Slot zero is the document
//! node and owns the shared style context. Element nodes carry a tag, id,
//! classes, attributes, dynamic pseudo-class state, an inline style block,
//! and the per-element style bookkeeping Stylo needs; text nodes carry
//! character data. Element/text variants share tree links and an opaque
//! [`payload`](Node::payload) value that is not part of DOM or selector state.
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
//! Everything else (tag/classes/attrs/text/payload) is **immutable during a
//! flush**: mutation requires `&mut Document`, which
//! [`Document::flush_styles`](crate::Document::flush_styles) holds
//! exclusively for the whole traversal.

use std::cell::UnsafeCell;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc as StdArc, OnceLock};

use atomic_refcell::{AtomicRef, AtomicRefCell};
use dom::ElementState;
#[cfg(feature = "layout-test-utils")]
use neutron_star::compute::LeafMetrics;
use neutron_star::compute::NaturalSize;
use neutron_star::text::{ArtifactSlots, TextContext};
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
        /// One lazily-created, reusable Parley session for the whole
        /// document. Non-text documents never pay font-context setup cost;
        /// layout is single-threaded, while the atomic borrow cell preserves
        /// the node's thread-safe shape during parallel style traversal.
        text_context: Box<OnceLock<AtomicRefCell<TextContext>>>,
        #[cfg(debug_assertions)]
        in_flush: AtomicBool,
    },
    Element(T),
    Text(T),
}

/// Literal or replaced content carried only by nodes that have it.
///
/// Keeping this behind one nullable pointer reuses the storage that literal
/// text already required instead of enlarging every ordinary element for
/// natural sizes or retained Parley artifacts.
enum NodeContent {
    Text {
        value: String,
        /// Artifact slots are much larger than the string descriptor. Keep
        /// them behind a second lazy allocation so untouched text nodes pay
        /// only for the content record.
        artifacts: OnceLock<Box<AtomicRefCell<ArtifactSlots>>>,
    },
    Replaced(NaturalSize),
    #[cfg(feature = "layout-test-utils")]
    Test(LeafMetrics),
}

impl NodeContent {
    fn text(value: String) -> Self {
        Self::Text {
            value,
            artifacts: OnceLock::new(),
        }
    }
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
    /// Parsed reflection of the node's `id` DOM attribute.
    pub(crate) id_attr: Option<Atom>,
    /// DOM attributes, keyed by their interned local names. This is the
    /// complete source of selector-visible attribute state; opaque payloads
    /// cannot synthesize attributes.
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
    /// the internal bit stylo's traversal walks down
    /// ([`TElement::has_dirty_descendants`](stylo::dom::TElement::has_dirty_descendants));
    /// embedders cannot inspect or manipulate it.
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

    /// Mutually exclusive literal or replaced content. Literal data is always
    /// present for a text node; element nodes use the same nullable slot for
    /// an element-backed text carrier, natural size, or synthetic test data.
    /// Ordinary container elements therefore retain only one null pointer.
    content: Option<Box<NodeContent>>,

    /// This node's derived layout state (measurement cache, unrounded and
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
                text_context: Box::default(),
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
        payload: T,
    ) -> Self {
        Self::new(tree, id, NodeData::Element(payload), Some(local_name), None)
    }

    /// Create a detached text node bound to its owning document slab.
    /// Crate-only: embedders go through
    /// [`Document::create_text_node`](crate::Document::create_text_node).
    pub(crate) fn new_text(tree: *mut Slab<Node<T>>, id: NodeId, text: String, payload: T) -> Self {
        Self::new(tree, id, NodeData::Text(payload), None, Some(text))
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
            content: text.map(|value| Box::new(NodeContent::text(value))),
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

    /// The owner document's reusable Parley text session.
    pub(crate) fn text_context(&self) -> &AtomicRefCell<TextContext> {
        match &self.owner_document().data {
            NodeData::Document { text_context, .. } => {
                text_context.get_or_init(|| AtomicRefCell::new(TextContext::new()))
            }
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

    /// A DOM attribute's value, addressed by its authored name.
    ///
    /// Attribute names are converted to stylo's interned [`LocalName`] at
    /// this DOM boundary; embedders do not need to traffic in stylo types.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        let name = LocalName::from(name);
        self.attr_local_name(&name)
    }

    /// Look up an already-interned attribute name on stylo's matching paths.
    pub(crate) fn attr_local_name(&self, name: &LocalName) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }

    /// The complete DOM attribute map as string name/value pairs, including
    /// reflected `id`, `class`, and `style` attributes.
    pub fn attrs(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.attrs
            .iter()
            .map(|(name, value)| (name.0.as_ref(), value.as_str()))
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
        match self.content.as_deref() {
            Some(NodeContent::Text { value, .. }) => Some(value),
            _ => None,
        }
    }

    /// The node's opaque payload.
    ///
    /// # Panics
    ///
    /// Panics when called on the document node, which deliberately has no
    /// payload.
    #[must_use]
    pub fn payload(&self) -> &T {
        match &self.data {
            NodeData::Element(payload) | NodeData::Text(payload) => payload,
            NodeData::Document { .. } => panic!("the document node has no payload"),
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
    pub(crate) fn has_style_data(&self) -> bool {
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

    // --- layout reads ---------------------------------------------------------

    /// A borrowed view of the device-pixel-snapped [`Layout`] from the last layout pass
    /// ([`Document::layout`](crate::Document::layout)):
    /// the border box painting consumes, `location` relative to the parent's
    /// border box. All-zero when the node has never been laid out or is
    /// inside a `display: none` subtree.
    ///
    /// The returned guard keeps this node's layout slot shared-borrowed; copy
    /// out individual fields as needed and do not retain it across code that
    /// may mutate layout. Clone the dereferenced [`Layout`] explicitly only
    /// when an owned whole-record snapshot is required.
    ///
    /// Must not be called while a layout pass is running on the document
    /// (impossible through the public API: layout holds `&mut Document`).
    #[must_use]
    pub fn layout(&self) -> impl std::ops::Deref<Target = Layout> + '_ {
        AtomicRef::map(self.layout_data.borrow(), |data| &data.rounded)
    }

    /// A borrowed view of the unrounded CSS-pixel [`Layout`] from the last layout pass — the
    /// values relayout derives from (rounded output is presentation, this is
    /// truth). As with [`layout`](Self::layout), clone the dereferenced record
    /// explicitly only when an owned snapshot is required.
    #[must_use]
    pub fn unrounded_layout(&self) -> impl std::ops::Deref<Target = Layout> + '_ {
        AtomicRef::map(self.layout_data.borrow(), |data| &data.unrounded)
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

    /// The decoded intrinsic dimensions/ratio used when this node lays out as
    /// replaced content.
    #[must_use]
    pub(crate) fn natural_size(&self) -> NaturalSize {
        match self.content.as_deref() {
            Some(NodeContent::Replaced(natural_size)) => *natural_size,
            _ => NaturalSize::NONE,
        }
    }

    pub(crate) fn set_natural_size(&mut self, natural_size: NaturalSize) -> bool {
        if self.natural_size() == natural_size {
            return false;
        }
        self.content = (natural_size != NaturalSize::NONE)
            .then(|| Box::new(NodeContent::Replaced(natural_size)));
        true
    }

    pub(crate) fn text_artifacts(&self) -> &AtomicRefCell<ArtifactSlots> {
        match self.content.as_deref() {
            Some(NodeContent::Text { artifacts, .. }) => artifacts
                .get_or_init(|| Box::new(AtomicRefCell::new(ArtifactSlots::default())))
                .as_ref(),
            _ => unreachable!("only literal-text content has Parley artifacts"),
        }
    }

    pub(crate) fn invalidate_text_artifacts(&self) {
        if let Some(NodeContent::Text { artifacts, .. }) = self.content.as_deref()
            && let Some(artifacts) = artifacts.get()
        {
            artifacts.borrow_mut().invalidate();
        }
    }

    #[cfg(feature = "layout-test-utils")]
    pub(crate) fn test_leaf_metrics(&self) -> Option<LeafMetrics> {
        match self.content.as_deref() {
            Some(NodeContent::Test(metrics)) => Some(*metrics),
            _ => None,
        }
    }

    #[cfg(feature = "layout-test-utils")]
    pub(crate) fn set_test_leaf_metrics(&mut self, metrics: LeafMetrics) {
        self.content = Some(Box::new(NodeContent::Test(metrics)));
    }

    pub(crate) fn set_literal_text(&mut self, text: Option<String>) {
        self.content = text.map(|value| Box::new(NodeContent::text(value)));
    }

    /// The accumulated stylo selector flags.
    pub(crate) fn selector_flags(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::from_bits_retain(self.selector_flags.load(Ordering::Relaxed))
    }

    /// Whether a descendant has pending style work.
    pub(crate) fn has_dirty_descendants(&self) -> bool {
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
    pub(crate) fn into_payload(self) -> T {
        match self.data {
            NodeData::Element(payload) | NodeData::Text(payload) => payload,
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
        self.text().is_none_or(str::is_empty)
            && self.children.iter().all(|&id| {
                let child = self
                    .tree()
                    .get(id)
                    .expect("internal tree links always resolve");
                !child.is_element()
                    && (!child.is_text_node() || child.text().is_none_or(str::is_empty))
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

// `stylo_data` (an `UnsafeCell`) and the opaque payload are deliberately
// omitted: the former is not `Debug` (and reading it would need the
// no-concurrent-flush invariant — stylo debug-prints nodes *during* the
// traversal), and
// printing the latter would demand a `T: Debug` bound this impl cannot carry
// — stylo's `TNode`/`TElement` require `&Node<T>: Debug` for every payload
// type.
impl<T> fmt::Debug for Node<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id)
            .field("node_type", &self.node_type())
            .field("tag", &self.tag())
            .field("text", &self.text())
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::Document;

    #[test]
    fn document_text_context_is_lazy_and_reused() {
        let document = Document::<()>::new();
        let root = document.root_node();
        let NodeData::Document { text_context, .. } = &root.data else {
            unreachable!("slot zero is the document node")
        };

        assert!(text_context.get().is_none());
        let first = root.text_context();
        assert!(text_context.get().is_some());
        assert!(std::ptr::eq(first, root.text_context()));
    }

    #[test]
    fn node_content_and_text_artifacts_are_lazy() {
        let mut document = Document::<()>::new();
        let element = document.create_element("view", ());
        assert!(document.get(element).unwrap().content.is_none());

        let text = document.create_text_node("hello", ());
        let text = document.get(text).unwrap();
        let Some(NodeContent::Text { artifacts, .. }) = text.content.as_deref() else {
            unreachable!("text nodes carry literal-text content")
        };
        assert!(artifacts.get().is_none());
        let first = text.text_artifacts();
        assert!(artifacts.get().is_some());
        assert!(std::ptr::eq(first, text.text_artifacts()));
    }
}
