//! Matching-relevant mutation, with its style invalidation baked in.

use std::borrow::Cow;
use std::sync::LazyLock;

use selectors::matching::ElementSelectorFlags;
use stylo::LocalName;
use stylo::attr::{AttrIdentifier, AttrValue};
use stylo::computed_value_flags::ComputedValueFlags;
use stylo::context::QuirksMode;
use stylo::dom::OpaqueNode;
use stylo::invalidation::element::restyle_hints::RestyleHint;
use stylo::properties::PropertyDeclarationBlock;
use stylo::properties::declaration_block::parse_style_attribute;
use stylo::selector_parser::Snapshot;
use stylo::servo_arc::Arc;
use stylo::shared_lock::Locked;
use stylo::stylesheets::CssRuleType;
use stylo::stylist::CascadeData;
use stylo_atoms::Atom;
use stylo_traits::ParsingMode;

use crate::style::engine::PropertyUpdate;
use crate::tree::document::{DOCUMENT_NODE_ID, Document, NodeId};
use crate::tree::node::Node;
use crate::tree::popover::is_popover_attribute;
use crate::tree::shadow::{
    PLACEHOLDER_PSEUDO_ID, is_pseudo_attribute, is_slot_assignment_attribute,
};

const STRUCTURE_SENSITIVE: ElementSelectorFlags = ElementSelectorFlags::HAS_SLOW_SELECTOR
    .union(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS)
    .union(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR)
    .union(ElementSelectorFlags::HAS_EMPTY_SELECTOR)
    .union(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION);

static CLASS: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("class"));
static ID: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("id"));
static STYLE: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("style"));
static LANG: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("lang"));
static PART: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("part"));
static EXPORTPARTS: LazyLock<LocalName> = LazyLock::new(|| LocalName::from("exportparts"));

impl<T> Document<T> {
    pub(crate) fn mark_subtree_dirty(&mut self, id: NodeId) {
        let node = self.live_element(id);
        if !node.flat_children().is_empty() {
            node.set_dirty_descendants_bit(true);
        }
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
        self.mark_ancestors_dirty_descendants(id);
    }

    pub(crate) fn mark_subtree_recascade(&mut self, id: NodeId) {
        let node = self.live_element(id);
        if !node.flat_children().is_empty() {
            node.set_dirty_descendants_bit(true);
        }
        self.add_restyle_hint(
            id,
            RestyleHint::RESTYLE_SELF | RestyleHint::RECASCADE_DESCENDANTS,
        );
        self.mark_ancestors_dirty_descendants(id);
    }

    pub(crate) fn mark_container_units_users(&mut self, container: NodeId) -> bool {
        let mut users = Vec::new();
        {
            let tree = self.arenas();
            let mut stack: Vec<NodeId> = tree
                .get(container)
                .map(|node| node.flat_children().to_vec())
                .unwrap_or_default();
            while let Some(id) = stack.pop() {
                let Some(node) = tree.get(id) else { continue };
                stack.extend_from_slice(node.flat_children());
                if reads_container_units(node) {
                    users.push(id);
                }
            }
        }
        if users.is_empty() {
            return false;
        }
        for id in users {
            self.add_restyle_hint(id, RestyleHint::RECASCADE_SELF);
            self.mark_dirty_descendants_below(id, container);
        }
        self.mark_ancestors_dirty_descendants(container);
        true
    }

    fn mark_dirty_descendants_below(&mut self, id: NodeId, container: NodeId) {
        let tree = self.arenas();
        let mut next = tree.get(id).and_then(Node::flat_parent_id);
        while let Some(pid) = next {
            let Some(node) = tree.get(pid) else { break };
            if node.has_dirty_descendants() {
                break;
            }
            node.set_dirty_descendants_bit(true);
            if pid == container {
                break;
            }
            next = node.flat_parent_id();
        }
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
        let flags = self.live(parent).selector_flags();
        if flags.intersects(STRUCTURE_SENSITIVE) {
            if flags.intersects(ElementSelectorFlags::HAS_EMPTY_SELECTOR) {
                self.note_emptiness_change(parent);
            }
            let restyle_from = if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR) {
                Some(0)
            } else if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS) {
                Some(index)
            } else {
                None
            };
            if let Some(from) = restyle_from {
                let parent_node = self.live(parent);
                let flat_children_are_dom_children = !self.has_shadow_roots()
                    || (parent_node.shadow_root_id().is_none() && !parent_node.is_slot());
                let ancestor_hint_carries =
                    from == 0 && flat_children_are_dom_children && parent_node.has_style_data();
                if ancestor_hint_carries {
                    self.add_restyle_hint(parent, RestyleHint::RESTYLE_DESCENDANTS);
                } else {
                    let mut position = from;
                    while let Some(&child) = self.live(parent).child_ids().get(position) {
                        if self.live(child).is_element() {
                            self.add_restyle_hint(child, RestyleHint::restyle_subtree());
                        }
                        position += 1;
                    }
                }
            } else if flags.intersects(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION) {
                let mut position = 0;
                while let Some(&child) = self.live(parent).child_ids().get(position) {
                    if self.live(child).is_element() {
                        self.add_restyle_hint(child, RestyleHint::RECASCADE_SELF);
                    }
                    position += 1;
                }
            }
            if flags.intersects(ElementSelectorFlags::HAS_EDGE_CHILD_SELECTOR) {
                for child in self.edge_element_children(parent).into_iter().flatten() {
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

    fn edge_element_children(&self, parent: NodeId) -> [Option<NodeId>; 4] {
        let children = self.live(parent).child_ids();
        let mut forward = children
            .iter()
            .filter(|&&child| self.live(child).is_element());
        let mut backward = children
            .iter()
            .rev()
            .filter(|&&child| self.live(child).is_element());
        [
            forward.next().copied(),
            forward.next().copied(),
            backward.next().copied(),
            backward.next().copied(),
        ]
    }

    fn note_emptiness_change(&mut self, id: NodeId) {
        self.add_restyle_hint(id, RestyleHint::restyle_subtree());
        let Some(after) = self.sibling_position(id).map(|position| position + 1) else {
            return;
        };
        let mut position = after;
        while let Some(sibling) = self.sibling_at(id, position) {
            self.add_restyle_hint(sibling, RestyleHint::restyle_subtree());
            position += 1;
        }
    }

    fn sibling_position(&self, id: NodeId) -> Option<usize> {
        let tree = self.arenas();
        let node = tree.get(id)?;
        let parent = tree.at(node.parent_slot()?);
        let index = node.index_in(parent);
        (parent.child_slots().get(index) == Some(&id)).then_some(index)
    }

    fn sibling_at(&self, id: NodeId, position: usize) -> Option<NodeId> {
        let tree = self.arenas();
        let parent = tree.get(id)?.parent_slot()?;
        tree.at(parent)
            .child_slots()
            .get(position)
            .map(|&slot| tree.at(slot).id())
    }
}

impl<T> Document<T> {
    pub fn set_classes(&mut self, id: NodeId, classes: &str) {
        let base = self.begin_reactions();
        self.enqueue_attribute_changed(id, &CLASS, Some(classes));
        self.note_class_attribute_change(id);
        let node = self.live_node_mut(id);
        node.classes = classes.split_whitespace().map(Atom::from).collect();
        let canonical = classes
            .split(' ')
            .eq(node.classes.iter().map(AsRef::<str>::as_ref));
        let text = if canonical {
            String::new()
        } else {
            classes.to_owned()
        };
        node.set_attr_local_name(CLASS.clone(), text);
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
        node.set_attr_local_name(CLASS.clone(), String::new());
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
        node.set_attr_local_name(CLASS.clone(), String::new());
        let new = self.observed_class_value(id);
        self.enqueue_attribute_changed_values(id, &CLASS, old, new);
        self.drain_reactions(base);
    }

    fn observed_class_value(&self, id: NodeId) -> Option<String> {
        if !self.observes_attribute(id, &CLASS) {
            return None;
        }
        self.live(id).attr_local_name(&CLASS).map(Cow::into_owned)
    }

    pub fn set_id_attribute(&mut self, id: NodeId, value: Option<&str>) {
        let base = self.begin_reactions();
        if value.is_some() || self.live(id).plain_attr(&ID).is_some() {
            self.enqueue_attribute_changed(id, &ID, value);
        }
        self.note_id_attribute_change(id);
        let node = self.live_node_mut(id);
        node.id_attribute = value.map(Atom::from);
        match value {
            Some(value) => node.set_attr_local_name(ID.clone(), value.to_owned()),
            None => node.remove_attr_local_name(&ID),
        }
        if self.live(id).in_svg_namespace() {
            self.note_svg_change(id);
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
        let popover = is_popover_attribute(name).then(|| self.popover_state(id));
        let editing = crate::editing::is_contenteditable_attribute(name);
        let pseudo = is_pseudo_attribute(name).then_some(value == PLACEHOLDER_PSEUDO_ID);
        if editing {
            self.note_contenteditable_change(id, Some(value));
        }
        let name = LocalName::from(name);
        let base = self.begin_reactions();
        self.enqueue_attribute_changed(id, &name, Some(value));
        self.note_attribute_change(id, &name);
        let node = self.live_node_mut(id);
        if node.in_svg_namespace() {
            node.set_attr_local_name(name.clone(), value.to_owned());
            self.reflect_svg_attribute(id, &name);
        } else {
            node.set_attr_local_name(name, value.to_owned());
        }
        if let Some(on) = pseudo {
            self.note_pseudo_attribute(id, on);
        }
        if slot_assignment {
            self.note_slot_assignment_attribute(id);
        }
        if let Some(old) = popover {
            self.note_popover_attribute_change(id, old);
        }
        if editing {
            self.fix_up_selection();
        }
        self.drain_reactions(base);
    }

    pub fn remove_attribute(&mut self, id: NodeId, name: &str) {
        if !self
            .live_element(id)
            .attribute_names()
            .any(|candidate| candidate == name)
        {
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
                self.note_attribute_change(id, &STYLE);
                self.apply_inline_style_block(id, None, None);
                self.drain_reactions(base);
                return;
            }
            _ => {}
        }
        let slot_assignment = is_slot_assignment_attribute(name);
        let popover = is_popover_attribute(name).then(|| self.popover_state(id));
        let editing = crate::editing::is_contenteditable_attribute(name);
        let pseudo = is_pseudo_attribute(name);
        if editing {
            self.note_contenteditable_change(id, None);
        }
        let name = LocalName::from(name);
        let base = self.begin_reactions();
        self.enqueue_attribute_changed(id, &name, None);
        self.note_attribute_change(id, &name);
        let node = self.live_node_mut(id);
        let svg = node.in_svg_namespace();
        node.remove_attr_local_name(&name);
        if svg {
            self.reflect_svg_attribute(id, &name);
        }
        if pseudo {
            self.note_pseudo_attribute(id, false);
        }
        if slot_assignment {
            self.note_slot_assignment_attribute(id);
        }
        if let Some(old) = popover {
            self.note_popover_attribute_change(id, old);
        }
        if editing {
            self.fix_up_selection();
        }
        self.drain_reactions(base);
    }

    fn note_pseudo_attribute(&mut self, id: NodeId, on: bool) {
        if self.live_node_mut(id).set_placeholder_pseudo(on) != on {
            self.mark_subtree_dirty(id);
        }
    }

    pub fn add_element_state(&mut self, id: NodeId, flags: stylo_dom::ElementState) {
        assert_settable_element_state(flags);
        self.update_element_state(id, flags, true);
    }

    pub fn remove_element_state(&mut self, id: NodeId, flags: stylo_dom::ElementState) {
        assert_settable_element_state(flags);
        self.update_element_state(id, flags, false);
    }

    pub(crate) fn update_element_state(
        &mut self,
        id: NodeId,
        flags: stylo_dom::ElementState,
        enabled: bool,
    ) {
        self.live_element(id);
        if self.any_rule_depends_on_state(id, flags) {
            self.ensure_snapshot(id);
            self.mark_ancestors_dirty_descendants(id);
        } else {
            self.note_visual_mutation();
        }
        self.live_node_mut(id).element_state.set(flags, enabled);
    }

    fn set_element_text_content(&mut self, id: NodeId, text: Option<Box<str>>) {
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
            && self.live(element).in_svg_namespace()
        {
            self.note_svg_change(element);
        }
        if let Some(element) = affected_element
            && watches_empty
            && was_empty != self.live_element(element).is_empty_element()
        {
            self.note_emptiness_change(element);
            self.mark_ancestors_dirty_descendants(element);
        }
        self.invalidate_text_artifact(id);
        self.invalidate_layout(id);
        if is_text_node {
            self.note_text_data_change(id);
        }
    }

    pub fn set_text_node_data(&mut self, id: NodeId, text: impl Into<Box<str>>) {
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
        self.note_attribute_change(id, &STYLE);
        self.apply_inline_style_block(id, block, Some(css.to_owned()));
        self.drain_reactions(base);
    }

    #[must_use]
    pub fn supports_style_property(&self, property: &str) -> bool {
        self.style_engine().supports_property(property)
    }

    pub fn set_inline_style_property(&mut self, id: NodeId, property: &str, value: &str) {
        let Ok(update) = self.style_engine().parse_style_update(
            self.live_element(id).parsed_inline_style.as_ref(),
            property,
            value,
            ParsingMode::DEFAULT,
        ) else {
            return;
        };

        let base = self.begin_reactions();
        let old = self.observed_style_value(id);
        self.note_attribute_change(id, &STYLE);
        let mut block = self.live_node_mut(id).parsed_inline_style.take();
        self.style_engine().apply_style_update(&mut block, update);
        self.apply_inline_style_block(id, block, Some(String::new()));
        let new = self.observed_style_value(id);
        self.enqueue_attribute_changed_values(id, &STYLE, old, new);
        self.drain_reactions(base);
    }

    pub fn set_presentational_hint(&mut self, id: NodeId, property: &str, value: &str) {
        let Ok(update) = self.style_engine().parse_style_update(
            self.live_element(id).presentational_hints.as_ref(),
            property,
            value,
            ParsingMode::DEFAULT,
        ) else {
            return;
        };
        self.apply_presentational_hint_update(id, update);
    }

    pub(crate) fn apply_presentational_hint_update(&mut self, id: NodeId, update: PropertyUpdate) {
        let (engine, tree) = self.style_and_tree_parts();
        let node = tree
            .get_mut(id)
            .expect("stale NodeId passed to a Document mutation method");
        engine.apply_style_update(&mut node.presentational_hints, update);

        self.note_visual_mutation();
        self.add_restyle_hint(id, RestyleHint::RESTYLE_SELF);
        self.mark_ancestors_dirty_descendants(id);
    }

    pub fn set_inline_style_declarations<'a>(
        &mut self,
        id: NodeId,
        declarations: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) {
        let block = self.style_engine().build_inline_style_block(declarations);
        let base = self.begin_reactions();
        let old = self.observed_style_value(id);
        self.note_attribute_change(id, &STYLE);
        self.apply_inline_style_block(id, block, Some(String::new()));
        let new = self.observed_style_value(id);
        self.enqueue_attribute_changed_values(id, &STYLE, old, new);
        self.drain_reactions(base);
    }

    fn observed_style_value(&self, id: NodeId) -> Option<String> {
        if !self.observes_attribute(id, &STYLE) {
            return None;
        }
        self.live(id).attr_local_name(&STYLE).map(Cow::into_owned)
    }

    fn apply_inline_style_block(
        &mut self,
        id: NodeId,
        block: Option<Arc<Locked<PropertyDeclarationBlock>>>,
        css: Option<String>,
    ) {
        self.note_visual_mutation();
        let node = self.live_node_mut(id);
        node.parsed_inline_style = block;
        match css {
            Some(css) => node.set_attr_local_name(STYLE.clone(), css),
            None => node.remove_attr_local_name(&STYLE),
        }
        insert_restyle_hint(node, RestyleHint::RESTYLE_STYLE_ATTRIBUTE);
        self.mark_ancestors_dirty_descendants(id);
    }

    fn note_generated_attribute_change(&mut self, id: NodeId, name: &LocalName) {
        let depends = self
            .live_element(id)
            .layout_computed_style()
            .is_some_and(|style| {
                style
                    .attribute_references
                    .as_ref()
                    .is_some_and(|references| references.contains_key(name))
            });
        if depends {
            insert_restyle_hint(self.live_node_mut(id), RestyleHint::RECASCADE_SELF);
            self.mark_ancestors_dirty_descendants(id);
            self.invalidate_layout(id);
        }
    }

    fn note_class_attribute_change(&mut self, id: NodeId) {
        self.note_generated_attribute_change(id, &CLASS);
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.class_changed = true;
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, &CLASS);
        }
        self.mark_ancestors_dirty_descendants(id);
    }

    fn note_id_attribute_change(&mut self, id: NodeId) {
        self.note_generated_attribute_change(id, &ID);
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.id_changed = true;
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, &ID);
        }
        self.mark_ancestors_dirty_descendants(id);
    }

    fn note_attribute_change(&mut self, id: NodeId, name: &LocalName) {
        self.note_generated_attribute_change(id, name);
        self.live_element(id);
        if !is_gate_exempt(name) && !self.any_rule_depends_on_attribute(id, name) {
            self.note_visual_mutation();
            return;
        }
        if let Some(snapshot) = self.ensure_snapshot(id) {
            snapshot.other_attributes_changed = true;
            push_changed_attr(snapshot, name);
        }
        self.mark_ancestors_dirty_descendants(id);
    }

    fn any_rule_depends_on_attribute(&self, id: NodeId, name: &LocalName) -> bool {
        self.any_applicable_cascade_data(id, &mut |data| data.might_have_attribute_dependency(name))
    }

    fn any_rule_depends_on_state(&self, id: NodeId, state: stylo_dom::ElementState) -> bool {
        self.any_applicable_cascade_data(id, &mut |data| data.has_state_dependency(state))
    }

    fn any_applicable_cascade_data(
        &self,
        id: NodeId,
        test: &mut dyn FnMut(&CascadeData) -> bool,
    ) -> bool {
        if self
            .style_engine()
            .stylist()
            .iter_origins()
            .any(|(data, _)| test(data))
        {
            return true;
        }
        if !self.has_shadow_roots() {
            return false;
        }
        let node = self.live(id);
        if let Some(data) = scoped_data(node.containing_shadow_root())
            && test(data)
        {
            return true;
        }
        if let Some(data) = scoped_data(node.shadow_root_id().map(|root| self.live(root)))
            && test(data)
        {
            return true;
        }
        let mut current = node.assigned_slot_id();
        while let Some(slot) = current {
            let slot = self.live(slot);
            if let Some(data) = scoped_data(slot.containing_shadow_root())
                && data.any_slotted_rule()
                && test(data)
            {
                return true;
            }
            current = slot.assigned_slot_id();
        }
        node.has_part_attr()
    }

    fn ensure_snapshot(&mut self, id: NodeId) -> Option<&mut Snapshot> {
        self.note_visual_mutation();
        let node = self.live_element(id);
        if !node.has_style_data() {
            return None;
        }
        let opaque = OpaqueNode(id.arena_key());
        let style_text = node.parsed_inline_style.is_some()
            && !self.has_pending_snapshot(opaque)
            && self.any_rule_depends_on_attribute(id, &STYLE);
        let (nodes, pending_snapshots) = self.snapshot_storage();
        Some(pending_snapshots.entry(opaque).or_insert_with(|| {
            let node = nodes
                .get(id)
                .expect("live node disappeared while recording its snapshot");
            let snapshot = build_snapshot(node, style_text);
            node.set_snapshot_present();
            snapshot
        }))
    }
}

fn assert_settable_element_state(flags: stylo_dom::ElementState) {
    assert!(
        !flags.contains(stylo_dom::ElementState::DEFINED),
        "Document::{{add,remove}}_element_state: `:defined` is owned by the custom element \
         state machine and is not settable as element state"
    );
    assert!(
        !flags.contains(stylo_dom::ElementState::POPOVER_OPEN),
        "Document::{{add,remove}}_element_state: `:popover-open` is owned by the popover \
         algorithms (`Document::show_popover`, `Document::hide_popover`) and is not settable \
         as element state"
    );
}

fn reads_container_units<T>(node: &Node<T>) -> bool {
    node.borrow_computed_style().is_some_and(|data| {
        data.styles.primary.as_ref().is_some_and(|style| {
            style
                .flags
                .contains(ComputedValueFlags::USES_CONTAINER_UNITS)
        })
    })
}

fn insert_restyle_hint<T>(node: &mut Node<T>, hint: RestyleHint) {
    if let Some(wrapper) = node.stylo_data_mut() {
        wrapper.borrow_mut().hint.insert(hint);
    }
}

fn scoped_data<T>(root: Option<&Node<T>>) -> Option<&CascadeData> {
    Some(&root?.shadow_data()?.styles.data)
}

fn is_gate_exempt(name: &LocalName) -> bool {
    *name == *LANG || *name == *PART || *name == *EXPORTPARTS
}

fn push_changed_attr(snapshot: &mut Snapshot, name: &LocalName) {
    if !snapshot.changed_attrs.contains(name) {
        snapshot.changed_attrs.push(name.clone());
    }
}

fn build_snapshot<T>(node: &Node<T>, style_text: bool) -> Snapshot {
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
    for (name, stored) in &node.attrs {
        if matches!(name.0.as_ref(), "id" | "class") {
            continue;
        }
        let value = if style_text || *name != *STYLE {
            node.attribute_text(name, stored).into_owned()
        } else {
            stored.clone()
        };
        attrs.push((attr_identifier(name.clone()), AttrValue::String(value)));
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::invalidation::element::restyle_hints::RestyleHint;

    use crate::test_common::Doc;

    fn root_hint(doc: &Doc) -> RestyleHint {
        doc.dom
            .get(doc.root)
            .expect("the document element")
            .style_data_wrapper()
            .map_or_else(RestyleHint::empty, |wrapper| wrapper.borrow().hint)
    }

    #[test]
    fn a_resize_mark_on_the_root_survives_an_animation_tick() {
        let mut doc = Doc::with_css(
            "@keyframes slide { from { transform: translateX(0px) } \
             to { transform: translateX(100px) } } \
             page { font-size: 5vw } \
             .mover { animation: slide 10s linear; width: 20px; height: 20px }",
        );
        let root = doc.root;
        doc.el(root, "view.mover");
        doc.dom.layout();
        doc.dom.advance_animations(0.0);

        doc.dom.set_viewport(400.0, 600.0);
        let marked = root_hint(&doc);
        assert!(
            marked.intersects(RestyleHint::RESTYLE_SELF | RestyleHint::RECASCADE_SELF),
            "the resize marks the element it lands on, not only its descendants"
        );

        doc.dom.advance_animations(5.0);

        assert!(
            root_hint(&doc).intersects(RestyleHint::RESTYLE_SELF | RestyleHint::RECASCADE_SELF),
            "an animation tick must not strip the resize mark before the flush \
             that reads it"
        );
    }

    fn schedules_a_flush(doc: &Doc) -> bool {
        doc.dom.document_element().needs_style_flush()
    }

    #[test]
    fn an_attribute_no_rule_mentions_schedules_no_flush() {
        let mut doc = Doc::with_css("view[data-on] { color: rgb(255, 0, 0) }");
        let root = doc.root;
        let el = doc.el(root, "view");
        doc.dom.layout();
        assert!(!schedules_a_flush(&doc), "styled and settled");

        doc.dom.set_attribute(el, "data-unmentioned", "1");
        assert!(
            !schedules_a_flush(&doc),
            "no rule names this attribute, so nothing has to be restyled"
        );

        doc.dom.set_attribute(el, "data-on", "1");
        assert!(
            schedules_a_flush(&doc),
            "a rule does name this one, so the restyle is still scheduled"
        );
    }

    #[test]
    fn an_element_state_no_rule_selects_on_schedules_no_flush() {
        let mut doc = Doc::with_css("view:hover { color: rgb(255, 0, 0) }");
        let root = doc.root;
        let el = doc.el(root, "view");
        doc.dom.layout();

        doc.dom
            .add_element_state(el, stylo_dom::ElementState::FOCUS);
        assert!(
            !schedules_a_flush(&doc),
            "nothing selects on :focus in this document"
        );

        doc.dom
            .add_element_state(el, stylo_dom::ElementState::HOVER);
        assert!(schedules_a_flush(&doc), ":hover is selected on");
    }

    #[test]
    fn an_inline_style_write_always_schedules_a_flush() {
        let mut doc = Doc::with_css("view { color: rgb(0, 0, 255) }");
        let root = doc.root;
        let el = doc.el(root, "view");
        doc.dom.layout();
        assert!(!schedules_a_flush(&doc));

        doc.dom.set_inline_style(el, "color: rgb(255, 0, 0)");
        assert!(
            schedules_a_flush(&doc),
            "no rule mentions [style], and the write still has to be flushed"
        );
    }

    #[test]
    fn attributes_matched_without_an_attribute_selector_are_never_gated() {
        for attribute in ["lang", "part", "exportparts", "style"] {
            let mut doc = Doc::with_css("view { color: rgb(0, 0, 255) }");
            let root = doc.root;
            let el = doc.el(root, "view");
            doc.dom.layout();
            assert!(!schedules_a_flush(&doc), "{attribute}: styled and settled");

            doc.dom.set_attribute(el, attribute, "en");
            assert!(
                schedules_a_flush(&doc),
                "{attribute} is matched on without an attribute selector naming \
                 it, so it cannot be gated on one"
            );
        }
    }

    #[test]
    fn a_shadow_tree_elsewhere_does_not_stop_the_gate() {
        let mut doc = Doc::with_css("view { color: rgb(0, 0, 255) }");
        let root = doc.root;
        let host = doc.el(root, "host");
        let shadow = doc
            .dom
            .attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
        doc.dom
            .add_shadow_stylesheet(shadow, "view[data-on] { color: rgb(255, 0, 0) }");
        let outside = doc.el(root, "view");
        doc.dom.layout();
        assert!(!schedules_a_flush(&doc));

        doc.dom.set_attribute(outside, "data-on", "1");
        assert!(
            !schedules_a_flush(&doc),
            "the rule naming this attribute is scoped to a tree this element is \
             not in, so it still cannot match"
        );
    }

    #[test]
    fn a_scoped_rule_gates_nothing_inside_its_own_tree() {
        let mut doc = Doc::with_css("view { color: rgb(0, 0, 255) }");
        let root = doc.root;
        let host = doc.el(root, "host");
        let shadow = doc
            .dom
            .attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
        doc.dom
            .add_shadow_stylesheet(shadow, "view[data-on] { color: rgb(255, 0, 0) }");
        let inside = doc.el(shadow, "view");
        doc.dom.layout();
        assert!(!schedules_a_flush(&doc));

        doc.dom.set_attribute(inside, "data-on", "1");
        assert!(
            schedules_a_flush(&doc),
            "the element lives in the tree the rule is scoped to"
        );
    }

    #[test]
    fn a_host_rule_gates_nothing_on_its_host() {
        let mut doc = Doc::with_css("host { color: rgb(0, 0, 255) }");
        let root = doc.root;
        let host = doc.el(root, "host");
        let shadow = doc
            .dom
            .attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
        doc.dom
            .add_shadow_stylesheet(shadow, ":host([data-on]) { color: rgb(255, 0, 0) }");
        doc.dom.layout();
        assert!(!schedules_a_flush(&doc));

        doc.dom.set_attribute(host, "data-on", "1");
        assert!(
            schedules_a_flush(&doc),
            "a :host rule selects the element that hosts the tree it is in"
        );
    }

    #[test]
    fn a_slotted_rule_gates_nothing_on_what_it_slots() {
        let mut doc = Doc::with_css("view { color: rgb(0, 0, 255) }");
        let root = doc.root;
        let host = doc.el(root, "host");
        let shadow = doc
            .dom
            .attach_shadow(host, crate::tree::shadow::ShadowRootMode::Open);
        doc.el(shadow, "slot");
        doc.dom
            .add_shadow_stylesheet(shadow, "::slotted([data-on]) { color: rgb(255, 0, 0) }");
        let slotted = doc.el(host, "view");
        doc.dom.layout();
        assert!(!schedules_a_flush(&doc));

        doc.dom.set_attribute(slotted, "data-on", "1");
        assert!(
            schedules_a_flush(&doc),
            "the element is assigned to a slot in the tree that scopes the rule"
        );
    }

    #[test]
    fn a_resize_that_changes_no_media_answer_only_recascades() {
        let mut doc = Doc::with_css("page { font-size: 5vw } view { width: 10px }");
        let root = doc.root;
        doc.el(root, "view");
        doc.dom.layout();

        doc.dom.set_viewport(400.0, 600.0);

        let hint = root_hint(&doc);
        assert!(
            !hint.contains(RestyleHint::RESTYLE_DESCENDANTS),
            "no media answer moved, so no descendant needs matching again"
        );
        assert!(
            hint.contains(RestyleHint::RECASCADE_DESCENDANTS),
            "but every descendant still has to be cascaded against the new device"
        );
    }

    #[test]
    fn a_resize_that_flips_a_media_answer_rematches() {
        let mut doc = Doc::with_css(
            "@media (min-width: 700px) { view { width: 30px } } view { width: 10px }",
        );
        let root = doc.root;
        doc.el(root, "view");
        doc.dom.layout();

        doc.dom.set_viewport(400.0, 600.0);

        assert!(
            root_hint(&doc).contains(RestyleHint::RESTYLE_DESCENDANTS),
            "crossing the breakpoint changes which rules apply"
        );
    }

    fn inline_block(
        doc: &Doc,
        id: crate::tree::document::NodeId,
    ) -> Option<*const std::ffi::c_void> {
        doc.dom
            .get(id)
            .expect("a live element")
            .parsed_inline_style
            .as_ref()
            .map(stylo::servo_arc::Arc::heap_ptr)
    }

    #[test]
    fn an_inline_property_write_copies_only_the_block_the_rule_tree_holds() {
        let mut doc = Doc::new();
        let root = doc.root;
        let el = doc.el(root, "view");
        doc.dom.set_inline_style_property(el, "width", "10px");
        let first = inline_block(&doc, el);
        doc.dom.set_inline_style_property(el, "height", "20px");
        assert_eq!(inline_block(&doc, el), first, "nothing holds it yet");
        doc.flush();

        doc.dom.set_inline_style_property(el, "width", "30px");
        let copy = inline_block(&doc, el);
        assert_ne!(copy, first, "the rule tree holds the flushed block");
        assert_eq!(
            doc.value(el, "width"),
            "10px",
            "the cascade's block is as it was"
        );
        doc.dom.set_inline_style_property(el, "height", "40px");
        assert_eq!(inline_block(&doc, el), copy, "nothing holds the copy yet");

        doc.flush();
        assert_eq!(doc.value(el, "width"), "30px");
        assert_eq!(doc.value(el, "height"), "40px");
        assert_eq!(
            doc.dom.get(el).unwrap().attribute("style").as_deref(),
            Some("width: 30px; height: 40px;")
        );
    }

    #[test]
    fn a_style_attribute_selector_sees_a_property_write() {
        let mut doc = Doc::with_css(
            "view { color: rgb(0, 0, 255) } \
             view[style*=\"30px\"] { color: rgb(255, 0, 0) }",
        );
        let root = doc.root;
        let el = doc.el(root, "view");
        doc.dom.set_inline_style_property(el, "width", "10px");
        doc.flush();
        assert_eq!(doc.color(el), crate::test_common::rgb(0, 0, 255));

        doc.dom.set_inline_style_property(el, "width", "30px");
        doc.flush();
        assert_eq!(doc.color(el), crate::test_common::rgb(255, 0, 0));

        doc.dom.set_inline_style_property(el, "width", "");
        doc.flush();
        assert_eq!(doc.color(el), crate::test_common::rgb(0, 0, 255));
        assert_eq!(
            doc.dom.get(el).unwrap().attribute("style").as_deref(),
            Some("")
        );
    }

    #[test]
    fn class_text_reads_back_as_written_or_as_the_class_list() {
        let mut doc = Doc::new();
        let root = doc.root;
        let el = doc.el(root, "view");
        let class = |doc: &Doc| {
            doc.dom
                .get(el)
                .unwrap()
                .attribute("class")
                .map(std::borrow::Cow::into_owned)
        };
        doc.dom.set_classes(el, " a  b ");
        assert_eq!(class(&doc).as_deref(), Some(" a  b "));
        doc.dom.set_classes(el, "a b");
        assert_eq!(class(&doc).as_deref(), Some("a b"));
        doc.dom.add_class(el, "c");
        assert_eq!(class(&doc).as_deref(), Some("a b c"));
        for name in ["a", "b", "c"] {
            doc.dom.remove_class(el, name);
        }
        assert_eq!(class(&doc).as_deref(), Some(""));
        doc.dom.set_classes(el, "");
        assert_eq!(class(&doc).as_deref(), Some(""));
    }
}
