//! Matching-relevant mutation, with its style invalidation baked in.
//!
//! Every setter here mutates one node **and** schedules exactly the style
//! work that mutation can cause. The pairing is structural — there is no
//! public "mutate without invalidating" path — which is what makes the
//! "snapshot before mutating" rule an implementation detail instead of an
//! embedder obligation. Two cooperating mechanisms decide what the next
//! [`StyleEngine::flush_document`](crate::StyleEngine::flush_document)
//! recomputes:
//!
//! 1. **Snapshots** (fine-grained, for attribute / class / id / state changes): before mutating,
//!    the setter records the node's *old* matching-relevant state in that node. At flush time the
//!    pending per-node snapshots are moved into stylo's temporary
//!    [`SnapshotMap`](stylo::selector_parser::SnapshotMap); its invalidation-set machinery compares
//!    old vs. new against the stylist's dependency maps and restyles only the nodes whose rules
//!    could actually be affected.
//!
//! 2. **Restyle hints** (for changes with no snapshot representation): structural mutations, text
//!    changes, and inline-style updates insert [`RestyleHint`] bits directly into the affected
//!    nodes' stylo `ElementData`. Structural invalidation is scoped by the selector flags stylo
//!    recorded during matching (`:empty` / position-dependent / edge selectors), so inserting a
//!    child into a parent nothing depends on costs nothing.
//!
//! Both paths also maintain the embedder-visible dirty bits
//! ([`Node::is_style_dirty`](crate::Node::is_style_dirty) /
//! [`Node::has_dirty_descendants`](crate::Node::has_dirty_descendants)):
//! `dirty_descendants` on the ancestor chain is what lets the traversal reach
//! the invalidated node.
//!
//! The one seam an embedder must handle itself: synthetic / reflected
//! attributes served by its [`ExternalState`](crate::ExternalState) hooks.
//! Their values live in the payload, so the document cannot see them change —
//! [`Document::note_external_attribute_change`] is the contractual companion
//! to [`Document::ext_mut`](crate::Document::ext_mut).

use selectors::matching::ElementSelectorFlags;
use stylo::LocalName;
use stylo::attr::{AttrIdentifier, AttrValue};
use stylo::context::QuirksMode;
use stylo::invalidation::element::restyle_hints::RestyleHint;
use stylo::properties::declaration_block::{parse_one_declaration_into, parse_style_attribute};
use stylo::properties::{
    Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration,
};
use stylo::selector_parser::Snapshot;
use stylo::servo_arc::Arc;
use stylo::stylesheets::{CssRuleType, Origin};
use stylo_atoms::Atom;
use stylo_traits::ParsingMode;

use crate::document::{Document, NodeId};
use crate::ext::ExternalState;
use crate::node::Node;

/// Selector-flag bits on a parent that make child-list mutations observable
/// by some matched rule. (`HAS_SLOW_SELECTOR_NTH`/`_NTH_OF` only refine the
/// two slow bits and never appear alone, so they need no entry here.)
const STRUCTURE_SENSITIVE: ElementSelectorFlags = ElementSelectorFlags::HAS_SLOW_SELECTOR
    .union(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS)
    .union(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR)
    .union(ElementSelectorFlags::HAS_EMPTY_SELECTOR)
    .union(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION);

impl<T> Document<T> {
    // --- scheduling primitives ---------------------------------------------

    /// Mark `id` as needing its own style recomputed, and flag its ancestors
    /// as having a dirty descendant.
    ///
    /// The ancestor walk stops early once it reaches an ancestor already
    /// marked `dirty_descendants`.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale or identifies a text node (the let-it-crash
    /// mutation contract; see the crate docs).
    pub fn mark_style_dirty(&mut self, id: NodeId) {
        self.live_element(id);
        self.live(id).set_style_dirty(true);
        self.add_restyle_hint(id, RestyleHint::RESTYLE_SELF);
        self.mark_ancestors_dirty_descendants(id);
    }

    /// Mark the entire subtree rooted at `id` as needing style recomputed,
    /// and flag `id`'s ancestors as having a dirty descendant.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale or identifies a text node (the let-it-crash
    /// mutation contract; see the crate docs).
    pub fn mark_subtree_dirty(&mut self, id: NodeId) {
        let node = self.live_element(id);
        node.set_style_dirty(true);
        if !node.child_ids().is_empty() {
            node.set_dirty_descendants_bit(true);
        }
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
        self.mark_ancestors_dirty_descendants(id);
    }

    /// Resolve a live node for a mutation path, with the let-it-crash
    /// contract applied.
    fn live(&self, id: NodeId) -> &Node<T> {
        self.get(id)
            .expect("stale NodeId passed to a Document mutation method")
    }

    /// Resolve a live element for an element-only mutation path.
    fn live_element(&self, id: NodeId) -> &Node<T> {
        let node = self.live(id);
        assert!(
            node.is_element(),
            "element-only Document mutation called with a text node"
        );
        node
    }

    /// Insert restyle-hint bits into `id`'s stylo `ElementData`, if it has
    /// been styled before. A node without style data needs no hint: the
    /// traversal styles data-less nodes unconditionally.
    pub(crate) fn add_restyle_hint(&mut self, id: NodeId, hint: RestyleHint) {
        if let Some(wrapper) = self.core_mut().node_mut(id).and_then(Node::stylo_data_mut) {
            wrapper.borrow_mut().hint.insert(hint);
        }
    }

    /// Walk from `id`'s parent to the root setting `dirty_descendants`,
    /// stopping at the first ancestor already marked.
    pub(crate) fn mark_ancestors_dirty_descendants(&mut self, id: NodeId) {
        let core = self.core_mut();
        let mut next = core.node(id).and_then(Node::parent_id);
        while let Some(pid) = next {
            let parent = core.link(pid);
            if parent.has_dirty_descendants() {
                break;
            }
            parent.set_dirty_descendants_bit(true);
            next = parent.parent_id();
        }
    }

    /// Set the node's own dirty bit and make it reachable from the root.
    fn mark_mutated(&mut self, id: NodeId) {
        self.live(id).set_style_dirty(true);
        self.mark_ancestors_dirty_descendants(id);
    }

    /// A previously-styled node moved: its matching context (ancestors,
    /// siblings) changed, so its whole subtree restyles. A fresh node needs
    /// nothing — nodes without style data are styled unconditionally when
    /// the traversal reaches them.
    pub(crate) fn note_moved_subtree(&mut self, id: NodeId) {
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
    }

    /// Invalidation for a child-list mutation on `parent` (`index` is the
    /// insertion/removal position). Scope comes from the selector flags stylo
    /// recorded on the parent during matching, so this is near-free unless
    /// some matched rule actually depends on child structure.
    pub(crate) fn note_child_list_change(&mut self, parent: NodeId, index: usize) {
        let parent_node = self.live_element(parent);
        let flags = parent_node.selector_flags();
        if flags.intersects(STRUCTURE_SENSITIVE) {
            let children = parent_node.child_ids().to_vec();
            let element_children: Vec<NodeId> = children
                .iter()
                .copied()
                .filter(|&child| self.live(child).is_element())
                .collect();
            if flags.intersects(ElementSelectorFlags::HAS_EMPTY_SELECTOR) {
                self.note_emptiness_change(parent);
            }
            if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR) {
                for &child in &element_children {
                    self.add_restyle_hint(child, RestyleHint::restyle_subtree());
                }
            } else if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS) {
                for &child in children.get(index..).unwrap_or_default() {
                    if self.live(child).is_element() {
                        self.add_restyle_hint(child, RestyleHint::restyle_subtree());
                    }
                }
            } else if flags.intersects(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION) {
                for &child in &element_children {
                    self.add_restyle_hint(child, RestyleHint::RECASCADE_SELF);
                }
            }
            if flags.intersects(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR) {
                // Both edges AND their inward neighbors: an edge insertion
                // displaces the old edge child one slot inward (a prepend
                // makes the old `:first-child` `element_children[1]`), and it
                // needs its edge styling dropped. Duplicate hints are
                // idempotent.
                let edges: Vec<NodeId> = element_children
                    .iter()
                    .take(2)
                    .chain(element_children.iter().rev().take(2))
                    .copied()
                    .collect();
                for child in edges {
                    self.add_restyle_hint(child, RestyleHint::restyle_subtree());
                }
            }
        }
        // The traversal must reach the parent's children regardless (a fresh
        // child has no style data and is styled on sight).
        {
            let node = self.live(parent);
            if !node.child_ids().is_empty() {
                node.set_dirty_descendants_bit(true);
            }
        }
        self.mark_ancestors_dirty_descendants(parent);
    }

    /// `id`'s `:empty` matching may have flipped. The flip can affect the
    /// node's own subtree and — through `+`/`~` (`.list:empty + .hint`) — its
    /// later siblings' subtrees (Gecko's `RestyleForEmptyChange`).
    fn note_emptiness_change(&mut self, id: NodeId) {
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
        self.live(id).set_style_dirty(true);
        let later_siblings: Vec<NodeId> = {
            let core = self.core();
            core.node(id)
                .and_then(|node| {
                    let siblings = core.link(node.parent_id()?).child_ids();
                    let pos = siblings.iter().position(|&c| c == id)?;
                    Some(siblings[pos + 1..].to_vec())
                })
                .unwrap_or_default()
        };
        for sibling in later_siblings {
            self.add_restyle_hint(sibling, RestyleHint::restyle_subtree());
        }
    }
}

impl<T: ExternalState> Document<T> {
    // --- matching-relevant setters -------------------------------------------

    /// Replace the node's class list from a whitespace-separated string
    /// (`className` semantics).
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn set_classes(&mut self, id: NodeId, classes: &str) {
        self.live_element(id);
        self.note_class_change(id);
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::set_classes")
            .classes = classes.split_whitespace().map(Atom::from).collect();
    }

    /// Add one class (a no-op when already present, costing no snapshot).
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn add_class(&mut self, id: NodeId, class: &str) {
        let class = Atom::from(class);
        if self.live_element(id).classes.contains(&class) {
            return;
        }
        self.note_class_change(id);
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::add_class")
            .classes
            .push(class);
    }

    /// Remove one class (a no-op when absent, costing no snapshot).
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn remove_class(&mut self, id: NodeId, class: &str) {
        let class = Atom::from(class);
        if !self.live_element(id).classes.contains(&class) {
            return;
        }
        self.note_class_change(id);
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::remove_class")
            .classes
            .retain(|existing| *existing != class);
    }

    /// Set or clear the node's `id` selector value.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn set_id_attr(&mut self, id: NodeId, value: Option<&str>) {
        self.live_element(id);
        self.note_id_change(id);
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::set_id_attr")
            .id_attr = value.map(Atom::from);
    }

    /// Set a plain attribute.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn set_attribute(&mut self, id: NodeId, name: &str, value: &str) {
        self.live_element(id);
        self.note_attribute_change(id, name);
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::set_attribute")
            .attrs
            .insert(name.into(), value.to_owned());
    }

    /// Remove a plain attribute (a no-op when absent).
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn remove_attribute(&mut self, id: NodeId, name: &str) {
        self.live_element(id);
        self.note_attribute_change(id, name);
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::remove_attribute")
            .attrs
            .remove(name);
    }

    /// Set or clear dynamic pseudo-class state bits (`:hover` / `:active` /
    /// `:focus`, as re-exported [`ElementState`](crate::ElementState) flags).
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn set_state(&mut self, id: NodeId, flags: dom::ElementState, on: bool) {
        self.live_element(id);
        // `ensure_snapshot` captures the old state on first call; state
        // invalidation keys off `snapshot.state`, so nothing further to flag.
        self.ensure_snapshot(id);
        self.mark_mutated(id);
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::set_state")
            .element_state
            .set(flags, on);
    }

    /// Set or clear a node's literal character data.
    ///
    /// On a text node, `None` is normalized to the empty string so the node
    /// remains a text node with valid character data. Element-backed text
    /// carriers may use `None` to clear their data. Character data
    /// participates in matching only through the containing element's
    /// `:empty` state; invalidation is scoped accordingly.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs).
    pub fn set_text(&mut self, id: NodeId, text: Option<String>) {
        let node = self.live(id);
        let is_text_node = node.is_text_node();
        let affected_element = if is_text_node {
            node.parent_id()
        } else {
            Some(id)
        };
        let (was_empty, watches_empty) = affected_element.map_or((false, false), |element| {
            let element = self.live_element(element);
            (
                element.is_empty_element(),
                element
                    .selector_flags()
                    .intersects(ElementSelectorFlags::HAS_EMPTY_SELECTOR),
            )
        });
        let text = if is_text_node {
            Some(text.unwrap_or_default())
        } else {
            text
        };
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::set_text")
            .text = text;
        if let Some(element) = affected_element
            && watches_empty
            && was_empty != self.live_element(element).is_empty_element()
        {
            self.note_emptiness_change(element);
            self.mark_ancestors_dirty_descendants(element);
        }
    }

    /// Replace a text node's character data.
    ///
    /// This is the kind-checked convenience form of [`set_text`](Self::set_text)
    /// for W3C text nodes. Element-backed text carriers continue to use
    /// `set_text` directly.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale or names an element node.
    pub fn set_text_data(&mut self, id: NodeId, text: impl Into<String>) {
        assert!(
            self.live(id).is_text_node(),
            "Document::set_text_data called with an element node"
        );
        self.set_text(id, Some(text.into()));
    }

    // --- inline style ----------------------------------------------------------

    /// Replace the node's inline style, parsing the whole declaration block
    /// through stylo (the `style` attribute). An empty string clears it.
    ///
    /// Uses stylo's dedicated style-attribute replacement hint, which swaps
    /// one cascade level instead of re-matching selectors.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn set_inline_style(&mut self, id: NodeId, css: &str) {
        self.live_element(id);
        let block = if css.is_empty() {
            None
        } else {
            let core = self.core();
            let parsed = parse_style_attribute(
                css,
                &core.url_data,
                None,
                QuirksMode::NoQuirks,
                CssRuleType::Style,
            );
            Some(Arc::new(core.lock.wrap(parsed)))
        };
        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::set_inline_style")
            .inline_block = block;
        self.note_inline_style_change(id);
    }

    /// Parse a single `name: value` declaration through stylo and merge it
    /// into the node's inline style block.
    ///
    /// Only the one new declaration is parsed and folded into a clone of the
    /// existing block, avoiding a whole-attribute re-parse. An unparseable
    /// property or value is dropped **by design** — that is CSS error
    /// handling (invalid declarations are ignored), not an unexpected
    /// parameter.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn add_inline_style(&mut self, id: NodeId, name: &str, value: &str) {
        self.live_element(id);
        let Ok(property_id) = PropertyId::parse_unchecked(name, None) else {
            return;
        };

        let core = self.core();
        let mut source = SourcePropertyDeclaration::default();
        if parse_one_declaration_into(
            &mut source,
            property_id,
            value,
            Origin::Author,
            &core.url_data,
            None,
            ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            CssRuleType::Style,
        )
        .is_err()
        {
            return;
        }

        let mut block = match &self.live(id).inline_block {
            Some(existing) => {
                let guard = core.lock.read();
                existing.read_with(&guard).clone()
            }
            None => PropertyDeclarationBlock::new(),
        };
        block.extend(source.drain(), Importance::Normal);
        let wrapped = Arc::new(core.lock.wrap(block));

        self.core_mut()
            .node_mut(id)
            .expect("stale NodeId passed to Document::add_inline_style")
            .inline_block = Some(wrapped);
        self.note_inline_style_change(id);
    }

    /// Count parsed declarations in the node's inline style (0 when none is
    /// set). Keeps the style lock encapsulated while still allowing
    /// diagnostics and tests to inspect parsed state.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale or identifies a text node (the let-it-crash
    /// mutation contract; see the crate docs).
    #[must_use]
    pub fn inline_style_declaration_count(&self, id: NodeId) -> usize {
        self.live_element(id);
        let core = self.core();
        let Some(block) = &self.live(id).inline_block else {
            return 0;
        };
        let guard = core.lock.read();
        block.read_with(&guard).declarations().len()
    }

    fn note_inline_style_change(&mut self, id: NodeId) {
        self.live(id).set_style_dirty(true);
        self.add_restyle_hint(id, RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
        self.mark_ancestors_dirty_descendants(id);
    }

    // --- external (synthetic / reflected) attributes ---------------------------

    /// Record that the synthetic / reflected attribute `name` — served by the
    /// payload's [`ExternalState`](crate::ExternalState) hooks — is changing.
    ///
    /// Call **before** the [`ext_mut`](crate::Document::ext_mut) mutation for
    /// names that existed before it, so the snapshot captures the old values;
    /// names that only exist *after* the mutation are also noted through this
    /// method (the snapshot keeps whatever state its first capture saw).
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale or identifies a text node (the let-it-crash
    /// mutation contract; see the crate docs).
    pub fn note_external_attribute_change(&mut self, id: NodeId, name: &str) {
        self.live_element(id);
        self.note_attribute_change(id, name);
    }

    /// Record a bulk synthetic / reflected attribute change (e.g. a dataset
    /// replacement) before naming individual attributes. Callers follow up
    /// with [`note_external_attribute_change`](Self::note_external_attribute_change)
    /// per affected name.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale or identifies a text node (the let-it-crash
    /// mutation contract; see the crate docs).
    pub fn note_external_attributes_change(&mut self, id: NodeId) {
        self.live_element(id);
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.other_attributes_changed = true;
        }
        self.mark_mutated(id);
    }

    // --- snapshot recording ------------------------------------------------------

    fn note_class_change(&mut self, id: NodeId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.class_changed = true;
        }
        self.mark_mutated(id);
    }

    fn note_id_change(&mut self, id: NodeId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.id_changed = true;
        }
        self.mark_mutated(id);
    }

    fn note_attribute_change(&mut self, id: NodeId, name: &str) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.other_attributes_changed = true;
            let local = LocalName::from(name);
            if !snapshot.changed_attrs.contains(&local) {
                snapshot.changed_attrs.push(local);
            }
        }
        self.mark_mutated(id);
    }

    /// Capture (once) the node's current matching-relevant state as its
    /// pre-mutation snapshot, returning the entry for the caller to refine.
    ///
    /// Returns `None` when the node has never been styled — an unstyled node
    /// is styled from scratch anyway, so a snapshot would be pure overhead.
    fn ensure_snapshot(&mut self, id: NodeId) -> Option<&mut Snapshot> {
        let node = self.live(id);
        if !node.has_style_data() {
            return None;
        }
        if node.snapshot.is_none() {
            // First snapshot for this node since the last flush: capture the
            // old state.
            let snapshot = build_snapshot(node);
            let node = self
                .core_mut()
                .node_mut(id)
                .expect("live node disappeared while recording its snapshot");
            node.snapshot = Some(Box::new(snapshot));
            node.set_snapshot_present();
        }
        self.core_mut()
            .node_mut(id)
            .expect("live node disappeared while refining its snapshot")
            .snapshot
            .as_deref_mut()
    }
}

/// Build a stylo element snapshot of the node's *current* (soon to be old)
/// state: dynamic pseudo-class bits plus every matching-relevant attribute —
/// the id selector value, classes, real attributes, and the embedder's
/// synthetic / reflected attributes.
fn build_snapshot<T: ExternalState>(node: &Node<T>) -> Snapshot {
    let mut attrs: Vec<(AttrIdentifier, AttrValue)> = Vec::new();

    // "id" and "class" go FIRST, and with the exact `AttrValue` variants the
    // snapshot accessors demand (`id_attr` calls `as_atom`, `has_class`/
    // `each_class` call `as_tokens`). Being first also means the snapshot's
    // first-match `get_attr` finds these canonical entries even if the node
    // carries plain attributes with the same names.
    if let Some(id_atom) = &node.id_attr {
        attrs.push((attr_identifier("id"), AttrValue::Atom(id_atom.clone())));
    }
    if !node.classes.is_empty() {
        attrs.push((
            attr_identifier("class"),
            AttrValue::TokenList(
                std::sync::OnceLock::new(),
                node.classes.iter().cloned().collect(),
            ),
        ));
    }
    for (name, value) in &node.attrs {
        attrs.push((attr_identifier(name), AttrValue::String(value.clone())));
    }
    node.ext().each_extra_attr_name(&mut |name| {
        let name_str: &str = name.0.as_ref();
        if let Some(value) = node.ext().extra_attr_value(name_str) {
            attrs.push((attr_identifier(name_str), AttrValue::String(value)));
        }
    });

    let mut snapshot = Snapshot::new();
    snapshot.state = Some(node.element_state());
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
