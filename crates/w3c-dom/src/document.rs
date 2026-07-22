//! The [`Document`] — one fixed-address slab containing the whole DOM tree.
//!
//! `Document<T>` owns a boxed [`Slab`] whose address never changes. Every
//! [`Node`] stores a backpointer to that slab, so a plain `&Node` can navigate
//! the tree and recover its owner document without a wrapper handle or a
//! separate tree/core object.
//!
//! Slot zero is always the real DOM document node. Element and text nodes are
//! allocated in the remaining slab slots and use the raw slab index as their
//! [`NodeId`]. IDs are context-local: callers must not route an ID to another
//! `Document`, and a removed ID must not outlive the ownership layer that
//! retained its node.

use std::fmt;
// `NonNull` is only used by the debug-only flush-phase marker below, so the
// import is gated to match (unused in release/bench builds otherwise).
#[cfg(debug_assertions)]
use std::ptr::NonNull;

use neutron_star::tree::LayoutInput;
use slab::Slab;
use stylo::LocalName;
use stylo::device::Device;
use stylo::dom::OpaqueNode;
use stylo::selector_parser::SnapshotMap;
use stylo::stylesheets::UrlExtraData;

use crate::damage::StyleDamage;
use crate::engine::StyleEngine;
use crate::node::Node;

/// A node's raw index in its owning document's slab.
///
/// IDs carry no document token and no allocation generation. The runtime
/// context/handle layer owns routing and lifetime; `w3c-dom` only resolves the
/// index in the `Document` passed to it.
pub type NodeId = usize;

/// The fixed slab slot occupied by the DOM document node.
pub const DOCUMENT_NODE_ID: NodeId = 0;

/// The placeholder base URL for parsing a standalone document's inline styles.
pub(crate) fn about_blank_url_data() -> UrlExtraData {
    UrlExtraData::from(::url::Url::parse("about:blank").expect("about:blank is a valid URL"))
}

/// One DOM tree, including its actual document node at slab slot zero.
///
/// The box is load-bearing: moving `Document` never moves the `Slab` value, so
/// every node's slab backpointer remains valid until the document is dropped.
pub struct Document<T> {
    /// This document's private stylesheet, device, cascade, and lock state.
    style_engine: StyleEngine,
    nodes: Box<Slab<Node<T>>>,
    /// Relayout boundaries that a boundary-stopped
    /// [`invalidate_layout`](Self::invalidate_layout) parked for the next
    /// layout pass, each paired with the exact [`LayoutInput`] it was last
    /// committed with.
    ///
    /// When the ancestor walk stops at a `contain: strict` / skipped
    /// `content-visibility` boundary it leaves that boundary's *ancestors'*
    /// caches warm — so the next `compute_root_layout` from the document root
    /// answers them from cache and never descends into the boundary. The
    /// boundary's own interior still changed, so it is re-run in place with its
    /// committed input via
    /// [`compute_boundary_relayout`](neutron_star::compute::compute_boundary_relayout)
    /// at the start of the layout pass (the engine-internal equivalent of
    /// neutron-star's `invalidate_for_relayout` re-layout root, using real
    /// parent links). Drained and cleared once the pass consumes it.
    ///
    /// The layout pass re-runs these **deepest-first** (by tree depth): when
    /// nested boundaries are parked together, an outer boundary's re-run
    /// re-imposes its inner boundaries' sizes and so must run last to have the
    /// final say over an inner boundary's stale committed replay (see
    /// `layout::host::run_layout`). Parking is duplicate-free by construction:
    /// [`invalidate_layout`](Self::invalidate_layout) stops at a boundary
    /// already present here instead of parking it twice or clearing past it.
    /// [`remove_subtree`](Self::remove_subtree) drops entries for every removed
    /// node before its raw slab id can be reused.
    relayout_roots: Vec<(NodeId, LayoutInput)>,
}

impl<T: fmt::Debug> fmt::Debug for Document<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("root_element", &self.root_element().map(Node::id))
            .field("style_engine", &self.style_engine)
            .field("nodes", &self.nodes)
            .finish_non_exhaustive()
    }
}

impl<T> Document<T> {
    /// Create an empty document with an independent `about:blank` style
    /// engine/context around the embedder-supplied device.
    #[must_use]
    pub fn new(device: Device) -> Self {
        Self::with_url_data(device, about_blank_url_data())
    }

    /// Create an empty document with an independent style engine/context and
    /// explicit CSS base URL.
    ///
    /// # Panics
    ///
    /// Panics only if the internal empty-slab invariant is broken and its
    /// first insertion does not occupy slot zero.
    #[must_use]
    pub fn with_url_data(device: Device, url_data: UrlExtraData) -> Self {
        let style_engine = StyleEngine::with_url_data(device, url_data);
        let lock = style_engine.lock();
        let url_data = style_engine.url_data();
        let mut nodes = Box::new(Slab::new());
        let tree = std::ptr::from_mut::<Slab<Node<T>>>(nodes.as_mut());
        let root = nodes.insert(Node::new_document(tree, lock, url_data));
        assert_eq!(
            root, DOCUMENT_NODE_ID,
            "the DOM document node must occupy slab slot zero"
        );
        Self {
            style_engine,
            nodes,
            relayout_roots: Vec::new(),
        }
    }

    /// This document's private style engine.
    pub(crate) const fn style_engine(&self) -> &StyleEngine {
        &self.style_engine
    }

    /// This document's private style engine.
    pub(crate) const fn style_engine_mut(&mut self) -> &mut StyleEngine {
        &mut self.style_engine
    }

    /// Park a boundary-stopped relayout root for the next layout pass (see
    /// [`relayout_roots`](Self::relayout_roots)). Called only by
    /// [`invalidate_layout`](Self::invalidate_layout).
    pub(crate) fn record_relayout_root(&mut self, id: NodeId, committed_input: LayoutInput) {
        self.relayout_roots.push((id, committed_input));
    }

    /// The relayout boundaries parked since the last pass (see
    /// [`relayout_roots`](Self::relayout_roots)).
    pub(crate) fn relayout_roots(&self) -> &[(NodeId, LayoutInput)] {
        &self.relayout_roots
    }

    /// Forget every parked relayout root (the layout pass has consumed them, or
    /// an [`invalidate_layout_all`](Self::invalidate_layout_all) subsumed them
    /// with a full re-layout).
    pub(crate) fn clear_relayout_roots(&mut self) {
        self.relayout_roots.clear();
    }

    /// Borrow the complete node slab.
    pub(crate) fn tree(&self) -> &Slab<Node<T>> {
        &self.nodes
    }

    /// Mutably borrow the complete node slab.
    ///
    /// Mutation is only reachable through `&mut Document`, so no shared node
    /// reference can coexist with this borrow.
    pub(crate) fn tree_mut(&mut self) -> &mut Slab<Node<T>> {
        &mut self.nodes
    }

    /// The actual DOM document node, permanently stored at slot zero.
    ///
    /// # Panics
    ///
    /// Panics only if an internal invariant was violated and slot zero was
    /// removed or replaced; the public mutation API cannot do either.
    #[must_use]
    pub fn root_node(&self) -> &Node<T> {
        self.nodes
            .get(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
    }

    /// The first element child of the document node.
    ///
    /// This is derived from the real child list rather than cached as a
    /// second root pointer.
    ///
    /// # Panics
    ///
    /// Panics if an internal document child link does not resolve. Public
    /// mutation APIs preserve this invariant.
    #[must_use]
    pub fn root_element(&self) -> Option<&Node<T>> {
        self.root_node().children().find(|node| node.is_element())
    }

    /// Debug-only: mark the document as inside a style traversal until the
    /// returned token is dropped, including while unwinding.
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

    // --- node factories ---------------------------------------------------

    /// Create a detached element and return its raw slab index.
    pub fn create_element(&mut self, tag: &str, payload: T) -> NodeId {
        let local_name = LocalName::from(tag);
        self.allocate_node(|tree, id| Node::new_element(tree, id, local_name, payload))
    }

    /// Create a detached text node and return its raw slab index.
    pub fn create_text_node(&mut self, text: impl Into<String>, payload: T) -> NodeId {
        let text = text.into();
        self.allocate_node(|tree, id| Node::new_text(tree, id, text, payload))
    }

    fn allocate_node(
        &mut self,
        make: impl FnOnce(*mut Slab<Node<T>>, NodeId) -> Node<T>,
    ) -> NodeId {
        let tree = std::ptr::from_mut::<Slab<Node<T>>>(self.nodes.as_mut());
        let entry = self.nodes.vacant_entry();
        let id = entry.key();
        entry.insert(make(tree, id));
        id
    }

    // --- document node ----------------------------------------------------

    /// Attach one element beneath the document node.
    ///
    /// This DOM subset permits one element child. The relationship is stored
    /// in the slot-zero node's ordinary `children` list, and the element's
    /// parent is the document node just like every other DOM link.
    ///
    /// # Panics
    ///
    /// Panics if `child` is not a live element, is the document node, or the
    /// document already has a different element child.
    pub fn append_child(&mut self, child: NodeId) {
        assert_ne!(
            child, DOCUMENT_NODE_ID,
            "Document::append_child cannot append the document to itself"
        );
        assert!(
            self.get(child).is_some_and(Node::is_element),
            "Document::append_child requires a live element"
        );
        if self.root_element().map(Node::id) == Some(child) {
            return;
        }
        assert!(
            self.root_element().is_none(),
            "Document::append_child: a document may have only one element child"
        );

        self.detach(child);
        self.nodes
            .get_mut(DOCUMENT_NODE_ID)
            .expect("the document node is never removed")
            .children
            .push(child);
        self.nodes
            .get_mut(child)
            .expect("the attached child was validated as live")
            .parent = Some(DOCUMENT_NODE_ID);
        self.mark_subtree_dirty(child);
    }

    // --- queries ----------------------------------------------------------

    /// Borrow a node by its raw slab index.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node<T>> {
        self.nodes.get(id)
    }

    /// Whether the slab index is currently occupied.
    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains(id)
    }

    /// Whether `id` is connected beneath the slot-zero document node.
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

    /// The position of `child` in `parent`'s child list.
    #[must_use]
    pub fn child_position(&self, parent: NodeId, child: NodeId) -> Option<usize> {
        self.get(parent)?
            .child_ids()
            .iter()
            .position(|&candidate| candidate == child)
    }

    /// Whether `ancestor` is a strict ancestor of `descendant`.
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

    // --- structure --------------------------------------------------------

    /// Insert `child` into the element `parent` before `before`, or append.
    ///
    /// # Panics
    ///
    /// Panics for vacant IDs, a non-element parent, the document as `child`,
    /// an invalid insertion reference, or a link that would violate the tree
    /// invariants.
    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: Option<NodeId>) {
        debug_assert!(self.contains(parent), "insert_before: stale parent");
        debug_assert!(self.contains(child), "insert_before: stale child");
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

        self.nodes
            .get_mut(parent)
            .expect("stale NodeId passed to Document::insert_before")
            .children
            .insert(index, child);
        self.nodes
            .get_mut(child)
            .expect("stale NodeId passed to Document::insert_before")
            .parent = Some(parent);

        self.note_moved_subtree(child);
        self.note_child_list_change(parent, index);
    }

    /// Append `child` as the final child of the element `parent`.
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        self.insert_before(parent, child, None);
    }

    /// Detach a non-document node from its current parent.
    ///
    /// # Panics
    ///
    /// Panics if `child` is vacant/out of range or is the document node.
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
        self.nodes
            .get_mut(child)
            .expect("stale NodeId passed to Document::detach")
            .parent = None;

        if parent != DOCUMENT_NODE_ID {
            self.note_child_list_change(parent, removed_index);
        }
    }

    /// Remove `id` and every descendant, returning their embedder payloads.
    ///
    /// # Panics
    ///
    /// Panics if `id` is vacant/out of range or is the document node.
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
                .nodes
                .try_remove(current)
                .expect("subtree links always resolve while removing");
            stack.extend_from_slice(&node.children);
            removed.push(node.into_payload());
        }
        // Parked roots carry raw, generation-less slab ids. Prune every
        // removed entry once, after the traversal, before a later node factory
        // can reuse any vacant slot. Keeping the slab borrow separate lets the
        // two vectors be borrowed independently and avoids an O(nodes × roots)
        // scan for large removed subtrees.
        let nodes = &self.nodes;
        self.relayout_roots
            .retain(|&(parked_id, _)| nodes.contains(parked_id));
        removed
    }

    // --- flush bookkeeping ------------------------------------------------

    /// Move reachable pending snapshots into Stylo's temporary map.
    pub(crate) fn take_snapshot_map(&mut self, root: NodeId) -> SnapshotMap {
        let mut snapshots = SnapshotMap::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(node) = self.nodes.get_mut(id) else {
                continue;
            };
            debug_assert_eq!(
                node.snapshot.is_some(),
                node.snapshot_present(),
                "snapshot slot and lifecycle flag diverged before flush"
            );
            if let Some(snapshot) = node.snapshot.take() {
                snapshots.insert(OpaqueNode(node.id()), *snapshot);
            }
            if node.has_dirty_descendants() {
                stack.extend_from_slice(&node.children);
            }
        }
        snapshots
    }

    /// Harvest the damage a style traversal produced and clear all of stylo's
    /// per-node restyle state, called once from
    /// [`Document::flush_styles_with_sink`](Self::flush_styles_with_sink)
    /// after the traversal returns.
    ///
    /// Two passes:
    /// 1. **Snapshot cleanup.** Clears the snapshot lifecycle bits on exactly the snapshotted set
    ///    (the [`SnapshotMap`] keys, so a snapshot on a node pruned mid-flush by a `display: none`
    ///    ancestor is still cleared). The per-node snapshot boxes were already drained into
    ///    `snapshots` by [`take_snapshot_map`](Self::take_snapshot_map), which is dropped when the
    ///    flush returns.
    /// 2. **Spine walk + harvest.** Walks from `root`, descending only where `dirty_descendants` is
    ///    set (the bit stylo sets while descending to restyled nodes and — in this postorder-less
    ///    servo config — never clears). `root` is always inspected even with no dirty bits. For
    ///    each visited node with style data it copies `ElementData::damage`, calls
    ///    `ElementData::clear_restyle_state` (draining `hint` + `damage` + the restyle flags),
    ///    unsets `dirty_descendants`, and clears the snapshot bits. If the copied damage is
    ///    non-empty, it consumes any relayout-class effect into the document's layout caches and
    ///    then streams `(id, StyleDamage(damage))` to `sink`. Because text nodes read inherited
    ///    text style from their direct parent but carry no stylo data of their own, relayout damage
    ///    on an element also clears each direct text child's box cache and retained Parley
    ///    artifacts.
    ///
    /// Consuming layout damage here is load-bearing: callers may legitimately
    /// discard [`FlushSummary`](crate::FlushSummary), and a later
    /// [`Document::layout`](Self::layout)
    /// performs a no-op style flush. Invalidating while the harvested ids are
    /// known-live avoids retaining the damaged ids themselves. Boundary-stopped
    /// invalidation may park live ancestor [`NodeId`]s on the document; the
    /// next layout pass consumes those roots, and subtree removal purges them
    /// before their slab slots can be reused.
    ///
    /// Clearing damage on harvest is the fix for a latent re-traversal bug:
    /// stylo never clears damage for a normal restyle, and in servo builds
    /// `element_needs_traversal` (`vendor/stylo/style/traversal.rs:226-228`)
    /// returns `true` for any element with non-empty damage — so without this
    /// pass every previously-restyled node would be re-traversed on every
    /// subsequent flush. The traversal already drained the visited nodes'
    /// hints (via `RestyleHint::propagate`'s `mem::replace`); re-clearing them
    /// here is belt-and-braces.
    ///
    /// The no-lingering-snapshot guarantee is scoped to **connected** nodes: a
    /// node snapshotted and then detached before the flush keeps its slot and
    /// `SNAPSHOT_PRESENT` bit (it is in neither the collected map nor the
    /// spine). That orphan state is inert while detached, is dominated by the
    /// subtree restyle a reattach schedules, and is dropped with the detached
    /// node if it is reclaimed.
    ///
    /// # Safety discipline (crate-internal)
    ///
    /// Runs under `&mut Document` after `driver::traverse_dom` has returned, so
    /// no rayon worker is concurrently touching any `ElementData` `UnsafeCell`;
    /// the `stylo_data_mut` reborrow below is exclusive.
    pub(crate) fn harvest_flush(
        &mut self,
        root: NodeId,
        snapshots: &SnapshotMap,
        sink: &mut dyn FnMut(NodeId, StyleDamage),
    ) {
        for opaque in snapshots.keys() {
            if let Some(node) = self.nodes.get(opaque.0) {
                node.clear_snapshot_flags();
            }
        }

        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let harvested = {
                let Some(node) = self.nodes.get_mut(current) else {
                    continue;
                };
                let mut harvested = None;
                if let Some(wrapper) = node.stylo_data_mut() {
                    let mut data = wrapper.borrow_mut();
                    let damage = data.damage;
                    data.clear_restyle_state();
                    if !damage.is_empty() {
                        harvested = Some(StyleDamage::from(damage));
                    }
                }
                let descend = node.has_dirty_descendants();
                node.set_dirty_descendants_bit(false);
                node.clear_snapshot_flags();
                if descend {
                    stack.extend_from_slice(&node.children);
                }
                harvested
            };
            let Some(damage) = harvested else {
                continue;
            };

            if damage.needs_relayout() {
                // Text nodes never receive stylo damage themselves, yet their
                // measurement depends on inherited values read from the direct
                // parent's ComputedValues. Clear both layers of their retained
                // state: the box cache alone is insufficient because Parley
                // would otherwise reuse the stale committed shape artifact.
                if let Some(element) = self.get(current) {
                    for child in element.children().filter(|child| child.is_text_node()) {
                        child.layout_data.borrow_mut().clear_measurement_cache();
                        child.invalidate_text_artifacts();
                    }
                }
                self.invalidate_layout(current);
                if damage.is_reconstruct() {
                    // Box generation changed, so the parent must re-collect
                    // its children as well as the node clearing its own cache.
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

        // SAFETY: the flag is stored in the slot-zero node, which cannot be
        // removed and outlives this token. It is atomic specifically so the
        // token need not retain a Rust borrow across the mutable flush.
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
    #[should_panic(expected = "internal tree links always resolve")]
    fn root_element_panics_on_a_dangling_document_child() {
        let mut document: Document<()> = Document::new(device());
        document
            .nodes
            .get_mut(DOCUMENT_NODE_ID)
            .expect("the document node is present")
            .children
            .push(usize::MAX);

        let _ = document.root_element();
    }
}
