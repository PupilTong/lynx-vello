//! The [`Document`] — one NodeId-aligned arena set: a fixed-address DOM/style
//! tree beside independently mutable layout/text state.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use hughie::geometry::Size;
use hughie::text::{TextContext, TextLayoutStore};
use hughie::tree::{LayoutInput, LayoutSlot};
use rustc_hash::FxHashSet;
use slab::Slab;
use stylo::LocalName;
use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::stylesheets::UrlExtraData;

use crate::style::damage::StyleDamage;
use crate::style::engine::StyleEngine;
use crate::tree::custom::CustomElementRegistry;
use crate::tree::node::Node;

pub type NodeId = usize;

pub(crate) const DOCUMENT_NODE_ID: NodeId = 0;
pub(crate) const DOCUMENT_ELEMENT_NODE_ID: NodeId = 1;
const INITIAL_NODE_CAPACITY: usize = 8;

pub(crate) enum PayloadSlot<T> {
    Document,
    ShadowRoot,
    Node(T),
}

#[inline]
pub(crate) fn slab_get_for_live_node<V>(slab: &Slab<V>, id: NodeId) -> &V {
    slab.get(id)
        .expect("live primary node must have matching arena state")
}

/// The fixed-address, document-owned arena set. `nodes` selects each `NodeId`;
/// the payload slab inserts/removes in exactly the same order and asserts that
/// its own free list returns that same key.
pub(crate) struct TreeArenas<T> {
    pub(crate) nodes: Slab<Node<T>>,
    pub(crate) payloads: Slab<PayloadSlot<T>>,
}

impl<T> TreeArenas<T> {
    fn new() -> Self {
        Self {
            nodes: Slab::with_capacity(INITIAL_NODE_CAPACITY),
            payloads: Slab::with_capacity(INITIAL_NODE_CAPACITY),
        }
    }

    #[expect(
        clippy::inline_always,
        reason = "keep the synchronized slab inserts in the node-allocation hot path"
    )]
    #[inline(always)]
    fn insert_side_state(&mut self, id: NodeId, payload: PayloadSlot<T>) {
        assert_eq!(self.payloads.vacant_key(), id);
        assert_eq!(self.payloads.insert(payload), id);
    }

    fn remove_side_state(&mut self, id: NodeId) -> PayloadSlot<T> {
        self.payloads
            .try_remove(id)
            .expect("removed element/text node must have payload-arena state")
    }
}

#[derive(Default)]
pub(crate) struct NodeLayoutState {
    pub(crate) slot: LayoutSlot,
    pub(crate) text: Option<Box<TextLayoutStore>>,
    pub(crate) scroll_offset: euclid::default::Vector2D<f32>,
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
    pub(crate) painter: RefCell<crate::paint::painter::Painter>,
    pending_snapshots: SnapshotMap,
    relayout_roots: Vec<PendingRelayout>,
    relayout_root_ids: FxHashSet<NodeId>,
    shadow_roots: usize,
    pub(crate) custom_elements: CustomElementRegistry<T>,
    node_removal_epoch: u64,
    visual_epoch: u64,
    layout_dirty: bool,
    layout_root_dirty: bool,
    last_layout_inputs: Option<(Size<f32>, f32)>,
    input: crate::input::InputState,
}

impl<T: fmt::Debug> fmt::Debug for Document<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("document_element", &self.document_element().id())
            .field("style_engine", &self.style_engine)
            .field("nodes", &self.tree.nodes)
            .finish_non_exhaustive()
    }
}

impl<T> Document<T> {
    /// Creates a document with a permanent document element.
    #[must_use]
    pub fn new(device: crate::style::device::Device, root_tag: &str, root_payload: T) -> Self {
        let style_engine = StyleEngine::new(device.into_stylo(), about_blank_url_data());
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
        let mut document = Self {
            style_engine,
            tree,
            layout,
            painter: RefCell::new(crate::paint::painter::Painter::default()),
            pending_snapshots: SnapshotMap::new(),
            relayout_roots: Vec::new(),
            relayout_root_ids: FxHashSet::default(),
            shadow_roots: 0,
            custom_elements: CustomElementRegistry::default(),
            node_removal_epoch: 0,
            visual_epoch: 0,
            layout_dirty: false,
            layout_root_dirty: false,
            last_layout_inputs: None,
            input: crate::input::InputState::default(),
        };
        let root = document.create_element(root_tag, root_payload);
        assert_eq!(
            root, DOCUMENT_ELEMENT_NODE_ID,
            "the document element must occupy slab slot one"
        );
        document.live_node_mut(DOCUMENT_NODE_ID).children.push(root);
        document.live_node_mut(root).parent = Some(DOCUMENT_NODE_ID);
        document.mark_subtree_dirty(root);
        document.invalidate_layout(root);
        document
    }

    pub(crate) fn input_state_mut(&mut self) -> &mut crate::input::InputState {
        &mut self.input
    }

    pub(crate) const fn style_engine(&self) -> &StyleEngine {
        &self.style_engine
    }

    pub(crate) const fn style_engine_mut(&mut self) -> &mut StyleEngine {
        &mut self.style_engine
    }

    pub(crate) const fn style_and_tree_parts(&mut self) -> (&mut StyleEngine, &mut Slab<Node<T>>) {
        (&mut self.style_engine, &mut self.tree.nodes)
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
        self.note_visual_mutation();
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

    pub(crate) fn visual_parts(&self) -> (&TreeArenas<T>, &DocumentLayoutState) {
        (&self.tree, &self.layout)
    }

    #[must_use]
    pub(crate) fn node_removal_epoch(&self) -> u64 {
        self.node_removal_epoch
    }

    #[must_use]
    pub(crate) fn visual_epoch(&self) -> u64 {
        self.visual_epoch
    }

    pub(crate) fn note_visual_mutation(&mut self) {
        self.visual_epoch += 1;
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

    /// The document element — the permanent root element created with the
    /// document itself. The document node's child list is structurally
    /// immutable after construction: this is its only child, forever.
    #[must_use]
    pub fn document_element(&self) -> &Node<T> {
        self.tree
            .nodes
            .get(DOCUMENT_ELEMENT_NODE_ID)
            .expect("the document element is never removed")
    }

    pub(crate) fn begin_flush_phase(&self) -> FlushPhaseToken {
        assert!(
            !self.custom_elements_are_draining(),
            "a custom element lifecycle callback cannot flush styles: the flush asserts single \
             entry and runs Stylo's parallel traversal"
        );
        let flag = std::sync::Arc::clone(self.root_node().flush_flag());
        let was = flag.swap(true, Ordering::AcqRel);
        assert!(!was, "flush re-entered on a document already being flushed");
        FlushPhaseToken { flag }
    }

    pub fn create_element(&mut self, tag: &str, payload: T) -> NodeId {
        let local_name = LocalName::from(tag);
        let base = self.begin_reactions();
        let id = self.allocate_node(PayloadSlot::Node(payload), {
            let local_name = local_name.clone();
            |owner, id| Node::new_element(owner, id, local_name)
        });
        self.pin_node(id);
        self.note_custom_element_created(id, &local_name);
        self.drain_reactions(base);
        self.unpin_node(id);
        id
    }

    pub fn create_text_node(&mut self, text: impl Into<String>, payload: T) -> NodeId {
        let text = text.into();
        self.allocate_node(PayloadSlot::Node(payload), |owner, id| {
            Node::new_text(owner, id, text)
        })
    }

    pub(crate) fn allocate_node(
        &mut self,
        payload: PayloadSlot<T>,
        make: impl FnOnce(*mut TreeArenas<T>, NodeId) -> Node<T>,
    ) -> NodeId {
        let owner = std::ptr::from_mut::<TreeArenas<T>>(self.tree.as_mut());
        let entry = self.tree.nodes.vacant_entry();
        let id = entry.key();
        entry.insert(make(owner, id));
        self.tree.insert_side_state(id, payload);
        self.layout.insert(id);
        id
    }

    #[must_use]
    pub(crate) fn has_shadow_roots(&self) -> bool {
        self.shadow_roots != 0
    }

    pub(crate) fn note_shadow_root_added(&mut self) {
        self.shadow_roots += 1;
    }

    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node<T>> {
        self.tree.nodes.get(id)
    }

    #[must_use]
    pub(crate) fn contains_node(&self, id: NodeId) -> bool {
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
    pub(crate) fn child_position(&self, parent: NodeId, child: NodeId) -> Option<usize> {
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
            self.get(parent)
                .is_some_and(|node| node.is_element() || node.is_shadow_root()),
            "insert_before: parent must be a live element or shadow root"
        );
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "insert_before: the document node cannot be reparented"
        );
        assert!(
            !self.get(child).is_some_and(Node::is_shadow_root),
            "insert_before: a shadow root is attached to its host, not inserted"
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

        let base = self.begin_reactions();
        self.detach_inner(child);
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
        let appended = index + 1 == self.live_node_mut(parent).children.len();

        self.note_moved_subtree(child);
        self.note_slot_assignment_inserted(parent, child, appended);
        self.note_child_list_change(parent, index);
        self.invalidate_layout(child);
        let connected = self.has_custom_element_definitions() && self.is_connected(child);
        self.note_custom_elements_inserted(child, connected);
        self.drain_reactions(base);
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, child, None);
    }

    pub fn detach(&mut self, child: NodeId) {
        let base = self.begin_reactions();
        self.detach_inner(child);
        self.drain_reactions(base);
    }

    fn detach_inner(&mut self, child: NodeId) {
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "Document::detach cannot detach the document node"
        );
        assert_ne!(
            child, DOCUMENT_ELEMENT_NODE_ID,
            "Document::detach cannot detach the permanent document element"
        );
        assert!(
            !self.get(child).is_some_and(Node::is_shadow_root),
            "Document::detach cannot detach a shadow root from its host"
        );
        let old_parent = self
            .get(child)
            .expect("stale NodeId passed to Document::detach")
            .parent_id();
        let Some(parent) = old_parent else {
            return;
        };
        let was_connected = self.has_custom_element_definitions() && self.is_connected(parent);

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
        self.note_slot_assignment_removed(parent, child);

        debug_assert_ne!(
            parent, DOCUMENT_NODE_ID,
            "only the permanent document element parents to the document node"
        );
        self.note_child_list_change(parent, removed_index);
        self.note_custom_elements_removed(child, was_connected);
    }

    pub fn remove_subtree(&mut self, id: NodeId) -> Vec<T> {
        assert_ne!(
            id, DOCUMENT_NODE_ID,
            "Document::remove_subtree cannot remove the document node"
        );
        assert_ne!(
            id, DOCUMENT_ELEMENT_NODE_ID,
            "Document::remove_subtree cannot remove the permanent document element"
        );
        assert!(
            !self.get(id).is_some_and(Node::is_shadow_root),
            "Document::remove_subtree cannot remove a shadow root on its own"
        );
        self.assert_subtree_not_pinned(id);
        let base = self.begin_reactions();
        self.detach_inner(id);
        self.pin_node(id);
        self.drain_reactions(base);
        self.unpin_node(id);
        assert!(
            self.get(id).is_some_and(|node| node.parent_id().is_none()),
            "Document::remove_subtree: a disconnected callback re-attached the subtree being \
             removed"
        );
        self.assert_subtree_not_pinned(id);
        self.node_removal_epoch += 1;
        self.note_visual_mutation();
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
                if let Some(root) = node.shadow_root_id() {
                    stack.push(root);
                }
                if node.is_shadow_root() {
                    self.shadow_roots -= 1;
                }
            }
            self.layout.remove(current);
            self.forget_reactions(current);
            match self.tree.remove_side_state(current) {
                PayloadSlot::Node(payload) => removed.push(payload),
                PayloadSlot::ShadowRoot => {}
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
                crate::tree::node::SNAPSHOT_PRESENT,
                "queued snapshots must be present and unhandled before a flush"
            );
        }
        std::mem::replace(&mut self.pending_snapshots, SnapshotMap::new())
    }

    pub(crate) fn harvest_flush<F>(
        &mut self,
        root: NodeId,
        mut snapshots: SnapshotMap,
        sink: &mut F,
    ) where
        F: FnMut(NodeId, StyleDamage),
    {
        self.retain_unhandled_snapshots(&mut snapshots);
        debug_assert!(
            self.pending_snapshots.is_empty(),
            "Document mutation cannot enqueue snapshots during an exclusive style flush"
        );
        self.pending_snapshots = snapshots;

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
                flags & crate::tree::node::SNAPSHOT_PRESENT,
                0,
                "snapshot queue entry lost its present flag during traversal"
            );
            if flags & crate::tree::node::SNAPSHOT_HANDLED != 0 {
                node.clear_snapshot_flags();
                false
            } else {
                true
            }
        });
    }

    fn harvest_style_damage<F>(&mut self, stack: &mut Vec<NodeId>, sink: &mut F)
    where
        F: FnMut(NodeId, StyleDamage),
    {
        while let Some(current) = stack.pop() {
            let harvested = {
                let (harvested, descend) = {
                    let Some(node) = self.tree.nodes.get_mut(current) else {
                        continue;
                    };
                    let mut refreshed = None;
                    let harvested = node.stylo_data_mut().and_then(|wrapper| {
                        let mut data = wrapper.borrow_mut();
                        refreshed.clone_from(&data.styles.primary);
                        let damage = data.damage;
                        data.clear_restyle_state();
                        (!damage.is_empty()).then(|| StyleDamage::from(damage))
                    });
                    let style_changed = node.refresh_layout_style(refreshed);
                    let dirty = node.styling.dirty_descendants.get_mut();
                    (harvested, std::mem::replace(dirty, false) || style_changed)
                };
                if descend {
                    let node = self
                        .tree
                        .nodes
                        .get(current)
                        .expect("the node was live one statement ago");
                    stack.extend_from_slice(node.flat_children());
                }
                harvested
            };
            let Some(damage) = harvested else {
                continue;
            };
            if damage.needs_relayout() {
                if let Some(element) = self.tree.nodes.get(current) {
                    for &child_id in element.flat_children() {
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

/// Marks the lifetime of a Stylo flush phase.
pub(crate) struct FlushPhaseToken {
    flag: std::sync::Arc<AtomicBool>,
}

impl Drop for FlushPhaseToken {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use euclid::{Scale, Size2D};
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
    use crate::standards_device;

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

    pub(crate) fn device() -> crate::style::device::Device {
        standards_device(
            MediaType::screen(),
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
    fn the_document_owns_render_resources_and_schedules_its_scene() {
        let mut document = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { width: 10px; height: 10px; background-color: red; }",
            crate::StylesheetOrigin::Author,
        );
        assert!(document.needs_render());
        assert!(document.render());
        assert!(!document.needs_render());
        assert!(!document.render());
        assert!(!document.scene().encoding().draw_tags.is_empty());

        let _ = document.images_mut().remove_url("missing");
        assert!(document.needs_render());
        assert!(document.render());
    }

    #[test]
    fn slabs_follow_primary_node_lifetime_and_id_reuse() {
        let mut document: Document<u32> = Document::new(device(), "page", 1);
        let id = document.create_element("view", 7);

        assert!(document.tree.nodes.contains(id));
        assert!(matches!(
            document.tree.payloads.get(id),
            Some(PayloadSlot::Node(7))
        ));
        assert!(document.layout.nodes.get(id).is_some());

        assert_eq!(document.remove_subtree(id), vec![7]);
        assert!(!document.tree.nodes.contains(id));
        assert!(document.tree.payloads.get(id).is_none());
        assert!(document.layout.nodes.get(id).is_none());
        assert_eq!(document.tree.nodes.vacant_key(), id);
        assert_eq!(document.tree.payloads.vacant_key(), id);
        assert_eq!(document.layout.nodes.vacant_key(), id);

        let reused = document.create_text_node("replacement", 11);
        assert_eq!(reused, id, "the primary slab should reuse its vacant ID");
        assert_eq!(document.get(reused).unwrap().payload(), &11);
        assert_eq!(document.layout_cache_is_empty(reused), Some(true));
        assert_eq!(
            document
                .get(reused)
                .unwrap()
                .styling_data()
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
    fn remove_subtree_prunes_the_parked_id_set_so_a_reused_slot_is_not_stale() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let a = document.document_element().id();
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
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(".hot { color: red; }", crate::StylesheetOrigin::Author);
        let root = document.document_element().id();
        let connected = document.create_element("view", ());
        let detached = document.create_element("view", ());
        document.append_child(root, connected);
        document.append_child(root, detached);
        document.flush_styles_with_damage_sink(&mut |_, _| {});

        document.detach(detached);
        document.flush_styles_with_damage_sink(&mut |_, _| {});
        document.set_classes(detached, "hot");
        document.set_classes(connected, "hot");
        assert_eq!(document.pending_snapshots.len(), 2);

        document.flush_styles_with_damage_sink(&mut |_, _| {});

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
            crate::tree::node::SNAPSHOT_PRESENT
        );
        assert_eq!(snapshot_flags(&document, connected), 0);

        document.append_child(root, detached);
        document.flush_styles_with_damage_sink(&mut |_, _| {});
        assert!(document.pending_snapshots.is_empty());
        assert_eq!(snapshot_flags(&document, detached), 0);
    }

    #[test]
    fn snapshot_queue_coalesces_and_subtree_removal_purges_reusable_ids() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let root = document.document_element().id();
        let removed = document.create_element("view", ());
        let descendant = document.create_element("view", ());
        document.append_child(removed, descendant);
        document.append_child(root, removed);
        document.flush_styles_with_damage_sink(&mut |_, _| {});

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
