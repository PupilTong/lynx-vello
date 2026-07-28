//! Bounded caches for decoded pixels and for the much cheaper natural sizes.
//!
//! Two caches rather than one, because the two things they hold differ by four
//! orders of magnitude in size and by rather more than that in usefulness per
//! byte. Evicting a bitmap costs a re-decode; evicting a natural size costs a
//! re-layout of everything below the node, which is the jank Lynx's own
//! bitmap-size cache exists to avoid.

use std::collections::BTreeMap;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::decode::{ImageHeader, PixelSize};
use crate::pixels::DecodedImage;

/// What one decode-cache entry is keyed on.
///
/// The source string is the host's `ResolvedLocator::cache_key` when the fetcher
/// supplied one and the resolved URL otherwise — never the pre-resolution
/// specifier, since two specifiers can resolve to one resource and a host's
/// rewrite hook is entitled to make them.
///
/// The decode target is part of the key because a 100 px and a 400 px decode of
/// one URL are different images, and handing a caller the wrong one would show
/// up as a silently blurry or silently oversized frame.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    source: Arc<str>,
    target: Option<PixelSize>,
}

impl CacheKey {
    #[must_use]
    pub fn new(source: impl Into<Arc<str>>, target: Option<PixelSize>) -> Self {
        Self {
            source: source.into(),
            target,
        }
    }

    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// An exact LRU bounded by **total decoded bytes**, not entry count.
///
/// Entry count is the wrong bound here: a 16x16 icon and a 4000x3000 photo are
/// one entry each and differ by five orders of magnitude in cost, so a
/// count-bounded cache either wastes memory or thrashes depending entirely on
/// what the page happens to contain.
///
/// Hand-rolled rather than the `lru` crate for the same reason — a byte budget
/// does not fit `LruCache::new(NonZeroUsize)`, and building on `unbounded()`
/// plus manual eviction leaves the crate supplying only an intrusive list.
#[derive(Debug)]
pub struct DecodeCache {
    entries: FxHashMap<CacheKey, Entry>,
    /// Recency index, oldest first. A `BTreeMap` keyed on a monotonic tick
    /// gives O(log n) eviction of the true least-recently-used entry without
    /// the unsafe intrusive list a hand-written O(1) LRU would need.
    recency: BTreeMap<u64, CacheKey>,
    clock: u64,
    bytes: u64,
    budget: u64,
}

#[derive(Debug)]
struct Entry {
    image: DecodedImage,
    tick: u64,
}

impl DecodeCache {
    #[must_use]
    pub fn with_budget(max_bytes: u64) -> Self {
        Self {
            entries: FxHashMap::default(),
            recency: BTreeMap::new(),
            clock: 0,
            bytes: 0,
            budget: max_bytes,
        }
    }

    /// Fetches and marks recently used. The returned image shares its buffer
    /// with the cached one, so this is a handle clone rather than a copy.
    pub fn get(&mut self, key: &CacheKey) -> Option<DecodedImage> {
        let tick = self.next_tick();
        let entry = self.entries.get_mut(key)?;
        self.recency.remove(&entry.tick);
        entry.tick = tick;
        self.recency.insert(tick, key.clone());
        Some(entry.image.clone())
    }

    /// Inserts, evicting least-recently-used entries until the budget holds.
    ///
    /// An image larger than the entire budget is **not** inserted, and evicts
    /// nothing: admitting it would flush every useful entry to store something
    /// that cannot be kept anyway.
    pub fn insert(&mut self, key: CacheKey, image: DecodedImage) {
        let size = image.byte_len() as u64;
        if size > self.budget {
            return;
        }
        self.remove(&key);
        while self.bytes + size > self.budget {
            if !self.evict_oldest() {
                break;
            }
        }
        let tick = self.next_tick();
        self.recency.insert(tick, key.clone());
        self.entries.insert(key, Entry { image, tick });
        self.bytes += size;
    }

    pub fn remove(&mut self, key: &CacheKey) -> Option<DecodedImage> {
        let entry = self.entries.remove(key)?;
        self.recency.remove(&entry.tick);
        self.bytes -= entry.image.byte_len() as u64;
        Some(entry.image)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.recency.clear();
        self.bytes = 0;
    }

    #[must_use]
    pub const fn byte_len(&self) -> u64 {
        self.bytes
    }

    #[must_use]
    pub const fn budget(&self) -> u64 {
        self.budget
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn evict_oldest(&mut self) -> bool {
        let Some((&tick, _)) = self.recency.iter().next() else {
            return false;
        };
        let Some(key) = self.recency.remove(&tick) else {
            return false;
        };
        if let Some(entry) = self.entries.remove(&key) {
            self.bytes -= entry.image.byte_len() as u64;
        }
        true
    }

    fn next_tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

/// Natural sizes, bounded by entry count.
///
/// Sized far larger than [`DecodeCache`] on purpose: an entry is tens of bytes
/// against megabytes for a bitmap, and it buys something the pixels cannot. A
/// second mount of a known URL can publish its natural size in the same commit
/// that creates the node, so the very first frame lays out final — no 0x0 frame
/// and no relayout when the decode lands.
#[derive(Debug)]
pub struct HeaderCache {
    entries: FxHashMap<Arc<str>, (ImageHeader, u64)>,
    recency: BTreeMap<u64, Arc<str>>,
    clock: u64,
    capacity: usize,
}

impl HeaderCache {
    #[must_use]
    pub fn with_capacity(entries: usize) -> Self {
        Self {
            entries: FxHashMap::default(),
            recency: BTreeMap::new(),
            clock: 0,
            capacity: entries.max(1),
        }
    }

    pub fn get(&mut self, source: &str) -> Option<ImageHeader> {
        self.clock += 1;
        let tick = self.clock;
        let (header, stored) = self.entries.get_mut(source)?;
        let header = *header;
        let previous = *stored;
        *stored = tick;
        let key = self.recency.remove(&previous)?;
        self.recency.insert(tick, key);
        Some(header)
    }

    pub fn insert(&mut self, source: impl Into<Arc<str>>, header: ImageHeader) {
        let source = source.into();
        if let Some((_, tick)) = self.entries.remove(&source) {
            self.recency.remove(&tick);
        }
        while self.entries.len() >= self.capacity {
            let Some((&tick, _)) = self.recency.iter().next() else {
                break;
            };
            if let Some(key) = self.recency.remove(&tick) {
                self.entries.remove(&key);
            }
        }
        self.clock += 1;
        self.recency.insert(self.clock, Arc::clone(&source));
        self.entries.insert(source, (header, self.clock));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.recency.clear();
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{CacheKey, DecodeCache, HeaderCache};
    use crate::decode::{ImageHeader, PixelSize};
    use crate::format::ImageFormat;
    use crate::pixels::{AlphaType, DecodedImage};

    /// `side * side * 4` bytes.
    fn image(side: u32) -> DecodedImage {
        DecodedImage::from_rgba8(
            side,
            side,
            AlphaType::Straight,
            vec![0u8; (side * side * 4) as usize],
            ImageFormat::Png,
        )
        .expect("well-formed buffer")
    }

    fn key(source: &str) -> CacheKey {
        CacheKey::new(source, None)
    }

    fn header(width: u32) -> ImageHeader {
        ImageHeader {
            format: ImageFormat::Png,
            natural_size: PixelSize { width, height: 1 },
            has_alpha: false,
            animated: false,
        }
    }

    #[test]
    fn tracks_bytes_rather_than_entries() {
        let mut cache = DecodeCache::with_budget(1 << 20);
        cache.insert(key("a"), image(4)); // 64 bytes
        cache.insert(key("b"), image(8)); // 256 bytes
        assert_eq!(cache.byte_len(), 64 + 256);
        assert_eq!(cache.len(), 2);

        cache.remove(&key("a"));
        assert_eq!(cache.byte_len(), 256);
        cache.clear();
        assert_eq!(cache.byte_len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn evicts_the_least_recently_used_entry_first() {
        // Budget fits exactly two 64-byte images.
        let mut cache = DecodeCache::with_budget(128);
        cache.insert(key("a"), image(4));
        cache.insert(key("b"), image(4));
        // Touch "a" so "b" becomes the oldest.
        assert!(cache.get(&key("a")).is_some());
        cache.insert(key("c"), image(4));

        assert!(cache.get(&key("a")).is_some(), "recently used survives");
        assert!(
            cache.get(&key("b")).is_none(),
            "least recently used evicted"
        );
        assert!(cache.get(&key("c")).is_some());
        assert_eq!(cache.byte_len(), 128);
    }

    #[test]
    fn an_entry_larger_than_the_budget_is_refused_without_flushing_the_cache() {
        let mut cache = DecodeCache::with_budget(128);
        cache.insert(key("keep"), image(4));
        cache.insert(key("huge"), image(16)); // 1024 bytes

        assert!(cache.get(&key("huge")).is_none());
        assert!(
            cache.get(&key("keep")).is_some(),
            "an unusable entry must not evict a usable one"
        );
    }

    #[test]
    fn reinserting_a_key_replaces_rather_than_double_counts() {
        let mut cache = DecodeCache::with_budget(1 << 20);
        cache.insert(key("a"), image(4));
        cache.insert(key("a"), image(8));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.byte_len(), 256);
    }

    #[test]
    fn the_decode_target_is_part_of_the_key() {
        let mut cache = DecodeCache::with_budget(1 << 20);
        let small = CacheKey::new(
            "one-url",
            Some(PixelSize {
                width: 100,
                height: 100,
            }),
        );
        let large = CacheKey::new(
            "one-url",
            Some(PixelSize {
                width: 400,
                height: 400,
            }),
        );
        cache.insert(small.clone(), image(4));
        cache.insert(large.clone(), image(8));

        assert_eq!(cache.len(), 2, "two decodes of one URL coexist");
        assert_eq!(cache.get(&small).expect("small").width(), 4);
        assert_eq!(cache.get(&large).expect("large").width(), 8);
        assert_eq!(small.source(), large.source());
    }

    #[test]
    fn header_cache_evicts_by_recency_within_its_capacity() {
        let mut cache = HeaderCache::with_capacity(2);
        cache.insert("a", header(1));
        cache.insert("b", header(2));
        assert!(cache.get("a").is_some());
        cache.insert("c", header(3));

        assert!(cache.get("a").is_some(), "recently used survives");
        assert!(cache.get("b").is_none(), "least recently used evicted");
        assert_eq!(cache.get("c").expect("c").natural_size.width, 3);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn header_cache_replaces_an_existing_source() {
        let mut cache = HeaderCache::with_capacity(4);
        cache.insert("a", header(1));
        cache.insert("a", header(9));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get("a").expect("a").natural_size.width, 9);
        cache.clear();
        assert!(cache.is_empty());
    }
}
