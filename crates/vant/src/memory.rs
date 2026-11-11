//! The in-memory tier: a least-recently-used cache under a byte budget, with
//! a pinned working set that eviction may never take.
//!
//! The budget is what "control the memory the cache holds" means here. It
//! is best-effort by design: the frame being drawn needs its images resident
//! every time it composes, so evicting one of them buys nothing but a
//! re-decode on the next read, and a single entry larger than the whole
//! budget still has to live somewhere while it is on screen. Eviction
//! therefore takes unpinned entries, least recently used first, until the
//! budget holds — and reports what it took, because the owner may have a
//! backing store to note the loss against.
//!
//! Entry counts are small — a page names tens of images, not thousands — so
//! recency is a monotonically increasing stamp per entry and eviction is a
//! scan for the smallest, which is simpler than an intrusive list and
//! costs nothing anyone will measure.

use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};

/// A least-recently-used cache keyed by source string, budgeted in bytes.
pub struct MemoryCache<V> {
    entries: FxHashMap<Arc<str>, Entry<V>>,
    pinned: FxHashSet<Arc<str>>,
    budget: usize,
    used: usize,
    clock: u64,
}

struct Entry<V> {
    value: V,
    bytes: usize,
    last_used: u64,
}

/// What an insert displaced: the previous value under the same key, and the
/// entries evicted to make room.
#[derive(Debug)]
pub struct Insertion<V> {
    pub replaced: Option<V>,
    pub evicted: Vec<(Arc<str>, V)>,
}

impl<V> std::fmt::Debug for MemoryCache<V> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryCache")
            .field("entries", &self.entries.len())
            .field("pinned", &self.pinned.len())
            .field("budget", &self.budget)
            .field("used", &self.used)
            .finish_non_exhaustive()
    }
}

impl<V> MemoryCache<V> {
    #[must_use]
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            entries: FxHashMap::default(),
            pinned: FxHashSet::default(),
            budget: budget_bytes,
            used: 0,
            clock: 0,
        }
    }

    #[must_use]
    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Changes the budget, evicting whatever no longer fits.
    pub fn set_budget(&mut self, budget_bytes: usize) -> Vec<(Arc<str>, V)> {
        self.budget = budget_bytes;
        self.trim()
    }

    /// Whether an entry of `bytes` would fit inside the budget on its own.
    #[must_use]
    pub fn fits(&self, bytes: usize) -> bool {
        bytes <= self.budget
    }

    /// Inserts or replaces `key`, then trims. The entry just inserted is
    /// never evicted by its own insert: the caller asked for it to be
    /// resident now, and an oversized one is the caller's decision to make
    /// (see [`Self::fits`]).
    pub fn insert(&mut self, key: Arc<str>, value: V, bytes: usize) -> Insertion<V> {
        let stamp = self.tick();
        let replaced = self.entries.remove(&key).map(|entry| {
            self.used -= entry.bytes;
            entry.value
        });
        self.used += bytes;
        let spared = Arc::clone(&key);
        self.entries.insert(
            key,
            Entry {
                value,
                bytes,
                last_used: stamp,
            },
        );
        let evicted = self.trim_sparing(Some(&spared));
        Insertion { replaced, evicted }
    }

    /// Reads `key`, marking it most recently used.
    pub fn get(&mut self, key: &str) -> Option<&V> {
        let stamp = self.tick();
        let entry = self.entries.get_mut(key)?;
        entry.last_used = stamp;
        Some(&entry.value)
    }

    /// Reads `key` mutably, marking it most recently used.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        let stamp = self.tick();
        let entry = self.entries.get_mut(key)?;
        entry.last_used = stamp;
        Some(&mut entry.value)
    }

    /// Reads `key` without touching recency.
    #[must_use]
    pub fn peek(&self, key: &str) -> Option<&V> {
        self.entries.get(key).map(|entry| &entry.value)
    }

    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn remove(&mut self, key: &str) -> Option<V> {
        let entry = self.entries.remove(key)?;
        self.used -= entry.bytes;
        Some(entry.value)
    }

    /// Replaces the pinned set with `keys` — the sources the frame is
    /// drawing — and trims, since what was pinned before may be evictable
    /// now. A pinned key that is absent stays pinned, so an insert of it
    /// later is protected from the start.
    pub fn pin(&mut self, keys: &[Arc<str>]) -> Vec<(Arc<str>, V)> {
        self.pinned.clear();
        self.pinned.extend(keys.iter().cloned());
        self.trim()
    }

    #[must_use]
    pub fn is_pinned(&self, key: &str) -> bool {
        self.pinned.contains(key)
    }

    #[must_use]
    pub fn used_bytes(&self) -> usize {
        self.used
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Evicts unpinned entries, least recently used first, until the budget
    /// holds; returns them.
    pub fn trim(&mut self) -> Vec<(Arc<str>, V)> {
        self.trim_sparing(None)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Arc<str>, &V)> {
        self.entries.iter().map(|(key, entry)| (key, &entry.value))
    }

    fn trim_sparing(&mut self, spared: Option<&Arc<str>>) -> Vec<(Arc<str>, V)> {
        let mut evicted = Vec::new();
        while self.used > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(key, _)| {
                    !self.pinned.contains(*key) && spared.is_none_or(|spared| spared != *key)
                })
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| Arc::clone(key));
            let Some(victim) = victim else {
                break;
            };
            let entry = self
                .entries
                .remove(&victim)
                .expect("the victim was just found");
            self.used -= entry.bytes;
            evicted.push((victim, entry.value));
        }
        evicted
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> Arc<str> {
        Arc::from(name)
    }

    fn keys(evicted: &[(Arc<str>, u32)]) -> Vec<&str> {
        evicted.iter().map(|(key, _)| &**key).collect()
    }

    #[test]
    fn eviction_is_least_recently_used_first() {
        let mut cache = MemoryCache::new(30);
        assert!(cache.insert(key("a"), 1, 10).evicted.is_empty());
        assert!(cache.insert(key("b"), 2, 10).evicted.is_empty());
        assert!(cache.insert(key("c"), 3, 10).evicted.is_empty());
        assert_eq!(
            cache.get("a"),
            Some(&1),
            "touching `a` makes `b` the oldest"
        );
        let insertion = cache.insert(key("d"), 4, 10);
        assert_eq!(keys(&insertion.evicted), ["b"]);
        assert_eq!(cache.used_bytes(), 30);
        assert_eq!(cache.len(), 3);
        assert!(cache.peek("b").is_none());
    }

    #[test]
    fn the_inserted_entry_survives_its_own_insert_even_when_oversized() {
        let mut cache = MemoryCache::new(10);
        cache.insert(key("small"), 1, 4);
        assert!(!cache.fits(50));
        let insertion = cache.insert(key("huge"), 2, 50);
        assert_eq!(
            keys(&insertion.evicted),
            ["small"],
            "everything else goes first"
        );
        assert!(
            cache.contains("huge"),
            "but the entry asked for is resident"
        );
        assert_eq!(cache.used_bytes(), 50);
        assert!(
            cache.trim().iter().any(|(key, _)| &**key == "huge"),
            "a later trim may take it"
        );
        assert!(cache.is_empty());
    }

    #[test]
    fn pinned_entries_are_never_evicted_and_pins_apply_to_later_inserts() {
        let mut cache = MemoryCache::new(20);
        cache.pin(&[key("frame"), key("later")]);
        cache.insert(key("frame"), 1, 15);
        cache.insert(key("other"), 2, 15);
        assert!(cache.contains("frame"));
        assert!(cache.contains("other"), "the insert itself is spared");
        assert_eq!(cache.used_bytes(), 30);
        let evicted = cache.insert(key("later"), 3, 15);
        assert_eq!(keys(&evicted.evicted), ["other"]);
        assert!(
            cache.contains("later"),
            "pinned before it existed, protected on arrival"
        );
        assert_eq!(
            cache.used_bytes(),
            30,
            "two pinned entries may exceed the budget"
        );
        assert!(cache.trim().is_empty(), "nothing unpinned is left to take");
        let evicted = cache.pin(&[]);
        assert_eq!(evicted.len(), 1, "unpinning makes the oldest evictable");
    }

    #[test]
    fn replacement_accounts_bytes_once_and_hands_back_the_old_value() {
        let mut cache = MemoryCache::new(100);
        cache.insert(key("a"), 1, 10);
        let insertion = cache.insert(key("a"), 2, 30);
        assert_eq!(insertion.replaced, Some(1));
        assert_eq!(cache.used_bytes(), 30);
        assert_eq!(cache.remove("a"), Some(2));
        assert_eq!(cache.used_bytes(), 0);
        assert_eq!(cache.remove("a"), None);
    }

    #[test]
    fn shrinking_the_budget_evicts_and_peek_does_not_touch_recency() {
        let mut cache = MemoryCache::new(30);
        cache.insert(key("a"), 1, 10);
        cache.insert(key("b"), 2, 10);
        cache.insert(key("c"), 3, 10);
        assert_eq!(cache.peek("a"), Some(&1));
        let evicted = cache.set_budget(15);
        assert_eq!(keys(&evicted), ["a", "b"], "peek left `a` the oldest");
        assert_eq!(cache.budget(), 15);
        assert_eq!(cache.iter().count(), 1);
        *cache.get_mut("c").expect("resident") = 30;
        assert_eq!(cache.peek("c"), Some(&30));
    }
}
