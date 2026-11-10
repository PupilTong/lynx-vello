//! The [`Store`](vant::Store)s this crate ships: [`FsStore`], a cache
//! directory, natively. The browser has none yet; its `fetch` keeps its own
//! HTTP cache.

#[cfg(not(target_arch = "wasm32"))]
mod fs;

#[cfg(not(target_arch = "wasm32"))]
pub use fs::{FsStore, default_cache_dir, hash_key};
