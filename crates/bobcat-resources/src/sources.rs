//! Active source loading owned by the fetcher, with concrete completion values.

use std::sync::Arc;

use bobcat_core::LynxViewError;
use bobcat_core::resource::{
    LoadedSource, ResourceErrorKind, ResourceErrorPhase, SourceCompletion, SourceRequest,
    StyleSheetSource,
};
use http::HeaderMap;
use rustc_hash::FxHashMap;
use tokio::sync::watch;
use url::Url;

use crate::{CachePolicy, Registered, Resources, SharedHandle, error, preprocess_fetched};

type SourceResult = Result<LoadedSource, LynxViewError>;
type CachedStyle = watch::Receiver<Option<SourceResult>>;

/// Owned by one Resources scope on the resource host's thread. A watch holds
/// the pending or completed response, shared by preloads and ordinary reads.
pub(crate) type StyleCache = FxHashMap<Url, CachedStyle>;

enum Destination {
    Request(SourceCompletion),
    Cache(watch::Sender<Option<SourceResult>>),
}

impl Destination {
    fn is_cancelled(&self) -> bool {
        match self {
            Self::Request(completion) => completion.is_cancelled(),
            Self::Cache(sender) => sender.is_closed(),
        }
    }

    fn complete(self, result: SourceResult) {
        match self {
            Self::Request(completion) => completion.complete(result),
            Self::Cache(sender) => {
                let _ = sender.send(Some(result));
            }
        }
    }
}

/// Preloading is a best-effort hint. Only stylesheet caching is needed here;
/// other source kinds continue to use their existing module/worker loaders.
pub(crate) fn preload(resources: &Resources, request: SourceRequest) {
    if let SourceRequest::StyleSheet(specifier) = request
        && let Ok(url) = resources
            .shared
            .transports
            .resolve(&specifier, resources.base_url().as_ref())
        && !matches!(
            resources.shared.transports.registry.get(&url),
            Some(Registered::StyleSheet(_))
        )
    {
        let _ = cached_style(resources, url);
    }
}

fn cached_style(resources: &Resources, url: Url) -> CachedStyle {
    let mut cache = resources.style_cache.borrow_mut();
    if let Some(response) = cache.get(&url) {
        return response.clone();
    }
    let (sender, response) = watch::channel(None);
    cache.insert(url.clone(), response.clone());
    start(resources, url, true, Destination::Cache(sender));
    response
}

async fn answer_cached_style(mut response: CachedStyle, completion: SourceCompletion) {
    // A closed pending entry means the loader task ended without an answer;
    // dropping the completion reports the existing resource protocol failure.
    let result = response.wait_for(Option::is_some).await.map(|value| {
        value
            .as_ref()
            .expect("wait_for required a completed result")
            .clone()
    });
    if let Ok(result) = result {
        completion.complete(result);
    }
}

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
                    .into_error(Some(base_url.into()))
                    .into()));
                    return;
                }
            };
            (specifier, false, Some(base))
        }
    };
    let url = match resources
        .shared
        .transports
        .resolve(&specifier, base_url.as_ref())
    {
        Ok(url) => url,
        Err(failure) => {
            completion.complete(Err(failure.into_error(Some(specifier.into())).into()));
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
    if style_sheet {
        let response = cached_style(resources, url);
        let answer = answer_cached_style(response, completion);
        #[cfg(not(target_arch = "wasm32"))]
        resources.executor.spawn(answer);
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(answer);
    } else {
        start(resources, url, false, Destination::Request(completion));
    }
}

fn start(resources: &Resources, url: Url, style_sheet: bool, completion: Destination) {
    #[cfg(not(target_arch = "wasm32"))]
    spawn(resources, url, style_sheet, completion);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(run(
        SharedHandle::clone(&resources.shared),
        url,
        style_sheet,
        completion,
    ));
}

/// One source load: two blocking hops with the cancellation check before
/// each, and the completion answered from the task itself.
///
/// The destination never enters a blocking closure: a panic becomes a failure
/// response, including for cache readers. Executor shutdown drops the
/// destination, so unanswered requests fail through the existing protocol.
/// A cache load stops when no scope or reader holds its response; an ordinary
/// request stops when its completion reports cancellation.
#[cfg(not(target_arch = "wasm32"))]
fn spawn(resources: &Resources, url: Url, style_sheet: bool, completion: Destination) {
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
                completion.complete(Err(panicked(&message, &url)));
                return;
            }
        };
        if completion.is_cancelled() {
            return;
        }
        let prepared = crate::executor::blocking(&handle, "source load", {
            let url = url.clone();
            move || prepare(fetched, &url, style_sheet)
        })
        .await;
        completion.complete(match prepared {
            Ok(result) => result,
            Err(message) => Err(panicked(&message, &url)),
        });
    });
}

/// A source load answered with what a blocking closure's panic left.
#[cfg(not(target_arch = "wasm32"))]
fn panicked(message: &str, url: &Url) -> LynxViewError {
    error::Failure::new(
        ResourceErrorKind::Unavailable,
        ResourceErrorPhase::ReadBody,
        message,
    )
    .into_error(Some(Arc::from(url.as_str())))
    .into()
}

#[cfg(target_arch = "wasm32")]
async fn run(shared: SharedHandle, url: Url, style_sheet: bool, completion: Destination) {
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
    completion.complete(prepare(fetched, &url, style_sheet));
}

/// The CPU half of a source load: preprocessing and the UTF-8 check the
/// engine's strict validation depends on.
fn prepare(
    fetched: Result<crate::Fetched, error::Failure>,
    url: &Url,
    style_sheet: bool,
) -> Result<LoadedSource, LynxViewError> {
    fetched
        .and_then(|fetched| preprocess_fetched(fetched, url))
        .map_err(|failure| LynxViewError::from(failure.into_error(Some(Arc::from(url.as_str())))))
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

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "sources_tests.rs"]
mod tests;
