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

use crate::render::image::ImageRole;
use crate::tree::custom::{CustomElementState, DefinitionId};
use crate::tree::document::{DOCUMENT_NODE_ID, NodeId, NodeSlot, TreeArenas};
use crate::tree::shadow::{ShadowLinks, ShadowRootData, ShadowRootMode};

pub(crate) const SNAPSHOT_PRESENT: u8 = 1 << 0;
pub(crate) const SNAPSHOT_HANDLED: u8 = 1 << 1;

/// [`StylingData::animates`] bits: a current or in-effect animation or
/// transition on the element names `opacity`, or `transform`.
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
    /// [`ANIMATES_OPACITY`] and [`ANIMATES_TRANSFORM`]. Web Animations makes
    /// such an element behave as if `will-change` named the property, so the
    /// stacking, group, Backdrop Root and containing-block rules read these
    /// rather than the document's animation map. The animation driver owns
    /// them.
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

/// What a node holds besides its tree links, boxed on the node because most
/// nodes hold nothing.
///
/// Every variant is one pointer wide or narrower, so the box a text node
/// allocates is the 16 B of its text's fat pointer: the replaced and test
/// variants box their wider payloads a second time rather than widen every
/// text node's allocation to theirs.
enum NodeContent {
    Text(Box<str>),
    Replaced(Box<Replaced>),
    #[cfg(feature = "layout-test-utils")]
    Test(Box<LeafMetrics>),
}

/// A replaced element's content: the bitmap it presents and where it comes
/// from.
struct Replaced {
    /// The dimensions of the bitmap this element presents — whichever of
    /// its two sources that is.
    ///
    /// Not independent of the sources: a completed load carries the
    /// image's own dimensions, and the document recomputes this from
    /// whichever source the element now draws.
    natural_size: NaturalSize,
    /// The image source the paint walk resolves against the document's
    /// image registry.
    source: Option<Box<str>>,
    /// The source drawn while `source` has no pixels, requested
    /// concurrently with it and never falling back from it: a source that
    /// loads suppresses this one for good.
    placeholder: Option<Box<str>>,
}

/// Per-node bits that each cost a padded byte of the node stride as a `bool`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NodeFlags(u8);

impl NodeFlags {
    const CUSTOM_SUBTREE_MAY_CONTAIN: u8 = 1 << 0;
    const MAY_OVERRIDE_CHILDREN: u8 = 1 << 1;
    const LYNX_TEXT: u8 = 1 << 2;
    const PLACEHOLDER_PSEUDO: u8 = 1 << 3;
    const LINE_IF_EMPTY: u8 = 1 << 4;
    /// A `::backdrop` box's node ([`crate::tree::top_layer`]).
    pub(crate) const BACKDROP: u8 = 1 << 5;
    /// An anonymous block box's node ([`crate::tree::anonymous`]).
    pub(crate) const ANONYMOUS: u8 = 1 << 6;

    const fn get(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    /// Sets `bit` to `on`, answering what it was.
    fn set(&mut self, bit: u8, on: bool) -> bool {
        let was = self.get(bit);
        if on {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
        was
    }
}

/// A single node in a [`Document`](crate::tree::document::Document) tree.
pub struct Node<T> {
    owner: *mut TreeArenas<T>,
    /// This node's handle: both its identity and the arena position it
    /// occupies, because those are now one thing. A walk that follows
    /// `parent`/`children` indexes the arena directly — there is no id table
    /// to resolve through, which is what keeps a walk step to one load.
    id: NodeId,
    data: NodeData,
    /// What the embedder filed this node with: `Some` for every element and
    /// text node it created, `None` for the nodes nobody hands out — the
    /// document node, a shadow root, a `::backdrop` box's node, an anonymous
    /// box's. A unit payload sits in the stride's padding.
    payload: Option<T>,

    pub(crate) parent: Option<NodeSlot>,
    pub(crate) children: Vec<NodeSlot>,
    pub(crate) local_name: Option<LocalName>,
    pub(crate) classes: SmallVec<[Atom; 2]>,
    pub(crate) id_attribute: Option<Atom>,
    pub(crate) attrs: Vec<(LocalName, String)>,
    pub(crate) element_state: ElementState,

    pub(crate) custom_definition: Option<DefinitionId>,
    pub(crate) custom_state: CustomElementState,
    flags: NodeFlags,

    pub(crate) parsed_inline_style: Option<Arc<Locked<PropertyDeclarationBlock>>>,
    pub(crate) presentational_hints: Option<Arc<Locked<PropertyDeclarationBlock>>>,

    pub(crate) shadow: Option<Box<ShadowLinks>>,

    style_data: ElementDataWrapper,
    stylo_data_present: AtomicBool,

    pub(crate) styling: StylingData,

    content: Option<Box<NodeContent>>,
}

/// What a post-flush style swap moved on one element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent verdicts the harvest reads one by one, not a state machine"
)]
pub(crate) struct StyleRefresh {
    /// Whether the element's computed style is a different `Arc` than before.
    pub(crate) changed: bool,
    /// Whether the paragraph's effective custom-property limits changed.
    /// These layout inputs are outside Stylo's longhand damage comparison.
    pub(crate) paragraph_limits_changed: bool,
    /// Whether the element started or stopped establishing the containing
    /// block of its absolute or fixed descendants. Its own relayout damage
    /// does not reach a positioned box deeper than a child, so the harvest
    /// hands this to [`crate::tree::document::Document::invalidate_containing_block`].
    pub(crate) containing_block_changed: bool,
    /// Whether the boxes the element generates may have changed shape: a
    /// first or last style, or a different `display` or `content`. Either
    /// decides whether the element is loose text of its parent's items, a
    /// transparent box, or a box of its own, so the anonymous boxes on both
    /// sides of it are re-derived ([`crate::tree::anonymous`]).
    pub(crate) box_tree_changed: bool,
}

impl StyleRefresh {
    pub(crate) const UNCHANGED: Self = Self {
        changed: false,
        paragraph_limits_changed: false,
        containing_block_changed: false,
        box_tree_changed: false,
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

    pub(crate) fn new_text(owner: *mut TreeArenas<T>, id: NodeId, text: Box<str>) -> Self {
        Self::new(owner, id, NodeData::Text, None, Some(text))
    }

    /// An element node the tree generates rather than an embedder creates —
    /// a `::backdrop` box's or an anonymous box's — marked by `marker`
    /// ([`NodeFlags::BACKDROP`] or [`NodeFlags::ANONYMOUS`]). It carries no
    /// payload.
    pub(crate) fn new_generated_element(
        owner: *mut TreeArenas<T>,
        id: NodeId,
        local_name: LocalName,
        marker: u8,
    ) -> Self {
        debug_assert!(
            marker == NodeFlags::BACKDROP || marker == NodeFlags::ANONYMOUS,
            "a generated element is a backdrop or an anonymous box"
        );
        let mut node = Self::new_element(owner, id, local_name);
        node.flags.set(marker, true);
        node
    }

    /// Files the embedder's payload on a node it creates.
    #[must_use]
    pub(crate) fn with_payload(mut self, payload: T) -> Self {
        self.payload = Some(payload);
        self
    }

    /// Takes the embedder's payload off a node the arenas have freed.
    pub(crate) fn into_payload(self) -> Option<T> {
        self.payload
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
        text: Option<Box<str>>,
    ) -> Self {
        Self {
            owner,
            id,
            data,
            payload: None,
            parent: None,
            children: Vec::new(),
            local_name,
            classes: SmallVec::new(),
            id_attribute: None,
            attrs: Vec::new(),
            element_state: ElementState::empty(),
            custom_definition: None,
            custom_state: CustomElementState::default(),
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
        // SAFETY: `owner` is the address of the `TreeArenas<T>` holding this
        // node — taken by `ptr::from_mut` at construction, never stored again,
        // and fixed for the document's life because `Document` boxes the arena
        // set. The arenas therefore outlive every node inside them, including
        // one `remove_node` has handed back. The reborrow is shared, and every
        // caller reaches it from a `&Node` already lent out of these same
        // arenas, so no `&mut TreeArenas<T>` is live across it.
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
            &*self.owner
        }
    }

    #[inline]
    pub(crate) fn styling_data(&self) -> &StylingData {
        &self.styling
    }

    #[must_use]
    pub(crate) fn custom_subtree_may_contain(&self) -> bool {
        self.flags.get(NodeFlags::CUSTOM_SUBTREE_MAY_CONTAIN)
    }

    /// Marks this node's shadow-including subtree as possibly containing a
    /// custom element, returning whether it was already marked.
    pub(crate) fn mark_custom_subtree_may_contain(&mut self) -> bool {
        self.flags.set(NodeFlags::CUSTOM_SUBTREE_MAY_CONTAIN, true)
    }

    /// Whether `tree::anonymous` may hold an override of this node's layout
    /// children — the one test `LayoutTree::children` makes before it looks
    /// one up. Set with the override; a stale `true` costs a lookup that
    /// finds nothing, so only clearing it is best effort.
    #[must_use]
    pub(crate) fn may_override_children(&self) -> bool {
        self.flags.get(NodeFlags::MAY_OVERRIDE_CHILDREN)
    }

    pub(crate) fn set_may_override_children(&mut self, may: bool) {
        self.flags.set(NodeFlags::MAY_OVERRIDE_CHILDREN, may);
    }

    /// Records that this element is a Lynx text element, from its custom
    /// element definition ([`crate::tree::custom::CustomElement::is_lynx_text`]):
    /// called once, where the element is given its definition.
    pub(crate) fn mark_lynx_text(&mut self) {
        self.flags.set(NodeFlags::LYNX_TEXT, true);
    }

    /// Whether this element's own paragraph keeps one empty line when it
    /// has no content, whether or not it is an editing host
    /// ([`crate::tree::document::Document::set_has_line_if_empty`]).
    pub(crate) fn has_line_if_empty(&self) -> bool {
        self.flags.get(NodeFlags::LINE_IF_EMPTY)
    }

    /// Records [`Self::has_line_if_empty`], answering what it was.
    pub(crate) fn set_has_line_if_empty(&mut self, on: bool) -> bool {
        self.flags.set(NodeFlags::LINE_IF_EMPTY, on)
    }

    /// Whether this is a `::backdrop` box's node ([`crate::tree::top_layer`]).
    #[must_use]
    pub(crate) fn is_backdrop_node(&self) -> bool {
        self.flags.get(NodeFlags::BACKDROP)
    }

    /// Whether this is an anonymous block box's node
    /// ([`crate::tree::anonymous`]).
    #[must_use]
    pub(crate) fn is_anonymous_box(&self) -> bool {
        self.flags.get(NodeFlags::ANONYMOUS)
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
            .any(|existing| AsRef::<str>::as_ref(existing) == class)
    }

    pub fn classes(&self) -> impl ExactSizeIterator<Item = &str> {
        self.classes.iter().map(AsRef::as_ref)
    }

    /// One attribute's value, as `getAttribute` reads it.
    ///
    /// Borrowed as stored, but for a `style` or `class` value that a write
    /// through the parsed form left to be serialized on read
    /// (`attribute_text`).
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<Cow<'_, str>> {
        self.attr_local_name(&LocalName::from(name))
    }

    pub(crate) fn attr_local_name(&self, name: &LocalName) -> Option<Cow<'_, str>> {
        self.attrs
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(name, stored)| self.attribute_text(name, stored))
    }

    /// The value of an attribute whose text is always stored as written:
    /// any attribute but `style` and `class` ([`Self::attribute_text`]).
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

    /// An attribute's value, from what is stored for it.
    ///
    /// `style` and `class` each have a parsed form — the inline declaration
    /// block, the class list — and their text is stored only while it is not
    /// what that form serializes to: the text a `setAttribute` wrote, kept as
    /// written. A write through the parsed form (one inline property, an
    /// inline-style record, a single class) and a class list written in its
    /// canonical form store an empty value instead, and the text is then the
    /// block's CSSOM serialization or the class list joined by single spaces —
    /// the text that write would otherwise have stored. An empty value with
    /// nothing parsed behind it is the empty attribute.
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

    /// Every attribute with its value ([`Self::attribute`]), in the order
    /// the element acquired them.
    pub fn attributes(&self) -> impl ExactSizeIterator<Item = (&str, Cow<'_, str>)> {
        self.attrs
            .iter()
            .map(|(name, stored)| (name.0.as_ref(), self.attribute_text(name, stored)))
    }

    /// Every attribute's name, in the order the element acquired them.
    pub fn attribute_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.attrs.iter().map(|(name, _)| name.0.as_ref())
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
            ("owner", size_of::<*mut TreeArenas<()>>(), false),
            ("id", size_of::<NodeId>(), false),
            ("data", size_of::<NodeData>(), false),
            ("payload", size_of::<Option<()>>(), false),
            ("parent", size_of::<Option<NodeSlot>>(), false),
            ("children", size_of::<Vec<NodeSlot>>(), false),
            ("local_name", size_of::<Option<LocalName>>(), true),
            ("classes", size_of::<SmallVec<[Atom; 2]>>(), true),
            ("id_attribute", size_of::<Option<Atom>>(), true),
            ("attrs", size_of::<Vec<(LocalName, String)>>(), true),
            ("element_state", size_of::<ElementState>(), true),
            ("custom_definition", size_of::<Option<DefinitionId>>(), true),
            ("custom_state", size_of::<CustomElementState>(), true),
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
                    }
                }
            }
            // A first style has no old role to flip from: the element's
            // descendants have never been laid out under it.
            _ => StyleRefresh {
                changed: true,
                paragraph_limits_changed: false,
                containing_block_changed: false,
                box_tree_changed: true,
            },
        };
        if !refresh.changed {
            return refresh;
        }
        let old = std::mem::replace(snapshot, live.cloned());
        drop(data);
        // Both roles read the element too (its root-ness, its transform
        // animation bit, its content-visibility relevance), none of which a
        // restyle moves, so evaluating each style against the same node
        // isolates what the style change did.
        if let (Some(old), NodeData::Element(Some(new))) = (old, &self.data) {
            let role = |style: &ComputedValues| {
                (
                    crate::layout::establishes_fixed_containing_block(self, style),
                    crate::layout::establishes_absolute_containing_block(self, style),
                )
            };
            refresh.containing_block_changed = role(&old) != role(new);
        }
        refresh
    }

    /// [`Self::layout_computed_style`] as the `Arc` the snapshot holds, for a
    /// reader that keeps it or compares it by pointer.
    pub(crate) fn layout_computed_style_arc(&self) -> Option<&Arc<ComputedValues>> {
        // Its divergence check.
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

    /// Stores a style the cascade did not produce for this node — a lazily
    /// cascaded pseudo-element style, or an anonymous box's inherited one —
    /// as its computed style: Stylo's primary style and the layout snapshot
    /// together, so the snapshot's divergence check above holds. Returns the
    /// old one.
    ///
    /// Only for a `::backdrop` node ([`crate::tree::top_layer`]) or an
    /// anonymous box's ([`crate::tree::anonymous`]): neither is ever in a
    /// parent's child list, so no traversal reaches it and no harvest swaps
    /// its style; this is the one writer.
    pub(crate) fn replace_pseudo_style(
        &mut self,
        style: Arc<ComputedValues>,
    ) -> Option<Arc<ComputedValues>> {
        let NodeData::Element(snapshot) = &mut self.data else {
            unreachable!("a generated box's node is an element node");
        };
        *self.stylo_data_present.get_mut() = true;
        self.style_data.borrow_mut().styles.primary = Some(style.clone());
        snapshot.replace(style)
    }

    #[must_use]
    pub(crate) fn natural_size(&self) -> NaturalSize {
        match self.content.as_deref() {
            Some(NodeContent::Replaced(replaced)) => replaced.natural_size,
            _ => NaturalSize::NONE,
        }
    }

    /// The URL this element holds in `role`, of the two it may hold at once.
    #[must_use]
    pub(crate) fn image_source(&self, role: ImageRole) -> Option<&str> {
        let Some(NodeContent::Replaced(replaced)) = self.content.as_deref() else {
            return None;
        };
        match role {
            ImageRole::Source => replaced.source.as_deref(),
            ImageRole::Placeholder => replaced.placeholder.as_deref(),
        }
    }

    /// Whether this element is a Lynx text element: one that lays out as a
    /// Lynx paragraph whatever its computed `display` is, other than `none`
    /// and `contents` ([`crate::layout::display_mode`]).
    pub(crate) fn is_lynx_text(&self) -> bool {
        self.flags.get(NodeFlags::LYNX_TEXT)
    }

    /// Records whether this element's `pseudo` attribute is
    /// [`PLACEHOLDER_PSEUDO_ID`](crate::tree::shadow::PLACEHOLDER_PSEUDO_ID), answering
    /// what it was. Only `Document::set_attribute` and
    /// `Document::remove_attribute` write it, with the attribute.
    pub(crate) fn set_placeholder_pseudo(&mut self, on: bool) -> bool {
        self.flags.set(NodeFlags::PLACEHOLDER_PSEUDO, on)
    }

    /// The pseudo-element this element implements for its shadow host: an
    /// element of a shadow tree whose `pseudo` attribute is
    /// [`PLACEHOLDER_PSEUDO_ID`](crate::tree::shadow::PLACEHOLDER_PSEUDO_ID) is the host's
    /// `::placeholder` (`docs/terminus-architecture.md`, "Shadow DOM and custom elements"). The
    /// attribute is read through a node bit, so an element without it answers in one load; the
    /// walk to the containing shadow root is paid by the elements that carry it.
    ///
    /// A light-tree element with the attribute implements nothing: a
    /// pseudo-element has an originating element, and here that is the
    /// host, as it is in Blink, whose UA shadow trees name their placeholder
    /// with this attribute.
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
        if self.natural_size() == natural_size && self.is_replaced() {
            return false;
        }
        let (source, placeholder) = self.take_image_sources();
        self.content = Some(Box::new(NodeContent::Replaced(Box::new(Replaced {
            natural_size,
            source,
            placeholder,
        }))));
        true
    }

    /// Sets the URL this element holds in `role`, making it replaced content
    /// — and, when it was the element's last source, ordinary content again.
    ///
    /// Either role alone is enough to make the element replaced, since a
    /// placeholder is exactly a bitmap drawn in the content box.
    ///
    /// Clearing a source a node never had is a no-op rather than a
    /// conversion: `is_replaced` is a layout input — it forces
    /// `DisplayMode::Leaf` and hides every child — so turning an ordinary
    /// element into a childless replaced box is not what "there is no image
    /// here" should mean. Which is the same reason taking the last source off
    /// one undoes it: an element with no bitmap to draw is not a box drawing
    /// nothing.
    ///
    /// An element made replaced by a natural size alone is left alone, since
    /// clearing the source it never had changes nothing about it.
    pub(crate) fn set_image_source(&mut self, role: ImageRole, value: Option<&str>) -> bool {
        if self.image_source(role) == value && (self.is_replaced() || value.is_none()) {
            return false;
        }
        let (source, placeholder) = self.take_image_sources();
        let value = value.map(Box::from);
        match role {
            ImageRole::Source => self.set_image_sources(value, placeholder),
            ImageRole::Placeholder => self.set_image_sources(source, value),
        }
        true
    }

    /// Installs both sources, keeping the natural size the document maintains
    /// from whichever one is drawn.
    ///
    /// With neither source left the element stops being replaced, and its
    /// natural size goes with the content it described: `is_replaced` forces
    /// `DisplayMode::Leaf`, so a box with no bitmap to draw must not keep it.
    fn set_image_sources(&mut self, source: Option<Box<str>>, placeholder: Option<Box<str>>) {
        if source.is_none() && placeholder.is_none() {
            debug_assert!(
                self.is_replaced(),
                "the no-op guards let only a replaced element reach here with no source"
            );
            self.content = None;
            return;
        }
        let natural_size = self.natural_size();
        self.content = Some(Box::new(NodeContent::Replaced(Box::new(Replaced {
            natural_size,
            source,
            placeholder,
        }))));
    }

    /// Moves both sources out of the current content, so rewriting one half of
    /// a replaced element's state neither copies the strings nor drops them.
    fn take_image_sources(&mut self) -> (Option<Box<str>>, Option<Box<str>>) {
        match self.content.as_deref_mut() {
            Some(NodeContent::Replaced(replaced)) => {
                (replaced.source.take(), replaced.placeholder.take())
            }
            _ => (None, None),
        }
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

    /// Whether a current or in-effect animation or transition names
    /// `opacity` (see [`StylingData::animates`]).
    pub(crate) fn animates_opacity(&self) -> bool {
        self.styling_data().animates.load(Ordering::Relaxed) & ANIMATES_OPACITY != 0
    }

    /// Whether a current or in-effect animation or transition names
    /// `transform` (see [`StylingData::animates`]).
    pub(crate) fn animates_transform(&self) -> bool {
        self.styling_data().animates.load(Ordering::Relaxed) & ANIMATES_TRANSFORM != 0
    }

    /// Replaces the [`StylingData::animates`] bits, answering the previous
    /// ones.
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
        // A handle is identity and storage position in one 8-byte value, and
        // `Option<NodeId>` has a niche where `Option<usize>` had none, so the
        // parent link costs 8 bytes rather than 16. Presentational hints add
        // one optional Arc; the declaration block allocates only when used.
        // No node keeps Stylo's postorder child counter (`StylingData`).
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
        assert_eq!(std::mem::size_of::<StylingData>(), 16);
        // The snapshot pointer, and the postorder child counter no traversal
        // here uses (`RecalcStyle::needs_postorder_traversal` is false).
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

        // A text node never gains a paragraph slot: it is content of the
        // block above it, and the block lives on the element that
        // establishes it.
        let (_, slot) = document.layout_state_mut().text_store_parts(text_slot);
        assert!(slot.is_none());
    }
}
