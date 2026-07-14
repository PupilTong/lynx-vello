//! [`NodeInfo`] + [`TNode`] for [`NodeRef`].

use stylo::dom::{NodeInfo, OpaqueNode, TNode};

use crate::arena::{ElementRef, NodeRef};
use crate::ext::ExternalState;

impl<T: ExternalState> NodeInfo for NodeRef<'_, T> {
    fn is_element(&self) -> bool {
        NodeRef::is_element(*self)
    }

    fn is_text_node(&self) -> bool {
        self.is_text()
    }
}

impl<'a, T: ExternalState> TNode for NodeRef<'a, T> {
    type ConcreteElement = ElementRef<'a, T>;
    type ConcreteDocument = ElementRef<'a, T>;
    type ConcreteShadowRoot = ElementRef<'a, T>;

    fn parent_node(&self) -> Option<Self> {
        self.parent()
    }

    fn first_child(&self) -> Option<Self> {
        NodeRef::first_child(*self)
    }

    fn last_child(&self) -> Option<Self> {
        NodeRef::last_child(*self)
    }

    fn prev_sibling(&self) -> Option<Self> {
        NodeRef::prev_sibling(*self)
    }

    fn next_sibling(&self) -> Option<Self> {
        NodeRef::next_sibling(*self)
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        // No separate Document node: the distinguished root Element acts as
        // it. An attached node reaches that root through its ancestors; a
        // detached Text node falls back to the root owned by the same arena.
        let mut current = *self;
        while let Some(parent) = current.parent() {
            current = parent;
        }
        current
            .as_element()
            .or_else(|| self.arena.document_element_ref())
            .expect("a DOM arena must contain a root Element")
    }

    fn is_in_document(&self) -> bool {
        let mut current = *self;
        while let Some(parent) = current.parent() {
            current = parent;
        }
        current
            .as_element()
            .is_some_and(|root| root.ext().is_root())
    }

    fn as_element(&self) -> Option<Self::ConcreteElement> {
        NodeRef::as_element(*self)
    }

    fn as_document(&self) -> Option<Self::ConcreteDocument> {
        None
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }

    fn opaque(&self) -> OpaqueNode {
        self.id().opaque()
    }

    fn debug_id(self) -> usize {
        usize::try_from(self.id().index()).unwrap_or(0)
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent()?.as_element()
    }
}
