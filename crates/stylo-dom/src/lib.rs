//! `stylo-dom` — a generic, stylo-integrated DOM core.
//!
//! This crate models a **strict subset of the HTML DOM** and everything stylo
//! needs to run its cascade over it: a generational [`Arena`] of [`Element`]s
//! addressed by an [`ElementId`], the [`ElementRef`] navigation handle, the
//! low-level tree-mutation + coarse-invalidation primitives, inline-style
//! parsing, and the stylo element-trait impls.
//!
//! It knows nothing about any particular embedder: [`Element`] is generic over
//! an external-state payload `T` (its [`ext`](Element::ext) field), and the
//! [`ExternalState`] trait is the only channel through which that payload can
//! influence matching (`:root` participation, synthetic / reflected
//! attributes). In lynx-vello the Lynx embedding layer supplies its own
//! payload type; `()` works as a payload wherever no external state is
//! needed.
//!
//! # stylo integration
//!
//! Elements are interned into stylo atoms ([`Element::tag`],
//! [`Element::classes`], [`Element::id_attr`]) and each [`Element`] owns
//! stylo's interior-mutable per-element state; the [`traits`] module implements
//! stylo's element traits ([`TNode`](stylo::dom::TNode) /
//! [`TElement`](stylo::dom::TElement) / [`TDocument`](stylo::dom::TDocument) /
//! [`selectors::Element`]) on [`ElementRef`]. Style *resolution* itself is
//! driven by the embedding style engine over these impls; this crate only
//! builds/mutates the tree and tracks dirty state.
//!
//! Inline styles are parsed at mutation time (see [`inline`]) into a stylo
//! [`PropertyDeclarationBlock`](stylo::properties::PropertyDeclarationBlock)
//! guarded by the arena's [`SharedRwLock`](stylo::shared_lock::SharedRwLock); to
//! style a tree, build the [`Arena`] with the style engine's lock
//! ([`Arena::with_lock`]) so the cascade's guards match.
//!
//! # Thread-safety
//!
//! Because an [`Element`] owns an `UnsafeCell` of stylo's `ElementData`, the
//! tree is **not** `Sync`, and the whole crate assumes a **single-threaded
//! flush**: resolution and mutation never run concurrently on the same arena.
//! The `unsafe` this requires is confined to [`traits`].
//!
//! # Layout
//!
//! - [`arena`] — the generational arena, [`ElementId`], and the [`ElementRef`] navigation handle
//!   (the type stylo traits are implemented on).
//! - [`element`] — the unified [`Element`] struct (the HTML-DOM-subset fields plus the `ext`
//!   payload).
//! - [`ext`] — the [`ExternalState`] trait the payload implements.
//! - [`state`] — the [`PseudoState`] flag set (`:hover` / `:active` / `:focus`).
//! - [`traits`] — stylo's element-trait impls on [`ElementRef`].
//!
//! The tree-mutation ([`tree`]), inline-style ([`inline`]), and
//! coarse-invalidation ([`dirty`]) primitives are added as methods on [`Arena`].

pub mod arena;
pub mod element;
pub mod ext;
pub mod state;
pub mod traits;

mod dirty;
mod inline;
mod tree;

pub use arena::{Arena, ElementId, ElementRef};
/// stylo's [`ElementState`](dom::ElementState), re-exported so downstream crates
/// never name the vendored `stylo_dom` package directly.
pub use dom::ElementState;
pub use element::Element;
pub use ext::ExternalState;
pub use state::PseudoState;
