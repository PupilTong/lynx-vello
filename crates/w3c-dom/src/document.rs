//! The [`Document`] — one fixed-address set of NodeId-keyed arenas containing
//! the DOM tree and its phase-specific state.

use std::fmt;
#[cfg(debug_assertions)]
use std::ptr::NonNull;
use std::sync::atomic::Ordering;

use atomic_refcell::AtomicRefCell;
use neutron_star::geometry::Size;
use neutron_star::tree::LayoutInput;
use rustc_hash::FxHashSet;
use slab::Slab;
use stylo::LocalName;
use stylo::device::Device;
use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::stylesheets::UrlExtraData;

use crate::damage::StyleDamage;
use crate::engine::StyleEngine;
use crate::layout::LayoutData;
use crate::node::{Node, StylingData};

pub type NodeId = usize;

pub const DOCUMENT_NODE_ID: NodeId = 0;

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
pub(crate) struct DocumentArenas<T> {
    pub(crate) nodes: Slab<Node<T>>,
    pub(crate) payloads: Slab<PayloadSlot<T>>,
    pub(crate) styling: Slab<StylingData>,
    pub(crate) layout: Slab<AtomicRefCell<LayoutData>>,
}

impl<T> DocumentArenas<T> {
    fn new() -> Self {
        Self {
            nodes: Slab::new(),
            payloads: Slab::new(),
            styling: Slab::new(),
            layout: Slab::new(),
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
    arenas: Box<DocumentArenas<T>>,
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
            .field("nodes", &self.arenas.nodes)
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
        let mut arenas = Box::new(DocumentArenas::new());
        let owner = std::ptr::from_mut::<DocumentArenas<T>>(arenas.as_mut());
        let root = arenas
            .nodes
            .insert(Node::new_document(owner, lock, url_data));
        assert_eq!(
            root, DOCUMENT_NODE_ID,
            "the DOM document node must occupy slab slot zero"
        );
        assert_eq!(arenas.payloads.insert(PayloadSlot::Document), root);
        assert_eq!(arenas.styling.insert(StylingData::default()), root);
        assert_eq!(
            arenas
                .layout
                .insert(AtomicRefCell::new(LayoutData::default())),
            root
        );
        Self {
            style_engine,
            arenas,
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

    pub(crate) fn is_relayout_root_parked(&self, id: NodeId) -> bool {
        self.relayout_root_ids.contains(&id)
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
        &self.arenas.nodes
    }

    pub(crate) fn tree_mut(&mut self) -> &mut Slab<Node<T>> {
        &mut self.arenas.nodes
    }

    pub(crate) fn layout_data_mut(
        &mut self,
    ) -> impl Iterator<Item = (NodeId, &mut AtomicRefCell<LayoutData>)> {
        self.arenas.layout.iter_mut()
    }

    pub(crate) fn styling_data(&self, id: NodeId) -> Option<&StylingData> {
        self.arenas.styling.get(id)
    }

    pub(crate) fn styling_data_mut(&mut self, id: NodeId) -> Option<&mut StylingData> {
        self.arenas.styling.get_mut(id)
    }

    #[must_use]
    pub fn root_node(&self) -> &Node<T> {
        self.arenas
            .nodes
            .get(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
    }

    #[must_use]
    pub fn root_element(&self) -> Option<&Node<T>> {
        self.root_node().children().find(|node| node.is_element())
    }

    #[cfg(debug_assertions)]
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
        make: impl FnOnce(*mut DocumentArenas<T>, NodeId) -> Node<T>,
    ) -> NodeId {
        let owner = std::ptr::from_mut::<DocumentArenas<T>>(self.arenas.as_mut());
        let entry = self.arenas.nodes.vacant_entry();
        let id = entry.key();
        assert_eq!(self.arenas.payloads.vacant_key(), id);
        assert_eq!(self.arenas.styling.vacant_key(), id);
        assert_eq!(self.arenas.layout.vacant_key(), id);
        entry.insert(make(owner, id));
        assert_eq!(self.arenas.payloads.insert(PayloadSlot::Node(payload)), id);
        assert_eq!(self.arenas.styling.insert(StylingData::default()), id);
        assert_eq!(
            self.arenas
                .layout
                .insert(AtomicRefCell::new(LayoutData::default())),
            id
        );
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
        self.arenas
            .nodes
            .get_mut(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
            .children
            .push(child);
        self.arenas
            .nodes
            .get_mut(child)
            .expect("the attached child was validated as live")
            .parent = Some(DOCUMENT_NODE_ID);
        self.mark_subtree_dirty(child);
    }

    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node<T>> {
        self.arenas.nodes.get(id)
    }

    #[must_use]
    pub fn contains_node(&self, id: NodeId) -> bool {
        self.arenas.nodes.contains(id)
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

        self.arenas
            .nodes
            .get_mut(parent)
            .expect("stale NodeId passed to Document::insert_before")
            .children
            .insert(index, child);
        self.arenas
            .nodes
            .get_mut(child)
            .expect("stale NodeId passed to Document::insert_before")
            .parent = Some(parent);

        self.note_moved_subtree(child);
        self.note_child_list_change(parent, index);
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

        let removed_index = {
            let parent_node = self
                .arenas
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
        self.arenas
            .nodes
            .get_mut(child)
            .expect("stale NodeId passed to Document::detach")
            .parent = None;

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
            {
                let node = self
                    .arenas
                    .nodes
                    .try_remove(current)
                    .expect("subtree links always resolve while removing");
                stack.extend_from_slice(&node.children);
            }
            let payload = self
                .arenas
                .payloads
                .try_remove(current)
                .expect("removed element/text node must have payload-arena state");
            self.arenas
                .styling
                .try_remove(current)
                .expect("removed node must have styling-arena state");
            self.arenas
                .layout
                .try_remove(current)
                .expect("removed node must have layout-arena state");
            match payload {
                PayloadSlot::Node(payload) => removed.push(payload),
                PayloadSlot::Document => unreachable!("the document node cannot be removed"),
            }
        }
        let nodes = &self.arenas.nodes;
        self.relayout_roots
            .retain(|pending| nodes.contains(pending.node_id));
        self.relayout_root_ids
            .retain(|&parked_id| nodes.contains(parked_id));
        removed
    }

    pub(crate) fn take_snapshot_map(&mut self, root: NodeId) -> SnapshotMap {
        let mut snapshots = SnapshotMap::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(node) = self.arenas.nodes.get(id) else {
                continue;
            };
            let styling = self
                .arenas
                .styling
                .get_mut(id)
                .expect("live node must have styling-arena state");
            debug_assert_eq!(
                styling.snapshot.is_some(),
                styling.snapshot_flags.load(Ordering::Relaxed) & crate::node::SNAPSHOT_PRESENT != 0,
                "snapshot slot and lifecycle flag diverged before flush"
            );
            if let Some(snapshot) = styling.snapshot.take() {
                snapshots.insert(OpaqueNode(node.id()), *snapshot);
            }
            if styling.dirty_descendants.load(Ordering::Relaxed) {
                stack.extend_from_slice(&node.children);
            }
        }
        snapshots
    }

    pub(crate) fn harvest_flush(
        &mut self,
        root: NodeId,
        snapshots: &SnapshotMap,
        sink: &mut dyn FnMut(NodeId, StyleDamage),
    ) {
        for opaque in snapshots.keys() {
            if let Some(styling) = self.arenas.styling.get(opaque.0) {
                styling.snapshot_flags.store(0, Ordering::Relaxed);
            }
        }

        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let harvested = {
                let arenas = &mut *self.arenas;
                let Some(node) = arenas.nodes.get_mut(current) else {
                    continue;
                };
                let styling = arenas
                    .styling
                    .get_mut(current)
                    .expect("live node must have styling-arena state");
                let mut harvested = None;
                if let Some(wrapper) = node.stylo_data_mut() {
                    let mut data = wrapper.borrow_mut();
                    let damage = data.damage;
                    data.clear_restyle_state();
                    if !damage.is_empty() {
                        harvested = Some(StyleDamage::from(damage));
                    }
                }
                let descend = styling.dirty_descendants.load(Ordering::Relaxed);
                styling.dirty_descendants.store(false, Ordering::Relaxed);
                styling.snapshot_flags.store(0, Ordering::Relaxed);
                if descend {
                    stack.extend_from_slice(&node.children);
                }
                harvested
            };
            let Some(damage) = harvested else {
                continue;
            };

            if damage.needs_relayout() {
                if let Some(element) = self.get(current) {
                    for child in element.children().filter(|child| child.is_text_node()) {
                        child.layout_data().borrow_mut().clear_measurement_cache();
                        child.invalidate_text_artifacts();
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

/// Debug-only RAII marker for the style-flush phase.
#[cfg(debug_assertions)]
pub(crate) struct FlushPhaseToken {
    flag: NonNull<std::sync::atomic::AtomicBool>,
}

#[cfg(debug_assertions)]
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

    #[test]
    fn slabs_follow_primary_node_lifetime_and_id_reuse() {
        let mut document: Document<u32> = Document::new(device());
        let id = document.create_element("view", 7);

        assert!(document.arenas.nodes.contains(id));
        assert!(matches!(
            document.arenas.payloads.get(id),
            Some(PayloadSlot::Node(7))
        ));
        assert!(document.arenas.styling.get(id).is_some());
        assert!(document.arenas.layout.get(id).is_some());

        assert_eq!(document.remove_subtree(id), vec![7]);
        assert!(!document.arenas.nodes.contains(id));
        assert!(document.arenas.payloads.get(id).is_none());
        assert!(document.arenas.styling.get(id).is_none());
        assert!(document.arenas.layout.get(id).is_none());
        assert_eq!(document.arenas.nodes.vacant_key(), id);
        assert_eq!(document.arenas.payloads.vacant_key(), id);
        assert_eq!(document.arenas.styling.vacant_key(), id);
        assert_eq!(document.arenas.layout.vacant_key(), id);

        let reused = document.create_text_node("replacement", 11);
        assert_eq!(reused, id, "the primary slab should reuse its vacant ID");
        assert_eq!(document.get(reused).unwrap().payload(), &11);
        assert!(document.get(reused).unwrap().layout_cache_is_empty());
        assert!(
            document
                .arenas
                .styling
                .get(reused)
                .is_some_and(|state| state.snapshot.is_none())
        );
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
            .arenas
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
        assert!(document.is_relayout_root_parked(b));

        assert_eq!(document.remove_subtree(b).len(), 1);
        assert!(
            !document.is_relayout_root_parked(b),
            "the removed id must not remain in the parked set",
        );

        let reused = document.create_element("view", ());
        assert_eq!(reused, b, "the freed slab slot is reused");
        assert!(
            !document.is_relayout_root_parked(reused),
            "a reused slab id must not inherit stale parked state",
        );
    }
}
