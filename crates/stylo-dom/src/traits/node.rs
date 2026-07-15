//! [`NodeInfo`] + [`TNode`] for [`&Node`](crate::Node).

use stylo::dom::{NodeInfo, OpaqueNode, TNode};

use crate::ext::ExternalState;
use crate::node::Node;

impl<T: ExternalState> NodeInfo for &Node<T> {
    fn is_element(&self) -> bool {
        // Every node is an element with a tag; character data rides on the
        // element itself (`Node::text`).
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
        self.parent()
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
        // Derived from the (index, generation) id — NOT the element's address:
        // stylo keys snapshot maps by `OpaqueNode` across arbitrary tree
        // mutations, and arena growth can reallocate and move every element.
        self.node_id().opaque()
    }

    fn debug_id(self) -> usize {
        // Diagnostic only: the arena slot index stands in for a node id.
        usize::try_from(self.node_id().index()).unwrap_or(0)
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent()
    }
}
