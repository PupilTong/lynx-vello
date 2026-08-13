//! Browser WebAssembly embedder for Bobcat.
//!
//! One `wasm32-wasip1-threads` NAPI-RS module owns both Bobcat's Vello/wgpu
//! Canvas renderer and real Rust threads over shared memory. WebGPU objects stay
//! on the browser thread that created them; blocking work runs in Workers.

#[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
mod browser;

#[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
pub use browser::*;

#[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
mod wasi;

#[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
pub use wasi::*;
