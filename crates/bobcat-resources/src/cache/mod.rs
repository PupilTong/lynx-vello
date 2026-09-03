//! The cache tiers: an in-memory LRU with a byte budget, and on native
//! targets a disk cache with HTTP freshness semantics.

pub mod memory;

#[cfg(not(target_arch = "wasm32"))]
pub mod disk;
#[cfg(not(target_arch = "wasm32"))]
pub mod http;
