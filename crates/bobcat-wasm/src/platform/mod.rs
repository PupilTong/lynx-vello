//! The browser's [`Platform`]: what `bobcat-resources` is built over in the
//! Render Worker.
//!
//! - [`BrowserFetch`]: the Render Worker's own `fetch`, whose HTTP cache is the only byte tier.
//! - [`BrowserDecoder`]: the Render Worker's own `createImageBitmap` and `WebCodecs`
//!   `ImageDecoder`, whose frames are [`BrowserFrame`]s: `ImageBitmap`s the painter copies straight
//!   into a texture.
//! - [`BrowserMedia`]: the video player, an `HTMLVideoElement` on the page's main thread whose
//!   `VideoFrame`s are transferred here and copied into a texture the same way.
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
mod frame;
mod media;

pub use cache::CacheStore;
pub use decode::BrowserDecoder;
pub use fetch::BrowserFetch;
pub use frame::BrowserFrame;
pub use media::{BrowserMedia, BrowserPlayer, deliver_media_event, deliver_media_frame};
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

/// The browser's Render Worker: its own `fetch` ([`BrowserFetch`]), its own
/// image decoders ([`BrowserDecoder`]), the facade's `<video>` elements
/// ([`BrowserMedia`]), and, when the Cache API can be opened, the code
/// cache's store ([`CacheStore`]).
#[derive(Debug)]
pub struct BrowserPlatform {
    fetch: BrowserFetch,
    decoder: BrowserDecoder,
    media: BrowserMedia,
    artifacts: Option<Tier<CacheStore>>,
}

impl BrowserPlatform {
    /// The browser platform, decoding images in this Worker, playing videos
    /// through `media`, and keeping compiled scripts in `code_cache`'s cache
    /// of the Cache API.
    ///
    /// The cache is opened once, here; a second view in this Worker shares it
    /// through `Resources::new_scope`, and another tab's Worker opens the
    /// same cache. Every failure is a note, never an error: without
    /// `createImageBitmap` no image decodes, and without the Cache API (a
    /// context that is not secure, a browser that refuses it in private
    /// browsing) the code cache keeps its artifacts in memory only.
    pub async fn new(
        code_cache: Option<CodeCacheConfig>,
        media: BrowserMedia,
    ) -> (Self, Vec<String>) {
        let mut notes = Vec::new();
        let decoder = BrowserDecoder::new();
        if let Err(error) = vant::ImageDecoder::available(&decoder) {
            notes.push(format!("images will not decode: {error}"));
        }
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
            media,
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
    type Media = BrowserMedia;

    /// No byte tier: the browser's HTTP cache plays that role
    /// ([`BrowserFetch`] owns it). The code cache gets the Cache API store
    /// when it opened.
    fn into_parts(self) -> Parts<BrowserFetch, NoStore, CacheStore, BrowserDecoder, BrowserMedia> {
        Parts {
            fetch: self.fetch,
            decoder: self.decoder,
            media: self.media,
            bytes: None,
            artifacts: self.artifacts.map(|tier| Tier {
                store: ArtifactStore::Async(tier.store),
                budget: tier.budget,
            }),
        }
    }
}
