//! Decoded raster images the paint engine draws.
//!
//! Decoding stays outside this crate (`bobcat-engine`'s resource protocol
//! delivers bytes; the embedder decodes). Painters look images up by the two
//! key spaces CSS produces: a `url(…)` string for `background-image` /
//! `mask-image` layers, and a `NodeId` for a replaced element's content
//! (the node whose layout used a decoded `NaturalSize`).
//!
//! Missing entries paint nothing — the layout-side natural size and the
//! paint-side pixels arrive independently, so a frame between the two just
//! skips the image, matching the browser's not-yet-loaded state.

use rustc_hash::FxHashMap;
use vello::peniko::ImageData;
use w3c_dom::NodeId;

/// Frame-independent image registry, keyed by CSS url and by replaced node.
#[derive(Debug, Default)]
pub struct ImageStore {
    by_url: FxHashMap<String, ImageData>,
    by_node: FxHashMap<NodeId, ImageData>,
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
    pub fn insert_node(&mut self, node: NodeId, image: ImageData) {
        self.by_node.insert(node, image);
    }

    pub fn remove_url(&mut self, url: &str) -> Option<ImageData> {
        self.by_url.remove(url)
    }

    pub fn remove_node(&mut self, node: NodeId) -> Option<ImageData> {
        self.by_node.remove(&node)
    }

    #[must_use]
    pub fn url(&self, url: &str) -> Option<&ImageData> {
        self.by_url.get(url)
    }

    #[must_use]
    pub fn node(&self, node: NodeId) -> Option<&ImageData> {
        self.by_node.get(&node)
    }
}
