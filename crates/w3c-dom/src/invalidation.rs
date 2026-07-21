//! Matching-relevant mutation, with its style invalidation baked in.
//!
//! Every setter here mutates one node **and** schedules exactly the style
//! work that mutation can cause. The pairing is structural — there is no
//! public "mutate without invalidating" path — which is what makes the
//! "snapshot before mutating" rule an implementation detail instead of an
//! embedder obligation. Two cooperating mechanisms decide what the next
//! [`Document::flush_styles`](crate::Document::flush_styles)
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
//! Both paths also maintain `dirty_descendants` on the ancestor chain, which
//! is what lets the traversal reach the invalidated node. Scheduling state is
//! wholly internal: embedders mutate real DOM fields and never set or clear
//! dirty state themselves. Selector-visible state always lives in the DOM
//! fields mutated here; the embedder payload is opaque and cannot inject
//! synthetic matching state.

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

use crate::document::{DOCUMENT_NODE_ID, Document, NodeId};
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

    /// Mark the entire subtree rooted at `id` as needing style recomputed,
    /// and flag `id`'s ancestors as having a dirty descendant.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale or identifies a text node (the let-it-crash
    /// mutation contract; see the crate docs).
    pub(crate) fn mark_subtree_dirty(&mut self, id: NodeId) {
        let node = self.live_element(id);
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
            "element-only Document mutation called with a non-element node"
        );
        node
    }

    /// Insert restyle-hint bits into `id`'s stylo `ElementData`, if it has
    /// been styled before. A node without style data needs no hint: the
    /// traversal styles data-less nodes unconditionally.
    pub(crate) fn add_restyle_hint(&mut self, id: NodeId, hint: RestyleHint) {
        if let Some(wrapper) = self.tree_mut().get_mut(id).and_then(Node::stylo_data_mut) {
            wrapper.borrow_mut().hint.insert(hint);
        }
    }

    /// Walk from `id`'s parent to the root setting `dirty_descendants`,
    /// stopping at the first ancestor already marked.
    pub(crate) fn mark_ancestors_dirty_descendants(&mut self, id: NodeId) {
        let tree = self.tree();
        let mut next = tree.get(id).and_then(Node::parent_id);
        while let Some(pid) = next {
            if pid == DOCUMENT_NODE_ID {
                break;
            }
            let parent = tree.get(pid).expect("internal tree links always resolve");
            if parent.has_dirty_descendants() {
                break;
            }
            parent.set_dirty_descendants_bit(true);
            next = parent.parent_id();
        }
    }

    /// Make the mutation reachable from the root.
    fn mark_mutated(&mut self, id: NodeId) {
        // Validate up front (let-it-crash) — the reachability walk below
        // silently skips stale ids.
        self.live(id);
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
        let later_siblings: Vec<NodeId> = {
            let tree = self.tree();
            tree.get(id)
                .and_then(|node| {
                    let siblings = tree
                        .get(node.parent_id()?)
                        .expect("internal tree links always resolve")
                        .child_ids();
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

impl<T> Document<T> {
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
        self.note_class_attribute_change(id);
        let node = self
            .tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::set_classes");
        node.classes = classes.split_whitespace().map(Atom::from).collect();
        node.attrs
            .insert(LocalName::from("class"), classes.to_owned());
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
        self.note_class_attribute_change(id);
        let node = self
            .tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::add_class");
        node.classes.push(class);
        let class_value = node
            .classes
            .iter()
            .map(AsRef::<str>::as_ref)
            .collect::<Vec<_>>()
            .join(" ");
        node.attrs.insert(LocalName::from("class"), class_value);
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
        self.note_class_attribute_change(id);
        let node = self
            .tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::remove_class");
        node.classes.retain(|existing| *existing != class);
        let class_value = node
            .classes
            .iter()
            .map(AsRef::<str>::as_ref)
            .collect::<Vec<_>>()
            .join(" ");
        node.attrs.insert(LocalName::from("class"), class_value);
    }

    /// Set or clear the node's `id` selector value.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn set_id_attr(&mut self, id: NodeId, value: Option<&str>) {
        self.live_element(id);
        self.note_id_attribute_change(id);
        let node = self
            .tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::set_id_attr");
        node.id_attr = value.map(Atom::from);
        match value {
            Some(value) => {
                node.attrs.insert(LocalName::from("id"), value.to_owned());
            }
            None => {
                node.attrs.remove(&LocalName::from("id"));
            }
        }
    }

    /// Set a DOM attribute. `id`, `class`, and `style` update their reflected
    /// selector/cascade state through the same operation.
    ///
    /// The authored string name is interned as a [`LocalName`] before it is
    /// stored or exposed to stylo's invalidation machinery.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn set_attribute(&mut self, id: NodeId, name: &str, value: &str) {
        match name {
            "id" => return self.set_id_attr(id, Some(value)),
            "class" => return self.set_classes(id, value),
            "style" => return self.set_inline_style(id, value),
            _ => {}
        }
        self.live_element(id);
        let name = LocalName::from(name);
        self.note_attribute_change(id, &name);
        self.tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::set_attribute")
            .attrs
            .insert(name, value.to_owned());
    }

    /// Remove a DOM attribute (a no-op when absent), including its reflected
    /// `id`, `class`, or inline-style state.
    ///
    /// # Panics
    ///
    /// Panics when `id` is stale (the let-it-crash mutation contract; see
    /// the crate docs), or when it names a text node.
    pub fn remove_attribute(&mut self, id: NodeId, name: &str) {
        if self.live_element(id).attr(name).is_none() {
            return;
        }
        match name {
            "id" => return self.set_id_attr(id, None),
            "class" => {
                self.note_class_attribute_change(id);
                let node = self
                    .tree_mut()
                    .get_mut(id)
                    .expect("stale NodeId passed to Document::remove_attribute");
                node.classes.clear();
                node.attrs.remove(&LocalName::from("class"));
                return;
            }
            "style" => {
                self.note_attribute_change(id, &LocalName::from("style"));
                let node = self
                    .tree_mut()
                    .get_mut(id)
                    .expect("stale NodeId passed to Document::remove_attribute");
                node.inline_block = None;
                node.attrs.remove(&LocalName::from("style"));
                self.note_inline_style_change(id);
                return;
            }
            _ => {}
        }
        let name = LocalName::from(name);
        self.note_attribute_change(id, &name);
        self.tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::remove_attribute")
            .attrs
            .remove(&name);
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
        self.tree_mut()
            .get_mut(id)
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
        self.tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::set_text")
            .set_literal_text(text);
        if let Some(element) = affected_element
            && watches_empty
            && was_empty != self.live_element(element).is_empty_element()
        {
            self.note_emptiness_change(element);
            self.mark_ancestors_dirty_descendants(element);
        }
        self.invalidate_layout(id);
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
        self.note_attribute_change(id, &LocalName::from("style"));
        let block = if css.is_empty() {
            None
        } else {
            let document = self.root_node();
            let parsed = parse_style_attribute(
                css,
                document.document_url_data(),
                None,
                QuirksMode::NoQuirks,
                CssRuleType::Style,
            );
            Some(Arc::new(document.document_lock().wrap(parsed)))
        };
        let node = self
            .tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::set_inline_style");
        node.inline_block = block;
        node.attrs.insert(LocalName::from("style"), css.to_owned());
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

        let document = self.root_node();
        let mut source = SourcePropertyDeclaration::default();
        if parse_one_declaration_into(
            &mut source,
            property_id,
            value,
            Origin::Author,
            document.document_url_data(),
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
                let guard = document.document_lock().read();
                existing.read_with(&guard).clone()
            }
            None => PropertyDeclarationBlock::new(),
        };
        block.extend(source.drain(), Importance::Normal);
        let wrapped = Arc::new(document.document_lock().wrap(block));

        let mut css = self.live(id).attr("style").unwrap_or_default().to_owned();
        if !css.is_empty() && !css.trim_end().ends_with(';') {
            css.push(';');
        }
        if !css.is_empty() {
            css.push(' ');
        }
        css.push_str(name);
        css.push_str(": ");
        css.push_str(value);
        css.push(';');

        self.note_attribute_change(id, &LocalName::from("style"));
        let node = self
            .tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::add_inline_style");
        node.inline_block = Some(wrapped);
        node.attrs.insert(LocalName::from("style"), css);
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
        let document = self.root_node();
        let Some(block) = &self.live(id).inline_block else {
            return 0;
        };
        let guard = document.document_lock().read();
        block.read_with(&guard).declarations().len()
    }

    fn note_inline_style_change(&mut self, id: NodeId) {
        self.add_restyle_hint(id, RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
    }

    // --- snapshot recording ------------------------------------------------------

    /// A reflected `class` mutation can affect both `.token` selectors and
    /// ordinary attribute selectors such as `[class~="token"]`.
    fn note_class_attribute_change(&mut self, id: NodeId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.class_changed = true;
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, &LocalName::from("class"));
        }
        self.mark_mutated(id);
    }

    /// A reflected `id` mutation can affect both `#id` selectors and ordinary
    /// attribute selectors such as `[id="value"]`.
    fn note_id_attribute_change(&mut self, id: NodeId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.id_changed = true;
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, &LocalName::from("id"));
        }
        self.mark_mutated(id);
    }

    fn note_attribute_change(&mut self, id: NodeId, name: &LocalName) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, name);
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
                .tree_mut()
                .get_mut(id)
                .expect("live node disappeared while recording its snapshot");
            node.snapshot = Some(Box::new(snapshot));
            node.set_snapshot_present();
        }
        self.tree_mut()
            .get_mut(id)
            .expect("live node disappeared while refining its snapshot")
            .snapshot
            .as_deref_mut()
    }
}

fn push_changed_attr(snapshot: &mut Snapshot, name: &LocalName) {
    if !snapshot.changed_attrs.contains(name) {
        snapshot.changed_attrs.push(name.clone());
    }
}

/// Build a stylo element snapshot of the node's *current* (soon to be old)
/// state: dynamic pseudo-class bits plus every matching-relevant DOM
/// attribute — the id selector value, classes, and real attributes.
fn build_snapshot<T>(node: &Node<T>) -> Snapshot {
    let mut attrs: Vec<(AttrIdentifier, AttrValue)> = Vec::new();

    // "id" and "class" go FIRST, and with the exact `AttrValue` variants the
    // snapshot accessors demand (`id_attr` calls `as_atom`, `has_class`/
    // `each_class` call `as_tokens`). The reflected entries in `attrs` are
    // skipped below so every real DOM attribute appears exactly once.
    if let Some(id_atom) = &node.id_attr {
        attrs.push((
            attr_identifier(LocalName::from("id")),
            AttrValue::Atom(id_atom.clone()),
        ));
    }
    if !node.classes.is_empty() {
        attrs.push((
            attr_identifier(LocalName::from("class")),
            AttrValue::TokenList(
                std::sync::OnceLock::new(),
                node.classes.iter().cloned().collect(),
            ),
        ));
    }
    for (name, value) in &node.attrs {
        if matches!(name.0.as_ref(), "id" | "class") {
            continue;
        }
        attrs.push((
            attr_identifier(name.clone()),
            AttrValue::String(value.clone()),
        ));
    }
    let mut snapshot = Snapshot::new();
    snapshot.state = Some(node.element_state());
    snapshot.attrs = Some(attrs);
    snapshot
}

fn attr_identifier(local_name: LocalName) -> AttrIdentifier {
    AttrIdentifier {
        name: local_name.clone(),
        local_name,
        namespace: stylo::Namespace::default(),
        prefix: None,
    }
}
