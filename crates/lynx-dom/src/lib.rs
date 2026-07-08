//! `lynx-dom` — the element/DOM layer of **lynx-vello**.
//!
//! lynx-vello is a from-scratch native Rust reimplementation of the `LynxJS`
//! web-bundle runtime (see `AGENTS.md` for the full mission). This crate owns
//! the retained element tree: a generational arena of [`Node`]s addressed by a
//! [`ElementId`], a `snake_case` Element-PAPI surface ([`Document`]) mirroring
//! Lynx's JS Element PAPI, and coarse style-invalidation bookkeeping.
//!
//! # stylo integration
//!
//! Elements are interned into stylo atoms ([`Node::tag`], [`Node::classes`],
//! [`Node::id_attr`]) and each [`Node`] owns stylo's interior-mutable
//! per-element state; the [`stylo_dom`] module implements stylo's element
//! traits ([`TNode`](stylo::dom::TNode) / [`TElement`](stylo::dom::TElement) /
//! [`TDocument`](stylo::dom::TDocument) / [`selectors::Element`]) on
//! [`ElemRef`]. Style *resolution* itself is driven by the separate
//! `lynx-style` crate over these impls; this crate only builds/mutates the tree
//! and tracks dirty state (see [`Document::has_dirty`] /
//! [`Document::clear_dirty`]).
//!
//! Inline styles are parsed at mutation time into a stylo
//! [`PropertyDeclarationBlock`](stylo::properties::PropertyDeclarationBlock)
//! guarded by the arena's [`SharedRwLock`](stylo::shared_lock::SharedRwLock);
//! to style a tree, build the [`Document`] with the `StyleEngine`'s lock
//! ([`Document::with_lock`]) so the cascade's guards match.
//!
//! # Thread-safety
//!
//! Because a [`Node`] owns an `UnsafeCell` of stylo's `ElementData`, the tree
//! is **not** `Sync`, and the whole crate assumes a **single-threaded flush**:
//! resolution and mutation never run concurrently on the same arena. The
//! `unsafe` this requires is confined to [`stylo_dom`].
//!
//! # Layout
//!
//! - [`arena`] — the generational arena, [`ElementId`], and the [`ElemRef`] navigation handle (the
//!   type stylo traits are implemented on).
//! - [`node`] — the unified [`Node`] struct plus event registration types.
//! - [`tag`] — the Lynx tag-name ↔ [`NodeKind`] mapping.
//! - [`state`] — the [`PseudoState`] flag set (`:hover` / `:active` / `:focus`).
//! - [`papi`] — the [`Document`] type and its Element-PAPI-shaped methods.
//! - [`stylo_dom`] — stylo's element-trait impls on [`ElemRef`].

pub mod arena;
pub mod node;
pub mod papi;
pub mod state;
pub mod stylo_dom;
pub mod tag;

mod dirty;

pub use arena::{Arena, ElemRef, ElementId};
pub use node::{EventKind, EventReg, Node};
pub use papi::{Document, DomError};
pub use state::PseudoState;
pub use tag::NodeKind;
