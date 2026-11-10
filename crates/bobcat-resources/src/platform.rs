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
#[cfg(not(target_arch = "wasm32"))]
use vant::Tier;
use vant::{Parts, Platform};

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
/// cache directory when one is configured ([`FsStore`]) with the code
/// cache's own directory inside it, and `ImageIO` or gdk-pixbuf
/// ([`NativeDecoder`]).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct NativePlatform {
    fetch: NativeFetch,
    bytes: Option<Tier<FsStore>>,
    artifacts: Option<Tier<FsStore>>,
    decoder: NativeDecoder,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativePlatform {
    /// The native platform, with the byte tier in `disk` when it is given
    /// and its directory opens, and the code cache in the `code` directory
    /// inside it when that opens. The two are opened, and fail, separately.
    /// The notes say what was left out: a cache directory that could not be
    /// opened, a decoder that cannot be reached.
    #[must_use]
    pub fn new(disk: Option<&crate::DiskCacheConfig>) -> (Self, Vec<String>) {
        let mut notes = Vec::new();
        let mut open = |what: &str, dir: &std::path::Path, budget: u64| match FsStore::open(dir) {
            Ok(store) => Some(Tier { store, budget }),
            Err(error) => {
                notes.push(format!(
                    "the {what} at `{}` could not be opened: {error}",
                    dir.display()
                ));
                None
            }
        };
        let bytes = disk.and_then(|disk| open("disk cache", &disk.dir, disk.budget_bytes));
        let artifacts = disk.and_then(|disk| {
            open(
                "code cache",
                &disk.dir.join(crate::CODE_CACHE_DIR),
                disk.code_budget_bytes,
            )
        });
        let decoder = NativeDecoder;
        if let Err(error) = decoder.available() {
            notes.push(format!("images will not decode: {error}"));
        }
        let platform = Self {
            fetch: NativeFetch,
            bytes,
            artifacts,
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

    fn into_parts(self) -> Parts<NativeFetch, FsStore, NativeDecoder> {
        Parts {
            fetch: self.fetch,
            decoder: self.decoder,
            bytes: self.bytes,
            artifacts: self.artifacts,
        }
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

    /// No Store for either tier: the browser's HTTP cache is the byte
    /// tier, and the code cache keeps its artifacts in memory only.
    fn into_parts(self) -> Parts<BrowserFetch, vant::NoStore, BrowserDecoder> {
        Parts {
            fetch: self.fetch,
            decoder: self.decoder,
            bytes: None,
            artifacts: None,
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::DiskCacheConfig;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "bobcat-resources-platform-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            let _ = std::fs::remove_file(&path);
            Self(path)
        }

        fn disk(&self) -> DiskCacheConfig {
            DiskCacheConfig {
                dir: self.0.clone(),
                budget_bytes: 1 << 20,
                code_budget_bytes: 2 << 20,
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn cache_notes(notes: &[String]) -> Vec<&String> {
        notes
            .iter()
            .filter(|note| note.contains(" cache at "))
            .collect()
    }

    #[test]
    fn the_native_platform_opens_the_byte_tier_and_the_code_cache_inside_it() {
        let dir = TempDir::new("both");
        let (platform, notes) = NativePlatform::new(Some(&dir.disk()));
        assert!(cache_notes(&notes).is_empty(), "{notes:?}");
        let parts = platform.into_parts();
        let bytes = parts.bytes.expect("the byte tier");
        let artifacts = parts.artifacts.expect("the code cache");
        assert_eq!(bytes.store.dir(), dir.0);
        assert_eq!(bytes.budget, 1 << 20);
        assert_eq!(artifacts.store.dir(), dir.0.join("code"));
        assert_eq!(artifacts.budget, 2 << 20);
    }

    #[test]
    fn the_native_platform_reports_each_tier_that_does_not_open_on_its_own() {
        let dir = TempDir::new("code-blocked");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.0.join("code"), b"a file, not a directory").unwrap();
        let (platform, notes) = NativePlatform::new(Some(&dir.disk()));
        let notes = cache_notes(&notes);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with("the code cache at `"), "{notes:?}");
        let parts = platform.into_parts();
        assert!(parts.bytes.is_some());
        assert!(parts.artifacts.is_none());

        let file = TempDir::new("both-blocked");
        std::fs::write(&file.0, b"a file where the cache directory goes").unwrap();
        let (platform, notes) = NativePlatform::new(Some(&file.disk()));
        let notes = cache_notes(&notes);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].starts_with("the disk cache at `"), "{notes:?}");
        assert!(notes[1].starts_with("the code cache at `"), "{notes:?}");
        let parts = platform.into_parts();
        assert!(parts.bytes.is_none());
        assert!(parts.artifacts.is_none());
    }

    #[test]
    fn without_a_disk_configuration_neither_tier_is_opened() {
        let (platform, notes) = NativePlatform::new(None);
        assert!(cache_notes(&notes).is_empty());
        let parts = platform.into_parts();
        assert!(parts.bytes.is_none());
        assert!(parts.artifacts.is_none());
    }
}
