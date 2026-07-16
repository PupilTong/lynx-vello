//! `w3c-dom` — a generic, stylo-integrated W3C-DOM-subset document tree.
//!
//! This crate is a **pure DOM layer** composed of [`Node<T>`]s, plus
//! everything stylo needs to run its cascade over it in place. The public
//! surface is deliberately small:
//!
//! - [`Document<T>`] — **the one tree and its actual DOM document node.** It owns every element,
//!   its optional `documentElement`, and the private style context. Elements are created by
//!   [`Document::create_node`] and mutated exclusively through `Document` methods; there is no way
//!   to construct, mutate, or re-home one outside its document (ONE TREE policy).
//! - [`Node<T>`] — the compositional unit: the W3C-DOM-subset fields (tree links, tag, id, classes,
//!   attributes, dynamic pseudo-class state, inline style, character data), its pending
//!   invalidation snapshot, and stylo's per-node style bookkeeping. Read-only from outside the
//!   crate.
//! - [`NodeId`] — a generational, staleness-detecting handle. The *read* handle is a plain
//!   `&Node<T>`; the stylo element traits are implemented directly on it (no wrapper type).
//! - [`StyleEngine`] — stylesheet parsing/building, matching, rule-tree insertion, cascade, and the
//!   style flush ([`StyleEngine::flush_document`]).
//! - [`ExternalState`] — the embedder-payload trait; the only channel through which the payload `T`
//!   influences matching (synthetic / reflected attributes).
//!
//! # Contract: let it crash
//!
//! Mutation methods treat invalid input — stale [`NodeId`]s, cycle-creating
//! links, foreign insertion references — as **caller bugs**, not conditions
//! to absorb: preconditions are `debug_assert!`ed and the internal lookups
//! panic rather than silently no-op. Query methods (`get`, `node_ref`,
//! `child_position`, …) return `Option` instead; asking is always legal.
//! Embedders facing untrusted handles (a scripting runtime) validate first
//! and map violations to their own error types.
//!
//! # stylo integration — one tree, one-word handles
//!
//! Tags/classes/ids are interned as stylo atoms and each [`Node`] owns
//! stylo's interior-mutable per-node state; the crate-private `traits` module
//! implements stylo's
//! element traits with `&'a Node<T>` as the hot [`TElement`](stylo::dom::TElement)
//! handle and a small internal node view for
//! [`TNode`](stylo::dom::TNode)/[`TDocument`](stylo::dom::TDocument), so the
//! distinct document node is represented without turning it into an Element.
//! Styling therefore runs **on the document itself** — no mirror tree is
//! built to enter the styling engine. Two design points make that work:
//!
//! - every node carries a **backpointer** to its (heap-pinned) document core, so tree navigation
//!   needs nothing but `&Node` — a shared reference is exactly the one-word `Copy` value stylo's
//!   style-sharing cache requires of a `TElement` handle;
//! - node identity for snapshots/traversal roots ([`OpaqueNode`](stylo::dom::OpaqueNode)) derives
//!   from the generational [`NodeId`], so it survives slab-storage growth moving nodes.
//!
//! Inline styles are parsed at mutation time into a stylo
//! [`PropertyDeclarationBlock`](stylo::properties::PropertyDeclarationBlock)
//! guarded by a crate-owned `SharedRwLock`. Create styled documents through
//! [`StyleEngine::new_document`]; the lock never crosses the public embedder
//! boundary.
//!
//! # Invalidation is not optional
//!
//! Every matching-relevant setter ([`Document::set_classes`],
//! [`Document::set_attribute`], [`Document::set_state`], structural
//! mutation, …) records its own pre-mutation snapshot or scoped restyle hint
//! before touching the node — the "snapshot before mutate" rule is enforced
//! by construction rather than asked of the embedder. The single exception
//! an embedder must handle is its own synthetic / reflected attributes:
//! pair [`Document::ext_mut`] with
//! [`Document::note_external_attribute_change`].
//!
//! # Thread-safety
//!
//! Style flushes ([`StyleEngine::flush_document`]) run **stylo's own restyle
//! traversal**, which may fan out over rayon workers sharing the document.
//! Every piece of node state stylo touches through `&self` during a
//! traversal is atomic; the one non-atomic slot (the `UnsafeCell` of stylo's
//! `ElementData`) is owned by exactly one worker at a time under stylo's
//! traversal discipline — see [`Node`] and the `SAFETY` notes in the
//! crate-private `traits` and `flush` modules. Outside a flush, mutation
//! goes through `&mut Document`, so
//! nothing races. This discipline is what the upcoming parallel style
//! resolving relies on; do not add non-atomic `&self` mutability to [`Node`].

mod document;
mod engine;
mod ext;
mod flush;
mod invalidation;
mod node;
mod traits;

/// stylo's [`ElementState`], re-exported so downstream crates can name
/// dynamic pseudo-class bits (`:hover`/`:active`/`:focus`) without depending
/// on the vendored stylo packages directly.
pub use dom::ElementState;

pub use crate::document::{Document, NodeId};
pub use crate::engine::{
    ComputedStyle, CssRule, RawDeclaration, StyleEngine, StylesheetOrigin, property_is_supported,
};
pub use crate::ext::ExternalState;
pub use crate::flush::Parallelism;
pub use crate::node::{ChildrenIter, Node};
#[doc(hidden)]
pub use crate::traits::{DomChildrenIter, DomDocument, DomNode};
