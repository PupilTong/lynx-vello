//! [`FsStore`]: a [`Store`] over one directory, natively.
//!
//! One entry is two files named by a hash of its key — `<hash>.bin` for the
//! body, `<hash>.json` for the [`Meta`]: `{ "key", "len", "last_used",
//! "record" }`, where `record` is the tier's own serialisation, kept as it
//! was handed over. The hash is 64-bit FNV-1a, which is not collision-free,
//! so the file stores the full key and every tier built on a Store checks it
//! on every read: a collision reads as a miss, never as another URL's bytes.
//!
//! Writes go to a temporary file in the same directory and are renamed into
//! place, so a crash mid-write leaves a stray temporary file rather than a
//! truncated entry, and [`Store::scan`] removes strays, orphaned halves and
//! records that do not parse. A record written by the disk tier this crate
//! had before `vant` (a flat record with no `record` member) does not parse
//! either and is deleted at the first scan: a cache is a cache.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use vant::{Estimate, Meta, Store, StoreError, Volatility};

use crate::file_body::{self, Body};

/// A [`Store`] over the files of one directory.
#[derive(Debug)]
pub struct FsStore {
    dir: PathBuf,
    /// Numbers this process's temporary files, so concurrent writes never
    /// share one.
    next_temp: AtomicU64,
}

impl FsStore {
    /// Opens `dir`, creating it, and checks that it can be listed. Nothing
    /// is read until [`Store::scan`].
    ///
    /// # Errors
    ///
    /// When the directory cannot be created or listed.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        fs::read_dir(&dir)?;
        Ok(Self {
            dir,
            next_temp: AtomicU64::new(0),
        })
    }

    /// The directory the entries live in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn body_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.bin", hash_key(key)))
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.json", hash_key(key)))
    }

    /// Writes `contents` to `path` through a temporary file renamed into
    /// place.
    fn write_atomically(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let temp = self.dir.join(format!(
            "{}-{}.tmp",
            std::process::id(),
            self.next_temp.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| {
            let mut file = fs::File::create(&temp)?;
            file.write_all(contents)?;
            fs::rename(&temp, path)
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temp);
        }
        written
    }

    fn write_meta(&self, meta: &Meta) -> io::Result<()> {
        let value = json!({
            "key": meta.key,
            "len": meta.len,
            "last_used": unix_seconds(meta.last_used),
            "record": meta.record,
        });
        self.write_atomically(&self.meta_path(&meta.key), value.to_string().as_bytes())
    }

    /// The [`Meta`] in the file at `path`, `None` when there is no file.
    fn read_meta(path: &Path) -> Result<Option<Meta>, StoreError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        serde_json::from_slice::<Value>(&bytes)
            .ok()
            .as_ref()
            .and_then(parse_meta)
            .map(Some)
            .ok_or_else(|| StoreError::Corrupt(format!("`{}` is not a record", path.display())))
    }
}

impl Store for FsStore {
    fn scan(&self) -> Result<Vec<Meta>, StoreError> {
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
        let mut entries = Vec::with_capacity(records.len());
        for path in records {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned();
            match Self::read_meta(&path) {
                Ok(Some(meta))
                    if hash_key(&meta.key) == stem && bodies.remove(&stem) == Some(meta.len) =>
                {
                    entries.push(meta);
                }
                _ => {
                    // A record without its body, a body of the wrong size, a
                    // record filed under the wrong name, or one that does not
                    // parse.
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
        Ok(entries)
    }

    fn get(&self, key: &str) -> Result<Option<(Meta, Bytes)>, StoreError> {
        let Some(meta) = self.get_meta(key)? else {
            return Ok(None);
        };
        match file_body::read(&self.body_path(key), Body::Read) {
            Ok(bytes) => Ok(Some((meta, bytes))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn get_meta(&self, key: &str) -> Result<Option<Meta>, StoreError> {
        Self::read_meta(&self.meta_path(key))
    }

    fn put(&self, meta: &Meta, body: &[u8]) -> Result<(), StoreError> {
        self.write_atomically(&self.body_path(&meta.key), body)?;
        self.write_meta(meta)?;
        Ok(())
    }

    fn put_meta(&self, meta: &Meta) -> Result<(), StoreError> {
        Ok(self.write_meta(meta)?)
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        for path in [self.body_path(key), self.meta_path(key)] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    /// Moves the body file's modification time to `now`, so a host that
    /// evicts by it agrees with the tier's order, and records `now` as the
    /// entry's last use.
    ///
    /// A body handed over mapped ([`Body::Mapped`]) is keyed by its file's
    /// modification time, so the next mapping after a touch is a new one;
    /// the tier touches an entry at most once a day.
    fn touch(&self, key: &str, now: SystemTime) -> Result<(), StoreError> {
        let Some(mut meta) = self.get_meta(key)? else {
            return Err(StoreError::Corrupt(format!("no record for `{key}`")));
        };
        fs::File::options()
            .write(true)
            .open(self.body_path(key))?
            .set_modified(now)?;
        meta.last_used = now;
        self.put_meta(&meta)
    }

    fn map_body(&self, key: &str) -> Option<Bytes> {
        file_body::read(&self.body_path(key), Body::Mapped).ok()
    }

    /// The bytes the bodies in the directory take, read from the directory
    /// itself. The OS names no quota for a cache directory.
    fn estimate(&self) -> Estimate {
        let used = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|item| {
                item.path()
                    .extension()
                    .is_some_and(|extension| extension == "bin")
            })
            .filter_map(|item| item.metadata().ok())
            .map(|metadata| metadata.len())
            .sum();
        Estimate { used, quota: None }
    }

    /// macOS may purge `~/Library/Caches` while the app is not running.
    /// Everywhere else the directory is treated as durable for now: Linux's
    /// `$XDG_CACHE_HOME` is only cleared by the user, and the hosts whose OS
    /// evicts at any moment (Android, iOS) bring their own Store.
    fn volatility(&self) -> Volatility {
        if cfg!(target_os = "macos") {
            Volatility::PurgedWhenIdle
        } else {
            Volatility::Durable
        }
    }
}

fn parse_meta(value: &Value) -> Option<Meta> {
    Some(Meta {
        key: value.get("key")?.as_str()?.to_owned(),
        len: value.get("len")?.as_u64()?,
        last_used: UNIX_EPOCH + Duration::from_secs(value.get("last_used")?.as_u64()?),
        record: value.get("record")?.clone(),
    })
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// 64-bit FNV-1a of the key, as sixteen hex digits: the name of the key's
/// two files.
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
    use http::HeaderMap;
    use http::header::{HeaderName, HeaderValue};
    use url::Url;
    use vant::byte_cache::{ByteCache, Lookup, Read};
    use vant::{CachePolicy, Response};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bobcat-resources-fs-store-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }

        fn file(&self, key: &str, extension: &str) -> PathBuf {
            self.0.join(format!("{}.{extension}", hash_key(key)))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn response(url: &str, pairs: &[(&str, &str)], body: &'static [u8]) -> Response {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                HeaderValue::from_str(value).expect("a header value"),
            );
        }
        Response {
            url: Url::parse(url).expect("a URL"),
            status: 200,
            headers,
            body: Bytes::from_static(body),
        }
    }

    fn meta(key: &str, len: u64) -> Meta {
        Meta {
            key: key.to_owned(),
            len,
            last_used: at(1_000),
            record: json!({ "anything": "the tier wrote" }),
        }
    }

    #[test]
    fn an_entry_round_trips_through_its_two_files() {
        let dir = TempDir::new();
        let store = FsStore::open(&dir.0).expect("open");
        store.put(&meta("k", 6), b"pixels").expect("put");
        assert!(dir.file("k", "bin").exists());
        assert!(dir.file("k", "json").exists());
        let (read, body) = store.get("k").expect("read").expect("present");
        assert_eq!(&body[..], b"pixels");
        assert_eq!(read.key, "k");
        assert_eq!(read.len, 6);
        assert_eq!(read.last_used, at(1_000));
        assert_eq!(read.record, json!({ "anything": "the tier wrote" }));
        assert_eq!(&store.map_body("k").expect("mapped")[..], b"pixels");
        assert_eq!(store.estimate().used, 6);
        assert!(store.get("missing").expect("read").is_none());

        store.delete("k").expect("delete");
        assert!(!dir.file("k", "bin").exists());
        assert!(!dir.file("k", "json").exists());
        store.delete("k").expect("deleting nothing is not an error");
    }

    #[test]
    fn a_stored_response_is_served_again_after_a_reopen() {
        let dir = TempDir::new();
        let url = "https://a.test/x.png";
        let now = SystemTime::now();
        let cache = ByteCache::open(FsStore::open(&dir.0).expect("open"), 1024);
        assert!(cache.store(
            url,
            Some("image/png"),
            &response(
                url,
                &[("etag", "\"1\""), ("cache-control", "max-age=60")],
                b"pixels"
            ),
            now,
        ));
        drop(cache);

        let reopened = ByteCache::open(FsStore::open(&dir.0).expect("reopen"), 1024);
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened.used_bytes(), 6);
        let Lookup::UseStored(entry, body) =
            reopened.lookup(url, CachePolicy::Default, now, Read::Mapped)
        else {
            panic!("a fresh stored response is used");
        };
        assert_eq!(&body[..], b"pixels");
        assert_eq!(entry.media_type.as_deref(), Some("image/png"));
        assert_eq!(entry.response.headers.get("etag").unwrap(), "\"1\"");
    }

    #[test]
    fn a_record_filed_under_another_keys_hash_reads_as_a_miss_and_is_removed() {
        let dir = TempDir::new();
        let cache = ByteCache::open(FsStore::open(&dir.0).expect("open"), 1024);
        let one = "https://a.test/one";
        assert!(cache.store(one, None, &response(one, &[], b"one"), SystemTime::now()));
        drop(cache);
        // Forge a record filed under `one`'s hash that claims another key.
        let forged = json!({
            "key": "https://a.test/two", "len": 3, "last_used": 1,
            "record": { "media_type": null, "status": 200, "headers": [], "stored_at": 1 },
        });
        fs::write(dir.file(one, "json"), forged.to_string()).expect("forge");
        let reopened = ByteCache::open(FsStore::open(&dir.0).expect("reopen"), 1024);
        assert!(
            reopened.is_empty(),
            "a record that is not for its file name is dropped"
        );
        assert!(!dir.file(one, "bin").exists());
        assert!(!dir.file(one, "json").exists());
    }

    #[test]
    fn corruption_is_recovered_from_on_open_and_on_read() {
        let dir = TempDir::new();
        let now = SystemTime::now();
        let cache = ByteCache::open(FsStore::open(&dir.0).expect("open"), 1024);
        let (a, b) = ("https://a.test/a", "https://a.test/b");
        assert!(cache.store(a, None, &response(a, &[], b"aaaa"), now));
        assert!(cache.store(b, None, &response(b, &[], b"bb"), now));
        drop(cache);
        fs::write(dir.0.join("garbage.tmp"), b"x").expect("stray");
        fs::write(dir.0.join("0123456789abcdef.bin"), b"orphan").expect("orphan");
        fs::write(dir.file(b, "json"), b"{not json").expect("corrupt");
        let reopened = ByteCache::open(FsStore::open(&dir.0).expect("reopen"), 1024);
        assert_eq!(reopened.len(), 1, "the corrupt record's entry is gone");
        assert!(!dir.0.join("garbage.tmp").exists());
        assert!(!dir.0.join("0123456789abcdef.bin").exists());
        assert!(!dir.file(b, "bin").exists());

        // A body that vanished after open reads as a miss and drops the entry.
        fs::remove_file(dir.file(a, "bin")).expect("rm");
        assert!(matches!(
            reopened.lookup(a, CachePolicy::ForceCache, now, Read::Mapped),
            Lookup::Fetch { .. }
        ));
        assert!(reopened.is_empty());
        assert_eq!(reopened.used_bytes(), 0);
    }

    #[test]
    fn a_record_from_the_disk_tier_before_vant_is_deleted_at_scan() {
        let dir = TempDir::new();
        let store = FsStore::open(&dir.0).expect("open");
        let key = "https://a.test/old";
        let flat = json!({
            "key": key, "media_type": null, "status": 200, "headers": [],
            "stored_at": 1, "last_used": 1, "len": 3,
        });
        fs::write(dir.file(key, "json"), flat.to_string()).expect("old record");
        fs::write(dir.file(key, "bin"), b"old").expect("old body");
        assert!(store.scan().expect("scan").is_empty());
        assert_eq!(fs::read_dir(&dir.0).expect("dir").count(), 0);
    }

    #[test]
    fn a_touch_moves_the_body_files_modification_time_and_the_last_use() {
        let dir = TempDir::new();
        let store = FsStore::open(&dir.0).expect("open");
        store.put(&meta("k", 1), b"k").expect("put");
        let later = at(2_000_000_000);
        store.touch("k", later).expect("touch");
        let modified = fs::metadata(dir.file("k", "bin"))
            .and_then(|metadata| metadata.modified())
            .expect("mtime");
        assert_eq!(modified, later);
        assert_eq!(
            store
                .get_meta("k")
                .expect("read")
                .expect("present")
                .last_used,
            later
        );
        assert!(store.touch("missing", later).is_err());
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
