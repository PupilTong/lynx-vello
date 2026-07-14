//! Style-invalidation scheduling on the [`Arena`].
//!
//! Two cooperating mechanisms decide what the next
//! [`StyleEngine::flush_tree`](crate::StyleEngine::flush_tree) recomputes:
//!
//! 1. **Snapshots** (fine-grained, for attribute / class / id / state changes): before mutating,
//!    the embedder calls the matching `note_*_change` method, which records the element's *old*
//!    matching-relevant state into the arena's
//!    [`SnapshotMap`](stylo::selector_parser::SnapshotMap). During the flush, stylo's
//!    invalidation-set machinery compares old vs. new against the stylist's dependency maps and
//!    restyles only the elements whose rules could actually be affected.
//!
//! 2. **Restyle hints** (for changes with no snapshot representation): structural mutations and
//!    inline-style updates insert [`RestyleHint`] bits directly into the affected elements' stylo
//!    `ElementData`. Structural invalidation is scoped by the selector flags stylo recorded during
//!    matching (`:empty` / position-dependent / edge selectors), so inserting a child into a parent
//!    nothing depends on costs nothing.
//!
//! Both paths also maintain the embedder-visible dirty bits
//! ([`Element::is_style_dirty`](crate::Element::is_style_dirty) /
//! [`Element::has_dirty_descendants`](crate::Element::has_dirty_descendants)):
//! `dirty_descendants` on the ancestor chain is what lets the traversal reach
//! the invalidated element.

use selectors::matching::ElementSelectorFlags;
use stylo::LocalName;
use stylo::attr::{AttrIdentifier, AttrValue};
use stylo::invalidation::element::restyle_hints::RestyleHint;
use stylo::selector_parser::Snapshot;

use crate::arena::{Arena, ElementId};
use crate::ext::ExternalState;

/// Selector-flag bits on a parent that make child-list mutations observable
/// by some matched rule. (`HAS_SLOW_SELECTOR_NTH`/`_NTH_OF` only refine the
/// two slow bits and never appear alone, so they need no entry here.)
const STRUCTURE_SENSITIVE: ElementSelectorFlags = ElementSelectorFlags::HAS_SLOW_SELECTOR
    .union(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS)
    .union(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR)
    .union(ElementSelectorFlags::HAS_EMPTY_SELECTOR)
    .union(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION);

impl<T> Arena<T> {
    /// Mark `id` as needing its own style recomputed, and flag its ancestors as
    /// having a dirty descendant.
    ///
    /// The ancestor walk stops early once it reaches an ancestor already marked
    /// `dirty_descendants`.
    pub fn mark_style_dirty(&mut self, id: ElementId) {
        match self.get(id) {
            Some(element) => element.set_style_dirty(true),
            None => return,
        }
        self.add_restyle_hint(id, RestyleHint::RESTYLE_SELF);
        self.mark_ancestors_dirty_descendants(id);
    }

    /// Mark the entire subtree rooted at `id` as needing style recomputed, and
    /// flag `id`'s ancestors as having a dirty descendant.
    pub fn mark_subtree_dirty(&mut self, id: ElementId) {
        match self.get(id) {
            Some(element) => {
                element.set_style_dirty(true);
                if !element.children.is_empty() {
                    element.set_dirty_descendants_bit(true);
                }
            }
            None => return,
        }
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
        self.mark_ancestors_dirty_descendants(id);
    }

    /// Insert restyle-hint bits into `id`'s stylo `ElementData`, if it has
    /// been styled before. An element without style data needs no hint: the
    /// traversal styles data-less elements unconditionally.
    pub(crate) fn add_restyle_hint(&mut self, id: ElementId, hint: RestyleHint) {
        let Some(element) = self.get_mut(id) else {
            return;
        };
        if let Some(wrapper) = element.stylo_data_mut() {
            wrapper.borrow_mut().hint.insert(hint);
        }
    }

    /// Note that the element's inline `style` block is about to change (or
    /// just changed). Uses stylo's dedicated style-attribute replacement hint,
    /// which swaps one cascade level instead of re-matching selectors.
    pub(crate) fn note_inline_style_change(&mut self, id: ElementId) {
        if let Some(element) = self.get(id) {
            element.set_style_dirty(true);
        } else {
            return;
        }
        self.add_restyle_hint(id, RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
        self.mark_ancestors_dirty_descendants(id);
    }

    /// Walk from `id`'s parent to the root setting `dirty_descendants`,
    /// stopping at the first ancestor already marked.
    pub(crate) fn mark_ancestors_dirty_descendants(&mut self, id: ElementId) {
        let mut next = match self.get(id) {
            Some(element) => element.parent,
            None => return,
        };
        while let Some(pid) = next {
            match self.get(pid) {
                Some(parent) if !parent.has_dirty_descendants() => {
                    parent.set_dirty_descendants_bit(true);
                    next = parent.parent;
                }
                _ => break,
            }
        }
    }

    /// Set the element's own dirty bit and make it reachable from the root.
    fn mark_mutated(&mut self, id: ElementId) {
        if let Some(element) = self.get(id) {
            element.set_style_dirty(true);
        } else {
            return;
        }
        self.mark_ancestors_dirty_descendants(id);
    }

    /// Invalidation for a child-list mutation on `parent` (`index` is the
    /// insertion/removal position). Scope comes from the selector flags stylo
    /// recorded on the parent during matching, so this is near-free unless
    /// some matched rule actually depends on child structure.
    pub(crate) fn note_child_list_change(&mut self, parent: ElementId, _index: usize) {
        let Some(parent_element) = self.get(parent) else {
            return;
        };
        let flags = parent_element.selector_flags();
        if flags.intersects(STRUCTURE_SENSITIVE) {
            // Structural selectors count Element children only. Text nodes
            // still affect `:empty`, but never `:nth-child`, edge-child, or
            // sibling-element relationships.
            let children = parent_element
                .children
                .iter()
                .copied()
                .filter(|&id| self.get(id).is_some())
                .collect::<Vec<_>>();
            if flags.intersects(ElementSelectorFlags::HAS_EMPTY_SELECTOR) {
                // The `:empty` flip can affect the container's own subtree
                // and — through `+`/`~` (`.list:empty + .hint`) — its later
                // siblings' subtrees (Gecko's `RestyleForEmptyChange`).
                self.add_restyle_hint(parent, RestyleHint::restyle_subtree());
                if let Some(element) = self.get(parent) {
                    element.set_style_dirty(true);
                }
                let later_siblings: Vec<ElementId> = self
                    .get(parent)
                    .and_then(|element| {
                        let grandparent = self.get(element.parent?)?;
                        let pos = grandparent.children.iter().position(|&c| c == parent)?;
                        Some(
                            grandparent.children[pos + 1..]
                                .iter()
                                .copied()
                                .filter(|&id| self.get(id).is_some())
                                .collect(),
                        )
                    })
                    .unwrap_or_default();
                for sibling in later_siblings {
                    self.add_restyle_hint(sibling, RestyleHint::restyle_subtree());
                }
            }
            if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR) {
                for &child in &children {
                    self.add_restyle_hint(child, RestyleHint::restyle_subtree());
                }
            } else if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS) {
                // `index` is a raw-node position and Text nodes do not count
                // for structural selectors. Conservatively invalidate every
                // Element child; selector matching remains exact.
                for &child in &children {
                    self.add_restyle_hint(child, RestyleHint::restyle_subtree());
                }
            } else if flags.intersects(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION) {
                for &child in &children {
                    self.add_restyle_hint(child, RestyleHint::RECASCADE_SELF);
                }
            }
            if flags.intersects(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR) {
                // Both edges AND their inward neighbors: an edge insertion
                // displaces the old edge child one slot inward (a prepend
                // makes the old `:first-child` `children[1]`), and it needs
                // its edge styling dropped. Duplicate hints are idempotent.
                let edges = children
                    .iter()
                    .take(2)
                    .chain(children.iter().rev().take(2))
                    .copied()
                    .collect::<Vec<_>>();
                for child in edges {
                    self.add_restyle_hint(child, RestyleHint::restyle_subtree());
                }
            }
        }
        // The traversal must reach the parent's children regardless (a fresh
        // child has no style data and is styled on sight).
        if let Some(element) = self.get(parent)
            && !element.children.is_empty()
        {
            element.set_dirty_descendants_bit(true);
        }
        self.mark_ancestors_dirty_descendants(parent);
    }
}

impl<T: ExternalState> Arena<T> {
    /// Record a pre-mutation snapshot for `id` and note that its `class` list
    /// is changing. Call **before** applying the mutation.
    pub fn note_class_change(&mut self, id: ElementId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.class_changed = true;
        }
        self.mark_mutated(id);
    }

    /// Record a pre-mutation snapshot for `id` and note that its id selector
    /// value is changing. Call **before** applying the mutation.
    pub fn note_id_change(&mut self, id: ElementId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.id_changed = true;
        }
        self.mark_mutated(id);
    }

    /// Record a pre-mutation snapshot for `id` and note that the attribute
    /// `name` is changing (covers real attributes and synthetic / reflected
    /// ones like `data-*`). Call **before** applying the mutation.
    pub fn note_attribute_change(&mut self, id: ElementId, name: &str) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.other_attributes_changed = true;
            let local = LocalName::from(name);
            if !snapshot.changed_attrs.contains(&local) {
                snapshot.changed_attrs.push(local);
            }
        }
        self.mark_mutated(id);
    }

    /// Record a pre-mutation snapshot for `id` before its dynamic
    /// pseudo-class state changes. Call **before** applying the mutation.
    pub fn note_state_change(&mut self, id: ElementId) {
        // `ensure_snapshot` captures the old state on first call; nothing
        // further to flag — state invalidation keys off `snapshot.state`.
        self.ensure_snapshot(id);
        self.mark_mutated(id);
    }

    /// Record a pre-mutation snapshot for `id` before a bulk attribute change
    /// (e.g. a dataset replacement) without naming individual attributes yet.
    /// Callers follow up with [`note_attribute_change`](Self::note_attribute_change)
    /// per affected name — including, after the mutation, names that only
    /// exist in the new state (the snapshot keeps the pre-mutation values
    /// captured here).
    pub fn note_other_attributes_change(&mut self, id: ElementId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.other_attributes_changed = true;
        }
        self.mark_mutated(id);
    }

    /// Capture (once) the element's current matching-relevant state as its
    /// pre-mutation snapshot, returning the entry for the caller to refine.
    ///
    /// Returns `None` when the element is stale or has never been styled — an
    /// unstyled element is styled from scratch anyway, so a snapshot would be
    /// pure overhead.
    fn ensure_snapshot(&mut self, id: ElementId) -> Option<&mut Snapshot> {
        let element = self.get(id)?;
        if !element.has_style_data() {
            return None;
        }
        if !element.snapshot_present() {
            // First snapshot for this element since the last flush: capture
            // the old state.
            let snapshot = build_snapshot(element);
            element.set_snapshot_present();
            let (map, ids) = self.snapshot_map_mut();
            map.insert(id.opaque(), snapshot);
            ids.push(id);
        }
        let (map, _) = self.snapshot_map_mut();
        map.get_mut(&id.opaque())
    }
}

/// Build a stylo element snapshot of the element's *current* (soon to be old)
/// state: dynamic pseudo-class bits plus every matching-relevant attribute —
/// the id selector value, classes, real attributes, and the embedder's
/// synthetic / reflected attributes.
fn build_snapshot<T: ExternalState>(element: &crate::element::Element<T>) -> Snapshot {
    let mut attrs: Vec<(AttrIdentifier, AttrValue)> = Vec::new();

    // "id" and "class" go FIRST, and with the exact `AttrValue` variants the
    // snapshot accessors demand (`id_attr` calls `as_atom`, `has_class`/
    // `each_class` call `as_tokens`). Being first also means the snapshot's
    // first-match `get_attr` finds these canonical entries even if the
    // element carries plain attributes with the same names.
    if let Some(id_atom) = &element.id_attr {
        attrs.push((attr_identifier("id"), AttrValue::Atom(id_atom.clone())));
    }
    if !element.classes.is_empty() {
        attrs.push((
            attr_identifier("class"),
            AttrValue::TokenList(
                std::sync::OnceLock::new(),
                element.classes.iter().cloned().collect(),
            ),
        ));
    }
    for (name, value) in &element.attrs {
        attrs.push((attr_identifier(name), AttrValue::String(value.clone())));
    }
    element.ext.each_extra_attr_name(&mut |name| {
        let name_str: &str = name.0.as_ref();
        if let Some(value) = element.ext.extra_attr_value(name_str) {
            attrs.push((attr_identifier(name_str), AttrValue::String(value)));
        }
    });

    let mut snapshot = Snapshot::new();
    snapshot.state = Some(element.element_state);
    snapshot.attrs = Some(attrs);
    snapshot
}

fn attr_identifier(name: &str) -> AttrIdentifier {
    let local = LocalName::from(name);
    AttrIdentifier {
        local_name: local.clone(),
        name: local,
        namespace: stylo::Namespace::default(),
        prefix: None,
    }
}
