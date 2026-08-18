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

use crate::tree::document::{Document, NodeId, NodeSlot, PayloadSlot};
use crate::tree::node::Node;

static SLOT: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("slot"));
static NAME: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("name"));
static PART: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("part"));
static EXPORT_PARTS: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("exportparts"));

pub(crate) fn is_slot_assignment_attribute(name: &str) -> bool {
    matches!(name, "slot" | "name")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowRootMode {
    Open,
    Closed,
}

/// State owned by a shadow-root node.
pub(crate) struct ShadowRootData {
    pub(crate) host: NodeSlot,
    pub(crate) mode: ShadowRootMode,
    pub(crate) styles: AuthorStyles<DocumentStyleSheet>,
    slots: Option<Vec<NodeId>>,
}

impl ShadowRootData {
    fn new(host: NodeSlot, mode: ShadowRootMode) -> Self {
        Self {
            host,
            mode,
            styles: AuthorStyles::new(),
            slots: None,
        }
    }
}

/// Optional host, slot, and assignment links.
#[derive(Default)]
pub(crate) struct ShadowLinks {
    pub(crate) shadow_root: Option<NodeSlot>,
    pub(crate) assigned_slot: Option<NodeSlot>,
    pub(crate) assigned_nodes: Vec<NodeSlot>,
}

impl<T> Node<T> {
    #[must_use]
    pub(crate) fn is_slot(&self) -> bool {
        self.local_name.as_ref().is_some_and(|name| *name == *SLOT)
    }

    pub(crate) fn flat_children(&self) -> &[NodeSlot] {
        let Some(links) = self.shadow.as_deref() else {
            return &self.children;
        };
        if let Some(root) = links.shadow_root {
            return &self.arenas().at(root).children;
        }
        if links.assigned_nodes.is_empty() {
            &self.children
        } else {
            &links.assigned_nodes
        }
    }

    pub(crate) fn flat_parent_slot(&self) -> Option<NodeSlot> {
        if let Some(links) = self.shadow.as_deref()
            && let Some(slot) = links.assigned_slot
        {
            return Some(slot);
        }
        if let Some(host) = self.shadow_host_slot() {
            return Some(host);
        }
        let parent_slot = self.parent?;
        let parent = self.arenas().at(parent_slot);
        if let Some(host) = parent.shadow_host_slot() {
            return Some(host);
        }
        if parent.shadow_root_slot().is_some() {
            return None;
        }
        Some(parent_slot)
    }

    #[must_use]
    pub(crate) fn flat_parent_id(&self) -> Option<NodeId> {
        self.flat_parent_slot()
            .map(|slot| self.arenas().at(slot).id())
    }

    #[must_use]
    pub(crate) fn flat_parent(&self) -> Option<&Node<T>> {
        self.flat_parent_slot().map(|slot| self.arenas().at(slot))
    }

    #[must_use]
    pub(crate) fn shadow_root_slot(&self) -> Option<NodeSlot> {
        self.shadow.as_deref()?.shadow_root
    }

    #[must_use]
    pub(crate) fn shadow_root_id(&self) -> Option<NodeId> {
        self.shadow_root_slot()
            .map(|slot| self.arenas().at(slot).id())
    }

    #[must_use]
    pub(crate) fn assigned_slot_slot(&self) -> Option<NodeSlot> {
        self.shadow.as_deref()?.assigned_slot
    }

    #[must_use]
    pub(crate) fn assigned_slot_id(&self) -> Option<NodeId> {
        self.assigned_slot_slot()
            .map(|slot| self.arenas().at(slot).id())
    }

    #[must_use]
    pub(crate) fn assigned_node_slots(&self) -> &[NodeSlot] {
        self.shadow
            .as_deref()
            .map_or(&[][..], |links| &links.assigned_nodes)
    }

    pub(crate) fn assigned_node_ids(&self) -> impl ExactSizeIterator<Item = NodeId> + '_ {
        let arenas = self.arenas();
        self.assigned_node_slots()
            .iter()
            .map(move |&slot| arenas.at(slot).id())
    }

    #[must_use]
    pub(crate) fn containing_shadow_root(&self) -> Option<&Node<T>> {
        let tree = self.arenas();
        let mut current = self.parent;
        while let Some(slot) = current {
            let node = tree.at(slot);
            if node.is_shadow_root() {
                return Some(node);
            }
            current = node.parent;
        }
        None
    }

    #[must_use]
    pub(crate) fn has_part_attr(&self) -> bool {
        self.attr_local_name(&PART).is_some()
    }

    #[must_use]
    pub(crate) fn is_part(&self, name: &str) -> bool {
        self.part_names().any(|part| part == name)
    }

    pub(crate) fn part_names(&self) -> impl Iterator<Item = &str> {
        self.attr_local_name(&PART)
            .unwrap_or_default()
            .split_whitespace()
    }

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

    pub(crate) fn each_exported_part(&self, inner: &str, mut callback: impl FnMut(&str)) {
        for (name, exported) in self.exported_parts() {
            if name == inner {
                callback(exported);
            }
        }
    }

    #[must_use]
    pub(crate) fn exports_any_part(&self) -> bool {
        self.attr_local_name(&EXPORT_PARTS).is_some()
    }

    #[must_use]
    pub(crate) fn imported_part(&self, outer: &str) -> Option<&str> {
        self.exported_parts()
            .find_map(|(inner, exported)| (exported == outer).then_some(inner))
    }

    pub(crate) fn links_mut(&mut self) -> &mut ShadowLinks {
        self.shadow.get_or_insert_default()
    }

    pub(crate) fn clear_assigned_slot(&mut self) {
        if let Some(links) = self.shadow.as_deref_mut() {
            links.assigned_slot = None;
        }
    }
}

impl<T> Document<T> {
    /// Attaches and returns a shadow root.
    pub fn attach_shadow(&mut self, host: NodeId, mode: ShadowRootMode) -> NodeId {
        assert!(
            self.get(host).is_some_and(Node::is_element),
            "Document::attach_shadow: host must be a live element"
        );
        assert!(
            self.get(host).and_then(Node::shadow_root_id).is_none(),
            "Document::attach_shadow: the host already hosts a shadow root"
        );
        let host_slot = self.live_slot(host);
        let root = self.allocate_node(PayloadSlot::ShadowRoot, |owner, id, slot| {
            Node::new_shadow_root(owner, id, slot, ShadowRootData::new(host_slot, mode))
        });
        let root_slot = self.live_slot(root);
        self.live_node_mut(root).parent = Some(host_slot);
        self.live_node_mut(host).links_mut().shadow_root = Some(root_slot);
        self.note_shadow_root_added();
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
    pub fn assigned_nodes(&self, slot: NodeId) -> Vec<NodeId> {
        self.get(slot)
            .map(|node| node.assigned_node_ids().collect())
            .unwrap_or_default()
    }

    #[must_use]
    fn shadow_root_of(&self, node: NodeId) -> Option<NodeId> {
        if self.get(node)?.is_shadow_root() {
            return Some(node);
        }
        self.containing_shadow_root(node)
    }

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

    pub(crate) fn note_slot_assignment_removed(&mut self, parent: NodeId, child: NodeId) {
        if !self.has_shadow_roots() {
            return;
        }
        if self.get(parent).and_then(Node::shadow_root_id).is_some() {
            self.unassign_slottable(child);
        }
        self.note_slot_set_change(parent, child);
    }

    fn note_slot_set_change(&mut self, parent: NodeId, child: NodeId) {
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
        let Some(root) = self.slot(id) else {
            return false;
        };
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let node = self.arenas().at(current);
            if node.is_slot() {
                return true;
            }
            stack.extend_from_slice(node.child_slots());
        }
        false
    }

    fn assign_appended_slottable(&mut self, shadow_root: NodeId, child: NodeId) {
        let is_slottable = self
            .get(child)
            .is_some_and(|node| node.is_element() || node.is_text_node());
        if !is_slottable {
            return;
        }
        self.ensure_slot_cache(shadow_root);
        let Some(slot) = self.matching_slot(shadow_root, child) else {
            return;
        };
        let slot_slot = self.live_slot(slot);
        let child_slot = self.live_slot(child);
        self.live_node_mut(slot)
            .links_mut()
            .assigned_nodes
            .push(child_slot);
        self.live_node_mut(child).links_mut().assigned_slot = Some(slot_slot);
        self.mark_ancestors_dirty_descendants(child);
    }

    fn unassign_slottable(&mut self, child: NodeId) {
        let Some(slot) = self.get(child).and_then(Node::assigned_slot_id) else {
            return;
        };
        self.mark_ancestors_dirty_descendants(child);
        let child_slot = self.live_slot(child);
        if let Some(links) = self.live_node_mut(slot).shadow.as_deref_mut()
            && let Some(index) = links
                .assigned_nodes
                .iter()
                .position(|&assigned| assigned == child_slot)
        {
            links.assigned_nodes.remove(index);
        }
        self.live_node_mut(child).clear_assigned_slot();
    }

    fn matching_slot(&self, shadow_root: NodeId, child: NodeId) -> Option<NodeId> {
        let wanted = self.get(child)?.attr_local_name(&SLOT).unwrap_or_default();
        let slots = self.get(shadow_root)?.shadow_data()?.slots.as_ref()?;
        slots.iter().copied().find(|&slot| {
            self.get(slot)
                .is_some_and(|slot| slot.attr_local_name(&NAME).unwrap_or_default() == wanted)
        })
    }

    fn collect_slots(&self, shadow_root: NodeId) -> Vec<NodeId> {
        let mut slots = Vec::new();
        let mut stack: Vec<NodeSlot> = self
            .live(shadow_root)
            .child_slots()
            .iter()
            .rev()
            .copied()
            .collect();
        while let Some(slot) = stack.pop() {
            let node = self.arenas().at(slot);
            if node.is_slot() {
                slots.push(node.id());
            }
            stack.extend(node.child_slots().iter().rev().copied());
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

    pub(crate) fn assign_slots(&mut self, shadow_root: NodeId) {
        let Some(host) = self.get(shadow_root).and_then(Node::shadow_host_id) else {
            return;
        };

        self.ensure_slot_cache(shadow_root);
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

        let light: Vec<NodeId> = self.live(host).child_ids().collect();
        let mut assignments: Vec<(NodeId, Option<NodeId>)> = Vec::with_capacity(light.len());
        for child in light {
            let node = self.live(child);
            if !(node.is_element() || node.is_text_node()) {
                continue;
            }
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
            let slot = slot.map(|id| self.live_slot(id));
            let node = self.live_node_mut(child);
            if node.assigned_slot_slot() != slot {
                node.links_mut().assigned_slot = slot;
                changed = true;
            }
        }
        for (slot, _, nodes) in slots {
            let nodes: Vec<NodeSlot> = nodes.iter().map(|&id| self.live_slot(id)).collect();
            let node = self.live_node_mut(slot);
            if node.assigned_node_slots() != nodes.as_slice() {
                node.links_mut().assigned_nodes = nodes;
                changed = true;
            }
        }

        if changed {
            self.mark_subtree_dirty(host);
            self.invalidate_layout(host);
        }
    }
}
