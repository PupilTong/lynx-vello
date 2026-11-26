//! `vant` — the resource policy every Bobcat embedder shares: the cache
//! policy vocabulary, the in-memory bitmap tier, RFC 9111 freshness, MIME
//! sniffing and preprocessing, image header probing, `data:` URLs, image
//! sizing decisions, the HTTP byte tier ([`ByteCache`](byte_cache::ByteCache))
//! over a [`Store`], the derived-artifact tier ([`artifact`]: what an engine
//! derives from a source text, such as a compiled script, shared in-process
//! and persisted over a [`Store`] or an [`AsyncStore`]), the registry of
//! contents the embedder already holds and the vocabulary registrations are
//! written through ([`registry`]), and the resource error vocabulary. It
//! sits below `terminus` and names no DOM, style, runtime or platform type.
//! What it asks of the embedder is four capabilities — a transport
//! ([`Fetch`]), persistent bytes ([`Store`], and for the artifact tier
//! optionally [`AsyncStore`]), platform pixels
//! ([`ImageDecoder`]) and a video player ([`media::Media`]) — bundled as one
//! [`Platform`]; their implementations live with the embedder
//! (`docs/resource-cache-design.md`).

#![warn(clippy::undocumented_unsafe_blocks)]

pub mod artifact;
pub mod byte_cache;
pub mod data_url;
pub mod error;
pub mod http;
pub mod image;
pub mod image_header;
pub mod media;
pub mod memory;
pub mod mime;
pub mod platform;
pub mod preprocess;
pub mod registry;
#[cfg(test)]
pub(crate) mod test_store;

pub use platform::{
    ArtifactStore, AsyncStore, Estimate, Fetch, ImageDecoder, MaybeSend, Meta, NoStore, Parts,
    PartsOf, Platform, Request, Response, Store, StoreError, Threaded, Tier, Volatility,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CachePolicy {
    #[default]
    Default,
    NoStore,
    Reload,
    NoCache,
    ForceCache,
    OnlyIfCached,
}
