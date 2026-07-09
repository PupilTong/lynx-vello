//! [`NodeInfo`] + [`TNode`] for [`ElementRef`].

use stylo::dom::{NodeInfo, OpaqueNode, TNode};

use crate::arena::ElementRef;
use crate::element::Element;
use crate::ext::ExternalState;

impl<T: ExternalState> NodeInfo for ElementRef<'_, T> {
    fn is_element(&self) -> bool {
        // Every node is an element with a tag; character data rides on the
        // element itself (`Element::text`).
        true
    }

    fn is_text_node(&self) -> bool {
        false
    }
}

impl<'a, T: ExternalState> TNode for ElementRef<'a, T> {
    type ConcreteElement = ElementRef<'a, T>;
    type ConcreteDocument = ElementRef<'a, T>;
    type ConcreteShadowRoot = ElementRef<'a, T>;

    fn parent_node(&self) -> Option<Self> {
        self.parent()
    }

    fn first_child(&self) -> Option<Self> {
        ElementRef::first_child(*self)
    }

    fn last_child(&self) -> Option<Self> {
        ElementRef::last_child(*self)
    }

    fn prev_sibling(&self) -> Option<Self> {
        ElementRef::prev_sibling(*self)
    }

    fn next_sibling(&self) -> Option<Self> {
        ElementRef::next_sibling(*self)
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        // No separate document node: the topmost ancestor acts as the
        // document.
        let mut cur = *self;
        while let Some(parent) = cur.parent() {
            cur = parent;
        }
        cur
    }

    fn is_in_document(&self) -> bool {
        // Style resolution only ever visits attached nodes; coarse but true for
        // everything the flush driver reaches.
        true
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        Some(*self)
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        // Our root is an ordinary element, not a distinct document node.
        None
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }

    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(std::ptr::from_ref::<Element<T>>(self.element()) as usize)
    }

    fn debug_id(self) -> usize {
        // Diagnostic only: the arena slot index stands in for a node id.
        usize::try_from(self.id().index()).unwrap_or(0)
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent()
    }
}
