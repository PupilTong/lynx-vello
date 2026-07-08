//! [`NodeInfo`] + [`TNode`] for [`ElemRef`].

use stylo::dom::{NodeInfo, OpaqueNode, TNode};

use crate::arena::ElemRef;
use crate::node::Node;

impl NodeInfo for ElemRef<'_> {
    fn is_element(&self) -> bool {
        // Every Lynx node — including `<raw-text>` — is an element with a tag.
        true
    }

    fn is_text_node(&self) -> bool {
        false
    }
}

impl<'a> TNode for ElemRef<'a> {
    type ConcreteElement = ElemRef<'a>;
    type ConcreteDocument = ElemRef<'a>;
    type ConcreteShadowRoot = ElemRef<'a>;

    fn parent_node(&self) -> Option<Self> {
        self.parent()
    }

    fn first_child(&self) -> Option<Self> {
        ElemRef::first_child(*self)
    }

    fn last_child(&self) -> Option<Self> {
        ElemRef::last_child(*self)
    }

    fn prev_sibling(&self) -> Option<Self> {
        ElemRef::prev_sibling(*self)
    }

    fn next_sibling(&self) -> Option<Self> {
        ElemRef::next_sibling(*self)
    }

    fn owner_doc(&self) -> Self::ConcreteDocument {
        // No separate document node: the topmost ancestor (the `<page>` root)
        // acts as the document.
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
        // Our root is the `<page>` element, not a distinct document node.
        None
    }

    fn as_shadow_root(&self) -> Option<Self::ConcreteShadowRoot> {
        None
    }

    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(std::ptr::from_ref::<Node>(self.node()) as usize)
    }

    fn debug_id(self) -> usize {
        usize::try_from(self.unique_id()).unwrap_or(0)
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent()
    }
}
