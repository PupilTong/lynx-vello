//! Low-level tree-mutation primitives on the [`Arena`], with their style
//! invalidation baked in.
//!
//! These live here (rather than in the `lynx-dom` PAPI layer) because their
//! invalidation is style-system logic: a structural change re-dirties the
//! affected parent's subtree and its following-sibling subtrees, exactly like
//! an attribute change (see [`crate::dirty`]). The PAPI layer validates opcode
//! semantics (stale ids, cycles, reference resolution) and produces its own
//! errors, then delegates the actual unlink/link/free to these primitives.
//!
//! # Invalidation contract
//!
//! - [`Arena::detach`] applies [`mark_attribute_changed`](Arena::mark_attribute_changed) to the
//!   *old* parent (a removal can flip the parent's `:empty` / `:nth-*` matching, observable through
//!   `+` / `~`).
//! - [`Arena::attach_at`] applies it to the *new* parent, for the same reason.
//!
//! Cycle detection is deliberately **not** here: it is the PAPI layer's job
//! because it produces a PAPI error. The read helpers ([`Arena::is_ancestor`]
//! etc.) that the PAPI layer needs to detect cycles / resolve references live
//! here so both layers share one implementation.

use crate::arena::{Arena, WidgetId};

impl Arena {
    /// The position of `child` within `parent`'s child list, if it is a child.
    #[must_use]
    pub fn child_position(&self, parent: WidgetId, child: WidgetId) -> Option<usize> {
        self.get(parent)?.children.iter().position(|&c| c == child)
    }

    /// The number of children of `parent` (0 if the handle is stale).
    #[must_use]
    pub fn children_len(&self, parent: WidgetId) -> usize {
        self.get(parent).map_or(0, |widget| widget.children.len())
    }

    /// Whether `child` is a direct child of `parent`.
    #[must_use]
    pub fn is_child_of(&self, child: WidgetId, parent: WidgetId) -> bool {
        self.child_position(parent, child).is_some()
    }

    /// Whether `ancestor` is a strict ancestor of `descendant`.
    #[must_use]
    pub fn is_ancestor(&self, ancestor: WidgetId, descendant: WidgetId) -> bool {
        let mut next = self.get(descendant).and_then(|widget| widget.parent);
        while let Some(current) = next {
            if current == ancestor {
                return true;
            }
            next = self.get(current).and_then(|widget| widget.parent);
        }
        false
    }

    /// Detach `child` from its current parent, if any, applying the structural
    /// invalidation at the old location: the parent's subtree plus the
    /// parent's following-sibling subtrees (a removal can flip the parent's
    /// `:empty`/`:nth-*` matching, observable through `+`/`~`).
    ///
    /// A no-op on an already-parentless (or stale) `child`.
    pub fn detach(&mut self, child: WidgetId) {
        let old_parent = match self.get(child) {
            Some(widget) => widget.parent,
            None => return,
        };
        if let Some(parent) = old_parent {
            if let Some(parent_widget) = self.get_mut(parent) {
                parent_widget.children.retain(|&c| c != child);
            }
            self.mark_attribute_changed(parent);
        }
        if let Some(child_widget) = self.get_mut(child) {
            child_widget.parent = None;
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
    pub fn attach_at(&mut self, parent: WidgetId, child: WidgetId, index: usize) {
        if let Some(parent_widget) = self.get_mut(parent) {
            parent_widget.children.insert(index, child);
        }
        if let Some(child_widget) = self.get_mut(child) {
            child_widget.parent = Some(parent);
        }
        self.mark_attribute_changed(parent);
    }

    /// Remove `root` and all its descendants from the arena, returning the Lynx
    /// `unique_id` of every element freed (in no particular order).
    ///
    /// The caller (the PAPI layer) uses the returned ids to drop the matching
    /// entries from its `unique_id` index. All handles into the subtree become
    /// stale. This does **not** unlink `root` from a parent first — callers
    /// that need that call [`Arena::detach`] beforehand.
    pub fn drop_subtree(&mut self, root: WidgetId) -> Vec<i32> {
        let mut removed = Vec::new();
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            if let Some(widget) = self.remove(current) {
                removed.push(widget.unique_id);
                stack.extend_from_slice(&widget.children);
            }
        }
        removed
    }
}
