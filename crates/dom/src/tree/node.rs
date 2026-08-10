//! [`Node`] — the unit the tree is composed of — and its `&Node` read/
//! navigation handle.

use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU8, AtomicUsize, Ordering};

#[cfg(feature = "layout-test-utils")]
use hughie::compute::LeafMetrics;
use hughie::compute::NaturalSize;
use selectors::matching::ElementSelectorFlags;
use slab::Slab;
use smallvec::SmallVec;
use stylo::LocalName;
use stylo::data::{ElementData, ElementDataRef, ElementDataWrapper};
use stylo::properties::{ComputedValues, PropertyDeclarationBlock};
use stylo::servo_arc::Arc;
use stylo::shared_lock::{Locked, SharedRwLock};
use stylo::stylesheets::UrlExtraData;
use stylo_atoms::Atom;
use stylo_dom::ElementState;

use crate::tree::custom::{CustomElementState, DefinitionId};
use crate::tree::document::{DOCUMENT_NODE_ID, NodeId, PayloadSlot, TreeArenas};
use crate::tree::shadow::{ShadowLinks, ShadowRootData, ShadowRootMode};

pub(crate) const SNAPSHOT_PRESENT: u8 = 1 << 0;
pub(crate) const SNAPSHOT_HANDLED: u8 = 1 << 1;

struct DocumentNodeData {
    lock: StdArc<SharedRwLock>,
    url_data: UrlExtraData,
    in_flush: StdArc<AtomicBool>,
}

enum NodeData {
    Document(Box<DocumentNodeData>),
    /// Layout/paint snapshot of the element's primary
    /// `Arc<ComputedValues>`, refreshed by the exclusive post-flush damage
    /// harvest. The `Arc` keeps the pointee alive, so reads are always
    /// memory-safe; a debug assertion in [`Node::layout_computed_style`]
    /// verifies the snapshot still matches Stylo's live primary style.
    Element(Option<Arc<ComputedValues>>),
    Text,
    /// The root of one shadow tree, reached from its host through
    /// [`Node::shadow_root_id`] rather than the host's child list. Boxed so a
    /// shadow root's scoped stylesheet set costs elements and text nodes
    /// nothing.
    ShadowRoot(Box<ShadowRootData>),
}

/// Stylo's per-node traversal and invalidation bookkeeping, stored inline on
/// the [`Node`] so traversal flag access shares the node's cache lines.
/// Snapshot payloads are sparse, document-owned state; only their atomic
/// traversal lifecycle flags remain here.
pub(crate) struct StylingData {
    pub(crate) selector_flags: AtomicUsize,
    pub(crate) dirty_descendants: AtomicBool,
    pub(crate) snapshot_flags: AtomicU8,
    pub(crate) children_to_process: AtomicIsize,
}

impl Default for StylingData {
    fn default() -> Self {
        Self {
            selector_flags: AtomicUsize::new(0),
            dirty_descendants: AtomicBool::new(false),
            snapshot_flags: AtomicU8::new(0),
            children_to_process: AtomicIsize::new(0),
        }
    }
}

enum NodeContent {
    Text(String),
    Replaced(NaturalSize),
    #[cfg(feature = "layout-test-utils")]
    Test(LeafMetrics),
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

    /// The definition bound to this element at creation, when its tag had one.
    /// `NonZeroU32`-backed so the `Option` is four bytes and lands in the
    /// primary node's existing tail padding.
    pub(crate) custom_definition: Option<DefinitionId>,
    /// This element's position in the custom element state machine. Non-element
    /// nodes keep the default and are never consulted.
    pub(crate) custom_state: CustomElementState,

    pub(crate) parsed_inline_style: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    /// Shadow-DOM links, allocated only for the nodes that take part in one:
    /// a host, a slot, or a slotted node. Every other node keeps one `None`
    /// word, so the flat-tree walks cost a predictable branch instead of a
    /// wider primary arena stride.
    pub(crate) shadow: Option<Box<ShadowLinks>>,

    /// Stylo's per-element style data, unconditionally present so no outer
    /// cell is needed: interior mutability lives entirely inside the upstream
    /// [`ElementDataWrapper`] (release-free, debug-checked borrows).
    /// [`Self::stylo_data_present`] tracks Stylo's has-data protocol, which
    /// the upstream wrapper does not model.
    style_data: ElementDataWrapper,
    stylo_data_present: AtomicBool,

    pub(crate) styling: StylingData,

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
                in_flush: StdArc::new(AtomicBool::new(false)),
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
        let mut node = Self::new(owner, id, NodeData::Element(None), Some(local_name), None);
        // Every element matches `:defined` and nothing ever clears the bit:
        // the standard's `undefined` state exists only for an element whose
        // definition has not arrived yet, and this crate requires definitions
        // to precede their elements (see `tree::custom`'s scope note). Seeding
        // it here rather than answering the selector from a state field keeps
        // the matcher a single bitset test.
        node.element_state = ElementState::DEFINED;
        node
    }

    pub(crate) fn new_text(owner: *mut TreeArenas<T>, id: NodeId, text: String) -> Self {
        Self::new(owner, id, NodeData::Text, None, Some(text))
    }

    pub(crate) fn new_shadow_root(
        owner: *mut TreeArenas<T>,
        id: NodeId,
        data: ShadowRootData,
    ) -> Self {
        Self::new(owner, id, NodeData::ShadowRoot(Box::new(data)), None, None)
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
            custom_definition: None,
            custom_state: CustomElementState::default(),
            parsed_inline_style: None,
            shadow: None,
            style_data: ElementDataWrapper::default(),
            stylo_data_present: AtomicBool::new(false),
            styling: StylingData::default(),
            content: text.map(|value| Box::new(NodeContent::Text(value))),
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
        &self.styling
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

    pub(crate) fn flush_flag(&self) -> &StdArc<AtomicBool> {
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

    /// Whether this node is the root of a shadow tree. A shadow root is
    /// neither an element nor a text node: it matches no selector, generates
    /// no box, and is transparent in the flat tree, where its children hang
    /// directly off its host.
    #[must_use]
    pub fn is_shadow_root(&self) -> bool {
        matches!(&self.data, NodeData::ShadowRoot(_))
    }

    /// The element this shadow root is attached to.
    #[must_use]
    pub(crate) fn shadow_host_id(&self) -> Option<NodeId> {
        match &self.data {
            NodeData::ShadowRoot(shadow) => Some(shadow.host),
            _ => None,
        }
    }

    #[must_use]
    pub(crate) fn shadow_root_mode(&self) -> Option<ShadowRootMode> {
        match &self.data {
            NodeData::ShadowRoot(shadow) => Some(shadow.mode),
            _ => None,
        }
    }

    pub(crate) fn shadow_data(&self) -> Option<&ShadowRootData> {
        match &self.data {
            NodeData::ShadowRoot(shadow) => Some(shadow),
            _ => None,
        }
    }

    pub(crate) fn shadow_data_mut(&mut self) -> Option<&mut ShadowRootData> {
        match &mut self.data {
            NodeData::ShadowRoot(shadow) => Some(shadow),
            _ => None,
        }
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
    fn local_name(&self) -> Option<&LocalName> {
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
                let slot = self
                    .arenas()
                    .payloads
                    .get(self.id)
                    .expect("live node must have payload-arena state");
                match slot {
                    PayloadSlot::Node(payload) => payload,
                    PayloadSlot::Document | PayloadSlot::ShadowRoot => {
                        unreachable!("payload-less sentinels belong to non-element nodes")
                    }
                }
            }
            NodeData::Document(_) => panic!("the document node has no payload"),
            NodeData::ShadowRoot(_) => panic!("a shadow root has no payload"),
        }
    }

    pub(crate) fn has_style_data(&self) -> bool {
        self.stylo_data_present.load(Ordering::Acquire)
    }

    pub(crate) fn needs_style_flush(&self) -> bool {
        let styling = self.styling_data();
        if styling.dirty_descendants.load(Ordering::Relaxed)
            || styling.snapshot_flags.load(Ordering::Relaxed) & SNAPSHOT_PRESENT != 0
        {
            return true;
        }
        !self.has_style_data() || !self.style_data.borrow().hint.is_empty()
    }

    #[must_use]
    pub fn computed_style(&self) -> Option<Arc<ComputedValues>> {
        self.borrow_computed_style()
            .and_then(|data| data.styles.primary.clone())
    }

    pub(crate) fn borrow_computed_style(&self) -> Option<ElementDataRef<'_>> {
        if !self.has_style_data() {
            return None;
        }
        let data = self.style_data.borrow();
        data.styles.primary.as_ref()?;
        Some(data)
    }

    pub(crate) fn style_data_wrapper(&self) -> Option<&ElementDataWrapper> {
        self.has_style_data().then_some(&self.style_data)
    }

    /// Store the harvested layout-style snapshot, reporting whether the
    /// snapshot's `Arc` identity changed. The damage harvest keys its descent
    /// on that change: a freshly (re)styled element is exactly one whose
    /// snapshot moved, and its children may hold initial styles Stylo's
    /// dirty-descendants bookkeeping does not cover.
    pub(crate) fn refresh_layout_style(&mut self, style: Option<Arc<ComputedValues>>) -> bool {
        let NodeData::Element(snapshot) = &mut self.data else {
            debug_assert!(style.is_none(), "only elements own computed styles");
            return false;
        };
        let changed = match (&*snapshot, &style) {
            (None, None) => false,
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
            _ => true,
        };
        if changed {
            *snapshot = style;
        }
        changed
    }

    /// Borrow the harvested computed style without re-entering Stylo's
    /// runtime borrow checker or incrementing the style `Arc`.
    ///
    /// The snapshot is refreshed by the exclusive post-flush damage harvest
    /// and its `Arc` keeps the value alive, so this is always memory-safe.
    /// After a traversal that panicked mid-flush the snapshot can lag the
    /// live style (the document is unspecified per the let-it-crash policy);
    /// the debug assertion below reports any divergence.
    pub(crate) fn layout_computed_style(&self) -> Option<&ComputedValues> {
        let NodeData::Element(snapshot) = &self.data else {
            return None;
        };
        #[cfg(debug_assertions)]
        {
            let live = self.borrow_computed_style();
            let live_primary = live.as_ref().and_then(|data| data.styles.primary.as_ref());
            let matches = match (snapshot.as_ref(), live_primary) {
                (None, None) => true,
                (Some(old), Some(new)) => Arc::ptr_eq(old, new),
                _ => false,
            };
            debug_assert!(
                matches,
                "layout-style snapshot diverged from Stylo's live primary style — the damage \
                 harvest missed this element (invalidation bug) or a traversal did not complete"
            );
        }
        snapshot.as_deref()
    }

    #[must_use]
    pub(crate) fn natural_size(&self) -> NaturalSize {
        match self.content.as_deref() {
            Some(NodeContent::Replaced(natural_size)) => *natural_size,
            _ => NaturalSize::NONE,
        }
    }

    /// Whether this node generates a replaced box.
    ///
    /// Independent of whether a natural size has arrived: an `<img>` is a
    /// replaced element from the moment it exists, and its intrinsic dimensions
    /// show up one network round trip later. Conflating the two made an image
    /// stop being replaced whenever its size was unknown — which is exactly the
    /// pre-decode state — so layout would route it back into its parent's
    /// formatting context for the first frame and then move it out again.
    pub(crate) fn is_replaced(&self) -> bool {
        matches!(self.content.as_deref(), Some(NodeContent::Replaced(_)))
    }

    /// Installs intrinsic dimensions, and makes the node replaced if it is not
    /// already. [`NaturalSize::NONE`] clears the dimensions but keeps replaced
    /// status — use `set_element_text_content` to make a node non-replaced.
    pub(crate) fn set_natural_size(&mut self, natural_size: NaturalSize) -> bool {
        if self.natural_size() == natural_size && self.is_replaced() {
            return false;
        }
        self.content = Some(Box::new(NodeContent::Replaced(natural_size)));
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
        self.content = text.map(|value| Box::new(NodeContent::Text(value)));
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
        if *self.stylo_data_present.get_mut() {
            Some(&mut self.style_data)
        } else {
            None
        }
    }

    /// Marks the element's Stylo data present and lends the wrapper for the
    /// traversal's exclusive per-element write.
    pub(crate) fn ensure_style_data(&self) -> &ElementDataWrapper {
        self.stylo_data_present.store(true, Ordering::Release);
        &self.style_data
    }

    /// Resets the element's Stylo data to its unstyled state.
    pub(crate) fn clear_style_data(&self) {
        *self.style_data.borrow_mut() = ElementData::default();
        self.stylo_data_present.store(false, Ordering::Release);
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
        if self.is_shadow_root() {
            // A shadow root points at its host as its parent so the ancestor
            // spines stay connected, but it is not one of the host's children
            // and therefore has no siblings.
            return None;
        }
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
    pub fn children(&self) -> impl ExactSizeIterator<Item = &Node<T>> {
        self.children_iter()
    }

    pub(crate) fn children_iter(&self) -> ChildrenIter<'_, T> {
        ChildrenIter {
            tree: self.tree(),
            children: &self.children,
            index: 0,
        }
    }

    /// The flat-tree children — what Stylo traverses, layout lays out, and
    /// paint walks. Identical to [`Self::children_iter`] until a shadow root
    /// exists.
    pub(crate) fn flat_children_iter(&self) -> ChildrenIter<'_, T> {
        ChildrenIter {
            tree: self.tree(),
            children: self.flat_children(),
            index: 0,
        }
    }
}

impl<T> fmt::Debug for Node<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id)
            .field(
                "node_type",
                &if self.is_document() {
                    "document"
                } else if self.is_element() {
                    "element"
                } else if self.is_shadow_root() {
                    "shadow-root"
                } else {
                    "text"
                },
            )
            .field("tag", &self.tag_name())
            .field("text", &self.text())
            .field("classes", &self.classes)
            .field("id_attribute", &self.id_attribute)
            .field("element_state", &self.element_state)
            .field(
                "has_parsed_inline_style",
                &self.parsed_inline_style.is_some(),
            )
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
#[doc(hidden)]
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

        // The boxed shadow-root variant is why this is still 16: a shadow
        // root's host, mode, and scoped stylesheet set live behind one
        // pointer rather than widening every element and text node.
        assert_eq!(std::mem::size_of::<NodeData>(), 16);
        // Assumes the workspace-wide `smallvec/union` layout (root
        // Cargo.toml note).
        assert_eq!(
            std::mem::size_of::<Node<()>>(),
            if cfg!(debug_assertions) { 232 } else { 224 }
        );
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
        }

        assert_eq!(std::mem::size_of::<PreviousStylingData>(), 32);
        assert_eq!(std::mem::size_of::<StylingData>(), 24);
        assert_eq!(std::mem::size_of::<usize>(), 32 - 24);
    }

    #[test]
    fn document_text_context_is_lazy_and_reused() {
        let mut document = Document::<()>::new(crate::tree::document::tests::device(), "page", ());
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
    fn out_of_band_stylo_mutation_keeps_snapshot_readable() {
        let mut document = Document::<()>::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        document.flush_styles_with_damage_sink(&mut |_, _| {});

        let node = document.get(root).expect("root remains live");
        let before = std::ptr::from_ref(node.layout_computed_style().expect("root is styled"));
        // Hint-level out-of-band access — what invalidation writes between
        // flushes — does not move the primary style, so the harvested
        // snapshot stays identical and readable.
        drop(
            <&Node<()> as TElement>::mutate_data(&node).expect("a flushed element owns Stylo data"),
        );
        let after = std::ptr::from_ref(
            node.layout_computed_style()
                .expect("the snapshot remains readable"),
        );
        assert_eq!(
            before, after,
            "hint-level access must leave the snapshot untouched"
        );
    }

    /// An unvisited detached element whose primary style is cleared out of
    /// band diverges from its harvested snapshot. Release builds keep reading
    /// the snapshot (stale but `Arc`-owned, so memory-safe); debug builds
    /// report the divergence at the read site.
    #[cfg(debug_assertions)]
    #[test]
    fn diverged_snapshot_on_unvisited_element_is_reported_in_debug() {
        let mut document = Document::<()>::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let stale = document.create_element("view", ());
        document.append_child(root, stale);
        let dirty_sibling = document.create_element("view", ());
        document.append_child(root, dirty_sibling);
        document.flush_styles_with_damage_sink(&mut |_, _| {});
        document.detach(stale);
        document.flush_styles_with_damage_sink(&mut |_, _| {});

        {
            let node = document.get(stale).expect("child remains live");
            let mut data = <&Node<()> as TElement>::mutate_data(&node)
                .expect("a flushed element owns Stylo data");
            data.styles.primary = None;
        }
        document.set_inline_style(dirty_sibling, "width: 1px");
        document.flush_styles_with_damage_sink(&mut |_, _| {});

        let stale = document.get(stale).expect("child remains live");
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = stale.layout_computed_style();
            }))
            .is_err(),
            "debug builds must report a snapshot that diverged from the live style"
        );
    }

    #[test]
    fn node_content_and_text_artifacts_are_lazy() {
        let mut document = Document::<()>::new(crate::tree::document::tests::device(), "page", ());
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
