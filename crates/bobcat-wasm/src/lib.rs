//! Browser WebAssembly embedder for Bobcat.
//!
//! The browser UI thread only transfers an `OffscreenCanvas` and host events
//! to a dedicated embedder Worker. That Worker initializes this module and
//! permanently owns the engine, Vello/wgpu stack, and canvas; the engine's
//! unique DOM mutation owner runs on a nested shared-memory `wasm_thread`
//! Worker and communicates through Rust synchronization primitives.

#[cfg(target_arch = "wasm32")]
mod browser;

#[cfg(target_arch = "wasm32")]
pub use browser::*;
