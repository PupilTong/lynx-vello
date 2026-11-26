//! Retained font bytes shared with Parley without copying the payload.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use parley::fontique::Blob;

type Payload = dyn AsRef<[u8]> + Send + Sync;

#[derive(Clone, Debug)]
pub struct FontBlob(Blob<u8>);

struct Shared {
    address: usize,
    len: usize,
    payload: Weak<Payload>,
    id: u64,
}

static SHARED: Mutex<Vec<Shared>> = Mutex::new(Vec::new());

impl FontBlob {
    pub fn new<Data>(data: Data) -> Self
    where
        Data: AsRef<[u8]> + Send + Sync + 'static,
    {
        let bytes = data.as_ref();
        let (address, len) = (bytes.as_ptr().addr(), bytes.len());
        let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
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

    #[must_use]
    pub fn from_static(data: &'static [u8]) -> Self {
        Self::new(data)
    }

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

        assert_ne!(FontBlob::new(shared).id(), first);
    }
}
