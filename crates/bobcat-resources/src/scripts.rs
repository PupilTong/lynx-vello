//! Loading a `Worker`'s script: the push half of the resource protocol.
//!
//! The mirror of [`crate::images`], and deliberately so. Both answer the same
//! question — the engine named something at an arbitrary moment and wants it
//! later — so both take the same shape: ask without blocking, load on this
//! crate's own concurrency, ring the view's wakeup, and hand the answer over
//! in the painter's next turn.
//!
//! What differs is fan-out. An image URL is shared, so a completion goes to
//! every view that asked; a script request belongs to exactly one `Worker` in
//! exactly one view, so the completions channel is the view's own.

use bobcat_core::resource::{ScriptReports, ScriptRequest, ScriptRequestId};
use bytes::Bytes;

use crate::{Resources, preprocess_fetched};

/// One finished script load, on its way back to the painter's thread.
pub(crate) struct Completion {
    id: ScriptRequestId,
    /// The resolved URL and the decoded bytes, or why there are none.
    result: Result<(String, Bytes), String>,
}

/// Starts one script load. Non-blocking: resolution happens here, because it
/// is synchronous and its failure needs no thread, and the transport happens
/// wherever this crate's transports run.
pub(crate) fn request(
    resources: &Resources,
    completions: &flume::Sender<Completion>,
    request: &ScriptRequest,
) {
    let id = request.id;
    let base = resources.base_url();
    let url = match resources
        .shared
        .transports
        .resolve(&request.specifier, base.as_ref())
    {
        Ok(url) => url,
        Err(failure) => {
            // Nothing was started, so nothing will come back on its own.
            let _ = completions.send(Completion {
                id,
                result: Err(failure.to_string()),
            });
            return;
        }
    };
    spawn(resources, completions.clone(), id, url);
}

/// The painter's moment: everything the transports answered since the last
/// turn, handed to the engine.
pub(crate) fn service(completions: &flume::Receiver<Completion>, reports: &ScriptReports) {
    for completion in completions.try_iter() {
        match completion.result {
            Ok((url, bytes)) => reports.loaded(completion.id, &url, bytes),
            Err(message) => reports.failed(completion.id, &message),
        }
    }
}

/// Fetches and preprocesses one script off the painter's thread, then wakes
/// the view for the turn that will drain it.
#[cfg(not(target_arch = "wasm32"))]
fn spawn(
    resources: &Resources,
    completions: flume::Sender<Completion>,
    id: ScriptRequestId,
    url: url::Url,
) {
    let shared = crate::SharedHandle::clone(&resources.shared);
    resources.shared.executor.run(move || {
        let result = shared
            .transports
            .fetch_blocking(
                &url,
                bobcat_core::resource::CachePolicy::Default,
                &http::HeaderMap::new(),
            )
            .and_then(|fetched| preprocess_fetched(fetched, &url))
            .map(|(_, preprocessed)| (url.to_string(), preprocessed.bytes))
            .map_err(|failure| failure.to_string());
        let _ = completions.send(Completion { id, result });
        // Between turns is the normal case, and the painter has no event loop
        // of its own: without this the answer sits unread.
        shared.wake();
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn(
    resources: &Resources,
    completions: flume::Sender<Completion>,
    id: ScriptRequestId,
    url: url::Url,
) {
    let shared = crate::SharedHandle::clone(&resources.shared);
    wasm_bindgen_futures::spawn_local(async move {
        let result = match shared
            .transports
            .fetch(
                &url,
                bobcat_core::resource::CachePolicy::Default,
                &http::HeaderMap::new(),
            )
            .await
        {
            Ok(fetched) => preprocess_fetched(fetched, &url)
                .map(|(_, preprocessed)| (url.to_string(), preprocessed.bytes))
                .map_err(|failure| failure.to_string()),
            Err(failure) => Err(failure.to_string()),
        };
        let _ = completions.send(Completion { id, result });
        shared.wake();
    });
}
