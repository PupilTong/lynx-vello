//! A [`Store`] that lives inside one file with positioned, synchronous IO.
//!
//! A browser has one synchronous storage primitive: an OPFS file opened with
//! `createSyncAccessHandle` in a Worker. Opening one is asynchronous and
//! exclusive, so a Store, whose every method is synchronous, cannot open a
//! file per entry. [`PackStore`] keeps every entry in one file opened once,
//! written against [`SyncFile`] so its logic is tested natively over
//! [`MemFile`] and [`std::fs::File`]; the browser binding only implements the
//! trait.
//!
//! # File layout
//!
//! All integers are little-endian.
//!
//! ```text
//! header (64 bytes):
//!   magic "VANTPACK" (8) | format u32 (= 1) | reserved u32
//!   | move journal: from u64 | len u64 | CRC-64 of from and len u64
//!   | reserved, zero, to 64
//! records, appended, each:
//!   crc u64 | kind u8 | key_len u32 | meta_len u32 | body_len u64
//!   | last_used u64 (unix seconds) | key (UTF-8) | meta | body
//! ```
//!
//! A record's `crc` is the CRC-64/ECMA-182 of every byte after it in the
//! record, so it covers the kind, the lengths, the key, the meta and the
//! body, and the hashed bytes are contiguous. `meta` is the JSON text of
//! [`Meta::record`]. The kinds:
//!
//! - **1, put**: the entry's body and record; replaces any earlier state of the key.
//! - **2, meta**: a new record and `last_used` for an entry already present, with no body
//!   (`body_len` 0); the body stays the put's.
//! - **3, delete**: removes the key (`meta_len` and `body_len` 0).
//! - **4, touch**: a new `last_used` for an entry already present.
//!
//! A meta or touch record for an absent key changes nothing.
//!
//! The 16 bytes after `format` hold no state; the header was widened from 16
//! to 64 bytes for the move journal (see *Compaction*), which must be found
//! before any record is read.
//!
//! # Invariants
//!
//! - Outside compaction every write is one `write_at` at the end of the log. Put, meta and delete
//!   records are followed by `flush`; a touch is not (losing one only ages an entry).
//! - The in-memory index is the replay of the file: key → the put record's offset and length, the
//!   body's offset, and the entry's current [`Meta`]. It changes only after the write it reflects
//!   succeeded, so a put is atomic per entry: the old record stays live until the new one is fully
//!   appended.
//! - A write or flush that fails leaves the index as it was and marks the tail for repair: the next
//!   write first truncates the file back to the end of the last good record, so a half-written
//!   record can never sit between two good ones.
//! - [`PackStore::open`] replays the log from the first record and stops at the first one that does
//!   not verify (its header does not parse, it runs past the end of the file, its checksum fails,
//!   or its key or meta is not what the kind requires), truncating the file there. A crash
//!   mid-append leaves exactly such a record. A bit flipped in the middle of a closed file also
//!   stops the replay there, losing the entries after it: the format cannot tell rot from a torn
//!   tail. A body found corrupt while the store is open is handled better (see *Reads*).
//! - An empty file, a file shorter than the header, a file with another magic or format, and a
//!   header whose move journal verifies but names an impossible move are reinitialised to an empty
//!   store: a cache is a cache.
//!
//! # Reads
//!
//! [`Store::get`] reads the whole put record back and verifies its checksum,
//! kind, key and length before it hands the body over, as a slice of the
//! read buffer. A record that fails is [`StoreError::Corrupt`]; the entry is
//! dropped from the index and the store compacts at once, which rewrites the
//! file without the bad record, so a later open does not stop at it.
//!
//! # Compaction
//!
//! Dead bytes are those the live entries would not need if each were one
//! fresh put record: replaced and deleted puts, tombstones, meta and touch
//! records. When they exceed both 1 MiB and half the file after a put or a
//! delete, [`PackStore::compact`] runs. It needs no second file (in the
//! browser a second file would need an asynchronous open) and survives a
//! crash at any point:
//!
//! 1. **Append.** For every live entry, in log order, a fresh put record holding its body, current
//!    record and `last_used` is appended at the end `E` of the log, then the file is flushed. This
//!    is ordinary appending: a crash leaves the old log followed by fresh puts that restate what it
//!    already says, perhaps with a torn last one, which the replay drops. The `n` fresh bytes fit
//!    in front of their copy (`64 + n <= E`), because each fresh record is no longer than the put
//!    plus the latest meta record it restates.
//! 2. **Journal.** The header's move journal is set to `(E, n)` with its checksum, and flushed.
//!    From here the index points at the fresh records, which hold the whole live state.
//! 3. **Move.** `[E, E + n)` is copied to `[64, 64 + n)` in 64 KiB chunks and flushed, the file is
//!    truncated to `64 + n` and flushed, and the journal is cleared and flushed.
//!
//! A forward copy of the old live records in place would not survive a
//! crash: a record moved over part of itself is torn by an interrupted
//! write, and the bytes after the last moved record are fragments the
//! replay could misread. Copying from a region that is not overwritten
//! makes the move repeatable, and the journal says when to repeat it. At
//! open, a journal that verifies means the move may be unfinished: if the
//! file still holds `[E, E + n)` the source is whole (it was flushed before
//! the journal was written, and nothing is appended while a move is
//! pending), so the copy is redone; if the file is shorter the truncation
//! already happened, which follows a flushed copy, so the front is whole.
//! Either way the file is cut to `64 + n`, the journal cleared, and the
//! replay finds the fresh records. A journal that does not verify was torn
//! while being written or cleared: before the copy started, or after the
//! truncation, and both leave a log that replays as it stands. So a crash
//! anywhere in a compaction reopens to every entry it started with.
//!
//! In-process, a move that fails stays pending: reads go to the fresh
//! records, which the move never overwrites, and the next write finishes
//! the move first.
//!
//! # Concurrency
//!
//! The index and the file position are held under one mutex, and file IO
//! runs under it too, as in [`ByteCache`](crate::byte_cache::ByteCache).

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fmt, io};

use bytes::Bytes;
use rustc_hash::FxHashMap;

use crate::artifact::crc64;
use crate::platform::{Estimate, Meta, Store, StoreError, Threaded, Volatility};

/// One file with positioned, synchronous IO. Implemented over an OPFS sync
/// access handle in the browser and over [`std::fs::File`] natively.
#[expect(
    clippy::len_without_is_empty,
    reason = "a file's length is all a pack asks of it"
)]
pub trait SyncFile: Threaded {
    /// The file's length in bytes.
    fn len(&self) -> io::Result<u64>;
    /// Fills `buffer` from `offset`; a short read is an error
    /// ([`io::ErrorKind::UnexpectedEof`]).
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()>;
    /// Writes all of `bytes` at `offset`, extending the file as needed.
    fn write_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()>;
    /// Sets the file's length, cutting it or extending it with zeros.
    fn truncate(&self, len: u64) -> io::Result<()>;
    /// Makes what was written durable.
    fn flush(&self) -> io::Result<()>;
}

#[cfg(unix)]
impl SyncFile for std::fs::File {
    fn len(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(self, buffer, offset)
    }

    fn write_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        std::os::unix::fs::FileExt::write_all_at(self, bytes, offset)
    }

    fn truncate(&self, len: u64) -> io::Result<()> {
        self.set_len(len)
    }

    fn flush(&self) -> io::Result<()> {
        self.sync_data()
    }
}

#[cfg(windows)]
impl SyncFile for std::fs::File {
    fn len(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }

    fn read_at(&self, mut offset: u64, mut buffer: &mut [u8]) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        while !buffer.is_empty() {
            match self.seek_read(buffer, offset) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(read) => {
                    buffer = &mut std::mem::take(&mut buffer)[read..];
                    offset += read as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn write_at(&self, mut offset: u64, mut bytes: &[u8]) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        while !bytes.is_empty() {
            match self.seek_write(bytes, offset) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(written) => {
                    bytes = &bytes[written..];
                    offset += written as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn truncate(&self, len: u64) -> io::Result<()> {
        self.set_len(len)
    }

    fn flush(&self) -> io::Result<()> {
        self.sync_data()
    }
}

/// An in-memory [`SyncFile`]: for tests, and for a host that wants a
/// [`PackStore`] with no persistence.
#[derive(Default)]
pub struct MemFile(Mutex<Vec<u8>>);

impl MemFile {
    /// A file holding `bytes`.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Mutex::new(bytes))
    }

    /// A copy of the file's contents.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        self.contents().clone()
    }

    /// The file's contents.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.0.into_inner().unwrap_or_else(PoisonError::into_inner)
    }

    fn contents(&self) -> MutexGuard<'_, Vec<u8>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for MemFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemFile")
            .field("len", &self.contents().len())
            .finish()
    }
}

/// `offset..offset + len` as a range of memory, when it is one.
fn span(offset: u64, len: usize) -> io::Result<std::ops::Range<usize>> {
    let start = usize::try_from(offset).map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| io::Error::from(io::ErrorKind::OutOfMemory))?;
    Ok(start..end)
}

impl SyncFile for MemFile {
    fn len(&self) -> io::Result<u64> {
        Ok(self.contents().len() as u64)
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let contents = self.contents();
        let range = span(offset, buffer.len())?;
        let source = contents
            .get(range)
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        buffer.copy_from_slice(source);
        Ok(())
    }

    fn write_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()> {
        let mut contents = self.contents();
        let range = span(offset, bytes.len())?;
        if contents.len() < range.end {
            contents.resize(range.end, 0);
        }
        contents[range].copy_from_slice(bytes);
        Ok(())
    }

    fn truncate(&self, len: u64) -> io::Result<()> {
        let len = span(len, 0)?.start;
        self.contents().resize(len, 0);
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}

const MAGIC: &[u8; 8] = b"VANTPACK";
const FORMAT: u32 = 1;
const HEADER_SIZE: usize = 64;
const HEADER_LEN: u64 = HEADER_SIZE as u64;
/// Where the move journal sits in the header: `from u64 | len u64 | crc u64`.
const JOURNAL_AT: u64 = 16;
const JOURNAL_LEN: usize = 24;
/// The fixed part of a record: `crc u64 | kind u8 | key_len u32 | meta_len
/// u32 | body_len u64 | last_used u64`.
const FIXED: usize = 33;
/// Dead bytes past which compaction may run; it also needs them to be more
/// than half the file.
const COMPACT_MIN_DEAD: u64 = 1 << 20;
/// The move's copy chunk.
const COPY_CHUNK: u64 = 64 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Put = 1,
    Meta = 2,
    Delete = 3,
    Touch = 4,
}

impl Kind {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Put),
            2 => Some(Self::Meta),
            3 => Some(Self::Delete),
            4 => Some(Self::Touch),
            _ => None,
        }
    }
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn from_unix_seconds(seconds: u64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .unwrap_or(UNIX_EPOCH)
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("a slice of four bytes"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(
        bytes[at..at + 8]
            .try_into()
            .expect("a slice of eight bytes"),
    )
}

/// One record's bytes, ready for a single `write_at`.
fn encode(
    kind: Kind,
    key: &str,
    meta: &[u8],
    body: &[u8],
    last_used: u64,
) -> Result<Vec<u8>, StoreError> {
    let too_long = |_| StoreError::Other("a key or record is too long for a pack record".into());
    let key_len = u32::try_from(key.len()).map_err(too_long)?;
    let meta_len = u32::try_from(meta.len()).map_err(too_long)?;
    let mut record = Vec::with_capacity(FIXED + key.len() + meta.len() + body.len());
    record.extend_from_slice(&[0; 8]);
    record.push(kind as u8);
    record.extend_from_slice(&key_len.to_le_bytes());
    record.extend_from_slice(&meta_len.to_le_bytes());
    record.extend_from_slice(&(body.len() as u64).to_le_bytes());
    record.extend_from_slice(&last_used.to_le_bytes());
    record.extend_from_slice(key.as_bytes());
    record.extend_from_slice(meta);
    record.extend_from_slice(body);
    let crc = crc64(&record[8..]);
    record[..8].copy_from_slice(&crc.to_le_bytes());
    Ok(record)
}

/// The total length of the record whose fixed part is `fixed`, when its
/// kind is known and its lengths add up.
fn record_len(fixed: &[u8]) -> Option<u64> {
    Kind::from_byte(fixed[8])?;
    let key_len = u64::from(u32_at(fixed, 9));
    let meta_len = u64::from(u32_at(fixed, 13));
    (FIXED as u64)
        .checked_add(key_len)?
        .checked_add(meta_len)?
        .checked_add(u64_at(fixed, 17))
}

/// A record that verified.
struct Record {
    kind: Kind,
    key: String,
    last_used: u64,
    bytes: Vec<u8>,
    meta_start: usize,
    body_start: usize,
}

impl Record {
    /// Verifies `bytes` as one whole record: kind, lengths, checksum, a
    /// UTF-8 key, and a meta and body the kind allows (JSON for put and
    /// meta, nothing for delete and touch, no body for meta).
    fn verify(bytes: Vec<u8>) -> Option<(Self, Option<serde_json::Value>)> {
        if bytes.len() < FIXED || record_len(&bytes[..FIXED])? != bytes.len() as u64 {
            return None;
        }
        if u64_at(&bytes, 0) != crc64(&bytes[8..]) {
            return None;
        }
        let kind = Kind::from_byte(bytes[8])?;
        let meta_start = FIXED + usize::try_from(u32_at(&bytes, 9)).ok()?;
        let body_start = meta_start + usize::try_from(u32_at(&bytes, 13)).ok()?;
        let has_body = body_start < bytes.len();
        let has_meta = meta_start < body_start;
        let meta = match kind {
            Kind::Put => Some(serde_json::from_slice(&bytes[meta_start..body_start]).ok()?),
            Kind::Meta if !has_body => {
                Some(serde_json::from_slice(&bytes[meta_start..body_start]).ok()?)
            }
            Kind::Delete | Kind::Touch if !has_meta && !has_body => None,
            _ => return None,
        };
        let key = std::str::from_utf8(&bytes[FIXED..meta_start])
            .ok()?
            .to_owned();
        let record = Self {
            kind,
            key,
            last_used: u64_at(&bytes, 25),
            bytes,
            meta_start,
            body_start,
        };
        Some((record, meta))
    }

    fn meta_len(&self) -> u64 {
        (self.body_start - self.meta_start) as u64
    }

    fn body(&self) -> &[u8] {
        &self.bytes[self.body_start..]
    }
}

/// Reads and verifies the record at `at` of a file `len` bytes long;
/// `None` when it does not verify.
fn read_record<F: SyncFile>(
    file: &F,
    at: u64,
    len: u64,
) -> io::Result<Option<(Record, Option<serde_json::Value>)>> {
    let room = len.saturating_sub(at);
    if room < FIXED as u64 {
        return Ok(None);
    }
    let mut fixed = [0; FIXED];
    file.read_at(at, &mut fixed)?;
    let Some(total) = record_len(&fixed).filter(|&total| total <= room) else {
        return Ok(None);
    };
    let Ok(total) = usize::try_from(total) else {
        return Ok(None);
    };
    let mut bytes = vec![0; total];
    file.read_at(at, &mut bytes)?;
    Ok(Record::verify(bytes))
}

fn write_journal<F: SyncFile>(file: &F, journal: Option<(u64, u64)>) -> io::Result<()> {
    let mut bytes = [0; JOURNAL_LEN];
    if let Some((from, len)) = journal {
        bytes[..8].copy_from_slice(&from.to_le_bytes());
        bytes[8..16].copy_from_slice(&len.to_le_bytes());
        let crc = crc64(&bytes[..16]);
        bytes[16..].copy_from_slice(&crc.to_le_bytes());
    }
    file.write_at(JOURNAL_AT, &bytes)
}

fn initialise<F: SyncFile>(file: &F) -> io::Result<()> {
    let mut header = [0; HEADER_SIZE];
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&FORMAT.to_le_bytes());
    file.truncate(0)?;
    file.write_at(0, &header)?;
    file.flush()
}

/// Finishes the move of `[from, from + len)` to the front of the log (see
/// the module doc's *Compaction*). `truncated` turns true once the file has
/// been cut to the moved records, after which the source is gone; a call
/// that finds it true only clears the journal.
fn finish_move<F: SyncFile>(file: &F, from: u64, len: u64, truncated: &mut bool) -> io::Result<()> {
    if !*truncated {
        write_journal(file, Some((from, len)))?;
        file.flush()?;
        if file.len()? >= from + len {
            let mut chunk = vec![0; usize::try_from(len.min(COPY_CHUNK)).unwrap_or(0)];
            let mut done = 0;
            while done < len {
                let step = (len - done).min(COPY_CHUNK);
                let buffer = &mut chunk[..usize::try_from(step).unwrap_or(0)];
                file.read_at(from + done, buffer)?;
                file.write_at(HEADER_LEN + done, buffer)?;
                done += step;
            }
            file.flush()?;
        }
        file.truncate(HEADER_LEN + len)?;
        *truncated = true;
    }
    file.flush()?;
    write_journal(file, None)?;
    file.flush()
}

/// The index's view of one entry.
#[derive(Clone, Debug)]
struct Slot {
    meta: Meta,
    /// The put record holding the body.
    record_at: u64,
    record_len: u64,
    body_at: u64,
    /// The length of the one put record compaction would write for it.
    fresh_len: u64,
}

impl Slot {
    fn shift_down(&mut self, by: u64) {
        self.record_at -= by;
        self.body_at -= by;
    }
}

/// What the next write must do first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Repair {
    None,
    /// A write failed: bytes past `end` may be a partial record.
    Truncate,
    /// A compaction's move is unfinished (see [`finish_move`]).
    Move {
        from: u64,
        len: u64,
        truncated: bool,
    },
}

#[derive(Debug)]
struct State {
    slots: FxHashMap<String, Slot>,
    /// The end of the last good record, where the next append goes.
    end: u64,
    /// The sum of the slots' `fresh_len`.
    live: u64,
    repair: Repair,
}

impl State {
    fn insert(&mut self, slot: Slot) {
        self.live += slot.fresh_len;
        if let Some(previous) = self.slots.insert(slot.meta.key.clone(), slot) {
            self.live -= previous.fresh_len;
        }
    }

    fn remove(&mut self, key: &str) {
        if let Some(previous) = self.slots.remove(key) {
            self.live -= previous.fresh_len;
        }
    }

    fn dead(&self) -> u64 {
        self.end
            .saturating_sub(HEADER_LEN)
            .saturating_sub(self.live)
    }

    /// Applies one replayed record found at `at`.
    fn apply(&mut self, at: u64, record: &Record, meta: Option<serde_json::Value>) {
        match (record.kind, meta) {
            (Kind::Put, Some(meta)) => self.insert(Slot {
                meta: Meta {
                    key: record.key.clone(),
                    len: record.body().len() as u64,
                    last_used: from_unix_seconds(record.last_used),
                    record: meta,
                },
                record_at: at,
                record_len: record.bytes.len() as u64,
                body_at: at + record.body_start as u64,
                fresh_len: record.bytes.len() as u64,
            }),
            (Kind::Meta, Some(meta)) => {
                if let Some(slot) = self.slots.get_mut(&record.key) {
                    let fresh_len =
                        FIXED as u64 + record.key.len() as u64 + record.meta_len() + slot.meta.len;
                    self.live = self.live - slot.fresh_len + fresh_len;
                    slot.fresh_len = fresh_len;
                    slot.meta.record = meta;
                    slot.meta.last_used = from_unix_seconds(record.last_used);
                }
            }
            (Kind::Delete, _) => self.remove(&record.key),
            (Kind::Touch, _) => {
                if let Some(slot) = self.slots.get_mut(&record.key) {
                    slot.meta.last_used = from_unix_seconds(record.last_used);
                }
            }
            _ => {}
        }
    }
}

/// A [`Store`] inside one [`SyncFile`]: an append-only log of records with
/// an in-memory index, compacted in place (see the [module doc](self)).
pub struct PackStore<F: SyncFile> {
    file: F,
    volatility: Volatility,
    state: Mutex<State>,
}

impl<F: SyncFile> fmt::Debug for PackStore<F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state();
        formatter
            .debug_struct("PackStore")
            .field("entries", &state.slots.len())
            .field("live_bytes", &state.live)
            .field("file_bytes", &state.end)
            .field("volatility", &self.volatility)
            .finish_non_exhaustive()
    }
}

impl<F: SyncFile> PackStore<F> {
    /// Opens the store in `file`: an empty file is initialised; a file with
    /// another magic or format version is truncated and initialised (a
    /// cache is a cache); a move a compaction left unfinished is finished;
    /// otherwise the log is replayed to rebuild the index, stopping at the
    /// first record that does not verify and truncating the file there (a
    /// torn append from a crash).
    pub fn open(file: F, volatility: Volatility) -> Result<Self, StoreError> {
        let len = Self::prepare(&file)?;
        let mut state = State {
            slots: FxHashMap::default(),
            end: HEADER_LEN,
            live: 0,
            repair: Repair::None,
        };
        while let Some((record, meta)) = read_record(&file, state.end, len)? {
            state.apply(state.end, &record, meta);
            state.end += record.bytes.len() as u64;
        }
        if state.end < len {
            file.truncate(state.end)?;
            file.flush()?;
        }
        Ok(Self {
            file,
            volatility,
            state: Mutex::new(state),
        })
    }

    /// Checks the header, initialising the file or finishing a journalled
    /// move as needed, and answers the file's length.
    fn prepare(file: &F) -> io::Result<u64> {
        let len = file.len()?;
        if len >= HEADER_LEN {
            let mut header = [0; HEADER_SIZE];
            file.read_at(0, &mut header)?;
            if &header[..8] == MAGIC && u32_at(&header, 8) == FORMAT {
                let journal = &header[16..16 + JOURNAL_LEN];
                let from = u64_at(journal, 0);
                let moved = u64_at(journal, 8);
                if from == 0 || u64_at(journal, 16) != crc64(&journal[..16]) {
                    // No move, or a journal torn while it was written or
                    // cleared: the log replays as it stands.
                    return Ok(len);
                }
                if from > HEADER_LEN && HEADER_LEN.checked_add(moved).is_some_and(|end| end <= from)
                {
                    finish_move(file, from, moved, &mut false)?;
                    return file.len();
                }
            }
        }
        initialise(file)?;
        Ok(HEADER_LEN)
    }

    /// Rewrites the live records to the front of the file and truncates.
    /// Runs by itself when dead bytes exceed both 1 MiB and half the file
    /// after a put or a delete, and when a read finds a corrupt record.
    pub fn compact(&self) -> Result<(), StoreError> {
        let mut state = self.state();
        self.compact_locked(&mut state)
    }

    /// The file the store lives in.
    pub fn file(&self) -> &F {
        &self.file
    }

    /// The bytes the live entries would take as one put record each.
    #[must_use]
    pub fn live_bytes(&self) -> u64 {
        self.state().live
    }

    /// The length of the log, header included.
    #[must_use]
    pub fn file_bytes(&self) -> u64 {
        self.state().end
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs the repair a failed write left, before any other write.
    fn settle(&self, state: &mut State) -> Result<(), StoreError> {
        match state.repair {
            Repair::None => Ok(()),
            Repair::Truncate => {
                self.file.truncate(state.end)?;
                state.repair = Repair::None;
                Ok(())
            }
            Repair::Move {
                from,
                len,
                truncated,
            } => {
                let mut now_truncated = truncated;
                let result = finish_move(&self.file, from, len, &mut now_truncated);
                if now_truncated && !truncated {
                    for slot in state.slots.values_mut() {
                        slot.shift_down(from - HEADER_LEN);
                    }
                    state.end = HEADER_LEN + len;
                }
                state.repair = match result {
                    Ok(()) => Repair::None,
                    Err(_) => Repair::Move {
                        from,
                        len,
                        truncated: now_truncated,
                    },
                };
                result.map_err(StoreError::from)
            }
        }
    }

    /// Appends `record` at the end of the log, flushing it when `durable`,
    /// and answers its offset. A failure leaves the tail for repair.
    fn append(&self, state: &mut State, record: &[u8], durable: bool) -> Result<u64, StoreError> {
        self.settle(state)?;
        let at = state.end;
        let written = self
            .file
            .write_at(at, record)
            .and_then(|()| if durable { self.file.flush() } else { Ok(()) });
        if let Err(error) = written {
            state.repair = Repair::Truncate;
            return Err(error.into());
        }
        state.end = at + record.len() as u64;
        Ok(at)
    }

    /// Reads and verifies `slot`'s put record and answers its body.
    fn read_body(&self, slot: &Slot) -> Result<Bytes, StoreError> {
        let corrupt =
            || StoreError::Corrupt(format!("the record of {:?} does not verify", slot.meta.key));
        let len = usize::try_from(slot.record_len).map_err(|_| corrupt())?;
        let mut bytes = vec![0; len];
        self.file.read_at(slot.record_at, &mut bytes)?;
        let (record, _) = Record::verify(bytes).ok_or_else(corrupt)?;
        if record.kind != Kind::Put
            || record.key != slot.meta.key
            || slot.record_at + record.body_start as u64 != slot.body_at
        {
            return Err(corrupt());
        }
        Ok(Bytes::from(record.bytes).slice(record.body_start..))
    }

    fn compact_if_due(&self, state: &mut State) {
        let dead = state.dead();
        if dead > COMPACT_MIN_DEAD && dead > state.end / 2 {
            // The write that made it due succeeded; a failed compaction
            // leaves a consistent store behind (see `compact_locked`).
            let _ = self.compact_locked(state);
        }
    }

    fn compact_locked(&self, state: &mut State) -> Result<(), StoreError> {
        self.settle(state)?;
        if state.dead() == 0 {
            return Ok(());
        }
        let from = state.end;
        let mut order: Vec<(u64, String)> = state
            .slots
            .iter()
            .map(|(key, slot)| (slot.record_at, key.clone()))
            .collect();
        order.sort_unstable();

        // 1. Append a fresh put of every live entry.
        let mut fresh: Vec<Slot> = Vec::with_capacity(order.len());
        let mut at = from;
        for (_, key) in order {
            let slot = &state.slots[&key];
            let body = match self.read_body(slot) {
                Ok(body) => body,
                Err(StoreError::Corrupt(_)) => {
                    state.remove(&key);
                    continue;
                }
                Err(error) => {
                    state.repair = Repair::Truncate;
                    return Err(error);
                }
            };
            let meta = serde_json::to_vec(&slot.meta.record)
                .map_err(|error| StoreError::Other(error.to_string()))?;
            let record = encode(
                Kind::Put,
                &key,
                &meta,
                &body,
                unix_seconds(slot.meta.last_used),
            )?;
            if let Err(error) = self.file.write_at(at, &record) {
                state.repair = Repair::Truncate;
                return Err(error.into());
            }
            let len = record.len() as u64;
            fresh.push(Slot {
                meta: slot.meta.clone(),
                record_at: at,
                record_len: len,
                body_at: at + len - body.len() as u64,
                fresh_len: len,
            });
            at += len;
        }
        if let Err(error) = self.file.flush() {
            state.repair = Repair::Truncate;
            return Err(error.into());
        }
        let moved = at - from;
        if HEADER_LEN + moved > from {
            // Cannot happen (see the module doc), but a move that overlaps
            // its source would not be repeatable: drop the fresh copies.
            state.repair = Repair::Truncate;
            return self.settle(state);
        }

        // 2. and 3. Point the index at the fresh copies and move them.
        for slot in fresh {
            state.insert(slot);
        }
        state.end = at;
        state.repair = Repair::Move {
            from,
            len: moved,
            truncated: false,
        };
        self.settle(state)
    }
}

impl<F: SyncFile> Store for PackStore<F> {
    fn scan(&self) -> Result<Vec<Meta>, StoreError> {
        Ok(self
            .state()
            .slots
            .values()
            .map(|slot| slot.meta.clone())
            .collect())
    }

    fn get(&self, key: &str) -> Result<Option<(Meta, Bytes)>, StoreError> {
        let mut state = self.state();
        let Some(slot) = state.slots.get(key) else {
            return Ok(None);
        };
        match self.read_body(slot) {
            Ok(body) => Ok(Some((slot.meta.clone(), body))),
            Err(error @ StoreError::Corrupt(_)) => {
                state.remove(key);
                // Rewrite the file without the bad record, so a later open
                // does not stop at it; best effort.
                let _ = self.compact_locked(&mut state);
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    fn get_meta(&self, key: &str) -> Result<Option<Meta>, StoreError> {
        Ok(self.state().slots.get(key).map(|slot| slot.meta.clone()))
    }

    fn put(&self, meta: &Meta, body: &[u8]) -> Result<(), StoreError> {
        let json = serde_json::to_vec(&meta.record)
            .map_err(|error| StoreError::Other(error.to_string()))?;
        let last_used = unix_seconds(meta.last_used);
        let record = encode(Kind::Put, &meta.key, &json, body, last_used)?;
        let mut state = self.state();
        let at = self.append(&mut state, &record, true)?;
        let len = record.len() as u64;
        state.insert(Slot {
            meta: Meta {
                key: meta.key.clone(),
                len: body.len() as u64,
                last_used: from_unix_seconds(last_used),
                record: meta.record.clone(),
            },
            record_at: at,
            record_len: len,
            body_at: at + len - body.len() as u64,
            fresh_len: len,
        });
        self.compact_if_due(&mut state);
        Ok(())
    }

    fn put_meta(&self, meta: &Meta) -> Result<(), StoreError> {
        let json = serde_json::to_vec(&meta.record)
            .map_err(|error| StoreError::Other(error.to_string()))?;
        let last_used = unix_seconds(meta.last_used);
        let mut state = self.state();
        if !state.slots.contains_key(&meta.key) {
            return Err(StoreError::Corrupt(format!(
                "no entry for {:?} to update",
                meta.key
            )));
        }
        let record = encode(Kind::Meta, &meta.key, &json, &[], last_used)?;
        self.append(&mut state, &record, true)?;
        let State { slots, live, .. } = &mut *state;
        let slot = slots
            .get_mut(&meta.key)
            .expect("the entry was present under the same lock");
        let fresh_len = record.len() as u64 + slot.meta.len;
        *live = *live - slot.fresh_len + fresh_len;
        slot.fresh_len = fresh_len;
        slot.meta.record = meta.record.clone();
        slot.meta.last_used = from_unix_seconds(last_used);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), StoreError> {
        let mut state = self.state();
        if !state.slots.contains_key(key) {
            return Ok(());
        }
        let record = encode(Kind::Delete, key, &[], &[], 0)?;
        self.append(&mut state, &record, true)?;
        state.remove(key);
        self.compact_if_due(&mut state);
        Ok(())
    }

    fn touch(&self, key: &str, now: SystemTime) -> Result<(), StoreError> {
        let mut state = self.state();
        if !state.slots.contains_key(key) {
            return Ok(());
        }
        let last_used = unix_seconds(now);
        let record = encode(Kind::Touch, key, &[], &[], last_used)?;
        self.append(&mut state, &record, false)?;
        if let Some(slot) = state.slots.get_mut(key) {
            slot.meta.last_used = from_unix_seconds(last_used);
        }
        Ok(())
    }

    fn estimate(&self) -> Estimate {
        Estimate {
            used: self.live_bytes(),
            quota: None,
        }
    }

    fn volatility(&self) -> Volatility {
        self.volatility
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use http::HeaderMap;
    use http::header::{CACHE_CONTROL, HeaderValue};
    use serde_json::json;
    use url::Url;

    use super::*;
    use crate::CachePolicy;
    use crate::artifact::{
        ArtifactCache, ArtifactConfig, ArtifactKey, Artifacts, EngineId, SourceDigest, Variant,
    };
    use crate::byte_cache::{ByteCache, Lookup, Read};
    use crate::platform::Response;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn meta(key: &str, len: usize, seconds: u64) -> Meta {
        Meta {
            key: key.to_owned(),
            len: len as u64,
            last_used: at(seconds),
            record: json!({ "for": key, "len": len }),
        }
    }

    fn put<F: SyncFile>(store: &PackStore<F>, key: &str, body: &[u8], seconds: u64) {
        store.put(&meta(key, body.len(), seconds), body).unwrap();
    }

    /// A body unlike any other key's, `len` bytes long.
    fn body(key: &str, len: usize) -> Vec<u8> {
        key.bytes()
            .cycle()
            .zip(0..len)
            .map(|(byte, index)| byte ^ u8::try_from(index % 256).unwrap())
            .collect()
    }

    fn mem_store() -> PackStore<MemFile> {
        PackStore::open(MemFile::default(), Volatility::Durable).unwrap()
    }

    fn reopen_bytes(bytes: Vec<u8>) -> PackStore<MemFile> {
        PackStore::open(MemFile::new(bytes), Volatility::Durable).unwrap()
    }

    fn reopen(store: &PackStore<MemFile>) -> PackStore<MemFile> {
        reopen_bytes(store.file().bytes())
    }

    /// Every entry, read back through `get`.
    fn contents<F: SyncFile>(store: &PackStore<F>) -> BTreeMap<String, Vec<u8>> {
        store
            .scan()
            .unwrap()
            .into_iter()
            .map(|meta| {
                let (read, body) = store.get(&meta.key).unwrap().expect("a scanned entry");
                assert_eq!(read.len, body.len() as u64);
                (meta.key, body.to_vec())
            })
            .collect()
    }

    fn expected(entries: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
        entries
            .iter()
            .map(|(key, body)| ((*key).to_owned(), body.to_vec()))
            .collect()
    }

    /// A [`MemFile`] whose writes start failing after a number of them; the
    /// failing write lands its first half, as a torn write would.
    struct Flaky {
        file: MemFile,
        writes_left: AtomicUsize,
    }

    impl Flaky {
        fn new(bytes: Vec<u8>, writes: usize) -> Self {
            Self {
                file: MemFile::new(bytes),
                writes_left: AtomicUsize::new(writes),
            }
        }

        fn heal(&self) {
            self.writes_left.store(usize::MAX, Ordering::Relaxed);
        }
    }

    impl SyncFile for Flaky {
        fn len(&self) -> io::Result<u64> {
            self.file.len()
        }

        fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
            self.file.read_at(offset, buffer)
        }

        fn write_at(&self, offset: u64, bytes: &[u8]) -> io::Result<()> {
            if self.writes_left.load(Ordering::Relaxed) == 0 {
                self.file.write_at(offset, &bytes[..bytes.len() / 2])?;
                return Err(io::Error::other("injected write failure"));
            }
            self.writes_left.fetch_sub(1, Ordering::Relaxed);
            self.file.write_at(offset, bytes)
        }

        fn truncate(&self, len: u64) -> io::Result<()> {
            self.file.truncate(len)
        }

        fn flush(&self) -> io::Result<()> {
            self.file.flush()
        }
    }

    #[test]
    fn a_put_reads_back_through_get_get_meta_and_scan_and_a_put_replaces() {
        let store = mem_store();
        assert!(store.get("a").unwrap().is_none());
        put(&store, "a", b"alpha", 10);
        put(&store, "b", b"beta", 11);
        let (read, bytes) = store.get("a").unwrap().unwrap();
        assert_eq!(&bytes[..], b"alpha");
        assert_eq!(
            (read.key.as_str(), read.len, read.last_used),
            ("a", 5, at(10))
        );
        assert_eq!(read.record, json!({ "for": "a", "len": 5 }));
        assert_eq!(store.get_meta("b").unwrap().unwrap().last_used, at(11));
        assert_eq!(
            contents(&store),
            expected(&[("a", b"alpha"), ("b", b"beta")])
        );

        put(&store, "a", b"another", 12);
        assert_eq!(
            contents(&store),
            expected(&[("a", b"another"), ("b", b"beta")])
        );
        assert_eq!(store.estimate().used, store.live_bytes());
        assert_eq!(store.estimate().quota, None);
        assert_eq!(store.volatility(), Volatility::Durable);
        assert!(store.map_body("a").is_none());
    }

    #[test]
    fn put_meta_keeps_the_body_and_touch_moves_last_used() {
        let store = mem_store();
        put(&store, "a", b"alpha", 10);
        let mut updated = meta("a", 5, 20);
        updated.record = json!({ "refreshed": true });
        store.put_meta(&updated).unwrap();
        let (read, bytes) = store.get("a").unwrap().unwrap();
        assert_eq!(&bytes[..], b"alpha");
        assert_eq!(
            (read.record, read.last_used),
            (json!({ "refreshed": true }), at(20))
        );
        assert!(matches!(
            store.put_meta(&meta("absent", 0, 0)),
            Err(StoreError::Corrupt(_))
        ));

        store.touch("a", at(30)).unwrap();
        assert_eq!(store.get_meta("a").unwrap().unwrap().last_used, at(30));
        let before = store.file_bytes();
        store.touch("absent", at(30)).unwrap();
        assert_eq!(
            store.file_bytes(),
            before,
            "touching nothing writes nothing"
        );

        let reopened = reopen(&store);
        let read = reopened.get_meta("a").unwrap().unwrap();
        assert_eq!(
            (read.record, read.last_used, read.len),
            (json!({ "refreshed": true }), at(30), 5)
        );
        assert_eq!(contents(&reopened), expected(&[("a", b"alpha")]));
    }

    #[test]
    fn delete_leaves_a_tombstone_and_deleting_nothing_writes_nothing() {
        let store = mem_store();
        put(&store, "a", b"alpha", 10);
        put(&store, "b", b"beta", 10);
        store.delete("a").unwrap();
        assert!(store.get("a").unwrap().is_none());
        assert!(store.get_meta("a").unwrap().is_none());
        let before = store.file_bytes();
        store.delete("a").unwrap();
        assert_eq!(store.file_bytes(), before);
        assert_eq!(contents(&reopen(&store)), expected(&[("b", b"beta")]));
    }

    #[test]
    fn reopening_the_file_replays_the_same_contents() {
        let store = mem_store();
        put(&store, "a", b"alpha", 10);
        put(&store, "b", b"beta", 11);
        put(&store, "a", b"again", 12);
        put(&store, "c", b"", 13);
        store.delete("b").unwrap();
        store.touch("c", at(14)).unwrap();
        let reopened = reopen(&store);
        assert_eq!(contents(&reopened), contents(&store));
        assert_eq!(reopened.file_bytes(), store.file_bytes());
        assert_eq!(reopened.live_bytes(), store.live_bytes());
        for key in ["a", "c"] {
            let (left, right) = (
                store.get_meta(key).unwrap().unwrap(),
                reopened.get_meta(key).unwrap().unwrap(),
            );
            assert_eq!(
                (left.last_used, left.record, left.len),
                (right.last_used, right.record, right.len)
            );
        }
    }

    #[test]
    fn an_empty_file_is_initialised() {
        let store = mem_store();
        assert_eq!(store.file_bytes(), HEADER_LEN);
        let bytes = store.file().bytes();
        assert_eq!(bytes.len() as u64, HEADER_LEN);
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(u32_at(&bytes, 8), FORMAT);
        assert!(store.scan().unwrap().is_empty());
    }

    #[test]
    fn a_file_with_foreign_magic_or_another_format_is_reinitialised() {
        let store = mem_store();
        put(&store, "a", b"alpha", 10);
        let good = store.file().bytes();

        let mut foreign = good.clone();
        foreign[..8].copy_from_slice(b"NOTAPACK");
        let mut newer = good.clone();
        newer[8..12].copy_from_slice(&2u32.to_le_bytes());
        let short = b"VANTPACK".to_vec();
        for bytes in [foreign, newer, short, vec![7; 1000]] {
            let reopened = reopen_bytes(bytes);
            assert!(reopened.scan().unwrap().is_empty());
            assert_eq!(reopened.file().bytes(), good[..HEADER_SIZE]);
        }
    }

    #[test]
    fn a_torn_append_recovers_every_earlier_entry_and_cuts_the_file_there() {
        let store = mem_store();
        put(&store, "a", b"alpha", 10);
        put(&store, "b", b"beta", 11);
        let good = store.file_bytes();
        put(&store, "c", b"the last record", 12);
        let bytes = store.file().bytes();
        for cut in usize::try_from(good).unwrap()..bytes.len() {
            let reopened = reopen_bytes(bytes[..cut].to_vec());
            assert_eq!(
                contents(&reopened),
                expected(&[("a", b"alpha"), ("b", b"beta")]),
                "cut at {cut}"
            );
            assert_eq!(reopened.file_bytes(), good);
            assert_eq!(
                reopened.file().len().unwrap(),
                good,
                "the torn record is cut off"
            );
            put(&reopened, "d", b"after", 13);
            assert_eq!(contents(&reopen(&reopened)).len(), 3);
        }
    }

    #[test]
    fn a_flipped_bit_in_a_body_is_corrupt_and_the_entry_disappears() {
        let store = mem_store();
        put(&store, "a", b"alpha", 10);
        put(&store, "b", b"the body of b", 11);
        put(&store, "c", b"gamma", 12);
        {
            let mut bytes = store.file().contents();
            let at = bytes
                .windows(13)
                .position(|window| window == b"the body of b")
                .unwrap();
            bytes[at + 4] ^= 0x10;
        }
        assert!(matches!(store.get("b"), Err(StoreError::Corrupt(_))));
        assert!(store.get_meta("b").unwrap().is_none());
        assert!(store.get("b").unwrap().is_none());
        put(&store, "d", b"delta", 13);
        let left = expected(&[("a", b"alpha"), ("c", b"gamma"), ("d", b"delta")]);
        assert_eq!(contents(&store), left);
        assert_eq!(
            contents(&reopen(&store)),
            left,
            "the bad record is gone from the file"
        );
    }

    #[test]
    fn compaction_keeps_every_live_entry_and_shrinks_the_file() {
        let store = mem_store();
        for round in 0..3 {
            for key in ["a", "b", "c", "d", "e"] {
                put(&store, key, &body(key, 100 + round * 10), round as u64);
            }
        }
        store.delete("b").unwrap();
        store.touch("c", at(50)).unwrap();
        let mut refreshed = meta("d", 120, 60);
        refreshed.record = json!({ "a longer record": "than the one it replaces" });
        store.put_meta(&refreshed).unwrap();
        let before = (contents(&store), store.file_bytes());

        store.compact().unwrap();
        assert_eq!(contents(&store), before.0);
        assert!(store.file_bytes() < before.1);
        assert_eq!(store.file_bytes(), HEADER_LEN + store.live_bytes());
        assert_eq!(store.file().len().unwrap(), store.file_bytes());
        assert_eq!(store.get_meta("c").unwrap().unwrap().last_used, at(50));
        assert_eq!(
            store.get_meta("d").unwrap().unwrap().record,
            refreshed.record
        );

        let reopened = reopen(&store);
        assert_eq!(contents(&reopened), before.0);
        assert_eq!(reopened.file_bytes(), store.file_bytes());
        assert_eq!(
            reopened.get_meta("d").unwrap().unwrap().record,
            refreshed.record
        );
        let compacted = store.file_bytes();
        store.compact().unwrap();
        assert_eq!(store.file_bytes(), compacted, "nothing dead, nothing to do");
    }

    #[test]
    fn compaction_runs_by_itself_past_the_thresholds() {
        let store = mem_store();
        put(&store, "small", b"small", 0);
        let big = 600 << 10;
        put(&store, "big", &body("one", big), 1);
        put(&store, "big", &body("two", big), 2);
        assert!(
            store.file_bytes() > HEADER_LEN + store.live_bytes(),
            "under 1 MiB dead: no compaction"
        );
        put(&store, "big", &body("three", big), 3);
        assert_eq!(store.file_bytes(), HEADER_LEN + store.live_bytes());
        assert_eq!(store.file().len().unwrap(), store.file_bytes());
        let left = BTreeMap::from([
            ("big".to_owned(), body("three", big)),
            ("small".to_owned(), b"small".to_vec()),
        ]);
        assert_eq!(contents(&store), left);
        assert_eq!(contents(&reopen(&store)), left);
    }

    #[test]
    fn a_crash_at_any_write_of_a_compaction_reopens_to_every_entry() {
        let store = mem_store();
        let mut want = BTreeMap::new();
        for (index, key) in ["a", "b", "c", "d", "e", "f"].into_iter().enumerate() {
            let first = body(key, 40_000 + index * 9_000);
            put(&store, key, &first, 1);
            want.insert(key.to_owned(), first);
        }
        for key in ["b", "d"] {
            let second = body(&key.repeat(3), 70_000);
            put(&store, key, &second, 2);
            want.insert(key.to_owned(), second);
        }
        store.delete("e").unwrap();
        want.remove("e");
        store.touch("a", at(3)).unwrap();
        assert_eq!(contents(&store), want);
        let bytes = store.file().bytes();

        let mut writes = 0;
        loop {
            let flaky =
                PackStore::open(Flaky::new(bytes.clone(), writes), Volatility::Durable).unwrap();
            let result = flaky.compact();
            if let Err(error) = &result {
                assert!(
                    matches!(error, StoreError::Io(_)),
                    "write {writes}: {error}"
                );
            }

            // A crash here: the bytes as they stand reopen to every entry.
            let crashed = reopen_bytes(flaky.file().file.bytes());
            assert_eq!(contents(&crashed), want, "a crash at write {writes}");

            // No crash: the same store reads every entry and writes again.
            flaky.file().heal();
            assert_eq!(contents(&flaky), want, "in process after write {writes}");
            put(&flaky, "z", b"after", 4);
            let mut after = want.clone();
            after.insert("z".to_owned(), b"after".to_vec());
            assert_eq!(contents(&reopen_bytes(flaky.file().file.bytes())), after);

            if result.is_ok() {
                assert_eq!(crashed.file_bytes(), HEADER_LEN + crashed.live_bytes());
                break;
            }
            writes += 1;
            assert!(writes < 1_000, "the compaction never completed");
        }
        assert!(writes > 10, "the copy took several writes ({writes})");
    }

    #[test]
    fn an_io_error_surfaces_as_io_and_leaves_the_index_consistent() {
        let store = PackStore::open(Flaky::new(Vec::new(), 2), Volatility::Durable).unwrap();
        // Initialising took the first write.
        put(&store, "a", b"alpha", 1);
        let before = store.file_bytes();
        assert!(matches!(
            store.put(&meta("a", 9, 2), b"replaced!"),
            Err(StoreError::Io(_))
        ));
        assert!(matches!(store.delete("a"), Err(StoreError::Io(_))));
        assert!(matches!(store.touch("a", at(3)), Err(StoreError::Io(_))));
        assert_eq!(store.file_bytes(), before);
        assert_eq!(contents(&store), expected(&[("a", b"alpha")]));
        assert_eq!(store.get_meta("a").unwrap().unwrap().last_used, at(1));

        store.file().heal();
        put(&store, "b", b"beta", 4);
        let left = expected(&[("a", b"alpha"), ("b", b"beta")]);
        assert_eq!(contents(&store), left);
        let reopened = reopen_bytes(store.file().file.bytes());
        assert_eq!(
            contents(&reopened),
            left,
            "the torn write was cut before the next append"
        );
        assert_eq!(reopened.file_bytes(), store.file_bytes());
    }

    fn response(body: &[u8]) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=1000"));
        Response {
            url: Url::parse("https://a.test/x.js").unwrap(),
            status: 200,
            headers,
            body: Bytes::copy_from_slice(body),
        }
    }

    #[test]
    fn the_byte_tier_over_a_pack_hits_after_a_reopen() {
        let cache = ByteCache::open(mem_store(), 1 << 20);
        assert!(cache.store(
            "https://a.test/x.js",
            Some("text/javascript"),
            &response(b"code"),
            at(100)
        ));
        let bytes = cache.store_ref().file().bytes();
        drop(cache);

        let reopened = ByteCache::open(reopen_bytes(bytes), 1 << 20);
        match reopened.lookup(
            "https://a.test/x.js",
            CachePolicy::Default,
            at(101),
            Read::Owned,
        ) {
            Lookup::UseStored(entry, body) => {
                assert_eq!(&body[..], b"code");
                assert_eq!(entry.media_type.as_deref(), Some("text/javascript"));
            }
            other => panic!("expected a hit, got {other:?}"),
        }
    }

    #[test]
    fn the_artifact_tier_over_a_pack_preloads_after_a_reopen() {
        let engine = EngineId(42);
        let key = ArtifactKey {
            source: SourceDigest::of(b"text"),
            variant: Variant::of(&[b"module"]),
        };
        let open = |store| {
            ArtifactCache::open(
                Arc::new(Artifacts::new(1 << 20)),
                store,
                engine,
                ArtifactConfig::default(),
                at(100),
            )
        };
        let cache = open(mem_store());
        cache
            .memory()
            .insert(engine, key, Bytes::from_static(b"bytecode"));
        assert_eq!(cache.flush(at(100)), 1);
        let bytes = cache.store_ref().file().bytes();
        drop(cache);

        let reopened = open(reopen_bytes(bytes));
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened.preload(key.source, at(101)), 1);
        assert_eq!(
            reopened.memory().get(engine, key).as_deref(),
            Some(&b"bytecode"[..])
        );
    }

    /// A file in the temporary directory, removed on drop.
    #[cfg(any(unix, windows))]
    struct TempPath(std::path::PathBuf);

    #[cfg(any(unix, windows))]
    impl TempPath {
        fn new(name: &str) -> Self {
            Self(std::env::temp_dir().join(format!("vant-pack-{}-{name}", std::process::id())))
        }

        fn open(&self) -> std::fs::File {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&self.0)
                .unwrap()
        }
    }

    #[cfg(any(unix, windows))]
    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_pack_in_a_real_file_survives_a_reopen_and_a_compaction() {
        let path = TempPath::new("reopen");
        let store = PackStore::open(path.open(), Volatility::PurgedWhenIdle).unwrap();
        assert_eq!(store.volatility(), Volatility::PurgedWhenIdle);
        put(&store, "a", b"alpha", 1);
        put(&store, "b", &body("b", 5000), 2);
        put(&store, "a", b"again", 3);
        store.delete("b").unwrap();
        put(&store, "c", b"gamma", 4);
        drop(store);

        let left = expected(&[("a", b"again"), ("c", b"gamma")]);
        let reopened = PackStore::open(path.open(), Volatility::Durable).unwrap();
        assert_eq!(contents(&reopened), left);
        reopened.compact().unwrap();
        assert_eq!(
            reopened.file().len().unwrap(),
            HEADER_LEN + reopened.live_bytes()
        );
        drop(reopened);
        assert_eq!(
            contents(&PackStore::open(path.open(), Volatility::Durable).unwrap()),
            left
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn a_real_file_reads_short_as_unexpected_eof() {
        let path = TempPath::new("short");
        let file = path.open();
        file.write_at(0, b"four").unwrap();
        let mut buffer = [0; 8];
        assert_eq!(
            SyncFile::read_at(&file, 0, &mut buffer).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(SyncFile::len(&file).unwrap(), 4);
    }
}
