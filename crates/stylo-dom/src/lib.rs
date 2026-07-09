//! `stylo-dom` — the stylo-integration DOM machinery of **lynx-vello**.
//!
//! lynx-vello is a from-scratch native Rust reimplementation of the `LynxJS`
//! web-bundle runtime (see `AGENTS.md` for the full mission). This crate owns
//! the retained element tree and everything stylo needs to run its cascade over
//! it: a generational [`Arena`] of [`Widget`]s addressed by a [`WidgetId`], the
//! [`WidgetRef`] navigation handle, the low-level tree-mutation +
//! coarse-invalidation primitives, inline-style parsing, and the stylo
//! element-trait impls.
//!
//! The thin Lynx Element-PAPI surface (`WidgetTree`) that validates opcode
//! semantics and drives these primitives lives in the separate `lynx-dom`
//! crate; the CSS cascade over these impls is driven by `lynx-style`.
//!
//! # stylo integration
//!
//! Elements are interned into stylo atoms ([`Widget::tag`], [`Widget::classes`],
//! [`Widget::id_attr`]) and each [`Widget`] owns stylo's interior-mutable
//! per-element state; the [`traits`] module implements stylo's element traits
//! ([`TNode`](stylo::dom::TNode) / [`TElement`](stylo::dom::TElement) /
//! [`TDocument`](stylo::dom::TDocument) / [`selectors::Element`]) on
//! [`WidgetRef`]. Style *resolution* itself is driven by the separate
//! `lynx-style` crate over these impls; this crate only builds/mutates the tree
//! and tracks dirty state.
//!
//! Inline styles are parsed at mutation time (see [`inline`]) into a stylo
//! [`PropertyDeclarationBlock`](stylo::properties::PropertyDeclarationBlock)
//! guarded by the arena's [`SharedRwLock`](stylo::shared_lock::SharedRwLock); to
//! style a tree, build the [`Arena`] with the `StyleEngine`'s lock
//! ([`Arena::with_lock`]) so the cascade's guards match.
//!
//! # Thread-safety
//!
//! Because a [`Widget`] owns an `UnsafeCell` of stylo's `ElementData`, the tree
//! is **not** `Sync`, and the whole crate assumes a **single-threaded flush**:
//! resolution and mutation never run concurrently on the same arena. The
//! `unsafe` this requires is confined to [`traits`].
//!
//! # Layout
//!
//! - [`arena`] — the generational arena, [`WidgetId`], and the [`WidgetRef`] navigation handle (the
//!   type stylo traits are implemented on).
//! - [`widget`] — the unified [`Widget`] struct plus event registration types.
//! - [`kind`] — the Lynx tag-name ↔ [`WidgetKind`] mapping.
//! - [`state`] — the [`PseudoState`] flag set (`:hover` / `:active` / `:focus`).
//! - [`traits`] — stylo's element-trait impls on [`WidgetRef`].
//!
//! The tree-mutation ([`tree`]), inline-style ([`inline`]), and
//! coarse-invalidation ([`dirty`]) primitives are added as methods on [`Arena`].

pub mod arena;
pub mod kind;
pub mod state;
pub mod traits;
pub mod widget;

mod dirty;
mod inline;
mod tree;

pub use arena::{Arena, WidgetId, WidgetRef};
/// stylo's [`ElementState`](dom::ElementState), re-exported so downstream crates
/// never name the vendored `stylo_dom` package directly.
pub use dom::ElementState;
pub use kind::WidgetKind;
pub use state::PseudoState;
pub use widget::{EventKind, EventReg, Widget};
