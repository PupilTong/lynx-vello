//! stylo element-trait implementations for [`ElementRef`](crate::ElementRef).
//!
//! stylo drives selector matching and the cascade over any type implementing
//! its element traits. This module wires our arena-backed DOM to that model by
//! implementing, on the `Copy` handle [`ElementRef`](crate::ElementRef) (for
//! any payload `T: `[`ExternalState`](crate::ExternalState)):
//!
//! - [`NodeInfo`](stylo::dom::NodeInfo) + [`TNode`](stylo::dom::TNode) ([`node`])
//! - [`TElement`](stylo::dom::TElement) ([`element`])
//! - [`TDocument`](stylo::dom::TDocument) + [`TShadowRoot`](stylo::dom::TShadowRoot) ([`document`])
//! - [`selectors::Element`] ([`selector`])
//!
//! Modelled on Paws' `engine/src/style/dom/*` (stylo 0.13), adapted to the
//! vendored stylo 0.19 trait surface.
//!
//! # Model
//!
//! - **Every node is an element.** There is no separate document or text node: the topmost ancestor
//!   acts as the document root (see [`TNode::owner_doc`](stylo::dom::TNode::owner_doc)), and
//!   character data rides on the element ([`Element::text`](crate::Element::text)).
//! - **`:hover`/`:active`/`:focus`** are matched from the element's
//!   [`ElementState`](crate::ElementState) (unlike Paws, which stubs them to `false`).
//! - **`:root`** matches a parentless element whose
//!   [`ExternalState::is_root`](crate::ExternalState::is_root) hook agrees.
//! - **Synthetic / reflected attributes** beyond the element's real attrs map are served by the
//!   [`ExternalState`](crate::ExternalState) attribute hooks.
//! - **Shadow DOM / pseudo-elements / animations** are stubbed (`None`/`false`) — none exist in
//!   this model yet.
//! - **Snapshots** are unused: invalidation is coarse (see [`crate::dirty`]), so `has_snapshot()`
//!   is `false` and `handled_snapshot()` a no-op.
//!
//! # Safety
//!
//! The `unsafe` needed for stylo's interior-mutable per-element state is
//! confined to [`element`]; see that module's `SAFETY` notes. The invariant is
//! the crate-wide one: **single-threaded flush** — no element's stylo data is
//! touched concurrently.

mod document;
mod element;
mod node;
mod selector;
