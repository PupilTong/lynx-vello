//! Registered contents: resources the embedder already holds, served under
//! a URL of its choosing ahead of every other transport.
//!
//! This is what lets one fetcher replace the in-memory ones the embedders
//! carried: a CLI registers the scripts and stylesheet it decoded out of a
//! bundle under `bobcat-memory://` URLs, a browser host registers the bytes
//! its own `fetch` produced under their response URLs, and a test registers
//! a PNG. A registered URL resolves and fetches like any other, with any
//! scheme at all and bypasses the transport cache. A decoded stylesheet response
//! may be cached by the source loader; registration changes invalidate it.

use std::sync::{Arc, Mutex};

use bobcat_core::PreparsedStyleSheet;
use bytes::Bytes;
use rustc_hash::FxHashMap;
use tokio::sync::watch;
use url::{Position, Url};

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
    /// A stylesheet the host already parsed; it answers only a stylesheet
    /// source request, since it has no bytes to give any other one.
    StyleSheet(Arc<PreparsedStyleSheet>),
}

/// The registered contents, keyed by normalized URL. Read from the fetcher's
/// blocking-pool threads, which serve image and source loads from it, so it
/// locks rather than borrows.
#[derive(Debug, Default)]
pub struct Registry {
    entries: Mutex<FxHashMap<String, Registered>>,
    /// The containers still being registered off the thread that asked
    /// ([`crate::Resources::register_in_background`]), each with the URLs
    /// it covers and what it ends with.
    installs: Mutex<Vec<Install>>,
}

/// How a background install ended: `None` while it runs, then `Ok` once
/// everything it registers is registered, or the message it failed with.
pub(crate) type InstallResult = Option<Result<(), Arc<str>>>;

/// One background install of a container.
#[derive(Debug)]
struct Install {
    id: u64,
    /// The container URL up to the end of its path, one trailing `/` of the
    /// path dropped: every URL a container registers is this path or under
    /// it, whatever its query and fragment.
    path: String,
    done: watch::Receiver<InstallResult>,
}

impl Install {
    fn covers(&self, url: &Url) -> bool {
        let path = &url[..Position::AfterPath];
        path.strip_prefix(self.path.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }
}

impl Registry {
    /// Holds every URL under `container` for a background install until it
    /// ends; `id` releases it.
    pub(crate) fn hold(&self, id: u64, container: &Url, done: watch::Receiver<InstallResult>) {
        let path = &container[..Position::AfterPath];
        let path = path.strip_suffix('/').unwrap_or(path).to_owned();
        self.lock_installs().push(Install { id, path, done });
    }

    /// Ends the hold `id` placed.
    pub(crate) fn release(&self, id: u64) {
        self.lock_installs().retain(|install| install.id != id);
    }

    /// What the install that will register `url`, if one is running, ends
    /// with.
    pub(crate) fn install_covering(&self, url: &Url) -> Option<watch::Receiver<InstallResult>> {
        self.lock_installs()
            .iter()
            .find(|install| install.covers(url))
            .map(|install| install.done.clone())
    }

    fn lock_installs(&self) -> std::sync::MutexGuard<'_, Vec<Install>> {
        self.installs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

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
