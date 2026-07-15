//! [`TDocument`] + [`TShadowRoot`] for [`&Node`](crate::Node).
//!
//! There is no distinct document node in this model — the tree root doubles as
//! the document — and no shadow DOM, so [`TShadowRoot`] is a stub that is never
//! actually reached (`TNode::as_shadow_root` always returns `None`).

use stylo::context::QuirksMode;
use stylo::dom::{TDocument, TNode, TShadowRoot};
use stylo::shared_lock::SharedRwLock;
use stylo::stylist::CascadeData;

use crate::ext::ExternalState;
use crate::node::Node;

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
        self.document().shared_lock()
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
