//! Active source loading owned by the fetcher, with concrete completion values.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bobcat_core::LynxViewError;
use bobcat_core::resource::{
    CachePolicy, LoadedSource, RequestId, SourceCompletion, SourceRequest, StyleSheetSource,
};
use http::HeaderMap;
use url::Url;

use crate::{Registered, Resources, SharedHandle, error, preprocess_fetched};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

pub(crate) fn request(resources: &Resources, request: SourceRequest, completion: SourceCompletion) {
    if completion.is_cancelled() {
        return;
    }
    let (specifier, kind) = match request {
        SourceRequest::StyleSheet(url) => (url, SourceKind::StyleSheet),
        SourceRequest::Entry(url) => (url, SourceKind::Entry),
        SourceRequest::WorkerScript(url) => (url, SourceKind::WorkerScript),
    };
    let id = RequestId {
        namespace: NEXT_REQUEST.fetch_add(1, Ordering::Relaxed),
        sequence: 0,
    };
    let url = match resources
        .shared
        .transports
        .resolve(&specifier, resources.base_url().as_ref())
    {
        Ok(url) => url,
        Err(failure) => {
            completion.complete(Err(failure
                .into_error(Some(id), Some(specifier.into()))
                .into()));
            return;
        }
    };
    if matches!(kind, SourceKind::StyleSheet)
        && let Some(Registered::StyleSheet(sheet)) = resources.shared.transports.registry.get(&url)
    {
        completion.complete(Ok(LoadedSource::StyleSheet(StyleSheetSource::Preparsed(
            sheet,
        ))));
        return;
    }
    let job = SourceJob {
        shared: SharedHandle::clone(&resources.shared),
        url,
        id,
        kind,
        completion,
    };
    #[cfg(not(target_arch = "wasm32"))]
    resources.shared.executor.source(job);
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(job.run());
}

/// A known job variant, not an erased closure. It carries no painter-owned state.
pub(crate) struct SourceJob {
    shared: SharedHandle,
    url: Url,
    id: RequestId,
    kind: SourceKind,
    completion: SourceCompletion,
}

/// Which of the three a job is answering.
///
/// Two questions, not three: whether to consult the pre-parsed stylesheet
/// registry and which encoding error to name, and which `LoadedSource` shape
/// to build. A worker's script differs from the entry only on the second —
/// *which* worker is the engine's business, kept in the completion.
#[derive(Clone, Copy)]
enum SourceKind {
    StyleSheet,
    Entry,
    WorkerScript,
}

impl SourceJob {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn run(self) {
        if self.completion.is_cancelled() {
            return;
        }
        let fetched = self.shared.transports.fetch_blocking(
            &self.url,
            CachePolicy::Default,
            &HeaderMap::new(),
        );
        self.finish(fetched);
    }

    #[cfg(target_arch = "wasm32")]
    async fn run(self) {
        if self.completion.is_cancelled() {
            return;
        }
        let fetched = self
            .shared
            .transports
            .fetch(&self.url, CachePolicy::Default, &HeaderMap::new())
            .await;
        self.finish(fetched);
    }

    fn finish(self, fetched: Result<crate::Fetched, error::Failure>) {
        if self.completion.is_cancelled() {
            return;
        }
        let result = fetched
            .and_then(|fetched| preprocess_fetched(fetched, &self.url))
            .map_err(|failure| {
                LynxViewError::from(
                    failure.into_error(Some(self.id), Some(Arc::from(self.url.as_str()))),
                )
            })
            .and_then(|(_, processed)| {
                let url = self.url.to_string();
                let source = std::str::from_utf8(&processed.bytes)
                    .map_err(|error| match self.kind {
                        SourceKind::StyleSheet => LynxViewError::InvalidStyleSheetEncoding {
                            url: url.clone(),
                            message: error.to_string(),
                        },
                        SourceKind::Entry | SourceKind::WorkerScript => {
                            LynxViewError::InvalidScriptEncoding {
                                url: url.clone(),
                                message: error.to_string(),
                            }
                        }
                    })?
                    .to_owned();
                Ok(match self.kind {
                    SourceKind::StyleSheet => {
                        LoadedSource::StyleSheet(StyleSheetSource::Text(source))
                    }
                    SourceKind::Entry => LoadedSource::Entry { source, url },
                    SourceKind::WorkerScript => LoadedSource::WorkerScript { source, url },
                })
            });
        self.completion.complete(result);
    }
}
