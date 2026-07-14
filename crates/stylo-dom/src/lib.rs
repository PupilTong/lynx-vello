//! `stylo-dom` — a generic, stylo-integrated DOM core.
//!
//! This crate models a **strict subset of the HTML DOM** and everything stylo
//! needs to run its cascade over it: a generational [`Arena`] of real
//! [`Element`] and [`TextNode`] nodes addressed by a [`NodeId`], the
//! [`NodeRef`] / [`ElementRef`] navigation handles, the
//! low-level tree-mutation + coarse-invalidation primitives, inline-style
//! parsing, the stylo element-trait impls, and a standards-oriented CSS
//! computation engine.
//!
//! It knows nothing about any particular embedder: [`Element`] is generic over
//! an external-state payload `T` (its [`ext`](Element::ext) field), and the
//! [`ExternalState`] trait is the only channel through which that payload can
//! influence matching (`:root` participation, synthetic / reflected
//! attributes). In lynx-vello the Lynx embedding layer supplies its own
//! payload type; `()` works as a payload wherever no external state is needed.
//!
//! # stylo integration
//!
//! Elements are interned into stylo atoms ([`Element::tag`],
//! [`Element::classes`], [`Element::id_attr`]) and each [`Element`] owns
//! stylo's interior-mutable per-element state; the [`traits`] module implements
//! stylo's element traits ([`TNode`](stylo::dom::TNode) /
//! [`TElement`](stylo::dom::TElement) / [`TDocument`](stylo::dom::TDocument) /
//! [`selectors::Element`]) on [`NodeRef`] and [`ElementRef`]. [`StyleEngine`] owns stylesheet
//! parsing, matching, rule-tree insertion, cascade, and the shared style lock.
//! Embedders supply a [`stylo::device::Device`] and keep platform-specific
//! metrics outside this crate.
//!
//! Inline styles are parsed at mutation time into a stylo
//! [`PropertyDeclarationBlock`](stylo::properties::PropertyDeclarationBlock)
//! guarded by a crate-owned [`SharedRwLock`](stylo::shared_lock::SharedRwLock).
//! Create styled trees through [`StyleEngine::new_arena`]; the lock never needs
//! to cross the public embedder boundary.
//!
//! # Thread-safety
//!
//! Style flushes ([`StyleEngine::flush_tree`]) run **stylo's own restyle
//! traversal**, which may fan out over rayon workers sharing `&Arena`. Every
//! piece of element state stylo touches through `&self` during a traversal is
//! atomic; the one non-atomic slot (the `UnsafeCell` of stylo's
//! `ElementData`) is owned by exactly one worker at a time under stylo's
//! traversal discipline — see [`Element`] and the `SAFETY` notes in
//! [`traits`] and [`flush`]. Outside a flush, mutation goes through
//! `&mut Arena`, so nothing races.
//!
//! # Layout
//!
//! - [`arena`] — the generational arena, [`NodeId`], and read-only node/element handles.
//! - [`node`] / [`element`] — distinct Text and Element node variants; only elements carry the
//!   embedder's `ext` payload and Stylo style data.
//! - [`ext`] — the [`ExternalState`] trait the payload implements.
//! - [`state`] — the [`PseudoState`] flag set (`:hover` / `:active` / `:focus`).
//! - [`style`] — the generic [`StyleEngine`] and stylesheet/cascade pipeline.
//! - [`traits`] — stylo's element-trait impls on [`ElementRef`].
//!
//! Tree-mutation, inline-style, and coarse-invalidation primitives are added
//! as methods on [`Arena`].

pub mod arena;
pub mod element;
pub mod ext;
pub mod flush;
pub mod layout;
pub mod node;
pub mod state;
pub mod style;
pub mod traits;

mod dirty;
mod inline;
mod tree;

pub use arena::{Arena, ElementId, ElementRef, NodeId, NodeRef};
/// stylo's [`ElementState`], re-exported so downstream crates
/// never name the vendored `stylo_dom` package directly.
pub use dom::ElementState;
pub use element::Element;
pub use ext::ExternalState;
pub use flush::Parallelism;
pub use node::{Node, TextNode};
pub use state::PseudoState;
pub use style::{
    ComputedStyle, CssRule, RawDeclaration, StyleEngine, StylesheetOrigin, property_is_supported,
};
