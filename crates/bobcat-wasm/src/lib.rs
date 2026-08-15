//! Browser WebAssembly embedder for Bobcat.
//!
//! The browser UI thread only transfers an `OffscreenCanvas` and host events
//! to a dedicated embedder Worker. That Worker initializes this module and
//! permanently owns the opaque Lynx view, Vello/wgpu stack, and canvas. Core
//! owns its nested main-thread VM Worker and Stylo workers; no document or
//! element-tree handle crosses the embedder boundary.

#[cfg(target_arch = "wasm32")]
mod browser;

#[cfg(target_arch = "wasm32")]
pub use browser::*;
