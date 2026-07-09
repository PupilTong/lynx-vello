//! [`NodeInfo`] + [`TNode`] for [`WidgetRef`].

use stylo::dom::{NodeInfo, OpaqueNode, TNode};

use crate::arena::WidgetRef;
use crate::widget::Widget;

impl NodeInfo for WidgetRef<'_> {
    fn is_element(&self) -> bool {
        // Every Lynx node — including `<raw-text>` — is an element with a tag.
        true
    }

    fn is_text_node(&self) -> bool {
        false
    }
}

impl<'a> TNode for WidgetRef<'a> {
    type ConcreteElement = WidgetRef<'a>;
    type ConcreteDocument = WidgetRef<'a>;
    type ConcreteShadowRoot = WidgetRef<'a>;

    fn parent_node(&self) -> Option<Self> {
        self.parent()
    }

    fn first_child(&self) -> Option<Self> {
        WidgetRef::first_child(*self)
    }

    fn last_child(&self) -> Option<Self> {
        WidgetRef::last_child(*self)
    }

    fn prev_sibling(&self) -> Option<Self> {
        WidgetRef::prev_sibling(*self)
    }

    fn next_sibling(&self) -> Option<Self> {
        WidgetRef::next_sibling(*self)
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
        OpaqueNode(std::ptr::from_ref::<Widget>(self.widget()) as usize)
    }

    fn debug_id(self) -> usize {
        usize::try_from(self.unique_id()).unwrap_or(0)
    }

    fn traversal_parent(&self) -> Option<Self::ConcreteElement> {
        self.parent()
    }
}
