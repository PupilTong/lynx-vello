//! Matching-relevant mutation, with its style invalidation baked in.

use std::sync::atomic::Ordering;

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
use crate::node::{Node, SNAPSHOT_PRESENT};

const STRUCTURE_SENSITIVE: ElementSelectorFlags = ElementSelectorFlags::HAS_SLOW_SELECTOR
    .union(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS)
    .union(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR)
    .union(ElementSelectorFlags::HAS_EMPTY_SELECTOR)
    .union(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION);

impl<T> Document<T> {
    pub(crate) fn mark_subtree_dirty(&mut self, id: NodeId) {
        let node = self.live_element(id);
        if !node.child_ids().is_empty() {
            node.set_dirty_descendants_bit(true);
        }
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
        self.mark_ancestors_dirty_descendants(id);
    }

    fn live(&self, id: NodeId) -> &Node<T> {
        self.get(id)
            .expect("stale NodeId passed to a Document mutation method")
    }

    fn live_element(&self, id: NodeId) -> &Node<T> {
        let node = self.live(id);
        assert!(
            node.is_element(),
            "element-only Document mutation called with a non-element node"
        );
        node
    }

    pub(crate) fn add_restyle_hint(&mut self, id: NodeId, hint: RestyleHint) {
        if let Some(wrapper) = self.tree_mut().get_mut(id).and_then(Node::stylo_data_mut) {
            wrapper.borrow_mut().hint.insert(hint);
        }
    }

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

    fn mark_mutated(&mut self, id: NodeId) {
        self.live(id);
        self.mark_ancestors_dirty_descendants(id);
    }

    pub(crate) fn note_moved_subtree(&mut self, id: NodeId) {
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
    }

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
        {
            let node = self.live(parent);
            if !node.child_ids().is_empty() {
                node.set_dirty_descendants_bit(true);
            }
        }
        self.mark_ancestors_dirty_descendants(parent);
    }

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

    pub fn set_id_attribute(&mut self, id: NodeId, value: Option<&str>) {
        self.live_element(id);
        self.note_id_attribute_change(id);
        let node = self
            .tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::set_id_attribute");
        node.id_attribute = value.map(Atom::from);
        match value {
            Some(value) => {
                node.attrs.insert(LocalName::from("id"), value.to_owned());
            }
            None => {
                node.attrs.remove(&LocalName::from("id"));
            }
        }
    }

    pub fn set_attribute(&mut self, id: NodeId, name: &str, value: &str) {
        match name {
            "id" => return self.set_id_attribute(id, Some(value)),
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

    pub fn remove_attribute(&mut self, id: NodeId, name: &str) {
        if self.live_element(id).attribute(name).is_none() {
            return;
        }
        match name {
            "id" => return self.set_id_attribute(id, None),
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

    pub fn add_element_state(&mut self, id: NodeId, flags: dom::ElementState) {
        self.update_element_state(id, flags, true);
    }

    pub fn remove_element_state(&mut self, id: NodeId, flags: dom::ElementState) {
        self.update_element_state(id, flags, false);
    }

    fn update_element_state(&mut self, id: NodeId, flags: dom::ElementState, enabled: bool) {
        self.live_element(id);
        self.ensure_snapshot(id);
        self.mark_mutated(id);
        self.tree_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::update_element_state")
            .element_state
            .set(flags, enabled);
    }

    pub fn set_element_text_content(&mut self, id: NodeId, text: Option<String>) {
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
            .expect("stale NodeId passed to Document::set_element_text_content")
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

    pub fn set_text_node_data(&mut self, id: NodeId, text: impl Into<String>) {
        assert!(
            self.live(id).is_text_node(),
            "Document::set_text_node_data called with an element node"
        );
        self.set_element_text_content(id, Some(text.into()));
    }

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

        let mut css = self
            .live(id)
            .attribute("style")
            .unwrap_or_default()
            .to_owned();
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

    fn note_class_attribute_change(&mut self, id: NodeId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.class_changed = true;
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, &LocalName::from("class"));
        }
        self.mark_mutated(id);
    }

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

    fn ensure_snapshot(&mut self, id: NodeId) -> Option<&mut Snapshot> {
        let node = self.live(id);
        if !node.has_style_data() {
            return None;
        }
        if self
            .styling_data(id)
            .expect("live node must have styling-arena state")
            .snapshot
            .is_none()
        {
            let snapshot = build_snapshot(node);
            let styling = self
                .styling_data_mut(id)
                .expect("live node disappeared while recording its snapshot");
            styling.snapshot = Some(Box::new(snapshot));
            styling
                .snapshot_flags
                .fetch_or(SNAPSHOT_PRESENT, Ordering::Relaxed);
        }
        self.styling_data_mut(id)
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

fn build_snapshot<T>(node: &Node<T>) -> Snapshot {
    let mut attrs: Vec<(AttrIdentifier, AttrValue)> = Vec::new();

    if let Some(id_atom) = &node.id_attribute {
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
