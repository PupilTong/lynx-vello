//! Coarse style-invalidation helpers on the [`Arena`].
//!
//! These set the `style_dirty` / `dirty_descendants` bits the `lynx-style`
//! crate reads to decide what to restyle. Invalidation here is deliberately
//! coarse — an attribute change re-dirties the whole affected subtree and every
//! following sibling's subtree — which conservatively covers descendant and
//! `+` / `~` combinator selectors without tracking exactly which rules could
//! match.

use crate::arena::{Arena, ElementId};

impl Arena {
    /// Mark `id` as needing its own style recomputed, and flag its ancestors as
    /// having a dirty descendant.
    ///
    /// The ancestor walk stops early once it reaches an ancestor already marked
    /// `dirty_descendants`.
    pub fn mark_style_dirty(&mut self, id: ElementId) {
        match self.get_mut(id) {
            Some(node) => node.style_dirty = true,
            None => return,
        }
        self.mark_ancestors_dirty_descendants(id);
    }

    /// Mark the entire subtree rooted at `id` as needing style recomputed, and
    /// flag `id`'s ancestors as having a dirty descendant.
    pub fn mark_subtree_dirty(&mut self, id: ElementId) {
        if !self.contains(id) {
            return;
        }
        self.mark_ancestors_dirty_descendants(id);
        self.mark_subtree_style_dirty(id);
    }

    /// Invalidation for an attribute / class / state / `css_id` change on `id`.
    ///
    /// Marks `id`'s own subtree dirty (covering descendant selectors) and every
    /// following sibling's subtree dirty (covering `+` / `~` combinators).
    /// Earlier siblings are deliberately left untouched.
    pub fn mark_attribute_changed(&mut self, id: ElementId) {
        self.mark_style_dirty(id);
        self.mark_subtree_dirty(id);

        let Some(node) = self.get(id) else { return };
        let Some(parent) = node.parent else { return };
        let Some(parent_node) = self.get(parent) else {
            return;
        };
        let siblings = parent_node.children.clone();
        let Some(pos) = siblings.iter().position(|&c| c == id) else {
            return;
        };
        for &sibling in &siblings[pos + 1..] {
            self.mark_subtree_dirty(sibling);
        }
    }

    /// Walk from `id`'s parent to the root setting `dirty_descendants`,
    /// stopping at the first ancestor already marked.
    fn mark_ancestors_dirty_descendants(&mut self, id: ElementId) {
        let mut next = match self.get(id) {
            Some(node) => node.parent,
            None => return,
        };
        while let Some(pid) = next {
            match self.get_mut(pid) {
                Some(parent) if !parent.dirty_descendants => {
                    parent.dirty_descendants = true;
                    next = parent.parent;
                }
                _ => break,
            }
        }
    }

    /// Set `style_dirty` on every element in the subtree rooted at `id`, and
    /// `dirty_descendants` on each non-leaf within it.
    fn mark_subtree_style_dirty(&mut self, id: ElementId) {
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let Some(node) = self.get_mut(current) else {
                continue;
            };
            node.style_dirty = true;
            if node.children.is_empty() {
                continue;
            }
            node.dirty_descendants = true;
            stack.extend_from_slice(&node.children);
        }
    }
}
