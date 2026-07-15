//! stylo element-trait implementations for [`&Node`](crate::Node).
//!
//! stylo drives selector matching and the cascade over any type implementing
//! its element traits. This module wires our arena-backed DOM to that model by
//! implementing them on the `Copy` shared reference `&Node<T>` (for
//! any payload `T: `[`ExternalState`](crate::ExternalState)):
//!
//! - [`NodeInfo`](stylo::dom::NodeInfo) + [`TNode`](stylo::dom::TNode) (`node`)
//! - [`TElement`](stylo::dom::TElement) (`element`)
//! - [`TDocument`](stylo::dom::TDocument) + [`TShadowRoot`](stylo::dom::TShadowRoot) (`document`)
//! - [`selectors::Element`] (`selector`)
//!
//! Modelled on Paws' `engine/src/style/dom/*` (stylo 0.13), adapted to the
//! vendored stylo 0.19 trait surface.
//!
//! # Model
//!
//! - **Every node is an element.** There is no separate document or text node: the topmost ancestor
//!   acts as the document root (see [`TNode::owner_doc`](stylo::dom::TNode::owner_doc)), and
//!   character data rides on the element ([`Node::text`](crate::Node::text)).
//! - **`:hover`/`:active`/`:focus`** are matched from the element's
//!   [`ElementState`](crate::ElementState) (unlike Paws, which stubs them to `false`).
//! - **`:root`** matches a parentless element whose
//!   [`ExternalState::is_root`](crate::ExternalState::is_root) hook agrees.
//! - **Synthetic / reflected attributes** beyond the element's real attrs map are served by the
//!   [`ExternalState`](crate::ExternalState) attribute hooks.
//! - **Shadow DOM / pseudo-elements / animations** are stubbed (`None`/`false`) — none exist in
//!   this model yet.
//! - **Snapshots** record matching-relevant pre-mutation state and are consumed by stylo's
//!   invalidation-set pass.
//!
//! # Safety
//!
//! The `unsafe` needed for stylo's interior-mutable per-element state is
//! confined to the `element` implementation; see that module's `SAFETY`
//! notes. A flush freezes document topology and ordinary node data, then stylo
//! may distribute `&Node<T>` references across workers. Its traversal
//! discipline gives one worker ownership of each node's `ElementData`, while
//! cross-worker state is atomic. [`TraversalGuard`](crate::arena::TraversalGuard)
//! enforces the frozen phase and validates its mutation epoch.

mod document;
mod element;
mod node;
mod selector;
