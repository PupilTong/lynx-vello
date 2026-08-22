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
use stylo::stylist::CascadeData;
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

    /// Marks a subtree to be cascaded again without being matched again.
    ///
    /// Every descendant cascades and none is matched: Stylo propagates
    /// `RECASCADE_DESCENDANTS` down as `recascade_subtree()`, transitively. The
    /// element the mark lands on is the only one matched again, which is one
    /// element against the whole tree.
    ///
    /// That element's share is spelled `RESTYLE_SELF` rather than the
    /// `RECASCADE_SELF` that `RestyleHint::recascade_subtree()` would pair
    /// here, because `RECASCADE_SELF` does not survive until the flush that
    /// should read it. It is inside
    /// `RestyleHint::has_animation_hint_or_recascade`'s mask, and
    /// `remove_animation_hints` deletes it outright, on the stated assumption
    /// that only a traversal ever sets it — so an animation tick between this
    /// mark and the next style flush strips it. Measured directly: with
    /// `recascade_subtree()` the mark reads `RECASCADE_SELF |
    /// RECASCADE_DESCENDANTS` after a resize and `RECASCADE_DESCENDANTS` alone
    /// after one `advance_animations`, while the spelling below survives a tick
    /// unchanged. No stale rendering was reproduced from the loss — something
    /// downstream evidently recomputes the root anyway — so this is avoiding a
    /// dependency on that, not fixing an observed defect.
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
            // `HAS_SLOW_SELECTOR` restyles every child; its later-siblings
            // counterpart restyles the suffix that starts at the mutated
            // position, which is every child when that position is the first.
            let restyle_from = if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR) {
                Some(0)
            } else if flags.intersects(ElementSelectorFlags::HAS_SLOW_SELECTOR_LATER_SIBLINGS) {
                Some(index)
            } else {
                None
            };
            if let Some(from) = restyle_from {
                // Restyling every element child is what `RESTYLE_DESCENDANTS`
                // on the parent already denotes: Stylo propagates it to each
                // child as `restyle_subtree()` — the hint the walk below would
                // write on each of them — and recurses identically, so the set
                // of restyled elements is the same one, not a larger one.
                // Leaving `RESTYLE_SELF` off is what keeps the parent itself
                // out of it.
                //
                // Two things have to hold for the one write to stand in for
                // the walk. Stylo propagates along the *flat* tree, which is
                // the child list only while nothing redirects it, and exactly
                // two things redirect it: a host, whose flat children are its
                // shadow root's children, and a slot with assigned nodes. A
                // shadow root parent also holds no `ElementData` for the hint
                // to land in.
                //
                // Both are asked of the parent itself rather than of the
                // document, because a page whose components are custom
                // elements holds shadow roots everywhere and a document-wide
                // answer would give the collapse up on all of them. Neither
                // property can turn on between here and the traversal that
                // reads the hint: only a slot ever gains assigned nodes, and
                // `attach_shadow` marks its host's whole subtree dirty, which
                // is the stronger hint. The fallback is still the
                // allocation-free walk, so it costs a walk, not a copy.
                let parent_node = self.live(parent);
                let flat_children_are_dom_children = !self.has_shadow_roots()
                    || (parent_node.shadow_root_id().is_none() && !parent_node.is_slot());
                let ancestor_hint_carries =
                    from == 0 && flat_children_are_dom_children && parent_node.has_style_data();
                if ancestor_hint_carries {
                    self.add_restyle_hint(parent, RestyleHint::RESTYLE_DESCENDANTS);
                } else {
                    // Hinting a child never edits a child list, so each step
                    // re-reads the parent's rather than holding a copy across
                    // the `&mut self` calls. A copy would be proportional to
                    // the sibling count on *every* insertion, which is what
                    // made filling a list quadratic.
                    let mut position = from;
                    while let Some(&child) = self.live(parent).child_ids().get(position) {
                        if self.live(child).is_element() {
                            self.add_restyle_hint(child, RestyleHint::restyle_subtree());
                        }
                        position += 1;
                    }
                }
            } else if flags.intersects(ElementSelectorFlags::MAY_HAVE_TREE_COUNTING_FUNCTION) {
                // No ancestor hint denotes this set. `RECASCADE_DESCENDANTS`
                // reaches the whole subtree, while `sibling-index()` and
                // `sibling-count()` only change for the children themselves, so
                // the walk stays.
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

    /// The first two and last two element children, which are every child an
    /// edge-child selector (`:first-child`, `:last-child`, `:only-child`, and
    /// their `-of-type` forms) can start or stop matching when one child is
    /// added or removed. Non-element children are skipped, so the scan reaches
    /// past leading and trailing text nodes but stops at the second element
    /// from each end.
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
        // `:empty` turning on or off on this element moves what every later
        // sibling matches. The walk indexes the parent's list a step at a time
        // rather than collecting it, for the reason
        // `note_child_list_change` does: hinting a sibling cannot edit that
        // list, and this runs on every child-list change of the element.
        let Some(after) = self.sibling_position(id).map(|position| position + 1) else {
            return;
        };
        let mut position = after;
        while let Some(sibling) = self.sibling_at(id, position) {
            self.add_restyle_hint(sibling, RestyleHint::restyle_subtree());
            position += 1;
        }
    }

    /// This node's index in its parent's child list.
    fn sibling_position(&self, id: NodeId) -> Option<usize> {
        let tree = self.arenas();
        let node = tree.get(id)?;
        let self_slot = tree.slot(id)?;
        let siblings = tree.at(node.parent_slot()?).child_slots();
        siblings.iter().position(|&slot| slot == self_slot)
    }

    /// The `position`th child of `id`'s parent, if there is one.
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
        // Element state is element-only, and the gate below can skip the
        // `ensure_snapshot` that used to carry this check.
        self.live_element(id);
        if self.any_rule_depends_on_state(id, flags) {
            self.ensure_snapshot(id);
            self.mark_ancestors_dirty_descendants(id);
        } else {
            // Nothing can match on these bits, so no restyle is scheduled. The
            // state still lands on the node, and a stylesheet mounted later
            // restyles from the root, which covers every element this skipped.
            self.note_visual_mutation();
        }
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

    /// Sets one property in an element's inline declaration block.
    ///
    /// This has the mutation semantics of
    /// [`CSSStyleDeclaration.setProperty`](https://drafts.csswg.org/cssom/#dom-cssstyledeclaration-setproperty):
    /// an empty value removes that property, while an unknown property or an
    /// invalid value leaves the declaration block unchanged. The property name
    /// is CSS syntax (for example `background-color` or `--theme-color`), not a
    /// Rust or wire-format property id.
    pub fn set_inline_style_property(&mut self, id: NodeId, property: &str, value: &str) {
        let existing = self.live_element(id).parsed_inline_style.clone();
        let Some((block, css)) =
            self.style_engine()
                .update_inline_style_property(existing.as_ref(), property, value)
        else {
            return;
        };

        let base = self.begin_reactions();
        self.enqueue_attribute_changed(id, &STYLE, Some(&css));
        self.apply_inline_style_block(id, block, Some(css));
        self.drain_reactions(base);
    }

    /// Replaces an element's whole inline declaration block with a record of
    /// property/value pairs.
    ///
    /// This is the block-level setter — the one whose semantics are
    /// *replacement*, like assigning `style.cssText`, rather than the
    /// property-level mutation of [`Self::set_inline_style_property`]. The
    /// block is built from empty, so applying an `n`-declaration record costs
    /// one parse per declaration and one serialization, where replaying the
    /// record through the property-level setter costs `n` whole-block clones
    /// and `n` serializations.
    ///
    /// Each declaration keeps the property-level setter's parse semantics: an
    /// unknown property name or an invalid value drops that declaration and
    /// leaves the rest of the record alone. It is deliberately *not* the same
    /// as joining the record into style-attribute text and parsing that — a
    /// value containing a `;` would there start a second declaration instead
    /// of being rejected.
    ///
    /// An empty record leaves an empty `style` attribute rather than removing
    /// it, which is what assigning an empty declaration block does.
    pub fn set_inline_style_declarations<'a>(
        &mut self,
        id: NodeId,
        declarations: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) {
        let (block, css) = self.style_engine().build_inline_style_block(declarations);
        let base = self.begin_reactions();
        self.enqueue_attribute_changed(id, &STYLE, Some(&css));
        self.apply_inline_style_block(id, block, Some(css));
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
        // Attributes are an element-only concept. The check used to ride on
        // `ensure_snapshot`, which the gate below can skip, so it is taken here
        // where it happens on every path.
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

    /// Whether any rule that can match this element selects on this attribute.
    ///
    /// Stylo records, per origin, every attribute name that appears in an
    /// attribute selector. The record is a superset of what a plain selector
    /// needs: `visit_attribute_selector` inserts into `attribute_dependencies`
    /// unconditionally and only *adds* the `:nth-child(... of S)` entry on top,
    /// and a relative selector list — `:has()` — is walked into by
    /// `visit_relative_selector_list`, so its attribute selectors land in the
    /// same set. A name absent from every origin therefore cannot start or stop
    /// any selector matching, and the snapshot the invalidator would take of it
    /// would have no reader.
    ///
    /// `class`, `id`, and `style` never reach this: they carry their own
    /// invalidation, which does not depend on a rule naming them as attributes.
    ///
    /// The sets are read at mutation time, so a rule arriving afterwards would
    /// have missed the write. What makes that sound is that every rule-set
    /// addition re-matches from the root — `Document::change_style_rules` and
    /// `add_shadow_stylesheet` both call `mark_subtree_dirty`, and a device
    /// change that moves a media answer takes the same path. Weakening any of
    /// those to something narrower would strand every write this gate skipped;
    /// `an_attribute_no_rule_mentions_is_matched_by_a_stylesheet_added_later`
    /// in tests/style.rs is what holds that down.
    fn any_rule_depends_on_attribute(&self, id: NodeId, name: &LocalName) -> bool {
        self.any_applicable_cascade_data(id, &mut |data| data.might_have_attribute_dependency(name))
    }

    /// Whether any rule that can match this element selects on these state
    /// bits.
    ///
    /// The same superset argument as [`Self::any_rule_depends_on_attribute`]:
    /// `state_dependencies` takes every `NonTSPseudoClass`'s state flag
    /// unconditionally, with the `:nth-child(... of S)` set recorded separately
    /// on top rather than instead.
    fn any_rule_depends_on_state(&self, id: NodeId, state: stylo_dom::ElementState) -> bool {
        self.any_applicable_cascade_data(id, &mut |data| data.has_state_dependency(state))
    }

    /// Runs `test` over every rule set whose selectors can match this element,
    /// stopping at the first that answers yes.
    ///
    /// `Stylist::iter_origins` reaches the document's three origins. It does
    /// not reach the per-shadow-root `CascadeData` a scoped stylesheet builds,
    /// and answering "is there a shadow root anywhere" instead would give up
    /// the gate entirely on a page whose components are custom elements with
    /// shadow trees — which is where this is going. So the scoped sets that can
    /// reach *this* element are visited, and only those.
    ///
    /// This mirrors `TElement::each_applicable_non_document_style_rule_data`
    /// rather than calling it: that is a `TElement` method, and the `TElement`
    /// impl for `&Node<T>` requires `T: Sync`, which the mutation API does not
    /// ask of its callers. Two deliberate differences, both widening the set
    /// and so both safe: the document's author origin is tested even for an
    /// element inside a shadow tree, where Stylo would skip it; and an element
    /// carrying a `part` attribute answers yes outright instead of walking the
    /// `::part()` chain outwards.
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
        // No scoped rule set exists at all, which is the whole answer.
        if !self.has_shadow_roots() {
            return false;
        }
        let node = self.live(id);
        // The tree this element lives in.
        if let Some(data) = scoped_data(node.containing_shadow_root())
            && test(data)
        {
            return true;
        }
        // The tree it hosts, whose `:host` rules match it.
        if let Some(data) = scoped_data(node.shadow_root_id().map(|root| self.live(root)))
            && test(data)
        {
            return true;
        }
        // Every slot it is assigned to, outwards, for `::slotted()`.
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
        // `::part()` reaches outwards through trees this walk does not follow.
        // Parts are rare; answering yes costs one snapshot.
        node.has_part_attr()
    }

    fn ensure_snapshot(&mut self, id: NodeId) -> Option<&mut Snapshot> {
        self.note_visual_mutation();
        if !self.live_element(id).has_style_data() {
            return None;
        }
        let opaque = OpaqueNode(id.arena_key());
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

/// Attributes whose invalidation is not recorded in
/// `CascadeData::attribute_dependencies`, and which therefore cannot be gated
/// on it.
///
/// The gate asks Stylo whether an attribute selector anywhere names this
/// attribute. Four attributes matter to matching without ever being named by
/// one:
///
/// - `style` carries declarations, not matching. Its flush is scheduled by the
///   `RESTYLE_STYLE_ATTRIBUTE` hint plus the ancestor mark, never by a rule mentioning `[style]`.
/// - `lang` is what `:lang()` reads. Stylo files that dependency into the `InvalidationMap` from
///   `on_pseudo_class`, under the attribute name — a different structure from the
///   `attribute_dependencies` set this gate reads, which only `visit_attribute_selector` fills.
/// - `part` and `exportparts` drive `::part()` matching through
///   `TElement::has_part_attr`/`imported_part`, again with no attribute selector involved.
///
/// Only `style` is load-bearing today. `::part()` reaches an element only through a containing
/// shadow root, and a document that has one already answers the gate conservatively; `:lang()`
/// cannot match anything while `TElement::match_element_lang` returns false. The other three are
/// listed so that closing either of those gaps does not quietly make the gate wrong.
/// The rule set a shadow root scopes, if it holds one.
fn scoped_data<T>(root: Option<&Node<T>>) -> Option<&CascadeData> {
    Some(&root?.shadow_data()?.styles.data)
}

fn is_gate_exempt(name: &LocalName) -> bool {
    *name == *STYLE || *name == *LANG || *name == *PART || *name == *EXPORTPARTS
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

    /// The mark a resize leaves on the document element has to still be there
    /// at the next style flush. An animation tick runs between the two on the
    /// presenting thread, and it strips `RECASCADE_SELF` from every hint it
    /// walks past — see [`Document::mark_subtree_recascade`].
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

    /// The flush root only reports work when something scheduled it. This is
    /// what the attribute and state gates actually change: a write nothing can
    /// match on leaves the tree clean, so the flush returns without traversing.
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

    /// The style attribute is exempt from the gate. Its invalidation does not
    /// come from a rule naming `[style]` — it comes from the declaration block
    /// changing — so gating it on rule dependencies would strand the write.
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

    /// `:lang()` reads the `lang` attribute, but Stylo files that dependency in
    /// the `InvalidationMap` rather than in the `attribute_dependencies` set the
    /// gate reads — so `lang` is exempt rather than gated. `part` and
    /// `exportparts` are exempt for the same shape of reason: `::part()` matches
    /// through them without any attribute selector naming them.
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

    /// A scoped rule set reaches only the tree it is scoped to, so a shadow
    /// root elsewhere in the document must not cost the gate anything. This is
    /// the case that matters once Lynx elements are custom elements: every page
    /// would hold shadow roots, and a document-wide bail-out would leave the
    /// gate permanently off.
    #[test]
    fn a_shadow_tree_elsewhere_does_not_stop_the_gate() {
        let mut doc = Doc::with_css("view { color: rgb(0, 0, 255) }");
        let root = doc.root;
        let host = doc.el(root, "host");
        let shadow = doc.dom.attach_shadow(host, crate::ShadowRootMode::Open);
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

    /// Inside the tree that scopes the rule, the same write must not be gated.
    #[test]
    fn a_scoped_rule_gates_nothing_inside_its_own_tree() {
        let mut doc = Doc::with_css("view { color: rgb(0, 0, 255) }");
        let root = doc.root;
        let host = doc.el(root, "host");
        let shadow = doc.dom.attach_shadow(host, crate::ShadowRootMode::Open);
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

    /// `:host()` selects the host from inside the tree it hosts, and the host
    /// itself is in the light DOM — so the tree it hosts has to be consulted
    /// too, not just the tree it lives in.
    #[test]
    fn a_host_rule_gates_nothing_on_its_host() {
        let mut doc = Doc::with_css("host { color: rgb(0, 0, 255) }");
        let root = doc.root;
        let host = doc.el(root, "host");
        let shadow = doc.dom.attach_shadow(host, crate::ShadowRootMode::Open);
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

    /// `::slotted()` selects light children through the slot they are assigned
    /// to, which is a tree the element does not live in either.
    #[test]
    fn a_slotted_rule_gates_nothing_on_what_it_slots() {
        let mut doc = Doc::with_css("view { color: rgb(0, 0, 255) }");
        let root = doc.root;
        let host = doc.el(root, "host");
        let shadow = doc.dom.attach_shadow(host, crate::ShadowRootMode::Open);
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

    /// A resize with no media query flipping must not re-run selector
    /// matching on the tree, only re-cascade it.
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

    /// A resize that does flip a media query has to re-match: a rule that was
    /// not applying before can start to.
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
}
