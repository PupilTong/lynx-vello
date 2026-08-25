//! An in-memory [`dom::ImageStore`] for tests and benchmarks.
//!
//! Production stores fetch, decode, cache and evict; this one does none of
//! that. It holds exactly the pixels a test put in it, so a render either
//! draws them or proves it looked the image up under the wrong source. Its
//! [`ImageStore::get`] never fetches — it answers from the same map `peek`
//! reads, and reports a missing source as an error — which is what makes it
//! usable from a test with no async runtime at all.

use std::collections::HashMap;
use std::sync::Mutex;

use dom::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use dom::{ImageFuture, ImageStore};

/// Decoded images keyed by the source string the paint walk asks for.
#[derive(Debug, Default)]
pub struct TestImages {
    images: Mutex<HashMap<String, ImageData>>,
}

impl TestImages {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes `image` under `source`, replacing any previous entry.
    ///
    /// Takes `&self` because a store is installed on a document behind an
    /// `Arc` and a test still has to change what it answers afterwards.
    pub fn insert(&self, source: impl Into<String>, image: ImageData) {
        self.lock().insert(source.into(), image);
    }

    /// Publishes tightly packed, row-major, straight-alpha RGBA8 pixels.
    ///
    /// # Panics
    ///
    /// If `pixels` is not exactly `width * height * 4` bytes.
    pub fn insert_rgba8(
        &self,
        source: impl Into<String>,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) {
        assert_eq!(
            pixels.len(),
            width as usize * height as usize * 4,
            "an RGBA8 buffer must be width * height * 4 bytes"
        );
        self.insert(source, rgba8(width, height, pixels));
    }

    /// Drops the entry for `source`, so later lookups miss.
    pub fn remove(&self, source: &str) {
        self.lock().remove(source);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, ImageData>> {
        self.images.lock().expect("test image store")
    }
}

impl ImageStore for TestImages {
    fn peek(&self, source: &str) -> Option<ImageData> {
        self.lock().get(source).cloned()
    }

    fn get<'a>(&'a self, source: &'a str) -> ImageFuture<'a> {
        Box::pin(async move {
            self.peek(source)
                .ok_or_else(|| format!("no test image was published for {source}").into())
        })
    }

    fn prefetch(&self, _source: &str) {}
}

/// Wraps tightly packed, row-major, straight-alpha RGBA8 pixels as the
/// `peniko` image a store hands the paint walk.
#[must_use]
pub fn rgba8(width: u32, height: u32, pixels: Vec<u8>) -> ImageData {
    ImageData {
        data: Blob::from(pixels),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    }
}
