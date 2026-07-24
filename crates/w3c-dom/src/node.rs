//! [`Node`] — the unit the tree is composed of — and its `&Node` read/
//! navigation handle.

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc as StdArc, OnceLock};

use atomic_refcell::{AtomicRef, AtomicRefCell};
use dom::ElementState;
#[cfg(feature = "layout-test-utils")]
use neutron_star::compute::LeafMetrics;
use neutron_star::compute::NaturalSize;
use neutron_star::text::{TextContext, TextLayoutStore};
use neutron_star::tree::Layout;
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

use crate::document::{
    DOCUMENT_NODE_ID, DocumentArenas, NodeId, PayloadSlot, slab_get_for_live_node,
};
use crate::layout::{LayoutData, LayoutResults};

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeType {
    Document,
    Element,
    Text,
}

struct DocumentNodeData {
    lock: StdArc<SharedRwLock>,
    url_data: UrlExtraData,
    text_context: OnceLock<AtomicRefCell<TextContext>>,
    #[cfg(debug_assertions)]
    in_flush: AtomicBool,
}

enum NodeData {
    Document(Box<DocumentNodeData>),
    Element,
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
    Text {
        value: String,
        artifacts: OnceLock<Box<AtomicRefCell<TextLayoutStore>>>,
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
pub struct Node<T> {
    owner: AtomicPtr<DocumentArenas<T>>,
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

    pub(crate) layout_results: AtomicRefCell<LayoutResults>,
}

impl<T> Node<T> {
    pub(crate) fn new_document(
        owner: *mut DocumentArenas<T>,
        lock: StdArc<SharedRwLock>,
        url_data: UrlExtraData,
    ) -> Self {
        Self::new(
            owner,
            DOCUMENT_NODE_ID,
            NodeData::Document(Box::new(DocumentNodeData {
                lock,
                url_data,
                text_context: OnceLock::new(),
                #[cfg(debug_assertions)]
                in_flush: AtomicBool::new(false),
            })),
            None,
            None,
        )
    }

    pub(crate) fn new_element(
        owner: *mut DocumentArenas<T>,
        id: NodeId,
        local_name: LocalName,
    ) -> Self {
        Self::new(owner, id, NodeData::Element, Some(local_name), None)
    }

    pub(crate) fn new_text(owner: *mut DocumentArenas<T>, id: NodeId, text: String) -> Self {
        Self::new(owner, id, NodeData::Text, None, Some(text))
    }

    fn new(
        owner: *mut DocumentArenas<T>,
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
            layout_results: AtomicRefCell::new(LayoutResults::default()),
        }
    }

    pub(crate) fn arenas(&self) -> &DocumentArenas<T> {
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

    #[inline]
    pub(crate) fn layout_data(&self) -> &AtomicRefCell<LayoutData> {
        slab_get_for_live_node(&self.arenas().layout, self.id)
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

    pub(crate) fn text_context(&self) -> &AtomicRefCell<TextContext> {
        self.document_data()
            .text_context
            .get_or_init(|| AtomicRefCell::new(TextContext::new()))
    }

    #[cfg(debug_assertions)]
    pub(crate) fn flush_flag(&self) -> &AtomicBool {
        &self.document_data().in_flush
    }

    #[cfg(debug_assertions)]
    pub(crate) fn in_flush(&self) -> bool {
        self.flush_flag().load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn id(&self) -> NodeId {
        self.id
    }

    #[must_use]
    pub fn node_type(&self) -> NodeType {
        match &self.data {
            NodeData::Document(_) => NodeType::Document,
            NodeData::Element => NodeType::Element,
            NodeData::Text => NodeType::Text,
        }
    }

    #[must_use]
    pub fn is_document(&self) -> bool {
        matches!(&self.data, NodeData::Document(_))
    }

    #[must_use]
    pub fn is_element(&self) -> bool {
        matches!(&self.data, NodeData::Element)
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
            Some(NodeContent::Text { value, .. }) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn payload(&self) -> &T {
        match &self.data {
            NodeData::Element | NodeData::Text => {
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

    #[must_use]
    pub fn rounded_layout(&self) -> impl std::ops::Deref<Target = Layout> + '_ {
        AtomicRef::map(self.layout_results.borrow(), |results| &results.rounded)
    }

    #[must_use]
    pub fn unrounded_layout(&self) -> impl std::ops::Deref<Target = Layout> + '_ {
        AtomicRef::map(self.layout_results.borrow(), |results| &results.unrounded)
    }

    #[must_use]
    pub fn layout_cache_is_empty(&self) -> bool {
        self.layout_data().borrow().measure_cache.is_empty()
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

    pub(crate) fn text_artifacts(&self) -> &AtomicRefCell<TextLayoutStore> {
        match self.content.as_deref() {
            Some(NodeContent::Text { artifacts, .. }) => artifacts
                .get_or_init(|| Box::new(AtomicRefCell::new(TextLayoutStore::default())))
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
    use super::*;
    use crate::Document;

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn document_only_state_stays_out_of_the_primary_node_stride() {
        const PRE_BOXING_NODE_DATA_SIZE: usize = 32;
        const PRE_BOXING_NODE_STRIDE: usize = 408;

        assert_eq!(std::mem::size_of::<NodeData>(), 16);
        assert_eq!(std::mem::size_of::<Node<()>>(), 368);
        assert!(
            std::mem::size_of::<NodeData>() < PRE_BOXING_NODE_DATA_SIZE,
            "document-only state must not inflate element and text nodes"
        );
        assert!(
            std::mem::size_of::<Node<()>>() < PRE_BOXING_NODE_STRIDE,
            "boxing document-only state must reduce the primary arena stride"
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
        let document = Document::<()>::new(crate::document::tests::device());
        let root = document.root_node();
        let NodeData::Document(document) = &root.data else {
            unreachable!("slot zero is the document node")
        };

        assert!(document.text_context.get().is_none());
        let first = root.text_context();
        assert!(document.text_context.get().is_some());
        assert!(std::ptr::eq(first, root.text_context()));
    }

    #[test]
    fn node_content_and_text_artifacts_are_lazy() {
        let mut document = Document::<()>::new(crate::document::tests::device());
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
