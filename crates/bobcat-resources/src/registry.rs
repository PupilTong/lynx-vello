//! The containers still being registered off the thread that asked
//! ([`crate::Resources::register_in_background`]): what holds a source
//! request for one of their URLs until the install ends.
//!
//! The registered contents themselves are [`vant::registry::Registry`]; the
//! holds stay here because they wait on a `tokio` channel, which `vant` does
//! not depend on.

use std::sync::Mutex;

use tokio::sync::watch;
use url::{Position, Url};

/// How a background install ended: `None` while it runs, then `Ok` once
/// everything it registers is registered, or the message it failed with.
pub(crate) type InstallResult = Option<Result<(), std::sync::Arc<str>>>;

/// The background installs of one scope, each with the URLs it covers and
/// what it ends with.
#[derive(Debug, Default)]
pub(crate) struct Installs {
    installs: Mutex<Vec<Install>>,
}

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

impl Installs {
    /// Holds every URL under `container` for a background install until it
    /// ends; `id` releases it.
    pub(crate) fn hold(&self, id: u64, container: &Url, done: watch::Receiver<InstallResult>) {
        let path = &container[..Position::AfterPath];
        let path = path.strip_suffix('/').unwrap_or(path).to_owned();
        self.lock().push(Install { id, path, done });
    }

    /// Ends the hold `id` placed.
    pub(crate) fn release(&self, id: u64) {
        self.lock().retain(|install| install.id != id);
    }

    /// What the install that will register `url`, if one is running, ends
    /// with.
    pub(crate) fn install_covering(&self, url: &Url) -> Option<watch::Receiver<InstallResult>> {
        self.lock()
            .iter()
            .find(|install| install.covers(url))
            .map(|install| install.done.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Install>> {
        self.installs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
