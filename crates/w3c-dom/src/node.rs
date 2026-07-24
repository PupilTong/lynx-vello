//! [`Node`] — the unit the tree is composed of — and its `&Node` read/
//! navigation handle.

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, AtomicUsize, Ordering};

use dom::ElementState;
#[cfg(feature = "layout-test-utils")]
use neutron_star::compute::LeafMetrics;
use neutron_star::compute::NaturalSize;
use selectors::matching::ElementSelectorFlags;
use slab::Slab;
use smallvec::SmallVec;
use stylo::LocalName;
use stylo::data::{ElementDataRef, ElementDataWrapper};
use stylo::properties::{ComputedValues, PropertyDeclarationBlock};
use stylo::servo_arc::Arc;
use stylo::shared_lock::{Locked, SharedRwLock};
use stylo::stylesheets::UrlExtraData;
use stylo_atoms::Atom;

use crate::document::{DOCUMENT_NODE_ID, NodeId, PayloadSlot, TreeArenas, slab_get_for_live_node};

#[cfg(debug_assertions)]
pub(crate) mod slot_guard {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

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

pub(crate) const SNAPSHOT_PRESENT: u8 = 1 << 0;
pub(crate) const SNAPSHOT_HANDLED: u8 = 1 << 1;

static STALE_STYLE_MARKER: u8 = 0;

fn stale_layout_style_pointer() -> *mut ComputedValues {
    std::ptr::from_ref(&STALE_STYLE_MARKER)
        .cast::<ComputedValues>()
        .cast_mut()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeType {
    Document,
    Element,
    Text,
}

struct DocumentNodeData {
    lock: StdArc<SharedRwLock>,
    url_data: UrlExtraData,
    layout_styles_ready: AtomicBool,
    in_flush: AtomicBool,
}

enum NodeData {
    Document(Box<DocumentNodeData>),
    /// Stable pointer into the element's Stylo-owned primary
    /// `Arc<ComputedValues>`, published by each style traversal. A dangling
    /// sentinel fail-closes elements mutated outside that traversal until
    /// their own preorder callback publishes a new generation.
    Element(AtomicPtr<ComputedValues>),
    Text,
}

/// Stylo's per-node traversal and invalidation bookkeeping, stored in the
/// document's styling secondary arena under the owning node's [`NodeId`].
/// Snapshot payloads are sparse, document-owned state; only their atomic
/// traversal lifecycle flags remain here.
pub(crate) struct StylingData {
    pub(crate) selector_flags: AtomicUsize,
    pub(crate) dirty_descendants: AtomicBool,
    pub(crate) snapshot_flags: AtomicU8,
    pub(crate) children_to_process: AtomicIsize,
    #[cfg(debug_assertions)]
    pub(crate) slot_guard: slot_guard::SlotGuard,
}

impl Default for StylingData {
    fn default() -> Self {
        Self {
            selector_flags: AtomicUsize::new(0),
            dirty_descendants: AtomicBool::new(false),
            snapshot_flags: AtomicU8::new(0),
            children_to_process: AtomicIsize::new(0),
            #[cfg(debug_assertions)]
            slot_guard: slot_guard::SlotGuard::new(),
        }
    }
}

enum NodeContent {
    Text(String),
    Replaced(NaturalSize),
    #[cfg(feature = "layout-test-utils")]
    Test(LeafMetrics),
}

impl NodeContent {
    fn text(value: String) -> Self {
        Self::Text(value)
    }
}

/// A single node in a [`Document`](crate::Document) tree.
pub struct Node<T> {
    owner: AtomicPtr<TreeArenas<T>>,
    id: NodeId,
    data: NodeData,
    payload: PhantomData<T>,

    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    pub(crate) local_name: Option<LocalName>,
    pub(crate) classes: SmallVec<[Atom; 2]>,
    pub(crate) id_attribute: Option<Atom>,
    pub(crate) attrs: Vec<(LocalName, String)>,
    pub(crate) element_state: ElementState,

    pub(crate) inline_block: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    pub(crate) stylo_data: UnsafeCell<Option<ElementDataWrapper>>,

    content: Option<Box<NodeContent>>,
}

impl<T> Node<T> {
    pub(crate) fn new_document(
        owner: *mut TreeArenas<T>,
        lock: StdArc<SharedRwLock>,
        url_data: UrlExtraData,
    ) -> Self {
        Self::new(
            owner,
            DOCUMENT_NODE_ID,
            NodeData::Document(Box::new(DocumentNodeData {
                lock,
                url_data,
                layout_styles_ready: AtomicBool::new(true),
                in_flush: AtomicBool::new(false),
            })),
            None,
            None,
        )
    }

    pub(crate) fn new_element(
        owner: *mut TreeArenas<T>,
        id: NodeId,
        local_name: LocalName,
    ) -> Self {
        Self::new(
            owner,
            id,
            NodeData::Element(AtomicPtr::new(std::ptr::null_mut())),
            Some(local_name),
            None,
        )
    }

    pub(crate) fn new_text(owner: *mut TreeArenas<T>, id: NodeId, text: String) -> Self {
        Self::new(owner, id, NodeData::Text, None, Some(text))
    }

    fn new(
        owner: *mut TreeArenas<T>,
        id: NodeId,
        data: NodeData,
        local_name: Option<LocalName>,
        text: Option<String>,
    ) -> Self {
        Self {
            owner: AtomicPtr::new(owner),
            id,
            data,
            payload: PhantomData,
            parent: None,
            children: Vec::new(),
            local_name,
            classes: SmallVec::new(),
            id_attribute: None,
            attrs: Vec::new(),
            element_state: ElementState::empty(),
            inline_block: None,
            stylo_data: UnsafeCell::new(None),
            content: text.map(|value| Box::new(NodeContent::text(value))),
        }
    }

    pub(crate) fn arenas(&self) -> &TreeArenas<T> {
        #[expect(unsafe_code, reason = "deref the owning arena-set backpointer")]
        unsafe {
            &*self.owner.load(Ordering::Relaxed)
        }
    }

    pub(crate) fn tree(&self) -> &slab::Slab<Node<T>> {
        &self.arenas().nodes
    }

    #[inline]
    pub(crate) fn styling_data(&self) -> &StylingData {
        slab_get_for_live_node(&self.arenas().styling, self.id)
    }

    pub(crate) fn owner_document(&self) -> &Node<T> {
        self.tree()
            .get(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
    }

    fn document_data(&self) -> &DocumentNodeData {
        let node = if self.is_document() {
            self
        } else {
            self.owner_document()
        };
        let NodeData::Document(document) = &node.data else {
            unreachable!("slot zero must contain the document node")
        };
        document
    }

    pub(crate) fn document_lock(&self) -> &StdArc<SharedRwLock> {
        &self.document_data().lock
    }

    pub(crate) fn document_url_data(&self) -> &UrlExtraData {
        &self.document_data().url_data
    }

    pub(crate) fn set_layout_styles_ready(&self, ready: bool) {
        self.document_data()
            .layout_styles_ready
            .store(ready, Ordering::Release);
    }

    fn layout_styles_ready(&self) -> bool {
        self.document_data()
            .layout_styles_ready
            .load(Ordering::Acquire)
    }

    pub(crate) fn flush_flag(&self) -> &AtomicBool {
        &self.document_data().in_flush
    }

    pub(crate) fn in_flush(&self) -> bool {
        self.flush_flag().load(Ordering::Acquire)
    }

    #[must_use]
    pub fn id(&self) -> NodeId {
        self.id
    }

    #[must_use]
    pub fn node_type(&self) -> NodeType {
        match &self.data {
            NodeData::Document(_) => NodeType::Document,
            NodeData::Element(_) => NodeType::Element,
            NodeData::Text => NodeType::Text,
        }
    }

    #[must_use]
    pub fn is_document(&self) -> bool {
        matches!(&self.data, NodeData::Document(_))
    }

    #[must_use]
    pub fn is_element(&self) -> bool {
        matches!(&self.data, NodeData::Element(_))
    }

    #[must_use]
    pub fn is_text_node(&self) -> bool {
        matches!(&self.data, NodeData::Text)
    }

    #[must_use]
    pub fn parent_id(&self) -> Option<NodeId> {
        self.parent
    }

    #[must_use]
    pub fn child_ids(&self) -> &[NodeId] {
        &self.children
    }

    #[must_use]
    pub fn local_name(&self) -> Option<&LocalName> {
        self.local_name.as_ref()
    }

    #[must_use]
    pub fn tag_name(&self) -> Option<&str> {
        self.local_name().map(|name| name.0.as_ref())
    }

    #[must_use]
    pub fn id_attribute(&self) -> Option<&str> {
        self.id_attribute.as_deref()
    }

    #[must_use]
    pub fn has_class(&self, class: &str) -> bool {
        self.classes
            .iter()
            .any(|existing| existing.as_ref() == class)
    }

    pub fn classes(&self) -> impl ExactSizeIterator<Item = &str> {
        self.classes.iter().map(AsRef::as_ref)
    }

    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&str> {
        let name = LocalName::from(name);
        self.attr_local_name(&name)
    }

    pub(crate) fn attr_local_name(&self, name: &LocalName) -> Option<&str> {
        self.attrs
            .iter()
            .find_map(|(candidate, value)| (candidate == name).then_some(value.as_str()))
    }

    pub(crate) fn set_attr_local_name(&mut self, name: LocalName, value: String) {
        if let Some((_, current)) = self
            .attrs
            .iter_mut()
            .find(|(candidate, _)| *candidate == name)
        {
            *current = value;
        } else {
            self.attrs.push((name, value));
        }
    }

    pub(crate) fn remove_attr_local_name(&mut self, name: &LocalName) {
        if let Some(index) = self
            .attrs
            .iter()
            .position(|(candidate, _)| candidate == name)
        {
            self.attrs.remove(index);
        }
    }

    pub fn attributes(&self) -> impl ExactSizeIterator<Item = (&str, &str)> {
        self.attrs
            .iter()
            .map(|(name, value)| (name.0.as_ref(), value.as_str()))
    }

    #[must_use]
    pub fn element_state(&self) -> ElementState {
        self.element_state
    }

    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self.content.as_deref() {
            Some(NodeContent::Text(value)) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn payload(&self) -> &T {
        match &self.data {
            NodeData::Element(_) | NodeData::Text => {
                match slab_get_for_live_node(&self.arenas().payloads, self.id) {
                    PayloadSlot::Node(payload) => payload,
                    PayloadSlot::Document => {
                        unreachable!("document payload sentinel is only at slot zero")
                    }
                }
            }
            NodeData::Document(_) => panic!("the document node has no payload"),
        }
    }

    pub(crate) fn has_style_data(&self) -> bool {
        #[expect(unsafe_code, reason = "UnsafeCell discriminant read outside any flush")]
        unsafe {
            (*self.stylo_data.get()).is_some()
        }
    }

    pub(crate) fn needs_style_flush(&self) -> bool {
        let styling = self.styling_data();
        if styling.dirty_descendants.load(Ordering::Relaxed)
            || styling.snapshot_flags.load(Ordering::Relaxed) & SNAPSHOT_PRESENT != 0
        {
            return true;
        }
        #[expect(unsafe_code, reason = "ElementData is only read outside a style flush")]
        unsafe {
            (*self.stylo_data.get())
                .as_ref()
                .is_none_or(|data| !data.borrow().hint.is_empty())
        }
    }

    #[must_use]
    pub fn computed_style(&self) -> Option<Arc<ComputedValues>> {
        self.borrow_computed_style()
            .and_then(|data| data.styles.primary.clone())
    }

    pub(crate) fn borrow_computed_style(&self) -> Option<ElementDataRef<'_>> {
        #[expect(unsafe_code, reason = "UnsafeCell read outside any flush")]
        let slot = unsafe { (*self.stylo_data.get()).as_ref() };
        let data = slot?.borrow();
        data.styles.primary.as_ref()?;
        Some(data)
    }

    /// Publish the layout-only pointer after Stylo has finished mutating this
    /// element's data in its preorder callback.
    ///
    /// The pointee is owned by the primary style's `Arc`. A later traversal
    /// may replace that `Arc`, but `Document` requires exclusive access for
    /// both traversal and layout, and publishes this pointer before layout can
    /// observe the new generation.
    pub(crate) fn set_layout_style_pointer(&self, style: *mut ComputedValues) {
        let NodeData::Element(pointer) = &self.data else {
            debug_assert!(style.is_null());
            return;
        };
        pointer.store(style, Ordering::Relaxed);
    }

    pub(crate) fn mark_layout_style_stale(&self) {
        let NodeData::Element(pointer) = &self.data else {
            return;
        };
        pointer.store(stale_layout_style_pointer(), Ordering::Relaxed);
    }

    /// Borrow the post-flush computed style without re-entering Stylo's
    /// runtime borrow checker or incrementing the style `Arc`.
    pub(crate) fn layout_computed_style(&self) -> Option<&ComputedValues> {
        assert!(
            self.layout_styles_ready(),
            "computed styles are unavailable because the preceding style traversal did not complete"
        );
        let NodeData::Element(pointer) = &self.data else {
            return None;
        };
        let pointer = pointer.load(Ordering::Relaxed);
        assert!(
            !std::ptr::addr_eq(pointer, stale_layout_style_pointer()),
            "computed style for this element was mutated outside the completed style traversal"
        );
        #[expect(
            unsafe_code,
            reason = "the pointer is refreshed under Document's exclusive style/layout phase boundary and remains Arc-owned until the next exclusive traversal"
        )]
        unsafe {
            pointer.as_ref()
        }
    }

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

    pub(crate) fn selector_flags(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::from_bits_retain(
            self.styling_data().selector_flags.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn has_dirty_descendants(&self) -> bool {
        self.styling_data()
            .dirty_descendants
            .load(Ordering::Relaxed)
    }

    pub(crate) fn set_dirty_descendants_bit(&self, dirty: bool) {
        self.styling_data()
            .dirty_descendants
            .store(dirty, Ordering::Relaxed);
    }

    pub(crate) fn snapshot_present(&self) -> bool {
        self.snapshot_flags() & SNAPSHOT_PRESENT != 0
    }

    pub(crate) fn snapshot_handled(&self) -> bool {
        self.snapshot_flags() & SNAPSHOT_HANDLED != 0
    }

    pub(crate) fn snapshot_flags(&self) -> u8 {
        self.styling_data().snapshot_flags.load(Ordering::Relaxed)
    }

    pub(crate) fn set_snapshot_present(&self) {
        self.styling_data()
            .snapshot_flags
            .fetch_or(SNAPSHOT_PRESENT, Ordering::Relaxed);
    }

    pub(crate) fn set_snapshot_handled(&self) {
        self.styling_data()
            .snapshot_flags
            .fetch_or(SNAPSHOT_HANDLED, Ordering::Relaxed);
    }

    pub(crate) fn clear_snapshot_flags(&self) {
        self.styling_data()
            .snapshot_flags
            .store(0, Ordering::Relaxed);
    }

    pub(crate) fn stylo_data_mut(&mut self) -> Option<&mut ElementDataWrapper> {
        self.stylo_data.get_mut().as_mut()
    }

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

impl<T> Node<T> {
    #[must_use]
    pub fn parent(&self) -> Option<&Node<T>> {
        self.parent.map(|id| {
            self.tree()
                .get(id)
                .expect("internal tree links always resolve")
        })
    }

    #[must_use]
    pub fn first_child(&self) -> Option<&Node<T>> {
        self.children.first().map(|&id| {
            self.tree()
                .get(id)
                .expect("internal tree links always resolve")
        })
    }

    #[must_use]
    pub fn last_child(&self) -> Option<&Node<T>> {
        self.children.last().map(|&id| {
            self.tree()
                .get(id)
                .expect("internal tree links always resolve")
        })
    }

    #[must_use]
    pub fn next_sibling(&self) -> Option<&Node<T>> {
        self.sibling_at(1)
    }

    #[must_use]
    pub fn previous_sibling(&self) -> Option<&Node<T>> {
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

    #[must_use]
    pub fn children(&self) -> ChildrenIter<'_, T> {
        ChildrenIter {
            tree: self.tree(),
            children: &self.children,
            index: 0,
        }
    }
}

impl<T> fmt::Debug for Node<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id)
            .field("node_type", &self.node_type())
            .field("tag", &self.tag_name())
            .field("text", &self.text())
            .field("classes", &self.classes)
            .field("id_attribute", &self.id_attribute)
            .field("element_state", &self.element_state)
            .field("has_inline_block", &self.inline_block.is_some())
            .field("dirty_descendants", &self.has_dirty_descendants())
            .field("children", &self.children)
            .finish_non_exhaustive()
    }
}

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

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.children.len() - self.index;
        (remaining, Some(remaining))
    }
}

impl<T> ExactSizeIterator for ChildrenIter<'_, T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::dom::TElement;

    use super::*;
    use crate::Document;

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn document_only_state_stays_out_of_the_primary_node_stride() {
        const PRE_BOXING_NODE_DATA_SIZE: usize = 32;
        const PRE_BOXING_NODE_STRIDE: usize = 408;
        const PRE_STATIC_SPLIT_NODE_STRIDE: usize = 368;

        assert_eq!(std::mem::size_of::<NodeData>(), 16);
        assert_eq!(std::mem::size_of::<Node<()>>(), 208);
        assert!(
            std::mem::size_of::<NodeData>() < PRE_BOXING_NODE_DATA_SIZE,
            "document-only state must not inflate element and text nodes"
        );
        assert!(
            std::mem::size_of::<Node<()>>() < PRE_STATIC_SPLIT_NODE_STRIDE
                && PRE_STATIC_SPLIT_NODE_STRIDE < PRE_BOXING_NODE_STRIDE,
            "document-owned layout and boxed document-only state must reduce the primary arena \
             stride"
        );
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn styling_data_has_no_per_node_snapshot_pointer() {
        #[allow(dead_code)]
        struct PreviousStylingData {
            snapshot: Option<Box<stylo::selector_parser::Snapshot>>,
            selector_flags: AtomicUsize,
            dirty_descendants: AtomicBool,
            snapshot_flags: AtomicU8,
            children_to_process: AtomicIsize,
            #[cfg(debug_assertions)]
            slot_guard: slot_guard::SlotGuard,
        }

        let before = if cfg!(debug_assertions) { 48 } else { 32 };
        let after = if cfg!(debug_assertions) { 40 } else { 24 };
        assert_eq!(std::mem::size_of::<PreviousStylingData>(), before);
        assert_eq!(std::mem::size_of::<StylingData>(), after);
        assert_eq!(before - after, std::mem::size_of::<usize>());
    }

    #[test]
    fn document_text_context_is_lazy_and_reused() {
        let mut document = Document::<()>::new(crate::document::tests::device());
        assert!(document.layout_state().text_context.is_none());

        assert_eq!(document.register_fonts(b"not a font"), 0);
        let first = std::ptr::from_ref(
            document
                .layout_state()
                .text_context
                .as_deref()
                .expect("font registration lazily creates the text context"),
        );
        assert_eq!(document.register_fonts(b"still not a font"), 0);
        let second = std::ptr::from_ref(
            document
                .layout_state()
                .text_context
                .as_deref()
                .expect("the text context remains installed"),
        );
        assert_eq!(first, second);
    }

    #[test]
    fn out_of_band_stylo_mutation_fail_closes_layout_style_access() {
        let mut document = Document::<()>::new(crate::document::tests::device());
        let root = document.create_element("page", ());
        document.append_document_element(root);
        document.flush_styles();

        let node = document.get(root).expect("root remains live");
        assert!(node.layout_computed_style().is_some());
        drop(
            <&Node<()> as TElement>::mutate_data(&node).expect("a flushed element owns Stylo data"),
        );
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = node.layout_computed_style();
            }))
            .is_err(),
            "safe out-of-band mutable access must make stale layout pointers inaccessible"
        );
    }

    #[test]
    fn connected_flush_cannot_reenable_a_detached_stale_style_pointer() {
        let mut document = Document::<()>::new(crate::document::tests::device());
        let root = document.create_element("page", ());
        document.append_document_element(root);
        let stale = document.create_element("view", ());
        document.append_child(root, stale);
        let dirty_sibling = document.create_element("view", ());
        document.append_child(root, dirty_sibling);
        document.flush_styles();
        document.detach(stale);
        document.flush_styles();

        {
            let node = document.get(stale).expect("child remains live");
            let mut data = <&Node<()> as TElement>::mutate_data(&node)
                .expect("a flushed element owns Stylo data");
            data.styles.primary = None;
        }
        document.set_inline_style(dirty_sibling, "width: 1px");
        document.flush_styles();

        let stale = document.get(stale).expect("child remains live");
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = stale.layout_computed_style();
            }))
            .is_err(),
            "a connected traversal must not make an unvisited detached stale pointer observable"
        );
    }

    #[test]
    fn node_content_and_text_artifacts_are_lazy() {
        let mut document = Document::<()>::new(crate::document::tests::device());
        let element = document.create_element("view", ());
        assert!(document.get(element).unwrap().content.is_none());

        let text_id = document.create_text_node("hello", ());
        let text = document.get(text_id).unwrap();
        let Some(NodeContent::Text(_)) = text.content.as_deref() else {
            unreachable!("text nodes carry literal-text content")
        };
        assert!(
            document
                .layout_state()
                .nodes
                .get(text_id)
                .expect("text node has aligned layout state")
                .text
                .is_none()
        );

        let first = {
            let (_, artifacts) = document.layout_state_mut().text_parts(text_id);
            std::ptr::from_mut(artifacts)
        };
        assert!(
            document
                .layout_state()
                .nodes
                .get(text_id)
                .expect("text node has aligned layout state")
                .text
                .is_some()
        );
        let second = {
            let (_, artifacts) = document.layout_state_mut().text_parts(text_id);
            std::ptr::from_mut(artifacts)
        };
        assert_eq!(first, second);
    }
}
