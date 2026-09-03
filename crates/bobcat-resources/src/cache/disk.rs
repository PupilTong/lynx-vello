//! The disk tier: fetched bytes kept across runs under a byte budget, with
//! the HTTP record needed to decide whether they are still good.
//!
//! One entry is two files named by a hash of the cache key — `<hash>.bin`
//! for the body, `<hash>.json` for the record. The hash is 64-bit FNV-1a,
//! which is not collision-free, so the record stores the full key and a
//! lookup checks it: a collision reads as a miss, never as another URL's
//! bytes. Writes go to a temporary file in the same directory and are
//! renamed into place, so a crash mid-write leaves a stray temporary file
//! rather than a truncated entry, and the sweep at open removes both strays
//! and orphaned halves.
//!
//! The index is held under one mutex, and IO runs under it too. That
//! serialises the disk tier, which is fine for the traffic a page produces
//! and simpler than anything that is not.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::header::{HeaderMap, HeaderName, HeaderValue};
use rustc_hash::FxHashMap;
use serde_json::{Value, json};

use crate::cache::http::StoredResponse;

/// The record kept beside a stored body.
#[derive(Clone, Debug)]
pub struct DiskEntry {
    pub key: String,
    pub media_type: Option<String>,
    pub response: StoredResponse,
    pub len: u64,
    pub last_used: SystemTime,
}

#[derive(Debug, thiserror::Error)]
pub enum DiskCacheError {
    #[error("disk cache IO failed: {0}")]
    Io(#[from] io::Error),
    #[error("an entry of {len} bytes cannot fit a disk cache budgeted at {budget} bytes")]
    TooLarge { len: u64, budget: u64 },
}

/// A byte-budgeted cache of fetched bodies on disk.
pub struct DiskCache {
    dir: PathBuf,
    budget: u64,
    index: Mutex<Index>,
}

#[derive(Default)]
struct Index {
    entries: FxHashMap<String, DiskEntry>,
    used: u64,
    next_temp: u64,
}

impl std::fmt::Debug for DiskCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiskCache")
            .field("dir", &self.dir)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl DiskCache {
    /// Opens `dir`, creating it, scanning the entries already there, and
    /// trimming them to `budget_bytes`.
    pub fn open(dir: impl Into<PathBuf>, budget_bytes: u64) -> Result<Self, DiskCacheError> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let cache = Self {
            dir,
            budget: budget_bytes,
            index: Mutex::new(Index::default()),
        };
        cache.rescan()?;
        Ok(cache)
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
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
        self.index().entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index().entries.is_empty()
    }

    /// The record for `key`, without reading the body.
    #[must_use]
    pub fn entry(&self, key: &str) -> Option<DiskEntry> {
        self.index().entries.get(key).cloned()
    }

    /// The record and body for `key`, marking it most recently used.
    ///
    /// Any inconsistency — a missing or unreadable body, a record whose key
    /// is another URL's — reads as a miss, and the broken pair is removed
    /// so it cannot mislead a later lookup either.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<(DiskEntry, Vec<u8>)> {
        let mut index = self.index();
        let entry = index.entries.get_mut(key)?;
        let path = self.body_path(key);
        match fs::read(&path) {
            Ok(bytes) if bytes.len() as u64 == entry.len => {
                entry.last_used = SystemTime::now();
                let entry = entry.clone();
                drop(index);
                // Best effort: the sidecar's `last_used` only affects the
                // eviction order after the next open.
                let _ = self.write_record(&entry);
                Some((entry, bytes))
            }
            _ => {
                let removed = index.entries.remove(key).expect("the entry was just found");
                index.used -= removed.len;
                drop(index);
                self.delete_pair(key);
                None
            }
        }
    }

    /// Stores `bytes` under `key` with its record, replacing any existing
    /// entry, and evicts least-recently-used entries until the budget holds.
    pub fn put(
        &self,
        key: &str,
        media_type: Option<&str>,
        response: &StoredResponse,
        bytes: &[u8],
    ) -> Result<(), DiskCacheError> {
        let len = bytes.len() as u64;
        if len > self.budget {
            return Err(DiskCacheError::TooLarge {
                len,
                budget: self.budget,
            });
        }
        let entry = DiskEntry {
            key: key.to_owned(),
            media_type: media_type.map(str::to_owned),
            response: response.clone(),
            len,
            last_used: SystemTime::now(),
        };
        let mut index = self.index();
        let temp = self.temp_path(&mut index);
        fs::write(&temp, bytes)?;
        fs::rename(&temp, self.body_path(key))?;
        if let Some(previous) = index.entries.insert(key.to_owned(), entry.clone()) {
            index.used -= previous.len;
        }
        index.used += len;
        self.write_record(&entry)?;
        self.trim(&mut index);
        Ok(())
    }

    /// Rewrites only `key`'s record, for a `304` that refreshed its headers.
    pub fn update_response(
        &self,
        key: &str,
        response: &StoredResponse,
    ) -> Result<(), DiskCacheError> {
        let mut index = self.index();
        let Some(entry) = index.entries.get_mut(key) else {
            return Ok(());
        };
        entry.response = response.clone();
        entry.last_used = SystemTime::now();
        let entry = entry.clone();
        self.write_record(&entry)
    }

    pub fn remove(&self, key: &str) -> Result<(), DiskCacheError> {
        let mut index = self.index();
        if let Some(entry) = index.entries.remove(key) {
            index.used -= entry.len;
        }
        drop(index);
        self.delete_pair(key);
        Ok(())
    }

    pub fn clear(&self) -> Result<(), DiskCacheError> {
        let mut index = self.index();
        let keys: Vec<String> = index.entries.keys().cloned().collect();
        for key in keys {
            self.delete_pair(&key);
        }
        index.entries.clear();
        index.used = 0;
        Ok(())
    }

    fn index(&self) -> std::sync::MutexGuard<'_, Index> {
        self.index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn rescan(&self) -> Result<(), DiskCacheError> {
        let mut index = self.index();
        index.entries.clear();
        index.used = 0;
        let mut bodies: FxHashMap<String, u64> = FxHashMap::default();
        let mut records: Vec<PathBuf> = Vec::new();
        for item in fs::read_dir(&self.dir)? {
            let item = item?;
            let path = item.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some((stem, extension)) = name.rsplit_once('.') else {
                continue;
            };
            let well_named = stem.len() == 16 && stem.bytes().all(|byte| byte.is_ascii_hexdigit());
            match extension {
                "bin" if well_named => {
                    bodies.insert(stem.to_owned(), item.metadata()?.len());
                }
                "json" if well_named => records.push(path),
                // Temporary files from an interrupted write, and anything
                // else that is not ours.
                _ => {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        for path in records {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned();
            let entry = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|value| parse_record(&value));
            match entry {
                Some(entry)
                    if hash_key(&entry.key) == stem && bodies.remove(&stem) == Some(entry.len) =>
                {
                    index.used += entry.len;
                    index.entries.insert(entry.key.clone(), entry);
                }
                _ => {
                    // A record without its body, a body of the wrong size,
                    // or a record filed under the wrong name.
                    let _ = fs::remove_file(&path);
                    if bodies.remove(&stem).is_some() {
                        let _ = fs::remove_file(self.dir.join(format!("{stem}.bin")));
                    }
                }
            }
        }
        for stem in bodies.keys() {
            let _ = fs::remove_file(self.dir.join(format!("{stem}.bin")));
        }
        self.trim(&mut index);
        Ok(())
    }

    fn trim(&self, index: &mut Index) {
        while index.used > self.budget {
            let victim = index
                .entries
                .values()
                .min_by_key(|entry| entry.last_used)
                .map(|entry| entry.key.clone());
            let Some(victim) = victim else {
                break;
            };
            let entry = index
                .entries
                .remove(&victim)
                .expect("the victim was just found");
            index.used -= entry.len;
            self.delete_pair(&victim);
        }
    }

    fn write_record(&self, entry: &DiskEntry) -> Result<(), DiskCacheError> {
        let record = json!({
            "key": entry.key,
            "media_type": entry.media_type,
            "status": entry.response.status,
            "headers": entry
                .response
                .headers
                .iter()
                .filter_map(|(name, value)| {
                    value.to_str().ok().map(|value| json!([name.as_str(), value]))
                })
                .collect::<Vec<Value>>(),
            "stored_at": unix_seconds(entry.response.stored_at),
            "last_used": unix_seconds(entry.last_used),
            "len": entry.len,
        });
        let path = self.record_path(&entry.key);
        let temp = path.with_extension("json.tmp");
        {
            let mut file = fs::File::create(&temp)?;
            file.write_all(record.to_string().as_bytes())?;
        }
        fs::rename(&temp, &path)?;
        Ok(())
    }

    fn delete_pair(&self, key: &str) {
        let _ = fs::remove_file(self.body_path(key));
        let _ = fs::remove_file(self.record_path(key));
    }

    fn body_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.bin", hash_key(key)))
    }

    fn record_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.json", hash_key(key)))
    }

    fn temp_path(&self, index: &mut Index) -> PathBuf {
        index.next_temp += 1;
        self.dir
            .join(format!("{}-{}.tmp", std::process::id(), index.next_temp))
    }
}

fn parse_record(value: &Value) -> Option<DiskEntry> {
    let key = value.get("key")?.as_str()?.to_owned();
    let media_type = value
        .get("media_type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let status = u16::try_from(value.get("status")?.as_u64()?).ok()?;
    let mut headers = HeaderMap::new();
    for pair in value.get("headers")?.as_array()? {
        let name = pair.get(0)?.as_str()?;
        let header_value = pair.get(1)?.as_str()?;
        if let (Ok(name), Ok(header_value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(header_value),
        ) {
            headers.append(name, header_value);
        }
    }
    Some(DiskEntry {
        key,
        media_type,
        response: StoredResponse {
            status,
            headers,
            stored_at: from_unix_seconds(value.get("stored_at")?.as_u64()?),
        },
        len: value.get("len")?.as_u64()?,
        last_used: from_unix_seconds(value.get("last_used")?.as_u64()?),
    })
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn from_unix_seconds(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// 64-bit FNV-1a of the key, as sixteen hex digits.
#[must_use]
pub fn hash_key(key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The platform's per-user cache directory for this product, or `None` when
/// the environment does not say where that is.
#[must_use]
pub fn default_cache_dir() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        Some(home()?.join("Library").join("Caches").join("bobcat"))
    } else if cfg!(windows) {
        Some(
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)?
                .join("bobcat")
                .join("cache"),
        )
    } else {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| Some(home()?.join(".cache")))?;
        Some(base.join("bobcat"))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bobcat-resources-disk-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn response(pairs: &[(&str, &str)]) -> StoredResponse {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                HeaderValue::from_str(value).expect("a header value"),
            );
        }
        StoredResponse {
            status: 200,
            headers,
            stored_at: UNIX_EPOCH + Duration::from_secs(1_000_000),
        }
    }

    #[test]
    fn a_body_and_its_record_round_trip() {
        let dir = TempDir::new();
        let cache = DiskCache::open(&dir.0, 1024).expect("open");
        cache
            .put(
                "https://a.test/x.png",
                Some("image/png"),
                &response(&[("etag", "\"1\""), ("cache-control", "max-age=5")]),
                b"pixels",
            )
            .expect("put");
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.used_bytes(), 6);
        let (entry, bytes) = cache.get("https://a.test/x.png").expect("hit");
        assert_eq!(bytes, b"pixels");
        assert_eq!(entry.media_type.as_deref(), Some("image/png"));
        assert_eq!(entry.response.headers.get("etag").unwrap(), "\"1\"");
        assert_eq!(
            entry.response.stored_at,
            UNIX_EPOCH + Duration::from_secs(1_000_000)
        );
        assert!(cache.get("https://a.test/other").is_none());

        // A fresh open rebuilds the index from disk.
        drop(cache);
        let reopened = DiskCache::open(&dir.0, 1024).expect("reopen");
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened.used_bytes(), 6);
        assert_eq!(reopened.entry("https://a.test/x.png").unwrap().len, 6);
    }

    #[test]
    fn a_hash_collision_reads_as_a_miss_and_is_cleaned_up() {
        let dir = TempDir::new();
        let cache = DiskCache::open(&dir.0, 1024).expect("open");
        cache
            .put("https://a.test/one", None, &response(&[]), b"one")
            .expect("put");
        // Forge a record filed under `one`'s hash that claims another key.
        let forged = json!({
            "key": "https://a.test/two", "media_type": null, "status": 200,
            "headers": [], "stored_at": 1, "last_used": 1, "len": 3,
        });
        fs::write(
            dir.0
                .join(format!("{}.json", hash_key("https://a.test/one"))),
            forged.to_string(),
        )
        .expect("forge");
        let reopened = DiskCache::open(&dir.0, 1024).expect("reopen");
        assert!(
            reopened.is_empty(),
            "a record that is not for its file name is dropped"
        );
        assert!(
            !dir.0
                .join(format!("{}.bin", hash_key("https://a.test/one")))
                .exists()
        );
    }

    #[test]
    fn corruption_is_recovered_from_on_open_and_on_read() {
        let dir = TempDir::new();
        let cache = DiskCache::open(&dir.0, 1024).expect("open");
        cache
            .put("https://a.test/a", None, &response(&[]), b"aaaa")
            .expect("put");
        cache
            .put("https://a.test/b", None, &response(&[]), b"bb")
            .expect("put");
        fs::write(dir.0.join("garbage.tmp"), b"x").expect("stray");
        fs::write(dir.0.join("0123456789abcdef.bin"), b"orphan").expect("orphan");
        fs::write(
            dir.0.join(format!("{}.json", hash_key("https://a.test/b"))),
            b"{not json",
        )
        .expect("corrupt");
        let reopened = DiskCache::open(&dir.0, 1024).expect("reopen");
        assert_eq!(reopened.len(), 1, "the corrupt record's entry is gone");
        assert!(!dir.0.join("garbage.tmp").exists());
        assert!(!dir.0.join("0123456789abcdef.bin").exists());

        // A body that vanished after open reads as a miss and drops the entry.
        fs::remove_file(dir.0.join(format!("{}.bin", hash_key("https://a.test/a")))).expect("rm");
        assert!(reopened.get("https://a.test/a").is_none());
        assert!(reopened.is_empty());
        assert_eq!(reopened.used_bytes(), 0);
    }

    #[test]
    fn eviction_takes_the_least_recently_used_and_oversized_entries_are_refused() {
        let dir = TempDir::new();
        let cache = DiskCache::open(&dir.0, 10).expect("open");
        cache.put("a", None, &response(&[]), b"aaaa").expect("put");
        std::thread::sleep(Duration::from_millis(5));
        cache.put("b", None, &response(&[]), b"bbbb").expect("put");
        std::thread::sleep(Duration::from_millis(5));
        assert!(
            cache.get("a").is_some(),
            "touching `a` makes `b` the oldest"
        );
        std::thread::sleep(Duration::from_millis(5));
        cache.put("c", None, &response(&[]), b"cccc").expect("put");
        assert!(cache.entry("b").is_none(), "evicted");
        assert!(cache.entry("a").is_some());
        assert!(cache.entry("c").is_some());
        assert_eq!(cache.used_bytes(), 8);
        assert!(matches!(
            cache.put("huge", None, &response(&[]), &[0; 11]),
            Err(DiskCacheError::TooLarge {
                len: 11,
                budget: 10
            })
        ));
    }

    #[test]
    fn replacement_update_remove_and_clear_keep_the_accounting_straight() {
        let dir = TempDir::new();
        let cache = DiskCache::open(&dir.0, 1024).expect("open");
        cache.put("k", None, &response(&[]), b"12345").expect("put");
        cache
            .put("k", Some("text/plain"), &response(&[]), b"12")
            .expect("replace");
        assert_eq!(cache.used_bytes(), 2);
        assert_eq!(cache.get("k").unwrap().1, b"12");
        cache
            .update_response("k", &response(&[("etag", "\"new\"")]))
            .expect("update");
        assert_eq!(
            cache
                .entry("k")
                .unwrap()
                .response
                .headers
                .get("etag")
                .unwrap(),
            "\"new\""
        );
        assert_eq!(
            cache.get("k").unwrap().1,
            b"12",
            "the body is untouched by a record update"
        );
        cache
            .update_response("missing", &response(&[]))
            .expect("a no-op");
        cache.put("j", None, &response(&[]), b"j").expect("put");
        cache.remove("k").expect("remove");
        assert_eq!(cache.used_bytes(), 1);
        cache.clear().expect("clear");
        assert!(cache.is_empty());
        assert_eq!(fs::read_dir(&dir.0).expect("dir").count(), 0);
    }

    #[test]
    fn keys_hash_deterministically() {
        assert_eq!(hash_key(""), "cbf29ce484222325");
        assert_eq!(hash_key("a"), "af63dc4c8601ec8c");
        assert_ne!(hash_key("https://a.test/1"), hash_key("https://a.test/2"));
    }

    #[test]
    fn the_default_cache_dir_ends_in_bobcat() {
        if let Some(dir) = default_cache_dir() {
            assert!(
                dir.ends_with("bobcat") || dir.ends_with("cache"),
                "{}",
                dir.display()
            );
        }
    }
}
