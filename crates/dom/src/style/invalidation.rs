//! Matching-relevant mutation, with its style invalidation baked in.

use std::collections::hash_map::Entry;
use std::sync::LazyLock;

use selectors::matching::ElementSelectorFlags;
use stylo::LocalName;
use stylo::attr::{AttrIdentifier, AttrValue};
use stylo::context::QuirksMode;
use stylo::dom::OpaqueNode;
use stylo::invalidation::element::restyle_hints::RestyleHint;
use stylo::properties::PropertyDeclarationBlock;
use stylo::properties::declaration_block::parse_style_attribute;
use stylo::selector_parser::Snapshot;
use stylo::servo_arc::Arc;
use stylo::shared_lock::Locked;
use stylo::stylesheets::CssRuleType;
use stylo_atoms::Atom;

use crate::tree::document::{DOCUMENT_NODE_ID, Document, NodeId};
use crate::tree::node::Node;
use crate::tree::shadow::is_slot_assignment_attribute;

const STRUCTURE_SENSITIVE: ElementSelectorFlags = ElementSelectorFlags::HAS_SLOW_SELECTOR
    .union(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS)
    .union(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR)
    .union(ElementSelectorFlags::HAS_EMPTY_SELECTOR)
    .union(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION);

static CLASS: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("class"));
static ID: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("id"));
static STYLE: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("style"));

impl<T> Document<T> {
    pub(crate) fn mark_subtree_dirty(&mut self, id: NodeId) {
        let node = self.live_element(id);
        if !node.flat_children().is_empty() {
            node.set_dirty_descendants_bit(true);
        }
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
        self.mark_ancestors_dirty_descendants(id);
    }

    pub(crate) fn live(&self, id: NodeId) -> &Node<T> {
        self.get(id)
            .expect("stale NodeId passed to a Document method")
    }

    pub(crate) fn live_element(&self, id: NodeId) -> &Node<T> {
        let node = self.live(id);
        assert!(
            node.is_element(),
            "element-only Document method called with a non-element node"
        );
        node
    }

    fn add_restyle_hint(&mut self, id: NodeId, hint: RestyleHint) {
        insert_restyle_hint(self.live_node_mut(id), hint);
    }

    pub(crate) fn mark_ancestors_dirty_descendants(&mut self, id: NodeId) {
        let tree = self.arenas();
        let mut next = tree.get(id).and_then(Node::flat_parent_id);
        while let Some(pid) = next {
            if pid == DOCUMENT_NODE_ID {
                break;
            }
            let parent = tree.get(pid).expect("internal tree links always resolve");
            if parent.has_dirty_descendants() {
                break;
            }
            parent.set_dirty_descendants_bit(true);
            next = parent.flat_parent_id();
        }
    }

    pub(crate) fn note_moved_subtree(&mut self, id: NodeId) {
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
    }

    pub(crate) fn note_child_list_change(&mut self, parent: NodeId, index: usize) {
        self.note_visual_mutation();
        let parent_node = self.live(parent);
        let flags = parent_node.selector_flags();
        if flags.intersects(STRUCTURE_SENSITIVE) {
            let children: Vec<NodeId> = parent_node.child_ids().collect();
            if flags.intersects(ElementSelectorFlags::HAS_EMPTY_SELECTOR) {
                self.note_emptiness_change(parent);
            }
            let (hint, affected) = if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR) {
                (Some(RestyleHint::restyle_subtree()), &children[..])
            } else if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS) {
                (
                    Some(RestyleHint::restyle_subtree()),
                    children.get(index..).unwrap_or_default(),
                )
            } else if flags.intersects(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION) {
                (Some(RestyleHint::RECASCADE_SELF), &children[..])
            } else {
                (None, &[][..])
            };
            if let Some(hint) = hint {
                for &child in affected {
                    if self.live(child).is_element() {
                        self.add_restyle_hint(child, hint);
                    }
                }
            }
            if flags.intersects(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR) {
                let mut forward = children.iter().filter(|&&c| self.live(c).is_element());
                let mut backward = children
                    .iter()
                    .rev()
                    .filter(|&&c| self.live(c).is_element());
                let edges = [
                    forward.next().copied(),
                    forward.next().copied(),
                    backward.next().copied(),
                    backward.next().copied(),
                ];
                for child in edges.into_iter().flatten() {
                    self.add_restyle_hint(child, RestyleHint::restyle_subtree());
                }
            }
        }
        {
            let node = self.live(parent);
            if !node.flat_children().is_empty() {
                node.set_dirty_descendants_bit(true);
            }
        }
        self.mark_ancestors_dirty_descendants(parent);
    }

    fn note_emptiness_change(&mut self, id: NodeId) {
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
        let later_siblings: Vec<NodeId> = {
            let tree = self.arenas();
            tree.get(id)
                .and_then(|node| {
                    let self_slot = tree.slot(id)?;
                    let siblings = tree.at(node.parent_slot()?).child_slots();
                    let pos = siblings.iter().position(|&c| c == self_slot)?;
                    Some(
                        siblings[pos + 1..]
                            .iter()
                            .map(|&slot| tree.at(slot).id())
                            .collect::<Vec<_>>(),
                    )
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
        let base = self.begin_reactions();
        self.enqueue_attribute_changed(id, &CLASS, Some(classes));
        self.note_class_attribute_change(id);
        let node = self.live_node_mut(id);
        node.classes = classes.split_whitespace().map(Atom::from).collect();
        node.set_attr_local_name(CLASS.clone(), classes.to_owned());
        self.drain_reactions(base);
    }

    pub fn add_class(&mut self, id: NodeId, class: &str) {
        let class = Atom::from(class);
        if self.live_element(id).classes.contains(&class) {
            return;
        }
        let base = self.begin_reactions();
        let old = self.observed_class_value(id);
        self.note_class_attribute_change(id);
        let node = self.live_node_mut(id);
        node.classes.push(class);
        sync_class_attribute(node);
        let new = self.observed_class_value(id);
        self.enqueue_attribute_changed_values(id, &CLASS, old, new);
        self.drain_reactions(base);
    }

    pub fn remove_class(&mut self, id: NodeId, class: &str) {
        let class = Atom::from(class);
        if !self.live_element(id).classes.contains(&class) {
            return;
        }
        let base = self.begin_reactions();
        let old = self.observed_class_value(id);
        self.note_class_attribute_change(id);
        let node = self.live_node_mut(id);
        node.classes.retain(|existing| *existing != class);
        sync_class_attribute(node);
        let new = self.observed_class_value(id);
        self.enqueue_attribute_changed_values(id, &CLASS, old, new);
        self.drain_reactions(base);
    }

    fn observed_class_value(&self, id: NodeId) -> Option<String> {
        if !self.observes_attribute(id, &CLASS) {
            return None;
        }
        self.live(id).attr_local_name(&CLASS).map(str::to_owned)
    }

    pub fn set_id_attribute(&mut self, id: NodeId, value: Option<&str>) {
        let base = self.begin_reactions();
        if value.is_some() || self.live(id).attr_local_name(&ID).is_some() {
            self.enqueue_attribute_changed(id, &ID, value);
        }
        self.note_id_attribute_change(id);
        let node = self.live_node_mut(id);
        node.id_attribute = value.map(Atom::from);
        match value {
            Some(value) => node.set_attr_local_name(ID.clone(), value.to_owned()),
            None => node.remove_attr_local_name(&ID),
        }
        self.drain_reactions(base);
    }

    pub fn set_attribute(&mut self, id: NodeId, name: &str, value: &str) {
        match name {
            "id" => return self.set_id_attribute(id, Some(value)),
            "class" => return self.set_classes(id, value),
            "style" => return self.set_inline_style(id, value),
            _ => {}
        }
        let slot_assignment = is_slot_assignment_attribute(name);
        let name = LocalName::from(name);
        let base = self.begin_reactions();
        self.enqueue_attribute_changed(id, &name, Some(value));
        self.note_attribute_change(id, &name);
        self.live_node_mut(id)
            .set_attr_local_name(name, value.to_owned());
        if slot_assignment {
            self.note_slot_assignment_attribute(id);
        }
        self.drain_reactions(base);
    }

    pub fn remove_attribute(&mut self, id: NodeId, name: &str) {
        if self.live_element(id).attribute(name).is_none() {
            return;
        }
        match name {
            "id" => return self.set_id_attribute(id, None),
            "class" => {
                let base = self.begin_reactions();
                self.enqueue_attribute_changed(id, &CLASS, None);
                self.note_class_attribute_change(id);
                let node = self.live_node_mut(id);
                node.classes.clear();
                node.remove_attr_local_name(&CLASS);
                self.drain_reactions(base);
                return;
            }
            "style" => {
                let base = self.begin_reactions();
                self.enqueue_attribute_changed(id, &STYLE, None);
                self.apply_inline_style_block(id, None, None);
                self.drain_reactions(base);
                return;
            }
            _ => {}
        }
        let slot_assignment = is_slot_assignment_attribute(name);
        let name = LocalName::from(name);
        let base = self.begin_reactions();
        self.enqueue_attribute_changed(id, &name, None);
        self.note_attribute_change(id, &name);
        self.live_node_mut(id).remove_attr_local_name(&name);
        if slot_assignment {
            self.note_slot_assignment_attribute(id);
        }
        self.drain_reactions(base);
    }

    pub fn add_element_state(&mut self, id: NodeId, flags: stylo_dom::ElementState) {
        self.update_element_state(id, flags, true);
    }

    pub fn remove_element_state(&mut self, id: NodeId, flags: stylo_dom::ElementState) {
        self.update_element_state(id, flags, false);
    }

    fn update_element_state(&mut self, id: NodeId, flags: stylo_dom::ElementState, enabled: bool) {
        assert!(
            !flags.contains(stylo_dom::ElementState::DEFINED),
            "Document::{{add,remove}}_element_state: `:defined` is owned by the custom element \
             state machine and is not settable as element state"
        );
        self.ensure_snapshot(id);
        self.mark_ancestors_dirty_descendants(id);
        self.live_node_mut(id).element_state.set(flags, enabled);
    }

    fn set_element_text_content(&mut self, id: NodeId, text: Option<String>) {
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
        self.live_node_mut(id).set_literal_text(text);
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
        let base = self.begin_reactions();
        self.enqueue_attribute_changed(id, &STYLE, Some(css));
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
            Some(Arc::new(self.style_engine().shared_lock().wrap(parsed)))
        };
        self.apply_inline_style_block(id, block, Some(css.to_owned()));
        self.drain_reactions(base);
    }

    fn apply_inline_style_block(
        &mut self,
        id: NodeId,
        block: Option<Arc<Locked<PropertyDeclarationBlock>>>,
        css: Option<String>,
    ) {
        self.note_visual_mutation();
        self.note_attribute_change(id, &STYLE);
        let node = self.live_node_mut(id);
        node.parsed_inline_style = block;
        match css {
            Some(css) => node.set_attr_local_name(STYLE.clone(), css),
            None => node.remove_attr_local_name(&STYLE),
        }
        insert_restyle_hint(node, RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
    }

    fn note_class_attribute_change(&mut self, id: NodeId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.class_changed = true;
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, &CLASS);
        }
        self.mark_ancestors_dirty_descendants(id);
    }

    fn note_id_attribute_change(&mut self, id: NodeId) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.id_changed = true;
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, &ID);
        }
        self.mark_ancestors_dirty_descendants(id);
    }

    fn note_attribute_change(&mut self, id: NodeId, name: &LocalName) {
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, name);
        }
        self.mark_ancestors_dirty_descendants(id);
    }

    fn ensure_snapshot(&mut self, id: NodeId) -> Option<&mut Snapshot> {
        self.note_visual_mutation();
        if !self.live_element(id).has_style_data() {
            return None;
        }
        let opaque = OpaqueNode(id);
        let (nodes, pending_snapshots) = self.snapshot_storage();
        match pending_snapshots.entry(opaque) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                let node = nodes
                    .get(id)
                    .expect("live node disappeared while recording its snapshot");
                let snapshot = entry.insert(build_snapshot(node));
                node.set_snapshot_present();
                Some(snapshot)
            }
        }
    }
}

fn insert_restyle_hint<T>(node: &mut Node<T>, hint: RestyleHint) {
    if let Some(wrapper) = node.stylo_data_mut() {
        wrapper.borrow_mut().hint.insert(hint);
    }
}

fn sync_class_attribute<T>(node: &mut Node<T>) {
    let value = node
        .classes
        .iter()
        .map(AsRef::<str>::as_ref)
        .collect::<Vec<_>>()
        .join(" ");
    node.set_attr_local_name(CLASS.clone(), value);
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
            attr_identifier(ID.clone()),
            AttrValue::Atom(id_atom.clone()),
        ));
    }
    if !node.classes.is_empty() {
        attrs.push((
            attr_identifier(CLASS.clone()),
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
