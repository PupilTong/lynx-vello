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
use crate::tree::document::{DOCUMENT_NODE_ID, NodeId, NodeSlot, PayloadSlot, TreeArenas};
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
    Element(Option<Arc<ComputedValues>>),
    Text,
    ShadowRoot(Box<ShadowRootData>),
}

/// Inline Stylo traversal and invalidation state.
pub(crate) struct StylingData {
    pub(crate) selector_flags: AtomicUsize,
    pub(crate) dirty_descendants: AtomicBool,
    /// The animation-only counterpart of `dirty_descendants`. Stylo keeps the
    /// two apart so an animation tick can descend to the animating elements
    /// without consuming the dirty bits a pending normal restyle depends on.
    pub(crate) animation_dirty_descendants: AtomicBool,
    /// Whether this element owns any animation or transition state. Stylo
    /// consults it before every `animation_declarations` call during selector
    /// matching, so it has to answer without touching the document's animation
    /// map.
    pub(crate) may_have_animations: AtomicBool,
    pub(crate) snapshot_flags: AtomicU8,
    pub(crate) children_to_process: AtomicIsize,
}

impl Default for StylingData {
    fn default() -> Self {
        Self {
            selector_flags: AtomicUsize::new(0),
            dirty_descendants: AtomicBool::new(false),
            animation_dirty_descendants: AtomicBool::new(false),
            may_have_animations: AtomicBool::new(false),
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
    /// This node's handle: both its identity and the arena position it
    /// occupies, because those are now one thing. A walk that follows
    /// `parent`/`children` indexes the arena directly — there is no id table
    /// to resolve through, which is what keeps a walk step to one load.
    id: NodeId,
    data: NodeData,
    payload: PhantomData<T>,

    pub(crate) parent: Option<NodeSlot>,
    pub(crate) children: Vec<NodeSlot>,
    pub(crate) local_name: Option<LocalName>,
    pub(crate) classes: SmallVec<[Atom; 2]>,
    pub(crate) id_attribute: Option<Atom>,
    pub(crate) attrs: Vec<(LocalName, String)>,
    pub(crate) element_state: ElementState,

    pub(crate) custom_definition: Option<DefinitionId>,
    pub(crate) custom_state: CustomElementState,
    custom_subtree_may_contain: bool,

    pub(crate) parsed_inline_style: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    pub(crate) shadow: Option<Box<ShadowLinks>>,

    style_data: ElementDataWrapper,
    stylo_data_present: AtomicBool,

    pub(crate) styling: StylingData,

    content: Option<Box<NodeContent>>,
}

/// What a post-flush style swap moved on one element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StyleRefresh {
    /// Whether the element's computed style is a different `Arc` than before.
    pub(crate) changed: bool,
    /// Whether anything a descendant text node is *shaped* from moved with
    /// it. Only meaningful when `changed`.
    pub(crate) shaping_changed: bool,
}

impl StyleRefresh {
    pub(crate) const UNCHANGED: Self = Self {
        changed: false,
        shaping_changed: false,
    };
}

impl<T> Node<T> {
    pub(crate) fn new_document(
        owner: *mut TreeArenas<T>,
        id: NodeId,
        lock: StdArc<SharedRwLock>,
        url_data: UrlExtraData,
    ) -> Self {
        debug_assert_eq!(id, DOCUMENT_NODE_ID, "the document node is the first node");
        Self::new(
            owner,
            id,
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
            custom_subtree_may_contain: false,
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
        // SAFETY: `owner` is the address of the `TreeArenas<T>` holding this
        // node — taken by `ptr::from_mut` at construction, never stored again,
        // and fixed for the document's life because `Document` boxes the arena
        // set. The arenas therefore outlive every node inside them, including
        // one `remove_node` has handed back. The reborrow is shared, and every
        // caller reaches it from a `&Node` already lent out of these same
        // arenas, so no `&mut TreeArenas<T>` is live across it. `Relaxed`
        // orders nothing it needs to: the pointer is atomic only so that a
        // bare `*mut` does not cost `Node<T>` the `Sync` that Stylo's parallel
        // traversal requires.
        unsafe {
            &*self.owner.load(Ordering::Relaxed)
        }
    }

    #[inline]
    pub(crate) fn styling_data(&self) -> &StylingData {
        &self.styling
    }

    #[must_use]
    pub(crate) fn custom_subtree_may_contain(&self) -> bool {
        self.custom_subtree_may_contain
    }

    /// Marks this node's shadow-including subtree as possibly containing a
    /// custom element, returning whether it was already marked.
    pub(crate) fn mark_custom_subtree_may_contain(&mut self) -> bool {
        std::mem::replace(&mut self.custom_subtree_may_contain, true)
    }

    pub(crate) fn owner_document(&self) -> &Node<T> {
        self.arenas()
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

    /// Whether this node is a shadow root.
    #[must_use]
    pub fn is_shadow_root(&self) -> bool {
        matches!(&self.data, NodeData::ShadowRoot(_))
    }

    #[must_use]
    pub(crate) fn shadow_host_slot(&self) -> Option<NodeSlot> {
        match &self.data {
            NodeData::ShadowRoot(shadow) => Some(shadow.host),
            _ => None,
        }
    }

    #[must_use]
    pub(crate) fn shadow_host_id(&self) -> Option<NodeId> {
        self.shadow_host_slot()
            .map(|slot| self.arenas().at(slot).id())
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

    /// This node's own storage position, for a walk that is already in slot
    /// space.
    #[must_use]
    #[inline]
    pub(crate) const fn slot(&self) -> NodeSlot {
        self.id
    }

    #[must_use]
    #[inline]
    pub(crate) const fn parent_slot(&self) -> Option<NodeSlot> {
        self.parent
    }

    #[must_use]
    #[inline]
    pub(crate) fn child_slots(&self) -> &[NodeSlot] {
        &self.children
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

    /// Every field of a node, its size in bytes, and whether only an element
    /// can ever use it.
    ///
    /// One node type is stored for all four kinds, so a text node carries an
    /// element's fields without a use for any of them. Nothing in this crate
    /// reads this; it exists so `examples/mem_harness.rs` can price that
    /// without a mirror of the struct that would silently go stale. Read-only,
    /// and the sizes differ between profiles — `ElementDataWrapper` carries a
    /// debug-only borrow flag — so a census has to report which profile it ran
    /// under.
    #[doc(hidden)]
    #[must_use]
    pub fn census_field_sizes() -> &'static [(&'static str, usize, bool)] {
        use std::mem::size_of;
        &[
            ("owner", size_of::<AtomicPtr<TreeArenas<()>>>(), false),
            ("id", size_of::<NodeId>(), false),
            ("data", size_of::<NodeData>(), false),
            ("parent", size_of::<Option<NodeSlot>>(), false),
            ("children", size_of::<Vec<NodeSlot>>(), false),
            ("local_name", size_of::<Option<LocalName>>(), true),
            ("classes", size_of::<SmallVec<[Atom; 2]>>(), true),
            ("id_attribute", size_of::<Option<Atom>>(), true),
            ("attrs", size_of::<Vec<(LocalName, String)>>(), true),
            ("element_state", size_of::<ElementState>(), true),
            ("custom_definition", size_of::<Option<DefinitionId>>(), true),
            ("custom_state", size_of::<CustomElementState>(), true),
            (
                "parsed_inline_style",
                size_of::<Option<Arc<Locked<PropertyDeclarationBlock>>>>(),
                true,
            ),
            ("shadow", size_of::<Option<Box<ShadowLinks>>>(), true),
            ("style_data", size_of::<ElementDataWrapper>(), true),
            ("stylo_data_present", size_of::<AtomicBool>(), true),
            ("styling", size_of::<StylingData>(), true),
            ("content", size_of::<Option<Box<NodeContent>>>(), false),
        ]
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
                let arenas = self.arenas();
                match arenas.payload_at(arenas.live_slot(self.id)) {
                    PayloadSlot::Node(payload) => payload,
                    PayloadSlot::Document | PayloadSlot::ShadowRoot | PayloadSlot::Reserved => {
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

    /// Swaps in the post-flush computed style, reporting what moved.
    ///
    /// The shaping verdict has to be taken here, while both the old and the
    /// new style are alive: the caller only sees the new one, and the old
    /// style structs can be freed — and their addresses reused — the moment
    /// this returns.
    ///
    /// Stylo's style is read here rather than passed in, so the two `Arc`s can
    /// be compared before either is cloned. A harvest visits every element
    /// under a dirty ancestor, but only the ones the flush actually restyled
    /// hold a different `Arc` than they did before, so cloning first would pay
    /// an atomic increment and a decrement per visited element for a value
    /// immediately dropped. Reading it here is also what makes that possible:
    /// the comparison needs Stylo's data and the snapshot at once, and only
    /// disjoint field borrows can hold both.
    pub(crate) fn refresh_layout_style(&mut self) -> StyleRefresh {
        let data = self
            .stylo_data_present
            .get_mut()
            .then(|| self.style_data.borrow());
        let live = data.as_ref().and_then(|data| data.styles.primary.as_ref());
        let NodeData::Element(snapshot) = &mut self.data else {
            debug_assert!(live.is_none(), "only elements own computed styles");
            return StyleRefresh::UNCHANGED;
        };
        let refresh = match (&*snapshot, live) {
            (None, None) => StyleRefresh::UNCHANGED,
            (Some(old), Some(new)) => {
                if Arc::ptr_eq(old, new) {
                    StyleRefresh::UNCHANGED
                } else {
                    StyleRefresh {
                        changed: true,
                        shaping_changed: crate::layout::shaping_inputs_changed(old, new),
                    }
                }
            }
            _ => StyleRefresh {
                changed: true,
                shaping_changed: true,
            },
        };
        if refresh.changed {
            *snapshot = live.cloned();
        }
        refresh
    }

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

    pub(crate) fn is_replaced(&self) -> bool {
        matches!(self.content.as_deref(), Some(NodeContent::Replaced(_)))
    }

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

    pub(crate) fn has_animation_dirty_descendants(&self) -> bool {
        self.styling_data()
            .animation_dirty_descendants
            .load(Ordering::Relaxed)
    }

    pub(crate) fn set_animation_dirty_descendants_bit(&self, dirty: bool) {
        self.styling_data()
            .animation_dirty_descendants
            .store(dirty, Ordering::Relaxed);
    }

    pub(crate) fn may_have_animations(&self) -> bool {
        self.styling_data()
            .may_have_animations
            .load(Ordering::Relaxed)
    }

    pub(crate) fn set_may_have_animations(&self, may: bool) {
        self.styling_data()
            .may_have_animations
            .store(may, Ordering::Relaxed);
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

    pub(crate) fn ensure_style_data(&self) -> &ElementDataWrapper {
        self.stylo_data_present.store(true, Ordering::Release);
        &self.style_data
    }

    pub(crate) fn clear_style_data(&self) {
        *self.style_data.borrow_mut() = ElementData::default();
        self.stylo_data_present.store(false, Ordering::Release);
    }

    pub(crate) fn is_empty_element(&self) -> bool {
        debug_assert!(self.is_element(), "`:empty` is only defined for elements");
        self.text().is_none_or(str::is_empty)
            && self.children.iter().all(|&slot| {
                let child = self.arenas().at(slot);
                !child.is_element()
                    && (!child.is_text_node() || child.text().is_none_or(str::is_empty))
            })
    }
}

impl<T> Node<T> {
    #[must_use]
    pub fn parent(&self) -> Option<&Node<T>> {
        self.parent.map(|slot| self.arenas().at(slot))
    }

    #[must_use]
    pub fn first_child(&self) -> Option<&Node<T>> {
        self.children.first().map(|&slot| self.arenas().at(slot))
    }

    #[must_use]
    pub fn last_child(&self) -> Option<&Node<T>> {
        self.children.last().map(|&slot| self.arenas().at(slot))
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
            return None;
        }
        let tree = self.arenas();
        let siblings = &tree.at(self.parent?).children;
        let pos = siblings
            .iter()
            .position(|&c| c == self.id)
            .expect("node must appear in its parent's child list");
        let sibling = *siblings.get(pos.checked_add_signed(offset)?)?;
        Some(tree.at(sibling))
    }

    #[must_use]
    pub fn children(&self) -> impl ExactSizeIterator<Item = &Node<T>> {
        self.children_iter()
    }

    pub(crate) fn children_iter(&self) -> ChildrenIter<'_, T> {
        ChildrenIter {
            tree: self.arenas(),
            children: &self.children,
            index: 0,
        }
    }

    pub(crate) fn flat_children_iter(&self) -> ChildrenIter<'_, T> {
        ChildrenIter {
            tree: self.arenas(),
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
    tree: &'a TreeArenas<T>,
    children: &'a [NodeSlot],
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
        let slot = *self.children.get(self.index)?;
        self.index += 1;
        Some(self.tree.at(slot))
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
    use crate::{Document, FontBlob};

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn document_only_state_stays_out_of_the_primary_node_stride() {
        const PRE_BOXING_NODE_DATA_SIZE: usize = 32;
        const PRE_BOXING_NODE_STRIDE: usize = 408;
        const PRE_STATIC_SPLIT_NODE_STRIDE: usize = 368;

        assert_eq!(std::mem::size_of::<NodeData>(), 16);
        // A handle is identity and storage position in one 8-byte value, and
        // `Option<NodeId>` has a niche where `Option<usize>` had none, so the
        // parent link costs 8 bytes rather than 16.
        assert_eq!(
            std::mem::size_of::<Node<()>>(),
            if cfg!(debug_assertions) { 224 } else { 216 }
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
            animation_dirty_descendants: AtomicBool,
            may_have_animations: AtomicBool,
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

        assert_eq!(
            document.register_fonts(FontBlob::from_static(b"not a font")),
            0
        );
        let first = std::ptr::from_ref(
            document
                .layout_state()
                .text_context
                .as_deref()
                .expect("font registration lazily creates the text context"),
        );
        assert_eq!(
            document.register_fonts(FontBlob::from_static(b"still not a font")),
            0
        );
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

    /// A detached element reports a diverged harvested style in debug builds.
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
        document.remove_element(stale);
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
        let text_slot = document.live_slot(text_id);
        let text = document.get(text_id).unwrap();
        let Some(NodeContent::Text(_)) = text.content.as_deref() else {
            unreachable!("text nodes carry literal-text content")
        };
        assert!(
            document
                .layout_state()
                .get(text_slot)
                .is_none_or(|state| state.text.is_none())
        );

        let first = {
            let (_, artifacts) = document.layout_state_mut().text_parts(text_slot);
            std::ptr::from_mut(artifacts)
        };
        assert!(document.layout_state().at(text_slot).text.is_some());
        let second = {
            let (_, artifacts) = document.layout_state_mut().text_parts(text_slot);
            std::ptr::from_mut(artifacts)
        };
        assert_eq!(first, second);
    }
}
