//! The [`Document`] — one NodeId-aligned arena set: a fixed-address DOM/style
//! tree beside independently mutable layout/text state.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use rustc_hash::FxHashSet;
use seldon::geometry::Size;
use seldon::tree::{LayoutInput, LayoutOutput};
use stylo::LocalName;
use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::stylesheets::UrlExtraData;

use crate::style::damage::StyleDamage;
use crate::style::engine::StyleEngine;
pub use crate::tree::arena::NodeId;
pub(crate) use crate::tree::arena::{
    DOCUMENT_ELEMENT_NODE_ID, DOCUMENT_NODE_ID, DeferredContainer, DocumentLayoutState,
    NodeLayoutState, NodeSlot, StickyContainingBlock, TreeArenas,
};
use crate::tree::custom::CustomElementRegistry;
use crate::tree::node::Node;

pub(crate) fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

pub(crate) fn engine_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("chrome://engine/").expect("a valid URL"))
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RelayoutKind {
    Boundary,
    InPlace { previous: LayoutOutput },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingRelayout {
    pub node_id: NodeId,
    pub input: LayoutInput,
    pub kind: RelayoutKind,
}

pub struct Document<T> {
    style_engine: StyleEngine,
    style_pool: Option<std::rc::Rc<crate::style::pool::StylePool>>,
    tree: Box<TreeArenas<T>>,
    layout: DocumentLayoutState,
    pub(crate) painter: RefCell<crate::paint::painter::FramePainter>,
    pub(crate) images: crate::render::image::ImageRegistry,
    pub(crate) content_visibility_changes: Vec<crate::visual::ContentVisibilityChange>,
    pub(crate) intersections: crate::visual::intersection::IntersectionObservers<T>,
    pub(crate) resizes: crate::visual::resize::ResizeObservers<T>,
    pub(crate) listeners: crate::event::listeners::Listeners,
    pub(crate) editing: crate::editing::EditingState,
    pub(crate) queued_events: Option<std::rc::Rc<dyn crate::event::QueuedEventSink>>,
    pending_snapshots: SnapshotMap,
    relayout_roots: Vec<PendingRelayout>,
    relayout_root_ids: FxHashSet<NodeId>,
    shadow_roots: usize,
    pub(crate) custom_elements: CustomElementRegistry<T>,
    pub(crate) canvases: crate::html::canvas::Canvases,
    animations: crate::style::animation::AnimationDriver,
    commit_id: u64,
    pub(crate) commit_timer: crate::visual::timing::CommitTimer,
    pub(crate) content_detection: crate::visual::timing::ContentDetection,
    pub(crate) container_roots: crate::visual::container_timing::ContainerRoots,
    pub(crate) svg_roots: crate::svg::present::SvgRoots,
    pub(crate) animated_images: crate::svg::image::AnimatedImages,
    visual_dirty: bool,
    layout_dirty: bool,
    layout_root_dirty: bool,
    last_layout_inputs: Option<(Size<f32>, f32)>,
}

impl<T: fmt::Debug> fmt::Debug for Document<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("document_element", &self.document_element().id())
            .field("style_engine", &self.style_engine)
            .field("nodes", &self.tree.nodes())
            .finish_non_exhaustive()
    }
}

impl<T> Document<T> {
    #[must_use]
    pub fn new(device: crate::style::device::Device, root_tag: &str, root_payload: T) -> Self {
        let style_engine = StyleEngine::new(device.into_stylo(), about_blank_url_data());
        let lock = style_engine.lock();
        let url_data = style_engine.url_data();
        let mut tree = Box::new(TreeArenas::new());
        tree.reserve_zero();
        let root = tree.insert_node(|owner, id| Node::new_document(owner, id, lock, url_data));
        assert_eq!(
            root, DOCUMENT_NODE_ID,
            "the DOM document node must take the first id the document ever issues"
        );
        let layout = DocumentLayoutState::new();
        let mut document = Self {
            style_engine,
            style_pool: None,
            tree,
            layout,
            painter: RefCell::new(crate::paint::painter::FramePainter::default()),
            images: crate::render::image::ImageRegistry::default(),
            content_visibility_changes: Vec::new(),
            intersections: crate::visual::intersection::IntersectionObservers::default(),
            resizes: crate::visual::resize::ResizeObservers::default(),
            listeners: crate::event::listeners::Listeners::default(),
            editing: crate::editing::EditingState::default(),
            queued_events: None,
            pending_snapshots: SnapshotMap::new(),
            relayout_roots: Vec::new(),
            relayout_root_ids: FxHashSet::default(),
            shadow_roots: 0,
            custom_elements: CustomElementRegistry::default(),
            canvases: crate::html::canvas::Canvases::default(),
            animations: crate::style::animation::AnimationDriver::default(),
            commit_id: 0,
            commit_timer: crate::visual::timing::CommitTimer::default(),
            content_detection: crate::visual::timing::ContentDetection::Off,
            container_roots: crate::visual::container_timing::ContainerRoots::default(),
            svg_roots: crate::svg::present::SvgRoots::default(),
            animated_images: crate::svg::image::AnimatedImages::default(),
            visual_dirty: true,
            layout_dirty: false,
            layout_root_dirty: false,
            last_layout_inputs: None,
        };
        let root = document.create_element(root_tag, root_payload);
        assert_eq!(
            root, DOCUMENT_ELEMENT_NODE_ID,
            "the document element must take the second id the document ever issues"
        );
        let document_slot = document.live_slot(DOCUMENT_NODE_ID);
        let root_slot = document.live_slot(root);
        document.tree.insert_child(document_slot, 0, root_slot);
        document.mark_subtree_dirty(root);
        document.invalidate_layout(root);
        document
    }

    pub(crate) const fn style_engine(&self) -> &StyleEngine {
        &self.style_engine
    }

    pub fn set_style_pool(
        &mut self,
        pool: std::rc::Rc<crate::style::pool::StylePool>,
    ) -> Option<std::rc::Rc<crate::style::pool::StylePool>> {
        self.style_pool.replace(pool)
    }

    pub(crate) fn style_pool(&self) -> Option<&crate::style::pool::StylePool> {
        self.style_pool.as_deref()
    }

    pub(crate) const fn animations(&self) -> &crate::style::animation::AnimationDriver {
        &self.animations
    }

    pub(crate) const fn animations_mut(&mut self) -> &mut crate::style::animation::AnimationDriver {
        &mut self.animations
    }

    pub(crate) const fn style_engine_mut(&mut self) -> &mut StyleEngine {
        &mut self.style_engine
    }

    pub(crate) const fn style_and_tree_parts(&mut self) -> (&mut StyleEngine, &mut TreeArenas<T>) {
        (&mut self.style_engine, &mut self.tree)
    }

    pub(crate) fn record_relayout_root(
        &mut self,
        id: NodeId,
        committed_input: LayoutInput,
        kind: RelayoutKind,
    ) {
        self.relayout_roots.push(PendingRelayout {
            node_id: id,
            input: committed_input,
            kind,
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

    pub(crate) fn layout_owed(&self) -> bool {
        self.layout_dirty || self.document_element().needs_style_flush()
    }

    pub(crate) fn layout_requires_full_pass(&self, viewport: Size<f32>, scale: f32) -> bool {
        self.layout_root_dirty || self.layout_inputs_changed(viewport, scale)
    }

    pub(crate) fn layout_inputs_changed(&self, viewport: Size<f32>, scale: f32) -> bool {
        self.last_layout_inputs != Some((viewport, scale))
    }

    pub(crate) const fn has_laid_out(&self) -> bool {
        self.last_layout_inputs.is_some()
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

    pub(crate) fn arenas(&self) -> &TreeArenas<T> {
        &self.tree
    }

    pub(crate) fn arenas_mut(&mut self) -> &mut TreeArenas<T> {
        &mut self.tree
    }

    pub(crate) fn live_node_mut(&mut self, id: NodeId) -> &mut Node<T> {
        self.tree
            .get_mut(id)
            .expect("stale NodeId passed to a Document mutation method")
    }

    #[inline]
    pub(crate) fn slot(&self, id: NodeId) -> Option<NodeSlot> {
        self.tree.slot(id)
    }

    #[inline]
    pub(crate) fn live_slot(&self, id: NodeId) -> NodeSlot {
        self.tree.live_slot(id)
    }

    pub(crate) fn layout_state(&self) -> &DocumentLayoutState {
        &self.layout
    }

    pub(crate) fn layout_state_mut(&mut self) -> &mut DocumentLayoutState {
        &mut self.layout
    }

    pub(crate) fn image_apply_parts(
        &mut self,
    ) -> (
        &mut crate::render::image::ImageRegistry,
        &mut Option<Box<seldon::text::TextContext>>,
    ) {
        (&mut self.images, &mut self.layout.text_context)
    }

    pub(crate) fn layout_parts(&mut self) -> (&TreeArenas<T>, &mut DocumentLayoutState) {
        (&self.tree, &mut self.layout)
    }

    pub(crate) fn visual_parts(&self) -> (&TreeArenas<T>, &DocumentLayoutState) {
        (&self.tree, &self.layout)
    }

    #[must_use]
    pub(crate) fn commit_id(&self) -> u64 {
        self.commit_id
    }

    pub(crate) fn next_commit_id(&mut self) -> u64 {
        self.commit_id += 1;
        self.visual_dirty = false;
        self.commit_id
    }

    #[must_use]
    pub(crate) fn visual_dirty(&self) -> bool {
        self.visual_dirty
    }

    pub(crate) fn note_visual_mutation(&mut self) {
        self.visual_dirty = true;
    }

    pub(crate) const fn clear_visual_dirty(&mut self) {
        self.visual_dirty = false;
    }

    pub(crate) fn layout_data_mut(&mut self) -> impl Iterator<Item = &mut NodeLayoutState> {
        self.layout.iter_mut()
    }

    pub(crate) fn snapshot_storage(&mut self) -> (&TreeArenas<T>, &mut SnapshotMap) {
        (&self.tree, &mut self.pending_snapshots)
    }

    pub(crate) fn has_pending_snapshot(&self, opaque: stylo::dom::OpaqueNode) -> bool {
        self.pending_snapshots.contains_key(&opaque)
    }

    #[must_use]
    pub fn root_node(&self) -> &Node<T> {
        self.tree
            .get(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
    }

    #[must_use]
    pub fn document_element(&self) -> &Node<T> {
        self.tree
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
        if tag == crate::svg::SVG_TAG {
            return self.create_svg_namespace_element(tag, payload);
        }
        let local_name = LocalName::from(tag);
        let base = self.begin_reactions();
        let id = self.allocate_node({
            let local_name = local_name.clone();
            |owner, id| Node::new_element(owner, id, local_name).with_payload(payload)
        });
        self.pin_node(id);
        self.note_custom_element_created(id, &local_name);
        self.drain_reactions(base);
        self.unpin_node(id);
        id
    }

    pub fn create_svg_element(&mut self, tag: &str, payload: T) -> NodeId {
        self.create_svg_namespace_element(tag, payload)
    }

    pub(crate) fn create_svg_namespace_element(&mut self, tag: &str, payload: T) -> NodeId {
        let local_name = LocalName::from(tag);
        let id = self.allocate_node(|owner, id| {
            Node::new_svg_element(owner, id, local_name).with_payload(payload)
        });
        if tag == crate::svg::SVG_TAG {
            self.update_svg_natural_size(id);
        }
        id
    }

    pub fn create_text_node(&mut self, text: impl Into<Box<str>>, payload: T) -> NodeId {
        let text = text.into();
        self.allocate_node(|owner, id| Node::new_text(owner, id, text).with_payload(payload))
    }

    pub(crate) fn allocate_node(
        &mut self,
        make: impl FnOnce(*mut TreeArenas<T>, NodeId) -> Node<T>,
    ) -> NodeId {
        self.tree.insert_node(make)
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
        self.tree.get(id)
    }

    #[must_use]
    pub(crate) fn contains_node(&self, id: NodeId) -> bool {
        self.tree.contains(id)
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
        let parent = self.get(parent)?;
        let index = self.get(child)?.index_in(parent);
        (parent.child_slots().get(index) == Some(&child)).then_some(index)
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
        self.unlink_from_parent(child);
        let index = match before {
            None => self
                .get(parent)
                .expect("stale NodeId passed to Document::insert_before")
                .child_slots()
                .len(),
            Some(reference) => self
                .child_position(parent, reference)
                .expect("insert_before reference must be a child of parent"),
        };

        let parent_slot = self.live_slot(parent);
        let child_slot = self.live_slot(child);
        self.tree.insert_child(parent_slot, index, child_slot);
        let appended = index + 1 == self.live(parent).children.len();
        let contains_custom_elements = self.note_custom_subtree_inserted(child);

        self.note_moved_subtree(child);
        self.note_container_timing_inserted(parent, child);
        self.note_slot_assignment_inserted(parent, child, appended);
        self.note_child_list_change(parent, index);
        self.note_child_inserted(parent, child);
        if self.tree.at(child_slot).in_svg_namespace()
            || self.tree.at(parent_slot).in_svg_namespace()
        {
            self.note_svg_change(child);
        }
        self.invalidate_layout(child);
        self.invalidate_hoisted_under(child);
        let connected = contains_custom_elements && self.is_connected(child);
        self.note_custom_elements_inserted(child, connected);
        self.drain_reactions(base);
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, child, None);
    }

    pub fn swap_element(&mut self, a: NodeId, b: NodeId) {
        assert_ne!(a, b, "swap_element: the operands must differ");
        let position = |document: &Self, node: NodeId| {
            let node = document.get(node).expect("swap_element: stale NodeId");
            let parent = node
                .parent_id()
                .expect("swap_element: both operands must be attached");
            (parent, node.next_sibling().map(Node::id))
        };
        let (parent_a, next_a) = position(self, a);
        let (parent_b, next_b) = position(self, b);
        if next_a == Some(b) {
            self.insert_before(parent_a, b, Some(a));
        } else if next_b == Some(a) {
            self.insert_before(parent_b, a, Some(b));
        } else {
            self.insert_before(parent_b, a, Some(b));
            self.insert_before(parent_a, b, next_a);
        }
    }

    pub fn remove_element(&mut self, child: NodeId) {
        let base = self.begin_reactions();
        self.unlink_from_parent(child);
        self.drain_reactions(base);
    }

    fn free_owned_subtree(&mut self, root: NodeId, in_shadow_tree: bool) -> (Option<T>, usize) {
        debug_assert!(
            self.live(root).parent_id().is_none(),
            "Document::free_owned_subtree frees detached nodes only"
        );
        let mut payload = None;
        let mut freed = 0;
        let mut stack = vec![(root, in_shadow_tree)];
        while let Some((current, in_shadow_tree)) = stack.pop() {
            self.assert_not_pinned(current);
            let children: Vec<NodeId> = self.live(current).child_ids().to_vec();
            for child in children {
                if !in_shadow_tree && self.live(child).is_element() {
                    self.unlink_from_doomed_parent(child, current);
                } else {
                    stack.push((child, in_shadow_tree));
                }
            }
            if let Some(shadow_root) = self.live(current).shadow_root_id() {
                stack.push((shadow_root, true));
            }
            let node = self.free_node(current);
            freed += 1;
            if current == root {
                payload = Some(node.into_payload());
            }
        }
        self.prune_relayout_roots();
        (
            payload.expect("the root is the first node the walk frees"),
            freed,
        )
    }

    fn unlink_from_doomed_parent(&mut self, child: NodeId, parent: NodeId) {
        self.tree.remove_child(parent, child);
        self.note_slot_assignment_removed(parent, child);
    }

    fn unlink_from_parent(&mut self, child: NodeId) {
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "the document node cannot be removed: it has no parent"
        );
        assert_ne!(
            child, DOCUMENT_ELEMENT_NODE_ID,
            "the permanent document element cannot be removed from the document node"
        );
        assert!(
            !self.get(child).is_some_and(Node::is_shadow_root),
            "a shadow root cannot be removed from its host"
        );
        let old_parent = self
            .get(child)
            .expect("stale NodeId passed to a Document removal method")
            .parent_id();
        let Some(parent) = old_parent else {
            return;
        };
        let was_connected = self.custom_subtree_may_contain(child) && self.is_connected(parent);

        self.cancel_animations_in_subtree(child);
        self.invalidate_layout(child);

        let removed_index = self.tree.remove_child(parent, child);
        self.note_slot_assignment_removed(parent, child);
        if self.live(parent).in_svg_namespace() {
            self.note_svg_change(parent);
        }

        debug_assert_ne!(
            parent, DOCUMENT_NODE_ID,
            "only the permanent document element parents to the document node"
        );
        self.note_child_list_change(parent, removed_index);
        self.note_child_removed(parent);
        self.note_custom_elements_removed(child, was_connected);
        self.drop_disconnected_top_layer_entries();
        self.fix_up_selection();
    }

    pub fn drop_element(&mut self, id: NodeId) -> T {
        assert_ne!(
            id, DOCUMENT_NODE_ID,
            "Document::drop_element cannot drop the document node"
        );
        assert_ne!(
            id, DOCUMENT_ELEMENT_NODE_ID,
            "Document::drop_element cannot drop the permanent document element"
        );
        assert!(
            !self.get(id).is_some_and(Node::is_shadow_root),
            "Document::drop_element cannot drop a shadow root on its own"
        );
        assert!(
            self.get(id).is_some(),
            "stale NodeId passed to Document::drop_element"
        );
        self.assert_not_pinned(id);
        let in_shadow_tree = self.containing_shadow_root(id).is_some();
        let base = self.begin_reactions();
        self.unlink_from_parent(id);
        self.run_reactions(base);
        let payload = self
            .free_owned_subtree(id, in_shadow_tree)
            .0
            .expect("a node an embedder can name carries its payload");
        self.fire_slot_changes();
        payload
    }

    pub fn drop_subtree(&mut self, id: NodeId) -> Vec<T> {
        assert_ne!(
            id, DOCUMENT_NODE_ID,
            "Document::drop_subtree cannot remove the document node"
        );
        assert_ne!(
            id, DOCUMENT_ELEMENT_NODE_ID,
            "Document::drop_subtree cannot remove the permanent document element"
        );
        assert!(
            !self.get(id).is_some_and(Node::is_shadow_root),
            "Document::drop_subtree cannot remove a shadow root on its own"
        );
        self.assert_subtree_not_pinned(id);
        let base = self.begin_reactions();
        self.unlink_from_parent(id);
        self.run_reactions(base);
        self.note_visual_mutation();
        let mut removed = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let node = self.free_node(current);
            stack.extend(
                node.child_slots()
                    .iter()
                    .map(|&slot| self.tree.at(slot).id()),
            );
            if let Some(root) = node.shadow_root_id() {
                stack.push(root);
            }
            debug_assert!(
                !node.is_backdrop_node() && !node.is_anonymous_box(),
                "a generated box's node is in no child list"
            );
            if let Some(payload) = node.into_payload() {
                removed.push(payload);
            }
        }
        self.prune_relayout_roots();
        self.fire_slot_changes();
        removed
    }

    fn cancel_animations_in_subtree(&mut self, root: NodeId) {
        if self.animations.is_empty() {
            return;
        }
        let mut subtree = Vec::new();
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let Some(node) = self.tree.get(current) else {
                continue;
            };
            node.set_may_have_animations(false);
            node.replace_animates(0);
            subtree.push(current);
            stack.extend(
                node.child_slots()
                    .iter()
                    .map(|&slot| self.tree.at(slot).id()),
            );
            if let Some(shadow) = node.shadow_root_id() {
                stack.push(shadow);
            }
        }
        self.animations.forget(&subtree);
    }

    pub(crate) fn free_node(&mut self, id: NodeId) -> Node<T> {
        let removed_snapshot = self
            .pending_snapshots
            .remove(&OpaqueNode(id.arena_key()))
            .is_some();
        self.animations.forget(&[id]);
        if !self.intersections.is_empty() {
            self.intersections.forget_node(id);
        }
        if !self.resizes.is_empty() {
            self.resizes.forget_node(id);
        }
        if !self.listeners.is_empty() {
            self.listeners.forget_node(id);
        }
        self.forget_anonymous_boxes(id);
        self.forget_svg_root(id);
        let node = self.tree.remove_node(id);
        if let Some(crate::render::picture::PictureKey::Url(source)) = node.picture() {
            self.images.unbind(source);
            self.forget_animated_image(source);
        }
        let slot = id;
        debug_assert_eq!(
            removed_snapshot,
            node.snapshot_present(),
            "the document snapshot queue and node lifecycle flag diverged during removal"
        );
        if node.is_shadow_root() {
            self.shadow_roots -= 1;
        }
        if node.is_container_timing_root() {
            self.forget_container_timing_root(id);
        }
        self.note_freed_editing_node(&node);
        self.layout.remove(slot);
        #[cfg(debug_assertions)]
        self.assert_not_pinned(id);
        self.note_custom_element_freed(id, node.custom_definition);
        node
    }

    fn prune_relayout_roots(&mut self) {
        if self.relayout_roots.is_empty() && self.relayout_root_ids.is_empty() {
            return;
        }
        let tree = &self.tree;
        self.relayout_roots
            .retain(|pending| tree.contains(pending.node_id));
        self.relayout_root_ids
            .retain(|&parked_id| tree.contains(parked_id));
    }

    pub(crate) fn take_snapshot_map(&mut self) -> SnapshotMap {
        #[cfg(debug_assertions)]
        for opaque in self.pending_snapshots.keys() {
            let node = self
                .tree
                .id_at_arena_key(opaque.0)
                .and_then(|id| self.tree.get(id))
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
    ) -> Vec<NodeId>
    where
        F: FnMut(NodeId, StyleDamage),
    {
        self.retain_unhandled_snapshots(&mut snapshots);
        debug_assert!(
            self.pending_snapshots.is_empty(),
            "Document mutation cannot enqueue snapshots during an exclusive style flush"
        );
        self.pending_snapshots = snapshots;

        let mut stack = vec![root];
        self.harvest_style_damage(&mut stack, sink)
    }

    fn retain_unhandled_snapshots(&self, snapshots: &mut SnapshotMap) {
        snapshots.retain(|opaque, _| {
            let node = self
                .tree
                .id_at_arena_key(opaque.0)
                .and_then(|id| self.tree.get(id));
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

    pub(crate) fn invalidate_text_artifact(&mut self, id: NodeId) {
        if let Some(slot) = self.tree.slot(id) {
            self.layout.clear_text_artifact(slot);
        }
    }

    fn harvest_style_damage<F>(&mut self, stack: &mut Vec<NodeId>, sink: &mut F) -> Vec<NodeId>
    where
        F: FnMut(NodeId, StyleDamage),
    {
        let mut containing_blocks = Vec::new();
        let mut anchor_restyled = Vec::new();
        while let Some(current) = stack.pop() {
            let (harvested, refresh, svg) = {
                let (harvested, refresh, descend, anchor_restyle, svg) = {
                    let Some(node) = self.tree.get_mut(current) else {
                        continue;
                    };
                    let damage = node.stylo_data_mut().map(|wrapper| {
                        let mut data = wrapper.borrow_mut();
                        let damage = data.damage;
                        data.clear_restyle_state();
                        damage
                    });
                    let refresh = node.refresh_layout_style();
                    let anchor_restyle = refresh.changed.then(|| {
                        node.layout_computed_style()
                            .is_some_and(crate::layout::anchors::declares_anchor_state)
                    });
                    if refresh.changed {
                        self.animations
                            .timelines
                            .restyled(current, node.layout_computed_style());
                    }
                    let harvested = StyleDamage::from_style_change(
                        damage.unwrap_or_default(),
                        refresh.paragraph_limits_changed,
                    );
                    let svg = node.in_svg_namespace();
                    let dirty = node.styling.dirty_descendants.get_mut();
                    (
                        harvested,
                        refresh,
                        std::mem::replace(dirty, false) || refresh.changed,
                        anchor_restyle,
                        svg,
                    )
                };
                if let Some(declares) = anchor_restyle
                    && (declares || self.tree.anchors().knows(current))
                {
                    anchor_restyled.push(current);
                }
                if descend {
                    let node = self
                        .tree
                        .get(current)
                        .expect("the node was live one statement ago");
                    let arenas = &self.tree;
                    stack.extend(
                        node.flat_children()
                            .iter()
                            .map(|&slot| arenas.at(slot).id()),
                    );
                }
                (harvested, refresh, svg)
            };
            self.note_box_restyle(
                current,
                refresh,
                harvested.is_some_and(StyleDamage::needs_relayout),
            );
            let Some(damage) = harvested else {
                if svg && refresh.first {
                    self.note_svg_first_style(current);
                }
                continue;
            };
            let boxless = svg && self.note_svg_restyle(current);
            if refresh.containing_block_changed {
                containing_blocks.push(current);
            }
            if damage.needs_relayout() && !boxless {
                self.invalidate_layout(current);
            }
            sink(current, damage);
        }
        for id in containing_blocks {
            self.invalidate_containing_block(id);
        }
        anchor_restyled
    }
}

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
    use crate::style::device::standards_device;

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
            crate::style::engine::StylesheetOrigin::Author,
        );
        assert!(document.needs_render());
        assert!(document.render());
        assert!(!document.needs_render());
        assert!(!document.render());
        assert!(!document.scene(&crate::render::image::NoImages).is_empty());

        document.apply_image_events(&[crate::render::image::ImageEvent::Failed {
            source: std::sync::Arc::from("app:///a.png"),
        }]);
        assert!(document.needs_render());
        assert!(document.render());
    }

    fn assert_children(document: &Document<()>, parent: NodeId, expected: &[NodeId]) {
        assert_eq!(document.live(parent).child_ids(), expected);
        for (index, &child) in expected.iter().enumerate() {
            let node = document.live(child);
            assert_eq!(node.parent_id(), Some(parent));
            assert_eq!(document.child_position(parent, child), Some(index));
            assert_eq!(
                node.previous_sibling().map(Node::id),
                index.checked_sub(1).map(|before| expected[before])
            );
            assert_eq!(
                node.next_sibling().map(Node::id),
                expected.get(index + 1).copied()
            );
        }
    }

    #[test]
    fn sibling_navigation_follows_every_child_list_change_under_a_wide_parent() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        let wide = document.create_element("view", ());
        let other = document.create_element("view", ());
        document.append_child(page, wide);
        document.append_child(page, other);
        assert_children(&document, page, &[wide, other]);

        let mut model: Vec<NodeId> = Vec::new();
        let mut other_model: Vec<NodeId> = Vec::new();
        for _ in 0..64 {
            let child = document.create_element("view", ());
            document.append_child(wide, child);
            model.push(child);
        }
        for _ in 0..64 {
            let child = document.create_element("view", ());
            document.insert_before(wide, child, Some(model[0]));
            model.insert(0, child);
        }
        for step in 0..64 {
            let at = (step * 37) % model.len();
            let child = document.create_element("view", ());
            document.insert_before(wide, child, Some(model[at]));
            model.insert(at, child);
        }
        assert_children(&document, wide, &model);

        for step in 0..48 {
            let at = match step % 4 {
                0 => 0,
                1 => model.len() - 1,
                2 => model.len() / 3,
                _ => model.len() * 2 / 3,
            };
            document.remove_element(model.remove(at));
        }
        assert_children(&document, wide, &model);

        for step in 0..48 {
            let from = (step * 29 + 5) % model.len();
            let child = model.remove(from);
            if step % 3 == 2 {
                document.append_child(wide, child);
                model.push(child);
            } else {
                let to = (step * 53 + 11) % model.len();
                document.insert_before(wide, child, Some(model[to]));
                model.insert(to, child);
            }
        }
        assert_children(&document, wide, &model);

        for step in 0..24 {
            let child = model.remove((step * 17) % model.len());
            document.append_child(other, child);
            other_model.push(child);
        }
        for step in 0..8 {
            let child = other_model.remove(step % other_model.len());
            let to = (step * 41) % model.len();
            document.insert_before(wide, child, Some(model[to]));
            model.insert(to, child);
        }
        assert_children(&document, wide, &model);
        assert_children(&document, other, &other_model);

        let pairs = [(0, 1), (5, 90), (model.len() - 1, 3)];
        for (x, y) in pairs {
            document.swap_element(model[x], model[y]);
            model.swap(x, y);
        }
        document.swap_element(model[10], other_model[0]);
        std::mem::swap(&mut model[10], &mut other_model[0]);
        assert_children(&document, wide, &model);
        assert_children(&document, other, &other_model);

        document.drop_element(other);
        for (step, &child) in other_model.iter().enumerate() {
            assert_eq!(document.live(child).parent_id(), None);
            let to = (step * 13) % model.len();
            document.insert_before(wide, child, Some(model[to]));
            model.insert(to, child);
        }
        assert_children(&document, wide, &model);
        assert_children(&document, page, &[wide]);
    }

    #[test]
    fn swap_element_exchanges_positions_within_and_across_parents() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        let a = document.create_element("view", ());
        let b = document.create_element("view", ());
        let c = document.create_element("view", ());
        let inner = document.create_element("view", ());
        document.insert_before(page, a, None);
        document.insert_before(page, b, None);
        document.insert_before(page, c, None);
        document.insert_before(c, inner, None);

        document.swap_element(a, b);
        assert_eq!(document.get(page).unwrap().child_ids(), [b, a, c]);
        document.swap_element(a, b);
        assert_eq!(document.get(page).unwrap().child_ids(), [a, b, c]);

        document.swap_element(a, c);
        assert_eq!(document.get(page).unwrap().child_ids(), [c, b, a]);

        document.swap_element(b, inner);
        assert_eq!(document.get(page).unwrap().child_ids(), [c, inner, a]);
        assert_eq!(document.get(c).unwrap().child_ids(), [b]);
    }

    #[test]
    fn drop_element_frees_one_node_and_unlinks_its_element_children() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        let root = document.create_element("view", ());
        let first = document.create_element("view", ());
        let second = document.create_element("view", ());
        let grandchild = document.create_element("view", ());
        document.append_child(page, root);
        document.append_child(root, first);
        document.append_child(root, second);
        document.append_child(first, grandchild);

        document.remove_element(root);
        assert!(
            document.get(root).is_some(),
            "a removal frees nothing: the owner outside the tree still names it"
        );

        document.drop_element(root);
        assert!(document.get(root).is_none());
        for child in [first, second] {
            assert_eq!(
                document.get(child).map(Node::parent_id),
                Some(None),
                "an element child outlives its parent as a detached root"
            );
        }
        assert_eq!(
            document.get(first).unwrap().child_ids(),
            [grandchild],
            "and keeps the subtree under it intact"
        );

        document.append_child(page, first);
        assert!(document.is_connected(grandchild), "and re-attaches whole");
    }

    #[test]
    fn dropping_inside_a_shadow_tree_takes_the_elements_under_it() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        let host = document.create_element("view", ());
        document.append_child(page, host);
        let shadow = document.attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
        let container = document.create_element("view", ());
        let inside = document.create_element("view", ());
        document.append_child(shadow, container);
        document.append_child(container, inside);

        document.drop_element(container);
        assert!(document.get(container).is_none());
        assert!(
            document.get(inside).is_none(),
            "a shadow tree's elements have no owner outside it to survive for"
        );
    }

    #[test]
    fn drop_element_frees_the_text_nodes_and_shadow_tree_it_owns() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        let carrier = document.create_element("view", ());
        let run = document.create_text_node("run", ());
        let host = document.create_element("view", ());
        document.append_child(page, carrier);
        document.append_child(carrier, run);
        document.append_child(carrier, host);
        let shadow = document.attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
        let inside = document.create_element("view", ());
        document.append_child(shadow, inside);

        document.drop_element(carrier);
        assert!(document.get(carrier).is_none());
        assert!(
            document.get(run).is_none(),
            "the run goes with its carrier: nothing else would ever free it"
        );
        assert_eq!(
            document.get(host).map(Node::parent_id),
            Some(None),
            "the element child is still an element child"
        );

        document.drop_element(host);
        assert!(document.get(host).is_none());
        assert!(
            document.get(shadow).is_none() && document.get(inside).is_none(),
            "a shadow tree is the host's own, reachable from nowhere else"
        );
    }

    #[test]
    fn dropping_a_detached_node_schedules_no_layout_and_invalidates_nothing() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        let root = document.create_element("view", ());
        document.append_child(page, root);
        for _ in 0..4 {
            let child = document.create_element("view", ());
            document.append_child(root, child);
        }
        document.layout();
        document.mark_layout_complete(seldon::geometry::Size::new(800.0, 600.0), 1.0);
        document.remove_element(root);
        document.mark_layout_complete(seldon::geometry::Size::new(800.0, 600.0), 1.0);
        document.render();
        document.drop_element(root);
        assert!(
            !document.layout_root_dirty,
            "freeing a detached node must not schedule a whole-document layout"
        );
        assert!(!document.layout_dirty, "nor any layout at all");
        assert!(
            !document.needs_render(),
            "nor invalidate the retained frame"
        );
    }

    #[test]
    #[should_panic(expected = "swap_element: both operands must be attached")]
    fn swap_element_rejects_a_detached_operand() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        let attached = document.create_element("view", ());
        let detached = document.create_element("view", ());
        document.insert_before(page, attached, None);
        document.swap_element(attached, detached);
    }

    #[test]
    fn arena_storage_is_reused_while_the_id_that_named_it_retires() {
        let mut document: Document<u32> = Document::new(device(), "page", 1);
        let id = document.create_element("view", 7);
        let slot = document.live_slot(id);

        assert!(document.contains_node(id));
        assert_eq!(document.get(id).map(Node::payload), Some(&7));
        assert!(document.layout_cache_is_empty(id).is_some());

        assert_eq!(document.drop_subtree(id), vec![7]);
        assert!(!document.contains_node(id));
        assert!(
            document.get(id).is_none(),
            "a retired id resolves to nothing, forever"
        );
        assert!(document.layout_cache_is_empty(id).is_none());

        let next = document.create_text_node("replacement", 11);
        assert_ne!(next, id, "a retired id is never handed out again");
        assert_eq!(
            document.live_slot(next).arena_key(),
            slot.arena_key(),
            "the freed storage is handed to the next node"
        );
        assert_eq!(document.get(next).unwrap().payload(), &11);
        assert_eq!(document.layout_cache_is_empty(next), Some(true));
        assert_eq!(
            document
                .get(next)
                .unwrap()
                .styling_data()
                .snapshot_flags
                .load(Ordering::Relaxed),
            0
        );
        assert!(document.pending_snapshots.is_empty());
        assert!(
            document.get(id).is_none(),
            "reusing the storage must not resurrect the id that named it"
        );
    }

    #[test]
    fn a_handle_never_names_a_later_occupant_of_its_storage() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let root = document.document_element().id();
        assert_eq!(
            (DOCUMENT_NODE_ID.arena_key(), root.arena_key()),
            (1, 2),
            "arena key zero is reserved, so the document node and element take the next two"
        );

        let mut retired = Vec::new();
        for _ in 0..8 {
            let id = document.create_element("view", ());
            assert!(
                document.get(id).is_some(),
                "a fresh handle names its own node"
            );
            document.drop_subtree(id);
            retired.push(id);
            for &dead in &retired {
                assert!(
                    document.get(dead).is_none(),
                    "every retired handle stays refused as its storage is reused"
                );
            }
        }
    }

    #[test]
    fn a_unit_payload_does_not_change_primary_node_stride() {
        assert_eq!(
            std::mem::size_of::<Node<()>>(),
            std::mem::size_of::<Node<std::convert::Infallible>>()
        );
    }

    #[test]
    fn drop_subtree_prunes_the_parked_id_set_so_reused_storage_is_not_stale() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let a = document.document_element().id();
        let b = document.create_element("view", ());
        let slot = document.live_slot(b);
        document.append_child(a, b);

        document.record_relayout_root(b, LayoutInput::default(), RelayoutKind::Boundary);
        assert!(document.relayout_root_ids.contains(&b));

        assert_eq!(document.drop_subtree(b).len(), 1);
        assert!(
            !document.relayout_root_ids.contains(&b),
            "the removed id must not remain in the parked set",
        );

        let next = document.create_element("view", ());
        assert_eq!(
            document.live_slot(next).arena_key(),
            slot.arena_key(),
            "the freed storage is handed to the next node",
        );
        assert!(
            !document.relayout_root_ids.contains(&next),
            "a node reusing freed storage must not inherit stale parked state",
        );
    }

    #[test]
    fn detached_snapshot_survives_an_unrelated_connected_flush() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            ".hot { color: red; }",
            crate::style::engine::StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let connected = document.create_element("view", ());
        let detached = document.create_element("view", ());
        document.append_child(root, connected);
        document.append_child(root, detached);
        document.flush_styles_with_damage_sink(&mut |_, _| {});

        document.remove_element(detached);
        document.flush_styles_with_damage_sink(&mut |_, _| {});
        document.set_classes(detached, "hot");
        document.set_classes(connected, "hot");
        assert_eq!(document.pending_snapshots.len(), 2);

        document.flush_styles_with_damage_sink(&mut |_, _| {});

        assert!(
            document
                .pending_snapshots
                .contains_key(&OpaqueNode(detached.arena_key())),
            "a snapshot outside the traversed document tree must stay pending"
        );
        assert!(
            !document
                .pending_snapshots
                .contains_key(&OpaqueNode(connected.arena_key())),
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
    fn snapshot_queue_coalesces_and_subtree_removal_purges_queued_snapshots() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let root = document.document_element().id();
        let removed = document.create_element("view", ());
        let descendant = document.create_element("view", ());
        let removed_slot = document.live_slot(removed);
        let descendant_slot = document.live_slot(descendant);
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
            .find_map(|(opaque, snapshot)| (opaque.0 == removed.arena_key()).then_some(snapshot))
            .unwrap();
        assert!(snapshot.class_changed);
        assert!(snapshot.id_changed);

        document.set_classes(descendant, "nested");
        assert_eq!(document.pending_snapshots.len(), 2);
        assert_eq!(document.drop_subtree(removed).len(), 2);
        assert!(
            document.pending_snapshots.is_empty(),
            "removing a subtree must purge every queued snapshot"
        );

        let next = document.create_element("replacement", ());
        assert!(
            [removed_slot.arena_key(), descendant_slot.arena_key()]
                .contains(&document.live_slot(next).arena_key()),
            "the next node should take storage freed by the removed subtree"
        );
        assert_eq!(
            snapshot_flags(&document, next),
            0,
            "reused storage must not carry snapshot lifecycle flags into its new node"
        );
    }
}
