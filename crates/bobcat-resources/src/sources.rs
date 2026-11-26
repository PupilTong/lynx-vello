//! Active source loading owned by the fetcher, with concrete completion values.

use std::sync::Arc;

use bobcat_core::performance::Instant;
use bobcat_core::resource::{
    FetchOutcome, LoadedSource, ResourceErrorKind, ResourceErrorPhase, SourceCompletion,
    SourceRequest, StyleSheetSource,
};
use bobcat_core::{FontBlob, LynxViewError};
use http::HeaderMap;
use rustc_hash::FxHashMap;
use tokio::sync::watch;
use url::Url;
use vant::Platform;

use crate::{CachePolicy, Registered, Resources, SharedHandle, preprocess_fetched};

type SourceResult = Result<LoadedSource, LynxViewError>;
type CachedStyle = watch::Receiver<Option<SourceResult>>;

pub(crate) type StyleCache = FxHashMap<Url, CachedStyle>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    StyleSheet,
    Script,
    Font,
    Fetch { timed: bool },
}

impl SourceKind {
    #[cfg(not(target_arch = "wasm32"))]
    fn body(self) -> crate::file_body::Body {
        match self {
            Self::Font => crate::file_body::Body::Mapped,
            Self::StyleSheet | Self::Script | Self::Fetch { .. } => crate::file_body::Body::Read,
        }
    }
}

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

pub(crate) fn preload<P: Platform>(resources: &Resources<P>, request: SourceRequest) {
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

fn cached_style<P: Platform>(resources: &Resources<P>, url: Url) -> CachedStyle {
    let mut cache = resources.style_cache.borrow_mut();
    if let Some(response) = cache.get(&url) {
        return response.clone();
    }
    let (sender, response) = watch::channel(None);
    cache.insert(url.clone(), response.clone());
    start(
        resources,
        url,
        SourceKind::StyleSheet,
        Destination::Cache(sender),
    );
    response
}

async fn answer_cached_style(mut response: CachedStyle, completion: SourceCompletion) {
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

pub(crate) fn request<P: Platform>(
    resources: &Resources<P>,
    request: SourceRequest,
    completion: SourceCompletion,
) {
    if completion.is_cancelled() {
        return;
    }
    let (specifier, kind) = match request {
        SourceRequest::StyleSheet(url) => (url, SourceKind::StyleSheet),
        SourceRequest::Module(url) => (url, SourceKind::Script),
        SourceRequest::Font { url } => (url, SourceKind::Font),
        SourceRequest::Fetch { url, timed } => (url, SourceKind::Fetch { timed }),
    };
    let url = match resources
        .shared
        .transports
        .resolve(&specifier, resources.base_url().as_ref())
    {
        Ok(url) => url,
        Err(failure) => {
            completion.complete(Err(failure.into_error(Some(specifier.into())).into()));
            return;
        }
    };
    if kind == SourceKind::StyleSheet
        && let Some(Registered::StyleSheet(sheet)) = resources.shared.transports.registry.get(&url)
    {
        completion.complete(Ok(LoadedSource::StyleSheet(StyleSheetSource::Preparsed(
            sheet,
        ))));
        return;
    }
    if kind == SourceKind::StyleSheet {
        let response = cached_style(resources, url);
        let answer = answer_cached_style(response, completion);
        #[cfg(not(target_arch = "wasm32"))]
        resources.executor.spawn(answer);
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(answer);
    } else {
        start(resources, url, kind, Destination::Request(completion));
    }
}

fn start<P: Platform>(
    resources: &Resources<P>,
    url: Url,
    kind: SourceKind,
    completion: Destination,
) {
    #[cfg(not(target_arch = "wasm32"))]
    spawn(resources, url, kind, completion);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(run(
        SharedHandle::clone(&resources.shared),
        url,
        kind,
        completion,
    ));
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn<P: Platform>(
    resources: &Resources<P>,
    url: Url,
    kind: SourceKind,
    completion: Destination,
) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    resources.executor.spawn(async move {
        if completion.is_cancelled() {
            return;
        }
        if let Some(answer) = after_install(&shared, &url, kind).await {
            completion.complete(answer);
            return;
        }
        let fetched = {
            let shared = SharedHandle::clone(&shared);
            let url = url.clone();
            crate::executor::blocking(&handle, "source load", move || {
                shared.fetch_job(&url, CachePolicy::Default, &HeaderMap::new(), kind.body())
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
        let received = arrival(kind);
        if completion.is_cancelled() {
            return;
        }
        let prepared = crate::executor::blocking(&handle, "source load", {
            let url = url.clone();
            let shared = SharedHandle::clone(&shared);
            move || prepare(fetched, &url, kind, received, &shared)
        })
        .await;
        let prepared = match prepared {
            Ok(result) => result,
            Err(message) => Err(panicked(&message, &url)),
        };
        completion.complete(preloaded(&shared, prepared).await);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn panicked(message: &str, url: &Url) -> LynxViewError {
    vant::error::Failure::new(
        ResourceErrorKind::Unavailable,
        ResourceErrorPhase::ReadBody,
        message,
    )
    .into_error(Some(Arc::from(url.as_str())))
    .into()
}

#[cfg(target_arch = "wasm32")]
async fn run<P: Platform>(
    shared: SharedHandle<P>,
    url: Url,
    kind: SourceKind,
    completion: Destination,
) {
    if completion.is_cancelled() {
        return;
    }
    if let Some(answer) = after_install(&shared, &url, kind).await {
        completion.complete(answer);
        return;
    }
    let fetched = shared
        .transports
        .fetch(&url, CachePolicy::Default, &HeaderMap::new())
        .await;
    let received = arrival(kind);
    if completion.is_cancelled() {
        return;
    }
    let prepared = prepare(fetched, &url, kind, received, &shared);
    completion.complete(preloaded(&shared, prepared).await);
}

async fn preloaded<P: Platform>(shared: &SharedHandle<P>, prepared: SourceResult) -> SourceResult {
    let text = match &prepared {
        Ok(LoadedSource::Module { source, .. }) => Some(source.as_str()),
        _ => None,
    };
    if let (Some(code), Some(text)) = (&shared.code, text) {
        code.preload_async(text).await;
    }
    prepared
}

fn arrival(kind: SourceKind) -> Option<Instant> {
    matches!(kind, SourceKind::Fetch { timed: true }).then(Instant::now)
}

async fn after_install<P: Platform>(
    shared: &SharedHandle<P>,
    url: &Url,
    kind: SourceKind,
) -> Option<SourceResult> {
    if let Some(mut done) = shared.transports.installs.install_covering(url) {
        let ended = done
            .wait_for(Option::is_some)
            .await
            .map(|result| result.clone().expect("wait_for required a result"));
        let failure = |message: &str| {
            Some(Err(vant::error::Failure::new(
                ResourceErrorKind::ResponseBody,
                ResourceErrorPhase::ReadBody,
                message,
            )
            .into_error(Some(Arc::from(url.as_str())))
            .into()))
        };
        match ended {
            Ok(Ok(())) => {}
            Ok(Err(message)) => return failure(&message),
            Err(_) => return failure("the container install ended without an answer"),
        }
    }
    if kind == SourceKind::StyleSheet
        && let Some(Registered::StyleSheet(sheet)) = shared.transports.registry.get(url)
    {
        return Some(Ok(LoadedSource::StyleSheet(StyleSheetSource::Preparsed(
            sheet,
        ))));
    }
    None
}

fn prepare<P: Platform>(
    fetched: Result<crate::Fetched, vant::error::Failure>,
    url: &Url,
    kind: SourceKind,
    received: Option<Instant>,
    shared: &SharedHandle<P>,
) -> Result<LoadedSource, LynxViewError> {
    if let SourceKind::Fetch { timed } = kind {
        let bytes = fetched
            .map_err(|failure| {
                LynxViewError::from(failure.into_error(Some(Arc::from(url.as_str()))))
            })?
            .bytes;
        let install_start = timed.then(Instant::now);
        return shared
            .install_container(url, &bytes)
            .map(|installed| {
                shared.fetches.remember(url);
                LoadedSource::Fetched(if timed {
                    FetchOutcome {
                        size_bytes: u64::try_from(bytes.len()).ok(),
                        received,
                        installed: install_start
                            .filter(|_| installed)
                            .map(|start| start..Instant::now()),
                    }
                } else {
                    FetchOutcome::default()
                })
            })
            .map_err(|message| {
                vant::error::Failure::new(
                    ResourceErrorKind::ResponseBody,
                    ResourceErrorPhase::ReadBody,
                    message,
                )
                .into_error(Some(Arc::from(url.as_str())))
                .into()
            });
    }
    fetched
        .and_then(|fetched| preprocess_fetched(fetched, url))
        .map_err(|failure| LynxViewError::from(failure.into_error(Some(Arc::from(url.as_str())))))
        .and_then(|(fetched, processed)| {
            let url = fetched.url.to_string();
            if kind == SourceKind::Font {
                return Ok(LoadedSource::Font(FontBlob::new(processed.bytes)));
            }
            let style_sheet = kind == SourceKind::StyleSheet;
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
            if style_sheet {
                return Ok(LoadedSource::StyleSheet(StyleSheetSource::Text(source)));
            }
            if let Some(code) = &shared.code {
                code.preload(&source);
            }
            Ok(LoadedSource::Module { source, url })
        })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "sources_tests.rs"]
mod tests;
