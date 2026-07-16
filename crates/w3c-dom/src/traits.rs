//! stylo element-trait implementations, directly on `&Node`.
//!
//! stylo drives selector matching and the cascade over any type implementing
//! its element traits. This module wires the document tree to that model by
//! implementing, on the plain shared reference `&'a Node<T>` (for any
//! payload `T: `[`ExternalState`]):
//!
//! - [`NodeInfo`] + [`TNode`]
//! - [`TElement`]
//! - [`TDocument`] + [`TShadowRoot`]
//! - [`selectors::Element`]
//!
//! There is no handle type: a reference is already the one-word `Copy` value
//! stylo requires (its style-sharing cache sizes TLS for a word-sized
//! `TElement` handle), and navigation runs through the node's document
//! backpointer — so stylo styles the document **in place**, no second tree
//! is materialized to enter the styling engine.
//!
//! Implementation note: inside these impls, inherent `Node` methods that
//! share a name with a trait method (`parent`, `first_child`,
//! `next_sibling`, `id`, `has_dirty_descendants`, …) are called **fully
//! qualified** (`Node::parent(*self)`), never with method syntax — on a
//! `&Node` receiver with the trait in scope, method-call syntax resolves to
//! the trait impl first, which here would recurse.
//!
//! # Model
//!
//! - **Every node is an element.** There is no separate document or text node: the topmost ancestor
//!   acts as the document root (see [`TNode::owner_doc`]), and character data rides on the node
//!   ([`Node::text`](crate::Node::text)).
//! - **`:hover`/`:active`/`:focus`** are matched from the node's
//!   [`ElementState`](crate::ElementState).
//! - **`:root`** matches a parentless node whose [`ExternalState::is_root`] hook agrees.
//! - **Synthetic / reflected attributes** beyond the node's real attribute map are served by the
//!   [`ExternalState`] attribute hooks.
//! - **Shadow DOM / pseudo-elements / animations** are stubbed (`None`/`false`) — none exist in
//!   this model yet.
//!
//! # Safety
//!
//! This module carries the `unsafe` for the interior-mutable per-node state
//! stylo mandates ([`ensure_data`](TElement::ensure_data),
//! [`clear_data`](TElement::clear_data), `borrow_data`, `mutate_data`). Each
//! `unsafe` access relies on **stylo's traversal discipline**: during a
//! (possibly parallel) restyle traversal, each node's
//! [`stylo_data`](crate::Node) is touched by exactly one worker at a time (a
//! parent reads/writes a child's data only in `note_children`, strictly
//! before any worker takes ownership of that child), and outside a traversal
//! the embedder holds `&mut Document`. All other per-node state stylo mutates
//! through `&self` is atomic (see [`Node`](crate::Node)).
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

use crate::ext::ExternalState;
use crate::node::{ChildrenIter, Node};

/// The single shared empty namespace, returned by [`TElement::namespace`]
/// (tags are never namespaced here).
fn empty_namespace() -> &'static <SelectorImpl as selectors::SelectorImpl>::BorrowedNamespaceUrl {
    static EMPTY: OnceLock<Namespace> = OnceLock::new();
    &EMPTY.get_or_init(Namespace::default).0
}

// --- NodeInfo + TNode -------------------------------------------------------

impl<T: ExternalState> NodeInfo for &Node<T> {
    fn is_element(&self) -> bool {
        // Every node is an element with a tag; character data rides on the
        // node itself (`Node::text`).
        true
    }

    fn is_text_node(&self) -> bool {
        false
    }
}

impl<'a, T: ExternalState> TNode for &'a Node<T> {
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
        Node::prev_sibling(*self)
    }

    fn next_sibling(&self) -> Option<Self> {
        Node::next_sibling(*self)
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        // No separate document node: the topmost ancestor acts as the
        // document.
        let mut cur = *self;
        while let Some(parent) = Node::parent(cur) {
            cur = parent;
        }
        cur
    }

    fn is_in_document(&self) -> bool {
        // Style resolution only ever visits attached nodes; coarse but true
        // for everything the flush driver reaches.
        true
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        Some(*self)
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        // Our root is an ordinary node, not a distinct document node.
        None
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }

    fn opaque(&self) -> OpaqueNode {
        // Derived from the (index, generation) id — NOT the node's address:
        // stylo keys snapshot maps by `OpaqueNode` across arbitrary tree
        // mutations, and slot-storage growth can move every node.
        Node::id(*self).opaque()
    }

    fn debug_id(self) -> usize {
        // Diagnostic only: the slot index stands in for a node id.
        usize::try_from(Node::id(self).index()).unwrap_or(0)
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        Node::parent(*self)
    }
}

// --- TDocument + TShadowRoot --------------------------------------------------

impl<'a, T: ExternalState> TDocument for &'a Node<T> {
    type ConcreteNode = &'a Node<T>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn is_html_document(&self) -> bool {
        true
    }

    fn quirks_mode(&self) -> QuirksMode {
        QuirksMode::NoQuirks
    }

    fn shared_lock(&self) -> &SharedRwLock {
        // The document backpointer serves stylo's lock lookup without any
        // per-node lock clone.
        &self.tree().lock
    }
}

impl<'a, T: ExternalState> TShadowRoot for &'a Node<T> {
    type ConcreteNode = &'a Node<T>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn host(&self) -> <Self::ConcreteNode as TNode>::ConcreteElement {
        // Unreachable: we never expose shadow roots (`as_shadow_root` is
        // always `None`), so stylo never calls `host()`.
        *self
    }

    fn style_data<'b>(&self) -> Option<&'b CascadeData>
    where
        Self: 'b,
    {
        None
    }
}

// --- TElement -----------------------------------------------------------------

impl<'a, T: ExternalState> TElement for &'a Node<T> {
    type ConcreteNode = &'a Node<T>;
    type TraversalChildrenIterator = ChildrenIter<'a, T>;

    fn as_node(&self) -> Self::ConcreteNode {
        *self
    }

    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(Node::children(*self))
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
        // In the servo build stylo's `WeakAtom` is `stylo_atoms::Atom`.
        self.id_attr.as_ref()
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
        for name in self.attrs.keys() {
            callback(&LocalName::from(name.as_ref()));
        }
        // Synthetic / reflected attribute names come from the embedder, so
        // the bloom filter accounts for them too (see
        // `ExternalState::each_extra_attr_name`).
        self.ext.each_extra_attr_name(&mut callback);
    }

    fn has_dirty_descendants(&self) -> bool {
        Node::has_dirty_descendants(self)
    }

    fn has_snapshot(&self) -> bool {
        // Set by the document's snapshot recorders (see
        // `crate::invalidation`); consumed by stylo's invalidation pass.
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
        self.children_to_process.store(n, Ordering::SeqCst);
    }

    fn did_process_child(&self) -> isize {
        self.children_to_process.fetch_sub(1, Ordering::SeqCst) - 1
    }

    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        // Debug contract check: slot-exclusive access, traversal phase only.
        // (The returned borrow is separately tracked by `ElementDataWrapper`.)
        #[cfg(debug_assertions)]
        let _access = {
            debug_assert!(
                self.tree().in_flush(),
                "TElement::ensure_data called outside a style traversal"
            );
            self.slot_guard.begin_write()
        };
        // SAFETY: traversal discipline — the caller holds exclusive access to
        // this node, so creating/borrowing its `ElementData` cannot race.
        let slot = unsafe { &mut *self.stylo_data.get() };
        slot.get_or_insert_with(ElementDataWrapper::default)
            .borrow_mut()
    }

    unsafe fn clear_data(&self) {
        #[cfg(debug_assertions)]
        let _access = {
            debug_assert!(
                self.tree().in_flush(),
                "TElement::clear_data called outside a style traversal"
            );
            self.slot_guard.begin_write()
        };
        // SAFETY: traversal discipline — exclusive access to this node, no
        // concurrent borrow of its stylo state.
        unsafe {
            *self.stylo_data.get() = None;
        }
        self.selector_flags.store(0, Ordering::Relaxed);
    }

    fn has_data(&self) -> bool {
        #[cfg(debug_assertions)]
        let _access = self.slot_guard.begin_read();
        // SAFETY: reads only the `Option` discriminant; the slot is only
        // created/removed by this node's owning worker (or under
        // `&mut Document`), never concurrently with this read.
        unsafe { (*self.stylo_data.get()).is_some() }
    }

    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        #[cfg(debug_assertions)]
        let _access = self.slot_guard.begin_read();
        // SAFETY: `ElementDataWrapper` tracks borrows internally (debug
        // builds); the traversal discipline rules out a concurrent mutable
        // borrow.
        unsafe {
            (*self.stylo_data.get())
                .as_ref()
                .map(ElementDataWrapper::borrow)
        }
    }

    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        // Slot-wise this is a *read* (`as_ref`); the mutable borrow of the
        // inner data is tracked by `ElementDataWrapper` itself.
        #[cfg(debug_assertions)]
        let _access = self.slot_guard.begin_read();
        // SAFETY: as `borrow_data`, plus exclusive access under the traversal
        // discipline.
        unsafe {
            (*self.stylo_data.get())
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
        &self.tag.0
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
        let name: &str = attr.0.as_ref();
        if let Some(value) = self.attrs.get(name) {
            return Some(value.clone());
        }
        // Synthetic / reflected attributes are the embedder's: consulted only
        // after the real attribute map misses, matching `attr_matches`.
        self.ext.extra_attr_value(name)
    }
}

// --- selectors::Element ---------------------------------------------------------

/// id/class matching is **case-sensitive**; `:hover`/`:active`/`:focus` are
/// matched from the node's [`ElementState`]; attribute
/// matching covers the node's real attributes plus whatever synthetic /
/// reflected attributes the embedder's [`ExternalState`] hooks serve.
impl<T: ExternalState> Element for &Node<T> {
    type Impl = SelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(*self)
    }

    fn parent_element(&self) -> Option<Self> {
        Node::parent(*self)
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
        Node::prev_sibling(*self)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        Node::next_sibling(*self)
    }

    fn first_element_child(&self) -> Option<Self> {
        Node::first_child(*self)
    }

    fn is_html_element_in_html_document(&self) -> bool {
        true
    }

    fn has_local_name(
        &self,
        local_name: &<Self::Impl as selectors::SelectorImpl>::BorrowedLocalName,
    ) -> bool {
        self.tag.0 == *local_name
    }

    fn has_namespace(
        &self,
        ns: &<Self::Impl as selectors::SelectorImpl>::BorrowedNamespaceUrl,
    ) -> bool {
        // Nodes are never namespaced here: only the empty namespace matches.
        ns.is_empty()
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.tag == other.tag
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&Namespace>,
        local_name: &LocalName,
        operation: &AttrSelectorOperation<&AtomString>,
    ) -> bool {
        // Known gap: `class`/`id` live in their own fields (`classes`,
        // `id_attr`), not in `attrs`, and are not reflected here — so
        // attribute-form selectors like `[class~=x]`/`[id=y]` never match
        // them (use `.x`/`#y`). Reflecting them costs a string build on this
        // hot path for a selector form ReactLynx CSS does not use;
        // invalidation (the class/id snapshot recorders) is consistent with
        // this choice.
        let name: &str = local_name.0.as_ref();
        if let Some(value) = self.attrs.get(name) {
            return operation.eval_str(value);
        }
        // Synthetic / reflected attributes are the embedder's: consulted only
        // after the real attribute map misses (see `ExternalState::extra_attr_value`).
        self.ext
            .extra_attr_value(name)
            .is_some_and(|value| operation.eval_str(&value))
    }

    fn match_non_ts_pseudo_class(
        &self,
        pc: &NonTSPseudoClass,
        _context: &mut selectors::context::MatchingContext<Self::Impl>,
    ) -> bool {
        // Match the dynamic pseudo-classes against the node's state. Every
        // other non-tree-structural pseudo-class is unsupported → false.
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
        // stylo's contract splits the flags: `for_self()` bits land on this
        // node, `for_parent()` bits (slow-selector / edge-child markers) on
        // its parent. Atomic ORs: parallel workers matching sibling nodes may
        // both push parent flags onto the shared parent.
        let self_flags = flags.for_self();
        if !self_flags.is_empty() {
            self.selector_flags
                .fetch_or(self_flags.bits(), Ordering::Relaxed);
        }
        let parent_flags = flags.for_parent();
        if !parent_flags.is_empty()
            && let Some(parent) = Node::parent(*self)
        {
            parent
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
        self.id_attr
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
        // Non-empty if the node has any child, or carries non-empty
        // character data.
        self.children.is_empty() && self.text.as_ref().is_none_or(String::is_empty)
    }

    fn is_root(&self) -> bool {
        // `:root` is a parentless node whose external state also deems it a
        // root. An embedder with a distinguished root node narrows
        // `ExternalState::is_root` so a detached subtree's parentless top
        // does not match `:root` during resolve; the default keeps
        // parentless ⇒ root.
        self.parent.is_none() && self.ext.is_root()
    }

    fn add_element_unique_hashes(&self, filter: &mut BloomFilter) -> bool {
        stylo::bloom::each_relevant_element_hash(*self, |hash| {
            filter.insert_hash(hash & BLOOM_HASH_MASK);
        });
        true
    }
}
