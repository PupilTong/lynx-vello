//! The [`Document`] — one NodeId-aligned arena set: a fixed-address DOM/style
//! tree beside independently mutable layout/text state.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use hughie::geometry::Size;
use hughie::tree::{LayoutInput, LayoutOutput};
use rustc_hash::FxHashSet;
use stylo::LocalName;
use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::stylesheets::UrlExtraData;

use crate::style::damage::StyleDamage;
use crate::style::engine::StyleEngine;
pub use crate::tree::arena::NodeId;
pub(crate) use crate::tree::arena::{
    DOCUMENT_ELEMENT_NODE_ID, DOCUMENT_NODE_ID, DocumentLayoutState, NodeLayoutState, NodeSlot,
    PayloadSlot, TreeArenas,
};
use crate::tree::custom::CustomElementRegistry;
use crate::tree::node::Node;

pub(crate) fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

/// How a scheduled committed-input relayout is allowed to finish.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RelayoutKind {
    /// A `contain: strict` boundary: containment guarantees nothing escapes,
    /// so the recompute is final whatever it produces.
    Boundary,
    /// An ordinary node whose committed input is content-independent: the
    /// recompute stands only if it reproduces the previous output bit for
    /// bit; any difference escalates to a whole-tree pass.
    InPlace { previous: LayoutOutput },
}

/// A node scheduled for a committed-input relayout.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingRelayout {
    pub node_id: NodeId,
    pub input: LayoutInput,
    pub kind: RelayoutKind,
}

/// One DOM tree, including its actual document node at primary-arena slot
/// zero.
pub struct Document<T> {
    style_engine: StyleEngine,
    tree: Box<TreeArenas<T>>,
    layout: DocumentLayoutState,
    pub(crate) painter: RefCell<crate::paint::painter::Painter>,
    /// The embedder's decoded-image owner, read by the paint walk and by the
    /// layer that drives loads. A document starts with the store that is
    /// resident in nothing, so painting a page with images before an embedder
    /// installs one skips them rather than failing.
    /// The document's image name table. Holds no pixels and no store.
    pub(crate) images: crate::render::image::ImageRegistry,
    pending_snapshots: SnapshotMap,
    relayout_roots: Vec<PendingRelayout>,
    relayout_root_ids: FxHashSet<NodeId>,
    shadow_roots: usize,
    pub(crate) custom_elements: CustomElementRegistry<T>,
    animations: crate::style::animation::AnimationDriver,
    /// The id the most recently built frame carries; monotone per build, so
    /// it orders commits for every consumer of published frames.
    commit_id: u64,
    /// Whether anything visually observable changed since the retained
    /// frame was built. This is the whole invalidation state: one bit, not
    /// a counter — a frame's identity is its commit id.
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
    /// Creates a document with a permanent document element.
    #[must_use]
    pub fn new(device: crate::style::device::Device, root_tag: &str, root_payload: T) -> Self {
        let style_engine = StyleEngine::new(device.into_stylo(), about_blank_url_data());
        let lock = style_engine.lock();
        let url_data = style_engine.url_data();
        let mut tree = Box::new(TreeArenas::new());
        tree.reserve_zero();
        let root = tree.insert_node(PayloadSlot::Document, |owner, id| {
            Node::new_document(owner, id, lock, url_data)
        });
        assert_eq!(
            root, DOCUMENT_NODE_ID,
            "the DOM document node must take the first id the document ever issues"
        );
        let layout = DocumentLayoutState::new();
        let mut document = Self {
            style_engine,
            tree,
            layout,
            painter: RefCell::new(crate::paint::painter::Painter::default()),
            images: crate::render::image::ImageRegistry::default(),
            pending_snapshots: SnapshotMap::new(),
            relayout_roots: Vec::new(),
            relayout_root_ids: FxHashSet::default(),
            shadow_roots: 0,
            custom_elements: CustomElementRegistry::default(),
            animations: crate::style::animation::AnimationDriver::default(),
            commit_id: 0,
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
        document
            .live_node_mut(DOCUMENT_NODE_ID)
            .children
            .push(root_slot);
        document.live_node_mut(root).parent = Some(document_slot);
        document.mark_subtree_dirty(root);
        document.invalidate_layout(root);
        document
    }

    pub(crate) const fn style_engine(&self) -> &StyleEngine {
        &self.style_engine
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

    pub(crate) fn layout_requires_full_pass(&self, viewport: Size<f32>, scale: f32) -> bool {
        self.layout_root_dirty || self.layout_inputs_changed(viewport, scale)
    }

    /// Whether the viewport or device scale moved since the last pass. Both
    /// feed the rounding function itself, so nothing about the previous
    /// rounding survives them — including for boxes no layout write touched.
    pub(crate) fn layout_inputs_changed(&self, viewport: Size<f32>, scale: f32) -> bool {
        self.last_layout_inputs != Some((viewport, scale))
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

    /// Resolves a node to its arena slot, for the state that lives outside
    /// [`TreeArenas`].
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

    pub(crate) fn layout_parts(
        &mut self,
    ) -> (&TreeArenas<T>, &mut DocumentLayoutState, &FxHashSet<NodeId>) {
        (&self.tree, &mut self.layout, &self.relayout_root_ids)
    }

    pub(crate) fn visual_parts(&self) -> (&TreeArenas<T>, &DocumentLayoutState) {
        (&self.tree, &self.layout)
    }

    /// The id of the most recently built frame.
    #[must_use]
    pub(crate) fn commit_id(&self) -> u64 {
        self.commit_id
    }

    /// Claims the next commit id for the frame about to be built, clearing
    /// the dirty bit with it: mutations from here on belong to the *next*
    /// frame. A build or paint that panics leaves the previously retained
    /// frame carrying an older id, which is what keeps `needs_render`
    /// reporting stale without a counter.
    pub(crate) fn next_commit_id(&mut self) -> u64 {
        self.commit_id += 1;
        self.visual_dirty = false;
        self.commit_id
    }

    #[must_use]
    pub(crate) fn visual_dirty(&self) -> bool {
        self.visual_dirty
    }

    /// Notes that the retained frame is painted for a state that no longer
    /// exists.
    pub(crate) fn note_visual_mutation(&mut self) {
        self.visual_dirty = true;
    }

    /// Every live node's layout state, in arena order. The keys are arena
    /// slots, not ids: this walks storage, not identity.
    pub(crate) fn layout_data_mut(
        &mut self,
    ) -> impl Iterator<Item = (usize, &mut NodeLayoutState)> {
        self.layout.iter_mut()
    }

    pub(crate) fn snapshot_storage(&mut self) -> (&TreeArenas<T>, &mut SnapshotMap) {
        (&self.tree, &mut self.pending_snapshots)
    }

    #[must_use]
    pub fn root_node(&self) -> &Node<T> {
        self.tree
            .get(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
    }

    /// The document element — the permanent root element created with the
    /// document itself. The document node's child list is structurally
    /// immutable after construction: this is its only child, forever.
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

    /// Files a node in all three arenas under one freshly issued id.
    pub(crate) fn allocate_node(
        &mut self,
        payload: PayloadSlot<T>,
        make: impl FnOnce(*mut TreeArenas<T>, NodeId) -> Node<T>,
    ) -> NodeId {
        self.tree.insert_node(payload, make)
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
        let child = self.slot(child)?;
        self.get(parent)?
            .child_slots()
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
        self.live_node_mut(parent)
            .children
            .insert(index, child_slot);
        self.live_node_mut(child).parent = Some(parent_slot);
        let appended = index + 1 == self.live_node_mut(parent).children.len();
        let contains_custom_elements = self.note_custom_subtree_inserted(child);

        self.note_moved_subtree(child);
        self.note_slot_assignment_inserted(parent, child, appended);
        self.note_child_list_change(parent, index);
        self.invalidate_layout(child);
        let connected = contains_custom_elements && self.is_connected(child);
        self.note_custom_elements_inserted(child, connected);
        self.drain_reactions(base);
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, child, None);
    }

    /// Exchanges the positions of two distinct attached elements, within one
    /// parent or across parents.
    pub fn swap_element(&mut self, a: NodeId, b: NodeId) {
        assert_ne!(a, b, "swap_element: the operands must differ");
        let position = |document: &Self, node: NodeId| {
            let parent = document
                .get(node)
                .expect("swap_element: stale NodeId")
                .parent_id()
                .expect("swap_element: both operands must be attached");
            let node_slot = document.live_slot(node);
            let parent_node = document.get(parent).expect("a child's parent is live");
            let siblings = parent_node.child_slots();
            let index = siblings
                .iter()
                .position(|&sibling| sibling == node_slot)
                .expect("a child appears in its parent's child list");
            let next = siblings
                .get(index + 1)
                .map(|&slot| document.arenas().at(slot).id());
            (parent, next)
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

    /// Unlinks `child` from its parent, leaving it allocated as a detached
    /// root. Freeing is [`Self::drop_element`]'s and
    /// [`Self::drop_subtree`]'s; a removal never frees anything, because the
    /// owner outside the tree that named the node still names it.
    pub fn remove_element(&mut self, child: NodeId) {
        let base = self.begin_reactions();
        self.unlink_from_parent(child);
        self.drain_reactions(base);
    }

    /// Frees `root`, a detached node, together with everything under it that
    /// belongs to it alone — its non-element descendants and its shadow tree
    /// in full — and returns the root's payload with the number of nodes
    /// freed.
    ///
    /// An **element** child is unlinked where the walk meets it and stays
    /// allocated as a detached root, because an element is what an embedder
    /// names: in Bobcat every element carries a script handle, and that
    /// handle is what decides when the element goes. Nothing else in a tree
    /// is handed out that way — a text node is minted by whatever reflects
    /// it (`raw-text`'s run), and a shadow tree is a component's own — so
    /// leaving those behind would leak them, and they go with their owner.
    ///
    /// A surviving child is unlinked while its parent is still live, so the
    /// removal bookkeeping runs against a node that exists — by
    /// [`Self::unlink_from_doomed_parent`], since everything this walk
    /// touches is detached already.
    fn free_owned_subtree(
        &mut self,
        root: NodeId,
        in_shadow_tree: bool,
    ) -> (PayloadSlot<T>, usize) {
        debug_assert!(
            self.live(root).parent_id().is_none(),
            "Document::free_owned_subtree frees detached nodes only"
        );
        let mut payload = None;
        let mut freed = 0;
        // A node inside a shadow tree is freed whole, elements included:
        // nothing in one is reachable from outside, so an element there has
        // no owner to survive for. Whether `root` is one has to be decided by
        // the caller, before it unlinks the node that would have answered.
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
            let (_, slot) = self.free_node(current);
            freed += 1;
            if current == root {
                payload = Some(slot);
            }
        }
        self.prune_relayout_roots();
        (
            payload.expect("the root is the first node the walk frees"),
            freed,
        )
    }

    /// Takes `child` out of a `parent` that is about to be freed, leaving it
    /// a detached root.
    ///
    /// The structural half of [`Self::unlink_from_parent`] and no more. Every
    /// node [`Self::free_owned_subtree`] walks is inside a detached subtree,
    /// so nothing here is rendered, laid out, animating, or connected: the
    /// invalidation, the animation cancel, the child-list style hint and the
    /// disconnected reaction the general path records would all be work
    /// against a node about to stop existing — and the first of them would
    /// schedule a whole-document layout for a frame in which nothing visible
    /// changed. The removal that detached the subtree recorded all of it
    /// already, for the subtree as a whole.
    fn unlink_from_doomed_parent(&mut self, child: NodeId, parent: NodeId) {
        let child_slot = self.live_slot(child);
        let parent_node = self
            .tree
            .get_mut(parent)
            .expect("internal tree link must resolve to a live node");
        let index = parent_node
            .children
            .iter()
            .position(|&candidate| candidate == child_slot)
            .expect("child must appear in its parent's child list");
        parent_node.children.remove(index);
        self.live_node_mut(child).parent = None;
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

        let child_slot = self.live_slot(child);
        let removed_index = {
            let parent_node = self
                .tree
                .get_mut(parent)
                .expect("internal tree link must resolve to a live node");
            let index = parent_node
                .children
                .iter()
                .position(|&candidate| candidate == child_slot)
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

    /// Frees `id`, returning its payload. Its element children are unlinked
    /// and stay allocated as detached roots, each for its own owner to free;
    /// everything else under it — its text nodes, its shadow tree — is freed
    /// with it, as [`Self::free_owned_subtree`] describes.
    ///
    /// Only the named node's payload comes back. The payloads of what goes
    /// with it are dropped, because nothing outside the tree named those
    /// nodes to be given one back for; an embedder whose `T` owns a resource
    /// and that mints text nodes or shadow trees wants
    /// [`Self::drop_subtree`], which answers with every payload it frees.
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
        // Before anything is mutated, as `drop_subtree`'s preflight is: a
        // refused drop must leave the document exactly as it found it, not
        // detached and half-destroyed.
        self.assert_not_pinned(id);
        // Read while the node still has the parent that answers it: an
        // element inside a shadow tree is not reachable from outside, so the
        // whole subtree under it goes, elements and all.
        let in_shadow_tree = self.containing_shadow_root(id).is_some();
        let base = self.begin_reactions();
        self.unlink_from_parent(id);
        self.drain_reactions(base);
        // No visual mutation is noted here. Every node this frees was either
        // detached already or detached by the unlink above, which noted one
        // itself; freeing what is not rendered changes no frame.
        match self.free_owned_subtree(id, in_shadow_tree).0 {
            PayloadSlot::Node(payload) => payload,
            PayloadSlot::ShadowRoot => unreachable!("a shadow root is refused above"),
            PayloadSlot::Document => unreachable!("the document node is refused above"),
            PayloadSlot::Reserved => unreachable!("no id resolves to the reservation"),
        }
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
        self.drain_reactions(base);
        self.note_visual_mutation();
        let mut removed = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let (node, payload) = self.free_node(current);
            // The node is already out of the arenas, so its child links have
            // to be turned back into ids before the walk can follow them.
            stack.extend(
                node.child_slots()
                    .iter()
                    .map(|&slot| self.tree.at(slot).id()),
            );
            if let Some(root) = node.shadow_root_id() {
                stack.push(root);
            }
            match payload {
                PayloadSlot::Node(payload) => removed.push(payload),
                PayloadSlot::ShadowRoot => {}
                PayloadSlot::Document => unreachable!("the document node cannot be removed"),
                PayloadSlot::Reserved => unreachable!("no id resolves to the reservation"),
            }
        }
        self.prune_relayout_roots();
        removed
    }

    /// Cancels the animations of a subtree that is leaving its parent.
    ///
    /// A node taken out of the tree stops being rendered, so its animations
    /// end — and a move, which unlinks before re-inserting, restarts them,
    /// which is what browsers do too. Costs one map read on a document that
    /// never animates.
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
            // The node survives an unlink, so its animation bit has to be
            // cleared with its animations rather than left to the next flush.
            node.set_may_have_animations(false);
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

    /// Empties all three arenas of one node and retires its id.
    fn free_node(&mut self, id: NodeId) -> (Node<T>, PayloadSlot<T>) {
        let removed_snapshot = self
            .pending_snapshots
            .remove(&OpaqueNode(id.arena_key()))
            .is_some();
        self.animations.forget(&[id]);
        let (node, payload) = self.tree.remove_node(id);
        let slot = id;
        debug_assert_eq!(
            removed_snapshot,
            node.snapshot_present(),
            "the document snapshot queue and node lifecycle flag diverged during removal"
        );
        if node.is_shadow_root() {
            self.shadow_roots -= 1;
        }
        self.layout.remove(slot);
        self.forget_reactions(id);
        (node, payload)
    }

    fn prune_relayout_roots(&mut self) {
        // Freeing is per node now — one call per script handle the collector
        // takes — so the empty case is the one that has to be free, rather
        // than two `retain` passes over vectors that hold nothing.
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

    /// Invalidates an element's text children for a relayout of that element.
    ///
    /// `invalidate_layout` walks *up*, so nothing else clears a text child's
    /// box cache — without this the child's committed measurement answers the
    /// re-layout unchanged. The shaped layout behind it is a separate
    /// question: it survives every change the element made to its own
    /// geometry, and dies only when `shaping_changed` says Parley would shape
    /// the paragraph differently.
    pub(crate) fn invalidate_text_children(&mut self, element: NodeId, shaping_changed: bool) {
        let Some(node) = self.tree.get(element) else {
            return;
        };
        let text_children: Vec<NodeSlot> = node
            .flat_children()
            .iter()
            .copied()
            .filter(|&slot| self.tree.at(slot).is_text_node())
            .collect();
        for slot in text_children {
            if shaping_changed {
                self.layout.clear_layout_cache(slot);
            } else {
                self.layout.clear_box_cache(slot);
            }
        }
    }

    fn harvest_style_damage<F>(&mut self, stack: &mut Vec<NodeId>, sink: &mut F)
    where
        F: FnMut(NodeId, StyleDamage),
    {
        while let Some(current) = stack.pop() {
            let harvested = {
                let (harvested, descend) = {
                    let Some(node) = self.tree.get_mut(current) else {
                        continue;
                    };
                    let harvested = node.stylo_data_mut().and_then(|wrapper| {
                        let mut data = wrapper.borrow_mut();
                        let damage = data.damage;
                        data.clear_restyle_state();
                        (!damage.is_empty()).then(|| StyleDamage::from(damage))
                    });
                    // Reads the primary style after the clear above, which is
                    // sound only because clearing restyle state touches the
                    // hint, the damage, and the flags — never `styles`.
                    let refresh = node.refresh_layout_style();
                    let dirty = node.styling.dirty_descendants.get_mut();
                    (
                        harvested.map(|damage| (damage, refresh)),
                        std::mem::replace(dirty, false) || refresh.changed,
                    )
                };
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
                harvested
            };
            let Some((damage, refresh)) = harvested else {
                continue;
            };
            if damage.needs_relayout() {
                self.invalidate_text_children(current, refresh.shaping_changed);
                // Reconstruction damage needs no second walk from the parent:
                // this one already cleared the parent's cache (or stopped
                // because it was empty, or because the parent skips its
                // contents and cannot see the change), and the ancestor it
                // parked on relays the reconstructed box tree under its own
                // algorithm. A second walk would restart above that park and
                // clear past it.
                self.invalidate_layout(current);
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
        assert!(
            !document
                .scene(&crate::NoImages)
                .encoding()
                .draw_tags
                .is_empty()
        );

        document.apply_image_events(&[crate::ImageEvent::Failed {
            id: crate::ImageId(std::num::NonZeroU32::new(1).expect("a non-zero id")),
        }]);
        assert!(document.needs_render());
        assert!(document.render());
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

    /// A removal only unlinks. What frees a node is a drop, and a drop frees
    /// one node: its element children survive it as detached roots, each
    /// waiting for the owner that names it.
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

    /// A node *inside* a shadow tree is not reachable from outside either, so
    /// its element children go with it rather than surviving as detached
    /// roots nothing could ever name or free.
    #[test]
    fn dropping_inside_a_shadow_tree_takes_the_elements_under_it() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        let host = document.create_element("view", ());
        document.append_child(page, host);
        let shadow = document.attach_shadow(host, crate::ShadowRootMode::Open);
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

    /// The other half of the rule: what no owner outside the tree could name
    /// goes with its owner, or nothing would ever free it.
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
        let shadow = document.attach_shadow(host, crate::ShadowRootMode::Open);
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

    /// Freeing what is already detached changes no frame, so it must cost no
    /// frame: no layout is scheduled, least of all a whole-document one, and
    /// the retained frame stays valid. The unlink that detached the subtree
    /// is what recorded all of that, once, for the subtree as a whole.
    #[test]
    fn dropping_a_detached_node_schedules_no_layout_and_invalidates_nothing() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        let root = document.create_element("view", ());
        document.append_child(page, root);
        // Element children, because they are what the free walk has to unlink
        // — and the general unlink path notes a visual mutation per child.
        for _ in 0..4 {
            let child = document.create_element("view", ());
            document.append_child(root, child);
        }
        document.layout();
        document.mark_layout_complete(hughie::geometry::Size::new(800.0, 600.0), 1.0);
        document.remove_element(root);
        document.mark_layout_complete(hughie::geometry::Size::new(800.0, 600.0), 1.0);
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

    /// The three arenas rise and fall together, the freed *storage* comes
    /// back, and the freed *id* never does.
    #[test]
    fn arena_storage_is_reused_while_the_id_that_named_it_retires() {
        let mut document: Document<u32> = Document::new(device(), "page", 1);
        let id = document.create_element("view", 7);
        let slot = document.live_slot(id);

        assert!(document.contains_node(id));
        assert!(matches!(
            document.arenas().payload_at(slot),
            PayloadSlot::Node(7)
        ));
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

    /// A handle names one node for that node's whole life and is refused
    /// afterwards, however many nodes reuse its storage. That is the guarantee
    /// the deleted id table used to buy with a monotonic counter, and the
    /// generation buys it without one.
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
    fn payload_size_does_not_change_primary_node_stride() {
        assert_eq!(
            std::mem::size_of::<Node<()>>(),
            std::mem::size_of::<Node<[u8; 1_024]>>()
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
        document.add_stylesheet(".hot { color: red; }", crate::StylesheetOrigin::Author);
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
