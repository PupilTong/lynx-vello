//! Low-level tree-mutation primitives on the [`Document`](crate::Document), with their style
//! invalidation baked in.
//!
//! These live here (rather than in the embedder's API layer) because their
//! invalidation is style-system logic: a structural change re-dirties the
//! affected parent's subtree and its following-sibling subtrees, exactly like
//! an attribute change (see [`crate::dirty`]). The embedder's API layer
//! validates its own semantics (stale ids, cycles, reference resolution) and
//! produces its own errors, then delegates the actual unlink/link to these
//! primitives. Physical reclamation is available only through
//! [`Document::collect_detached`], which cannot remove connected nodes and
//! requires the embedder to prove that every node in a detached subtree is
//! externally unretained.
//!
//! # Invalidation contract
//!
//! - [`Document::detach`](crate::Document::detach) applies the child-list invalidation
//!   ([`note_child_list_change`](crate::Document::note_child_list_change)) to the *old* parent —
//!   scoped by the parent's stylo selector flags, so a removal only restyles what `:empty` /
//!   `:nth-*` / edge-child rules can actually observe.
//! - [`Document::attach_at`](crate::Document::attach_at) applies it to the *new* parent, and
//!   additionally schedules a subtree restyle on a previously-styled `child` (its matching context
//!   — ancestors, siblings — changed with the move).
//!
//! Cycle detection is deliberately **not** here: it is the embedding layer's
//! job because it produces that layer's errors. The read helpers
//! ([`Document::is_ancestor`](crate::Document::is_ancestor) etc.) the embedder needs to detect
//! cycles / resolve references live here so both layers share one implementation.

use rustc_hash::FxHashSet;
use stylo::invalidation::element::restyle_hints::RestyleHint;

use crate::arena::{Document, ElementId};

impl<T> Document<T> {
    /// The position of `child` within `parent`'s child list, if it is a child.
    #[must_use]
    pub fn child_position(&self, parent: ElementId, child: ElementId) -> Option<usize> {
        self.get(parent)?.children.iter().position(|&c| c == child)
    }

    /// The number of children of `parent` (0 if the handle is stale).
    #[must_use]
    pub fn children_len(&self, parent: ElementId) -> usize {
        self.get(parent).map_or(0, |element| element.children.len())
    }

    /// Whether `child` is a direct child of `parent`.
    #[must_use]
    pub fn is_child_of(&self, child: ElementId, parent: ElementId) -> bool {
        self.child_position(parent, child).is_some()
    }

    /// Whether `ancestor` is a strict ancestor of `descendant`.
    ///
    /// # Panics
    ///
    /// In debug/test builds, panics if the existing topology contains a
    /// parent cycle. Validated embedding APIs must prevent such cycles.
    #[must_use]
    pub fn is_ancestor(&self, ancestor: ElementId, descendant: ElementId) -> bool {
        let mut next = self.get(descendant).and_then(|element| element.parent);
        #[cfg(debug_assertions)]
        let mut visited = FxHashSet::default();
        while let Some(current) = next {
            #[cfg(debug_assertions)]
            assert!(
                visited.insert(current),
                "tree topology contains a parent cycle"
            );
            if current == ancestor {
                return true;
            }
            next = self.get(current).and_then(|element| element.parent);
        }
        false
    }

    /// Detach `child` from its current parent, if any, applying the
    /// selector-flag-scoped child-list invalidation at the old location (a
    /// removal can flip `:empty` / `:nth-*` / edge-child matching).
    ///
    /// A no-op on an already-parentless (or stale) `child`.
    pub fn detach(&mut self, child: ElementId) {
        let old_parent = match self.get(child) {
            Some(element) => element.parent,
            None => return,
        };
        if let Some(parent) = old_parent {
            let mut removed_index = 0;
            if let Some(parent_element) = self.node_mut(parent)
                && let Some(pos) = parent_element.children.iter().position(|&c| c == child)
            {
                removed_index = pos;
                parent_element.children.remove(pos);
            }
            self.note_child_list_change(parent, removed_index);
        }
        if let Some(child_element) = self.node_mut(child) {
            child_element.parent = None;
        }
    }

    /// Link `child` into `parent` at `index`, applying the
    /// selector-flag-scoped child-list invalidation at the new location.
    ///
    /// Assumes `child` is already detached and that the caller has validated
    /// the link (no cycle, live handles); the index is clamped by
    /// [`Vec::insert`]'s contract, so callers pass a position in `0..=len`.
    ///
    /// A `child` that has been styled before is scheduled for a subtree
    /// restyle: its matching context (ancestors, siblings) changed with the
    /// move. A fresh child needs nothing — elements without style data are
    /// styled unconditionally when the traversal reaches them.
    pub fn attach_at(&mut self, parent: ElementId, child: ElementId, index: usize) {
        if let Some(parent_element) = self.node_mut(parent) {
            parent_element.children.insert(index, child);
        }
        if let Some(child_element) = self.node_mut(child) {
            child_element.parent = Some(parent);
        }
        self.add_restyle_hint(child, RestyleHint::restyle_subtree());
        self.note_child_list_change(parent, index);
    }

    /// Reclaim detached subtrees whose nodes have no external strong handles.
    ///
    /// `retained_roots` are parentless nodes that still belong to the live DOM
    /// tree (normally the document/page root). Every other parentless node is
    /// a detached-subtree candidate. A candidate is reclaimed only when
    /// `is_externally_retained` returns `false` for every node in that subtree.
    ///
    /// This is the only public physical-reclamation path. It cannot destroy a
    /// connected node, and it never partially destroys a retained subtree.
    /// Embedders should derive `is_externally_retained` from their strong
    /// handle registry rather than from VM-specific guesses.
    ///
    /// # Panics
    ///
    /// Panics when the document's supposedly live topology contains an
    /// unresolved id or changes between validation and reclamation. Under the
    /// owner-thread mutation model, either condition is an invariant failure.
    pub fn collect_detached(
        &mut self,
        retained_roots: &[ElementId],
        mut is_externally_retained: impl FnMut(ElementId) -> bool,
    ) -> Vec<(ElementId, T)> {
        let roots: Vec<_> = self
            .live_ids()
            .into_iter()
            .filter(|&id| {
                self.get(id).is_some_and(|node| node.parent.is_none())
                    && !retained_roots.contains(&id)
            })
            .collect();
        let mut removed = Vec::new();

        for root in roots {
            if !self.contains(root) {
                continue;
            }

            let mut subtree = Vec::new();
            let mut stack = vec![root];
            let mut retained = false;
            while let Some(current) = stack.pop() {
                let Some(node) = self.get(current) else {
                    debug_assert!(false, "live topology contained a stale ElementId");
                    retained = true;
                    break;
                };
                if is_externally_retained(current) {
                    retained = true;
                    break;
                }
                subtree.push(current);
                stack.extend_from_slice(&node.children);
            }

            if retained {
                continue;
            }
            for id in subtree {
                let node = self
                    .remove_node(id)
                    .expect("collector validated every node before reclamation");
                removed.push((id, node.ext));
            }
        }

        removed
    }

    /// Assert the bidirectional parent/child topology invariants.
    ///
    /// This is cheap enough for tests and debug mutation checks, but is not
    /// called by the generic document automatically in release builds.
    ///
    /// # Panics
    ///
    /// Panics on a stale parent/child id, a missing back-pointer, a duplicate
    /// child entry, a parent cycle, or any other mismatch in the bidirectional
    /// topology.
    pub fn assert_tree_integrity(&self) {
        let live_ids = self.live_ids();
        for &id in &live_ids {
            let node = self.get(id).expect("live id must resolve");
            if let Some(parent) = node.parent {
                let parent_node = self
                    .get(parent)
                    .expect("a live node's parent id must resolve");
                assert_eq!(
                    parent_node
                        .children
                        .iter()
                        .filter(|&&child| child == id)
                        .count(),
                    1,
                    "a live node must occur exactly once in its parent's child list"
                );
            }
            for &child in &node.children {
                let child_node = self
                    .get(child)
                    .expect("a live node's child id must resolve");
                assert_eq!(
                    child_node.parent,
                    Some(id),
                    "child back-pointer must name the containing parent"
                );
            }
        }

        for id in live_ids {
            let mut visited = FxHashSet::default();
            let mut current = Some(id);
            while let Some(ancestor) = current {
                assert!(
                    visited.insert(ancestor),
                    "tree topology contains a parent cycle"
                );
                current = self
                    .get(ancestor)
                    .expect("a live ancestor id must resolve")
                    .parent;
            }
        }
    }
}
