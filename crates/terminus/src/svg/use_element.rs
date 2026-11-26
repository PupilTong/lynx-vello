//! SVG's `use` element (SVG 2 §5.6): a closed shadow root holding a deep
//! clone of the element its `href` names, as Gecko
//! (`dom/svg/SVGUseElement.cpp`, `UpdateShadowTree`) and Chromium
//! (`core/svg/svg_use_element.cc`, `CreateInstanceTree`) build it.
//!
//! - The `href` (or `xlink:href`, [`href`]) is fragment-only (`#id`) and resolves among the
//!   elements the markup built — the originals, never a clone — so an `id` inside an instance never
//!   shadows the element it was cloned from. The document's own id lookup (`#id` in
//!   `query_selector`) walks the light tree and never enters a shadow root, so it does not see the
//!   clones either.
//! - A clone is made through the SVG-namespace creation path and `set_attribute`, so its
//!   presentation attributes are hints as the original's are. Text nodes are cloned; shadow roots
//!   are not (a `use` inside the clone gets its own instance).
//! - A `use` whose expansion reaches a reference cycle — its target containing it, or a chain of
//!   `use`s that comes back to a target already being expanded — instantiates nothing, as both
//!   browsers refuse a circular reference; an unresolvable or non-fragment `href` instantiates
//!   nothing either.
//! - The expansion is a level of its own, so instances nested in instances count against
//!   [`MAX_NESTING`] like nested elements; elements past it are not cloned. One build clones at
//!   most [`MAX_INSTANCE_NODES`] nodes, because a chain of `use`s each naming the previous one
//!   several times grows exponentially with the chain's length.
//!
//! The instance inherits from the `use` element (the flat tree runs through
//! the shadow root), and the document's rules match it: stylo skips a
//! shadow root whose host is an SVG `use` when it looks for the tree whose
//! rules apply (`containing_shadow_ignoring_svg_use`), which needs
//! `is_svg_element` and the local name of the host, and asserts that the
//! root has no style data, which `TShadowRoot::style_data` answers for a
//! `use` host's root (`style/traits.rs`) from `ShadowRootData`'s
//! `svg_use_instance`, set once in `attach_shadow`.
//!
//! No re-instantiation on a later mutation of the `use`, its target or the
//! target's subtree.

use rustc_hash::{FxHashMap, FxHashSet};

use super::attributes::href;
use super::gradient::fragment_id;
use super::{USE_TAG, is_use};
use crate::tree::document::{Document, NodeId};
use crate::tree::shadow::ShadowRootMode;
use crate::tree::xml::MAX_NESTING;

pub(crate) const MAX_INSTANCE_NODES: usize = 1 << 16;

struct References<'a> {
    ids: &'a FxHashMap<&'a str, NodeId>,
    acyclic: FxHashSet<NodeId>,
}

impl<T> Document<T> {
    pub(crate) fn instantiate_uses(
        &mut self,
        uses: &[(NodeId, u32)],
        ids: &FxHashMap<&str, NodeId>,
        payload: &mut impl FnMut() -> T,
    ) {
        let mut references = References {
            ids,
            acyclic: FxHashSet::default(),
        };
        let mut budget = MAX_INSTANCE_NODES;
        for &(element, depth) in uses {
            if self.reaches_cycle(element, &mut references, &mut Vec::new()) {
                continue;
            }
            self.instantiate(element, depth, &references, payload, &mut budget);
        }
    }

    fn use_target(&self, element: NodeId, references: &References<'_>) -> Option<NodeId> {
        let href = href(self.live(element))?;
        references.ids.get(fragment_id(href)?).copied()
    }

    fn reaches_cycle(
        &self,
        element: NodeId,
        references: &mut References<'_>,
        expanding: &mut Vec<NodeId>,
    ) -> bool {
        let Some(target) = self.use_target(element, references) else {
            return false;
        };
        if references.acyclic.contains(&target) {
            return false;
        }
        if target == element || self.is_ancestor(target, element) || expanding.contains(&target) {
            return true;
        }
        expanding.push(target);
        let mut uses = Vec::new();
        self.collect_uses(target, &mut uses);
        let cycle = uses
            .into_iter()
            .any(|inner| self.reaches_cycle(inner, references, expanding));
        expanding.pop();
        if !cycle {
            references.acyclic.insert(target);
        }
        cycle
    }

    fn collect_uses(&self, root: NodeId, out: &mut Vec<NodeId>) {
        let node = self.live(root);
        if is_use(node) {
            out.push(root);
        }
        for &child in node.child_ids() {
            self.collect_uses(child, out);
        }
    }

    fn instantiate(
        &mut self,
        element: NodeId,
        depth: u32,
        references: &References<'_>,
        payload: &mut impl FnMut() -> T,
        budget: &mut usize,
    ) {
        if depth >= MAX_NESTING {
            return;
        }
        let Some(target) = self.use_target(element, references) else {
            return;
        };
        let shadow = self.attach_shadow(element, ShadowRootMode::Closed);
        let mut nested = Vec::new();
        self.clone_into(target, shadow, depth + 1, payload, budget, &mut nested);
        for (inner, inner_depth) in nested {
            self.instantiate(inner, inner_depth, references, payload, budget);
        }
    }

    fn clone_into(
        &mut self,
        source: NodeId,
        parent: NodeId,
        depth: u32,
        payload: &mut impl FnMut() -> T,
        budget: &mut usize,
        nested: &mut Vec<(NodeId, u32)>,
    ) {
        if *budget == 0 {
            return;
        }
        let node = self.live(source);
        if let Some(text) = node.text().filter(|_| node.is_text_node()) {
            *budget -= 1;
            let text = text.to_owned();
            let clone = self.create_text_node(text, payload());
            self.append_child(parent, clone);
            return;
        }
        if !node.is_element() || depth > MAX_NESTING {
            return;
        }
        *budget -= 1;
        let local_name = node
            .tag_name()
            .expect("an element has a local name")
            .to_owned();
        let attributes: Vec<(String, String)> = node
            .attributes()
            .map(|(name, value)| (name.to_owned(), value.into_owned()))
            .collect();
        let children = node.child_ids().to_vec();
        let clone = self.create_svg_namespace_element(&local_name, payload());
        for (name, value) in &attributes {
            self.set_attribute(clone, name, value);
        }
        self.append_child(parent, clone);
        if local_name == USE_TAG {
            nested.push((clone, depth));
        }
        for child in children {
            self.clone_into(child, clone, depth + 1, payload, budget, nested);
        }
    }
}
