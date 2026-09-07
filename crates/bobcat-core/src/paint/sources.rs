//! Buffered resource requests from main, serviced during ordinary painter turns.
//! Resolution and UTF-8 validation stay on the fetcher's owning thread.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::str;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Waker};

use http::HeaderMap;

use crate::resource::{
    CachePolicy, RequestContext, RequestId, ResolveRequest, ResourceDescriptor, ResourceFetcher,
    ResourcePriority, ResourceRequest, StyleSheetPayload,
};
use crate::view::{LoadedSource, LynxViewError, SourceRequest, StyleSheetSource};

/// Namespaces request ids per view, so two views' requests never collide in a
/// host that keys its own bookkeeping on them.
static NEXT_REQUEST_NAMESPACE: AtomicU64 = AtomicU64::new(1);

/// A fresh request-id namespace for one view's startup.
pub(super) fn mint_namespace() -> RequestId {
    RequestId {
        namespace: NEXT_REQUEST_NAMESPACE.fetch_add(1, Ordering::Relaxed),
        sequence: 0,
    }
}

/// Loads one stylesheet: resolve, fetch, decode. A sheet may come back
/// pre-parsed from a host that decoded a bundle.
pub(super) async fn load_style_sheet<F: ResourceFetcher>(
    fetcher: &F,
    request_id: RequestId,
    specifier: &str,
) -> Result<LoadedSource, LynxViewError> {
    let (request, url) = resolve(fetcher, request_id, specifier).await?;
    let sheet = match fetcher.fetch_style_sheet(request).await?.payload {
        StyleSheetPayload::Preparsed(sheet) => StyleSheetSource::Preparsed(sheet),
        StyleSheetPayload::Text(bytes) => StyleSheetSource::Text(
            str::from_utf8(&bytes)
                .map_err(|error| LynxViewError::InvalidStyleSheetEncoding {
                    url,
                    message: error.to_string(),
                })?
                .to_owned(),
        ),
    };
    Ok(LoadedSource::StyleSheet(sheet))
}

/// Loads the entry module: resolve, fetch, decode. Always bytes.
pub(super) async fn load_entry<F: ResourceFetcher>(
    fetcher: &F,
    request_id: RequestId,
    specifier: &str,
) -> Result<LoadedSource, LynxViewError> {
    let (request, url) = resolve(fetcher, request_id, specifier).await?;
    let response = fetcher.fetch_resource(request).await?;
    let source = str::from_utf8(&response.bytes)
        .map_err(|error| LynxViewError::InvalidScriptEncoding {
            url: url.clone(),
            message: error.to_string(),
        })?
        .to_owned();
    Ok(LoadedSource::Entry { source, url })
}

/// Resolves a specifier and returns the fetch request plus the resolved URL —
/// which is the name every later error reports against.
async fn resolve<F: ResourceFetcher>(
    fetcher: &F,
    request_id: RequestId,
    specifier: &str,
) -> Result<(ResourceRequest, String), LynxViewError> {
    let context = RequestContext {
        id: request_id,
        priority: ResourcePriority::Critical,
    };
    let resolved = fetcher
        .resolve_locator(ResolveRequest {
            context: context.clone(),
            resource: ResourceDescriptor {
                specifier: Arc::from(specifier),
                base_url: None,
            },
            percent_decode: false,
        })
        .await?;
    let url = resolved.url.to_string();
    Ok((
        ResourceRequest {
            context,
            resource: resolved,
            headers: HeaderMap::new(),
            cache_policy: CachePolicy::Default,
        },
        url,
    ))
}

type PendingSource = Pin<Box<dyn Future<Output = Result<LoadedSource, LynxViewError>>>>;

/// At most one boot source is outstanding: main requests the next stylesheet
/// after mounting the preceding one, then requests the entry.
pub(super) struct SourceLoads {
    requests: RequestId,
    pending: Option<PendingSource>,
}

impl SourceLoads {
    pub(super) fn new() -> Self {
        Self {
            requests: mint_namespace(),
            pending: None,
        }
    }

    pub(super) fn request<F: ResourceFetcher + 'static>(
        &mut self,
        fetcher: Rc<F>,
        request: SourceRequest,
    ) {
        assert!(self.pending.is_none(), "main requests one source at a time");
        let request_id = self.requests;
        self.requests.sequence += 1;
        self.pending = Some(Box::pin(async move {
            match request {
                SourceRequest::StyleSheet(specifier) => {
                    load_style_sheet(&*fetcher, request_id, &specifier).await
                }
                SourceRequest::Entry(specifier) => {
                    load_entry(&*fetcher, request_id, &specifier).await
                }
            }
        }));
    }

    pub(super) fn poll(&mut self, waker: &Waker) -> Option<Result<LoadedSource, LynxViewError>> {
        let pending = self.pending.as_mut()?;
        match pending.as_mut().poll(&mut Context::from_waker(waker)) {
            Poll::Pending => None,
            Poll::Ready(result) => {
                self.pending = None;
                Some(result)
            }
        }
    }

    #[cfg(test)]
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn cancel(&mut self) {
        self.pending = None;
    }
}
