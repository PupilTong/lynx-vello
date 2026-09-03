//! Registered contents: resources the embedder already holds, served under
//! a URL of its choosing ahead of every other transport.
//!
//! This is what lets one fetcher replace the in-memory ones the embedders
//! carried: a CLI registers the scripts and stylesheet it decoded out of a
//! bundle under `bobcat-memory://` URLs, a browser host registers the bytes
//! its own `fetch` produced under their response URLs, and a test registers
//! a PNG. A registered URL resolves and fetches like any other, with any
//! scheme at all, and is never cached — it is already resident.

use std::sync::{Arc, Mutex};

use bobcat_core::PreparsedStyleSheet;
use bytes::Bytes;
use rustc_hash::FxHashMap;
use url::Url;

use crate::mime::MediaType;

/// One registered resource.
#[derive(Clone, Debug)]
pub enum Registered {
    /// Bytes with the media type they were registered under, if any; an
    /// absent type is sniffed like a response without a `Content-Type`.
    Bytes {
        bytes: Bytes,
        media_type: Option<MediaType>,
    },
    /// A stylesheet the host already parsed; it answers only
    /// `fetch_style_sheet`, since it has no bytes to give a byte request.
    StyleSheet(Arc<PreparsedStyleSheet>),
}

/// The registered contents, keyed by normalized URL. Shared with the IO
/// workers, which serve image loads from it, so it locks rather than
/// borrows.
#[derive(Debug, Default)]
pub struct Registry {
    entries: Mutex<FxHashMap<String, Registered>>,
}

impl Registry {
    pub(crate) fn insert(&self, url: &Url, registered: Registered) {
        self.lock().insert(url.to_string(), registered);
    }

    pub(crate) fn get(&self, url: &Url) -> Option<Registered> {
        self.lock().get(url.as_str()).cloned()
    }

    pub(crate) fn contains(&self, url: &Url) -> bool {
        self.lock().contains_key(url.as_str())
    }

    pub(crate) fn remove(&self, url: &Url) -> Option<Registered> {
        self.lock().remove(url.as_str())
    }

    pub(crate) fn clear(&self) {
        self.lock().clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FxHashMap<String, Registered>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
