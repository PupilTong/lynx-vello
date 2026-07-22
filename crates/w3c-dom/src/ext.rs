//! The [`ExternalState`] marker trait for embedder payloads.
//!
//! [`Node`](crate::Node) is generic over an external-state payload `T`,
//! carried in its [`ext`](crate::Node::ext) field. The payload is opaque to
//! this crate: selector-visible state belongs in the node's real DOM fields,
//! including [`Node::attrs`](crate::Node::attrs). The marker only records the
//! `Sync` requirement imposed by stylo's parallel traversal.

/// The embedder-supplied external state carried by every
/// [`Node`](crate::Node).
///
/// Implementations hold whatever per-node data the embedder needs alongside
/// the W3C-DOM-subset fields. Matching-relevant data must be written through
/// the corresponding [`Document`](crate::Document) mutation APIs rather than
/// read indirectly from this payload.
///
/// `Sync` is required because the restyle traversal may run in parallel:
/// rayon workers may share references to nodes and their payloads.
pub trait ExternalState: Sync {}

/// The no-op payload used by this crate's tests and embedders that need no
/// external state.
impl ExternalState for () {}
