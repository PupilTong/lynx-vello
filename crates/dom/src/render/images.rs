//! Decoded raster images the paint engine draws.
//!
//! Decoding stays outside this crate: `bobcat-core`'s resource protocol
//! delivers bytes and the engine's image pipeline turns them into `peniko::ImageData`
//! (`DecodedImage::to_image_data`). The DOM paint pipeline looks images up by the two
//! key spaces CSS produces: a `url(…)` string for `background-image` /
//! `mask-image` layers, and an opaque `u64` owner key for replaced content.
//! The DOM layer uses its private node id as that key without making this
//! lower-level registry depend on DOM types.
//!
//! Missing entries paint nothing — the layout-side natural size and the
//! paint-side pixels arrive independently, so a frame between the two just
//! skips the image, matching the browser's not-yet-loaded state.
//!
//! # Retention
//!
//! Entries are CPU-side pixel buffers. Vello's own atlas residency is a
//! separate cache with its own eviction (`vello_encoding`'s `ImageCache`
//! drops entries unused for two resolve passes), so nothing here controls GPU
//! memory; what this registry controls is how long the decoded bytes stay
//! reachable on the CPU.
//!
//! Two key spaces, two retention rules, because only one of them has a
//! decidable end of life:
//!
//! - **Replaced content** is keyed by an owner the caller can test for liveness. Once that owner is
//!   gone the key can never be presented to [`ImageStore::node`] again, so the entry is unreachable
//!   and dropping it cannot change any future frame. [`ImageStore::retain_nodes`] drops exactly
//!   those, driven by a predicate the DOM-aware caller supplies.
//! - **`url(…)` entries** have no such test. A style change can reference any url at any time, and
//!   this crate cannot re-decode — decoding lives above it and is push-only into this registry — so
//!   an entry dropped while its url is still reachable renders as nothing, permanently. Url entries
//!   are therefore never dropped implicitly. [`ImageStore::remove_url`] and
//!   [`ImageStore::retain_urls`] let the layer that owns decoding drop them, and
//!   [`ImageStore::url_bytes`] reports the pressure that decision needs.
//!   [`ImageStore::set_url_budget`] switches on least-recently-used eviction above a byte cap for
//!   an embedder that can re-register on demand; it is off by default.
//!
//! Recency comes from [`ImageStore::begin_frame`] plus a per-entry frame stamp
//! written by the `&self` lookups, because the paint walk holds the registry
//! immutably. The stamp counts painted frames — scene builds — not presents:
//! a document that does not repaint produces no new usage information and
//! ages nothing.
//!
//! # Registrations vello cannot render
//!
//! Vello packs every scene image into one shared square atlas that grows by
//! doubling to 8192 px ([`MAX_RENDERABLE_DIMENSION`]). An image longer than
//! that on either axis never fits at any atlas size: vello's resolve pass
//! leaves it unallocated and zeroes the draw's dimensions, so it renders as
//! nothing with no error anywhere. Storing such a buffer costs the memory and
//! buys no pixels, so registration refuses it, drops any pixels previously
//! registered under that key, and records the refusal for
//! [`ImageStore::oversized`] / [`ImageStore::oversized_count`], and writes one
//! line to standard error.
//!
//! Both channels exist because neither alone reaches the failure. This crate
//! has no logging dependency, so the record is the only channel a program can
//! read back, and nothing in the engine reads it — the stderr line is what a
//! person building a page actually sees. It is not an assertion: the pixels
//! come from decoded remote content through the embedder's public image
//! registration, so an image past the bound is data, not a programming error,
//! and must not stop the process in any build. The line is emitted at most
//! [`MAX_OVERSIZED_REPORTS`] times per registry so a caller re-registering the
//! same image every frame cannot flood the stream.

use std::cell::Cell;

use rustc_hash::FxHashMap;
use vello::peniko::ImageData;

/// Largest per-axis pixel count vello can render.
///
/// Vello's image atlas starts at 1024 px and doubles until an image fits,
/// stopping at `vello_encoding`'s `MAX_ATLAS_SIZE` of 8192. An image within
/// this bound may still fail to allocate transiently when the atlas is full
/// of other images — vello resolves that itself by evicting or growing —
/// but an image past it can never be placed at all.
pub const MAX_RENDERABLE_DIMENSION: u32 = 8192;

/// Painted frames an url entry must go unused before budget eviction may
/// drop it.
///
/// Two, matching the atlas cache's own `EVICT_AFTER_GENERATIONS`: this
/// registry has no reason to discard a buffer sooner than the layer it feeds.
const URL_EVICTION_MIN_IDLE_FRAMES: u64 = 2;

/// Rejected registrations retained for read-back.
///
/// Bounded, because a caller that re-registers the same oversized image every
/// frame must not turn the diagnostic into the leak it reports.
/// [`ImageStore::oversized_count`] keeps counting past the bound.
const MAX_OVERSIZED_REPORTS: usize = 16;

/// Which key space a registration used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageKey {
    /// A CSS `url(…)` value.
    Url(String),
    /// A replaced-element owner key.
    Node(u64),
}

impl std::fmt::Display for ImageKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(url) => write!(formatter, "url({url})"),
            Self::Node(node) => write!(formatter, "replaced element {node}"),
        }
    }
}

/// One registration refused because vello could never render it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OversizedImage {
    /// The key the pixels were offered under.
    pub key: ImageKey,
    /// Registered width in pixels.
    pub width: u32,
    /// Registered height in pixels.
    pub height: u32,
}

/// One registered image plus the retention state the registry keeps for it.
#[derive(Debug)]
struct Entry {
    image: ImageData,
    /// Retained pixel bytes, read once at registration: the buffer behind an
    /// `ImageData` is immutable, so this cannot drift.
    bytes: u64,
    /// Painted frame of the most recent lookup, or of registration.
    ///
    /// A [`Cell`] rather than an atomic or a lock because the paint walk
    /// holds the whole registry through a shared reference and never leaves
    /// the thread that owns the document. `Cell` needs no `unsafe`; it makes
    /// [`ImageStore`] `!Sync`, which changes nothing, because the registry
    /// lives inside the document's `RefCell<Painter>` and that is already
    /// `!Sync`. It stays `Send`, which is what sharing the document behind a
    /// mutex requires.
    last_used_frame: Cell<u64>,
}

impl Entry {
    fn new(image: ImageData, frame: u64) -> Self {
        let bytes = u64::try_from(image.data.data().len()).unwrap_or(u64::MAX);
        Self {
            image,
            bytes,
            last_used_frame: Cell::new(frame),
        }
    }

    fn touch(&self, frame: u64) -> &ImageData {
        self.last_used_frame.set(frame);
        &self.image
    }
}

/// Image registry keyed by CSS url and by replaced node, stamping each entry
/// with the painted frame that last looked it up.
#[derive(Debug, Default)]
pub struct ImageStore {
    by_url: FxHashMap<String, Entry>,
    by_node: FxHashMap<u64, Entry>,
    url_bytes: u64,
    node_bytes: u64,
    url_budget: Option<u64>,
    frame: u64,
    oversized: Vec<OversizedImage>,
    oversized_count: u64,
}

impl ImageStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a new painted frame, so lookups made during it stamp a new
    /// value.
    pub(crate) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// The painted-frame counter the lookups stamp entries with.
    #[must_use]
    pub(crate) const fn frame_index(&self) -> u64 {
        self.frame
    }

    /// Registers the decoded pixels for a `url(…)` image value, replacing
    /// any previous entry.
    ///
    /// Pixels past [`MAX_RENDERABLE_DIMENSION`] are refused: the previous
    /// entry is still dropped, because the registration supersedes it, and
    /// the refusal is recorded for [`ImageStore::oversized`].
    pub fn insert_url(&mut self, url: impl Into<String>, image: ImageData) {
        let url = url.into();
        self.remove_url(&url);
        if !self.accepts(&image, || ImageKey::Url(url.clone())) {
            return;
        }
        let entry = Entry::new(image, self.frame);
        self.url_bytes += entry.bytes;
        self.by_url.insert(url, entry);
        self.enforce_url_budget();
    }

    /// Registers the decoded content of a replaced element, replacing any
    /// previous entry.
    ///
    /// Same refusal rule as [`ImageStore::insert_url`].
    pub fn insert_node(&mut self, node: u64, image: ImageData) {
        self.remove_node(node);
        if !self.accepts(&image, || ImageKey::Node(node)) {
            return;
        }
        let entry = Entry::new(image, self.frame);
        self.node_bytes += entry.bytes;
        self.by_node.insert(node, entry);
    }

    pub fn remove_url(&mut self, url: &str) -> Option<ImageData> {
        let entry = self.by_url.remove(url)?;
        self.url_bytes = self.url_bytes.saturating_sub(entry.bytes);
        Some(entry.image)
    }

    pub fn remove_node(&mut self, node: u64) -> Option<ImageData> {
        let entry = self.by_node.remove(&node)?;
        self.node_bytes = self.node_bytes.saturating_sub(entry.bytes);
        Some(entry.image)
    }

    /// Drops every replaced-content entry whose owner key `keep` rejects.
    ///
    /// The caller decides what liveness means, so this registry keeps its
    /// owner keys opaque. The paint side passes a predicate that resolves the
    /// key back to a document node and asks whether that node still exists;
    /// a key whose node is gone can never be looked up again, so dropping its
    /// entry cannot change a later frame.
    pub fn retain_nodes(&mut self, mut keep: impl FnMut(u64) -> bool) {
        let bytes = &mut self.node_bytes;
        self.by_node.retain(|&node, entry| {
            if keep(node) {
                return true;
            }
            *bytes = bytes.saturating_sub(entry.bytes);
            false
        });
    }

    /// Drops every `url(…)` entry whose url `keep` rejects.
    ///
    /// Only the layer that can decode again should call this: this crate
    /// cannot re-create a dropped entry, so a url still reachable from a
    /// computed style renders as nothing once its pixels are gone.
    pub fn retain_urls(&mut self, mut keep: impl FnMut(&str) -> bool) {
        let bytes = &mut self.url_bytes;
        self.by_url.retain(|url, entry| {
            if keep(url) {
                return true;
            }
            *bytes = bytes.saturating_sub(entry.bytes);
            false
        });
    }

    /// Drops every entry in both key spaces.
    pub fn clear(&mut self) {
        self.by_url.clear();
        self.by_node.clear();
        self.url_bytes = 0;
        self.node_bytes = 0;
    }

    /// Sets a byte cap on `url(…)` pixels, above which the least recently
    /// used entries are dropped, or `None` to never drop them implicitly.
    ///
    /// `None` is the default and the only setting that cannot blank a page:
    /// decoding lives above this crate and only pushes into this registry, so
    /// a dropped url is gone until the embedder registers it again. Set a cap
    /// only from a layer that will re-register on demand, and set it to that
    /// layer's own decoded-image budget, so every entry this registry drops is
    /// one the layer above can still hand back without fetching or decoding
    /// again.
    ///
    /// The cap is not a hard limit. An entry used within the last
    /// [`URL_EVICTION_MIN_IDLE_FRAMES`] painted frames is never dropped, so a
    /// document whose visible images alone exceed the cap exceeds it rather
    /// than losing pixels that are on screen.
    pub fn set_url_budget(&mut self, budget: Option<u64>) {
        self.url_budget = budget;
        self.enforce_url_budget();
    }

    /// Retained `url(…)` pixel bytes.
    ///
    /// Two entries sharing one decoded buffer are counted once each, so this
    /// never reports less than the registry actually holds.
    #[must_use]
    pub const fn url_bytes(&self) -> u64 {
        self.url_bytes
    }

    /// Retained replaced-content pixel bytes, counted like
    /// [`ImageStore::url_bytes`].
    #[must_use]
    pub const fn node_bytes(&self) -> u64 {
        self.node_bytes
    }

    /// Entry counts: `url(…)` values, then replaced-element owners.
    #[must_use]
    pub fn len(&self) -> (usize, usize) {
        (self.by_url.len(), self.by_node.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_url.is_empty() && self.by_node.is_empty()
    }

    /// The most recent registrations refused for exceeding
    /// [`MAX_RENDERABLE_DIMENSION`], oldest first.
    ///
    /// Capped; [`ImageStore::oversized_count`] counts every refusal.
    #[must_use]
    pub fn oversized(&self) -> &[OversizedImage] {
        &self.oversized
    }

    /// How many registrations have been refused for exceeding
    /// [`MAX_RENDERABLE_DIMENSION`].
    #[must_use]
    pub const fn oversized_count(&self) -> u64 {
        self.oversized_count
    }

    /// Forgets the recorded refusals, leaving
    /// [`ImageStore::oversized_count`] alone.
    pub fn clear_oversized(&mut self) {
        self.oversized.clear();
    }

    #[must_use]
    pub fn url(&self, url: &str) -> Option<&ImageData> {
        Some(self.by_url.get(url)?.touch(self.frame))
    }

    #[must_use]
    pub fn node(&self, node: u64) -> Option<&ImageData> {
        Some(self.by_node.get(&node)?.touch(self.frame))
    }

    /// Whether these pixels can be rendered at all, reporting the refusal
    /// when they cannot.
    ///
    /// The key is built lazily so the accepted path allocates nothing.
    fn accepts(&mut self, image: &ImageData, key: impl FnOnce() -> ImageKey) -> bool {
        if image.width <= MAX_RENDERABLE_DIMENSION && image.height <= MAX_RENDERABLE_DIMENSION {
            return true;
        }
        self.refuse(image, key);
        false
    }

    /// Records and reports one refused registration.
    ///
    /// Split out and marked cold so the size check `accepts` performs on every
    /// registration stays a single comparison pair with no reporting code in
    /// its inlined body.
    #[cold]
    #[inline(never)]
    fn refuse(&mut self, image: &ImageData, key: impl FnOnce() -> ImageKey) {
        self.oversized_count = self.oversized_count.saturating_add(1);
        if self.oversized.len() >= MAX_OVERSIZED_REPORTS {
            return;
        }
        let key = key();
        // Both channels are bounded by the same cap, so the stream cannot be
        // flooded and the read-back cannot grow into the leak it reports.
        eprintln!(
            "dom: refused a {}x{} image registered for {key}: vello renders \
             nothing past {MAX_RENDERABLE_DIMENSION}px per axis",
            image.width, image.height,
        );
        self.oversized.push(OversizedImage {
            key,
            width: image.width,
            height: image.height,
        });
    }

    fn enforce_url_budget(&mut self) {
        let Some(budget) = self.url_budget else {
            return;
        };
        if self.url_bytes <= budget {
            return;
        }
        let frame = self.frame;
        let mut idle: Vec<(u64, &str)> = self
            .by_url
            .iter()
            .filter(|(_, entry)| {
                frame.saturating_sub(entry.last_used_frame.get()) >= URL_EVICTION_MIN_IDLE_FRAMES
            })
            .map(|(url, entry)| (entry.last_used_frame.get(), url.as_str()))
            .collect();
        // The url breaks ties so eviction order does not depend on hash order.
        idle.sort_unstable();
        let victims: Vec<String> = idle.into_iter().map(|(_, url)| url.to_owned()).collect();
        for url in victims {
            if self.url_bytes <= budget {
                break;
            }
            self.remove_url(&url);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

    use super::{ImageKey, ImageStore, MAX_OVERSIZED_REPORTS, MAX_RENDERABLE_DIMENSION};

    /// An image with the given declared size and an unrelated buffer length,
    /// so a test can name a large image without allocating one.
    fn image(width: u32, height: u32, bytes: usize) -> ImageData {
        ImageData {
            data: Blob::from(vec![0_u8; bytes]),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        }
    }

    fn pixel() -> ImageData {
        image(1, 1, 4)
    }

    /// Registers pixels vello cannot render. The refusal returns normally in
    /// every build: the pixels are decoded content, not a caller mistake.
    fn register_oversized(store: &mut ImageStore, key: &ImageKey, image: ImageData) {
        match key {
            ImageKey::Url(url) => store.insert_url(url.clone(), image),
            ImageKey::Node(node) => store.insert_node(*node, image),
        }
    }

    #[test]
    fn registration_round_trips_through_both_key_spaces() {
        let mut store = ImageStore::new();
        store.insert_url("app:///a.png", pixel());
        store.insert_node(7, pixel());

        assert!(store.url("app:///a.png").is_some());
        assert!(store.node(7).is_some());
        assert!(store.url("app:///missing.png").is_none());
        assert!(store.node(8).is_none());
        assert_eq!(store.len(), (1, 1));
        assert_eq!((store.url_bytes(), store.node_bytes()), (4, 4));
    }

    #[test]
    fn byte_accounting_follows_replacement_and_removal() {
        let mut store = ImageStore::new();
        store.insert_url("app:///a.png", image(2, 2, 16));
        assert_eq!(store.url_bytes(), 16);

        store.insert_url("app:///a.png", image(1, 1, 4));
        assert_eq!(store.url_bytes(), 4, "replacement releases the old buffer");
        assert_eq!(store.len().0, 1);

        assert!(store.remove_url("app:///a.png").is_some());
        assert_eq!(store.url_bytes(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn a_registration_at_the_atlas_bound_is_kept() {
        let mut store = ImageStore::new();
        store.insert_url("app:///wide.png", image(MAX_RENDERABLE_DIMENSION, 1, 4));
        store.insert_node(1, image(1, MAX_RENDERABLE_DIMENSION, 4));

        assert!(store.url("app:///wide.png").is_some());
        assert!(store.node(1).is_some());
        assert_eq!(store.oversized_count(), 0);
    }

    #[test]
    fn an_oversized_registration_is_refused_and_recorded() {
        let mut store = ImageStore::new();
        let key = ImageKey::Url("app:///a.png".to_owned());
        store.insert_url("app:///a.png", pixel());

        register_oversized(
            &mut store,
            &key,
            image(1, MAX_RENDERABLE_DIMENSION + 1, 4_096),
        );

        assert!(
            store.url("app:///a.png").is_none(),
            "the refused registration still supersedes the pixels it replaced"
        );
        assert_eq!(store.url_bytes(), 0);
        assert_eq!(store.oversized_count(), 1);
        assert_eq!(store.oversized()[0].key, key);
        assert_eq!(
            (store.oversized()[0].width, store.oversized()[0].height),
            (1, MAX_RENDERABLE_DIMENSION + 1)
        );

        store.clear_oversized();
        assert!(store.oversized().is_empty());
        assert_eq!(store.oversized_count(), 1, "the count survives a clear");
    }

    #[test]
    fn the_oversized_record_is_bounded_but_the_count_is_not() {
        let mut store = ImageStore::new();
        let overflow = MAX_OVERSIZED_REPORTS as u64 + 5;
        for node in 0..overflow {
            register_oversized(
                &mut store,
                &ImageKey::Node(node),
                image(MAX_RENDERABLE_DIMENSION + 1, 1, 4),
            );
        }

        assert_eq!(store.oversized().len(), MAX_OVERSIZED_REPORTS);
        assert_eq!(store.oversized_count(), overflow);
        assert_eq!(store.oversized()[0].key, ImageKey::Node(0));
        assert!(store.is_empty(), "nothing unrenderable is retained");
    }

    #[test]
    fn retain_nodes_drops_exactly_the_owners_the_predicate_rejects() {
        let mut store = ImageStore::new();
        store.insert_node(1, image(1, 1, 4));
        store.insert_node(2, image(2, 2, 16));
        store.insert_url("app:///a.png", image(1, 1, 4));

        store.retain_nodes(|node| node == 1);

        assert!(store.node(1).is_some());
        assert!(store.node(2).is_none());
        assert_eq!(store.node_bytes(), 4);
        assert!(
            store.url("app:///a.png").is_some(),
            "a node sweep must not touch the url key space"
        );
        assert_eq!(store.url_bytes(), 4);
    }

    #[test]
    fn retain_urls_drops_exactly_the_urls_the_predicate_rejects() {
        let mut store = ImageStore::new();
        store.insert_url("app:///a.png", image(1, 1, 4));
        store.insert_url("app:///b.png", image(2, 2, 16));

        store.retain_urls(|url| url.ends_with("b.png"));

        assert!(store.url("app:///a.png").is_none());
        assert!(store.url("app:///b.png").is_some());
        assert_eq!(store.url_bytes(), 16);
    }

    #[test]
    fn clear_releases_both_key_spaces() {
        let mut store = ImageStore::new();
        store.insert_url("app:///a.png", image(1, 1, 4));
        store.insert_node(1, image(1, 1, 4));

        store.clear();

        assert!(store.is_empty());
        assert_eq!((store.url_bytes(), store.node_bytes()), (0, 0));
    }

    #[test]
    fn without_a_budget_url_entries_are_never_dropped_implicitly() {
        let mut store = ImageStore::new();
        for index in 0..8 {
            store.insert_url(format!("app:///{index}.png"), image(1, 1, 1_024));
        }
        for _ in 0..16 {
            store.begin_frame();
        }
        store.insert_url("app:///last.png", image(1, 1, 1_024));

        assert_eq!(store.len().0, 9);
        assert_eq!(store.url_bytes(), 9 * 1_024);
    }

    #[test]
    fn budget_eviction_drops_the_least_recently_used_url_first() {
        let mut store = ImageStore::new();
        store.set_url_budget(Some(3_072));
        store.insert_url("app:///a.png", image(1, 1, 1_024));
        store.insert_url("app:///b.png", image(1, 1, 1_024));
        store.insert_url("app:///c.png", image(1, 1, 1_024));

        // Three painted frames that look up only two of the three entries.
        for _ in 0..3 {
            store.begin_frame();
            assert!(store.url("app:///a.png").is_some());
            assert!(store.url("app:///c.png").is_some());
        }

        store.insert_url("app:///d.png", image(1, 1, 1_024));

        assert!(store.url("app:///b.png").is_none(), "the idle entry goes");
        assert!(store.url("app:///a.png").is_some());
        assert!(store.url("app:///c.png").is_some());
        assert!(store.url("app:///d.png").is_some());
        assert_eq!(store.url_bytes(), 3_072);
    }

    #[test]
    fn budget_eviction_never_drops_pixels_used_this_frame() {
        let mut store = ImageStore::new();
        store.set_url_budget(Some(1_024));
        store.insert_url("app:///a.png", image(1, 1, 4_096));
        store.insert_url("app:///b.png", image(1, 1, 4_096));

        store.begin_frame();
        assert!(store.url("app:///a.png").is_some());
        assert!(store.url("app:///b.png").is_some());
        store.begin_frame();
        assert!(store.url("app:///a.png").is_some());
        assert!(store.url("app:///b.png").is_some());

        store.set_url_budget(Some(1_024));

        assert_eq!(store.len().0, 2, "visible pixels outrank the byte cap");
        assert!(store.url_bytes() > 1_024);
    }

    #[test]
    fn raising_and_lowering_the_budget_is_applied_immediately() {
        let mut store = ImageStore::new();
        store.insert_url("app:///a.png", image(1, 1, 1_024));
        store.insert_url("app:///b.png", image(1, 1, 1_024));
        for _ in 0..3 {
            store.begin_frame();
        }

        store.set_url_budget(Some(u64::MAX));
        assert_eq!(store.len().0, 2);

        store.set_url_budget(Some(1_024));
        assert_eq!(store.len().0, 1);

        store.set_url_budget(None);
        assert_eq!(store.len().0, 1, "clearing the cap drops nothing further");
    }
}
