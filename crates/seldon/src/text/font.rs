//! Retained font bytes shared with Parley without copying the payload.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use parley::fontique::Blob;

type Payload = dyn AsRef<[u8]> + Send + Sync;

/// Owned font bytes retained by the text engine.
///
/// `FontBlob::new` moves any owned, thread-safe byte container into Parley's
/// shared resource handle. The font payload is not copied; cloning a
/// `FontBlob` only increments the handle's reference count.
///
/// One payload is one blob, process-wide: wrapping bytes a live blob already
/// wraps — a second handle to the same shared buffer or memory mapping, as
/// every view loading one font from one file or one registered container
/// holds — returns that blob rather than a second one. Every view then
/// registers the same blob, so the caches keyed by a blob's id (parsed font
/// tables, outlines, glyph atlases) are filled once for all of them.
#[derive(Clone, Debug)]
pub struct FontBlob(Blob<u8>);

/// A blob that may still be alive, under the address and length of its
/// payload. Two live payloads at one address and length are the same bytes.
struct Shared {
    address: usize,
    len: usize,
    payload: Weak<Payload>,
    id: u64,
}

/// Every blob [`FontBlob::new`] made, weakly: the table never keeps a font
/// alive, and an entry whose blob is gone is dropped by the next miss. A
/// process registers tens of fonts, so a scan is all a lookup needs.
static SHARED: Mutex<Vec<Shared>> = Mutex::new(Vec::new());

impl FontBlob {
    /// Wraps an owned byte container without copying its payload, or returns
    /// the live blob that already wraps the same bytes.
    pub fn new<Data>(data: Data) -> Self
    where
        Data: AsRef<[u8]> + Send + Sync + 'static,
    {
        let bytes = data.as_ref();
        let (address, len) = (bytes.as_ptr().addr(), bytes.len());
        let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
        // While an entry upgrades, its payload is alive, so no other
        // allocation can occupy its address: a match is the same memory.
        if let Some(blob) = shared
            .iter()
            .filter(|entry| entry.address == address && entry.len == len)
            .find_map(|entry| {
                let payload = entry.payload.upgrade()?;
                Some(Blob::from_raw_parts(payload, entry.id))
            })
        {
            return Self(blob);
        }
        shared.retain(|entry| entry.payload.strong_count() > 0);
        let blob = Blob::new(Arc::new(data));
        let (payload, id) = blob.clone().into_raw_parts();
        shared.push(Shared {
            address,
            len,
            payload: Arc::downgrade(&payload),
            id,
        });
        Self(blob)
    }

    /// Wraps program-lifetime font bytes without copying their payload.
    #[must_use]
    pub fn from_static(data: &'static [u8]) -> Self {
        Self::new(data)
    }

    /// Copies borrowed bytes into an owned font blob.
    ///
    /// Use this only when the caller cannot transfer ownership of the backing
    /// allocation. Runtime resource loaders should prefer [`Self::new`].
    #[must_use]
    pub fn copy_from_slice(data: &[u8]) -> Self {
        Self::new(data.to_vec())
    }

    pub(super) fn into_inner(self) -> Blob<u8> {
        self.0
    }

    #[cfg(test)]
    pub(super) fn id(&self) -> u64 {
        self.0.id()
    }
}

impl AsRef<[u8]> for FontBlob {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl From<Vec<u8>> for FontBlob {
    fn from(data: Vec<u8>) -> Self {
        Self::new(data)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn owned_vector_keeps_its_payload_allocation() {
        let data = vec![1, 2, 3, 4];
        let original = data.as_ptr();

        let blob = FontBlob::new(data);

        assert_eq!(blob.as_ref().as_ptr(), original);
    }

    #[test]
    fn handles_to_one_payload_share_one_blob() {
        static BYTES: [u8; 4] = [1, 2, 3, 4];
        let shared: Arc<[u8]> = Arc::from(vec![5, 6, 7, 8]);

        let first = FontBlob::new(Arc::clone(&shared));
        let second = FontBlob::new(Arc::clone(&shared));
        assert_eq!(first.id(), second.id());
        assert_eq!(
            FontBlob::from_static(&BYTES).id(),
            FontBlob::from_static(&BYTES).id()
        );

        // A prefix of the payload is other bytes, and so another blob.
        let prefix = FontBlob::from_static(&BYTES[..2]);
        assert_ne!(prefix.id(), FontBlob::from_static(&BYTES).id());
    }

    #[test]
    fn equal_bytes_elsewhere_are_another_blob() {
        let first = FontBlob::new(vec![1, 2, 3, 4]);
        let second = FontBlob::new(vec![1, 2, 3, 4]);

        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn a_dropped_blob_is_not_revived() {
        let shared: Arc<[u8]> = Arc::from(vec![9, 9, 9]);
        let first = FontBlob::new(Arc::clone(&shared)).id();

        // The table held the first blob weakly, so wrapping the same payload
        // again after it is gone makes a new one.
        assert_ne!(FontBlob::new(shared).id(), first);
    }
}
