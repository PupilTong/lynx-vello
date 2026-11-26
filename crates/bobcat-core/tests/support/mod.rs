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
    F: ResourceFetcher<PlatformFrame = std::convert::Infallible> + 'static,
    B: FnOnce(ImageReports) -> F,
{
    let view = LynxGroup::new(event_requester, StyleThreads::Auto)
        .await?
        .create_lynx_view(
            width,
            height,
            device_pixel_ratio,
            resources,
            Vec::new(),
            sources,
        )?;
    let mut painter = Painter::new(target, width, height, device_pixel_ratio).await?;
    painter.attach(&view)?;
    Ok((view, painter))
}

pub fn wait_for_script<F: ResourceFetcher + 'static>(
    view: &mut LynxView<F>,
) -> Result<Vec<EngineEvent>, LynxViewError> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut events = view.pump().into_iter();
        while let Some(event) = events.next() {
            match event {
                EngineEvent::ScriptFinished => return Ok(events.collect()),
                EngineEvent::ScriptRunError(error) | EngineEvent::Panicked(error) => {
                    return Err(error.into());
                }
                EngineEvent::StartupFailed(error) => return Err(error),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "script thread did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub fn tick(painter: &mut Painter, force: bool) -> Result<bool, bobcat_core::EngineError> {
    let posted = painter.post_frame()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !painter.frame_serviced(&posted) {
        assert!(
            Instant::now() < deadline,
            "the view never serviced the frame: a page that asks its host for something during \
             a frame needs its view pumped while the frame is outstanding"
        );
        std::thread::sleep(Duration::from_micros(200));
    }
    painter.draw_posted(posted, force)
}

#[derive(Debug)]
pub struct FetcherDouble {
    pub bytes: Vec<u8>,
    pub resolve_to: Mutex<Option<String>>,
    pub resolves: AtomicUsize,
    pub fetches: AtomicUsize,
    pub style_sheet: Option<Arc<PreparsedStyleSheet>>,
    pub style_sheet_text: Option<Vec<u8>>,
    pub style_sheet_fetches: AtomicUsize,
    pub images: Option<Rc<prime_radiant::TestImages>>,
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

    #[must_use]
    pub fn card(body: impl AsRef<[u8]>) -> Self {
        let mut bytes = bobcat_core::MTS_CHUNK_PREAMBLE.as_bytes().to_vec();
        bytes.extend_from_slice(body.as_ref());
        Self::new(bytes)
    }

    pub fn image_request_count(&self) -> usize {
        self.image_requests.load(Ordering::Relaxed)
    }

    pub fn image_service_count(&self) -> usize {
        self.image_services.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn with_images(mut self, images: Rc<prime_radiant::TestImages>) -> Self {
        self.images = Some(images);
        self
    }

    #[must_use]
    pub fn serving(self, sink: bobcat_core::ImageReports) -> Self {
        if let Some(images) = self.images.as_ref() {
            images.attach(sink);
        }
        self
    }

    #[must_use]
    pub fn with_preparsed_style_sheet(mut self, sheet: PreparsedStyleSheet) -> Self {
        self.style_sheet = Some(Arc::new(sheet));
        self
    }

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

    fn resolve(&self, specifier: &str) -> Result<Url, ResourceError> {
        self.resolves.fetch_add(1, Ordering::Relaxed);
        let text = self
            .resolve_to
            .lock()
            .expect("resolve override")
            .clone()
            .unwrap_or_else(|| format!("https://example.test/{specifier}"));
        Url::parse(&text).map_err(|error| ResourceError {
            kind: ResourceErrorKind::InvalidUrl,
            phase: ResourceErrorPhase::Resolve,
            locator: Some(Arc::from(specifier)),
            message: error.to_string().into(),
            retry: RetryAdvice::Never,
        })
    }

    fn payload(&self) -> Vec<u8> {
        self.fetches.fetch_add(1, Ordering::Relaxed);
        self.bytes.clone()
    }

    pub fn load_source(&self, request: SourceRequest) -> Result<LoadedSource, LynxViewError> {
        let (specifier, style_sheet) = match request {
            SourceRequest::StyleSheet(url) => (url, true),
            SourceRequest::Module(url) => (url, false),
            SourceRequest::Font { url } => {
                return Err(ResourceError {
                    kind: ResourceErrorKind::NotFound,
                    phase: ResourceErrorPhase::Resolve,
                    locator: Some(Arc::from(url.as_str())),
                    message: "this double serves no fonts".into(),
                    retry: RetryAdvice::Never,
                }
                .into());
            }
            SourceRequest::Fetch { url, .. } => {
                return Err(ResourceError {
                    kind: ResourceErrorKind::NotFound,
                    phase: ResourceErrorPhase::Resolve,
                    locator: Some(Arc::from(url.as_str())),
                    message: "this double serves no plain fetch".into(),
                    retry: RetryAdvice::Never,
                }
                .into());
            }
        };
        let url = self.resolve(&specifier)?.to_string();
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
            LoadedSource::Module { source, url }
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

    fn request_document(&self, source: &str, bytes: bytes::Bytes, kind: bobcat_core::DocumentKind) {
        if let Some(images) = self.images.as_ref() {
            images.request_document(source, &bytes, kind);
        }
    }

    fn service_images(&self) {
        self.image_services.fetch_add(1, Ordering::Relaxed);
    }
}

impl bobcat_core::FrameImages for FetcherDouble {
    type PlatformFrame = std::convert::Infallible;

    fn want(&self, picture: &bobcat_core::PictureKey, hint: bobcat_core::ImageSizeHint) {
        if let Some(images) = self.images.as_ref() {
            images.want(picture, hint);
        }
    }

    fn retain(&self, frame: &[bobcat_core::PictureKey]) {
        if let Some(images) = self.images.as_ref() {
            images.retain(frame);
        }
    }

    fn describe(
        &self,
        picture: &bobcat_core::PictureKey,
        hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::FrameImage> {
        self.images
            .as_ref()
            .and_then(|images| images.describe(picture, hint))
    }

    fn release(&self, sources: &[bobcat_core::PictureKey]) {
        if let Some(images) = self.images.as_ref() {
            images.release(sources);
        }
    }
}
