//! Active source loading owned by the fetcher, with concrete completion values.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bobcat_core::LynxViewError;
use bobcat_core::resource::{
    CachePolicy, LoadedSource, RequestId, ResourceErrorKind, ResourceErrorPhase, SourceCompletion,
    SourceRequest, StyleSheetSource,
};
use http::HeaderMap;
use url::Url;

use crate::{Registered, Resources, SharedHandle, error, preprocess_fetched};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

pub(crate) fn request(resources: &Resources, request: SourceRequest, completion: SourceCompletion) {
    if completion.is_cancelled() {
        return;
    }
    let (specifier, style_sheet, base_url) = match request {
        SourceRequest::StyleSheet(url) => (url, true, resources.base_url()),
        SourceRequest::Entry(url) | SourceRequest::Module(url) => {
            (url, false, resources.base_url())
        }
        SourceRequest::Worker {
            specifier,
            base_url,
        } => {
            let base = match Url::parse(&base_url) {
                Ok(base) => base,
                Err(error) => {
                    completion.complete(Err(error::Failure::new(
                        ResourceErrorKind::InvalidUrl,
                        ResourceErrorPhase::Resolve,
                        format!("invalid worker base URL: {error}"),
                    )
                    .into_error(None, Some(base_url.into()))
                    .into()));
                    return;
                }
            };
            (specifier, false, Some(base))
        }
    };
    let id = RequestId {
        namespace: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
        sequence: 0,
    };
    let url = match resources
        .shared
        .transports
        .resolve(&specifier, base_url.as_ref())
    {
        Ok(url) => url,
        Err(failure) => {
            completion.complete(Err(failure
                .into_error(Some(id), Some(specifier.into()))
                .into()));
            return;
        }
    };
    if style_sheet
        && let Some(Registered::StyleSheet(sheet)) = resources.shared.transports.registry.get(&url)
    {
        completion.complete(Ok(LoadedSource::StyleSheet(StyleSheetSource::Preparsed(
            sheet,
        ))));
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    spawn(resources, url, id, style_sheet, completion);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(run(
        SharedHandle::clone(&resources.shared),
        url,
        id,
        style_sheet,
        completion,
    ));
}

/// One source load: two blocking hops with the cancellation check before
/// each, and the completion answered from the task itself.
///
/// The [`SourceCompletion`] never enters a blocking closure, so a panic in
/// one cannot lose it: it becomes the protocol failure below and the
/// completion is answered with that. A completion dropped without an answer
/// — which is what the executor's shutdown does to this task — reports
/// `bobcat-core`'s unanswered-source failure instead, unless the view is
/// already cancelled.
///
/// Both cancellation checks are structural, and no test in this crate reaches
/// either: a cancelled [`SourceCompletion`] cannot be built here at all,
/// because `bobcat-core`'s two constructors for one are `pub(crate)`. What a
/// cancelled completion does is `bobcat-core`'s to pin, and its startup tests
/// do pin it — a completion a fetcher still holds when the view is released
/// reads as cancelled.
#[cfg(not(target_arch = "wasm32"))]
fn spawn(
    resources: &Resources,
    url: Url,
    id: RequestId,
    style_sheet: bool,
    completion: SourceCompletion,
) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    resources.executor.spawn(async move {
        if completion.is_cancelled() {
            return;
        }
        let fetched = {
            let shared = SharedHandle::clone(&shared);
            let url = url.clone();
            crate::executor::blocking(&handle, "source load", move || {
                shared.fetch_job(&url, CachePolicy::Default, &HeaderMap::new())
            })
            .await
        };
        let fetched = match fetched {
            Ok(fetched) => fetched,
            Err(message) => {
                completion.complete(Err(panicked(&message, id, &url)));
                return;
            }
        };
        if completion.is_cancelled() {
            return;
        }
        let prepared = crate::executor::blocking(&handle, "source load", {
            let url = url.clone();
            move || prepare(fetched, &url, id, style_sheet)
        })
        .await;
        completion.complete(match prepared {
            Ok(result) => result,
            Err(message) => Err(panicked(&message, id, &url)),
        });
    });
}

/// A source load answered with what a blocking closure's panic left.
#[cfg(not(target_arch = "wasm32"))]
fn panicked(message: &str, id: RequestId, url: &Url) -> LynxViewError {
    error::Failure::new(
        ResourceErrorKind::Unavailable,
        ResourceErrorPhase::ReadBody,
        message,
    )
    .into_error(Some(id), Some(Arc::from(url.as_str())))
    .into()
}

#[cfg(target_arch = "wasm32")]
async fn run(
    shared: SharedHandle,
    url: Url,
    id: RequestId,
    style_sheet: bool,
    completion: SourceCompletion,
) {
    if completion.is_cancelled() {
        return;
    }
    let fetched = shared
        .transports
        .fetch(&url, CachePolicy::Default, &HeaderMap::new())
        .await;
    if completion.is_cancelled() {
        return;
    }
    completion.complete(prepare(fetched, &url, id, style_sheet));
}

/// The CPU half of a source load: preprocessing and the UTF-8 check the
/// engine's strict validation depends on.
fn prepare(
    fetched: Result<crate::Fetched, error::Failure>,
    url: &Url,
    id: RequestId,
    style_sheet: bool,
) -> Result<LoadedSource, LynxViewError> {
    fetched
        .and_then(|fetched| preprocess_fetched(fetched, url))
        .map_err(|failure| {
            LynxViewError::from(failure.into_error(Some(id), Some(Arc::from(url.as_str()))))
        })
        .and_then(|(fetched, processed)| {
            let url = fetched.url.to_string();
            let source = std::str::from_utf8(&processed.bytes)
                .map_err(|error| {
                    if style_sheet {
                        LynxViewError::InvalidStyleSheetEncoding {
                            url: url.clone(),
                            message: error.to_string(),
                        }
                    } else {
                        LynxViewError::InvalidScriptEncoding {
                            url: url.clone(),
                            message: error.to_string(),
                        }
                    }
                })?
                .to_owned();
            Ok(if style_sheet {
                LoadedSource::StyleSheet(StyleSheetSource::Text(source))
            } else {
                LoadedSource::Entry { source, url }
            })
        })
}
