//! [`TElement`] for [`&Node`](crate::Node).
//!
//! # Safety
//!
//! This module carries the `unsafe` for the interior-mutable per-element
//! state stylo mandates ([`ensure_data`](TElement::ensure_data),
//! [`clear_data`](TElement::clear_data), `borrow_data`, `mutate_data`). Each
//! `unsafe` access relies on **stylo's traversal discipline**: during a
//! (possibly parallel) restyle traversal, each element's
//! [`stylo_data`](crate::Node::stylo_data) is touched by exactly one
//! worker at a time (a parent reads/writes a child's data only in
//! `note_children`, strictly before any worker takes ownership of that
//! child), and outside a traversal the embedder holds `&mut Document`. All other
//! per-element state stylo mutates through `&self` is atomic (see
//! [`Node`](crate::Node)).
#![allow(unsafe_code)]

use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use app_units::Au;
use dom::ElementState;
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

use crate::arena::{DocumentInner, ElementId};
use crate::ext::ExternalState;
use crate::node::Node;

/// The children iterator stylo's restyle traversal walks. Skips over any child
/// whose handle no longer resolves (defensive; live trees never hit that).
pub struct ChildrenIter<'a, T> {
    document: &'a DocumentInner<T>,
    children: &'a [ElementId],
    index: usize,
}

impl<T> std::fmt::Debug for ChildrenIter<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChildrenIter")
            .field("remaining", &self.children.len().saturating_sub(self.index))
            .finish()
    }
}

impl<'a, T> Iterator for ChildrenIter<'a, T> {
    type Item = &'a Node<T>;

    fn next(&mut self) -> Option<&'a Node<T>> {
        while self.index < self.children.len() {
            let id = self.children[self.index];
            self.index += 1;
            if let Some(elem) = self.document.node(id) {
                return Some(elem);
            }
        }
        None
    }
}

/// The single shared empty namespace, returned by [`TElement::namespace`]
/// (tags are never namespaced here).
fn empty_namespace() -> &'static <SelectorImpl as selectors::SelectorImpl>::BorrowedNamespaceUrl {
    static EMPTY: OnceLock<Namespace> = OnceLock::new();
    &EMPTY.get_or_init(Namespace::default).0
}

impl<'a, T: ExternalState> TElement for &'a Node<T> {
    type ConcreteNode = &'a Node<T>;
    type TraversalChildrenIterator = ChildrenIter<'a, T>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(ChildrenIter {
            document: self.document(),
            children: &self.element().children,
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
        self.element().inline_block.as_ref().map(Arc::borrow_arc)
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
        self.element().element_state
    }

    fn has_part_attr(&self) -> bool {
        false
    }

    fn exports_any_part(&self) -> bool {
        false
    }

    fn id(&self) -> Option<&Atom> {
        // In the servo build stylo's `WeakAtom` is `stylo_atoms::Atom`.
        self.element().id_attr.as_ref()
    }

    fn each_class<F>(&self, mut callback: F)
    where
        F: FnMut(&AtomIdent),
    {
        for class in &self.element().classes {
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
        for name in self.element().attrs.keys() {
            callback(&LocalName::from(name.as_ref()));
        }
        // Synthetic / reflected attribute names come from the embedder, so the
        // bloom filter accounts for them too (see
        // `ExternalState::each_extra_attr_name`).
        self.element().ext.each_extra_attr_name(&mut callback);
    }

    fn has_dirty_descendants(&self) -> bool {
        self.element().has_dirty_descendants()
    }

    fn has_snapshot(&self) -> bool {
        // Set by the arena's `note_*_change` snapshot recorders (see
        // `crate::dirty`); consumed by stylo's invalidation pass.
        self.element().snapshot_present()
    }

    fn handled_snapshot(&self) -> bool {
        self.element().snapshot_handled()
    }

    unsafe fn set_handled_snapshot(&self) {
        self.element().set_snapshot_handled();
    }

    unsafe fn set_dirty_descendants(&self) {
        self.element().set_dirty_descendants_bit(true);
    }

    unsafe fn unset_dirty_descendants(&self) {
        self.element().set_dirty_descendants_bit(false);
    }

    fn store_children_to_process(&self, n: isize) {
        self.element()
            .children_to_process
            .store(n, Ordering::SeqCst);
    }

    fn did_process_child(&self) -> isize {
        self.element()
            .children_to_process
            .fetch_sub(1, Ordering::SeqCst)
            - 1
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        // SAFETY: traversal discipline — the caller holds exclusive access to
        // this element, so creating/borrowing its `ElementData` cannot race.
        let slot = unsafe { &mut *self.element().stylo_data.get() };
        slot.get_or_insert_with(ElementDataWrapper::default)
            .borrow_mut()
    }

    unsafe fn clear_data(&self) {
        // SAFETY: traversal discipline — exclusive access to this element, no
        // concurrent borrow of its stylo state.
        unsafe {
            *self.element().stylo_data.get() = None;
        }
        self.element().selector_flags.store(0, Ordering::Relaxed);
    }

    fn has_data(&self) -> bool {
        // SAFETY: reads only the `Option` discriminant; the slot is only
        // created/removed by this element's owning worker (or under `&mut
        // Document`), never concurrently with this read.
        unsafe { (*self.element().stylo_data.get()).is_some() }
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        // SAFETY: `ElementDataWrapper` tracks borrows internally (debug
        // builds); the traversal discipline rules out a concurrent mutable
        // borrow.
        unsafe {
            (*self.element().stylo_data.get())
                .as_ref()
                .map(ElementDataWrapper::borrow)
        }
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        // SAFETY: as `borrow_data`, plus exclusive access under the traversal
        // discipline.
        unsafe {
            (*self.element().stylo_data.get())
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
        &self.element().tag.0
    }

    fn namespace(&self) -> &<SelectorImpl as selectors::SelectorImpl>::BorrowedNamespaceUrl {
        empty_namespace()
    }

    fn query_container_size(&self, _display: &Display) -> Size2D<Option<Au>> {
        Size2D::new(None, None)
    }

    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.element().selector_flags().contains(flags)
    }

    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        ElementSelectorFlags::empty()
    }

    fn get_attr(&self, attr: &LocalName, _namespace: &Namespace) -> Option<String> {
        let name: &str = attr.0.as_ref();
        if let Some(value) = self.element().attrs.get(name) {
            return Some(value.clone());
        }
        // Synthetic / reflected attributes are the embedder's: consulted only
        // after the real attrs map misses, matching `attr_matches`.
        self.element().ext.extra_attr_value(name)
    }
}
