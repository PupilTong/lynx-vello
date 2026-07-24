//! Stylo DOM traits implemented directly on the one-word `&Node` handle.
#![allow(unsafe_code)]

use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use app_units::Au;
use dom::ElementState;
use euclid::default::Size2D;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::{BLOOM_HASH_MASK, BloomFilter};
use selectors::matching::{ElementSelectorFlags, VisitedHandlingMode};
use selectors::sink::Push;
use selectors::{Element, OpaqueElement};
use stylo::applicable_declarations::ApplicableDeclarationBlock;
use stylo::context::{QuirksMode, SharedStyleContext};
use stylo::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};
use stylo::dom::{LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode, TShadowRoot};
use stylo::properties::PropertyDeclarationBlock;
use stylo::selector_parser::{AttrValue, Lang, NonTSPseudoClass, PseudoElement, SelectorImpl};
use stylo::servo_arc::{Arc, ArcBorrow};
use stylo::shared_lock::{Locked, SharedRwLock};
use stylo::stylist::CascadeData;
use stylo::values::computed::Display;
use stylo::values::{AtomIdent, AtomString};
use stylo::{CaseSensitivityExt, LocalName, Namespace};
use stylo_atoms::Atom;

use crate::node::{ChildrenIter, Node};

fn empty_namespace() -> &'static <SelectorImpl as selectors::SelectorImpl>::BorrowedNamespaceUrl {
    static EMPTY: OnceLock<Namespace> = OnceLock::new();
    &EMPTY.get_or_init(Namespace::default).0
}

impl<T: Sync> NodeInfo for &Node<T> {
    fn is_element(&self) -> bool {
        Node::is_element(self)
    }

    fn is_text_node(&self) -> bool {
        Node::is_text_node(self)
    }
}

impl<'a, T: Sync> TNode for &'a Node<T> {
    type ConcreteElement = &'a Node<T>;
    type ConcreteDocument = &'a Node<T>;
    type ConcreteShadowRoot = &'a Node<T>;

    fn parent_node(&self) -> Option<Self> {
        Node::parent(*self)
    }

    fn first_child(&self) -> Option<Self> {
        Node::first_child(*self)
    }

    fn last_child(&self) -> Option<Self> {
        Node::last_child(*self)
    }

    fn prev_sibling(&self) -> Option<Self> {
        Node::previous_sibling(*self)
    }

    fn next_sibling(&self) -> Option<Self> {
        Node::next_sibling(*self)
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        Node::owner_document(*self)
    }

    fn is_in_document(&self) -> bool {
        let mut current = *self;
        loop {
            if current.is_document() {
                return true;
            }
            let Some(parent) = Node::parent(current) else {
                return false;
            };
            current = parent;
        }
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        Node::is_element(self).then_some(*self)
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        Node::is_document(self).then_some(*self)
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }

    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(Node::id(self))
    }

    fn debug_id(self) -> usize {
        Node::id(self)
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        let parent = Node::parent(*self)?;
        parent.is_element().then_some(parent)
    }
}

impl<'a, T: Sync> TDocument for &'a Node<T> {
    type ConcreteNode = &'a Node<T>;

    fn as_node(&self) -> Self::ConcreteNode {
        debug_assert!(Node::is_document(self));
        *self
    }

    fn is_html_document(&self) -> bool {
        true
    }

    fn quirks_mode(&self) -> QuirksMode {
        QuirksMode::NoQuirks
    }

    fn shared_lock(&self) -> &SharedRwLock {
        debug_assert!(Node::is_document(self));
        self.document_lock()
    }
}

impl<'a, T: Sync> TShadowRoot for &'a Node<T> {
    type ConcreteNode = &'a Node<T>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn host(&self) -> <Self::ConcreteNode as TNode>::ConcreteElement {
        unreachable!("w3c-dom does not model shadow roots")
    }

    fn style_data<'b>(&self) -> Option<&'b CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

impl<'a, T: Sync> TElement for &'a Node<T> {
    type ConcreteNode = &'a Node<T>;
    type TraversalChildrenIterator = ChildrenIter<'a, T>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(Node::children(*self))
    }

    fn is_html_element(&self) -> bool {
        Node::is_element(self)
    }

    fn is_mathml_element(&self) -> bool {
        false
    }

    fn is_svg_element(&self) -> bool {
        false
    }

    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        self.inline_block.as_ref().map(Arc::borrow_arc)
    }

    fn animation_rule(
        &self,
        _: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn transition_rule(
        &self,
        _: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }

    fn state(&self) -> ElementState {
        self.element_state
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&Atom> {
        self.id_attribute.as_ref()
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        for class in &self.classes {
            callback(AtomIdent::cast(class));
        }
    }

    fn each_custom_state<F>(&self, _callback: F)
    where
        F: FnMut(&AtomIdent),
    {
    }

    fn each_attr_name<F>(&self, mut callback: F)
    where
        F: FnMut(&LocalName),
    {
        for (name, _) in &self.attrs {
            callback(name);
        }
    }

    fn has_dirty_descendants(&self) -> bool {
        Node::has_dirty_descendants(self)
    }

    fn has_snapshot(&self) -> bool {
        self.snapshot_present()
    }

    fn handled_snapshot(&self) -> bool {
        self.snapshot_handled()
    }

    unsafe fn set_handled_snapshot(&self) {
        self.set_snapshot_handled();
    }

    unsafe fn set_dirty_descendants(&self) {
        self.set_dirty_descendants_bit(true);
    }

    unsafe fn unset_dirty_descendants(&self) {
        self.set_dirty_descendants_bit(false);
    }

    fn store_children_to_process(&self, n: isize) {
        self.styling_data()
            .children_to_process
            .store(n, Ordering::SeqCst);
    }

    fn did_process_child(&self) -> isize {
        self.styling_data()
            .children_to_process
            .fetch_sub(1, Ordering::SeqCst)
            - 1
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        #[cfg(debug_assertions)]
        let _access = {
            debug_assert!(
                self.in_flush(),
                "TElement::ensure_data called outside a style traversal"
            );
            self.styling_data().slot_guard.begin_write()
        };
        let slot = unsafe { &mut *self.stylo_data.get() };
        slot.get_or_insert_with(ElementDataWrapper::default)
            .borrow_mut()
    }

    unsafe fn clear_data(&self) {
        #[cfg(debug_assertions)]
        let _access = {
            debug_assert!(
                self.in_flush(),
                "TElement::clear_data called outside a style traversal"
            );
            self.styling_data().slot_guard.begin_write()
        };
        self.set_layout_style_pointer(std::ptr::null_mut());
        unsafe {
            *self.stylo_data.get() = None;
        }
        self.styling_data()
            .selector_flags
            .store(0, Ordering::Relaxed);
    }

    fn has_data(&self) -> bool {
        #[cfg(debug_assertions)]
        let _access = self.styling_data().slot_guard.begin_read();
        unsafe { (*self.stylo_data.get()).is_some() }
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        #[cfg(debug_assertions)]
        let _access = self.styling_data().slot_guard.begin_read();
        unsafe {
            (*self.stylo_data.get())
                .as_ref()
                .map(ElementDataWrapper::borrow)
        }
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        #[cfg(debug_assertions)]
        let _access = self.styling_data().slot_guard.begin_read();
        #[expect(unsafe_code, reason = "Stylo owns the ElementData access contract")]
        let data = unsafe {
            (*self.stylo_data.get())
                .as_ref()
                .map(ElementDataWrapper::borrow_mut)
        }?;
        if !self.in_flush() {
            self.mark_layout_style_stale();
            self.set_layout_styles_ready(false);
        }
        Some(data)
    }

    fn skip_item_display_fixup(&self) -> bool {
        false
    }

    fn may_have_animations(&self) -> bool {
        false
    }

    fn has_animations(&self, _: &SharedStyleContext) -> bool {
        false
    }

    fn has_css_animations(&self, _: &SharedStyleContext, _: Option<PseudoElement>) -> bool {
        false
    }

    fn has_css_transitions(&self, _: &SharedStyleContext, _: Option<PseudoElement>) -> bool {
        false
    }

    fn shadow_root(&self) -> Option<&'a Node<T>> {
        None
    }

    fn containing_shadow(&self) -> Option<&'a Node<T>> {
        None
    }

    fn lang_attr(&self) -> Option<AttrValue> {
        None
    }

    fn match_element_lang(&self, _override_lang: Option<Option<AttrValue>>, _value: &Lang) -> bool {
        false
    }

    fn is_html_document_body_element(&self) -> bool {
        false
    }

    fn synthesize_presentational_hints_for_legacy_attributes<V>(
        &self,
        _visited_handling: VisitedHandlingMode,
        _hints: &mut V,
    ) where
        V: Push<ApplicableDeclarationBlock>,
    {
    }

    fn local_name(&self) -> &<SelectorImpl as selectors::SelectorImpl>::BorrowedLocalName {
        &self
            .local_name
            .as_ref()
            .expect("TElement::local_name called for a text node")
            .0
    }

    fn namespace(&self) -> &<SelectorImpl as selectors::SelectorImpl>::BorrowedNamespaceUrl {
        empty_namespace()
    }

    fn query_container_size(&self, _display: &Display) -> Size2D<Option<Au>> {
        Size2D::new(None, None)
    }

    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.selector_flags().contains(flags)
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::empty()
    }

    fn get_attr(&self, attr: &LocalName, _namespace: &Namespace) -> Option<String> {
        self.attr_local_name(attr).map(str::to_owned)
    }
}

impl<T: Sync> Element for &Node<T> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(*self)
    }

    fn parent_element(&self) -> Option<Self> {
        Node::parent(*self).filter(|parent| Node::is_element(*parent))
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
        let mut sibling = Node::previous_sibling(*self);
        while let Some(node) = sibling {
            if node.is_element() {
                return Some(node);
            }
            sibling = Node::previous_sibling(node);
        }
        None
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let mut sibling = Node::next_sibling(*self);
        while let Some(node) = sibling {
            if node.is_element() {
                return Some(node);
            }
            sibling = Node::next_sibling(node);
        }
        None
    }

    fn first_element_child(&self) -> Option<Self> {
        let mut child = Node::first_child(*self);
        while let Some(node) = child {
            if node.is_element() {
                return Some(node);
            }
            child = Node::next_sibling(node);
        }
        None
    }

    fn is_html_element_in_html_document(&self) -> bool {
        Node::is_element(self)
    }

    fn has_local_name(
        &self,
        local_name: &<Self::Impl as selectors::SelectorImpl>::BorrowedLocalName,
    ) -> bool {
        self.local_name
            .as_ref()
            .is_some_and(|name| name.0 == *local_name)
    }

    fn has_namespace(
        &self,
        ns: &<Self::Impl as selectors::SelectorImpl>::BorrowedNamespaceUrl,
    ) -> bool {
        self.is_element() && ns.is_empty()
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.is_element() && other.is_element() && self.local_name == other.local_name
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&Namespace>,
        local_name: &LocalName,
        operation: &AttrSelectorOperation<&AtomString>,
    ) -> bool {
        self.attr_local_name(local_name)
            .is_some_and(|value| operation.eval_str(value))
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &NonTSPseudoClass,
        _context: &mut selectors::context::MatchingContext<Self::Impl>,
    ) -> bool {
        match pc {
            NonTSPseudoClass::Hover | NonTSPseudoClass::Active | NonTSPseudoClass::Focus => {
                self.element_state.contains(pc.state_flag())
            }
            _ => false,
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &PseudoElement,
        _context: &mut selectors::context::MatchingContext<Self::Impl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        let self_flags = flags.for_self();
        if !self_flags.is_empty() {
            self.styling_data()
                .selector_flags
                .fetch_or(self_flags.bits(), Ordering::Relaxed);
        }
        let parent_flags = flags.for_parent();
        if !parent_flags.is_empty()
            && let Some(parent) = Node::parent(*self)
        {
            parent
                .styling_data()
                .selector_flags
                .fetch_or(parent_flags.bits(), Ordering::Relaxed);
        }
    }

    fn is_link(&self) -> bool {
        false
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.id_attribute
            .as_ref()
            .is_some_and(|my_id| case_sensitivity.eq_atom(my_id, id))
    }

    fn has_class(&self, name: &AtomIdent, case_sensitivity: CaseSensitivity) -> bool {
        self.classes
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
        self.is_element() && self.is_empty_element()
    }

    fn is_root(&self) -> bool {
        Node::parent(*self).is_some_and(Node::is_document)
    }

    fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool {
        if !self.is_element() {
            return false;
        }
        stylo::bloom::each_relevant_element_hash(*self, |hash| {
            filter.insert_hash(hash & BLOOM_HASH_MASK);
        });
        true
    }
}
