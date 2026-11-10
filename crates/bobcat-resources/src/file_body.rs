//! A local file's whole contents as [`Bytes`]: read into the heap, or mapped.
//!
//! What decides is how long the consumer keeps the bytes. A script is decoded
//! to text and an image to pixels as soon as they arrive, so their bytes are
//! read and dropped. A font is retained by the text engine for as long as
//! its view lives, so a heap copy of it would be private memory held that
//! long, once per view: a mapping is the file's own page-cache pages instead,
//! which the system can drop and read back, and every view mapping one file
//! is handed the same mapping.

use std::fs::{File, Metadata};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use bytes::Bytes;
use memmap2::Mmap;

/// How a file's contents are handed over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    /// Read into a buffer of their own, for bytes consumed on arrival.
    Read,
    /// Mapped read-only, for bytes retained as long as their consumer lives.
    ///
    /// One unchanged file is one mapping, process-wide: a second read while
    /// any handle to the first is alive answers with the same memory, which
    /// is what lets a [`FontBlob`](bobcat_core::FontBlob) made from it be the
    /// one every view already registered.
    ///
    /// The bytes are the file's for as long as they are held, under the
    /// contract the platform's own font files are mapped with: a file
    /// rewritten in place changes under its readers, and one truncated
    /// faults them. A file replaced by a rename — every write `FsStore`
    /// makes — leaves the mapping on the old contents.
    Mapped,
}

/// The contents of the file at `path`, handed over as `body` says.
pub fn read(path: &Path, body: Body) -> io::Result<Bytes> {
    match body {
        Body::Read => std::fs::read(path).map(Bytes::from),
        Body::Mapped => map(path),
    }
}

/// What a file is told apart by: which file it is and which contents it has.
/// A file whose length or modification time moved is another key, and so
/// another mapping, beside any still held on the old contents.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileKey {
    device: u64,
    inode: u64,
    len: u64,
    modified: (i64, i64),
}

impl FileKey {
    /// `None` where the platform gives no file identity to key on: every
    /// read there is its own mapping.
    #[cfg_attr(
        unix,
        expect(
            clippy::unnecessary_wraps,
            reason = "one signature for the platforms with a file identity and those without"
        )
    )]
    fn of(metadata: &Metadata) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            Some(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                len: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            None
        }
    }
}

/// Every live mapping [`map`] made, weakly: the table never keeps a file
/// mapped, and an entry whose mapping is gone is dropped by the next miss.
/// Only retained resources are mapped — tens per process — so a scan is all
/// a lookup needs.
static MAPPINGS: Mutex<Vec<(FileKey, Weak<Mmap>)>> = Mutex::new(Vec::new());

/// A shared mapping as the owner of [`Bytes`].
struct Mapping(Arc<Mmap>);

impl AsRef<[u8]> for Mapping {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[expect(
    unsafe_code,
    reason = "memory-mapping a file is unsafe in memmap2; the invariant is stated at the call"
)]
fn map(path: &Path) -> io::Result<Bytes> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if metadata.len() == 0 {
        // Nothing to map, and some platforms refuse an empty mapping.
        return Ok(Bytes::new());
    }
    let key = FileKey::of(&metadata);
    let mut mappings = MAPPINGS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(key) = key
        && let Some(mapping) = mappings
            .iter()
            .filter(|(entry, _)| *entry == key)
            .find_map(|(_, mapping)| mapping.upgrade())
    {
        return Ok(Bytes::from_owner(Mapping(mapping)));
    }
    // SAFETY: the mapping is read-only, and nothing in this process writes
    // a mapped file in place: `FsStore`, the one writer here, replaces
    // its bodies by a rename. Another process changing the file in
    // place is the hazard every mapped font carries, the platform's own
    // included; `Body::Mapped` states it for its callers.
    let mapping = Arc::new(unsafe { Mmap::map(&file)? });
    mappings.retain(|(_, mapping)| mapping.strong_count() > 0);
    if let Some(key) = key {
        mappings.push((key, Arc::downgrade(&mapping)));
    }
    Ok(Bytes::from_owner(Mapping(mapping)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, contents: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bobcat-resources-file-body-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("face.ttf");
        std::fs::write(&path, contents).expect("write");
        path
    }

    #[test]
    fn both_bodies_hold_the_file_contents() {
        let path = temp_file("contents", b"font bytes");

        assert_eq!(&read(&path, Body::Read).expect("read")[..], b"font bytes");
        assert_eq!(&read(&path, Body::Mapped).expect("map")[..], b"font bytes");
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[cfg(unix)]
    #[test]
    fn one_unchanged_file_is_one_mapping_while_it_is_held() {
        let path = temp_file("shared", b"one face");

        let first = read(&path, Body::Mapped).expect("map");
        let second = read(&path, Body::Mapped).expect("map");
        assert_eq!(first.as_ptr(), second.as_ptr());

        // A heap read is a buffer of its own.
        let copied = read(&path, Body::Read).expect("read");
        assert_ne!(copied.as_ptr(), first.as_ptr());

        // Replaced by a rename, the path names another file: a new mapping,
        // while the held one keeps the old contents.
        let replacement = path.with_extension("tmp");
        std::fs::write(&replacement, b"two faces").expect("write");
        std::fs::rename(&replacement, &path).expect("rename");
        let third = read(&path, Body::Mapped).expect("map");
        assert_eq!(&third[..], b"two faces");
        assert_eq!(&first[..], b"one face");
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn an_empty_file_maps_to_no_bytes() {
        let path = temp_file("empty", b"");

        assert!(read(&path, Body::Mapped).expect("map").is_empty());
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }
}
