//! The [`Platform`]s this crate ships: [`NativePlatform`] on macOS and
//! Linux, [`BrowserPlatform`] in the browser's Render Worker. Either is the
//! target's [`DefaultPlatform`](crate::DefaultPlatform), which
//! [`Resources::new`](crate::Resources::new) builds with no persistent
//! store; an embedder with a disk cache builds one itself and hands it to
//! [`Resources::with_platform`](crate::Resources::with_platform).
//!
//! Building a platform never fails on anything the embedder chose: a store
//! that cannot be opened, or a decoder that cannot be reached, is left out
//! and described in the notes `new` returns, which the resource system
//! reports through [`Resources::take_notes`](crate::Resources::take_notes).

#[cfg(not(target_arch = "wasm32"))]
use vant::ImageDecoder as _;
use vant::Platform;

#[cfg(target_arch = "wasm32")]
use crate::decode::BrowserDecoder;
#[cfg(not(target_arch = "wasm32"))]
use crate::decode::NativeDecoder;
#[cfg(not(target_arch = "wasm32"))]
use crate::store::FsStore;
#[cfg(target_arch = "wasm32")]
use crate::transport::BrowserFetch;
#[cfg(not(target_arch = "wasm32"))]
use crate::transport::NativeFetch;

/// macOS and Linux: libcurl and the local filesystem ([`NativeFetch`]), a
/// cache directory when one is configured ([`FsStore`]), and `ImageIO` or
/// gdk-pixbuf ([`NativeDecoder`]).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct NativePlatform {
    fetch: NativeFetch,
    store: Option<FsStore>,
    budget: u64,
    decoder: NativeDecoder,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativePlatform {
    /// The native platform, with the byte tier in `disk` when it is given
    /// and its directory opens. The notes say what was left out: a cache
    /// directory that could not be opened, a decoder that cannot be reached.
    #[must_use]
    pub fn new(disk: Option<&crate::DiskCacheConfig>) -> (Self, Vec<String>) {
        let mut notes = Vec::new();
        let store = disk.and_then(|disk| match FsStore::open(&disk.dir) {
            Ok(store) => Some(store),
            Err(error) => {
                notes.push(format!(
                    "the disk cache at `{}` could not be opened: {error}",
                    disk.dir.display()
                ));
                None
            }
        });
        let decoder = NativeDecoder;
        if let Err(error) = decoder.available() {
            notes.push(format!("images will not decode: {error}"));
        }
        let platform = Self {
            fetch: NativeFetch,
            store,
            budget: disk.map_or(u64::MAX, |disk| disk.budget_bytes),
            decoder,
        };
        (platform, notes)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Platform for NativePlatform {
    type Fetch = NativeFetch;
    type Store = FsStore;
    type Decoder = NativeDecoder;

    fn into_parts(self) -> (NativeFetch, Option<FsStore>, NativeDecoder) {
        (self.fetch, self.store, self.decoder)
    }

    fn byte_budget(&self) -> u64 {
        self.budget
    }
}

/// The browser's Render Worker: its own `fetch` ([`BrowserFetch`]), whose
/// HTTP cache is the only byte tier, no [`Store`](vant::Store), and the main
/// thread's `Image` element ([`BrowserDecoder`]).
#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
pub struct BrowserPlatform {
    fetch: BrowserFetch,
    decoder: BrowserDecoder,
}

#[cfg(target_arch = "wasm32")]
impl BrowserPlatform {
    /// The browser platform, decoding through `image_port` — this Worker's
    /// end of the channel whose other end the host's main-thread image
    /// decoder listens on (`js/image-decoder.ts` in the `bobcat-wasm`
    /// package). Without it no image decodes, which the notes say.
    #[must_use]
    pub fn new(image_port: Option<web_sys::MessagePort>) -> (Self, Vec<String>) {
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
        let platform = Self {
            fetch: BrowserFetch,
            decoder,
        };
        (platform, notes)
    }
}

#[cfg(target_arch = "wasm32")]
impl Platform for BrowserPlatform {
    type Fetch = BrowserFetch;
    type Store = vant::NoStore;
    type Decoder = BrowserDecoder;

    fn into_parts(self) -> (BrowserFetch, Option<vant::NoStore>, BrowserDecoder) {
        (self.fetch, None, self.decoder)
    }
}
