//! Shared-memory browser composition exported through `wasm-bindgen`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::{fmt, mem};

use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
use bobcat_core::resource::{
    CacheStatus, RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceFuture, ResourceLocality,
    ResourceMetadata, ResourceRequest, ResourceResponse, ResourceSource, ResourceTiming,
    RetryAdvice, StyleSheetResponse,
};
use bobcat_core::{
    DrawTarget, EngineEvent, EventRequester, FontBlob, FrameSize, LynxView, PageConfig,
    ViewSources, WindowTarget, configure_wasm_workers,
};
use http::HeaderMap;
use js_sys::{Array, Promise};
use url::Url;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;
use web_sys::OffscreenCanvas;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(value: &JsValue);
}

const MAX_STYLE_THREADS: u32 = 6;
const POINTER_DEVICE_MOUSE: u8 = 0;
const POINTER_DEVICE_TOUCH: u8 = 1;
const POINTER_DEVICE_PEN: u8 = 2;
const POINTER_PHASE_DOWN: u8 = 0;
const POINTER_PHASE_MOVE: u8 = 1;
const POINTER_PHASE_UP: u8 = 2;
const POINTER_PHASE_CANCEL: u8 = 3;
static RENDERER_CREATED: AtomicBool = AtomicBool::new(false);

/// Lost-wake-safe engine signal: one durable wakeup covering both a lifecycle
/// event to drain and a frame to draw. The Lynx-main Worker arms it whenever
/// it publishes, and this Worker arms it for itself whenever it hands the
/// view a fact whose frame it still owes — the view paints on this thread and
/// wakes nobody on its own. The atomic is the durable edge; the waker list
/// only turns it into an awaitable browser Promise.
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

/// Browser-owned resources registered by the Render Worker after it applies
/// the browser's URL, fetch, CORS, cache, and credentials policies.
type BrowserResourceRegistry = Mutex<HashMap<String, Arc<[u8]>>>;

#[derive(Debug, Default)]
struct BrowserResources {
    resources: BrowserResourceRegistry,
}

impl BrowserResources {
    fn clear(&self) {
        self.resources
            .lock()
            .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
            .clear();
    }

    fn register_script(&self, url: &str, bytes: Vec<u8>) -> Result<String, JsValue> {
        self.register("script", url, bytes)
    }

    fn register_style_sheet(&self, url: &str, bytes: Vec<u8>) -> Result<String, JsValue> {
        self.register("stylesheet", url, bytes)
    }

    fn register(&self, label: &str, url: &str, bytes: Vec<u8>) -> Result<String, JsValue> {
        let url = Url::parse(url)
            .map_err(|error| js_error(format!("the {label} URL `{url}` is invalid: {error}")))?;
        let normalized = url.to_string();
        self.resources
            .lock()
            .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
            .insert(normalized.clone(), Arc::from(bytes));
        Ok(normalized)
    }

    fn contains_url(&self, url: &Url) -> bool {
        self.resources
            .lock()
            .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
            .contains_key(url.as_str())
    }

    fn registered_bytes(&self, url: &Url) -> Option<Arc<[u8]>> {
        self.resources
            .lock()
            .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
            .get(url.as_str())
            .cloned()
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
}

impl ResourceFetcher for BrowserResources {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        capability == ResourceCapability::BufferedResource
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        let request_id = request.context.id;
        let locator = request.resource.specifier.clone();

        let parsed = Url::parse(&locator).or_else(|_| {
            request
                .resource
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
                "resource locator is not a valid URL",
            );
        };
        if !self.contains_url(&url) {
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

    fn fetch_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceResponse> {
        let request_id = request.context.id;
        let locator: Arc<str> = Arc::from(request.resource.url.as_str());
        let source = self.registered_bytes(&request.resource.url);
        let Some(source) = source else {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Open,
                Some(locator),
                "the registered resource disappeared before it was loaded",
            );
        };
        let content_length = source.len() as u64;

        let resource = request.resource;
        Box::pin(async move {
            Ok(ResourceResponse {
                metadata: ResourceMetadata {
                    request_id,
                    resource,
                    headers: HeaderMap::default(),
                    content_length: Some(content_length),
                    media_type: None,
                    source: ResourceSource::MemoryCache,
                    cache_status: CacheStatus::default(),
                    timing: ResourceTiming::default(),
                },
                bytes: source.as_ref().to_vec().into(),
            })
        })
    }
}

/// The browser embedder serves no images yet: a page draws whatever its own
/// stylesheet describes, and nothing here fetches a bitmap. Every image draw
/// therefore resolves to nothing, which is what an unloaded image looks like.
impl bobcat_core::FrameImages for BrowserResources {
    fn read(&self, _image: bobcat_core::ImageRef) -> Option<bobcat_core::vello::peniko::ImageData> {
        None
    }
}

/// The per-view handle on the Worker's one resource registry.
///
/// The registry outlives every page this Worker shows, so it stays an `Arc`;
/// the painter's handle is an `Rc`, because a fetcher never leaves the thread
/// that owns the view.
struct ViewResources(Arc<BrowserResources>);

impl bobcat_core::FrameImages for ViewResources {
    fn read(&self, image: bobcat_core::ImageRef) -> Option<bobcat_core::vello::peniko::ImageData> {
        self.0.read(image)
    }
}

impl ResourceFetcher for ViewResources {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        self.0.supports_capability(capability)
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        self.0.resolve_locator(request)
    }

    fn fetch_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceResponse> {
        self.0.fetch_resource(request)
    }

    fn fetch_style_sheet(
        &self,
        request: ResourceRequest,
    ) -> ResourceFuture<'_, StyleSheetResponse> {
        self.0.fetch_style_sheet(request)
    }
}

/// A complete browser embedder, permanently owned by the explicit Render
/// Worker that constructs it. Its canvas, Wasm instance, resource provider,
/// and Stylo pool outlive every page it shows; each document, element tree,
/// `QuickJS` realm, and engine stays behind one opaque `LynxView`, built by
/// [`BobcatRenderer::load`] and replaced wholesale by the next load.
#[wasm_bindgen]
pub struct BobcatRenderer {
    view: Option<LynxView>,
    resources: Arc<BrowserResources>,
    canvas: OffscreenCanvas,
    events: Arc<EventSignal>,
    config: PageConfig,
    width: f32,
    height: f32,
    device_pixel_ratio: f32,
    /// Owned font containers are part of the stable browser wrapper, so every
    /// view this renderer builds receives the same registered faces without
    /// another UI-to-Worker transfer.
    fonts: Vec<FontBlob>,
    /// The selected embedder default is stable wrapper state too; every view
    /// this renderer builds gets the same generic-family map.
    default_font_family: Option<String>,
    script_finished: bool,
    disposed: bool,
}

impl fmt::Debug for BobcatRenderer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BobcatRenderer")
            .field("has_view", &self.view.is_some())
            .field("disposed", &self.disposed)
            .finish_non_exhaustive()
    }
}

#[wasm_bindgen]
impl BobcatRenderer {
    /// Construct the browser embedder on its explicit Render Worker and give
    /// core the Worker bootstrap it will use for the main-thread VM and Stylo.
    ///
    /// No native view exists until [`Self::load`] builds one.
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
        FrameSize::for_viewport(width, height, device_pixel_ratio).map_err(js_error)?;
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
                "one Bobcat Wasm instance supports exactly one renderer; load another page into the existing renderer to replace its native view",
            ));
        }

        let result = async move {
            configure_wasm_workers(worker_url, style_thread_count as usize).map_err(js_error)?;

            let resources = Arc::new(BrowserResources::default());
            let events = Arc::new(EventSignal::default());
            let config = PageConfig {
                default_display_linear,
                default_overflow_visible,
                enable_css_selector,
            };

            Ok(Self {
                view: None,
                resources,
                canvas,
                events,
                config,
                width,
                height,
                device_pixel_ratio,
                fonts: Vec::new(),
                default_font_family: None,
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

    /// Build the native `LynxView` for one page and attach it to the
    /// Worker-owned `OffscreenCanvas`.
    ///
    /// A view is its page, so loading a second one replaces the view rather
    /// than mutating the running one. The Wasm instance, Stylo pool, resource
    /// provider, page configuration, metrics, font containers, and default
    /// family are this wrapper's and are reapplied to each new view.
    #[wasm_bindgen(js_name = load)]
    pub async fn load(
        &mut self,
        entry_url: String,
        style_sheet_urls: Vec<String>,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;

        // Dropping the previous view explicitly stops and joins its Lynx-main
        // Worker before construction of the independent replacement begins.
        drop(self.view.take());
        // Release the old page's post-boot waiter. The Render Worker advances
        // its generation before calling load, so it exits without pumping the
        // replacement view.
        self.events.request_event();
        self.script_finished = false;

        let sources = ViewSources {
            fonts: self.fonts.clone(),
            default_font_family: self.default_font_family.clone(),
            style_sheets: style_sheet_urls,
            ..ViewSources::new(
                {
                    let resources = Arc::clone(&self.resources);
                    Box::new(move |_sink| Rc::new(ViewResources(resources)))
                },
                entry_url,
            )
        };
        // A canvas owns its own resolution — configuring a context does not
        // set it — and the view builds its surface from this canvas during
        // construction. So it is sized first, to the same physical target
        // those metrics give the view.
        let frame_size = FrameSize::for_viewport(self.width, self.height, self.device_pixel_ratio)
            .map_err(js_error)?;
        set_canvas_size(&self.canvas, frame_size);
        let built = LynxView::new(
            self.config,
            self.events.clone(),
            self.width,
            self.height,
            self.device_pixel_ratio,
            DrawTarget::window(WindowTarget::OffscreenCanvas(self.canvas.clone())),
            sources,
        )
        .await;
        // Construction has finished every source load and released its fetcher
        // clone, so this page's registered bytes are dead either way. Clearing
        // here keeps a Render Worker that loads page after page from growing a
        // registry of them.
        self.resources.clear();
        self.view = Some(built.map_err(js_error)?);
        // Boot published its frame before this Worker took a turn, and a
        // frame from below wakes through the same signal — but the wakeup it
        // sent was consumed by the loop that was waiting on the *previous*
        // page. This Worker therefore owes itself the first turn.
        self.events.request_event();
        Ok(())
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
    /// Render Worker hands `main` and a present style URL to [`Self::load`];
    /// a background section is reported by URL only, and neither retained nor
    /// executed until Bobcat implements the background-thread runtime.
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
        // The background body is named, not retained: nothing executes it, and
        // a view's construction copies only what it loads.
        let background_url = parsed
            .background_thread_script
            .map(|_| background_section_url);

        let registration = Array::new();
        registration.push(&JsValue::from(main_url));
        registration.push(&style_url.map_or(JsValue::NULL, JsValue::from));
        registration.push(&background_url.map_or(JsValue::NULL, JsValue::from));
        Ok(registration)
    }

    /// Register CSS bytes the browser host fetched, under a URL [`Self::load`]
    /// will name among its stylesheets.
    ///
    /// A browser embedder never decodes a `.web.bundle`, so the bytes it
    /// registers are CSS text and core takes the text arm of the stylesheet
    /// contract.
    #[wasm_bindgen(js_name = registerStyleSheet)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript string and Uint8Array arguments"
    )]
    pub fn register_style_sheet(&self, url: String, bytes: Vec<u8>) -> Result<String, JsValue> {
        self.ensure_running()?;
        self.resources.register_style_sheet(&url, bytes)
    }

    /// Retain a font container for every view this renderer builds. Faces are
    /// registered at construction, so call this before [`Self::load`].
    #[wasm_bindgen(js_name = registerFonts)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript Uint8Array arguments"
    )]
    pub fn register_fonts(&mut self, bytes: Vec<u8>) -> Result<(), JsValue> {
        self.ensure_running()?;
        self.fonts.push(FontBlob::new(bytes));
        Ok(())
    }

    /// Map CSS's default generic families to an embedder-selected family for
    /// every view this renderer builds. The name is checked at construction:
    /// [`Self::load`] fails if nothing provides it.
    #[wasm_bindgen(js_name = setDefaultFontFamily)]
    pub fn set_default_font_family(&mut self, family: String) -> Result<(), JsValue> {
        self.ensure_running()?;
        self.default_font_family = Some(family);
        Ok(())
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

    /// Answer one durable engine wakeup: run the view's turn on this
    /// Worker — routing whatever was queued, drawing the frame it owes — and
    /// hand back the lifecycle events it produced. Returns whether the entry
    /// module has finished booting.
    ///
    /// One call covers everything because the engine has one wakeup: a commit
    /// and a `ScriptFinished` arrive on the same signal, and the Worker turn
    /// that observes it is the turn that presents — no animation-frame
    /// callback sits between a committed frame and the canvas. No timestamp
    /// crosses either: the animation timeline is core's own clock, read on
    /// this Worker once the canvas surface has handed over an image.
    ///
    /// Anything the turn leaves owed — a running animation, a swap chain
    /// that had no image to give — is answered by [`Self::owes_frame`], which
    /// this Worker's loop reads before it parks, and takes at the display's
    /// own rate. Nothing is re-armed here: the signal carries facts from the
    /// Lynx main thread, and a frame is not one of them.
    ///
    /// A fatal error is reported after the turn, never instead of it: the
    /// frame a failed script did commit still reaches the canvas, with nobody
    /// left to ask for another.
    #[wasm_bindgen(js_name = pump)]
    pub fn pump(&mut self) -> Result<bool, JsValue> {
        self.ensure_running()?;
        // Clear the durable edge before serving, so anything the turn itself
        // publishes re-arms it and the Worker comes back for it.
        self.events.take();
        let Some(view) = self.view.as_mut() else {
            return Ok(self.script_finished);
        };
        let mut fatal = None;
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => self.script_finished = true,
                // The first failure is the one reported.
                EngineEvent::ScriptRunError(error) if fatal.is_none() => {
                    fatal = Some(js_error(error));
                }
                EngineEvent::RenderFailed(error) if fatal.is_none() => {
                    fatal = Some(js_error(error));
                }
                EngineEvent::ListenerFailed(error) => console_error(&js_error(error)),
                _ => {}
            }
        }
        if let Some(error) = fatal {
            return Err(error);
        }
        Ok(self.script_finished)
    }

    /// Whether the view has a frame to put on the canvas: a running
    /// animation, a gesture deadline the clock has yet to reach, a swap chain
    /// that had no image to give, or a commit the last turn did not draw.
    ///
    /// The Render Worker's continuation signal. While it holds, the loop
    /// takes its next turn at the display's own rate —
    /// `requestAnimationFrame` on this target — instead of parking on the
    /// engine's wakeup. Nothing crosses a thread for it: on a canvas the
    /// acquire never waits, so the display is the only honest pace there is.
    #[wasm_bindgen(js_name = owesFrame)]
    pub fn owes_frame(&self) -> bool {
        self.view.as_ref().is_some_and(LynxView::owes_frame)
    }

    /// Route one browser `PointerEvent` into the opaque native view.
    ///
    /// The JavaScript facade owns pointer capture and converts client
    /// coordinates into viewport CSS px. Core stamps the event's arrival from
    /// its own clock as it takes it, so a long idle period cannot make a new
    /// `longpress` deadline start from the last rendered frame.
    #[wasm_bindgen(js_name = dispatchPointer)]
    #[allow(
        clippy::too_many_arguments,
        reason = "the internal wasm-bindgen seam carries one flat pointer message"
    )]
    pub fn dispatch_pointer(
        &mut self,
        x: f32,
        y: f32,
        pointer_id: u32,
        device: u8,
        phase: u8,
        default_prevented: bool,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        if !x.is_finite() || !y.is_finite() {
            return Err(js_error("pointer coordinates must be finite"));
        }
        let device = match device {
            POINTER_DEVICE_MOUSE => PointerKind::Mouse,
            POINTER_DEVICE_TOUCH => PointerKind::Touch,
            POINTER_DEVICE_PEN => PointerKind::Pen,
            _ => return Err(js_error(format!("unknown browser pointer device {device}"))),
        };
        let phase = match phase {
            POINTER_PHASE_DOWN => PointerPhase::Down,
            POINTER_PHASE_MOVE => PointerPhase::Move,
            POINTER_PHASE_UP => PointerPhase::Up,
            POINTER_PHASE_CANCEL => PointerPhase::Cancel,
            _ => return Err(js_error(format!("unknown browser pointer phase {phase}"))),
        };
        let event = InputEvent::pointer(Point2D::new(x, y), pointer_id, device, phase)
            .with_default_prevented(default_prevented);
        // A pointer that arrives before any page is loaded has nothing to
        // reach; there is no view to route it against and nothing to buffer.
        if let Some(view) = self.view.as_mut() {
            view.dispatch_input(event);
            // Routing happened here, on this Worker; the frame it may owe is
            // this Worker's to take, and the loop is parked until it is told.
            self.events.request_event();
        }
        Ok(())
    }

    /// Apply browser device metrics and resize the Worker-owned surface.
    /// Metrics that arrive before a page is loaded become the next view's.
    #[wasm_bindgen(js_name = resize)]
    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        FrameSize::for_viewport(width, height, device_pixel_ratio).map_err(js_error)?;
        self.width = width;
        self.height = height;
        self.device_pixel_ratio = device_pixel_ratio;
        if let Some(view) = self.view.as_mut() {
            view.resize(width, height, device_pixel_ratio)
                .map_err(js_error)?;
            let frame_size = view.frame_size();
            set_canvas_size(&self.canvas, frame_size);
            self.events.request_event();
        }
        Ok(())
    }

    /// Release the current native view before the outer facade terminates its
    /// Render Worker and the Wasm session with it. Dropping the view stops and
    /// joins its Lynx-main Worker.
    #[wasm_bindgen(js_name = dispose)]
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.events.request_event();
        drop(self.view.take());
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

fn set_canvas_size(canvas: &OffscreenCanvas, size: FrameSize) {
    canvas.set_width(size.width);
    canvas.set_height(size.height);
}

fn js_error(error: impl fmt::Display) -> JsValue {
    js_sys::Error::new(&error.to_string()).into()
}
