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

pub(crate) type InstallResult = Option<Result<(), std::sync::Arc<str>>>;

#[derive(Debug, Default)]
pub(crate) struct Installs {
    installs: Mutex<Vec<Install>>,
}

#[derive(Debug)]
struct Install {
    id: u64,
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
    pub(crate) fn hold(&self, id: u64, container: &Url, done: watch::Receiver<InstallResult>) {
        let path = &container[..Position::AfterPath];
        let path = path.strip_suffix('/').unwrap_or(path).to_owned();
        self.lock().push(Install { id, path, done });
    }

    pub(crate) fn release(&self, id: u64) {
        self.lock().retain(|install| install.id != id);
    }

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
