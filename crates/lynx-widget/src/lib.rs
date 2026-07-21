//! `lynx-widget` — the Widget Element-PAPI layer of **lynx-vello**.
//!
//! lynx-vello is a from-scratch native Rust reimplementation of the `LynxJS`
//! web-bundle runtime (see `AGENTS.md` for the full mission). This crate is the
//! thin PAPI surface Lynx's JS Element API is shaped after: [`WidgetTree`]
//! validates opcode semantics (stale handles, cycles, insertion references, the
//! `unique_id` minting + index, the `css_id` batch) and delegates every DOM
//! operation to the [`w3c_dom`] crate's [`Document`](w3c_dom::Document) — the
//! generic, W3C-DOM-subset single tree this crate embeds. Widgets are created
//! and mutated exclusively through `Document` methods (the DOM core does not
//! expose node construction), so style invalidation is carried by the
//! operations themselves. This crate also owns the Lynx-specific style
//! adapter: view metrics, viewport-relative `rpx`, and touch-first device
//! policy used to construct each [`w3c_dom::Document`]'s private,
//! standards-oriented cascade.
//!
//! # Vocabulary
//!
//! A Lynx-Element-PAPI-created instance is a [`Widget`] in this repo. The
//! `w3c-dom` core speaks W3C DOM (`Node<T>`, generic over an opaque payload);
//! the Lynx layer instantiates it with [`WidgetState`] — the Lynx-specific
//! payload carrying the [`WidgetKind`], `unique_id`, and event bindings — and
//! names the result Widget-first. CSS scope and dataset values are real DOM
//! attributes, not payload-provided selector state:
//!
//! - [`Widget`] = `w3c_dom::Node<WidgetState>`
//! - [`WidgetRef`] = `&w3c_dom::Node<WidgetState>` (a plain reference — the stylo traits live on
//!   it)
//!
//! The retained tree is a [`WidgetTree`], errors are [`WidgetError`]. Method
//! names keep the `element` wording of the `__*Element` PAPI opcodes they
//! mirror (see [`papi`]).
//!
//! # Layout
//!
//! - [`papi`] — the [`WidgetTree`] type and its Element-PAPI-shaped methods, plus [`WidgetError`].
//! - [`handle`] — [`WidgetHandle`], the canonical handle registry types.
//! - [`kind`] — the Lynx tag-name ↔ [`WidgetKind`] mapping.
//! - [`state`] — the opaque [`WidgetState`] payload and event-registration types.
//! - [`style`] — the Lynx metrics/device adapter around the generic style engine.

pub mod handle;
pub mod kind;
pub mod papi;
pub mod state;
pub mod style;
pub mod ua;

mod ingest;

pub use handle::WidgetHandle;
pub use kind::WidgetKind;
pub use papi::{WidgetError, WidgetTree};
pub use state::{EventKind, EventReg, WidgetState};
pub use style::{EngineMetrics, StyleEngine};
pub use ua::PageConfig;
pub use w3c_dom::layout::{NaturalSize, Size};
pub use w3c_dom::{
    ComputedStyle, ElementState, Parallelism, StylesheetOrigin, property_is_supported,
};

/// A Lynx widget: the generic W3C-DOM-subset node carrying the Lynx-specific
/// [`WidgetState`] payload.
pub type Widget = w3c_dom::Node<WidgetState>;

/// A read-only navigation handle over a [`Widget`]: a plain shared
/// reference (the type stylo's element traits are implemented on).
pub type WidgetRef<'a> = &'a Widget;
