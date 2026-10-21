#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// `hint::likely` is still unstable, on the newest nightly as much as the
// pinned one. It marks the branch every tree walk runs into — the staleness
// check in `TreeArenas` — so the live path stays fall-through.
#![feature(likely_unlikely)]
#![deny(unreachable_pub)]
// The crate's whole `unsafe` surface is two blocks — the arena-set backpointer
// deref and Stylo's `TElement::ensure_data` contract call — and each states the
// invariant it rests on. The workspace-wide `unsafe_code = "warn"` says a block
// may exist; this says it must explain itself.
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
mod tree;
mod visual;
