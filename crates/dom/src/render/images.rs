//! Decoded raster images the paint engine draws.
//!
//! Decoding stays outside this crate: `bobcat-core`'s resource protocol
//! delivers bytes and the engine's image pipeline turns them into `peniko::ImageData`
//! (`DecodedImage::to_image_data`). The DOM paint pipeline looks images up by the two
//! key spaces CSS produces: a `url(…)` string for `background-image` /
//! `mask-image` layers, and an opaque `usize` owner key for replaced content.
//! The DOM layer uses its private node id as that key without making this
//! lower-level registry depend on DOM types.
//!
//! Missing entries paint nothing — the layout-side natural size and the
//! paint-side pixels arrive independently, so a frame between the two just
//! skips the image, matching the browser's not-yet-loaded state.

use rustc_hash::FxHashMap;
use vello::peniko::ImageData;

/// Frame-independent image registry, keyed by CSS url and by replaced node.
#[derive(Debug, Default)]
pub struct ImageStore {
    by_url: FxHashMap<String, ImageData>,
    by_node: FxHashMap<usize, ImageData>,
}

impl ImageStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the decoded pixels for a `url(…)` image value, replacing
    /// any previous entry.
    pub fn insert_url(&mut self, url: impl Into<String>, image: ImageData) {
        self.by_url.insert(url.into(), image);
    }

    /// Registers the decoded content of a replaced element, replacing any
    /// previous entry.
    pub fn insert_node(&mut self, node: usize, image: ImageData) {
        self.by_node.insert(node, image);
    }

    pub fn remove_url(&mut self, url: &str) -> Option<ImageData> {
        self.by_url.remove(url)
    }

    pub fn remove_node(&mut self, node: usize) -> Option<ImageData> {
        self.by_node.remove(&node)
    }

    #[must_use]
    pub fn url(&self, url: &str) -> Option<&ImageData> {
        self.by_url.get(url)
    }

    #[must_use]
    pub fn node(&self, node: usize) -> Option<&ImageData> {
        self.by_node.get(&node)
    }
}
