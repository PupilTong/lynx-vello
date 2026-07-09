//! Low-level tree-mutation primitives on the [`Arena`], with their style
//! invalidation baked in.
//!
//! These live here (rather than in the embedder's API layer) because their
//! invalidation is style-system logic: a structural change re-dirties the
//! affected parent's subtree and its following-sibling subtrees, exactly like
//! an attribute change (see [`crate::dirty`]). The embedder's API layer
//! validates its own semantics (stale ids, cycles, reference resolution) and
//! produces its own errors, then delegates the actual unlink/link/free to
//! these primitives.
//!
//! # Invalidation contract
//!
//! - [`Arena::detach`] applies [`mark_attribute_changed`](Arena::mark_attribute_changed) to the
//!   *old* parent (a removal can flip the parent's `:empty` / `:nth-*` matching, observable through
//!   `+` / `~`).
//! - [`Arena::attach_at`] applies it to the *new* parent, for the same reason.
//!
//! Cycle detection is deliberately **not** here: it is the embedding layer's
//! job because it produces that layer's errors. The read helpers
//! ([`Arena::is_ancestor`] etc.) the embedder needs to detect cycles / resolve
//! references live here so both layers share one implementation.

use crate::arena::{Arena, ElementId};

impl<T> Arena<T> {
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
    #[must_use]
    pub fn is_ancestor(&self, ancestor: ElementId, descendant: ElementId) -> bool {
        let mut next = self.get(descendant).and_then(|element| element.parent);
        while let Some(current) = next {
            if current == ancestor {
                return true;
            }
            next = self.get(current).and_then(|element| element.parent);
        }
        false
    }

    /// Detach `child` from its current parent, if any, applying the structural
    /// invalidation at the old location: the parent's subtree plus the
    /// parent's following-sibling subtrees (a removal can flip the parent's
    /// `:empty`/`:nth-*` matching, observable through `+`/`~`).
    ///
    /// A no-op on an already-parentless (or stale) `child`.
    pub fn detach(&mut self, child: ElementId) {
        let old_parent = match self.get(child) {
            Some(element) => element.parent,
            None => return,
        };
        if let Some(parent) = old_parent {
            if let Some(parent_element) = self.get_mut(parent) {
                parent_element.children.retain(|&c| c != child);
            }
            self.mark_attribute_changed(parent);
        }
        if let Some(child_element) = self.get_mut(child) {
            child_element.parent = None;
        }
    }

    /// Link `child` into `parent` at `index`, applying the structural
    /// invalidation at the new location.
    ///
    /// Assumes `child` is already detached and that the caller has validated
    /// the link (no cycle, live handles); the index is clamped by
    /// [`Vec::insert`]'s contract, so callers pass a position in `0..=len`.
    ///
    /// Coarse invalidation: like an attribute change, this re-dirties the new
    /// parent's whole subtree and its following-sibling subtrees, because the
    /// mutation can flip the parent's own matching (`:empty`,
    /// `:nth-child(..) of` the parent) observed through `+`/`~`.
    pub fn attach_at(&mut self, parent: ElementId, child: ElementId, index: usize) {
        if let Some(parent_element) = self.get_mut(parent) {
            parent_element.children.insert(index, child);
        }
        if let Some(child_element) = self.get_mut(child) {
            child_element.parent = Some(parent);
        }
        self.mark_attribute_changed(parent);
    }

    /// Remove `root` and all its descendants from the arena, returning the
    /// external-state payload of every element freed (in no particular order).
    ///
    /// The caller (the embedder's API layer) harvests whatever it indexed from
    /// the returned payloads. All handles into the subtree become stale. This
    /// does **not** unlink `root` from a parent first — callers that need that
    /// call [`Arena::detach`] beforehand.
    pub fn drop_subtree(&mut self, root: ElementId) -> Vec<T> {
        let mut removed = Vec::new();
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            if let Some(element) = self.remove(current) {
                stack.extend_from_slice(&element.children);
                removed.push(element.ext);
            }
        }
        removed
    }
}
