//! stylo element-trait implementations for [`ElemRef`](crate::ElemRef).
//!
//! stylo drives selector matching and the cascade over any type implementing
//! its element traits. This module wires our arena-backed DOM to that model by
//! implementing, on the `Copy` handle [`ElemRef`](crate::ElemRef):
//!
//! - [`NodeInfo`](stylo::dom::NodeInfo) + [`TNode`](stylo::dom::TNode) ([`node`])
//! - [`TElement`](stylo::dom::TElement) ([`element`])
//! - [`TDocument`](stylo::dom::TDocument) + [`TShadowRoot`](stylo::dom::TShadowRoot) ([`document`])
//! - [`selectors::Element`] ([`selector`])
//!
//! Modelled on Paws' `engine/src/style/dom/*` (stylo 0.13), adapted to the
//! vendored stylo 0.19 trait surface.
//!
//! # Lynx specifics
//!
//! - **Every node is an element.** There is no separate document node: the `<page>` root *is* the
//!   document root, which is what makes `:root` match it and what
//!   [`TNode::owner_doc`](stylo::dom::TNode::owner_doc) returns.
//! - **`:hover`/`:active`/`:focus`** are matched from the node's
//!   [`ElementState`](stylo_dom::ElementState) (unlike Paws, which stubs them to `false`).
//! - **`l-css-id`** is exposed as a synthetic attribute (the element's `css_id`) for the future
//!   scoped-CSS mode.
//! - **Shadow DOM / pseudo-elements / animations** are stubbed (`None`/`false`) — none exist in the
//!   Lynx model yet.
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
