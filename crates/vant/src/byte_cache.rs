//! The HTTP byte tier: fetched bodies kept in a [`Store`] under a byte
//! budget, with the response record that decides whether they are still
//! good.
//!
//! [`ByteCache::lookup`] runs the RFC 9111 plan ([`plan`]) over what is
//! stored and answers with the stored bytes, the request to make, or a
//! refusal; the caller makes the request through its [`Fetch`](crate::Fetch)
//! and hands the answer back with [`ByteCache::store`] or, for a `304`,
//! [`ByteCache::refreshed`].
//!
//! The in-memory index is rebuilt from [`Store::scan`] at open and is a hint
//! afterwards. A read that fails, finds another key's entry (a Store that
//! names entries by a hash may collide), or finds a body of the wrong length
//! is a miss, and the entry is deleted so it cannot mislead a later lookup.
//! Recency lives in the index; the Store hears of it through
//! [`Store::touch`] at most once a day per entry, so a hot entry costs no
//! write per hit.
//!
//! The index is held under one mutex, and Store IO runs under it too. That
//! serialises the tier, which is fine for the traffic a page produces and
//! simpler than anything that is not.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::HeaderMap;
use rustc_hash::FxHashMap;

use crate::CachePolicy;
use crate::http::{Plan, StoredResponse, plan};
use crate::platform::{Meta, Response, Store};

/// How often a hit's recency reaches the Store.
const TOUCH_INTERVAL: Duration = Duration::from_hours(24);

/// The index's view of one stored response.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The entry as the Store keeps it, with `last_used` as this tier last
    /// saw it used.
    pub meta: Meta,
    /// The media type the body was labelled with when it was stored.
    pub media_type: Option<String>,
    pub response: StoredResponse,
}

/// How a body read from the Store is handed over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Read {
    /// Read into memory the caller owns.
    #[default]
    Owned,
    /// Mapped read-only where the Store supports it ([`Store::map_body`]),
    /// for bytes the caller retains; read as [`Owned`](Self::Owned) where
    /// it does not.
    Mapped,
}

/// What a request should do, given what the tier holds for it.
#[derive(Clone, Debug)]
pub enum Lookup {
    /// Serve these stored bytes without a request.
    UseStored(Entry, Bytes),
    /// Make a request.
    Fetch {
        /// The conditional headers a revalidation adds to the request.
        conditional: Option<HeaderMap>,
        /// Whether a `304` answer refreshes the stored entry
        /// ([`ByteCache::refreshed`]).
        revalidating: bool,
        /// Whether the answer may be stored ([`ByteCache::store`]).
        store: bool,
    },
    /// `only-if-cached` with nothing stored: the request cannot be served.
    Unavailable,
}

/// A byte-budgeted cache of HTTP responses over a [`Store`].
pub struct ByteCache<S: Store> {
    store: S,
    budget: u64,
    index: Mutex<Index>,
}

#[derive(Default)]
struct Index {
    slots: FxHashMap<String, Slot>,
    used: u64,
}

struct Slot {
    entry: Entry,
    /// The `last_used` the Store last recorded, which rate-limits
    /// [`Store::touch`].
    persisted: SystemTime,
}

impl<S: Store> std::fmt::Debug for ByteCache<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ByteCache")
            .field("budget", &self.budget)
            .field("len", &self.len())
            .field("used", &self.used_bytes())
            .finish_non_exhaustive()
    }
}

impl<S: Store> ByteCache<S> {
    /// Opens the tier over `store`: indexes every entry [`Store::scan`]
    /// finds, deletes those whose record does not parse, and trims to the
    /// budget, which is `budget_bytes` or the Store's quota when that is
    /// smaller. A scan that fails opens an empty tier.
    #[must_use]
    pub fn open(store: S, budget_bytes: u64) -> Self {
        let budget = store
            .estimate()
            .quota
            .map_or(budget_bytes, |quota| budget_bytes.min(quota));
        let cache = Self {
            store,
            budget,
            index: Mutex::new(Index::default()),
        };
        let mut index = Index::default();
        for meta in cache.store.scan().unwrap_or_default() {
            match StoredResponse::from_record(&meta.record) {
                Some((response, media_type)) => {
                    let slot = Slot {
                        persisted: meta.last_used,
                        entry: Entry {
                            meta,
                            media_type,
                            response,
                        },
                    };
                    index.used += slot.entry.meta.len;
                    if let Some(previous) = index.slots.insert(slot.entry.meta.key.clone(), slot) {
                        index.used -= previous.entry.meta.len;
                    }
                }
                None => {
                    let _ = cache.store.delete(&meta.key);
                }
            }
        }
        cache.trim(&mut index);
        *cache.index() = index;
        cache
    }

    /// The Store the tier is built on.
    #[must_use]
    pub fn store_ref(&self) -> &S {
        &self.store
    }

    #[must_use]
    pub fn budget(&self) -> u64 {
        self.budget
    }

    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.index().used
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.index().slots.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index().slots.is_empty()
    }

    /// The index's entry for `key`, without reading the Store.
    #[must_use]
    pub fn entry(&self, key: &str) -> Option<Entry> {
        self.index().slots.get(key).map(|slot| slot.entry.clone())
    }

    /// Decides how a request for `key` with `policy` is served at `now`.
    ///
    /// A stored answer is read back and verified before it is offered; one
    /// that does not verify is deleted and the request is planned as if
    /// nothing were stored.
    #[must_use]
    pub fn lookup(&self, key: &str, policy: CachePolicy, now: SystemTime, read: Read) -> Lookup {
        let mut index = self.index();
        let stored = index.slots.get(key).map(|slot| &slot.entry.response);
        match plan(policy, stored, now) {
            Plan::UseStored => {
                if let Some(hit) = self.read_hit(&mut index, key, now, read) {
                    return Lookup::UseStored(hit.0, hit.1);
                }
                Self::lookup_without_entry(policy, now)
            }
            Plan::Revalidate => Lookup::Fetch {
                conditional: stored.map(StoredResponse::revalidation_headers),
                revalidating: true,
                store: true,
            },
            planned => Self::from_plan(planned),
        }
    }

    /// Stores `response` under `key` as fetched at `now`, replacing any
    /// existing entry, and evicts least-recently-used entries until the
    /// budget holds. `false` when the response may not be stored, is larger
    /// than the whole budget, or the Store failed to write it.
    pub fn store(
        &self,
        key: &str,
        media_type: Option<&str>,
        response: &Response,
        now: SystemTime,
    ) -> bool {
        let stored = StoredResponse {
            status: response.status,
            headers: response.headers.clone(),
            stored_at: now,
        };
        let len = response.body.len() as u64;
        if !stored.is_storable() || len > self.budget {
            return false;
        }
        let meta = Meta {
            key: key.to_owned(),
            len,
            last_used: now,
            record: stored.to_record(media_type),
        };
        let mut index = self.index();
        if self.store.put(&meta, &response.body).is_err() {
            return false;
        }
        let slot = Slot {
            entry: Entry {
                meta,
                media_type: media_type.map(str::to_owned),
                response: stored,
            },
            persisted: now,
        };
        if let Some(previous) = index.slots.insert(key.to_owned(), slot) {
            index.used -= previous.entry.meta.len;
        }
        index.used += len;
        self.trim(&mut index);
        true
    }

    /// The `304` path: refreshes `key`'s record with the headers the `304`
    /// carried, marks it used at `now`, and reads the body back. `None` when
    /// nothing is stored for `key` or the body no longer verifies, in which
    /// case the entry is gone and the caller fetches again.
    #[must_use]
    pub fn refreshed(
        &self,
        key: &str,
        not_modified: &HeaderMap,
        now: SystemTime,
        read: Read,
    ) -> Option<(Entry, Bytes)> {
        let mut index = self.index();
        let slot = index.slots.get_mut(key)?;
        let response = slot.entry.response.refreshed_by(not_modified, now);
        slot.entry.meta.record = response.to_record(slot.entry.media_type.as_deref());
        slot.entry.meta.last_used = now;
        slot.entry.response = response;
        // Best effort: a record that failed to persist still serves this
        // process, and the stale one on the Store only costs a revalidation.
        if self.store.put_meta(&slot.entry.meta).is_ok() {
            slot.persisted = now;
        }
        self.read_hit(&mut index, key, now, read)
    }

    /// `key`'s body mapped read-only, when it is stored and the Store maps
    /// bodies. A mapping that does not verify drops the entry.
    #[must_use]
    pub fn map_body(&self, key: &str) -> Option<Bytes> {
        let mut index = self.index();
        let len = index.slots.get(key)?.entry.meta.len;
        let bytes = self.store.map_body(key)?;
        if self.verified_mapping(key, len, &bytes) {
            Some(bytes)
        } else {
            self.drop_entry(&mut index, key);
            None
        }
    }

    /// Forgets `key` and deletes it from the Store.
    pub fn remove(&self, key: &str) {
        let mut index = self.index();
        self.drop_entry(&mut index, key);
    }

    /// Forgets every entry and deletes each from the Store.
    pub fn clear(&self) {
        let mut index = self.index();
        for key in index.slots.keys() {
            let _ = self.store.delete(key);
        }
        index.slots.clear();
        index.used = 0;
    }

    fn index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn from_plan(planned: Plan) -> Lookup {
        match planned {
            Plan::Unavailable => Lookup::Unavailable,
            Plan::FetchNoStore => Lookup::Fetch {
                conditional: None,
                revalidating: false,
                store: false,
            },
            Plan::UseStored | Plan::Revalidate | Plan::Fetch => Lookup::Fetch {
                conditional: None,
                revalidating: false,
                store: true,
            },
        }
    }

    /// The answer for a request whose stored entry turned out unusable.
    fn lookup_without_entry(policy: CachePolicy, now: SystemTime) -> Lookup {
        Self::from_plan(plan(policy, None, now))
    }

    /// Reads and verifies `key`'s body, marking the entry used at `now`.
    /// An entry that does not verify is dropped.
    fn read_hit(
        &self,
        index: &mut Index,
        key: &str,
        now: SystemTime,
        read: Read,
    ) -> Option<(Entry, Bytes)> {
        let len = index.slots.get(key)?.entry.meta.len;
        let Some(bytes) = self.read_body(key, len, read) else {
            self.drop_entry(index, key);
            return None;
        };
        let slot = index.slots.get_mut(key).expect("the entry was just found");
        slot.entry.meta.last_used = now;
        let due = now
            .duration_since(slot.persisted)
            .is_ok_and(|elapsed| elapsed >= TOUCH_INTERVAL);
        if due && self.store.touch(key, now).is_ok() {
            slot.persisted = now;
        }
        Some((slot.entry.clone(), bytes))
    }

    /// `key`'s body from the Store, or `None` when it is missing, belongs to
    /// another key, or is not `len` bytes long.
    fn read_body(&self, key: &str, len: u64, read: Read) -> Option<Bytes> {
        if read == Read::Mapped
            && let Some(bytes) = self.store.map_body(key)
        {
            return self.verified_mapping(key, len, &bytes).then_some(bytes);
        }
        match self.store.get(key) {
            Ok(Some((meta, bytes)))
                if meta.key == key && meta.len == len && bytes.len() as u64 == len =>
            {
                Some(bytes)
            }
            _ => None,
        }
    }

    /// Whether a mapped body is `key`'s: a mapping carries no [`Meta`], so
    /// the key is checked against the Store's record of it.
    fn verified_mapping(&self, key: &str, len: u64, bytes: &Bytes) -> bool {
        bytes.len() as u64 == len
            && matches!(
                self.store.get_meta(key),
                Ok(Some(meta)) if meta.key == key && meta.len == len
            )
    }

    fn drop_entry(&self, index: &mut Index, key: &str) {
        if let Some(slot) = index.slots.remove(key) {
            index.used -= slot.entry.meta.len;
        }
        let _ = self.store.delete(key);
    }

    fn trim(&self, index: &mut Index) {
        while index.used > self.budget {
            let victim = index
                .slots
                .values()
                .min_by_key(|slot| slot.entry.meta.last_used)
                .map(|slot| slot.entry.meta.key.clone());
            let Some(victim) = victim else {
                break;
            };
            self.drop_entry(index, &victim);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::UNIX_EPOCH;

    use http::header::{HeaderName, HeaderValue, IF_NONE_MATCH};
    use url::Url;

    use super::*;
    use crate::platform::{Estimate, StoreError, Volatility};

    /// An in-memory [`Store`] with knobs for the failures a real one has.
    /// Clones share their contents, so a test can reopen a tier over the
    /// same entries.
    #[derive(Clone, Default)]
    struct MemStore {
        entries: Arc<Mutex<HashMap<String, (Meta, Bytes)>>>,
        quota: Option<u64>,
        maps: bool,
        fail_put: Arc<AtomicBool>,
        touches: Arc<AtomicUsize>,
    }

    impl MemStore {
        fn entries(&self) -> MutexGuard<'_, HashMap<String, (Meta, Bytes)>> {
            self.entries.lock().unwrap()
        }

        /// Files `meta` and `body` under `slot`, whatever key `meta` names:
        /// a corrupt record, a hash collision, a truncated body.
        fn forge(&self, slot: &str, meta: Meta, body: &[u8]) {
            self.entries()
                .insert(slot.to_owned(), (meta, Bytes::copy_from_slice(body)));
        }

        fn holds(&self, key: &str) -> bool {
            self.entries().contains_key(key)
        }

        fn persisted_last_used(&self, key: &str) -> SystemTime {
            self.entries()[key].0.last_used
        }
    }

    impl Store for MemStore {
        fn scan(&self) -> Result<Vec<Meta>, StoreError> {
            Ok(self
                .entries()
                .values()
                .map(|(meta, _)| meta.clone())
                .collect())
        }

        fn get(&self, key: &str) -> Result<Option<(Meta, Bytes)>, StoreError> {
            Ok(self.entries().get(key).cloned())
        }

        fn get_meta(&self, key: &str) -> Result<Option<Meta>, StoreError> {
            Ok(self.entries().get(key).map(|(meta, _)| meta.clone()))
        }

        fn put(&self, meta: &Meta, body: &[u8]) -> Result<(), StoreError> {
            if self.fail_put.load(Ordering::Relaxed) {
                return Err(StoreError::Other("the disk is full".to_owned()));
            }
            self.forge(&meta.key, meta.clone(), body);
            Ok(())
        }

        fn put_meta(&self, meta: &Meta) -> Result<(), StoreError> {
            match self.entries().get_mut(&meta.key) {
                Some(entry) => {
                    entry.0 = meta.clone();
                    Ok(())
                }
                None => Err(StoreError::Corrupt("no body".to_owned())),
            }
        }

        fn delete(&self, key: &str) -> Result<(), StoreError> {
            self.entries().remove(key);
            Ok(())
        }

        fn touch(&self, key: &str, now: SystemTime) -> Result<(), StoreError> {
            self.touches.fetch_add(1, Ordering::Relaxed);
            if let Some(entry) = self.entries().get_mut(key) {
                entry.0.last_used = now;
            }
            Ok(())
        }

        fn map_body(&self, key: &str) -> Option<Bytes> {
            if self.maps {
                self.entries().get(key).map(|(_, body)| body.clone())
            } else {
                None
            }
        }

        fn estimate(&self) -> Estimate {
            Estimate {
                used: self
                    .entries()
                    .values()
                    .map(|(_, body)| body.len() as u64)
                    .sum(),
                quota: self.quota,
            }
        }

        fn volatility(&self) -> Volatility {
            Volatility::Durable
        }
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                HeaderValue::from_str(value).expect("a header value"),
            );
        }
        headers
    }

    fn response(pairs: &[(&str, &str)], body: &[u8]) -> Response {
        Response {
            url: Url::parse("https://a.test/").expect("a URL"),
            status: 200,
            headers: headers(pairs),
            body: Bytes::copy_from_slice(body),
        }
    }

    const FRESH: &[(&str, &str)] = &[("cache-control", "max-age=100")];

    fn hit(lookup: Lookup) -> (Entry, Bytes) {
        match lookup {
            Lookup::UseStored(entry, bytes) => (entry, bytes),
            other => panic!("expected a hit, got {other:?}"),
        }
    }

    fn is_plain_fetch(lookup: &Lookup) -> bool {
        matches!(
            lookup,
            Lookup::Fetch {
                conditional: None,
                revalidating: false,
                store: true
            }
        )
    }

    #[test]
    fn a_body_and_its_record_round_trip() {
        let store = MemStore::default();
        let cache = ByteCache::open(store.clone(), 1024);
        assert!(cache.store(
            "https://a.test/x.png",
            Some("image/png"),
            &response(
                &[("etag", "\"1\""), ("cache-control", "max-age=5")],
                b"pixels"
            ),
            at(1_000_000),
        ));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.used_bytes(), 6);
        let (entry, bytes) = hit(cache.lookup(
            "https://a.test/x.png",
            CachePolicy::Default,
            at(1_000_001),
            Read::Owned,
        ));
        assert_eq!(&bytes[..], b"pixels");
        assert_eq!(entry.media_type.as_deref(), Some("image/png"));
        assert_eq!(entry.response.headers.get("etag").unwrap(), "\"1\"");
        assert_eq!(entry.response.stored_at, at(1_000_000));
        assert_eq!(entry.meta.last_used, at(1_000_001));
        assert!(is_plain_fetch(&cache.lookup(
            "https://a.test/other",
            CachePolicy::Default,
            at(1_000_001),
            Read::Owned,
        )));

        // A fresh open rebuilds the index from the Store.
        drop(cache);
        let reopened = ByteCache::open(store, 1024);
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened.used_bytes(), 6);
        let entry = reopened.entry("https://a.test/x.png").unwrap();
        assert_eq!(entry.meta.len, 6);
        assert_eq!(entry.media_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn a_hash_collision_reads_as_a_miss_and_is_cleaned_up() {
        let store = MemStore::default();
        let cache = ByteCache::open(store.clone(), 1024);
        assert!(cache.store("https://a.test/one", None, &response(FRESH, b"one"), at(0)));
        // The Store now answers `one` with an entry that is another key's.
        let mut other = cache.entry("https://a.test/one").unwrap().meta;
        other.key = "https://a.test/two".to_owned();
        store.forge("https://a.test/one", other, b"two");
        assert!(is_plain_fetch(&cache.lookup(
            "https://a.test/one",
            CachePolicy::Default,
            at(1),
            Read::Owned,
        )));
        assert!(cache.is_empty(), "the index forgets the entry");
        assert_eq!(cache.used_bytes(), 0);
        assert!(!store.holds("https://a.test/one"), "and the Store drops it");
    }

    #[test]
    fn corruption_is_recovered_from_on_open_and_on_read() {
        let store = MemStore::default();
        let cache = ByteCache::open(store.clone(), 1024);
        for (key, body) in [("a", &b"aaaa"[..]), ("b", b"bb"), ("c", b"cc")] {
            assert!(cache.store(key, None, &response(FRESH, body), at(0)));
        }
        let mut corrupt = cache.entry("b").unwrap().meta;
        corrupt.record = serde_json::json!("{not a record");
        store.forge("b", corrupt, b"bb");
        drop(cache);

        let reopened = ByteCache::open(store.clone(), 1024);
        assert_eq!(reopened.len(), 2, "the corrupt record's entry is gone");
        assert!(!store.holds("b"));
        assert_eq!(reopened.used_bytes(), 6);

        // A body that vanished after open reads as a miss and drops the entry.
        store.delete("a").unwrap();
        assert!(is_plain_fetch(&reopened.lookup(
            "a",
            CachePolicy::Default,
            at(1),
            Read::Mapped
        )));
        assert!(reopened.entry("a").is_none());

        // So does a body whose length disagrees with its record.
        let meta = reopened.entry("c").unwrap().meta;
        store.forge("c", meta, b"c");
        assert!(matches!(
            reopened.lookup("c", CachePolicy::OnlyIfCached, at(1), Read::Owned),
            Lookup::Unavailable
        ));
        assert!(reopened.is_empty());
        assert_eq!(reopened.used_bytes(), 0);
        assert!(!store.holds("c"));
    }

    #[test]
    fn eviction_takes_the_least_recently_used_and_oversized_entries_are_refused() {
        let cache = ByteCache::open(MemStore::default(), 10);
        assert!(cache.store("a", None, &response(FRESH, b"aaaa"), at(1)));
        assert!(cache.store("b", None, &response(FRESH, b"bbbb"), at(2)));
        hit(cache.lookup("a", CachePolicy::Default, at(3), Read::Owned));
        assert!(cache.store("c", None, &response(FRESH, b"cccc"), at(4)));
        assert!(
            cache.entry("b").is_none(),
            "using `a` made `b` the oldest, so it is evicted"
        );
        assert!(cache.entry("a").is_some());
        assert!(cache.entry("c").is_some());
        assert_eq!(cache.used_bytes(), 8);
        assert!(!cache.store("huge", None, &response(FRESH, &[0; 11]), at(5)));
        assert!(!cache.store_ref().holds("huge"));
        assert_eq!(cache.used_bytes(), 8);
    }

    #[test]
    fn replacement_update_remove_and_clear_keep_the_accounting_straight() {
        let store = MemStore::default();
        let cache = ByteCache::open(store.clone(), 1024);
        assert!(cache.store("k", None, &response(FRESH, b"12345"), at(0)));
        assert!(cache.store("k", Some("text/plain"), &response(FRESH, b"12"), at(1)));
        assert_eq!(cache.used_bytes(), 2);
        assert_eq!(
            &hit(cache.lookup("k", CachePolicy::Default, at(2), Read::Owned)).1[..],
            b"12"
        );
        let (entry, bytes) = cache
            .refreshed(
                "k",
                &headers(&[("etag", "\"new\""), ("cache-control", "max-age=100")]),
                at(3),
                Read::Owned,
            )
            .expect("refreshed");
        assert_eq!(entry.response.headers.get("etag").unwrap(), "\"new\"");
        assert_eq!(
            &bytes[..],
            b"12",
            "the body is untouched by a record update"
        );
        assert_eq!(entry.media_type.as_deref(), Some("text/plain"));
        assert_eq!(cache.used_bytes(), 2);
        assert!(
            cache
                .refreshed("missing", &HeaderMap::new(), at(3), Read::Owned)
                .is_none()
        );
        assert!(cache.store("j", None, &response(FRESH, b"j"), at(4)));
        cache.remove("k");
        assert_eq!(cache.used_bytes(), 1);
        assert!(!store.holds("k"));
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.used_bytes(), 0);
        assert!(store.entries().is_empty());
    }

    #[test]
    fn a_fresh_entry_is_served_and_a_stale_one_is_revalidated_with_its_validators() {
        let cache = ByteCache::open(MemStore::default(), 1024);
        assert!(cache.store(
            "k",
            None,
            &response(
                &[("cache-control", "max-age=10"), ("etag", "\"v1\"")],
                b"body"
            ),
            at(100),
        ));
        hit(cache.lookup("k", CachePolicy::Default, at(109), Read::Owned));
        match cache.lookup("k", CachePolicy::Default, at(110), Read::Owned) {
            Lookup::Fetch {
                conditional: Some(conditional),
                revalidating: true,
                store: true,
            } => assert_eq!(conditional.get(IF_NONE_MATCH).unwrap(), "\"v1\""),
            other => panic!("expected a revalidation, got {other:?}"),
        }
        assert!(matches!(
            cache.lookup("k", CachePolicy::NoStore, at(109), Read::Owned),
            Lookup::Fetch {
                conditional: None,
                revalidating: false,
                store: false
            }
        ));
        assert!(is_plain_fetch(&cache.lookup(
            "k",
            CachePolicy::Reload,
            at(109),
            Read::Owned
        )));
    }

    #[test]
    fn only_if_cached_without_an_entry_is_unavailable() {
        let cache = ByteCache::open(MemStore::default(), 1024);
        assert!(matches!(
            cache.lookup("k", CachePolicy::OnlyIfCached, at(0), Read::Owned),
            Lookup::Unavailable
        ));
        assert!(cache.store(
            "k",
            None,
            &response(&[("cache-control", "max-age=0")], b"old"),
            at(0)
        ));
        assert_eq!(
            &hit(cache.lookup("k", CachePolicy::OnlyIfCached, at(500), Read::Owned)).1[..],
            b"old",
            "however stale"
        );
    }

    #[test]
    fn a_304_refreshes_the_record_in_the_store_and_serves_the_stored_body() {
        let store = MemStore::default();
        let cache = ByteCache::open(store.clone(), 1024);
        assert!(cache.store(
            "k",
            Some("text/css"),
            &response(
                &[("cache-control", "max-age=10"), ("etag", "\"v1\"")],
                b"body"
            ),
            at(0),
        ));
        let (entry, bytes) = cache
            .refreshed(
                "k",
                &headers(&[("cache-control", "max-age=600")]),
                at(50),
                Read::Owned,
            )
            .expect("refreshed");
        assert_eq!(&bytes[..], b"body");
        assert_eq!(entry.response.stored_at, at(50));
        assert_eq!(entry.meta.last_used, at(50));
        hit(cache.lookup("k", CachePolicy::Default, at(600), Read::Owned));

        // The refreshed record is what a later open reads.
        let reopened = ByteCache::open(store, 1024);
        let entry = reopened.entry("k").unwrap();
        assert_eq!(
            entry.response.headers.get("cache-control").unwrap(),
            "max-age=600"
        );
        assert_eq!(entry.response.headers.get("etag").unwrap(), "\"v1\"");
        assert_eq!(entry.media_type.as_deref(), Some("text/css"));
    }

    #[test]
    fn hits_reach_the_store_as_a_touch_at_most_once_a_day() {
        let store = MemStore::default();
        let cache = ByteCache::open(store.clone(), 1024);
        let forever = [("cache-control", "max-age=10000000")];
        assert!(cache.store("k", None, &response(&forever, b"body"), at(0)));
        let day = 86_400;
        for seconds in [1, 60, day - 1] {
            hit(cache.lookup("k", CachePolicy::Default, at(seconds), Read::Owned));
        }
        assert_eq!(store.touches.load(Ordering::Relaxed), 0);
        assert_eq!(cache.entry("k").unwrap().meta.last_used, at(day - 1));
        hit(cache.lookup("k", CachePolicy::Default, at(day), Read::Owned));
        assert_eq!(store.touches.load(Ordering::Relaxed), 1);
        assert_eq!(store.persisted_last_used("k"), at(day));
        hit(cache.lookup("k", CachePolicy::Default, at(2 * day - 1), Read::Owned));
        assert_eq!(store.touches.load(Ordering::Relaxed), 1);
        hit(cache.lookup("k", CachePolicy::Default, at(2 * day), Read::Owned));
        assert_eq!(store.touches.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn the_budget_is_clamped_to_the_store_quota_and_trimmed_on_open() {
        let unbounded = MemStore::default();
        let cache = ByteCache::open(unbounded.clone(), 100);
        assert_eq!(cache.budget(), 100);
        for (key, seconds) in [("a", 1), ("b", 2), ("c", 3)] {
            assert!(cache.store(key, None, &response(FRESH, b"xxxx"), at(seconds)));
        }
        drop(cache);

        let quota = MemStore {
            quota: Some(8),
            ..unbounded
        };
        let cache = ByteCache::open(quota.clone(), 100);
        assert_eq!(cache.budget(), 8);
        assert_eq!(cache.used_bytes(), 8);
        assert!(cache.entry("a").is_none(), "the oldest is trimmed at open");
        assert!(!quota.holds("a"));
        let generous = MemStore {
            quota: Some(1 << 30),
            ..quota
        };
        assert_eq!(ByteCache::open(generous, 100).budget(), 100);
    }

    #[test]
    fn a_response_that_cannot_be_stored_is_refused() {
        let store = MemStore::default();
        let cache = ByteCache::open(store.clone(), 1024);
        assert!(!cache.store(
            "k",
            None,
            &response(&[("cache-control", "no-store")], b"x"),
            at(0)
        ));
        let mut error = response(FRESH, b"x");
        error.status = 500;
        assert!(!cache.store("k", None, &error, at(0)));
        store.fail_put.store(true, Ordering::Relaxed);
        assert!(!cache.store("k", None, &response(FRESH, b"x"), at(0)));
        assert!(cache.is_empty());
        assert_eq!(cache.used_bytes(), 0);
    }

    #[test]
    fn mapped_reads_use_the_store_mapping_and_verify_it() {
        let store = MemStore {
            maps: true,
            ..MemStore::default()
        };
        let cache = ByteCache::open(store.clone(), 1024);
        assert!(cache.store("k", None, &response(FRESH, b"body"), at(0)));
        assert_eq!(&cache.map_body("k").unwrap()[..], b"body");
        assert_eq!(
            &hit(cache.lookup("k", CachePolicy::Default, at(1), Read::Mapped)).1[..],
            b"body"
        );
        assert!(cache.map_body("missing").is_none());

        // A mapping filed under another key's record is a miss.
        let mut other = cache.entry("k").unwrap().meta;
        other.key = "elsewhere".to_owned();
        store.forge("k", other, b"body");
        assert!(cache.map_body("k").is_none());
        assert!(cache.is_empty());

        // Without mapping support a mapped read falls back to an owned one.
        let plain = ByteCache::open(MemStore::default(), 1024);
        assert!(plain.store("k", None, &response(FRESH, b"body"), at(0)));
        assert!(plain.map_body("k").is_none());
        assert_eq!(
            &hit(plain.lookup("k", CachePolicy::Default, at(1), Read::Mapped)).1[..],
            b"body"
        );
    }
}
