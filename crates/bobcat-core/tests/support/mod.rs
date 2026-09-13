//! Shared test scaffolding: a scriptable `ResourceFetcher` double.

#![allow(dead_code)]

use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    RetryAdvice, SourceCompletion, SourceRequest, StyleSheetSource,
};
use bobcat_core::{
    DrawTarget, EngineEvent, EventRequester, ImageReports, LynxGroup, LynxView, LynxViewError,
    Painter, PreparsedStyleSheet, StyleThreads, ViewSources,
};
use url::Url;

/// One view in a group of its own, with a painter attached to it, at Stylo's
/// own thread count.
///
/// Almost every test here is about a page rather than about sharing, so it
/// wants exactly one view and never names the group again. The group handle
/// is dropped as this returns: the view holds its thread alive by itself, and
/// dropping the view is what ends it.
///
/// # Errors
///
/// Whatever building the group, the view or the painter failed with.
pub async fn solo_view<R, F, B>(
    event_requester: Arc<R>,
    width: f32,
    height: f32,
    device_pixel_ratio: f32,
    target: DrawTarget,
    resources: B,
    sources: ViewSources,
) -> Result<(LynxView<F>, Painter), LynxViewError>
where
    R: EventRequester,
    F: ResourceFetcher + 'static,
    B: FnOnce(ImageReports) -> F,
{
    let view = LynxGroup::new(event_requester, StyleThreads::Auto)
        .await?
        .create_lynx_view(width, height, device_pixel_ratio, resources, sources)?;
    let mut painter = Painter::new(target, width, height, device_pixel_ratio).await?;
    painter.attach(&view)?;
    Ok((view, painter))
}

/// Drives normal view turns until the terminal boot event arrives.
///
/// The view alone: booting needs the resource protocol serviced, which is
/// `LynxView::pump`'s and nobody else's.
pub fn wait_for_script<F: ResourceFetcher + 'static>(
    view: &mut LynxView<F>,
) -> Result<(), LynxViewError> {
    // Generous, like the engine's own BEGIN_FRAME_TIMEOUT: a debug-build
    // boot takes about two seconds on its own, so a tight deadline only
    // ever fires spuriously under parallel test load.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => return Ok(()),
                EngineEvent::ScriptRunError(error) => return Err(error.into()),
                EngineEvent::StartupFailed(error) => return Err(error),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "script thread did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// What the double resolves to, and the one payload it hands back.
#[derive(Debug)]
pub struct FetcherDouble {
    pub bytes: Vec<u8>,
    /// Overrides the resolved URL, so a test can drive the `data:` branch or a host rewrite
    /// without a real network.
    pub resolve_to: Mutex<Option<String>>,
    pub resolves: AtomicUsize,
    pub fetches: AtomicUsize,
    /// When set, stylesheet requests are answered pre-parsed instead of as
    /// CSS text — the arm a bundle-decoding embedder uses.
    pub style_sheet: Option<Arc<PreparsedStyleSheet>>,
    /// When set, stylesheet requests use these bytes while every other source
    /// keeps using `bytes` for the entry module.
    pub style_sheet_text: Option<Vec<u8>>,
    pub style_sheet_fetches: AtomicUsize,
    /// The images this host serves, if a test installed any. Shared with the
    /// test through an `Arc` so it can publish pixels and read the retain log
    /// while the painter owns its own handle.
    pub images: Option<Rc<flashbulb::TestImages>>,
    /// Every call the view made on this host's image half, counted whether or
    /// not a store was installed — which is what a test asserting that a view
    /// stopped asking reads.
    pub image_requests: AtomicUsize,
    pub image_services: AtomicUsize,
}

impl FetcherDouble {
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            resolve_to: Mutex::new(None),
            resolves: AtomicUsize::new(0),
            fetches: AtomicUsize::new(0),
            style_sheet: None,
            style_sheet_text: None,
            style_sheet_fetches: AtomicUsize::new(0),
            images: None,
            image_requests: AtomicUsize::new(0),
            image_services: AtomicUsize::new(0),
        }
    }

    pub fn image_request_count(&self) -> usize {
        self.image_requests.load(Ordering::Relaxed)
    }

    pub fn image_service_count(&self) -> usize {
        self.image_services.load(Ordering::Relaxed)
    }

    /// Serves images from `images`, which the test keeps a handle on so it
    /// can publish pixels and read the retain log.
    #[must_use]
    pub fn with_images(mut self, images: Rc<flashbulb::TestImages>) -> Self {
        self.images = Some(images);
        self
    }

    /// Points this double's store at the view's sink, so completed loads
    /// reach the document the way a real host's per-view value would.
    #[must_use]
    pub fn serving(self, sink: bobcat_core::ImageReports) -> Self {
        if let Some(images) = self.images.as_ref() {
            images.attach(sink);
        }
        self
    }

    /// Answers stylesheet requests with a host-decoded sheet.
    #[must_use]
    pub fn with_preparsed_style_sheet(mut self, sheet: PreparsedStyleSheet) -> Self {
        self.style_sheet = Some(Arc::new(sheet));
        self
    }

    /// Answers stylesheet requests with raw text bytes independently of the
    /// payload every other source is served from.
    #[must_use]
    pub fn with_style_sheet_text(mut self, bytes: Vec<u8>) -> Self {
        self.style_sheet_text = Some(bytes);
        self
    }

    pub fn style_sheet_fetch_count(&self) -> usize {
        self.style_sheet_fetches.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn resolving_to(self, url: &str) -> Self {
        *self.resolve_to.lock().expect("resolve override") = Some(url.to_owned());
        self
    }

    pub fn fetch_count(&self) -> usize {
        self.fetches.load(Ordering::Relaxed)
    }

    pub fn resolve_count(&self) -> usize {
        self.resolves.load(Ordering::Relaxed)
    }

    /// Where `specifier` resolves, counting the resolve a real host would do.
    ///
    /// `resolve_to` overrides the answer, so a test can point every source at
    /// one URL or at something that is not a URL at all.
    fn resolve(&self, specifier: &str, base_url: Option<&Url>) -> Result<Url, ResourceError> {
        self.resolves.fetch_add(1, Ordering::Relaxed);
        let text = self
            .resolve_to
            .lock()
            .expect("resolve override")
            .clone()
            .unwrap_or_else(|| {
                base_url.map_or_else(
                    || format!("https://example.test/{specifier}"),
                    |base| base.join(specifier).expect("test source URL").to_string(),
                )
            });
        Url::parse(&text).map_err(|error| ResourceError {
            kind: ResourceErrorKind::InvalidUrl,
            phase: ResourceErrorPhase::Resolve,
            locator: Some(Arc::from(specifier)),
            message: error.to_string().into(),
            retry: RetryAdvice::Never,
        })
    }

    /// The one payload this double serves every source from, counted as a
    /// fetch — which is what a test asserting that a pre-parsed sheet cost no
    /// payload reads.
    fn payload(&self) -> Vec<u8> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        self.bytes.clone()
    }

    /// Resolves and serves one source out of memory, inline.
    pub fn load_source(&self, request: SourceRequest) -> Result<LoadedSource, LynxViewError> {
        let (specifier, style_sheet, base_url) = match request {
            SourceRequest::StyleSheet(url) => (url, true, None),
            SourceRequest::Entry(url) | SourceRequest::Module(url) => (url, false, None),
            SourceRequest::Worker {
                specifier,
                base_url,
            } => (
                specifier,
                false,
                Some(Url::parse(&base_url).expect("entry URL")),
            ),
        };
        let url = self.resolve(&specifier, base_url.as_ref())?.to_string();
        let bytes = if style_sheet {
            self.style_sheet_fetches.fetch_add(1, Ordering::Relaxed);
            if let Some(sheet) = self.style_sheet.clone() {
                return Ok(LoadedSource::StyleSheet(StyleSheetSource::Preparsed(sheet)));
            }
            self.style_sheet_text
                .clone()
                .unwrap_or_else(|| self.payload())
        } else {
            self.payload()
        };
        let source = std::str::from_utf8(&bytes)
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
    }
}

impl ResourceFetcher for FetcherDouble {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        completion.complete(self.load_source(request));
    }

    fn request_image(&self, source: &str) {
        self.image_requests.fetch_add(1, Ordering::Relaxed);
        if let Some(images) = self.images.as_ref() {
            images.request(source);
        }
    }

    fn service_images(&self) {
        self.image_services.fetch_add(1, Ordering::Relaxed);
    }
}

/// A double serves images only when a test gave it a store; otherwise every
/// image draw resolves to nothing, which is what an unloaded image looks like.
impl bobcat_core::FrameImages for FetcherDouble {
    fn read(
        &self,
        source: &str,
        hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::vello::peniko::ImageData> {
        self.images
            .as_ref()
            .and_then(|images| images.read(source, hint))
    }

    fn retain(&self, frame: &[Arc<str>]) {
        if let Some(images) = self.images.as_ref() {
            images.retain(frame);
        }
    }
}
