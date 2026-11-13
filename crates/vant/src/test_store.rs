//! An in-memory [`Store`] for the tiers' tests, with knobs for the failures
//! a real one has, and an [`AsyncStore`] over it whose calls a test can hold
//! pending.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::SystemTime;

use bytes::Bytes;

use crate::platform::{AsyncStore, Estimate, Meta, Store, StoreError, Volatility};

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

/// One kind of [`AsyncStore`] call, as [`AsyncMemStore::hold`] names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Op {
    Get,
    Put,
    Delete,
    Touch,
}

/// A [`MemStore`] behind the [`AsyncStore`] interface, whose calls of a held
/// kind stay pending until the test releases that kind. Nothing wakes a
/// pending call: the tests poll by hand.
///
/// A `get` reads the entry when it is called and answers when it is
/// released; a `put`, `delete` or `touch` takes effect when it is released.
#[derive(Clone, Default)]
pub(crate) struct AsyncMemStore {
    pub(crate) inner: MemStore,
    pub(crate) held: Arc<Mutex<HashSet<Op>>>,
    pub(crate) gets: Arc<AtomicUsize>,
}

impl AsyncMemStore {
    pub(crate) fn hold(&self, op: Op) {
        self.held.lock().unwrap().insert(op);
    }

    pub(crate) fn release(&self, op: Op) {
        self.held.lock().unwrap().remove(&op);
    }

    /// A future that is pending while `op` is held, then answers `run()`.
    fn gated<T>(&self, op: Op, run: impl FnOnce() -> T + Send) -> impl Future<Output = T> + Send {
        let held = Arc::clone(&self.held);
        let mut run = Some(run);
        std::future::poll_fn(move |_| {
            if held.lock().unwrap().contains(&op) {
                return Poll::Pending;
            }
            Poll::Ready(run.take().expect("polled after it answered")())
        })
    }
}

impl AsyncStore for AsyncMemStore {
    fn scan(&self) -> impl Future<Output = Result<Vec<Meta>, StoreError>> + Send {
        std::future::ready(Store::scan(&self.inner))
    }

    fn get(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<(Meta, Bytes)>, StoreError>> + Send {
        self.gets.fetch_add(1, Ordering::Relaxed);
        let answer = Store::get(&self.inner, key);
        self.gated(Op::Get, move || answer)
    }

    fn put(&self, meta: &Meta, body: Bytes) -> impl Future<Output = Result<(), StoreError>> + Send {
        let (inner, meta) = (self.inner.clone(), meta.clone());
        self.gated(Op::Put, move || Store::put(&inner, &meta, &body))
    }

    fn delete(&self, key: &str) -> impl Future<Output = Result<(), StoreError>> + Send {
        let (inner, key) = (self.inner.clone(), key.to_owned());
        self.gated(Op::Delete, move || Store::delete(&inner, &key))
    }

    fn touch(
        &self,
        key: &str,
        now: SystemTime,
    ) -> impl Future<Output = Result<(), StoreError>> + Send {
        let (inner, key) = (self.inner.clone(), key.to_owned());
        self.gated(Op::Touch, move || Store::touch(&inner, &key, now))
    }

    fn estimate(&self) -> impl Future<Output = Estimate> + Send {
        std::future::ready(Store::estimate(&self.inner))
    }

    fn volatility(&self) -> Volatility {
        Volatility::EvictableAnytime
    }
}

/// Polls `future` once, with a waker that does nothing.
pub(crate) fn poll_once<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Runs `future` to its answer. Every [`AsyncMemStore`] call answers on its
/// first poll unless its kind is held, so a future that is still pending
/// after a few polls is waiting on a held call, which is a test bug.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    for _ in 0..64 {
        if let Poll::Ready(output) = poll_once(future.as_mut()) {
            return output;
        }
    }
    panic!("the future waits on a held store call")
}
