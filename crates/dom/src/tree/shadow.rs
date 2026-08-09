//! Shadow roots, slot assignment, and the flat tree.
//!
//! Three trees coexist once a shadow root exists, exactly as the DOM and CSS
//! Scoping standards define them:
//!
//! * the **node tree** — [`Node::parent`]/[`Node::children`]. Selectors match against it and the
//!   public navigation API reports it. A shadow root is *not* one of its host's children, so a
//!   host's child list stays its light children alone.
//! * the **shadow tree** — the node tree rooted at a shadow root, reached from its host only
//!   through [`Document::shadow_root`].
//! * the **flat tree** — the node tree with every host replaced by its shadow tree, and every
//!   `<slot>` replaced by the nodes assigned to it (or, when nothing is assigned, by the slot's own
//!   children as fallback content). Style inheritance, layout, painting, and hit testing read this
//!   tree and nothing else, which is what makes a shadow tree render at all.
//!
//! Slot assignment is eager: every mutation that can change it (a host's child
//! list, a shadow tree's slot set, a `slot` or `name` attribute) resolves the
//! affected tree's assignment in the same call, so no consumer ever has to
//! remember to resolve a pending assignment first. The whole hook costs one
//! branch on [`Document::has_shadow_roots`] in a document that has none.
//!
//! Eager does not mean recomputing the tree. Appending a light child to a host
//! and removing one — the two mutations a list does per row — update only the
//! slot involved; a full reassignment is reserved for the cases that can
//! reorder or re-target more than one node (a changed slot set, a changed
//! `slot`/`name`, a non-append insertion). `benches/shadow.rs` measures the
//! difference: with the append path reassigning the whole tree, building a
//! 1024-row host cost 51× the same rows with no shadow root; it now costs
//! 1.4×.

use std::sync::LazyLock;

use stylo::LocalName;
use stylo::author_styles::AuthorStyles;
use stylo::stylesheets::DocumentStyleSheet;

use crate::tree::document::{Document, NodeId, PayloadSlot};
use crate::tree::node::Node;

/// The `<slot>` element, and the `slot` attribute a slottable names its slot
/// with — one atom serves both, since a local name is a local name.
///
/// W3C vocabulary, not Lynx vocabulary: `<slot>` is defined by the HTML
/// standard as shadow DOM's own distribution point, so the generic DOM core
/// owns it exactly the way it already owns `id`, `class`, and `style`.
static SLOT: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("slot"));
/// The `name` a slot offers.
static NAME: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("name"));
/// CSS Shadow Parts' two attributes: what a shadow tree exposes outward, and
/// what a host forwards further outward on its behalf.
static PART: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("part"));
static EXPORT_PARTS: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("exportparts"));

/// Whether an attribute name can change a slot assignment: `slot` names the
/// slot a slottable wants, `name` names the slot itself.
pub(crate) fn is_slot_assignment_attribute(name: &str) -> bool {
    matches!(name, "slot" | "name")
}

/// Whether a shadow root is reachable from script (`ShadowRootMode`).
///
/// The DOM core has no script binding of its own, so this is recorded rather
/// than enforced: [`Document::shadow_root`] is the engine's own view and
/// answers for either mode. A binding layer above gates on
/// [`Document::shadow_root_mode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowRootMode {
    Open,
    Closed,
}

/// A shadow-root node's own state.
///
/// Boxed inside `NodeData` so the variant costs an element or text node
/// nothing: the scoped stylesheet set alone is far wider than the primary
/// arena's stride.
pub(crate) struct ShadowRootData {
    pub(crate) host: NodeId,
    pub(crate) mode: ShadowRootMode,
    /// This shadow root's scoped author stylesheets and the `CascadeData`
    /// Stylo matches them from. Rules here apply to this shadow tree only,
    /// which is the encapsulation half of shadow DOM.
    pub(crate) styles: AuthorStyles<DocumentStyleSheet>,
    /// This tree's `<slot>` elements in node-tree order, rebuilt only when a
    /// mutation changes the slot set — which lets an ordinary append cost one
    /// name comparison per slot instead of a walk of the whole tree.
    ///
    /// A cache, not observable state: every assignment materializes it before
    /// use, and a debug assertion re-derives it on each hit.
    slots: Option<Vec<NodeId>>,
}

impl ShadowRootData {
    fn new(host: NodeId, mode: ShadowRootMode) -> Self {
        Self {
            host,
            mode,
            styles: AuthorStyles::new(),
            slots: None,
        }
    }
}

/// The shadow-DOM links of one node, allocated only for the nodes that have
/// any — a host, a slot, or a slotted node. Every other node keeps a single
/// `None` word, so a document with no shadow root pays one predictable branch
/// and no extra cache line.
#[derive(Default)]
pub(crate) struct ShadowLinks {
    /// The shadow root this element hosts.
    pub(crate) shadow_root: Option<NodeId>,
    /// The slot this node is assigned to, when it is a host's light child that
    /// slot assignment matched.
    pub(crate) assigned_slot: Option<NodeId>,
    /// A slot's assigned nodes, in the host's child order.
    pub(crate) assigned_nodes: Vec<NodeId>,
}

impl<T> Node<T> {
    /// Whether this element is a `<slot>`. Only meaningful inside a shadow
    /// tree, which is the only place slot assignment ever runs.
    #[must_use]
    pub(crate) fn is_slot(&self) -> bool {
        self.local_name.as_ref().is_some_and(|name| *name == *SLOT)
    }

    /// This node's flat-tree children, always a slice of the arena so every
    /// consumer keeps iterating a plain `&[NodeId]`.
    pub(crate) fn flat_children(&self) -> &[NodeId] {
        let Some(links) = self.shadow.as_deref() else {
            return &self.children;
        };
        if let Some(root) = links.shadow_root {
            // A host renders its shadow tree; its light children reach the
            // flat tree only through the slots that claimed them.
            return &self
                .tree()
                .get(root)
                .expect("a host's shadow root outlives the host")
                .children;
        }
        if links.assigned_nodes.is_empty() {
            // Either not a slot at all, or a slot with nothing assigned — in
            // which case its own children are its fallback content.
            &self.children
        } else {
            &links.assigned_nodes
        }
    }

    /// This node's flat-tree parent.
    ///
    /// `None` for a light child of a host that no slot claimed: an unassigned
    /// slottable is not in the flat tree at all, so it is not styled, laid
    /// out, or painted. A shadow root reports its host, which keeps the
    /// dirty-descendant and layout-invalidation spines connected even though
    /// the shadow root itself generates no box.
    pub(crate) fn flat_parent_id(&self) -> Option<NodeId> {
        if let Some(links) = self.shadow.as_deref()
            && let Some(slot) = links.assigned_slot
        {
            return Some(slot);
        }
        if let Some(host) = self.shadow_host_id() {
            return Some(host);
        }
        let parent_id = self.parent?;
        let parent = self
            .tree()
            .get(parent_id)
            .expect("internal tree links always resolve");
        if let Some(host) = parent.shadow_host_id() {
            return Some(host);
        }
        if parent.shadow_root_id().is_some() {
            return None;
        }
        Some(parent_id)
    }

    #[must_use]
    pub(crate) fn flat_parent(&self) -> Option<&Node<T>> {
        self.flat_parent_id().map(|id| {
            self.tree()
                .get(id)
                .expect("internal tree links always resolve")
        })
    }

    /// The shadow root this element hosts.
    #[must_use]
    pub(crate) fn shadow_root_id(&self) -> Option<NodeId> {
        self.shadow.as_deref()?.shadow_root
    }

    /// The slot this node is assigned to.
    #[must_use]
    pub(crate) fn assigned_slot_id(&self) -> Option<NodeId> {
        self.shadow.as_deref()?.assigned_slot
    }

    /// The nodes assigned to this slot, in the host's child order.
    #[must_use]
    pub(crate) fn assigned_node_ids(&self) -> &[NodeId] {
        self.shadow
            .as_deref()
            .map_or(&[][..], |links| &links.assigned_nodes)
    }

    /// The shadow root whose tree this node is in.
    ///
    /// Node-tree ancestry only, so it stops at the shadow root rather than
    /// continuing into the host's light tree — that boundary is exactly what
    /// scopes a selector to one shadow tree. A shadow root itself reports the
    /// *outer* tree its host sits in, which is how `::part()` walks out
    /// through nested trees.
    #[must_use]
    pub(crate) fn containing_shadow_root(&self) -> Option<&Node<T>> {
        let tree = self.tree();
        let mut current = self.parent;
        while let Some(id) = current {
            let node = tree.get(id).expect("internal tree links always resolve");
            if node.is_shadow_root() {
                return Some(node);
            }
            current = node.parent;
        }
        None
    }

    /// Whether this element exposes any shadow part to its outer tree
    /// (`part="…"`, CSS Shadow Parts).
    #[must_use]
    pub(crate) fn has_part_attr(&self) -> bool {
        self.attr_local_name(&PART).is_some()
    }

    #[must_use]
    pub(crate) fn is_part(&self, name: &str) -> bool {
        self.part_names().any(|part| part == name)
    }

    /// The part names this element exposes, in `part` order.
    pub(crate) fn part_names(&self) -> impl Iterator<Item = &str> {
        self.attr_local_name(&PART)
            .unwrap_or_default()
            .split_whitespace()
    }

    /// This host's `exportparts` entries as `(inner, exported)` pairs. An
    /// entry is either `inner` (forwarded unrenamed) or `inner: exported`.
    fn exported_parts(&self) -> impl Iterator<Item = (&str, &str)> {
        self.attr_local_name(&EXPORT_PARTS)
            .unwrap_or_default()
            .split(',')
            .filter_map(|mapping| {
                let (inner, exported) = mapping.split_once(':').map_or_else(
                    || {
                        let inner = mapping.trim();
                        (inner, inner)
                    },
                    |(inner, exported)| (inner.trim(), exported.trim()),
                );
                (!inner.is_empty() && !exported.is_empty()).then_some((inner, exported))
            })
    }

    /// The names this host re-exports `inner` under — a callback rather than
    /// an `Option`, because one inner part may be exported several times.
    pub(crate) fn each_exported_part(&self, inner: &str, mut callback: impl FnMut(&str)) {
        for (name, exported) in self.exported_parts() {
            if name == inner {
                callback(exported);
            }
        }
    }

    /// Whether this host forwards any of its shadow tree's parts outward
    /// (`exportparts="…"`).
    #[must_use]
    pub(crate) fn exports_any_part(&self) -> bool {
        self.attr_local_name(&EXPORT_PARTS).is_some()
    }

    /// Translates a part name written in this host's *outer* tree into the
    /// name it has inside this host's shadow tree.
    #[must_use]
    pub(crate) fn imported_part(&self, outer: &str) -> Option<&str> {
        self.exported_parts()
            .find_map(|(inner, exported)| (exported == outer).then_some(inner))
    }

    pub(crate) fn links_mut(&mut self) -> &mut ShadowLinks {
        self.shadow.get_or_insert_default()
    }

    /// Drops a stale slot assignment when a node leaves its host's child list.
    /// Reassignment only visits the host's *current* children, so the node
    /// leaving is exactly the one it cannot reach.
    pub(crate) fn clear_assigned_slot(&mut self) {
        if let Some(links) = self.shadow.as_deref_mut() {
            links.assigned_slot = None;
        }
    }
}

impl<T> Document<T> {
    /// Attaches a shadow root to `host` and returns it. Append children to it
    /// with the ordinary [`Document::append_child`]; from that moment the host
    /// renders its shadow tree instead of its own children.
    ///
    /// Panics if `host` already hosts one — `attachShadow()` is once per
    /// element, and the DOM core is crash-on-misuse.
    pub fn attach_shadow(&mut self, host: NodeId, mode: ShadowRootMode) -> NodeId {
        assert!(
            self.get(host).is_some_and(Node::is_element),
            "Document::attach_shadow: host must be a live element"
        );
        assert!(
            self.get(host).and_then(Node::shadow_root_id).is_none(),
            "Document::attach_shadow: the host already hosts a shadow root"
        );
        let root = self.allocate_node(PayloadSlot::ShadowRoot, |owner, id| {
            Node::new_shadow_root(owner, id, ShadowRootData::new(host, mode))
        });
        self.live_node_mut(root).parent = Some(host);
        self.live_node_mut(host).links_mut().shadow_root = Some(root);
        self.note_shadow_root_added();
        // The host's light children just left the flat tree — nothing is
        // assigned yet, because the shadow tree has no slots yet.
        self.assign_slots(root);
        self.mark_subtree_dirty(host);
        self.invalidate_layout(host);
        root
    }

    /// The shadow root `host` hosts, in either mode.
    #[must_use]
    pub fn shadow_root(&self, host: NodeId) -> Option<NodeId> {
        self.get(host)?.shadow_root_id()
    }

    /// The element a shadow root is attached to.
    #[must_use]
    pub fn shadow_host(&self, shadow_root: NodeId) -> Option<NodeId> {
        self.get(shadow_root)?.shadow_host_id()
    }

    #[must_use]
    pub fn shadow_root_mode(&self, shadow_root: NodeId) -> Option<ShadowRootMode> {
        self.get(shadow_root)?.shadow_root_mode()
    }

    /// The slot a host's light child was assigned to, if any claimed it.
    #[must_use]
    pub fn assigned_slot(&self, node: NodeId) -> Option<NodeId> {
        self.get(node)?.assigned_slot_id()
    }

    /// The nodes a slot renders, in the host's child order. Empty when the
    /// slot shows its fallback content instead.
    #[must_use]
    pub fn assigned_nodes(&self, slot: NodeId) -> &[NodeId] {
        self.get(slot).map_or(&[][..], Node::assigned_node_ids)
    }

    /// The shadow tree `node` belongs to: itself when `node` is the shadow
    /// root, otherwise the nearest shadow root above it.
    #[must_use]
    fn shadow_root_of(&self, node: NodeId) -> Option<NodeId> {
        if self.get(node)?.is_shadow_root() {
            return Some(node);
        }
        self.containing_shadow_root(node)
    }

    /// The shadow root whose tree `node` is in, walking out through nested
    /// shadow trees but never into a host's light tree.
    #[must_use]
    pub(crate) fn containing_shadow_root(&self, node: NodeId) -> Option<NodeId> {
        let mut current = self.get(node)?.parent_id();
        while let Some(id) = current {
            let candidate = self.get(id)?;
            if candidate.is_shadow_root() {
                return Some(id);
            }
            current = candidate.parent_id();
        }
        None
    }

    /// Reassigns after `child` was linked into `parent`, `appended` when it
    /// went on the end of the child list.
    ///
    /// Appending to a host is the hot path — it is what building a list does,
    /// once per row — so it takes the incremental route: the new child is last
    /// in host order, therefore last in whatever slot claims it, and no other
    /// node's assignment can have changed. Reassigning the whole tree per
    /// append instead made construction quadratic in the host's child count
    /// (measured: 1024 rows went 51× slower than the same rows with no shadow
    /// root; `benches/shadow.rs::build_wide_host_{plain,shadow}`).
    pub(crate) fn note_slot_assignment_inserted(
        &mut self,
        parent: NodeId,
        child: NodeId,
        appended: bool,
    ) {
        if !self.has_shadow_roots() {
            return;
        }
        if let Some(root) = self.get(parent).and_then(Node::shadow_root_id) {
            if appended {
                self.assign_appended_slottable(root, child);
            } else {
                self.assign_slots(root);
            }
        }
        self.note_slot_set_change(parent, child);
    }

    /// Reassigns after `child` was unlinked from `parent`. Removing a light
    /// child can only take it out of its own slot, so that is all this does.
    pub(crate) fn note_slot_assignment_removed(&mut self, parent: NodeId, child: NodeId) {
        if !self.has_shadow_roots() {
            return;
        }
        if self.get(parent).and_then(Node::shadow_root_id).is_some() {
            self.unassign_slottable(child);
        }
        self.note_slot_set_change(parent, child);
    }

    /// Handles the other half of a child-list change: a subtree that carries a
    /// `<slot>` moving into or out of a shadow tree changes which slots exist,
    /// which is the one case that needs the whole tree reassigned. A subtree
    /// with no slot in it — every ordinary shadow-tree mutation — costs one
    /// walk of the subtree that was just created anyway.
    fn note_slot_set_change(&mut self, parent: NodeId, child: NodeId) {
        // `shadow_root_of`, not `containing_shadow_root`: a slot appended
        // straight to the shadow root changes *that* tree's slot set, and the
        // shadow root has no shadow root above it to find.
        let Some(root) = self.shadow_root_of(parent) else {
            return;
        };
        if !self.subtree_contains_slot(child) {
            return;
        }
        self.invalidate_slot_cache(root);
        self.assign_slots(root);
    }

    fn subtree_contains_slot(&self, id: NodeId) -> bool {
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let Some(node) = self.get(current) else {
                continue;
            };
            if node.is_slot() {
                return true;
            }
            // A nested host's shadow tree has its own slots, which belong to
            // its own assignment, so the walk stays in the node tree.
            stack.extend_from_slice(node.child_ids());
        }
        false
    }

    /// Assigns one freshly appended light child, leaving every other node's
    /// assignment alone.
    fn assign_appended_slottable(&mut self, shadow_root: NodeId, child: NodeId) {
        let is_slottable = self
            .get(child)
            .is_some_and(|node| node.is_element() || node.is_text_node());
        if !is_slottable {
            return;
        }
        self.ensure_slot_cache(shadow_root);
        let Some(slot) = self.matching_slot(shadow_root, child) else {
            // No slot claims it, so it is simply not in the flat tree — the
            // same state it was already in.
            return;
        };
        self.live_node_mut(slot)
            .links_mut()
            .assigned_nodes
            .push(child);
        self.live_node_mut(child).links_mut().assigned_slot = Some(slot);
        // The dirty spine the traversal will descend runs through the slot,
        // not through the host's child list, so it starts at the child.
        self.mark_ancestors_dirty_descendants(child);
    }

    /// Drops one node from the slot holding it.
    fn unassign_slottable(&mut self, child: NodeId) {
        let Some(slot) = self.get(child).and_then(Node::assigned_slot_id) else {
            return;
        };
        // While the link still points at the slot, so the walk goes out
        // through the flat tree the node is leaving.
        self.mark_ancestors_dirty_descendants(child);
        if let Some(links) = self.live_node_mut(slot).shadow.as_deref_mut()
            && let Some(index) = links
                .assigned_nodes
                .iter()
                .position(|&assigned| assigned == child)
        {
            links.assigned_nodes.remove(index);
        }
        self.live_node_mut(child).clear_assigned_slot();
    }

    /// The first slot in the tree whose `name` matches what `child` asks for.
    fn matching_slot(&self, shadow_root: NodeId, child: NodeId) -> Option<NodeId> {
        let wanted = self.get(child)?.attr_local_name(&SLOT).unwrap_or_default();
        let slots = self.get(shadow_root)?.shadow_data()?.slots.as_ref()?;
        slots.iter().copied().find(|&slot| {
            self.get(slot)
                .is_some_and(|slot| slot.attr_local_name(&NAME).unwrap_or_default() == wanted)
        })
    }

    /// The tree's `<slot>` elements in node-tree order.
    fn collect_slots(&self, shadow_root: NodeId) -> Vec<NodeId> {
        let mut slots = Vec::new();
        let mut stack: Vec<NodeId> = self
            .live(shadow_root)
            .child_ids()
            .iter()
            .rev()
            .copied()
            .collect();
        while let Some(id) = stack.pop() {
            let node = self.live(id);
            if node.is_slot() {
                slots.push(id);
            }
            // The node tree only: a nested host's own shadow tree is a tree of
            // its own, and its slots claim *its* light children, not ours.
            stack.extend(node.child_ids().iter().rev().copied());
        }
        slots
    }

    fn ensure_slot_cache(&mut self, shadow_root: NodeId) {
        let cached = self
            .get(shadow_root)
            .and_then(Node::shadow_data)
            .is_some_and(|shadow| shadow.slots.is_some());
        if cached {
            debug_assert_eq!(
                self.get(shadow_root)
                    .and_then(Node::shadow_data)
                    .and_then(|shadow| shadow.slots.as_deref()),
                Some(self.collect_slots(shadow_root).as_slice()),
                "the cached slot list diverged from the shadow tree — a mutation that changed \
                 the slot set did not invalidate it"
            );
            return;
        }
        let slots = self.collect_slots(shadow_root);
        self.install_slot_cache(shadow_root, Some(slots));
    }

    fn invalidate_slot_cache(&mut self, shadow_root: NodeId) {
        self.install_slot_cache(shadow_root, None);
    }

    fn install_slot_cache(&mut self, shadow_root: NodeId, slots: Option<Vec<NodeId>>) {
        if let Some(shadow) = self.live_node_mut(shadow_root).shadow_data_mut() {
            shadow.slots = slots;
        }
    }

    /// Recomputes slot assignment after a `slot` or `name` attribute changed
    /// on `node`: `slot` renames what a slottable asks for, `name` renames
    /// what a slot offers.
    pub(crate) fn note_slot_assignment_attribute(&mut self, node: NodeId) {
        if !self.has_shadow_roots() {
            return;
        }
        if let Some(root) = self
            .get(node)
            .and_then(Node::parent_id)
            .and_then(|parent| self.get(parent))
            .and_then(Node::shadow_root_id)
        {
            self.assign_slots(root);
        }
        if let Some(root) = self.containing_shadow_root(node) {
            self.assign_slots(root);
        }
    }

    /// DOM "assign slottables for a tree": walk the shadow tree's slots in
    /// node-tree order, then hand each of the host's light children to the
    /// first slot whose `name` matches its `slot`.
    pub(crate) fn assign_slots(&mut self, shadow_root: NodeId) {
        let Some(host) = self.get(shadow_root).and_then(Node::shadow_host_id) else {
            return;
        };

        self.ensure_slot_cache(shadow_root);
        // A missing `name`/`slot` and an empty one are the same name — the
        // default slot — so both sides normalize to a plain string.
        let mut slots: Vec<(NodeId, String, Vec<NodeId>)> = self
            .get(shadow_root)
            .and_then(Node::shadow_data)
            .and_then(|shadow| shadow.slots.as_deref())
            .unwrap_or_default()
            .iter()
            .map(|&slot| {
                let name = self
                    .live(slot)
                    .attr_local_name(&NAME)
                    .unwrap_or_default()
                    .to_owned();
                (slot, name, Vec::new())
            })
            .collect();

        let light = self.live(host).child_ids().to_vec();
        let mut assignments: Vec<(NodeId, Option<NodeId>)> = Vec::with_capacity(light.len());
        for child in light {
            let node = self.live(child);
            if !(node.is_element() || node.is_text_node()) {
                continue;
            }
            // A text node carries no attributes, so it always asks for the
            // default slot.
            let wanted = node.attr_local_name(&SLOT).unwrap_or_default();
            let matched = slots.iter_mut().find(|(_, name, _)| name == wanted);
            match matched {
                Some((slot, _, nodes)) => {
                    let slot = *slot;
                    nodes.push(child);
                    assignments.push((child, Some(slot)));
                }
                None => assignments.push((child, None)),
            }
        }

        let mut changed = false;
        for (child, slot) in assignments {
            let node = self.live_node_mut(child);
            if node.assigned_slot_id() != slot {
                node.links_mut().assigned_slot = slot;
                changed = true;
            }
        }
        for (slot, _, nodes) in slots {
            let node = self.live_node_mut(slot);
            if node.assigned_node_ids() != nodes.as_slice() {
                node.links_mut().assigned_nodes = nodes;
                changed = true;
            }
        }

        if changed {
            // The flat tree under the host moved: what each slot renders, and
            // which light children render at all, both changed.
            self.mark_subtree_dirty(host);
            self.invalidate_layout(host);
        }
    }
}
