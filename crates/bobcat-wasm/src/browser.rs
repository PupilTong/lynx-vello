//! Shared-memory browser composition exported through `wasm-bindgen`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::{fmt, mem};

use bobcat_core::resource::{
    BufferedResourceRequest, CacheStatus, HttpRequest, HttpResponse, PrefetchReceipt,
    PrefetchRequest, RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceFuture, ResourceKind,
    ResourceLocality, ResourceMetadata, ResourcePath, ResourceRequest, ResourceResponse,
    ResourceSource, ResourceStream, ResourceTiming, RetryAdvice,
};
use bobcat_core::{
    EngineEvent, EventRequester, FrameRequester, FrameSize, LynxView, ManualClock, PageConfig,
    Window, WindowTarget, configure_wasm_workers, quickjs_engine_factory,
};
use http::HeaderMap;
use js_sys::{Array, Promise};
use url::Url;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;
use web_sys::OffscreenCanvas;

const MAX_RENDER_DIMENSION: f64 = 16_384.0;
const MAX_STYLE_THREADS: u32 = 6;
static RENDERER_CREATED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Default)]
struct FrameSignal {
    requested: Arc<AtomicBool>,
}

impl FrameSignal {
    fn take(&self) -> bool {
        self.requested.swap(false, Ordering::AcqRel)
    }
}

impl FrameRequester for FrameSignal {
    fn request_frame(&self) {
        self.requested.store(true, Ordering::Release);
    }
}

/// Lost-wake-safe lifecycle signal shared between core-owned Workers and the
/// Render Worker. The atomic is the durable edge; the waker list only turns it
/// into an awaitable browser Promise.
#[derive(Debug, Default)]
struct EventSignal {
    pending: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

impl EventSignal {
    fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }

    fn wait(self: &Arc<Self>) -> EventWait {
        EventWait {
            signal: Arc::clone(self),
        }
    }
}

impl EventRequester for EventSignal {
    fn request_event(&self) {
        self.pending.store(true, Ordering::Release);
        let wakers = mem::take(
            &mut *self
                .wakers
                .lock()
                .unwrap_or_else(|error| panic!("the browser event signal is poisoned: {error}")),
        );
        for waker in wakers {
            waker.wake();
        }
    }
}

struct EventWait {
    signal: Arc<EventSignal>,
}

impl Future for EventWait {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.signal.pending.load(Ordering::Acquire) {
            return Poll::Ready(());
        }

        let mut wakers = self
            .signal
            .wakers
            .lock()
            .unwrap_or_else(|error| panic!("the browser event signal is poisoned: {error}"));
        // EventRequester stores the atomic before taking this mutex. This
        // second check closes the only lost-wake window between the first
        // check and registering our waker.
        if self.signal.pending.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        if !wakers.iter().any(|waker| waker.will_wake(context.waker())) {
            wakers.push(context.waker().clone());
        }
        Poll::Pending
    }
}

/// Browser marker for a view whose owned surface target lives in a Render
/// Worker. No value of this type is ever constructed.
#[derive(Debug)]
enum BrowserWindow {}

impl Window for BrowserWindow {
    type Target<'window> = WindowTarget<'window>;
    type Frames = FrameSignal;

    fn target(&self) -> Self::Target<'_> {
        match *self {}
    }

    fn frames(&self) -> Self::Frames {
        match *self {}
    }
}

/// Browser-owned resources registered by the Render Worker after it applies
/// the browser's URL, fetch, CORS, cache, and credentials policies.
type BrowserResourceRegistry = Mutex<HashMap<String, Arc<[u8]>>>;

#[derive(Debug, Default)]
struct BrowserResources {
    scripts: BrowserResourceRegistry,
    style_sheets: BrowserResourceRegistry,
}

impl BrowserResources {
    fn register_script(&self, url: &str, bytes: Vec<u8>) -> Result<String, JsValue> {
        Self::register(&self.scripts, "script", url, bytes)
    }

    fn register_style_sheet(&self, url: &str, bytes: Vec<u8>) -> Result<String, JsValue> {
        Self::register(&self.style_sheets, "stylesheet", url, bytes)
    }

    fn register(
        registry: &BrowserResourceRegistry,
        kind: &str,
        url: &str,
        bytes: Vec<u8>,
    ) -> Result<String, JsValue> {
        let url = Url::parse(url)
            .map_err(|error| js_error(format!("the {kind} URL `{url}` is invalid: {error}")))?;
        let normalized = url.to_string();
        registry
            .lock()
            .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
            .insert(normalized.clone(), Arc::from(bytes));
        Ok(normalized)
    }

    /// How a request of this kind is named in a message a host reads.
    fn label(kind: &ResourceKind) -> &'static str {
        match kind {
            ResourceKind::StyleSheet => "stylesheet",
            _ => "script",
        }
    }

    /// The media type a response of this kind is stamped with.
    fn media_type(kind: &ResourceKind) -> &'static str {
        match kind {
            ResourceKind::StyleSheet => "text/css; charset=utf-8",
            _ => "text/javascript; charset=utf-8",
        }
    }

    /// The registry a request of this kind is answered from.
    fn registry(&self, kind: &ResourceKind) -> Option<&BrowserResourceRegistry> {
        match kind {
            ResourceKind::ExternalJs => Some(&self.scripts),
            ResourceKind::StyleSheet => Some(&self.style_sheets),
            _ => None,
        }
    }

    fn error<T>(
        request_id: Option<RequestId>,
        kind: ResourceErrorKind,
        phase: ResourceErrorPhase,
        locator: Option<Arc<str>>,
        message: impl Into<Arc<str>>,
    ) -> ResourceFuture<'static, T> {
        let message = message.into();
        Box::pin(async move {
            Err(ResourceError {
                request_id,
                kind,
                phase,
                locator,
                status: None,
                message,
                retry: RetryAdvice::Never,
            })
        })
    }

    fn unsupported<T>(
        request_id: Option<RequestId>,
        phase: ResourceErrorPhase,
    ) -> ResourceFuture<'static, T> {
        Self::error(
            request_id,
            ResourceErrorKind::UnsupportedOperation,
            phase,
            None,
            "the browser source registry does not support this operation",
        )
    }
}

impl ResourceFetcher for BrowserResources {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        capability == ResourceCapability::BufferedResource
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        let request_id = request.context.id;
        let locator = request.resource.locator.specifier.clone();
        if request.context.cancellation.is_cancelled() {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::Cancelled,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "script resolution was cancelled",
            );
        }
        let kind = request.resource.kind.clone();
        let Some(registry) = self.registry(&kind) else {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::UnsupportedKind,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "the browser source registry only contains external scripts and stylesheets",
            );
        };

        let parsed = Url::parse(&locator).or_else(|_| {
            request
                .resource
                .locator
                .base_url
                .as_ref()
                .ok_or(url::ParseError::RelativeUrlWithoutBase)
                .and_then(|base| base.join(&locator))
        });
        let Ok(url) = parsed else {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::InvalidUrl,
                ResourceErrorPhase::Resolve,
                Some(locator),
                format!("the {} locator is not a valid URL", Self::label(&kind)),
            );
        };
        let present = registry
            .lock()
            .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
            .contains_key(url.as_str());
        if !present {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "the Render Worker has not registered this URL",
            );
        }

        let resource = request.resource;
        let cache_key = Some(Arc::from(url.as_str()));
        Box::pin(async move {
            Ok(ResolvedLocator {
                resource,
                url,
                rewrite_chain: Vec::new(),
                locality: ResourceLocality::Local,
                cache_key,
            })
        })
    }

    fn fetch_resource(
        &self,
        request: BufferedResourceRequest,
    ) -> ResourceFuture<'_, ResourceResponse> {
        let request_id = request.request.context.id;
        let locator: Arc<str> = Arc::from(request.request.resource.url.as_str());
        if request.request.context.cancellation.is_cancelled() {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::Cancelled,
                ResourceErrorPhase::Open,
                Some(locator),
                "script loading was cancelled",
            );
        }

        let kind = request.request.resource.resource.kind.clone();
        let media_type = Self::media_type(&kind);
        let source = self.registry(&kind).and_then(|registry| {
            registry
                .lock()
                .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
                .get(request.request.resource.url.as_str())
                .cloned()
        });
        let Some(source) = source else {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Open,
                Some(locator),
                format!(
                    "the registered {} disappeared before it was loaded",
                    Self::label(&kind)
                ),
            );
        };
        let content_length = source.len() as u64;
        if content_length > request.max_bytes {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::ResponseTooLarge,
                ResourceErrorPhase::ReadBody,
                Some(locator),
                format!(
                    "the registered {} exceeds Bobcat's buffered-resource limit",
                    Self::label(&kind)
                ),
            );
        }

        let resource = request.request.resource;
        Box::pin(async move {
            Ok(ResourceResponse {
                metadata: ResourceMetadata {
                    request_id,
                    resource,
                    headers: HeaderMap::default(),
                    content_length: Some(content_length),
                    media_type: Some(Arc::from(media_type)),
                    source: ResourceSource::MemoryCache,
                    cache_status: CacheStatus::default(),
                    timing: ResourceTiming::default(),
                },
                bytes: source.as_ref().to_vec().into(),
            })
        })
    }

    fn open_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceStream> {
        Self::unsupported(Some(request.context.id), ResourceErrorPhase::Open)
    }

    fn fetch_resource_path(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourcePath> {
        Self::unsupported(
            Some(request.context.id),
            ResourceErrorPhase::MaterializePath,
        )
    }

    fn fetch_http(&self, request: HttpRequest) -> ResourceFuture<'_, HttpResponse> {
        Self::unsupported(Some(request.context.id), ResourceErrorPhase::Connect)
    }

    fn prefetch(&self, request: PrefetchRequest) -> ResourceFuture<'_, PrefetchReceipt> {
        Self::unsupported(
            Some(request.request.context.id),
            ResourceErrorPhase::Prefetch,
        )
    }

    fn cancel_request(&self, request_id: RequestId) -> ResourceFuture<'_, ()> {
        Self::unsupported(Some(request_id), ResourceErrorPhase::Cancel)
    }
}

/// A complete browser embedder, permanently owned by the explicit Render
/// Worker that constructs it. The document, element tree, VM, and engine stay
/// behind the opaque `LynxView` facade.
#[wasm_bindgen]
pub struct BobcatRenderer {
    view: LynxView<'static, BrowserWindow, Arc<ManualClock>>,
    resources: Arc<BrowserResources>,
    canvas: OffscreenCanvas,
    frames: FrameSignal,
    events: Arc<EventSignal>,
    /// The same clock the view reads. A Worker could read a monotonic clock
    /// of its own, but `requestAnimationFrame` hands over the instant the
    /// frame is *for*, which is the better reading and the one browsers
    /// animate against.
    clock: Arc<ManualClock>,
    script_finished: bool,
    disposed: bool,
}

impl fmt::Debug for BobcatRenderer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BobcatRenderer")
            .field("view", &self.view)
            .field("disposed", &self.disposed)
            .finish_non_exhaustive()
    }
}

#[wasm_bindgen]
impl BobcatRenderer {
    /// Construct the browser embedder on its explicit Render Worker and give
    /// core the Worker bootstrap it will use for the main-thread VM and Stylo.
    #[wasm_bindgen(js_name = create)]
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        canvas: OffscreenCanvas,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        worker_url: String,
        style_thread_count: u32,
        default_display_linear: bool,
        default_overflow_visible: bool,
        enable_css_selector: bool,
    ) -> Result<BobcatRenderer, JsValue> {
        validate_metrics(width, height, device_pixel_ratio)?;
        if worker_url.is_empty() {
            return Err(js_error("the Bobcat worker URL must not be empty"));
        }
        if !(2..=MAX_STYLE_THREADS).contains(&style_thread_count) {
            return Err(js_error(format!(
                "the style thread count must be between 2 and {MAX_STYLE_THREADS}"
            )));
        }
        if RENDERER_CREATED.swap(true, Ordering::AcqRel) {
            return Err(js_error(
                "one Bobcat Wasm instance supports exactly one renderer; create each view in its own Render Worker",
            ));
        }

        let result = async move {
            configure_wasm_workers(worker_url, style_thread_count as usize).map_err(js_error)?;

            let resources = Arc::new(BrowserResources::default());
            let resource_fetcher: Arc<dyn ResourceFetcher> = resources.clone();
            let events = Arc::new(EventSignal::default());
            let config = PageConfig {
                default_display_linear,
                default_overflow_visible,
                enable_css_selector,
            };
            // The animation timeline is `requestAnimationFrame`'s timestamp,
            // written into this clock by `render_if_requested`. Naming it here
            // makes it part of the view's type; the handle below is the same
            // clock, since a shared clock is itself a clock.
            let clock = Arc::new(ManualClock::new());
            let mut view: LynxView<'static, BrowserWindow, Arc<ManualClock>> =
                LynxView::with_animation_clock(
                    config,
                    resource_fetcher,
                    quickjs_engine_factory(),
                    events.clone(),
                    width,
                    height,
                    device_pixel_ratio,
                    clock.clone(),
                )
                .map_err(js_error)?;
            set_canvas_size(&canvas, view.frame_size());

            let frames = FrameSignal::default();
            let target: WindowTarget<'static> = WindowTarget::OffscreenCanvas(canvas.clone());
            view.attach_target(target, frames.clone(), view.frame_size())
                .await
                .map_err(js_error)?;

            Ok(Self {
                view,
                resources,
                canvas,
                frames,
                events,
                clock,
                script_finished: false,
                disposed: false,
            })
        }
        .await;
        if result.is_err() {
            RENDERER_CREATED.store(false, Ordering::Release);
        }
        result
    }

    /// Internal Render-Worker seam: retain bytes that the browser host already
    /// fetched under its URL policy. Returns the normalized absolute URL.
    #[wasm_bindgen(js_name = registerScript)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript string and Uint8Array arguments"
    )]
    pub fn register_script(&self, url: String, bytes: Vec<u8>) -> Result<String, JsValue> {
        self.ensure_running()?;
        self.resources.register_script(&url, bytes)
    }

    /// Parse one browser-decoded Lynx XML source envelope and register its
    /// logical template sections as fixed fragments of the final response URL.
    ///
    /// The returned array is `[main, styleOrNull, backgroundOrNull]`. The
    /// Render Worker loads a present stylesheet before executing `main` and
    /// retains a present background script without executing it until Bobcat
    /// implements the background-thread runtime.
    #[wasm_bindgen(js_name = registerLynxXml)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript string arguments"
    )]
    pub fn register_lynx_xml(&self, source_url: String, source: String) -> Result<Array, JsValue> {
        self.ensure_running()?;
        let parsed = lynx_xml::parse(&source).map_err(js_error)?;

        let section_url = |fragment: &str| {
            let mut url = Url::parse(&source_url).map_err(|error| {
                js_error(format!(
                    "the Lynx XML response URL `{source_url}` is invalid: {error}"
                ))
            })?;
            url.set_fragment(Some(fragment));
            Ok::<_, JsValue>(url.to_string())
        };
        let main_section_url = section_url("main-thread")?;
        let style_section_url = section_url("style")?;
        let background_section_url = section_url("background-thread")?;

        let main_url = self.resources.register_script(
            &main_section_url,
            parsed.main_thread_script.as_bytes().to_vec(),
        )?;
        let style_url = parsed
            .style
            .map(|style| {
                self.resources
                    .register_style_sheet(&style_section_url, style.as_bytes().to_vec())
            })
            .transpose()?;
        let background_url = parsed
            .background_thread_script
            .map(|script| {
                self.resources
                    .register_script(&background_section_url, script.as_bytes().to_vec())
            })
            .transpose()?;

        let registration = Array::new();
        registration.push(&JsValue::from(main_url));
        registration.push(&style_url.map_or(JsValue::NULL, JsValue::from));
        registration.push(&background_url.map_or(JsValue::NULL, JsValue::from));
        Ok(registration)
    }

    /// Start the registered main-thread script through `LynxView`'s resource
    /// boundary. The Render Worker awaits completion independently from drawing.
    #[wasm_bindgen(js_name = executeScript)]
    pub async fn execute_script(&mut self, url: String) -> Result<(), JsValue> {
        self.ensure_running()?;
        self.view.execute_script(&url).await.map_err(js_error)
    }

    /// Load an author stylesheet through `LynxView`'s resource boundary.
    ///
    /// A browser embedder never decodes a `.web.bundle`, so the bytes it
    /// registers are CSS text and core takes the text arm of the stylesheet
    /// contract.
    #[wasm_bindgen(js_name = loadStyleSheet)]
    pub async fn load_style_sheet(&mut self, url: String) -> Result<(), JsValue> {
        self.ensure_running()?;
        self.view.load_style_sheet(&url).await.map_err(js_error)
    }

    /// Register CSS bytes the browser host fetched, under the URL
    /// [`Self::load_style_sheet`] will ask for.
    #[wasm_bindgen(js_name = registerStyleSheet)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript string and Uint8Array arguments"
    )]
    pub fn register_style_sheet(&self, url: String, bytes: Vec<u8>) -> Result<String, JsValue> {
        self.ensure_running()?;
        self.resources.register_style_sheet(&url, bytes)
    }

    /// Register every usable face in an embedder-provided font container.
    #[wasm_bindgen(js_name = registerFonts)]
    pub fn register_fonts(&mut self, bytes: Vec<u8>) -> Result<usize, JsValue> {
        self.ensure_running()?;
        self.view.register_fonts(bytes).map_err(js_error)
    }

    /// Map CSS's default generic families to an embedder-selected font family.
    #[wasm_bindgen(js_name = setDefaultFontFamily)]
    pub fn set_default_font_family(&mut self, family: String) -> Result<bool, JsValue> {
        self.ensure_running()?;
        self.view.set_default_font_family(&family).map_err(js_error)
    }

    /// Await the next durable engine wakeup without timer polling.
    #[wasm_bindgen(js_name = waitForEngineEvent)]
    pub fn wait_for_engine_event(&self) -> Result<Promise, JsValue> {
        self.ensure_running()?;
        let wait = self.events.wait();
        Ok(future_to_promise(async move {
            wait.await;
            Ok(JsValue::UNDEFINED)
        }))
    }

    /// Drain script engine events independently of animation frames.
    #[wasm_bindgen(js_name = pollScript)]
    pub fn poll_script(&mut self) -> Result<bool, JsValue> {
        self.ensure_running()?;
        if !self.script_finished && self.events.take() {
            for event in self.view.pump() {
                match event {
                    EngineEvent::ScriptFinished(Ok(())) => self.script_finished = true,
                    EngineEvent::ScriptFinished(Err(error)) => return Err(js_error(error)),
                    _ => {}
                }
            }
        }
        Ok(self.script_finished)
    }

    /// Present a requested frame without exposing the engine or document to
    /// the browser host.
    ///
    /// `now_ms` is `requestAnimationFrame`'s `DOMHighResTimeStamp`, which is
    /// also the animation timeline: every animation in the frame is sampled
    /// at the instant the host says the frame is for.
    #[wasm_bindgen(js_name = renderIfRequested)]
    pub fn render_if_requested(&mut self, now_ms: f64) -> Result<bool, JsValue> {
        self.ensure_running()?;
        if !self.frames.take() {
            return Ok(false);
        }
        if now_ms.is_finite() {
            self.clock.set(now_ms / 1000.0);
        }
        self.view.notify_redraw().map_err(js_error)?;
        Ok(true)
    }

    /// Apply browser device metrics and resize the Worker-owned surface.
    #[wasm_bindgen(js_name = resize)]
    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        validate_metrics(width, height, device_pixel_ratio)?;
        self.view
            .resize(width, height, device_pixel_ratio)
            .map_err(js_error)?;
        set_canvas_size(&self.canvas, self.view.frame_size());
        Ok(())
    }

    /// Release the browser facade. Engine-owned workers finish their current
    /// operation and drop their private runtime state naturally.
    #[wasm_bindgen(js_name = dispose)]
    pub fn dispose(&mut self) {
        self.disposed = true;
    }
}

impl BobcatRenderer {
    fn ensure_running(&self) -> Result<(), JsValue> {
        if self.disposed {
            Err(js_error("the Bobcat renderer is disposed"))
        } else {
            Ok(())
        }
    }
}

fn validate_metrics(width: f32, height: f32, ratio: f32) -> Result<(), JsValue> {
    let physical_width = f64::from(width) * f64::from(ratio);
    let physical_height = f64::from(height) * f64::from(ratio);
    if width.is_finite()
        && height.is_finite()
        && ratio.is_finite()
        && width > 0.0
        && height > 0.0
        && ratio > 0.0
        && physical_width <= MAX_RENDER_DIMENSION
        && physical_height <= MAX_RENDER_DIMENSION
    {
        Ok(())
    } else {
        Err(js_error(format!(
            "viewport metrics must be finite, positive, and no larger than \
             {MAX_RENDER_DIMENSION:.0} physical pixels per axis; got \
             {width}x{height} at {ratio}x"
        )))
    }
}

fn set_canvas_size(canvas: &OffscreenCanvas, size: FrameSize) {
    canvas.set_width(size.width);
    canvas.set_height(size.height);
}

fn js_error(error: impl fmt::Display) -> JsValue {
    js_sys::Error::new(&error.to_string()).into()
}
