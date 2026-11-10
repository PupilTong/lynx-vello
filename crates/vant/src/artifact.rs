//! The derived-artifact tier: what an engine derives from a source text —
//! compiled bytecode, in practice — kept so the next load of the same text
//! reads it back instead of deriving it again.
//!
//! Deriving is the expensive half. Reading a derived artifact back costs a
//! fraction of compiling the text it came from, so a script that ran once,
//! in this process or an earlier one, should not be compiled again. The
//! artifact is opaque here: this module names no engine and never looks
//! inside one.
//!
//! # Roles
//!
//! - **The engine** looks an artifact up in [`Artifacts`], the process-wide memory tier,
//!   synchronously, at the moment it would derive one, by [`EngineId`] and [`ArtifactKey`]. On a
//!   miss it derives the artifact and [`Artifacts::insert`]s it, which also queues it for
//!   persistence.
//! - **The resource system**, which owns the embedder's [`Store`] through an [`ArtifactCache`],
//!   [`preload`](ArtifactCache::preload)s every stored artifact of a script's text into the memory
//!   tier before it delivers that text, and later [`flush`](ArtifactCache::flush)es what the engine
//!   derived in the meantime to the Store.
//! - **A host with no Store** skips the second role. The memory tier alone then serves, with the
//!   same keys, so artifacts are still shared by every engine instance in the process (every view
//!   and every capture of a server process), and adding a Store later changes no caller.
//!
//! # Keys
//!
//! An artifact is named by the [`SourceDigest`] of the text it was derived
//! from, the [`Variant`] of the derivation (the kind of compile and the names
//! it bakes in), and the [`EngineId`] of the build that derived it. Both
//! hashes use fixed keys: they name content across processes and runs.
//!
//! # Entry layout
//!
//! What a Store holds for one artifact ([`encode`], [`decode`]), all integers
//! little-endian:
//!
//! ```text
//! magic "VANTART\0" (8) | format u32 (= 1) | engine u64 | source u128
//!   | variant u64 | payload length u64 | CRC-64/ECMA-182 of the payload u64
//!   | payload
//! ```
//!
//! An engine typically reads its artifacts with a reader that trusts its
//! input, so [`decode`] verifies every field and the checksum before it
//! hands the payload over, and an entry that fails is deleted.
//!
//! # Expiry
//!
//! [`ArtifactCache::open`] deletes entries another engine build wrote (a new
//! build wipes the namespace), entries not used within the configured TTL,
//! and entries whose record does not parse; it then trims least recently
//! used entries to the byte budget, which is the configured one or the
//! Store's quota when that is smaller. [`ArtifactCache::flush`] trims again
//! after it writes. Recency reaches the Store through [`Store::touch`] at
//! most once a day per entry, as in the HTTP byte tier.
//!
//! # Concurrency
//!
//! Entries are content-addressed: two processes that write one key write
//! identical bytes, so a flush needs no lock across processes and there is
//! no shared index file. Each process rebuilds its index from
//! [`Store::scan`] and treats it as a hint, since the Store may lose entries
//! at any time ([`Volatility::EvictableAnytime`](crate::Volatility)). Within
//! a process the index is held under one mutex, and Store IO runs under it,
//! as in [`ByteCache`](crate::byte_cache::ByteCache).

use std::collections::VecDeque;
use std::hash::Hasher;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use rustc_hash::FxHashMap;

use crate::byte_cache::touch_due;
use crate::platform::{Meta, Store};

/// The 128-bit SipHash-1-3 of a source text, with fixed zero keys: the
/// digest names content across processes and runs, so it must not be
/// seeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceDigest(pub u128);

impl SourceDigest {
    #[must_use]
    pub fn of(text: &[u8]) -> Self {
        Self(siphasher::sip128::SipHasher13::new().hash(text).as_u128())
    }
}

/// Which engine build wrote an artifact: everything that decides whether it
/// can be read back (the artifact format version, the engine's sources, its
/// flags). Supplied by the engine; opaque here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EngineId(pub u64);

/// What a derivation depends on beyond the text: the kind of compile and
/// the names it bakes in (a module's name; a `CommonJS` wrapper's file name
/// and parameter list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Variant(pub u64);

impl Variant {
    /// The 64-bit SipHash-1-3 of `parts`, with fixed zero keys. Each part is
    /// prefixed with its length as a little-endian `u64`, so `["ab", "c"]`
    /// and `["a", "bc"]` differ and the hash is the same on every target.
    #[must_use]
    pub fn of(parts: &[&[u8]]) -> Self {
        let mut hasher = siphasher::sip::SipHasher13::new();
        for part in parts {
            hasher.write(&(part.len() as u64).to_le_bytes());
            hasher.write(part);
        }
        Self(hasher.finish())
    }
}

/// One artifact's name within an engine build's namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArtifactKey {
    pub source: SourceDigest,
    pub variant: Variant,
}

/// The name an artifact is resident under in [`Artifacts`].
type Name = (EngineId, ArtifactKey);

/// How many inserted artifacts may wait for persistence; past it the oldest
/// is dropped from the queue (it stays resident).
const PENDING_LIMIT: usize = 256;

/// The process-wide memory tier: artifacts least recently used first out
/// under a byte budget, and the queue of those the engine derived that no
/// Store has been given yet.
///
/// Entry counts are small (a page runs tens of scripts), so recency is a
/// monotonic stamp per entry and eviction scans for the smallest, as in
/// [`MemoryCache`](crate::memory::MemoryCache). That type is not reused: it
/// is keyed by string, so every lookup would format a key, and it keeps an
/// oversized entry its caller inserted, which this tier refuses.
pub struct Artifacts {
    state: Mutex<State>,
    /// Whether `state.pending` is non-empty, readable without the lock.
    has_pending: AtomicBool,
}

struct State {
    entries: FxHashMap<Name, Resident>,
    /// Inserted artifacts not yet taken for persistence, oldest first. Each
    /// name is resident and appears once.
    pending: VecDeque<Name>,
    budget: usize,
    used: usize,
    clock: u64,
}

struct Resident {
    artifact: Bytes,
    last_used: u64,
}

impl State {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Makes `artifact` resident under `name`, replacing what was there.
    /// `false` when it does not fit the budget on its own, in which case
    /// nothing is resident under `name` afterwards.
    fn keep(&mut self, name: Name, artifact: Bytes) -> bool {
        if let Some(previous) = self.entries.remove(&name) {
            self.used -= previous.artifact.len();
        }
        if self.budget == 0 || artifact.len() > self.budget {
            self.pending.retain(|pending| *pending != name);
            return false;
        }
        let last_used = self.tick();
        self.used += artifact.len();
        self.entries.insert(
            name,
            Resident {
                artifact,
                last_used,
            },
        );
        self.trim(Some(name));
        true
    }

    fn remove(&mut self, name: Name) {
        if let Some(resident) = self.entries.remove(&name) {
            self.used -= resident.artifact.len();
            self.pending.retain(|pending| *pending != name);
        }
    }

    /// Evicts least recently used entries other than `spared` until the
    /// budget holds.
    fn trim(&mut self, spared: Option<Name>) {
        while self.used > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(name, _)| spared != Some(**name))
                .min_by_key(|(_, resident)| resident.last_used)
                .map(|(name, _)| *name);
            let Some(victim) = victim else {
                break;
            };
            self.remove(victim);
        }
    }
}

impl std::fmt::Debug for Artifacts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state();
        formatter
            .debug_struct("Artifacts")
            .field("len", &state.entries.len())
            .field("budget", &state.budget)
            .field("used", &state.used)
            .field("pending", &state.pending.len())
            .finish_non_exhaustive()
    }
}

impl Artifacts {
    /// The budget of [`Self::global`] until [`Self::set_budget`] changes it.
    pub const DEFAULT_BUDGET_BYTES: usize = 32 * 1024 * 1024;

    #[must_use]
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            state: Mutex::new(State {
                entries: FxHashMap::default(),
                pending: VecDeque::new(),
                budget: budget_bytes,
                used: 0,
                clock: 0,
            }),
            has_pending: AtomicBool::new(false),
        }
    }

    /// The tier every engine and resource system in the process shares,
    /// with a budget of [`Self::DEFAULT_BUDGET_BYTES`] until
    /// [`Self::set_budget`] changes it.
    pub fn global() -> &'static Arc<Artifacts> {
        static GLOBAL: OnceLock<Arc<Artifacts>> = OnceLock::new();
        GLOBAL.get_or_init(|| Arc::new(Self::new(Self::DEFAULT_BUDGET_BYTES)))
    }

    #[must_use]
    pub fn budget(&self) -> usize {
        self.state().budget
    }

    /// Changes the budget, evicting whatever no longer fits. 0 disables the
    /// tier: nothing is kept, and every [`get`](Self::get) misses.
    pub fn set_budget(&self, budget_bytes: usize) {
        let mut state = self.state();
        state.budget = budget_bytes;
        state.trim(None);
        self.sync_pending(&state);
    }

    #[must_use]
    pub fn used_bytes(&self) -> usize {
        self.state().used
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.state().entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state().entries.is_empty()
    }

    /// The artifact `engine` derived for `key`, when it is resident. A hit
    /// marks the entry most recently used.
    #[must_use]
    pub fn get(&self, engine: EngineId, key: ArtifactKey) -> Option<Bytes> {
        let mut state = self.state();
        let stamp = state.tick();
        let resident = state.entries.get_mut(&(engine, key))?;
        resident.last_used = stamp;
        Some(resident.artifact.clone())
    }

    /// Whether an artifact is resident for `key`, without touching recency.
    #[must_use]
    pub fn contains(&self, engine: EngineId, key: ArtifactKey) -> bool {
        self.state().entries.contains_key(&(engine, key))
    }

    /// An artifact the engine just derived: made resident, and queued for
    /// whoever persists the tier ([`Self::take_pending`]). An artifact
    /// larger than the whole budget is not kept.
    pub fn insert(&self, engine: EngineId, key: ArtifactKey, artifact: Bytes) {
        let name = (engine, key);
        let mut state = self.state();
        if state.keep(name, artifact) && !state.pending.contains(&name) {
            if state.pending.len() == PENDING_LIMIT {
                state.pending.pop_front();
            }
            state.pending.push_back(name);
        }
        self.sync_pending(&state);
    }

    /// An artifact read back from a Store: made resident, and not queued,
    /// since it is persisted already.
    pub fn admit(&self, engine: EngineId, key: ArtifactKey, artifact: Bytes) {
        self.admitted(engine, key, artifact);
    }

    /// [`Self::admit`], answering whether the artifact is resident now.
    fn admitted(&self, engine: EngineId, key: ArtifactKey, artifact: Bytes) -> bool {
        let name = (engine, key);
        let mut state = self.state();
        let kept = state.keep(name, artifact);
        state.pending.retain(|pending| *pending != name);
        self.sync_pending(&state);
        kept
    }

    /// Whether [`Self::take_pending`] would return anything: one atomic
    /// load, cheap enough to ask on every turn of a loop.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.has_pending.load(Ordering::Acquire)
    }

    /// Drains the queue of inserted artifacts not yet persisted, oldest
    /// first. Only resident artifacts are queued, and they stay resident.
    #[must_use]
    pub fn take_pending(&self) -> Vec<(EngineId, ArtifactKey, Bytes)> {
        let mut state = self.state();
        let pending = std::mem::take(&mut state.pending);
        self.sync_pending(&state);
        pending
            .into_iter()
            .filter_map(|name| {
                let resident = state.entries.get(&name)?;
                Some((name.0, name.1, resident.artifact.clone()))
            })
            .collect()
    }

    /// Forgets the artifact `engine` derived for `key`, and drops it from
    /// the queue if it was waiting there. What a Store holds is untouched.
    pub fn remove(&self, engine: EngineId, key: ArtifactKey) {
        let mut state = self.state();
        state.remove((engine, key));
        self.sync_pending(&state);
    }

    /// Forgets every artifact and the queue.
    pub fn clear(&self) {
        let mut state = self.state();
        state.entries.clear();
        state.pending.clear();
        state.used = 0;
        self.sync_pending(&state);
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Mirrors the queue's emptiness into the flag; called under the lock
    /// after every change to the queue.
    fn sync_pending(&self, state: &State) {
        self.has_pending
            .store(!state.pending.is_empty(), Ordering::Release);
    }
}

const MAGIC: &[u8; 8] = b"VANTART\0";
const FORMAT: u32 = 1;
/// magic, format, engine, source, variant, payload length, checksum.
const HEADER_LEN: usize = 8 + 4 + 8 + 16 + 8 + 8 + 8;

/// The entry a Store holds for `artifact`: the header that names it and
/// checks it, then the payload (see the module docs for the layout).
#[must_use]
pub fn encode(engine: EngineId, key: ArtifactKey, artifact: &[u8]) -> Vec<u8> {
    let mut entry = Vec::with_capacity(HEADER_LEN + artifact.len());
    entry.extend_from_slice(MAGIC);
    entry.extend_from_slice(&FORMAT.to_le_bytes());
    entry.extend_from_slice(&engine.0.to_le_bytes());
    entry.extend_from_slice(&key.source.0.to_le_bytes());
    entry.extend_from_slice(&key.variant.0.to_le_bytes());
    entry.extend_from_slice(&(artifact.len() as u64).to_le_bytes());
    entry.extend_from_slice(&crc64(artifact).to_le_bytes());
    entry.extend_from_slice(artifact);
    entry
}

/// The payload of `entry`, as a zero-copy slice of it, when the entry is
/// `engine`'s artifact for `key`: every header field matches and the
/// checksum holds. `None` for a truncated entry, another format, another
/// engine, another key or a corrupt payload.
#[must_use]
pub fn decode(entry: &Bytes, engine: EngineId, key: ArtifactKey) -> Option<Bytes> {
    let header = entry.get(..HEADER_LEN)?;
    let payload = &entry[HEADER_LEN..];
    let valid = header[..8] == MAGIC[..]
        && u32::from_le_bytes(field(header, 8)) == FORMAT
        && u64::from_le_bytes(field(header, 12)) == engine.0
        && u128::from_le_bytes(field(header, 20)) == key.source.0
        && u64::from_le_bytes(field(header, 36)) == key.variant.0
        && u64::from_le_bytes(field(header, 44)) == payload.len() as u64
        && u64::from_le_bytes(field(header, 52)) == crc64(payload);
    valid.then(|| entry.slice(HEADER_LEN..))
}

/// The `N` bytes of `header` at `at`, which the caller has bounds-checked.
fn field<const N: usize>(header: &[u8], at: usize) -> [u8; N] {
    header[at..at + N]
        .try_into()
        .expect("the header is HEADER_LEN bytes long")
}

/// The ECMA-182 polynomial, as CRC-64/ECMA-182 uses it: most significant
/// bit first, zero initial value, no final XOR.
const CRC64_POLY: u64 = 0x42F0_E1EB_A9EA_3693;

const CRC64_TABLE: [u64; 256] = crc64_table();

const fn crc64_table() -> [u64; 256] {
    let mut table = [0; 256];
    let mut byte = 0;
    while byte < 256 {
        let mut crc = (byte as u64) << 56;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & (1 << 63) == 0 {
                crc << 1
            } else {
                (crc << 1) ^ CRC64_POLY
            };
            bit += 1;
        }
        table[byte] = crc;
        byte += 1;
    }
    table
}

/// CRC-64/ECMA-182 of `bytes`. A checksum, not a hash: it catches a
/// corrupt payload, while the key fields catch a misfiled one.
pub(crate) fn crc64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0, |crc, &byte| {
        CRC64_TABLE[usize::from((crc >> 56) as u8 ^ byte)] ^ (crc << 8)
    })
}

/// How an [`ArtifactCache`] bounds what it keeps in its Store.
#[derive(Clone, Copy, Debug)]
pub struct ArtifactConfig {
    /// The most the Store holds, in encoded entry bytes; clamped further to
    /// the Store's quota.
    pub budget_bytes: u64,
    /// How long an entry may go unused before [`ArtifactCache::open`]
    /// deletes it.
    pub ttl: Duration,
}

impl ArtifactConfig {
    pub const DEFAULT_BUDGET_BYTES: u64 = 64 * 1024 * 1024;
    pub const DEFAULT_TTL: Duration = Duration::from_hours(15 * 24);
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            budget_bytes: Self::DEFAULT_BUDGET_BYTES,
            ttl: Self::DEFAULT_TTL,
        }
    }
}

/// Persistence for one engine build's artifacts over an embedder [`Store`],
/// in front of which sits the shared memory tier ([`Artifacts`]).
///
/// Owned by the resource system and used on its threads; the engine never
/// touches it.
pub struct ArtifactCache<S: Store> {
    memory: Arc<Artifacts>,
    engine: EngineId,
    store: S,
    budget: u64,
    index: Mutex<Index>,
}

/// What the Store holds, by source, so [`ArtifactCache::preload`] finds a
/// text's variants without a scan. A hint: the Store is the truth.
#[derive(Default)]
struct Index {
    sources: FxHashMap<SourceDigest, Vec<Stored>>,
    len: usize,
    used: u64,
}

struct Stored {
    variant: Variant,
    /// The entry as the Store keeps it, with `last_used` as this tier last
    /// saw it used.
    meta: Meta,
    /// The `last_used` the Store last recorded, which rate-limits
    /// [`Store::touch`].
    persisted: SystemTime,
}

impl Index {
    fn get_mut(&mut self, key: ArtifactKey) -> Option<&mut Stored> {
        self.sources
            .get_mut(&key.source)?
            .iter_mut()
            .find(|stored| stored.variant == key.variant)
    }

    fn contains(&self, key: ArtifactKey) -> bool {
        self.sources
            .get(&key.source)
            .is_some_and(|stored| stored.iter().any(|stored| stored.variant == key.variant))
    }

    fn insert(&mut self, source: SourceDigest, stored: Stored) {
        self.used += stored.meta.len;
        let variants = self.sources.entry(source).or_default();
        if let Some(existing) = variants
            .iter_mut()
            .find(|existing| existing.variant == stored.variant)
        {
            self.used -= existing.meta.len;
            *existing = stored;
        } else {
            variants.push(stored);
            self.len += 1;
        }
    }

    fn remove(&mut self, key: ArtifactKey) -> Option<Stored> {
        let variants = self.sources.get_mut(&key.source)?;
        let position = variants
            .iter()
            .position(|stored| stored.variant == key.variant)?;
        let stored = variants.swap_remove(position);
        if variants.is_empty() {
            self.sources.remove(&key.source);
        }
        self.len -= 1;
        self.used -= stored.meta.len;
        Some(stored)
    }

    fn least_recently_used(&self) -> Option<ArtifactKey> {
        self.sources
            .iter()
            .flat_map(|(source, variants)| variants.iter().map(move |stored| (*source, stored)))
            .min_by_key(|(_, stored)| stored.meta.last_used)
            .map(|(source, stored)| ArtifactKey {
                source,
                variant: stored.variant,
            })
    }
}

impl<S: Store> std::fmt::Debug for ArtifactCache<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArtifactCache")
            .field("engine", &self.engine)
            .field("budget", &self.budget)
            .field("len", &self.len())
            .field("used", &self.used_bytes())
            .finish_non_exhaustive()
    }
}

impl<S: Store> ArtifactCache<S> {
    /// Opens the tier over `store` for `engine`'s artifacts and indexes what
    /// [`Store::scan`] finds. Deleted on the way: entries whose record does
    /// not parse or does not match its key, entries another engine build
    /// wrote, and entries not used within `config.ttl` of `now`. Then trims
    /// least recently used entries to `config.budget_bytes`, or to the
    /// Store's quota when that is smaller. A scan that fails opens an empty
    /// tier.
    #[must_use]
    pub fn open(
        memory: Arc<Artifacts>,
        store: S,
        engine: EngineId,
        config: ArtifactConfig,
        now: SystemTime,
    ) -> Self {
        let budget = store
            .estimate()
            .quota
            .map_or(config.budget_bytes, |quota| config.budget_bytes.min(quota));
        let cache = Self {
            memory,
            engine,
            store,
            budget,
            index: Mutex::new(Index::default()),
        };
        let mut index = Index::default();
        for meta in cache.store.scan().unwrap_or_default() {
            let expired = now
                .duration_since(meta.last_used)
                .is_ok_and(|unused| unused > config.ttl);
            let kept = parse_record(&meta.record).filter(|(owner, key)| {
                *owner == engine && meta.key == store_key(*owner, *key) && !expired
            });
            match kept {
                Some((_, key)) => index.insert(
                    key.source,
                    Stored {
                        variant: key.variant,
                        persisted: meta.last_used,
                        meta,
                    },
                ),
                None => {
                    let _ = cache.store.delete(&meta.key);
                }
            }
        }
        cache.trim(&mut index);
        *cache.index() = index;
        cache
    }

    /// Run before a script's text is delivered: reads every stored variant
    /// of `source` that is not already resident, verifies it (the Store's
    /// key and length, then [`decode`]) and admits it to the memory tier.
    /// An entry that fails is a miss and is deleted. Every variant found is
    /// marked used at `now`. Returns how many were admitted.
    pub fn preload(&self, source: SourceDigest, now: SystemTime) -> usize {
        let mut index = self.index();
        let Some(variants) = index.sources.get(&source) else {
            return 0;
        };
        let variants: Vec<(Variant, u64)> = variants
            .iter()
            .map(|stored| (stored.variant, stored.meta.len))
            .collect();
        let mut admitted = 0;
        for (variant, len) in variants {
            let key = ArtifactKey { source, variant };
            if !self.memory.contains(self.engine, key) {
                let Some(artifact) = self.read(key, len) else {
                    self.drop_entry(&mut index, key);
                    continue;
                };
                if self.memory.admitted(self.engine, key, artifact) {
                    admitted += 1;
                }
            }
            self.mark_used(&mut index, key, now);
        }
        admitted
    }

    /// Writes what the engine derived since the last flush
    /// ([`Artifacts::take_pending`]) to the Store, then trims to the budget.
    /// Skipped: artifacts of another engine build (one process runs one
    /// build, so they are dropped from the queue), keys already stored, and
    /// entries larger than the whole budget. A write the Store refuses is
    /// lost; the artifact stays resident. Returns how many were written.
    pub fn flush(&self, now: SystemTime) -> usize {
        if !self.memory.has_pending() {
            return 0;
        }
        let pending = self.memory.take_pending();
        let mut index = self.index();
        let mut written = 0;
        for (engine, key, artifact) in pending {
            if engine != self.engine || index.contains(key) {
                continue;
            }
            let entry = encode(engine, key, &artifact);
            let len = entry.len() as u64;
            if len > self.budget {
                continue;
            }
            let meta = Meta {
                key: store_key(engine, key),
                len,
                last_used: now,
                record: record(engine, key),
            };
            if self.store.put(&meta, &entry).is_err() {
                continue;
            }
            index.insert(
                key.source,
                Stored {
                    variant: key.variant,
                    meta,
                    persisted: now,
                },
            );
            written += 1;
        }
        self.trim(&mut index);
        written
    }

    /// The memory tier this cache preloads into and flushes from.
    #[must_use]
    pub fn memory(&self) -> &Arc<Artifacts> {
        &self.memory
    }

    #[must_use]
    pub fn engine(&self) -> EngineId {
        self.engine
    }

    /// How many entries the Store holds, as far as the index knows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.index().len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index().len == 0
    }

    /// The encoded bytes the Store holds, as far as the index knows.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.index().used
    }

    #[must_use]
    pub fn budget(&self) -> u64 {
        self.budget
    }

    /// Forgets every entry and deletes each from the Store. The memory tier
    /// is untouched.
    pub fn clear(&self) {
        let mut index = self.index();
        for variants in index.sources.values() {
            for stored in variants {
                let _ = self.store.delete(&stored.meta.key);
            }
        }
        *index = Index::default();
    }

    /// The Store the tier is built on.
    #[must_use]
    pub fn store_ref(&self) -> &S {
        &self.store
    }

    fn index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `key`'s payload from the Store, or `None` when the entry is missing,
    /// filed under another key, not `len` bytes long, or fails [`decode`].
    fn read(&self, key: ArtifactKey, len: u64) -> Option<Bytes> {
        let name = store_key(self.engine, key);
        match self.store.get(&name) {
            Ok(Some((meta, entry)))
                if meta.key == name && meta.len == len && entry.len() as u64 == len =>
            {
                decode(&entry, self.engine, key)
            }
            _ => None,
        }
    }

    fn mark_used(&self, index: &mut Index, key: ArtifactKey, now: SystemTime) {
        let Some(stored) = index.get_mut(key) else {
            return;
        };
        stored.meta.last_used = now;
        if touch_due(stored.persisted, now) && self.store.touch(&stored.meta.key, now).is_ok() {
            stored.persisted = now;
        }
    }

    fn drop_entry(&self, index: &mut Index, key: ArtifactKey) {
        index.remove(key);
        let _ = self.store.delete(&store_key(self.engine, key));
    }

    fn trim(&self, index: &mut Index) {
        while index.used > self.budget {
            let Some(victim) = index.least_recently_used() else {
                break;
            };
            self.drop_entry(index, victim);
        }
    }
}

/// The Store key of `engine`'s artifact for `key`.
fn store_key(engine: EngineId, key: ArtifactKey) -> String {
    format!(
        "{:016x}/{:032x}/{:016x}",
        engine.0, key.source.0, key.variant.0
    )
}

/// The record a Store keeps beside an entry. The fields are hex strings,
/// since a `u128` does not fit a JSON number.
fn record(engine: EngineId, key: ArtifactKey) -> serde_json::Value {
    serde_json::json!({
        "engine": format!("{:016x}", engine.0),
        "source": format!("{:032x}", key.source.0),
        "variant": format!("{:016x}", key.variant.0),
    })
}

fn parse_record(record: &serde_json::Value) -> Option<(EngineId, ArtifactKey)> {
    let hex = |name: &str| record.get(name)?.as_str();
    Some((
        EngineId(u64::from_str_radix(hex("engine")?, 16).ok()?),
        ArtifactKey {
            source: SourceDigest(u128::from_str_radix(hex("source")?, 16).ok()?),
            variant: Variant(u64::from_str_radix(hex("variant")?, 16).ok()?),
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::UNIX_EPOCH;

    use super::*;
    use crate::test_store::MemStore;

    const ENGINE: EngineId = EngineId(0x0123_4567_89ab_cdef);
    const OTHER_ENGINE: EngineId = EngineId(7);
    const DAY: u64 = 86_400;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn key_of(text: &str, variant: &str) -> ArtifactKey {
        ArtifactKey {
            source: SourceDigest::of(text.as_bytes()),
            variant: Variant::of(&[variant.as_bytes()]),
        }
    }

    fn bytes(contents: &[u8]) -> Bytes {
        Bytes::copy_from_slice(contents)
    }

    fn config(budget_bytes: u64) -> ArtifactConfig {
        ArtifactConfig {
            budget_bytes,
            ..ArtifactConfig::default()
        }
    }

    fn open(store: &MemStore, budget_bytes: u64, now: SystemTime) -> ArtifactCache<MemStore> {
        ArtifactCache::open(
            Arc::new(Artifacts::new(1 << 20)),
            store.clone(),
            ENGINE,
            config(budget_bytes),
            now,
        )
    }

    /// The encoded size of an entry whose payload is `payload` bytes.
    fn entry_len(payload: usize) -> u64 {
        (HEADER_LEN + payload) as u64
    }

    #[test]
    fn digests_are_pinned_and_separate_texts_and_parts() {
        assert_eq!(
            SourceDigest::of(b"export default 1;"),
            SourceDigest(0x67bc_2c81_3bf0_f5b7_7f54_b1b3_1d9c_0041),
            "a seed or algorithm change renames every stored artifact"
        );
        assert_eq!(
            Variant::of(&[b"module", b"a.js"]),
            Variant(0x2c76_b4e7_fc74_dec0)
        );
        assert_ne!(SourceDigest::of(b"a"), SourceDigest::of(b"b"));
        assert_ne!(
            Variant::of(&[b"ab", b"c"]),
            Variant::of(&[b"a", b"bc"]),
            "parts are length-prefixed"
        );
        assert_ne!(Variant::of(&[b"abc"]), Variant::of(&[b"ab", b"c"]));
    }

    #[test]
    fn remove_forgets_one_artifact_and_its_place_in_the_queue() {
        let memory = Artifacts::new(1 << 20);
        let (kept, removed) = (key_of("a", "module"), key_of("b", "module"));
        memory.insert(ENGINE, kept, bytes(b"kept"));
        memory.insert(ENGINE, removed, bytes(b"removed"));
        memory.remove(ENGINE, removed);
        memory.remove(ENGINE, key_of("never", "module"));
        assert!(memory.contains(ENGINE, kept));
        assert!(!memory.contains(ENGINE, removed));
        assert_eq!(memory.used_bytes(), 4);
        let pending = memory.take_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, kept);
        memory.remove(ENGINE, kept);
        assert!(memory.is_empty());
        assert!(!memory.has_pending());
    }

    #[test]
    fn the_checksum_is_crc64_ecma_182() {
        assert_eq!(crc64(b"123456789"), 0x6C40_DF5F_0B49_7347);
        assert_eq!(crc64(b""), 0);
    }

    #[test]
    fn an_entry_round_trips_and_its_payload_is_a_slice_of_it() {
        let key = key_of("text", "module");
        let entry = Bytes::from(encode(ENGINE, key, b"bytecode"));
        assert_eq!(entry.len() as u64, entry_len(8));
        assert_eq!(&entry[..8], b"VANTART\0");
        let payload = decode(&entry, ENGINE, key).expect("a valid entry");
        assert_eq!(&payload[..], b"bytecode");
        assert_eq!(payload.as_ptr(), entry[HEADER_LEN..].as_ptr());
        let empty = Bytes::from(encode(ENGINE, key, b""));
        assert_eq!(decode(&empty, ENGINE, key).as_deref(), Some(&b""[..]));
    }

    #[test]
    fn decode_rejects_every_entry_that_is_not_exactly_the_one_asked_for() {
        let key = key_of("text", "module");
        let valid = encode(ENGINE, key, b"bytecode");
        let rejects = |entry: Vec<u8>| decode(&Bytes::from(entry), ENGINE, key).is_none();

        for cut in 0..valid.len() {
            assert!(rejects(valid[..cut].to_vec()), "truncated to {cut} bytes");
        }
        let mut longer = valid.clone();
        longer.push(0);
        assert!(rejects(longer), "a trailing byte");

        let corrupt = |at: usize| {
            let mut entry = valid.clone();
            entry[at] ^= 1;
            entry
        };
        assert!(rejects(corrupt(0)), "wrong magic");
        assert!(rejects(corrupt(8)), "wrong format");
        assert!(rejects(corrupt(12)), "another engine's field");
        assert!(rejects(corrupt(20)), "another source's field");
        assert!(rejects(corrupt(36)), "another variant's field");
        assert!(rejects(corrupt(44)), "wrong length field");
        assert!(rejects(corrupt(52)), "wrong checksum");
        assert!(rejects(corrupt(valid.len() - 1)), "a flipped payload bit");

        let entry = Bytes::from(valid);
        assert!(
            decode(&entry, OTHER_ENGINE, key).is_none(),
            "another engine"
        );
        let other_source = ArtifactKey {
            source: SourceDigest::of(b"other"),
            ..key
        };
        assert!(decode(&entry, ENGINE, other_source).is_none());
        let other_variant = ArtifactKey {
            variant: Variant::of(&[b"script"]),
            ..key
        };
        assert!(decode(&entry, ENGINE, other_variant).is_none());
    }

    #[test]
    fn an_inserted_artifact_is_resident_and_queued() {
        let artifacts = Artifacts::new(1024);
        let key = key_of("text", "module");
        assert!(artifacts.get(ENGINE, key).is_none());
        assert!(!artifacts.has_pending());
        artifacts.insert(ENGINE, key, bytes(b"code"));
        assert_eq!(artifacts.get(ENGINE, key).as_deref(), Some(&b"code"[..]));
        assert!(artifacts.contains(ENGINE, key));
        assert!(
            !artifacts.contains(OTHER_ENGINE, key),
            "engines are separate"
        );
        assert_eq!((artifacts.len(), artifacts.used_bytes()), (1, 4));
        assert!(artifacts.has_pending());

        // Replacing keeps the accounting and queues the key once.
        artifacts.insert(ENGINE, key, bytes(b"longer"));
        assert_eq!((artifacts.len(), artifacts.used_bytes()), (1, 6));
        let pending = artifacts.take_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(&pending[0].2[..], b"longer");
        assert!(!artifacts.has_pending());
        assert!(artifacts.take_pending().is_empty(), "drained");
        assert!(artifacts.contains(ENGINE, key), "and still resident");

        artifacts.clear();
        assert!(artifacts.is_empty());
        assert_eq!(artifacts.used_bytes(), 0);
    }

    #[test]
    fn admitted_artifacts_are_resident_but_not_queued() {
        let artifacts = Artifacts::new(1024);
        let (a, b) = (key_of("a", "m"), key_of("b", "m"));
        artifacts.admit(ENGINE, a, bytes(b"aa"));
        assert!(artifacts.contains(ENGINE, a));
        assert!(!artifacts.has_pending());
        artifacts.insert(ENGINE, b, bytes(b"bb"));
        // Admitting a queued key means a Store has it already.
        artifacts.admit(ENGINE, b, bytes(b"bb"));
        assert!(!artifacts.has_pending());
        assert!(artifacts.take_pending().is_empty());
    }

    #[test]
    fn eviction_takes_the_least_recently_used_and_drops_it_from_the_queue() {
        let artifacts = Artifacts::new(10);
        let (a, b, c) = (key_of("a", "m"), key_of("b", "m"), key_of("c", "m"));
        artifacts.insert(ENGINE, a, bytes(b"aaaa"));
        artifacts.insert(ENGINE, b, bytes(b"bbbb"));
        assert!(artifacts.get(ENGINE, a).is_some());
        artifacts.insert(ENGINE, c, bytes(b"cccc"));
        assert!(
            !artifacts.contains(ENGINE, b),
            "using `a` made `b` the oldest"
        );
        assert!(artifacts.contains(ENGINE, a) && artifacts.contains(ENGINE, c));
        assert_eq!(artifacts.used_bytes(), 8);
        let pending: Vec<ArtifactKey> = artifacts
            .take_pending()
            .into_iter()
            .map(|(_, key, _)| key)
            .collect();
        assert_eq!(pending, [a, c], "an evicted key is not handed out");
    }

    #[test]
    fn an_oversized_artifact_is_not_kept_and_budget_zero_keeps_nothing() {
        let artifacts = Artifacts::new(4);
        let key = key_of("a", "m");
        artifacts.insert(ENGINE, key, bytes(b"abc"));
        artifacts.insert(ENGINE, key, bytes(b"abcde"));
        assert!(
            artifacts.get(ENGINE, key).is_none(),
            "an oversized replacement leaves nothing behind"
        );
        assert_eq!(artifacts.used_bytes(), 0);
        assert!(!artifacts.has_pending());

        artifacts.insert(ENGINE, key, bytes(b"abcd"));
        artifacts.set_budget(0);
        assert!(artifacts.is_empty(), "lowering the budget evicts");
        assert!(!artifacts.has_pending());
        artifacts.insert(ENGINE, key, bytes(b""));
        artifacts.admit(ENGINE, key, bytes(b"a"));
        assert!(artifacts.get(ENGINE, key).is_none());
        assert!(artifacts.is_empty());
        assert_eq!(artifacts.budget(), 0);
    }

    #[test]
    fn the_queue_is_bounded_and_drops_its_oldest() {
        let artifacts = Artifacts::new(1 << 20);
        for index in 0..=PENDING_LIMIT {
            artifacts.insert(ENGINE, key_of(&index.to_string(), "m"), bytes(b"x"));
        }
        let pending = artifacts.take_pending();
        assert_eq!(pending.len(), PENDING_LIMIT);
        assert_eq!(pending[0].1, key_of("1", "m"));
        assert_eq!(artifacts.len(), PENDING_LIMIT + 1, "all stay resident");
    }

    #[test]
    fn the_global_tier_is_one_instance() {
        assert!(Arc::ptr_eq(Artifacts::global(), Artifacts::global()));
    }

    #[test]
    fn a_flushed_artifact_survives_a_reopen_and_is_preloaded() {
        let store = MemStore::default();
        let cache = open(&store, 1 << 20, at(0));
        let key = key_of("text", "module");
        cache.memory().insert(ENGINE, key, bytes(b"bytecode"));
        assert_eq!(cache.flush(at(1)), 1);
        assert_eq!(cache.flush(at(1)), 0, "nothing is pending");
        assert_eq!((cache.len(), cache.used_bytes()), (1, entry_len(8)));
        let name = store_key(ENGINE, key);
        assert!(store.holds(&name));
        let meta = store.get_meta(&name).unwrap().unwrap();
        assert_eq!(meta.record["engine"], "0123456789abcdef");
        assert_eq!(meta.len, entry_len(8));
        drop(cache);

        let reopened = open(&store, 1 << 20, at(2));
        assert_eq!(reopened.len(), 1);
        assert!(reopened.memory().is_empty());
        assert_eq!(reopened.preload(key.source, at(3)), 1);
        assert_eq!(
            reopened.memory().get(ENGINE, key).as_deref(),
            Some(&b"bytecode"[..])
        );
        assert!(!reopened.memory().has_pending(), "preloaded, not queued");
        assert_eq!(reopened.preload(key.source, at(4)), 0, "already resident");
    }

    #[test]
    fn preload_admits_every_variant_of_a_source_and_nothing_of_another() {
        let store = MemStore::default();
        let cache = open(&store, 1 << 20, at(0));
        let (module, script, other) = (
            key_of("a", "module"),
            key_of("a", "script"),
            key_of("b", "module"),
        );
        for key in [module, script, other] {
            cache.memory().insert(ENGINE, key, bytes(b"code"));
        }
        assert_eq!(cache.flush(at(0)), 3);

        let reopened = open(&store, 1 << 20, at(1));
        assert_eq!(reopened.preload(module.source, at(1)), 2);
        assert!(reopened.memory().contains(ENGINE, module));
        assert!(reopened.memory().contains(ENGINE, script));
        assert!(!reopened.memory().contains(ENGINE, other));
        assert_eq!(reopened.preload(SourceDigest::of(b"unknown"), at(1)), 0);
    }

    #[test]
    fn another_engines_entries_are_wiped_at_open() {
        let store = MemStore::default();
        let cache = open(&store, 1 << 20, at(0));
        cache
            .memory()
            .insert(ENGINE, key_of("a", "m"), bytes(b"code"));
        assert_eq!(cache.flush(at(0)), 1);

        let rebuilt = ArtifactCache::open(
            Arc::new(Artifacts::new(1024)),
            store.clone(),
            OTHER_ENGINE,
            ArtifactConfig::default(),
            at(1),
        );
        assert!(rebuilt.is_empty());
        assert!(
            store.entries().is_empty(),
            "the old build's namespace is gone"
        );
    }

    #[test]
    fn an_entry_unused_past_the_ttl_is_wiped_at_open() {
        let store = MemStore::default();
        let cache = open(&store, 1 << 20, at(0));
        let (old, recent) = (key_of("old", "m"), key_of("recent", "m"));
        cache.memory().insert(ENGINE, old, bytes(b"code"));
        assert_eq!(cache.flush(at(0)), 1);
        cache.memory().insert(ENGINE, recent, bytes(b"code"));
        assert_eq!(cache.flush(at(10)), 1);

        let ttl = ArtifactConfig::DEFAULT_TTL.as_secs();
        assert_eq!(
            open(&store, 1 << 20, at(ttl)).len(),
            2,
            "unused for exactly the TTL"
        );
        let reopened = open(&store, 1 << 20, at(ttl + 1));
        assert_eq!(reopened.len(), 1);
        assert!(!store.holds(&store_key(ENGINE, old)));
        assert!(store.holds(&store_key(ENGINE, recent)));
    }

    #[test]
    fn the_store_is_trimmed_to_the_budget_and_to_the_quota() {
        let store = MemStore::default();
        let budget = 2 * entry_len(4);
        let cache = open(&store, budget, at(0));
        let (a, b, c) = (key_of("a", "m"), key_of("b", "m"), key_of("c", "m"));
        cache.memory().insert(ENGINE, a, bytes(b"aaaa"));
        cache.flush(at(1));
        cache.memory().insert(ENGINE, b, bytes(b"bbbb"));
        cache.flush(at(2));
        // Preloading `a` marks it used, so `b` is the oldest.
        cache.memory().clear();
        assert_eq!(cache.preload(a.source, at(3)), 1);
        cache.memory().insert(ENGINE, c, bytes(b"cccc"));
        assert_eq!(cache.flush(at(4)), 1);
        assert_eq!(cache.used_bytes(), budget);
        assert!(!store.holds(&store_key(ENGINE, b)));
        assert!(store.holds(&store_key(ENGINE, a)) && store.holds(&store_key(ENGINE, c)));
        drop(cache);

        let quota = MemStore {
            quota: Some(entry_len(4)),
            ..store
        };
        let cache = open(&quota, 1 << 20, at(5));
        assert_eq!(cache.budget(), entry_len(4));
        assert_eq!(cache.len(), 1);
        assert!(
            quota.holds(&store_key(ENGINE, c)),
            "the most recent is kept"
        );
    }

    #[test]
    fn a_stored_entry_that_does_not_verify_is_a_miss_and_is_deleted() {
        let store = MemStore::default();
        let cache = open(&store, 1 << 20, at(0));
        let keys = [
            key_of("flipped", "m"),
            key_of("collided", "m"),
            key_of("truncated", "m"),
            key_of("vanished", "m"),
            key_of("misnamed", "m"),
        ];
        for key in keys {
            cache.memory().insert(ENGINE, key, bytes(b"bytecode"));
        }
        assert_eq!(cache.flush(at(0)), keys.len());
        let [flipped, collided, truncated, vanished, misnamed] =
            keys.map(|key| store_key(ENGINE, key));

        let (meta, entry) = store.get(&flipped).unwrap().unwrap();
        let mut corrupt = entry.to_vec();
        *corrupt.last_mut().unwrap() ^= 1;
        store.forge(&flipped, meta, &corrupt);

        let (mut meta, entry) = store.get(&collided).unwrap().unwrap();
        meta.key = "another/key".to_owned();
        store.forge(&collided, meta, &entry);

        let (meta, entry) = store.get(&truncated).unwrap().unwrap();
        store.forge(&truncated, meta, &entry[..entry.len() - 1]);

        store.delete(&vanished).unwrap();

        // The index still lists all five; each read fails and drops one.
        cache.memory().clear();
        for key in &keys[..4] {
            assert_eq!(cache.preload(key.source, at(1)), 0);
            assert!(!cache.memory().contains(ENGINE, *key));
        }
        assert_eq!((cache.len(), cache.used_bytes()), (1, entry_len(8)));
        assert!(!store.holds(&flipped) && !store.holds(&collided) && !store.holds(&truncated));

        // At open: a record that names another key than the one it is filed
        // under, and one that does not parse.
        let (mut meta, entry) = store.get(&misnamed).unwrap().unwrap();
        meta.record = record(ENGINE, key_of("elsewhere", "m"));
        store.forge(&misnamed, meta.clone(), &entry);
        meta.key = "garbage".to_owned();
        meta.record = serde_json::json!({"engine": 1});
        store.forge("garbage", meta, b"");
        let reopened = open(&store, 1 << 20, at(2));
        assert!(reopened.is_empty());
        assert!(store.entries().is_empty());
    }

    #[test]
    fn hits_reach_the_store_as_a_touch_at_most_once_a_day() {
        let store = MemStore::default();
        let cache = open(&store, 1 << 20, at(0));
        let key = key_of("a", "m");
        cache.memory().insert(ENGINE, key, bytes(b"code"));
        assert_eq!(cache.flush(at(0)), 1);
        let name = store_key(ENGINE, key);
        for seconds in [1, 60, DAY - 1] {
            cache.preload(key.source, at(seconds));
        }
        assert_eq!(store.touches.load(Ordering::Relaxed), 0);
        cache.preload(key.source, at(DAY));
        assert_eq!(store.touches.load(Ordering::Relaxed), 1);
        assert_eq!(store.persisted_last_used(&name), at(DAY));
        cache.preload(key.source, at(2 * DAY - 1));
        assert_eq!(store.touches.load(Ordering::Relaxed), 1);
        cache.preload(key.source, at(2 * DAY));
        assert_eq!(store.touches.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn flush_skips_stored_keys_oversized_entries_and_other_engines() {
        let store = MemStore::default();
        let cache = open(&store, entry_len(4), at(0));
        let key = key_of("a", "m");
        cache.memory().insert(ENGINE, key, bytes(b"code"));
        assert_eq!(cache.flush(at(0)), 1);
        cache.memory().insert(ENGINE, key, bytes(b"code"));
        assert_eq!(cache.flush(at(1)), 0, "already stored");
        assert_eq!(store.persisted_last_used(&store_key(ENGINE, key)), at(0));

        let big = key_of("big", "m");
        cache.memory().insert(ENGINE, big, bytes(b"codes"));
        cache
            .memory()
            .insert(OTHER_ENGINE, key_of("c", "m"), bytes(b"c"));
        assert_eq!(cache.flush(at(2)), 0);
        assert!(cache.memory().contains(ENGINE, big), "still resident");
        assert!(!cache.memory().has_pending());
        assert_eq!(store.entries().len(), 1);
    }

    #[test]
    fn a_store_that_refuses_a_write_loses_only_that_write() {
        let store = MemStore::default();
        let cache = open(&store, 1 << 20, at(0));
        let (a, b) = (key_of("a", "m"), key_of("b", "m"));
        cache.memory().insert(ENGINE, a, bytes(b"code"));
        store.fail_put.store(true, Ordering::Relaxed);
        assert_eq!(cache.flush(at(0)), 0);
        assert!(cache.is_empty());
        assert!(
            cache.memory().contains(ENGINE, a),
            "the artifact still serves"
        );
        store.fail_put.store(false, Ordering::Relaxed);
        cache.memory().insert(ENGINE, b, bytes(b"code"));
        assert_eq!(cache.flush(at(1)), 1);
        assert!(store.holds(&store_key(ENGINE, b)));
    }

    #[test]
    fn two_processes_flushing_one_key_leave_one_valid_entry() {
        let store = MemStore::default();
        let first = open(&store, 1 << 20, at(0));
        let second = open(&store, 1 << 20, at(0));
        let key = key_of("shared", "module");
        first.memory().insert(ENGINE, key, bytes(b"bytecode"));
        second.memory().insert(ENGINE, key, bytes(b"bytecode"));
        assert_eq!(first.flush(at(1)), 1);
        assert_eq!(second.flush(at(2)), 1, "the second index did not know");
        assert_eq!(store.entries().len(), 1);

        let third = open(&store, 1 << 20, at(3));
        assert_eq!(third.preload(key.source, at(3)), 1);
        assert_eq!(
            third.memory().get(ENGINE, key).as_deref(),
            Some(&b"bytecode"[..])
        );
    }

    #[test]
    fn clear_deletes_every_entry_and_leaves_the_memory_tier() {
        let store = MemStore::default();
        let cache = open(&store, 1 << 20, at(0));
        let key = key_of("a", "m");
        cache.memory().insert(ENGINE, key, bytes(b"code"));
        assert_eq!(cache.flush(at(0)), 1);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.used_bytes(), 0);
        assert!(store.entries().is_empty());
        assert!(cache.memory().contains(ENGINE, key));
    }

    #[test]
    fn the_memory_tier_is_shared_across_threads() {
        fn shared<T: Send + Sync>() {}
        shared::<Artifacts>();
    }
}
