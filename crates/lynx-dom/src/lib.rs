//! `lynx-dom` — the Widget Element-PAPI layer of **lynx-vello**.
//!
//! lynx-vello is a from-scratch native Rust reimplementation of the `LynxJS`
//! web-bundle runtime (see `AGENTS.md` for the full mission). This crate is the
//! thin PAPI surface Lynx's JS Element API is shaped after: [`WidgetTree`]
//! validates opcode semantics (stale handles, cycles, insertion references, the
//! `unique_id` index, the `css_id` batch) and delegates the actual tree
//! mutation, coarse invalidation, and inline-style parsing to the
//! [`stylo_dom`] crate — which carries most of the logic (the generational
//! [`Widget`] arena, the stylo element-trait impls, and the primitives).
//!
//! # Vocabulary
//!
//! A Lynx-Element-PAPI-created instance is a [`Widget`] in this repo — the Lynx
//! layer deliberately does **not** reuse the HTML `Element`/`Node`/`Document`
//! vocabulary for its own types. The retained tree is a [`WidgetTree`], errors
//! are [`WidgetError`], handles are [`WidgetId`]/[`WidgetRef`], and the element
//! kind is a [`WidgetKind`]. Method names, however, keep the `element` wording
//! of the `__*Element` PAPI opcodes they mirror (see [`papi`]).
//!
//! The [`stylo_dom`] vocabulary ([`Widget`], [`WidgetId`], [`WidgetRef`],
//! [`WidgetKind`], [`PseudoState`], [`EventKind`], [`EventReg`]) is re-exported
//! here so `lynx-style` and tests have a single import surface.
//!
//! # Layout
//!
//! - [`papi`] — the [`WidgetTree`] type and its Element-PAPI-shaped methods, plus [`WidgetError`].

pub mod papi;

pub use papi::{WidgetError, WidgetTree};
pub use stylo_dom::{EventKind, EventReg, PseudoState, Widget, WidgetId, WidgetKind, WidgetRef};
