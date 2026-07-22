//! `w3c-dom` — a generic, stylo-integrated W3C-DOM-subset document tree.
//!
//! This crate is a **pure DOM layer** composed of [`Node<T>`]s, plus
//! everything stylo needs to run its cascade over it in place. The public
//! surface is deliberately small:
//!
//! - [`Document<T>`] — **the one tree.** It owns a fixed-address slab whose slot zero is the actual
//!   DOM document node. Element and text nodes are created by [`Document::create_element`] /
//!   [`Document::create_text_node`] and mutated exclusively through `Document` methods; there is no
//!   way to construct, mutate, or re-home a node outside its document (ONE TREE policy).
//! - [`Node<T>`] — the compositional unit. [`NodeType::Document`] is slot zero,
//!   [`NodeType::Element`] nodes carry the W3C-DOM-subset element fields and stylo bookkeeping;
//!   [`NodeType::Text`] nodes carry character data. Element and text variants own the embedder
//!   payload; all nodes share tree links and the common bookkeeping layout. Read-only from outside
//!   the crate.
//! - [`NodeId`] — the raw `usize` slab index, scoped to its runtime context. The *read* handle is a
//!   plain `&Node<T>`; every stylo DOM trait is implemented directly on it (no wrapper type).
//! - [`StyleEngine`] — stylesheet parsing/building, matching, rule-tree insertion, cascade, and the
//!   style flush ([`StyleEngine::flush_document`], returning a [`FlushSummary`] of per-node
//!   [`StyleDamage`]).
//! - [`ExternalState`] — the embedder-payload trait; the only channel through which the payload `T`
//!   influences matching (synthetic / reflected attributes).
//!
//! # Contract: let it crash
//!
//! Mutation methods treat invalid input — vacant/out-of-range [`NodeId`]s,
//! cycle-creating links, unrelated insertion references — as **caller bugs**,
//! not conditions to absorb: preconditions are `debug_assert!`ed and the
//! internal lookups panic rather than silently no-op. Query methods (`get`,
//! `child_position`, …) return `Option` instead; asking is always legal. The
//! ownership layer must not retain a raw ID after its node is removed and the
//! slab slot becomes reusable.
//! Embedders facing untrusted handles (a scripting runtime) validate first
//! and map violations to their own error types.
//!
//! # stylo integration — one tree, one-word handles
//!
//! Element local names, classes, and ids are interned as stylo atoms, and each
//! element node owns stylo's interior-mutable style state; the crate-private
//! `traits` module implements stylo's
//! node traits with `&'a Node<T>` as the common
//! [`TElement`](stylo::dom::TElement)/[`TNode`](stylo::dom::TNode)/
//! [`TDocument`](stylo::dom::TDocument) handle. The internal `NodeData` distinguishes the
//! document, element, and text cases without wrapper structs.
//! Styling therefore runs **on the document itself** — no mirror tree is
//! built to enter the styling engine. Two design points make that work:
//!
//! - every node carries a **backpointer** to its document's fixed-address slab, so tree navigation
//!   needs nothing but `&Node` — a shared reference is exactly the one-word `Copy` value stylo's
//!   style-sharing cache requires of a `TElement` handle;
//! - node identity for snapshots/traversal roots ([`OpaqueNode`](stylo::dom::OpaqueNode)) is the
//!   raw slab index, so it survives slab-storage growth moving nodes.
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
//! # Layout
//!
//! The [`layout`] module is the crate's box-layout integration: it
//! implements `neutron-star`'s handle protocol (`LayoutNode` on a Copy
//! handle, stylo-vocabulary style views lent straight from
//! `ComputedValues`) directly over the document — Flexbox, Grid, and
//! Starlight Linear/Relative containers, with leaf content measured through
//! an embedder hook. Run it with [`StyleEngine::layout_document`] (styles
//! flush first — the style → layout phase barrier); results live **on the
//! nodes** ([`Node::layout`]), so layout state is created and dropped with
//! its node. See the module docs for the phase and invalidation contracts.
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

mod contain;
mod damage;
mod document;
mod engine;
mod ext;
mod flush;
mod invalidation;
pub mod layout;
mod node;
mod traits;

/// stylo's [`ElementState`], re-exported so downstream crates can name
/// dynamic pseudo-class bits (`:hover`/`:active`/`:focus`) without depending
/// on the vendored stylo packages directly.
pub use dom::ElementState;

/// stylo's computed containment value types ([`Contain`] /
/// [`ContentVisibility`]) are re-exported alongside the
/// [`effective_containment`] fold so downstream crates never name the
/// vendored stylo packages directly.
pub use crate::contain::{Contain, ContentVisibility, effective_containment};
pub use crate::damage::{FlushSummary, StyleDamage};
pub use crate::document::{DOCUMENT_NODE_ID, Document, NodeId};
pub use crate::engine::{
    ComputedStyle, CssRule, RawDeclaration, StyleEngine, StylesheetOrigin, property_is_supported,
};
pub use crate::ext::ExternalState;
pub use crate::flush::Parallelism;
pub use crate::node::{ChildrenIter, Node, NodeType};
