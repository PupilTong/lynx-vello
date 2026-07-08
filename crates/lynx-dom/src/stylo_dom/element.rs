//! [`TElement`] for [`ElemRef`].
//!
//! # Safety
//!
//! This is the only module carrying `unsafe`, hence the module-wide
//! `#![allow(unsafe_code)]`. All of it is the interior-mutable per-element
//! state stylo mandates ([`ensure_data`](TElement::ensure_data),
//! [`clear_data`](TElement::clear_data), `borrow_data`, `mutate_data`). Each
//! `unsafe` access relies on the crate-wide **single-threaded-flush**
//! invariant: an element's [`stylo_data`](crate::Node::stylo_data) is only ever
//! touched while the caller holds exclusive access to the tree, so no aliasing
//! or data race is possible.
#![allow(unsafe_code)]

use std::sync::OnceLock;

use app_units::Au;
use euclid::default::Size2D;
use selectors::matching::{ElementSelectorFlags, VisitedHandlingMode};
use selectors::sink::Push;
use stylo::applicable_declarations::ApplicableDeclarationBlock;
use stylo::context::SharedStyleContext;
use stylo::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};
use stylo::dom::{LayoutIterator, TElement};
use stylo::properties::PropertyDeclarationBlock;
use stylo::selector_parser::{AttrValue, Lang, PseudoElement, SelectorImpl};
use stylo::servo_arc::{Arc, ArcBorrow};
use stylo::shared_lock::Locked;
use stylo::values::AtomIdent;
use stylo::values::computed::Display;
use stylo::{LocalName, Namespace};
use stylo_atoms::Atom;
use stylo_dom::ElementState;

use crate::arena::{Arena, ElemRef, ElementId};

/// The children iterator stylo's restyle traversal walks. Skips over any child
/// whose handle no longer resolves (defensive; live trees never hit that).
#[derive(Debug)]
pub struct ChildrenIter<'a> {
    arena: &'a Arena,
    children: &'a [ElementId],
    index: usize,
}

impl<'a> Iterator for ChildrenIter<'a> {
    type Item = ElemRef<'a>;

    fn next(&mut self) -> Option<ElemRef<'a>> {
        while self.index < self.children.len() {
            let id = self.children[self.index];
            self.index += 1;
            if let Some(elem) = self.arena.elem_ref(id) {
                return Some(elem);
            }
        }
        None
    }
}

/// The single shared empty namespace, returned by [`TElement::namespace`] (Lynx
/// tags are never namespaced).
fn empty_namespace() -> &'static <SelectorImpl as selectors::SelectorImpl>::BorrowedNamespaceUrl {
    static EMPTY: OnceLock<Namespace> = OnceLock::new();
    &EMPTY.get_or_init(Namespace::default).0
}

impl<'a> TElement for ElemRef<'a> {
    type ConcreteNode = ElemRef<'a>;
    type TraversalChildrenIterator = ChildrenIter<'a>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(ChildrenIter {
            arena: self.arena,
            children: &self.node().children,
            index: 0,
        })
    }

    fn is_html_element(&self) -> bool {
        true
    }

    fn is_mathml_element(&self) -> bool {
        false
    }

    fn is_svg_element(&self) -> bool {
        false
    }

    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        self.node().inline_block.as_ref().map(Arc::borrow_arc)
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
        self.node().element_state
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&Atom> {
        // In the servo build stylo's `WeakAtom` is `stylo_atoms::Atom`.
        self.node().id_attr.as_ref()
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        for class in &self.node().classes {
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
        for name in self.node().attrs.keys() {
            callback(&LocalName::from(name.as_ref()));
        }
        // Expose the synthetic `l-css-id` attribute (see `attr_matches`) so the
        // bloom filter accounts for it in the future scoped-CSS mode.
        callback(&LocalName::from("l-css-id"));
    }

    fn has_dirty_descendants(&self) -> bool {
        self.node().dirty_descendants
    }

    fn has_snapshot(&self) -> bool {
        // Coarse invalidation (see `crate::dirty`): we never snapshot.
        false
    }

    fn handled_snapshot(&self) -> bool {
        true
    }

    unsafe fn set_handled_snapshot(&self) {}

    unsafe fn set_dirty_descendants(&self) {
        // No-op: the flush driver tracks descendant dirtiness itself
        // (`Node::dirty_descendants`); stylo's own traversal bits are unused in
        // this milestone.
    }

    unsafe fn unset_dirty_descendants(&self) {}

    fn store_children_to_process(&self, _n: isize) {}

    fn did_process_child(&self) -> isize {
        0
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        // SAFETY: single-threaded flush — the caller holds exclusive access to
        // this element, so creating/borrowing its `ElementData` cannot race.
        let slot = unsafe { &mut *self.node().stylo_data.get() };
        slot.get_or_insert_with(ElementDataWrapper::default)
            .borrow_mut()
    }

    unsafe fn clear_data(&self) {
        // SAFETY: single-threaded flush — exclusive access, no concurrent
        // borrow of this element's stylo state.
        unsafe {
            *self.node().stylo_data.get() = None;
        }
        *self.node().selector_flags.borrow_mut() = ElementSelectorFlags::empty();
    }

    fn has_data(&self) -> bool {
        // SAFETY: reads only the `Option` discriminant; no concurrent mutation
        // under the single-threaded-flush invariant.
        unsafe { (*self.node().stylo_data.get()).is_some() }
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        // SAFETY: `ElementDataWrapper` tracks borrows internally; single-threaded
        // flush rules out a concurrent mutable borrow.
        unsafe {
            (*self.node().stylo_data.get())
                .as_ref()
                .map(ElementDataWrapper::borrow)
        }
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        // SAFETY: as `borrow_data`, plus exclusive access under single-threaded
        // flush.
        unsafe {
            (*self.node().stylo_data.get())
                .as_ref()
                .map(ElementDataWrapper::borrow_mut)
        }
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

    fn shadow_root(&self) -> Option<ElemRef<'a>> {
        None
    }

    fn containing_shadow(&self) -> Option<ElemRef<'a>> {
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
        &self.node().tag.0
    }

    fn namespace(&self) -> &<SelectorImpl as selectors::SelectorImpl>::BorrowedNamespaceUrl {
        empty_namespace()
    }

    fn query_container_size(&self, _display: &Display) -> Size2D<Option<Au>> {
        Size2D::new(None, None)
    }

    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.node().selector_flags.borrow().contains(flags)
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::empty()
    }

    fn get_attr(&self, attr: &LocalName, _namespace: &Namespace) -> Option<String> {
        let name: &str = attr.0.as_ref();
        if name == "l-css-id" {
            return Some(self.node().css_id.to_string());
        }
        self.node().attrs.get(name).cloned()
    }
}
