//! An in-memory [`Store`] for the tiers' tests, with knobs for the failures
//! a real one has.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use bytes::Bytes;

use crate::platform::{Estimate, Meta, Store, StoreError, Volatility};

/// Clones share their contents, so a test can reopen a tier over the same
/// entries, or open two tiers over one Store as two processes would.
#[derive(Clone, Default)]
pub(crate) struct MemStore {
    pub(crate) entries: Arc<Mutex<HashMap<String, (Meta, Bytes)>>>,
    pub(crate) quota: Option<u64>,
    pub(crate) maps: bool,
    pub(crate) fail_put: Arc<AtomicBool>,
    pub(crate) touches: Arc<AtomicUsize>,
}

impl MemStore {
    pub(crate) fn entries(&self) -> MutexGuard<'_, HashMap<String, (Meta, Bytes)>> {
        self.entries.lock().unwrap()
    }

    /// Files `meta` and `body` under `slot`, whatever key `meta` names:
    /// a corrupt record, a hash collision, a truncated body.
    pub(crate) fn forge(&self, slot: &str, meta: Meta, body: &[u8]) {
        self.entries()
            .insert(slot.to_owned(), (meta, Bytes::copy_from_slice(body)));
    }

    pub(crate) fn holds(&self, key: &str) -> bool {
        self.entries().contains_key(key)
    }

    pub(crate) fn persisted_last_used(&self, key: &str) -> SystemTime {
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
