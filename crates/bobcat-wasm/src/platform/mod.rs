//! The browser's [`Platform`]: what `bobcat-resources` is built over in the
//! Render Worker.
//!
//! - [`BrowserFetch`]: the Render Worker's own `fetch`, whose HTTP cache is the only byte tier.
//! - [`BrowserDecoder`]: the main thread's `Image` element, over a `MessagePort` and a mailbox in
//!   shared Wasm memory.
//! - The code cache's store: a [`CacheStore`] over one cache of the web Cache API, so compiled
//!   scripts outlive the page and are shared by every tab of the origin.
//!
//! Building the platform never fails: a store that cannot be opened, or a
//! decoder that cannot be reached, is left out and described in the notes
//! [`BrowserPlatform::new`] returns, which the resource system reports
//! through `Resources::take_notes`.

mod cache;
mod decode;
mod fetch;
mod mailbox;

pub use cache::CacheStore;
pub use decode::BrowserDecoder;
pub use fetch::BrowserFetch;
use vant::{ArtifactStore, NoStore, Parts, Platform, Tier};

/// Which cache of the origin's Cache API the code cache keeps its entries
/// in, and how many bytes of entries it may keep there.
#[derive(Clone, Debug)]
pub struct CodeCacheConfig {
    /// The name `caches.open` is called with.
    pub cache_name: String,
    /// The code cache's budget, in stored entry bytes.
    pub budget_bytes: u64,
}

impl Default for CodeCacheConfig {
    /// `bobcat-code-cache`, under 32 MiB.
    fn default() -> Self {
        Self {
            cache_name: "bobcat-code-cache".to_owned(),
            budget_bytes: 32 << 20,
        }
    }
}

/// The browser's Render Worker: its own `fetch` ([`BrowserFetch`]), the main
/// thread's `Image` element ([`BrowserDecoder`]), and, when the Cache API
/// can be opened, the code cache's store ([`CacheStore`]).
#[derive(Debug)]
pub struct BrowserPlatform {
    fetch: BrowserFetch,
    decoder: BrowserDecoder,
    artifacts: Option<Tier<CacheStore>>,
}

impl BrowserPlatform {
    /// The browser platform, decoding through `image_port` — this Worker's
    /// end of the channel whose other end the host's main-thread image
    /// decoder listens on (`js/image-decoder.ts`) — and keeping compiled
    /// scripts in `code_cache`'s cache of the Cache API.
    ///
    /// The cache is opened once, here; a second view in this Worker shares it
    /// through `Resources::new_scope`, and another tab's Worker opens the
    /// same cache. Every failure is a note, never an error: without a port
    /// no image decodes, and without the Cache API (a context that is not
    /// secure, a browser that refuses it in private browsing) the code cache
    /// keeps its artifacts in memory only.
    pub async fn new(
        image_port: Option<web_sys::MessagePort>,
        code_cache: Option<CodeCacheConfig>,
    ) -> (Self, Vec<String>) {
        let mut notes = Vec::new();
        let decoder = if let Some(port) = image_port {
            BrowserDecoder::new(port).unwrap_or_else(|error| {
                notes.push(format!("images will not decode: {error}"));
                BrowserDecoder::none()
            })
        } else {
            notes.push("images will not decode: no image decode port was configured".to_owned());
            BrowserDecoder::none()
        };
        let artifacts = match code_cache {
            Some(config) => match CacheStore::open(&config.cache_name).await {
                Ok(store) => Some(Tier {
                    store,
                    budget: config.budget_bytes,
                }),
                Err(error) => {
                    notes.push(format!("the code cache will not persist: {error}"));
                    None
                }
            },
            None => None,
        };
        let platform = Self {
            fetch: BrowserFetch,
            decoder,
            artifacts,
        };
        (platform, notes)
    }
}

impl Platform for BrowserPlatform {
    type Fetch = BrowserFetch;
    type Store = NoStore;
    type AsyncStore = CacheStore;
    type Decoder = BrowserDecoder;

    /// No byte tier: the browser's HTTP cache plays that role
    /// ([`BrowserFetch`] owns it). The code cache gets the Cache API store
    /// when it opened.
    fn into_parts(self) -> Parts<BrowserFetch, NoStore, CacheStore, BrowserDecoder> {
        Parts {
            fetch: self.fetch,
            decoder: self.decoder,
            bytes: None,
            artifacts: self.artifacts.map(|tier| Tier {
                store: ArtifactStore::Async(tier.store),
                budget: tier.budget,
            }),
        }
    }
}
