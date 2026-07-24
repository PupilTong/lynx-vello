//! The [`Document`] — one NodeId-aligned arena set: a fixed-address DOM/style
//! tree beside independently mutable layout/text state.

use std::fmt;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use neutron_star::geometry::Size;
use neutron_star::text::{TextContext, TextLayoutStore};
use neutron_star::tree::{LayoutInput, LayoutSlot};
use rustc_hash::FxHashSet;
use slab::Slab;
use stylo::LocalName;
use stylo::device::Device;
use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::stylesheets::UrlExtraData;

use crate::damage::StyleDamage;
use crate::engine::StyleEngine;
use crate::node::{Node, StylingData};

pub type NodeId = usize;

pub const DOCUMENT_NODE_ID: NodeId = 0;
const INITIAL_NODE_CAPACITY: usize = 8;

pub(crate) enum PayloadSlot<T> {
    Document,
    Node(T),
}

#[inline]
pub(crate) fn slab_get_for_live_node<V>(slab: &Slab<V>, id: NodeId) -> &V {
    debug_assert!(
        slab.contains(id),
        "live primary node must have matching arena state"
    );
    #[expect(
        unsafe_code,
        reason = "elide redundant bounds/vacancy checks for a live-node slot"
    )]
    unsafe {
        slab.get_unchecked(id)
    }
}

/// The fixed-address, document-owned arena set. `nodes` selects each `NodeId`;
/// the other slabs insert/remove in exactly the same order and assert that
/// their own free lists return that same key.
pub(crate) struct TreeArenas<T> {
    pub(crate) nodes: Slab<Node<T>>,
    pub(crate) payloads: Slab<PayloadSlot<T>>,
    pub(crate) styling: Slab<StylingData>,
}

impl<T> TreeArenas<T> {
    fn new() -> Self {
        Self {
            nodes: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            payloads: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            styling: Slab::with_capacity(INITIAL_NODE_CAPACITY),
        }
    }

    #[expect(
        clippy::inline_always,
        reason = "keep the synchronized slab inserts in the node-allocation hot path"
    )]
    #[inline(always)]
    fn insert_side_state(&mut self, id: NodeId, payload: PayloadSlot<T>) {
        assert_eq!(self.payloads.vacant_key(), id);
        assert_eq!(self.styling.vacant_key(), id);
        assert_eq!(self.payloads.insert(payload), id);
        assert_eq!(self.styling.insert(StylingData::default()), id);
    }

    fn remove_side_state(&mut self, id: NodeId) -> PayloadSlot<T> {
        let payload = self
            .payloads
            .try_remove(id)
            .expect("removed element/text node must have payload-arena state");
        self.styling
            .try_remove(id)
            .expect("removed node must have styling-arena state");
        payload
    }
}

#[derive(Default)]
pub(crate) struct NodeLayoutState {
    pub(crate) slot: LayoutSlot,
    pub(crate) text: Option<Box<TextLayoutStore>>,
}

pub(crate) struct DocumentLayoutState {
    pub(crate) nodes: Slab<NodeLayoutState>,
    pub(crate) text_context: Option<Box<TextContext>>,
}

impl DocumentLayoutState {
    fn new() -> Self {
        Self {
            nodes: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            text_context: None,
        }
    }

    fn insert(&mut self, id: NodeId) {
        assert_eq!(self.nodes.vacant_key(), id);
        assert_eq!(self.nodes.insert(NodeLayoutState::default()), id);
    }

    fn remove(&mut self, id: NodeId) {
        self.nodes
            .try_remove(id)
            .expect("removed node must have layout-arena state");
    }

    pub(crate) fn text_parts(&mut self, id: NodeId) -> (&mut TextContext, &mut TextLayoutStore) {
        let Self {
            nodes,
            text_context,
        } = self;
        let context = text_context
            .get_or_insert_with(|| Box::new(TextContext::new()))
            .as_mut();
        let artifacts = nodes
            .get_mut(id)
            .expect("live node must have layout-arena state")
            .text
            .get_or_insert_with(|| Box::new(TextLayoutStore::default()))
            .as_mut();
        (context, artifacts)
    }

    pub(crate) fn clear_layout_cache(&mut self, id: NodeId) {
        let node = self
            .nodes
            .get_mut(id)
            .expect("live node must have layout-arena state");
        node.slot.clear_layout_cache();
        if let Some(artifacts) = node.text.as_deref_mut() {
            artifacts.invalidate();
        }
    }
}

pub(crate) fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

/// A containment boundary scheduled for a committed-input relayout.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingRelayout {
    pub node_id: NodeId,
    pub input: LayoutInput,
}

/// One DOM tree, including its actual document node at primary-arena slot
/// zero.
pub struct Document<T> {
    style_engine: StyleEngine,
    tree: Box<TreeArenas<T>>,
    layout: DocumentLayoutState,
    /// Pre-mutation state exists only while invalidation is pending. Keeping
    /// the payloads here leaves one byte-sized lifecycle flag, rather than a
    /// nullable snapshot pointer, in every live node's styling slot.
    pending_snapshots: SnapshotMap,
    relayout_roots: Vec<PendingRelayout>,
    relayout_root_ids: FxHashSet<NodeId>,
    layout_dirty: bool,
    layout_root_dirty: bool,
    last_layout_inputs: Option<(Size<f32>, f32)>,
}

impl<T: fmt::Debug> fmt::Debug for Document<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("root_element", &self.root_element().map(Node::id))
            .field("style_engine", &self.style_engine)
            .field("nodes", &self.tree.nodes)
            .finish_non_exhaustive()
    }
}

impl<T> Document<T> {
    #[must_use]
    pub fn new(device: Device) -> Self {
        Self::with_url_data(device, about_blank_url_data())
    }

    #[must_use]
    pub fn with_url_data(device: Device, url_data: UrlExtraData) -> Self {
        let style_engine = StyleEngine::with_url_data(device, url_data);
        let lock = style_engine.lock();
        let url_data = style_engine.url_data();
        let mut tree = Box::new(TreeArenas::new());
        let owner = std::ptr::from_mut::<TreeArenas<T>>(tree.as_mut());
        let root = tree.nodes.insert(Node::new_document(owner, lock, url_data));
        assert_eq!(
            root, DOCUMENT_NODE_ID,
            "the DOM document node must occupy slab slot zero"
        );
        tree.insert_side_state(root, PayloadSlot::Document);
        let mut layout = DocumentLayoutState::new();
        layout.insert(root);
        Self {
            style_engine,
            tree,
            layout,
            pending_snapshots: SnapshotMap::new(),
            relayout_roots: Vec::new(),
            relayout_root_ids: FxHashSet::default(),
            layout_dirty: false,
            layout_root_dirty: false,
            last_layout_inputs: None,
        }
    }

    pub(crate) const fn style_engine(&self) -> &StyleEngine {
        &self.style_engine
    }

    pub(crate) const fn style_engine_mut(&mut self) -> &mut StyleEngine {
        &mut self.style_engine
    }

    pub(crate) fn record_relayout_root(&mut self, id: NodeId, committed_input: LayoutInput) {
        self.relayout_roots.push(PendingRelayout {
            node_id: id,
            input: committed_input,
        });
        self.relayout_root_ids.insert(id);
    }

    pub(crate) fn relayout_roots(&self) -> &[PendingRelayout] {
        &self.relayout_roots
    }

    pub(crate) fn clear_relayout_roots(&mut self) {
        self.relayout_roots.clear();
        self.relayout_root_ids.clear();
    }

    pub(crate) fn layout_needs_pass(&self, viewport: Size<f32>, scale: f32) -> bool {
        self.layout_dirty || self.last_layout_inputs != Some((viewport, scale))
    }

    pub(crate) fn layout_requires_full_pass(&self, viewport: Size<f32>, scale: f32) -> bool {
        self.layout_root_dirty || self.last_layout_inputs != Some((viewport, scale))
    }

    pub(crate) fn mark_layout_complete(&mut self, viewport: Size<f32>, scale: f32) {
        self.layout_dirty = false;
        self.layout_root_dirty = false;
        self.last_layout_inputs = Some((viewport, scale));
    }

    pub(crate) fn mark_layout_dirty(&mut self, reached_root: bool) {
        self.layout_dirty = true;
        self.layout_root_dirty |= reached_root;
    }

    pub(crate) fn tree(&self) -> &Slab<Node<T>> {
        &self.tree.nodes
    }

    pub(crate) fn tree_mut(&mut self) -> &mut Slab<Node<T>> {
        &mut self.tree.nodes
    }

    pub(crate) fn live_node_mut(&mut self, id: NodeId) -> &mut Node<T> {
        self.tree
            .nodes
            .get_mut(id)
            .expect("stale NodeId passed to a Document mutation method")
    }

    pub(crate) fn layout_state(&self) -> &DocumentLayoutState {
        &self.layout
    }

    pub(crate) fn layout_state_mut(&mut self) -> &mut DocumentLayoutState {
        &mut self.layout
    }

    pub(crate) fn layout_parts(
        &mut self,
    ) -> (&TreeArenas<T>, &mut DocumentLayoutState, &FxHashSet<NodeId>) {
        (&self.tree, &mut self.layout, &self.relayout_root_ids)
    }

    pub(crate) fn layout_data_mut(
        &mut self,
    ) -> impl Iterator<Item = (NodeId, &mut NodeLayoutState)> {
        self.layout.nodes.iter_mut()
    }

    pub(crate) fn snapshot_storage(&mut self) -> (&Slab<Node<T>>, &mut SnapshotMap) {
        (&self.tree.nodes, &mut self.pending_snapshots)
    }

    #[must_use]
    pub fn root_node(&self) -> &Node<T> {
        self.tree
            .nodes
            .get(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
    }

    #[must_use]
    pub fn root_element(&self) -> Option<&Node<T>> {
        self.root_node().children().find(|node| node.is_element())
    }

    pub(crate) fn begin_flush_phase(&self) -> FlushPhaseToken {
        use std::sync::atomic::Ordering;

        let flag = self.root_node().flush_flag();
        let was = flag.swap(true, Ordering::AcqRel);
        assert!(!was, "flush re-entered on a document already being flushed");
        FlushPhaseToken {
            flag: NonNull::from(flag),
        }
    }

    pub fn create_element(&mut self, tag: &str, payload: T) -> NodeId {
        let local_name = LocalName::from(tag);
        self.allocate_node(payload, |owner, id| {
            Node::new_element(owner, id, local_name)
        })
    }

    pub fn create_text_node(&mut self, text: impl Into<String>, payload: T) -> NodeId {
        let text = text.into();
        self.allocate_node(payload, |owner, id| Node::new_text(owner, id, text))
    }

    fn allocate_node(
        &mut self,
        payload: T,
        make: impl FnOnce(*mut TreeArenas<T>, NodeId) -> Node<T>,
    ) -> NodeId {
        let owner = std::ptr::from_mut::<TreeArenas<T>>(self.tree.as_mut());
        let entry = self.tree.nodes.vacant_entry();
        let id = entry.key();
        entry.insert(make(owner, id));
        self.tree.insert_side_state(id, PayloadSlot::Node(payload));
        self.layout.insert(id);
        id
    }

    pub fn append_document_element(&mut self, child: NodeId) {
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "Document::append_document_element cannot append the document to itself"
        );
        assert!(
            self.get(child).is_some_and(Node::is_element),
            "Document::append_document_element requires a live element"
        );
        if self.root_element().map(Node::id) == Some(child) {
            return;
        }
        assert!(
            self.root_element().is_none(),
            "Document::append_document_element: a document may have only one element child"
        );

        self.detach(child);
        self.live_node_mut(DOCUMENT_NODE_ID).children.push(child);
        self.live_node_mut(child).parent = Some(DOCUMENT_NODE_ID);
        self.mark_subtree_dirty(child);
        self.invalidate_layout(child);
    }

    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node<T>> {
        self.tree.nodes.get(id)
    }

    #[must_use]
    pub fn contains_node(&self, id: NodeId) -> bool {
        self.tree.nodes.contains(id)
    }

    #[must_use]
    pub fn is_connected(&self, id: NodeId) -> bool {
        let mut current = id;
        loop {
            let Some(node) = self.get(current) else {
                return false;
            };
            if current == DOCUMENT_NODE_ID {
                return true;
            }
            let Some(parent) = node.parent_id() else {
                return false;
            };
            current = parent;
        }
    }

    #[must_use]
    pub fn child_position(&self, parent: NodeId, child: NodeId) -> Option<usize> {
        self.get(parent)?
            .child_ids()
            .iter()
            .position(|&candidate| candidate == child)
    }

    #[must_use]
    pub fn is_ancestor(&self, ancestor: NodeId, descendant: NodeId) -> bool {
        let mut next = self.get(descendant).and_then(Node::parent_id);
        while let Some(current) = next {
            if current == ancestor {
                return true;
            }
            next = self.get(current).and_then(Node::parent_id);
        }
        false
    }

    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: Option<NodeId>) {
        debug_assert!(self.contains_node(parent), "insert_before: stale parent");
        debug_assert!(self.contains_node(child), "insert_before: stale child");
        assert!(
            self.get(parent).is_some_and(Node::is_element),
            "insert_before: parent must be a live element"
        );
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "insert_before: the document node cannot be reparented"
        );
        debug_assert!(child != parent, "insert_before: child == parent");
        debug_assert!(
            !self.is_ancestor(child, parent),
            "insert_before: linking a node under its own descendant"
        );
        debug_assert!(
            before != Some(child),
            "insert_before: reference must differ from child"
        );

        self.detach(child);
        let index = match before {
            None => self
                .get(parent)
                .expect("stale NodeId passed to Document::insert_before")
                .child_ids()
                .len(),
            Some(reference) => self
                .child_position(parent, reference)
                .expect("insert_before reference must be a child of parent"),
        };

        self.live_node_mut(parent).children.insert(index, child);
        self.live_node_mut(child).parent = Some(parent);

        self.note_moved_subtree(child);
        self.note_child_list_change(parent, index);
        self.invalidate_layout(child);
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, child, None);
    }

    pub fn detach(&mut self, child: NodeId) {
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "Document::detach cannot detach the document node"
        );
        let old_parent = self
            .get(child)
            .expect("stale NodeId passed to Document::detach")
            .parent_id();
        let Some(parent) = old_parent else {
            return;
        };

        // Invalidate while the old link is still intact so the walk covers
        // the old parent's dirty spine and observes its containment boundary.
        // A subsequent insertion invalidates again after attaching, covering
        // the new parent's spine as well.
        self.invalidate_layout(child);

        let removed_index = {
            let parent_node = self
                .tree
                .nodes
                .get_mut(parent)
                .expect("internal tree link must resolve to a live node");
            let index = parent_node
                .children
                .iter()
                .position(|&candidate| candidate == child)
                .expect("child must appear in its parent's child list");
            parent_node.children.remove(index);
            index
        };
        self.live_node_mut(child).parent = None;

        if parent != DOCUMENT_NODE_ID {
            self.note_child_list_change(parent, removed_index);
        }
    }

    pub fn remove_subtree(&mut self, id: NodeId) -> Vec<T> {
        assert_ne!(
            id, DOCUMENT_NODE_ID,
            "Document::remove_subtree cannot remove the document node"
        );
        self.detach(id);
        let mut removed = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let node = self
                .tree
                .nodes
                .get(current)
                .expect("subtree links always resolve while removing");
            let removed_snapshot = self
                .pending_snapshots
                .remove(&OpaqueNode(current))
                .is_some();
            debug_assert_eq!(
                removed_snapshot,
                node.snapshot_present(),
                "the document snapshot queue and node lifecycle flag diverged during removal"
            );
            {
                let node = self
                    .tree
                    .nodes
                    .try_remove(current)
                    .expect("subtree links always resolve while removing");
                stack.extend_from_slice(&node.children);
            }
            self.layout.remove(current);
            match self.tree.remove_side_state(current) {
                PayloadSlot::Node(payload) => removed.push(payload),
                PayloadSlot::Document => unreachable!("the document node cannot be removed"),
            }
        }
        let nodes = &self.tree.nodes;
        self.relayout_roots
            .retain(|pending| nodes.contains(pending.node_id));
        self.relayout_root_ids
            .retain(|&parked_id| nodes.contains(parked_id));
        removed
    }

    pub(crate) fn take_snapshot_map(&mut self) -> SnapshotMap {
        #[cfg(debug_assertions)]
        for opaque in self.pending_snapshots.keys() {
            let node = self
                .tree
                .nodes
                .get(opaque.0)
                .expect("queued snapshot must belong to a live node");
            debug_assert!(node.is_element(), "only elements can own Stylo snapshots");
            debug_assert_eq!(
                node.snapshot_flags(),
                crate::node::SNAPSHOT_PRESENT,
                "queued snapshots must be present and unhandled before a flush"
            );
        }
        std::mem::replace(&mut self.pending_snapshots, SnapshotMap::new())
    }

    pub(crate) fn harvest_flush(
        &mut self,
        root: NodeId,
        mut snapshots: SnapshotMap,
        sink: &mut dyn FnMut(NodeId, StyleDamage),
    ) {
        self.retain_unhandled_snapshots(&mut snapshots);
        debug_assert!(
            self.pending_snapshots.is_empty(),
            "Document mutation cannot enqueue snapshots during an exclusive style flush"
        );
        self.pending_snapshots = snapshots;

        // Every pointer affected by the completed traversal was published by
        // its preorder callback before the driver returned. If traversal
        // unwound, this point is never reached and layout access keeps
        // panicking instead of dereferencing a stale pointer.
        self.root_node().set_layout_styles_ready(true);

        let mut stack = vec![root];
        self.harvest_style_damage(&mut stack, sink);
    }

    fn retain_unhandled_snapshots(&self, snapshots: &mut SnapshotMap) {
        snapshots.retain(|opaque, _| {
            let node = self.tree.nodes.get(opaque.0);
            debug_assert!(node.is_some(), "queued snapshot outlived its node");
            let Some(node) = node else {
                return false;
            };
            let flags = node.snapshot_flags();
            debug_assert_ne!(
                flags & crate::node::SNAPSHOT_PRESENT,
                0,
                "snapshot queue entry lost its present flag during traversal"
            );
            if flags & crate::node::SNAPSHOT_HANDLED != 0 {
                node.clear_snapshot_flags();
                false
            } else {
                true
            }
        });
    }

    fn harvest_style_damage(
        &mut self,
        stack: &mut Vec<NodeId>,
        sink: &mut dyn FnMut(NodeId, StyleDamage),
    ) {
        while let Some(current) = stack.pop() {
            let harvested = {
                let tree = &mut *self.tree;
                let Some(node) = tree.nodes.get_mut(current) else {
                    continue;
                };
                let styling = tree
                    .styling
                    .get_mut(current)
                    .expect("live node must have styling-arena state");
                let harvested = node.stylo_data_mut().and_then(|wrapper| {
                    let mut data = wrapper.borrow_mut();
                    let damage = data.damage;
                    data.clear_restyle_state();
                    (!damage.is_empty()).then(|| StyleDamage::from(damage))
                });
                let descend = styling.dirty_descendants.load(Ordering::Relaxed);
                styling.dirty_descendants.store(false, Ordering::Relaxed);
                if descend {
                    stack.extend_from_slice(&node.children);
                }
                harvested
            };
            let Some(damage) = harvested else {
                continue;
            };
            if damage.needs_relayout() {
                if let Some(element) = self.tree.nodes.get(current) {
                    for &child_id in &element.children {
                        if self
                            .tree
                            .nodes
                            .get(child_id)
                            .is_some_and(Node::is_text_node)
                        {
                            self.layout.clear_layout_cache(child_id);
                        }
                    }
                }
                self.invalidate_layout(current);
                if damage.requires_reconstruction() {
                    let parent = self.get(current).and_then(Node::parent_id);
                    if let Some(parent) = parent {
                        self.invalidate_layout(parent);
                    }
                }
            }
            sink(current, damage);
        }
    }
}

/// RAII marker distinguishing Stylo's own mutation phase from safe external
/// `TElement::mutate_data` access.
pub(crate) struct FlushPhaseToken {
    flag: NonNull<std::sync::atomic::AtomicBool>,
}

impl Drop for FlushPhaseToken {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;

        #[expect(unsafe_code, reason = "clear the document-node traversal flag")]
        unsafe {
            self.flag.as_ref().store(false, Ordering::Release);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use euclid::{Scale, Size2D};
    use stylo::context::QuirksMode;
    use stylo::device::servo::FontMetricsProvider;
    use stylo::font_metrics::FontMetrics;
    use stylo::media_queries::MediaType;
    use stylo::properties::ComputedValues;
    use stylo::properties::style_structs::Font;
    use stylo::queries::values::PrefersColorScheme;
    use stylo::servo::media_features::PointerCapabilities;
    use stylo::values::computed::font::GenericFontFamily;
    use stylo::values::computed::{CSSPixelLength, Length};
    use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};

    use super::*;

    #[derive(Debug)]
    struct NoFonts;

    impl FontMetricsProvider for NoFonts {
        fn query_font_metrics(
            &self,
            _: bool,
            _: &Font,
            _: CSSPixelLength,
            _: QueryFontMetricsFlags,
        ) -> FontMetrics {
            FontMetrics::default()
        }

        fn base_size_for_generic(&self, _: GenericFontFamily) -> Length {
            Length::new(FONT_MEDIUM_PX)
        }
    }

    pub(crate) fn device() -> Device {
        Device::new(
            MediaType::screen(),
            QuirksMode::NoQuirks,
            Size2D::new(800.0, 600.0),
            Size2D::new(800.0, 600.0),
            Scale::new(1.0),
            Box::new(NoFonts),
            ComputedValues::initial_values_with_font_override(Font::initial_values()),
            PrefersColorScheme::Light,
            PointerCapabilities::empty(),
            PointerCapabilities::empty(),
        )
    }

    fn snapshot_flags<T>(document: &Document<T>, id: NodeId) -> u8 {
        document
            .get(id)
            .expect("test node is live")
            .snapshot_flags()
    }

    #[test]
    fn slabs_follow_primary_node_lifetime_and_id_reuse() {
        let mut document: Document<u32> = Document::new(device());
        let id = document.create_element("view", 7);

        assert!(document.tree.nodes.contains(id));
        assert!(matches!(
            document.tree.payloads.get(id),
            Some(PayloadSlot::Node(7))
        ));
        assert!(document.tree.styling.get(id).is_some());
        assert!(document.layout.nodes.get(id).is_some());

        assert_eq!(document.remove_subtree(id), vec![7]);
        assert!(!document.tree.nodes.contains(id));
        assert!(document.tree.payloads.get(id).is_none());
        assert!(document.tree.styling.get(id).is_none());
        assert!(document.layout.nodes.get(id).is_none());
        assert_eq!(document.tree.nodes.vacant_key(), id);
        assert_eq!(document.tree.payloads.vacant_key(), id);
        assert_eq!(document.tree.styling.vacant_key(), id);
        assert_eq!(document.layout.nodes.vacant_key(), id);

        let reused = document.create_text_node("replacement", 11);
        assert_eq!(reused, id, "the primary slab should reuse its vacant ID");
        assert_eq!(document.get(reused).unwrap().payload(), &11);
        assert_eq!(document.layout_cache_is_empty(reused), Some(true));
        assert_eq!(
            document
                .tree
                .styling
                .get(reused)
                .unwrap()
                .snapshot_flags
                .load(Ordering::Relaxed),
            0
        );
        assert!(document.pending_snapshots.is_empty());
    }

    #[test]
    fn payload_size_does_not_change_primary_node_stride() {
        assert_eq!(
            std::mem::size_of::<Node<()>>(),
            std::mem::size_of::<Node<[u8; 1_024]>>()
        );
    }

    #[test]
    #[should_panic(expected = "internal tree links always resolve")]
    fn root_element_panics_on_a_dangling_document_child() {
        let mut document: Document<()> = Document::new(device());
        document
            .tree
            .nodes
            .get_mut(DOCUMENT_NODE_ID)
            .expect("the document node is present")
            .children
            .push(usize::MAX);

        let _ = document.root_element();
    }

    #[test]
    fn remove_subtree_prunes_the_parked_id_set_so_a_reused_slot_is_not_stale() {
        let mut document: Document<()> = Document::new(device());
        let a = document.create_element("view", ());
        document.append_document_element(a);
        let b = document.create_element("view", ());
        document.append_child(a, b);

        document.record_relayout_root(b, LayoutInput::default());
        assert!(document.relayout_root_ids.contains(&b));

        assert_eq!(document.remove_subtree(b).len(), 1);
        assert!(
            !document.relayout_root_ids.contains(&b),
            "the removed id must not remain in the parked set",
        );

        let reused = document.create_element("view", ());
        assert_eq!(reused, b, "the freed slab slot is reused");
        assert!(
            !document.relayout_root_ids.contains(&reused),
            "a reused slab id must not inherit stale parked state",
        );
    }

    #[test]
    fn detached_snapshot_survives_an_unrelated_connected_flush() {
        let mut document: Document<()> = Document::new(device());
        document.add_stylesheet(".hot { color: red; }", crate::StylesheetOrigin::Author);
        let root = document.create_element("page", ());
        let connected = document.create_element("view", ());
        let detached = document.create_element("view", ());
        document.append_child(root, connected);
        document.append_child(root, detached);
        document.append_document_element(root);
        document.flush_styles();

        document.detach(detached);
        document.flush_styles();
        document.set_classes(detached, "hot");
        document.set_classes(connected, "hot");
        assert_eq!(document.pending_snapshots.len(), 2);

        document.flush_styles();

        assert!(
            document
                .pending_snapshots
                .contains_key(&OpaqueNode(detached)),
            "a snapshot outside the traversed document tree must stay pending"
        );
        assert!(
            !document
                .pending_snapshots
                .contains_key(&OpaqueNode(connected)),
            "the handled connected snapshot must be retired"
        );
        assert_eq!(
            snapshot_flags(&document, detached),
            crate::node::SNAPSHOT_PRESENT
        );
        assert_eq!(snapshot_flags(&document, connected), 0);

        document.append_child(root, detached);
        document.flush_styles();
        assert!(document.pending_snapshots.is_empty());
        assert_eq!(snapshot_flags(&document, detached), 0);
    }

    #[test]
    fn snapshot_queue_coalesces_and_subtree_removal_purges_reusable_ids() {
        let mut document: Document<()> = Document::new(device());
        let root = document.create_element("page", ());
        let removed = document.create_element("view", ());
        let descendant = document.create_element("view", ());
        document.append_child(removed, descendant);
        document.append_child(root, removed);
        document.append_document_element(root);
        document.flush_styles();

        document.set_classes(removed, "hot");
        document.set_id_attribute(removed, Some("target"));
        assert_eq!(
            document.pending_snapshots.len(),
            1,
            "multiple pre-flush mutations must refine one snapshot"
        );
        let snapshot = document
            .pending_snapshots
            .iter()
            .find_map(|(opaque, snapshot)| (opaque.0 == removed).then_some(snapshot))
            .unwrap();
        assert!(snapshot.class_changed);
        assert!(snapshot.id_changed);

        document.set_classes(descendant, "nested");
        assert_eq!(document.pending_snapshots.len(), 2);
        assert_eq!(document.remove_subtree(removed).len(), 2);
        assert!(
            document.pending_snapshots.is_empty(),
            "removing a subtree must purge every queued snapshot"
        );

        let reused = document.create_element("replacement", ());
        assert!(
            [removed, descendant].contains(&reused),
            "the primary slab should reuse an ID from the removed subtree"
        );
        assert_eq!(
            snapshot_flags(&document, reused),
            0,
            "a reused ID must not inherit snapshot lifecycle flags"
        );
    }
}
