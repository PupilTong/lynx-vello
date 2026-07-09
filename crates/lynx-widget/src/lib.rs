//! `lynx-widget` — the Widget Element-PAPI layer of **lynx-vello**.
//!
//! lynx-vello is a from-scratch native Rust reimplementation of the `LynxJS`
//! web-bundle runtime (see `AGENTS.md` for the full mission). This crate is the
//! thin PAPI surface Lynx's JS Element API is shaped after: [`WidgetTree`]
//! validates opcode semantics (stale handles, cycles, insertion references, the
//! `unique_id` minting + index, the `css_id` batch) and delegates the actual
//! tree mutation, coarse invalidation, and inline-style parsing to the
//! [`stylo_dom`] crate — the generic, HTML-DOM-subset core this crate embeds.
//!
//! # Vocabulary
//!
//! A Lynx-Element-PAPI-created instance is a [`Widget`] in this repo. The
//! `stylo-dom` core speaks HTML DOM (`Element<T>`, generic over an external
//! state); the Lynx layer instantiates it with [`WidgetState`] — the
//! Lynx-specific payload carrying the [`WidgetKind`], `unique_id`, `css_id`,
//! `data-*` dataset, and event bindings — and names the result Widget-first:
//!
//! - [`Widget`] = `stylo_dom::Element<WidgetState>`
//! - [`WidgetId`] = `stylo_dom::ElementId`
//! - [`WidgetRef`] = `stylo_dom::ElementRef<'_, WidgetState>`
//!
//! The retained tree is a [`WidgetTree`], errors are [`WidgetError`]. Method
//! names keep the `element` wording of the `__*Element` PAPI opcodes they
//! mirror (see [`papi`]).
//!
//! # Layout
//!
//! - [`papi`] — the [`WidgetTree`] type and its Element-PAPI-shaped methods, plus [`WidgetError`].
//! - [`kind`] — the Lynx tag-name ↔ [`WidgetKind`] mapping.
//! - [`state`] — [`WidgetState`] (the `ExternalState` payload) and the event-registration types.

pub mod kind;
pub mod papi;
pub mod state;

pub use kind::WidgetKind;
pub use papi::{WidgetError, WidgetTree};
pub use state::{EventKind, EventReg, WidgetState};
pub use stylo_dom::PseudoState;

/// A Lynx widget: the generic HTML-DOM-subset element carrying the
/// Lynx-specific [`WidgetState`] payload in its `ext` field.
pub type Widget = stylo_dom::Element<WidgetState>;

/// A stable, generation-checked handle to a [`Widget`] in a [`WidgetTree`].
pub type WidgetId = stylo_dom::ElementId;

/// A `Copy` read-only navigation handle over a [`Widget`] and its arena (the
/// type the stylo element traits are implemented on).
pub type WidgetRef<'a> = stylo_dom::ElementRef<'a, WidgetState>;
