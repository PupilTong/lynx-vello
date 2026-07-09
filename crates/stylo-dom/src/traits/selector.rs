//! [`selectors::Element`] for [`ElementRef`].
//!
//! id/class matching is **case-sensitive**; `:hover`/`:active`/`:focus` are
//! matched from the element's [`ElementState`](crate::ElementState); attribute
//! matching covers the element's real attributes plus whatever synthetic /
//! reflected attributes the embedder's
//! [`ExternalState`](crate::ExternalState) hooks serve (see
//! [`TElement::get_attr`](stylo::dom::TElement::get_attr)).

use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::{BLOOM_HASH_MASK, BloomFilter};
use selectors::context::MatchingContext;
use selectors::matching::ElementSelectorFlags;
use selectors::{Element, OpaqueElement};
use stylo::selector_parser::{NonTSPseudoClass, PseudoElement, SelectorImpl};
use stylo::values::{AtomIdent, AtomString};
use stylo::{CaseSensitivityExt, LocalName, Namespace};

use crate::arena::ElementRef;
use crate::ext::ExternalState;

impl<T: ExternalState> Element for ElementRef<'_, T> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.element())
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
        self.element().tag.0 == *local_name
    }

    fn has_namespace(
        &self,
        ns: &<Self::Impl as selectors::SelectorImpl>::BorrowedNamespaceUrl,
    ) -> bool {
        // Elements are never namespaced here: only the empty namespace matches.
        ns.is_empty()
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.element().tag == other.element().tag
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&Namespace>,
        local_name: &LocalName,
        operation: &AttrSelectorOperation<&AtomString>,
    ) -> bool {
        let name: &str = local_name.0.as_ref();
        if let Some(value) = self.element().attrs.get(name) {
            return operation.eval_str(value);
        }
        // Synthetic / reflected attributes are the embedder's: consulted only
        // after the real attrs map misses (see `ExternalState::extra_attr_value`).
        self.element()
            .ext
            .extra_attr_value(name)
            .is_some_and(|value| operation.eval_str(&value))
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &NonTSPseudoClass,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        // Match the dynamic pseudo-classes against the element's state. Every
        // other non-tree-structural pseudo-class is unsupported → false.
        match pc {
            NonTSPseudoClass::Hover | NonTSPseudoClass::Active | NonTSPseudoClass::Focus => {
                self.element().element_state.contains(pc.state_flag())
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
            self.element()
                .selector_flags
                .borrow_mut()
                .insert(self_flags);
        }
        let parent_flags = flags.for_parent();
        if !parent_flags.is_empty()
            && let Some(parent) = self.parent()
        {
            parent
                .element()
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
        self.element()
            .id_attr
            .as_ref()
            .is_some_and(|my_id| case_sensitivity.eq_atom(my_id, id))
    }

    fn has_class(&self, name: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.element()
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
        // Non-empty if the element has any child, or carries non-empty
        // character data.
        self.children().next().is_none()
            && self.element().text.as_ref().is_none_or(String::is_empty)
    }

    fn is_root(&self) -> bool {
        // `:root` is a parentless element whose external state also deems it a
        // root. An embedder with a distinguished root element narrows
        // `ExternalState::is_root` so a detached subtree's parentless top does
        // not match `:root` during resolve; the default keeps parentless ⇒
        // root.
        self.parent().is_none() && self.element().ext.is_root()
    }

    fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool {
        stylo::bloom::each_relevant_element_hash(*self, |hash| {
            filter.insert_hash(hash & BLOOM_HASH_MASK);
        });
        true
    }
}
