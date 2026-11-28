//! [`Node`] — the unit the tree is composed of — and its `&Node` read/
//! navigation handle.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

#[cfg(feature = "layout-test-utils")]
use seldon::compute::LeafMetrics;
use seldon::compute::NaturalSize;
use selectors::matching::ElementSelectorFlags;
use smallvec::SmallVec;
use stylo::LocalName;
use stylo::data::{ElementData, ElementDataRef, ElementDataWrapper};
use stylo::properties::{ComputedValues, PropertyDeclarationBlock};
use stylo::selector_parser::PseudoElement;
use stylo::servo_arc::Arc;
use stylo::shared_lock::{Locked, SharedRwLock};
use stylo::stylesheets::UrlExtraData;
use stylo_atoms::Atom;
use stylo_dom::ElementState;

use crate::render::picture::PictureKey;
use crate::tree::arena::ArenaPtr;
use crate::tree::custom::{CustomElementState, DefinitionId};
use crate::tree::document::{DOCUMENT_NODE_ID, NodeId, NodeSlot, TreeArenas};
use crate::tree::shadow::{ShadowLinks, ShadowRootData, ShadowRootMode};

pub(crate) const SNAPSHOT_PRESENT: u8 = 1 << 0;
pub(crate) const SNAPSHOT_HANDLED: u8 = 1 << 1;

pub(crate) const ANIMATES_OPACITY: u8 = 1 << 0;
pub(crate) const ANIMATES_TRANSFORM: u8 = 1 << 1;

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

pub(crate) struct StylingData {
    pub(crate) selector_flags: AtomicUsize,
    pub(crate) dirty_descendants: AtomicBool,
    pub(crate) animation_dirty_descendants: AtomicBool,
    pub(crate) may_have_animations: AtomicBool,
    pub(crate) animates: AtomicU8,
    pub(crate) snapshot_flags: AtomicU8,
}

impl Default for StylingData {
    fn default() -> Self {
        Self {
            selector_flags: AtomicUsize::new(0),
            dirty_descendants: AtomicBool::new(false),
            animation_dirty_descendants: AtomicBool::new(false),
            may_have_animations: AtomicBool::new(false),
            animates: AtomicU8::new(0),
            snapshot_flags: AtomicU8::new(0),
        }
    }
}

enum NodeContent {
    Text(Box<str>),
    Replaced(Box<Replaced>),
    #[cfg(feature = "layout-test-utils")]
    Test(Box<LeafMetrics>),
}

struct Replaced {
    natural_size: NaturalSize,
    picture: Option<PictureKey>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NodeFlags([u8; 2]);

impl NodeFlags {
    const CUSTOM_SUBTREE_MAY_CONTAIN: u16 = 1 << 0;
    const MAY_OVERRIDE_CHILDREN: u16 = 1 << 1;
    const LYNX_TEXT: u16 = 1 << 2;
    const PLACEHOLDER_PSEUDO: u16 = 1 << 3;
    const LINE_IF_EMPTY: u16 = 1 << 4;
    pub(crate) const BACKDROP: u16 = 1 << 5;
    pub(crate) const ANONYMOUS: u16 = 1 << 6;
    const BASIS_FREE: u16 = 1 << 7;
    const CONTAINER_TIMING_ROOT: u16 = 1 << 8;
    const CONTAINER_TIMING_IGNORE: u16 = 1 << 9;
    const SELF_OR_ANCESTOR_HAS_CONTAINER_TIMING: u16 = 1 << 10;
    const SVG_NAMESPACE: u16 = 1 << 13;
    const CUSTOM_STATE: u16 = 0b11 << Self::CUSTOM_STATE_SHIFT;
    const CUSTOM_STATE_SHIFT: u16 = 11;

    const fn get(self, bit: u16) -> bool {
        u16::from_le_bytes(self.0) & bit != 0
    }

    fn set(&mut self, bit: u16, on: bool) -> bool {
        let bits = u16::from_le_bytes(self.0);
        self.0 = if on { bits | bit } else { bits & !bit }.to_le_bytes();
        bits & bit != 0
    }

    const fn custom_state(self) -> CustomElementState {
        match (u16::from_le_bytes(self.0) & Self::CUSTOM_STATE) >> Self::CUSTOM_STATE_SHIFT {
            0 => CustomElementState::Uncustomized,
            1 => CustomElementState::Constructing,
            _ => CustomElementState::Custom,
        }
    }

    fn set_custom_state(&mut self, state: CustomElementState) {
        let value: u16 = match state {
            CustomElementState::Uncustomized => 0,
            CustomElementState::Constructing => 1,
            CustomElementState::Custom => 2,
        };
        let bits = u16::from_le_bytes(self.0) & !Self::CUSTOM_STATE;
        self.0 = (bits | value << Self::CUSTOM_STATE_SHIFT).to_le_bytes();
    }
}

pub struct Node<T> {
    owner: ArenaPtr<T>,
    id: NodeId,
    data: NodeData,
    payload: Option<T>,

    pub(crate) parent: Option<NodeSlot>,
    pub(crate) children: Vec<NodeSlot>,
    pub(crate) child_index: u32,
    pub(crate) child_index_base: u32,
    pub(crate) local_name: Option<LocalName>,
    pub(crate) classes: SmallVec<[Atom; 2]>,
    pub(crate) id_attribute: Option<Atom>,
    pub(crate) attrs: Vec<(LocalName, String)>,
    pub(crate) element_state: ElementState,

    pub(crate) custom_definition: Option<DefinitionId>,
    flags: NodeFlags,

    pub(crate) parsed_inline_style: Option<Arc<Locked<PropertyDeclarationBlock>>>,
    pub(crate) presentational_hints: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    pub(crate) shadow: Option<Box<ShadowLinks>>,

    style_data: ElementDataWrapper,
    stylo_data_present: AtomicBool,

    pub(crate) styling: StylingData,

    content: Option<Box<NodeContent>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent verdicts the harvest reads one by one, not a state machine"
)]
pub(crate) struct StyleRefresh {
    pub(crate) changed: bool,
    pub(crate) paragraph_limits_changed: bool,
    pub(crate) containing_block_changed: bool,
    pub(crate) box_tree_changed: bool,
    pub(crate) first: bool,
}

impl StyleRefresh {
    pub(crate) const UNCHANGED: Self = Self {
        changed: false,
        paragraph_limits_changed: false,
        containing_block_changed: false,
        box_tree_changed: false,
        first: false,
    };
}

impl<T> Node<T> {
    pub(crate) fn new_document(
        owner: ArenaPtr<T>,
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

    pub(crate) fn new_element(owner: ArenaPtr<T>, id: NodeId, local_name: LocalName) -> Self {
        let mut node = Self::new(owner, id, NodeData::Element(None), Some(local_name), None);
        node.element_state = ElementState::DEFINED;
        node
    }

    pub(crate) fn new_text(owner: ArenaPtr<T>, id: NodeId, text: Box<str>) -> Self {
        Self::new(owner, id, NodeData::Text, None, Some(text))
    }

    pub(crate) fn new_generated_element(
        owner: ArenaPtr<T>,
        id: NodeId,
        local_name: LocalName,
        marker: u16,
    ) -> Self {
        debug_assert!(
            marker == NodeFlags::BACKDROP || marker == NodeFlags::ANONYMOUS,
            "a generated element is a backdrop or an anonymous box"
        );
        let mut node = Self::new_element(owner, id, local_name);
        node.flags.set(marker, true);
        node
    }

    pub(crate) fn new_svg_element(owner: ArenaPtr<T>, id: NodeId, local_name: LocalName) -> Self {
        let mut node = Self::new_element(owner, id, local_name);
        node.flags.set(NodeFlags::SVG_NAMESPACE, true);
        node
    }

    #[must_use]
    pub(crate) fn with_payload(mut self, payload: T) -> Self {
        self.payload = Some(payload);
        self
    }

    pub(crate) fn into_payload(self) -> Option<T> {
        self.payload
    }

    pub(crate) fn new_shadow_root(owner: ArenaPtr<T>, id: NodeId, data: ShadowRootData) -> Self {
        Self::new(owner, id, NodeData::ShadowRoot(Box::new(data)), None, None)
    }

    fn new(
        owner: ArenaPtr<T>,
        id: NodeId,
        data: NodeData,
        local_name: Option<LocalName>,
        text: Option<Box<str>>,
    ) -> Self {
        Self {
            owner,
            id,
            data,
            payload: None,
            parent: None,
            children: Vec::new(),
            child_index: 0,
            child_index_base: 0,
            local_name,
            classes: SmallVec::new(),
            id_attribute: None,
            attrs: Vec::new(),
            element_state: ElementState::empty(),
            custom_definition: None,
            flags: NodeFlags::default(),
            parsed_inline_style: None,
            presentational_hints: None,
            shadow: None,
            style_data: ElementDataWrapper::default(),
            stylo_data_present: AtomicBool::new(false),
            styling: StylingData::default(),
            content: text.map(|value| Box::new(NodeContent::Text(value))),
        }
    }

    pub(crate) fn arenas(&self) -> &TreeArenas<T> {
        #[expect(unsafe_code, reason = "deref the owning arena-set backpointer")]
        // SAFETY: `owner` is an `ArenaPtr`, which only `ArenaBox` makes: the
        // pointer it allocated this node's arena set through, handed to the
        // node's constructor when `ArenaBox::insert_node` filed it.
        //
        // It is valid: the allocation is freed only when that `ArenaBox`
        // drops, and no node outlives it. A node inside the arena set is
        // dropped with it, and one `remove_node` handed back is dropped by
        // the `Document` method that freed it, before that method returns.
        //
        // Its provenance is still usable here: every `&TreeArenas<T>` and
        // `&mut TreeArenas<T>` the document has used was derived from this
        // same pointer by `ArenaBox`'s `Deref`/`DerefMut`, so no write
        // through them has invalidated it. A pointer taken from a `&mut`
        // borrow would not stay usable: the next write through another
        // `&mut TreeArenas<T>` invalidates it, and moving a `Box` holding the
        // arena set retags the `Box`.
        //
        // Reading through it ends any `&mut` derived from it for the bytes
        // the read reaches, so no such `&mut` may be used again for those
        // bytes. The borrow checker enforces most of that: the result borrows
        // `&self`, a node lent out of these arenas, so the `&mut` that lent
        // the node is unusable while the result is in use. The case it does
        // not catch is a `&mut Node` that calls this through a reborrow,
        // reaches its own node through the result, and writes through the
        // `&mut` afterwards. No caller does; Miri under Tree Borrows reports
        // one on any test path that does.
        //
        // The raw pointer is also what states this tree's thread affinity. It
        // costs `Node<T>` its `Send`, and with it `Document<T>`'s — which is
        // the point: a document is created on the thread that owns it
        // (`bobcat-main`) and never leaves. It costs no `Sync`, because
        // `Node<T>` never had any — `StylingData`'s `UnsafeCell<ElementData>`
        // denies it whatever this field is. The `AtomicPtr` that used to sit
        // here was justified as buying that `Sync`; it did not, and the only
        // property it actually conferred was the `Send` a document must not
        // have.
        unsafe {
            &*self.owner.as_ptr()
        }
    }

    #[inline]
    pub(crate) const fn owner(&self) -> ArenaPtr<T> {
        self.owner
    }

    #[inline]
    pub(crate) fn styling_data(&self) -> &StylingData {
        &self.styling
    }

    #[must_use]
    pub(crate) fn custom_subtree_may_contain(&self) -> bool {
        self.flags.get(NodeFlags::CUSTOM_SUBTREE_MAY_CONTAIN)
    }

    pub(crate) fn mark_custom_subtree_may_contain(&mut self) -> bool {
        self.flags.set(NodeFlags::CUSTOM_SUBTREE_MAY_CONTAIN, true)
    }

    #[must_use]
    pub(crate) fn may_override_children(&self) -> bool {
        self.flags.get(NodeFlags::MAY_OVERRIDE_CHILDREN)
    }

    pub(crate) fn set_may_override_children(&mut self, may: bool) {
        self.flags.set(NodeFlags::MAY_OVERRIDE_CHILDREN, may);
    }

    pub(crate) fn mark_lynx_text(&mut self) {
        self.flags.set(NodeFlags::LYNX_TEXT, true);
        self.refresh_basis_free();
    }

    pub(crate) fn has_line_if_empty(&self) -> bool {
        self.flags.get(NodeFlags::LINE_IF_EMPTY)
    }

    pub(crate) fn set_has_line_if_empty(&mut self, on: bool) -> bool {
        self.flags.set(NodeFlags::LINE_IF_EMPTY, on)
    }

    #[must_use]
    #[inline]
    pub(crate) const fn in_svg_namespace(&self) -> bool {
        self.flags.get(NodeFlags::SVG_NAMESPACE)
    }

    #[must_use]
    pub fn is_svg_element(&self) -> bool {
        self.in_svg_namespace()
    }

    #[must_use]
    pub(crate) fn is_backdrop_node(&self) -> bool {
        self.flags.get(NodeFlags::BACKDROP)
    }

    #[must_use]
    pub(crate) fn is_anonymous_box(&self) -> bool {
        self.flags.get(NodeFlags::ANONYMOUS)
    }

    #[must_use]
    pub(crate) const fn custom_state(&self) -> CustomElementState {
        self.flags.custom_state()
    }

    pub(crate) fn set_custom_state(&mut self, state: CustomElementState) {
        self.flags.set_custom_state(state);
    }

    #[must_use]
    pub(crate) const fn is_container_timing_root(&self) -> bool {
        self.flags.get(NodeFlags::CONTAINER_TIMING_ROOT)
    }

    pub(crate) fn set_container_timing_root(&mut self, on: bool) -> bool {
        self.flags.set(NodeFlags::CONTAINER_TIMING_ROOT, on)
    }

    #[must_use]
    pub(crate) const fn is_container_timing_ignored(&self) -> bool {
        self.flags.get(NodeFlags::CONTAINER_TIMING_IGNORE)
    }

    pub(crate) fn set_container_timing_ignored(&mut self, on: bool) {
        self.flags.set(NodeFlags::CONTAINER_TIMING_IGNORE, on);
    }

    #[must_use]
    pub(crate) const fn self_or_ancestor_has_container_timing(&self) -> bool {
        self.flags
            .get(NodeFlags::SELF_OR_ANCESTOR_HAS_CONTAINER_TIMING)
    }

    pub(crate) fn note_container_timing_ancestor(&mut self) -> bool {
        self.flags
            .set(NodeFlags::SELF_OR_ANCESTOR_HAS_CONTAINER_TIMING, true)
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
            .any(|existing| AsRef::<str>::as_ref(existing) == class)
    }

    pub fn classes(&self) -> impl ExactSizeIterator<Item = &str> {
        self.classes.iter().map(AsRef::as_ref)
    }

    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<Cow<'_, str>> {
        self.attrs
            .iter()
            .find(|(candidate, _)| AsRef::<str>::as_ref(&candidate.0) == name)
            .map(|(name, stored)| self.attribute_text(name, stored))
    }

    pub(crate) fn attr_local_name(&self, name: &LocalName) -> Option<Cow<'_, str>> {
        self.attrs
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(name, stored)| self.attribute_text(name, stored))
    }

    pub(crate) fn plain_attr(&self, name: &LocalName) -> Option<&str> {
        debug_assert!(
            !matches!(name.0.as_ref(), "style" | "class"),
            "`{}` text can be derived; read it with `attr_local_name`",
            name.0
        );
        self.attrs
            .iter()
            .find_map(|(candidate, value)| (candidate == name).then_some(value.as_str()))
    }

    pub(crate) fn attribute_text<'a>(&'a self, name: &LocalName, stored: &'a str) -> Cow<'a, str> {
        if !stored.is_empty() {
            return Cow::Borrowed(stored);
        }
        match name.0.as_ref() {
            "style" => self
                .parsed_inline_style
                .as_ref()
                .map_or(Cow::Borrowed(stored), |block| {
                    let guard = self.document_lock().read();
                    let mut css = String::new();
                    block
                        .read_with(&guard)
                        .to_css(&mut css)
                        .expect("serializing a declaration block into a String cannot fail");
                    Cow::Owned(css)
                }),
            "class" if !self.classes.is_empty() => {
                let mut text = String::new();
                for (index, class) in self.classes.iter().enumerate() {
                    if index > 0 {
                        text.push(' ');
                    }
                    text.push_str(class);
                }
                Cow::Owned(text)
            }
            _ => Cow::Borrowed(stored),
        }
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

    pub fn attributes(&self) -> impl ExactSizeIterator<Item = (&str, Cow<'_, str>)> {
        self.attrs
            .iter()
            .map(|(name, stored)| (name.0.as_ref(), self.attribute_text(name, stored)))
    }

    pub fn attribute_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.attrs.iter().map(|(name, _)| name.0.as_ref())
    }

    #[must_use]
    pub fn element_state(&self) -> ElementState {
        self.element_state
    }

    #[doc(hidden)]
    #[must_use]
    pub fn census_field_sizes() -> &'static [(&'static str, usize, bool)] {
        use std::mem::size_of;
        &[
            ("owner", size_of::<ArenaPtr<()>>(), false),
            ("id", size_of::<NodeId>(), false),
            ("data", size_of::<NodeData>(), false),
            ("payload", size_of::<Option<()>>(), false),
            ("parent", size_of::<Option<NodeSlot>>(), false),
            ("children", size_of::<Vec<NodeSlot>>(), false),
            ("child_index", size_of::<u32>(), false),
            ("child_index_base", size_of::<u32>(), false),
            ("local_name", size_of::<Option<LocalName>>(), true),
            ("classes", size_of::<SmallVec<[Atom; 2]>>(), true),
            ("id_attribute", size_of::<Option<Atom>>(), true),
            ("attrs", size_of::<Vec<(LocalName, String)>>(), true),
            ("element_state", size_of::<ElementState>(), true),
            ("custom_definition", size_of::<Option<DefinitionId>>(), true),
            ("flags", size_of::<NodeFlags>(), false),
            (
                "parsed_inline_style",
                size_of::<Option<Arc<Locked<PropertyDeclarationBlock>>>>(),
                true,
            ),
            (
                "presentational_hints",
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
        if let Some(payload) = &self.payload {
            return payload;
        }
        match &self.data {
            NodeData::Element(_) if self.is_backdrop_node() => {
                panic!("a `::backdrop` node has no payload and is never handed out")
            }
            NodeData::Element(_) if self.is_anonymous_box() => {
                panic!("an anonymous box's node has no payload and is never handed out")
            }
            NodeData::Element(_) | NodeData::Text => {
                unreachable!("the arena's key-zero placeholder is the one payload-less text node")
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
        let mut refresh = match (&*snapshot, live) {
            (None, None) => StyleRefresh::UNCHANGED,
            (Some(old), Some(new)) => {
                if Arc::ptr_eq(old, new) {
                    StyleRefresh::UNCHANGED
                } else {
                    StyleRefresh {
                        changed: true,
                        paragraph_limits_changed: crate::layout::paragraph_limits_changed(old, new),
                        containing_block_changed: false,
                        box_tree_changed: *old.get_display() != *new.get_display()
                            || old.get_counters().content != new.get_counters().content,
                        first: false,
                    }
                }
            }
            (old, _) => StyleRefresh {
                changed: true,
                paragraph_limits_changed: false,
                containing_block_changed: false,
                box_tree_changed: true,
                first: old.is_none(),
            },
        };
        if !refresh.changed {
            return refresh;
        }
        let old = std::mem::replace(snapshot, live.cloned());
        drop(data);
        if let (Some(old), NodeData::Element(Some(new))) = (old, &self.data) {
            let role = |style: &ComputedValues| {
                (
                    crate::layout::establishes_fixed_containing_block(self, style),
                    crate::layout::establishes_absolute_containing_block(self, style),
                )
            };
            refresh.containing_block_changed = role(&old) != role(new);
        }
        self.refresh_basis_free();
        refresh
    }

    #[inline]
    pub(crate) fn layout_reads_percentage_basis(&self) -> bool {
        !self.flags.get(NodeFlags::BASIS_FREE)
    }

    #[inline]
    pub(crate) fn views_own_layout_style(&self, style: &ComputedValues) -> bool {
        match &self.data {
            NodeData::Element(Some(own)) => std::ptr::eq(own.as_ref(), style),
            _ => true,
        }
    }

    fn refresh_basis_free(&mut self) {
        let free = match &self.data {
            NodeData::Element(Some(style)) => !crate::layout::reads_percentage_basis(self, style),
            _ => false,
        };
        self.flags.set(NodeFlags::BASIS_FREE, free);
    }

    pub(crate) fn layout_computed_style_arc(&self) -> Option<&Arc<ComputedValues>> {
        self.layout_computed_style()?;
        let NodeData::Element(snapshot) = &self.data else {
            return None;
        };
        snapshot.as_ref()
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

    pub(crate) fn replace_pseudo_style(
        &mut self,
        style: Arc<ComputedValues>,
    ) -> Option<Arc<ComputedValues>> {
        let NodeData::Element(snapshot) = &mut self.data else {
            unreachable!("a generated box's node is an element node");
        };
        *self.stylo_data_present.get_mut() = true;
        self.style_data.borrow_mut().styles.primary = Some(style.clone());
        let old = snapshot.replace(style);
        self.refresh_basis_free();
        old
    }

    #[must_use]
    pub(crate) fn natural_size(&self) -> NaturalSize {
        match self.content.as_deref() {
            Some(NodeContent::Replaced(replaced)) => replaced.natural_size,
            _ => NaturalSize::NONE,
        }
    }

    pub(crate) fn is_lynx_text(&self) -> bool {
        self.flags.get(NodeFlags::LYNX_TEXT)
    }

    pub(crate) fn set_placeholder_pseudo(&mut self, on: bool) -> bool {
        self.flags.set(NodeFlags::PLACEHOLDER_PSEUDO, on)
    }

    pub(crate) fn implemented_pseudo_element(&self) -> Option<PseudoElement> {
        if !self.flags.get(NodeFlags::PLACEHOLDER_PSEUDO) {
            return None;
        }
        self.containing_shadow_root()
            .is_some()
            .then_some(PseudoElement::Placeholder)
    }

    pub(crate) fn is_replaced(&self) -> bool {
        matches!(self.content.as_deref(), Some(NodeContent::Replaced(_)))
    }

    pub(crate) fn set_natural_size(&mut self, natural_size: NaturalSize) -> bool {
        if let Some(NodeContent::Replaced(replaced)) = self.content.as_deref_mut() {
            if replaced.natural_size == natural_size {
                return false;
            }
            replaced.natural_size = natural_size;
            return true;
        }
        self.content = Some(Box::new(NodeContent::Replaced(Box::new(Replaced {
            natural_size,
            picture: None,
        }))));
        true
    }

    #[must_use]
    pub(crate) fn picture(&self) -> Option<&PictureKey> {
        match self.content.as_deref() {
            Some(NodeContent::Replaced(replaced)) => replaced.picture.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn set_picture(
        &mut self,
        value: Option<(PictureKey, NaturalSize)>,
    ) -> Option<PictureKey> {
        let Some((picture, natural_size)) = value else {
            let previous = self.picture().cloned();
            if self.is_replaced() {
                self.content = None;
            }
            return previous;
        };
        if let Some(NodeContent::Replaced(replaced)) = self.content.as_deref_mut() {
            replaced.natural_size = natural_size;
            return replaced.picture.replace(picture);
        }
        self.content = Some(Box::new(NodeContent::Replaced(Box::new(Replaced {
            natural_size,
            picture: Some(picture),
        }))));
        None
    }

    #[cfg(feature = "layout-test-utils")]
    pub(crate) fn test_leaf_metrics(&self) -> Option<LeafMetrics> {
        match self.content.as_deref() {
            Some(NodeContent::Test(metrics)) => Some(**metrics),
            _ => None,
        }
    }

    #[cfg(feature = "layout-test-utils")]
    pub(crate) fn set_test_leaf_metrics(&mut self, metrics: LeafMetrics) {
        self.content = Some(Box::new(NodeContent::Test(Box::new(metrics))));
    }

    pub(crate) fn set_literal_text(&mut self, text: Option<Box<str>>) {
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

    pub(crate) fn animates_opacity(&self) -> bool {
        self.styling_data().animates.load(Ordering::Relaxed) & ANIMATES_OPACITY != 0
    }

    pub(crate) fn animates_transform(&self) -> bool {
        self.styling_data().animates.load(Ordering::Relaxed) & ANIMATES_TRANSFORM != 0
    }

    pub(crate) fn replace_animates(&self, bits: u8) -> u8 {
        self.styling_data().animates.swap(bits, Ordering::Relaxed)
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
    pub fn index_in_parent(&self) -> Option<usize> {
        if self.is_shadow_root() {
            return None;
        }
        Some(self.index_in(self.arenas().at(self.parent?)))
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
        let parent = tree.at(self.parent?);
        let index = self.index_in(parent);
        debug_assert_eq!(
            parent.children.get(index),
            Some(&self.id),
            "node must appear in its parent's child list at its stored index"
        );
        let sibling = *parent.children.get(index.checked_add_signed(offset)?)?;
        Some(tree.at(sibling))
    }

    #[inline]
    pub(crate) fn index_in(&self, parent: &Self) -> usize {
        self.child_index.wrapping_sub(parent.child_index_base) as usize
    }

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
            .field(
                "has_presentational_hints",
                &self.presentational_hints.is_some(),
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
    use std::sync::atomic::AtomicIsize;

    use seldon::text::FontBlob;
    use stylo::dom::TElement;

    use super::*;
    use crate::tree::document::Document;

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn document_only_state_stays_out_of_the_primary_node_stride() {
        const PRE_BOXING_NODE_DATA_SIZE: usize = 32;
        const PRE_BOXING_NODE_STRIDE: usize = 408;
        const PRE_STATIC_SPLIT_NODE_STRIDE: usize = 368;

        assert_eq!(std::mem::size_of::<NodeData>(), 16);
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
            animation_dirty_descendants: AtomicBool,
            may_have_animations: AtomicBool,
            snapshot_flags: AtomicU8,
            children_to_process: AtomicIsize,
        }

        assert_eq!(std::mem::size_of::<PreviousStylingData>(), 32);
        assert_eq!(std::mem::size_of::<StylingData>(), 16);
        assert_eq!(
            std::mem::size_of::<usize>() + std::mem::size_of::<AtomicIsize>(),
            32 - 16
        );
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

        let (_, slot) = document.layout_state_mut().text_store_parts(text_slot);
        assert!(slot.is_none());
    }

    #[test]
    fn the_harvest_reevaluates_whether_layout_reads_the_basis() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let element = document.create_element("view", ());
        document.append_child(root, element);
        let run = document.create_text_node("run", ());
        document.append_child(element, run);
        let reads = |document: &Document<()>| {
            document
                .get(element)
                .unwrap()
                .layout_reads_percentage_basis()
        };
        assert!(reads(&document), "never styled: conservatively reads it");

        document.set_inline_style(element, "display: flex; width: 50%");
        document.layout();
        assert!(reads(&document));
        assert!(
            document.get(run).unwrap().layout_reads_percentage_basis(),
            "a text node has no style and reads it"
        );

        document.set_inline_style(element, "display: flex; width: 10px");
        document.layout();
        assert!(!reads(&document));

        document.set_inline_style(element, "display: flex; width: 10px; padding-left: 5%");
        document.layout();
        assert!(reads(&document));
    }
}
