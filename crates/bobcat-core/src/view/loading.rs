//! One-shot source loading performed before a view starts.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt, str};

use dom::{FontBlob, ImageStore};
use http::HeaderMap;

use super::{EngineError, EventRequester, LynxView, Window, frame_size};
use crate::resource::{
    CachePolicy, RequestContext, RequestId, ResolveRequest, ResourceDescriptor, ResourceFetcher,
    ResourcePriority, ResourceRequest, StyleSheetPayload,
};
use crate::tree::{LynxDocument, PageConfig, Viewport, new_document};

static NEXT_REQUEST_NAMESPACE: AtomicU64 = AtomicU64::new(1);

/// Everything loaded before the entry module starts.
#[derive(Clone)]
pub struct ViewSources {
    pub fonts: Vec<FontBlob>,
    pub default_font_family: Option<String>,
    pub image_store: Option<Arc<dyn ImageStore>>,
    pub style_sheets: Vec<String>,
    pub entry: String,
}

impl ViewSources {
    #[must_use]
    pub fn new(entry: impl Into<String>) -> Self {
        Self {
            fonts: Vec::new(),
            default_font_family: None,
            image_store: None,
            style_sheets: Vec::new(),
            entry: entry.into(),
        }
    }
}

impl fmt::Debug for ViewSources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ViewSources")
            .field("style_sheets", &self.style_sheets)
            .field("entry", &self.entry)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LynxViewError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Resource(#[from] crate::resource::ResourceError),
    #[error("script `{url}` is not valid UTF-8: {message}")]
    InvalidScriptEncoding { url: String, message: String },
    #[error("stylesheet `{url}` is not valid UTF-8: {message}")]
    InvalidStyleSheetEncoding { url: String, message: String },
    #[error("the image store could not load `{image_source}`: {message}")]
    Image {
        image_source: String,
        message: String,
    },
}

#[derive(Debug)]
pub(super) struct EntryModule {
    pub(super) source: String,
    pub(super) url: String,
}

impl<W: Window> LynxView<'_, W> {
    /// Loads every source, then starts the view's single entry module.
    pub async fn new(
        config: PageConfig,
        resource_fetcher: &dyn ResourceFetcher,
        event_requester: Arc<dyn EventRequester>,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        sources: ViewSources,
    ) -> Result<Self, LynxViewError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let mut document = new_document(viewport, config);
        for font in sources.fonts {
            document.register_fonts(font);
        }
        if let Some(family) = sources.default_font_family
            && !document.set_default_font_family(&family)
        {
            return Err(EngineError::UnknownFontFamily(family).into());
        }
        if let Some(store) = sources.image_store {
            document.set_image_store(store);
        }

        let mut requests = RequestId {
            namespace: NEXT_REQUEST_NAMESPACE.fetch_add(1, Ordering::Relaxed),
            sequence: 0,
        };
        for url in &sources.style_sheets {
            mount_style_sheet(resource_fetcher, &mut requests, url, &mut document).await?;
        }
        let entry = fetch_entry(resource_fetcher, &mut requests, &sources.entry).await?;
        Self::start(document, viewport, frame_size, event_requester, entry).map_err(Into::into)
    }
}

async fn mount_style_sheet(
    fetcher: &dyn ResourceFetcher,
    requests: &mut RequestId,
    url: &str,
    document: &mut LynxDocument,
) -> Result<(), LynxViewError> {
    let (request, source_name) = resolve_for_fetch(fetcher, requests, url).await?;
    match fetcher.fetch_style_sheet(request).await?.payload {
        StyleSheetPayload::Preparsed(sheet) => {
            crate::style::add_preparsed_style_sheet(document, &sheet);
        }
        StyleSheetPayload::Text(bytes) => {
            let css = str::from_utf8(&bytes).map_err(|error| {
                LynxViewError::InvalidStyleSheetEncoding {
                    url: source_name,
                    message: error.to_string(),
                }
            })?;
            crate::style::add_style_sheet_text(document, css);
        }
    }
    Ok(())
}

async fn fetch_entry(
    fetcher: &dyn ResourceFetcher,
    requests: &mut RequestId,
    url: &str,
) -> Result<EntryModule, LynxViewError> {
    let (request, url) = resolve_for_fetch(fetcher, requests, url).await?;
    let response = fetcher.fetch_resource(request).await?;
    let source = match str::from_utf8(&response.bytes) {
        Ok(source) => source.to_owned(),
        Err(error) => {
            return Err(LynxViewError::InvalidScriptEncoding {
                url,
                message: error.to_string(),
            });
        }
    };
    Ok(EntryModule { source, url })
}

async fn resolve_for_fetch(
    fetcher: &dyn ResourceFetcher,
    requests: &mut RequestId,
    url: &str,
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
                specifier: Arc::from(url),
                base_url: None,
            },
            percent_decode: false,
        })
        .await?;
    let source_name = resolved.url.to_string();
    Ok((
        ResourceRequest {
            context,
            resource: resolved,
            headers: HeaderMap::new(),
            cache_policy: CachePolicy::Default,
        },
        source_name,
    ))
}
