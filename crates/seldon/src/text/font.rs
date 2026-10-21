//! Retained font bytes shared with Parley without copying the payload.

use std::sync::Arc;

use parley::fontique::Blob;

/// Owned font bytes retained by the text engine.
///
/// `FontBlob::new` moves any owned, thread-safe byte container into Parley's
/// shared resource handle. The font payload is not copied; cloning a
/// `FontBlob` only increments the handle's reference count.
#[derive(Clone, Debug)]
pub struct FontBlob(Blob<u8>);

impl FontBlob {
    /// Wraps an owned byte container without copying its payload.
    pub fn new<Data>(data: Data) -> Self
    where
        Data: AsRef<[u8]> + Send + Sync + 'static,
    {
        Self(Blob::new(Arc::new(data)))
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
}
