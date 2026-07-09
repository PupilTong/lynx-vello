//! [`selectors::Element`] for [`ElemRef`].
//!
//! id/class matching is **case-sensitive** (Lynx authors selectors that way);
//! `:hover`/`:active`/`:focus` are matched from the element's
//! [`ElementState`](stylo_dom::ElementState); attribute matching covers the
//! node's real attributes plus the synthetic `l-css-id` (see
//! [`TElement::get_attr`](stylo::dom::TElement::get_attr)).

use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::{BLOOM_HASH_MASK, BloomFilter};
use selectors::context::MatchingContext;
use selectors::matching::ElementSelectorFlags;
use selectors::{Element, OpaqueElement};
use stylo::selector_parser::{NonTSPseudoClass, PseudoElement, SelectorImpl};
use stylo::values::{AtomIdent, AtomString};
use stylo::{CaseSensitivityExt, LocalName, Namespace};

use crate::arena::ElemRef;
use crate::tag::NodeKind;

impl Element for ElemRef<'_> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.node())
    }

    fn parent_element(&self) -> Option<Self> {
        self.parent()
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        // Every node is an element, so the immediate previous sibling is it.
        self.prev_sibling()
    }

    fn next_sibling_element(&self) -> Option<Self> {
        self.next_sibling()
    }

    fn first_element_child(&self) -> Option<Self> {
        self.first_child()
    }

    fn is_html_element_in_html_document(&self) -> bool {
        true
    }

    fn has_local_name(
        &self,
        local_name: &<Self::Impl as selectors::SelectorImpl>::BorrowedLocalName,
    ) -> bool {
        self.node().tag.0 == *local_name
    }

    fn has_namespace(
        &self,
        ns: &<Self::Impl as selectors::SelectorImpl>::BorrowedNamespaceUrl,
    ) -> bool {
        // Lynx elements are never namespaced: only the empty namespace matches.
        ns.is_empty()
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.node().tag == other.node().tag
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&Namespace>,
        local_name: &LocalName,
        operation: &AttrSelectorOperation<&AtomString>,
    ) -> bool {
        let name: &str = local_name.0.as_ref();
        if name == "l-css-id" {
            return operation.eval_str(&self.node().css_id.to_string());
        }
        if let Some(value) = self.node().attrs.get(name) {
            return operation.eval_str(value);
        }
        // web-core reflects dataset entries as `data-*` attributes (DOM
        // dataset reflection), so `[data-x]` selectors must see them too.
        // Keys are matched verbatim after the `data-` prefix; camelCase↔kebab
        // reflection is revisited with StyleInfo ingestion (M3) if ReactLynx
        // turns out to emit camelCase dataset keys.
        if let Some(key) = name.strip_prefix("data-") {
            return self
                .node()
                .dataset
                .get(key)
                .is_some_and(|value| operation.eval_str(value));
        }
        false
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &NonTSPseudoClass,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        // Match the dynamic Lynx pseudo-classes against the element's state.
        // Every other non-tree-structural pseudo-class is unsupported → false.
        match pc {
            NonTSPseudoClass::Hover | NonTSPseudoClass::Active | NonTSPseudoClass::Focus => {
                self.node().element_state.contains(pc.state_flag())
            }
            _ => false,
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &PseudoElement,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        // stylo's contract splits the flags: `for_self()` bits land on this
        // element, `for_parent()` bits (slow-selector / edge-child markers) on
        // its parent.
        let self_flags = flags.for_self();
        if !self_flags.is_empty() {
            self.node().selector_flags.borrow_mut().insert(self_flags);
        }
        let parent_flags = flags.for_parent();
        if !parent_flags.is_empty()
            && let Some(parent) = self.parent()
        {
            parent
                .node()
                .selector_flags
                .borrow_mut()
                .insert(parent_flags);
        }
    }

    fn is_link(&self) -> bool {
        false
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.node()
            .id_attr
            .as_ref()
            .is_some_and(|my_id| case_sensitivity.eq_atom(my_id, id))
    }

    fn has_class(&self, name: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.node()
            .classes
            .iter()
            .any(|class| case_sensitivity.eq_atom(class, name))
    }

    fn has_custom_state(&self, _name: &AtomIdent) -> bool {
        false
    }

    fn imported_part(&self, _name: &AtomIdent) -> Option<AtomIdent> {
        None
    }

    fn is_part(&self, _name: &AtomIdent) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        // Non-empty if it has any child element, or a `<raw-text>` child with
        // non-empty text content.
        self.children().next().is_none() && self.node().text.as_ref().is_none_or(String::is_empty)
    }

    fn is_root(&self) -> bool {
        // `:root` is exactly the `<page>` element (web-core rewrites `:root`
        // to the page part). Checking the kind — not parentlessness — keeps a
        // detached subtree's root from matching `:root` during resolve.
        self.node().kind == NodeKind::Page && self.parent().is_none()
    }

    fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool {
        stylo::bloom::each_relevant_element_hash(*self, |hash| {
            filter.insert_hash(hash & BLOOM_HASH_MASK);
        });
        true
    }
}
