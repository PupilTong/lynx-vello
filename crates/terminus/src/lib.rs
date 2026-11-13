#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// `hint::likely` is still unstable, on the newest nightly as much as the
// pinned one. It marks the branch every tree walk runs into — the staleness
// check in `TreeArenas` — so the live path stays fall-through.
#![feature(likely_unlikely)]
#![deny(unreachable_pub)]
// Proving a wgpu handle `Send`/`Sync` walks wgpu-core's whole context type
// (`Texture` → `CoreTexture` → `ContextWgpuCore` → ...), deeper than the
// default limit of 128; nightly reports that overflow as the future-error
// `recursion_depth_exceeding_limit` (rust-lang/rust#159228).
#![recursion_limit = "256"]
// The crate's whole `unsafe` surface is five blocks — the arena set's owning
// pointer (`tree/arena.rs`'s `ArenaBox`: lending the arena set shared, lending
// it exclusive, freeing it), the node backpointer deref that reads through a
// copy of that pointer, and Stylo's `TElement::ensure_data` contract call —
// and each states the invariant it rests on; a sixth, test-only one hands wgpu
// a bare `CAMetalLayer` for the window stack's test (`painter/graphics.rs`). The
// workspace-wide `unsafe_code = "warn"` says a block may exist; this says it
// must explain itself.
#![warn(clippy::undocumented_unsafe_blocks)]

//! `terminus` — a generic, stylo-integrated W3C-DOM-subset document tree.
//!
//! Its one public module is [`link`], the paths an embedder names; every
//! other module is private.

#[cfg(test)]
extern crate self as terminus;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_common;

mod clock;
mod editing;
mod event;
mod html;
mod input;
mod layout;
pub mod link;
mod paint;
mod painter;
mod render;
mod scroll;
mod style;
mod svg;
mod tree;
mod visual;

use vello_gpu::{kurbo, peniko};
