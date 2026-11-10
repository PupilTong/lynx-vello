//! `vant` — the resource policy every Bobcat embedder shares: the cache
//! policy vocabulary, the in-memory bitmap tier, RFC 9111 freshness, MIME
//! sniffing and preprocessing, image header probing, `data:` URLs, image
//! sizing decisions, the HTTP byte tier ([`ByteCache`](byte_cache::ByteCache))
//! over a [`Store`], and the resource error vocabulary. It sits below
//! `terminus` and names no DOM, style, runtime or platform type. What it
//! asks of the embedder is three capabilities — a transport ([`Fetch`]),
//! persistent bytes ([`Store`]) and platform pixels ([`ImageDecoder`]) —
//! bundled as one [`Platform`]; their implementations live with the embedder
//! (`docs/resource-cache-design.md`).

#![warn(clippy::undocumented_unsafe_blocks)]

pub mod byte_cache;
pub mod data_url;
pub mod error;
pub mod http;
pub mod image;
pub mod image_header;
pub mod memory;
pub mod mime;
pub mod platform;
pub mod preprocess;

pub use platform::{
    Estimate, Fetch, ImageDecoder, Meta, NoStore, Platform, Request, Response, Store, StoreError,
    Threaded, Volatility,
};

/// What a load may do with the caches on its way.
///
/// The fetcher's own vocabulary: every load names one, the byte tier turns it
/// into an RFC 9111 plan ([`plan`](crate::http::plan)), and the browser transport maps it
/// onto a `fetch` request's cache mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CachePolicy {
    /// Ordinary freshness rules: a fresh stored response answers, a stale one
    /// is revalidated.
    #[default]
    Default,
    /// Fetch, and store nothing.
    NoStore,
    /// Fetch unconditionally, ignoring what is stored.
    Reload,
    /// Revalidate whatever is stored before using it.
    NoCache,
    /// Use a stored response however stale, and fetch only without one.
    ForceCache,
    /// Use a stored response however stale, and fail without one.
    OnlyIfCached,
}
