//! DOM node variants stored by [`Arena`](crate::Arena).

use std::fmt;

use crate::arena::NodeId;
use crate::element::Element;

/// A DOM Text node.
///
/// Text nodes deliberately contain only character data and their tree link.
/// They do not carry an embedder payload, element attributes, or Stylo's
/// per-element computed-style state; their inherited text style comes from
/// their element ancestors.
#[derive(Clone, PartialEq, Eq)]
pub struct TextNode {
    /// The parent node, or `None` while detached.
    pub parent: Option<NodeId>,
    data: String,
}

impl TextNode {
    /// Create a detached Text node containing `data`.
    #[must_use]
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            parent: None,
            data: data.into(),
        }
    }

    /// This Text node's character data.
    #[must_use]
    pub fn data(&self) -> &str {
        &self.data
    }

    /// Replace this Text node's character data without applying tree/style
    /// invalidation. Prefer [`Arena::set_text`](crate::Arena::set_text) when
    /// the node is in an arena.
    pub(crate) fn set_data(&mut self, data: String) {
        self.data = data;
    }
}

impl fmt::Debug for TextNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextNode")
            .field("parent", &self.parent)
            .field("data", &self.data)
            .finish()
    }
}

/// A live DOM node stored in an [`Arena`](crate::Arena).
///
/// Only [`Element`] variants carry the embedder's external state and Stylo
/// element data. [`TextNode`] is a distinct DOM node kind rather than a
/// specially tagged element.
pub enum Node<T> {
    /// An element node.
    Element(Element<T>),
    /// A character-data Text node.
    Text(TextNode),
}

impl<T> Node<T> {
    /// Borrow this node as an element.
    #[must_use]
    pub fn as_element(&self) -> Option<&Element<T>> {
        match self {
            Self::Element(element) => Some(element),
            Self::Text(_) => None,
        }
    }

    /// Mutably borrow this node as an element.
    pub fn as_element_mut(&mut self) -> Option<&mut Element<T>> {
        match self {
            Self::Element(element) => Some(element),
            Self::Text(_) => None,
        }
    }

    /// Borrow this node as Text.
    #[must_use]
    pub fn as_text(&self) -> Option<&TextNode> {
        match self {
            Self::Element(_) => None,
            Self::Text(text) => Some(text),
        }
    }

    /// Mutably borrow this node as Text.
    pub fn as_text_mut(&mut self) -> Option<&mut TextNode> {
        match self {
            Self::Element(_) => None,
            Self::Text(text) => Some(text),
        }
    }

    /// The node's parent, if attached.
    #[must_use]
    pub(crate) fn parent(&self) -> Option<NodeId> {
        match self {
            Self::Element(element) => element.parent,
            Self::Text(text) => text.parent,
        }
    }

    /// Set the node's parent link.
    pub(crate) fn set_parent(&mut self, parent: Option<NodeId>) {
        match self {
            Self::Element(element) => element.parent = parent,
            Self::Text(text) => text.parent = parent,
        }
    }

    /// The node's children. Text nodes always return an empty slice.
    #[must_use]
    pub(crate) fn children(&self) -> &[NodeId] {
        match self {
            Self::Element(element) => &element.children,
            Self::Text(_) => &[],
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Node<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Element(element) => f.debug_tuple("Element").field(element).finish(),
            Self::Text(text) => f.debug_tuple("Text").field(text).finish(),
        }
    }
}
