//! Loading the view's startup sources, on the thread that owns the fetcher.
//!
//! The painter holds the specifiers and pushes each loaded source across
//! unasked. That split is what lets the Lynx main thread be purely
//! message-driven: it owns no fetcher, never holds a specifier, and never
//! awaits anything.
//!
//! Resolution, transport and UTF-8 decoding all happen here, so an encoding
//! error can name the URL the host actually resolved rather than the
//! specifier the page wrote.

use std::str;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use http::HeaderMap;

use crate::resource::{
    CachePolicy, RequestContext, RequestId, ResolveRequest, ResourceDescriptor, ResourceFetcher,
    ResourcePriority, ResourceRequest, StyleSheetPayload,
};
use crate::view::{LoadedSource, LynxViewError, StyleSheetSource};

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
    requests: &mut RequestId,
    specifier: &str,
) -> Result<LoadedSource, LynxViewError> {
    let (request, url) = resolve(fetcher, requests, specifier).await?;
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
    requests: &mut RequestId,
    specifier: &str,
) -> Result<LoadedSource, LynxViewError> {
    let (request, url) = resolve(fetcher, requests, specifier).await?;
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
    requests: &mut RequestId,
    specifier: &str,
) -> Result<(ResourceRequest, String), LynxViewError> {
    let context = RequestContext {
        id: *requests,
        priority: ResourcePriority::Critical,
    };
    requests.sequence += 1;
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
