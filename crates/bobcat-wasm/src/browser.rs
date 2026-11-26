//! Shared-memory browser composition exported through `wasm-bindgen`.
//!
//! Lynx performance entries reach the page only when it asks for them. The
//! facade sets `observePerformance` in the init message when the page passed
//! `onPerformanceEntry`; the Render Worker then hands
//! [`BobcatRenderer::create`] a callback, and each view this renderer builds
//! is told to [`LynxView::observe_performance`] every entry type. Without the
//! callback no view is asked, and the engine assembles nothing. Each
//! [`EngineEvent::Performance`] that [`BobcatRenderer::pump`] drains is
//! written here, on the Render Worker, as the JSON object Lynx native's
//! `onPerformanceEvent` hands a host
//! ([`PerformanceEntry::write_lynx_json`]), and crosses as one string; the
//! facade parses it. The host's own marks around a view (Lynx's
//! `setExtraTiming`) come back the other way through
//! [`BobcatRenderer::set_extra_timing`] as Unix milliseconds.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use std::{fmt, mem};

use bobcat_core::editing::{Rect, TextInputSource, TextInputState};
use bobcat_core::input::{InputEvent, KeyPhase, Modifiers, Point2D, PointerKind, PointerPhase};
use bobcat_core::performance::{EntryTypes, ExtraTiming, PerformanceEntry, TimeOrigin};
use bobcat_core::{
    DrawTarget, EngineEvent, EventRequester, FontBlob, FrameSize, LynxGroup, LynxView,
    NativeModule, PageConfig, Painter, ScreenMetrics, StyleThreads, ViewSources, WindowTarget,
    configure_wasm_workers,
};
use bobcat_resources::{Resources, ResourcesConfig, ViewResources};
use bobcat_source::{PageSource, ZipSource, register_lynx_xml_response};
use js_sys::{Array, Object, Promise, Reflect};
use url::Url;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;
use web_sys::OffscreenCanvas;
use web_time::SystemTime;

#[cfg(feature = "explorer-module")]
use crate::explorer::{ExplorerModule, ExplorerNotice, ExplorerPage, LogLevel};
use crate::platform::{BrowserFrame, BrowserMedia, BrowserPlatform, CodeCacheConfig};
use crate::text_input::{edit_command, enter_key_hint_name, input_mode_name, input_type_name};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(value: &JsValue);
    #[wasm_bindgen(js_namespace = console, js_name = warn)]
    fn console_warn(value: &JsValue);
    #[wasm_bindgen(js_namespace = console, js_name = log)]
    fn console_log(value: &JsValue);
    #[cfg(feature = "explorer-module")]
    #[wasm_bindgen(js_namespace = console, js_name = info)]
    fn console_info(value: &JsValue);
}

const DEFAULT_LOCALE: &str = "en-US";
const POINTER_DEVICE_MOUSE: u8 = 0;
const POINTER_DEVICE_TOUCH: u8 = 1;
const POINTER_DEVICE_PEN: u8 = 2;
const POINTER_PHASE_DOWN: u8 = 0;
const POINTER_PHASE_MOVE: u8 = 1;
const POINTER_PHASE_UP: u8 = 2;
const POINTER_PHASE_CANCEL: u8 = 3;
const MODIFIER_ALT: u8 = 1;
const MODIFIER_CTRL: u8 = 2;
const MODIFIER_META: u8 = 4;
const MODIFIER_SHIFT: u8 = 8;
static RENDERER_CREATED: AtomicBool = AtomicBool::new(false);

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
        if self.signal.pending.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        if !wakers.iter().any(|waker| waker.will_wake(context.waker())) {
            wakers.push(context.waker().clone());
        }
        Poll::Pending
    }
}

#[wasm_bindgen]
pub struct BobcatRenderer {
    painter: Option<Painter<BrowserFrame>>,
    view: Option<LynxView<ViewResources<BrowserPlatform>>>,
    resources: Resources<BrowserPlatform>,
    page_resources: Option<Resources<BrowserPlatform>>,
    boot_urls: Vec<String>,
    canvas: OffscreenCanvas,
    events: Arc<EventSignal>,
    config: PageConfig,
    width: f32,
    height: f32,
    device_pixel_ratio: f32,
    screen: ScreenMetrics,
    fonts: Vec<FontBlob>,
    default_font_family: Option<String>,
    fallback_fonts: Vec<FontBlob>,
    locale: String,
    style_threads: StyleThreads,
    #[cfg(feature = "explorer-module")]
    on_explorer: js_sys::Function,
    on_text_input: js_sys::Function,
    text_input_focused: bool,
    on_performance: Option<js_sys::Function>,
    performance_json: String,
    group: Option<LynxGroup>,
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
    #[wasm_bindgen(js_name = create)]
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the internal wasm-bindgen seam carries the whole init message flat"
    )]
    pub async fn create(
        canvas: OffscreenCanvas,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        screen_pixel_width: f32,
        screen_pixel_height: f32,
        worker_url: String,
        hardware_concurrency: u32,
        default_display_linear: bool,
        default_overflow_visible: bool,
        enable_css_selector: bool,
        on_explorer: js_sys::Function,
        on_text_input: js_sys::Function,
        on_performance: Option<js_sys::Function>,
        on_media: js_sys::Function,
    ) -> Result<BobcatRenderer, JsValue> {
        FrameSize::for_viewport(width, height, device_pixel_ratio).map_err(js_error)?;
        if worker_url.is_empty() {
            return Err(js_error("the Bobcat worker URL must not be empty"));
        }
        let style_threads = usize::try_from(hardware_concurrency)
            .map_or(StyleThreads::Sequential, StyleThreads::for_parallelism);
        let screen = measured_screen(device_pixel_ratio, screen_pixel_width, screen_pixel_height)
            .unwrap_or_else(|| ScreenMetrics::for_viewport(width, height, device_pixel_ratio));
        if RENDERER_CREATED.swap(true, Ordering::AcqRel) {
            return Err(js_error(
                "one Bobcat Wasm instance supports exactly one renderer; load another page into the existing renderer to replace its native view",
            ));
        }

        #[cfg(not(feature = "explorer-module"))]
        drop(on_explorer);

        let result = async move {
            configure_wasm_workers(worker_url).map_err(js_error)?;

            let events = Arc::new(EventSignal::default());
            let frame_size =
                FrameSize::for_viewport(width, height, device_pixel_ratio).map_err(js_error)?;
            set_canvas_size(&canvas, frame_size);
            let painter = Painter::with_platform_frames(
                DrawTarget::window(WindowTarget::OffscreenCanvas(canvas.clone())),
                width,
                height,
                device_pixel_ratio,
            )
            .await
            .map_err(js_error)?;
            let (platform, notes) = BrowserPlatform::new(
                Some(CodeCacheConfig::default()),
                BrowserMedia::new(on_media),
            )
            .await;
            let resources = Resources::with_platform(
                ResourcesConfig {
                    container_installer: Some(Arc::new(bobcat_source::BundleInstaller)),
                    ..ResourcesConfig::default()
                },
                platform,
                notes,
                {
                    let events = Arc::clone(&events);
                    move || events.request_event()
                },
            );
            warn_notes(&resources);
            let config = PageConfig {
                default_display_linear,
                default_overflow_visible,
                enable_css_selector,
                ..PageConfig::default()
            };

            Ok(Self {
                painter: Some(painter),
                view: None,
                group: None,
                resources,
                page_resources: None,
                boot_urls: Vec::new(),
                canvas,
                events,
                config,
                width,
                height,
                device_pixel_ratio,
                screen,
                fonts: Vec::new(),
                default_font_family: None,
                fallback_fonts: Vec::new(),
                locale: DEFAULT_LOCALE.to_owned(),
                style_threads,
                #[cfg(feature = "explorer-module")]
                on_explorer,
                on_text_input,
                text_input_focused: false,
                on_performance,
                performance_json: String::new(),
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

    #[wasm_bindgen(js_name = load)]
    pub async fn load(
        &mut self,
        entry_url: String,
        style_sheet_urls: Vec<String>,
        background_entry_url: Option<String>,
        global_props: Option<String>,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;

        let sources = ViewSources {
            config: self.config,
            style_sheets: style_sheet_urls,
            background_entry: background_entry_url,
            ..ViewSources::new(entry_url.clone(), entry_url, self.screen, &self.locale)
        };
        self.load_sources(sources, global_props).await
    }

    #[wasm_bindgen(js_name = loadTemplate)]
    pub async fn load_template(
        &mut self,
        url: String,
        bytes: Vec<u8>,
        global_props: Option<String>,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        let input = Url::parse(&url).map_err(js_error)?;
        let page = PageSource::defer(&input, bytes).map_err(js_error)?;
        page.register_in_background(&self.resources);
        self.load_sources(page.view_sources(self.screen, &self.locale), global_props)
            .await
    }

    #[wasm_bindgen(js_name = loadZip)]
    pub async fn load_zip(
        &mut self,
        entry_url: String,
        bytes: Vec<u8>,
        global_props: Option<String>,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        let input = Url::parse(&entry_url).map_err(js_error)?;
        let archive = ZipSource::from_bytes(&bytes).map_err(js_error)?;
        let page = archive.page(&input).map_err(js_error)?;
        archive
            .register_with(&self.resources, &input)
            .map_err(js_error)?;
        page.register_with(&self.resources);
        self.load_sources(page.view_sources(self.screen, &self.locale), global_props)
            .await
    }

    #[wasm_bindgen(js_name = registerScript)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript string and Uint8Array arguments"
    )]
    pub fn register_script(&self, url: String, bytes: Vec<u8>) -> Result<String, JsValue> {
        self.ensure_running()?;
        self.register(&url, bytes, "text/javascript")
    }

    #[wasm_bindgen(js_name = registerLynxXml)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript string arguments"
    )]
    pub fn register_lynx_xml(&self, source_url: String, source: String) -> Result<Array, JsValue> {
        self.ensure_running()?;
        let input_url = Url::parse(&source_url)
            .map_err(|error| js_error(format!("the Lynx XML response URL is invalid: {error}")))?;
        let registered =
            register_lynx_xml_response(&input_url, &source, &self.resources).map_err(js_error)?;
        let compatibility_warnings = Array::new();
        for warning in registered.compatibility_warnings() {
            compatibility_warnings.push(&JsValue::from(warning.as_str()));
        }

        let registration = Array::new();
        registration.push(&JsValue::from(registered.entry_url().to_string()));
        registration.push(
            &registered
                .style_sheet_url()
                .map_or(JsValue::NULL, |url| JsValue::from(url.to_string())),
        );
        registration.push(
            &registered
                .background_thread_url()
                .map_or(JsValue::NULL, |url| JsValue::from(url.to_string())),
        );
        registration.push(&compatibility_warnings);
        Ok(registration)
    }

    #[wasm_bindgen(js_name = registerStyleSheet)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript string and Uint8Array arguments"
    )]
    pub fn register_style_sheet(&self, url: String, bytes: Vec<u8>) -> Result<String, JsValue> {
        self.ensure_running()?;
        self.register(&url, bytes, "text/css")
    }

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

    #[wasm_bindgen(js_name = setDefaultFontFamily)]
    pub fn set_default_font_family(&mut self, family: String) -> Result<(), JsValue> {
        self.ensure_running()?;
        self.default_font_family = Some(family);
        Ok(())
    }

    #[wasm_bindgen(js_name = registerFallbackFonts)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript Uint8Array arguments"
    )]
    pub fn register_fallback_fonts(&mut self, bytes: Vec<u8>) -> Result<(), JsValue> {
        self.ensure_running()?;
        self.fallback_fonts.push(FontBlob::new(bytes));
        Ok(())
    }

    #[wasm_bindgen(js_name = setLocale)]
    pub fn set_locale(&mut self, tag: String) -> Result<(), JsValue> {
        self.ensure_running()?;
        self.locale = tag;
        Ok(())
    }

    #[wasm_bindgen(js_name = waitForEngineEvent)]
    pub fn wait_for_engine_event(&self) -> Result<Promise, JsValue> {
        self.ensure_running()?;
        let wait = self.events.wait();
        Ok(future_to_promise(async move {
            wait.await;
            Ok(JsValue::UNDEFINED)
        }))
    }

    #[wasm_bindgen(js_name = pump)]
    pub fn pump(&mut self) -> Result<bool, JsValue> {
        self.ensure_running()?;
        self.events.take();
        let mut fatal = None;
        if let Some(painter) = self.painter.as_mut()
            && let Err(error) = painter.pump()
        {
            fatal = Some(js_error(error));
        }
        let Some(view) = self.view.as_mut() else {
            return fatal.map_or(Ok(self.script_finished), Err);
        };
        let mut boot_finished = false;
        if let Some(resources) = &self.page_resources {
            warn_notes(resources);
        }
        let mut text_input = None;
        let origin = view.time_origin();
        for event in view.pump() {
            match event {
                EngineEvent::TextInputStateChanged(state) => text_input = Some(state),
                EngineEvent::Performance(entry) => {
                    post_performance(
                        self.on_performance.as_ref(),
                        &mut self.performance_json,
                        &entry,
                        &origin,
                    );
                }
                EngineEvent::ScriptFinished => {
                    self.script_finished = true;
                    boot_finished = true;
                }
                event if event.is_fatal() && fatal.is_none() => {
                    fatal = Some(fatal_error(event));
                }
                EngineEvent::ScriptRunError(error)
                | EngineEvent::ListenerFailed(error)
                | EngineEvent::TimerFailed(error) => {
                    console_error(&js_error(error));
                }
                EngineEvent::WorkerThrew { source, error } => {
                    console_error(&js_error(format!("[{source}] worker threw: {error}")));
                }
                EngineEvent::WorkerEnded { source, error } => {
                    console_error(&js_error(format!("[{source}] worker ended: {error}")));
                }
                EngineEvent::ScriptReported {
                    source,
                    level,
                    message,
                } => {
                    let message = JsValue::from_str(&format!("[{source}] [{level}] {message}"));
                    if level == "warn" {
                        console_warn(&message);
                    } else {
                        console_error(&message);
                    }
                }
                EngineEvent::ConsoleMessage {
                    source,
                    level,
                    message,
                } => {
                    let message = JsValue::from_str(&format!("[{source}] [{level}] {message}"));
                    match level.as_str() {
                        "error" => console_error(&message),
                        "warn" => console_warn(&message),
                        _ => console_log(&message),
                    }
                }
                _ => {}
            }
        }
        if let Some(state) = text_input {
            self.post_text_input(&state);
        }
        if (boot_finished || fatal.is_some())
            && let Some(resources) = &self.page_resources
        {
            for url in self.boot_urls.drain(..) {
                let _ = resources.unregister(&url);
            }
        }
        if let Some(error) = fatal {
            return Err(error);
        }
        Ok(self.script_finished)
    }

    #[wasm_bindgen(js_name = owesFrame)]
    pub fn owes_frame(&self) -> bool {
        self.painter.as_ref().is_some_and(Painter::owes_frame)
    }

    pub fn vsync(&mut self) {
        if let Some(painter) = self.painter.as_mut() {
            painter.vsync();
        }
    }

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
        button: i16,
        buttons: u16,
        default_prevented: bool,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        if !x.is_finite() || !y.is_finite() {
            return Err(js_error("pointer coordinates must be finite"));
        }
        let device = match device {
            POINTER_DEVICE_TOUCH => PointerKind::Touch,
            POINTER_DEVICE_MOUSE => PointerKind::Mouse,
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
            .with_buttons(button, buttons)
            .with_default_prevented(default_prevented);
        if let Some(painter) = self.painter.as_mut() {
            painter.dispatch_input(event);
            self.events.request_event();
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = dispatchWheel)]
    pub fn dispatch_wheel(
        &mut self,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        default_prevented: bool,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        if !x.is_finite() || !y.is_finite() {
            return Err(js_error("wheel coordinates must be finite"));
        }
        if !delta_x.is_finite() || !delta_y.is_finite() {
            return Err(js_error("wheel deltas must be finite"));
        }
        let event = InputEvent::wheel(Point2D::new(x, y), (delta_x, delta_y))
            .with_default_prevented(default_prevented);
        if let Some(painter) = self.painter.as_mut() {
            painter.dispatch_input(event);
            self.events.request_event();
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = dispatchKey)]
    pub fn dispatch_key(
        &mut self,
        key: String,
        code: String,
        down: bool,
        repeat: bool,
        modifiers: u8,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        let phase = if down { KeyPhase::Down } else { KeyPhase::Up };
        let modifiers = Modifiers {
            alt: modifiers & MODIFIER_ALT != 0,
            ctrl: modifiers & MODIFIER_CTRL != 0,
            meta: modifiers & MODIFIER_META != 0,
            shift: modifiers & MODIFIER_SHIFT != 0,
        };
        let event = InputEvent::key(phase, key, code, repeat, modifiers);
        if let Some(painter) = self.painter.as_mut() {
            painter.dispatch_input(event);
            self.events.request_event();
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = edit)]
    pub fn edit(
        &mut self,
        kind: &str,
        text: Option<String>,
        first: i32,
        second: i32,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        let command = edit_command(kind, text, first, second).map_err(js_error)?;
        if let Some(view) = self.view.as_ref() {
            let _ = view.edit(command);
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = setExtraTiming)]
    pub fn set_extra_timing(
        &self,
        open_time: f64,
        container_init_start: f64,
        container_init_end: f64,
        prepare_template_start: f64,
        prepare_template_end: f64,
    ) -> Result<(), JsValue> {
        self.ensure_running()?;
        let Some(view) = self.view.as_ref() else {
            return Ok(());
        };
        let _ = view.set_extra_timing(ExtraTiming {
            open_time: wall_clock(open_time),
            container_init_start: wall_clock(container_init_start),
            container_init_end: wall_clock(container_init_end),
            prepare_template_start: wall_clock(prepare_template_start),
            prepare_template_end: wall_clock(prepare_template_end),
        });
        Ok(())
    }

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
        if let Some(painter) = self.painter.as_mut() {
            painter
                .resize(width, height, device_pixel_ratio)
                .map_err(js_error)?;
            set_canvas_size(&self.canvas, painter.frame_size());
            self.events.request_event();
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = dispose)]
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.events.request_event();
        if let Some(painter) = self.painter.as_mut() {
            painter.detach();
        }
        drop(self.view.take());
        drop(self.group.take());
        drop(self.painter.take());
        self.page_resources = None;
        self.boot_urls.clear();
    }
}

impl BobcatRenderer {
    async fn load_sources(
        &mut self,
        mut sources: ViewSources,
        global_props: Option<String>,
    ) -> Result<(), JsValue> {
        let base_url = Url::parse(&sources.base_url).map_err(js_error)?;
        if let Some(painter) = self.painter.as_mut() {
            painter.detach();
        }
        drop(self.view.take());
        drop(self.group.take());
        self.page_resources = None;
        self.boot_urls.clear();
        if self.text_input_focused {
            self.post_text_input(&TextInputState::default());
        }
        self.events.request_event();
        self.script_finished = false;

        self.resources.set_base_url(Some(base_url));
        sources.fonts = self.fonts.clone();
        sources.default_font_family = self.default_font_family.clone();
        sources.fallback_fonts = self.fallback_fonts.clone();
        sources.locale.clone_from(&self.locale);
        sources.global_props = global_props;
        if self
            .painter
            .as_ref()
            .is_none_or(Painter::<BrowserFrame>::has_failed)
        {
            drop(self.painter.take());
            self.painter = Some(
                Painter::with_platform_frames(
                    DrawTarget::window(WindowTarget::OffscreenCanvas(self.canvas.clone())),
                    self.width,
                    self.height,
                    self.device_pixel_ratio,
                )
                .await
                .map_err(js_error)?,
            );
        }
        let next_resources = self.resources.new_scope();
        let resources = mem::replace(&mut self.resources, next_resources);
        let group = LynxGroup::new(self.events.clone(), self.style_threads)
            .await
            .map_err(js_error)?;
        let boot_urls: Vec<_> = std::iter::once(sources.entry.clone())
            .chain(sources.style_sheets.iter().cloned())
            .collect();
        let native_modules = self.native_modules();
        let built = group.create_lynx_view(
            self.width,
            self.height,
            self.device_pixel_ratio,
            resources.builder(),
            native_modules,
            sources,
        );
        warn_notes(&resources);
        let view = built.map_err(js_error)?;
        let _ = view.set_text_input_source(TextInputSource::Commands);
        if self.on_performance.is_some() {
            view.observe_performance(EntryTypes::ALL);
        }
        if let Some(painter) = self.painter.as_mut() {
            painter.attach(&view).map_err(js_error)?;
        }
        self.view = Some(view);
        self.page_resources = Some(resources);
        self.boot_urls = boot_urls;
        self.group = Some(group);
        self.events.request_event();
        Ok(())
    }

    fn post_text_input(&mut self, state: &TextInputState) {
        self.text_input_focused = state.host.is_some();
        let fields = text_input_fields(state);
        if let Err(error) = self.on_text_input.call1(&JsValue::NULL, &fields) {
            console_error(&error);
        }
    }

    fn register(&self, url: &str, bytes: Vec<u8>, media_type: &str) -> Result<String, JsValue> {
        self.resources
            .register(url, bytes, Some(media_type))
            .map(|url| url.to_string())
            .map_err(js_error)
    }

    fn ensure_running(&self) -> Result<(), JsValue> {
        if self.disposed {
            Err(js_error("the Bobcat renderer is disposed"))
        } else {
            Ok(())
        }
    }

    #[cfg_attr(
        not(feature = "explorer-module"),
        expect(
            clippy::unused_self,
            reason = "the build without the feature reads nothing of the renderer"
        )
    )]
    fn native_modules(&self) -> Vec<Box<dyn NativeModule>> {
        #[cfg(feature = "explorer-module")]
        {
            vec![Box::new(ExplorerModule::new(BrowserExplorerPage {
                post: self.on_explorer.clone(),
            }))]
        }
        #[cfg(not(feature = "explorer-module"))]
        {
            Vec::new()
        }
    }
}

#[cfg(feature = "explorer-module")]
#[derive(Debug)]
struct BrowserExplorerPage {
    post: js_sys::Function,
}

#[cfg(feature = "explorer-module")]
impl BrowserExplorerPage {
    fn post(&self, kind: &str, fields: &[(&str, &str)]) {
        let message = Object::new();
        let set = |key: &str, value: &str| {
            let _ = Reflect::set(&message, &JsValue::from_str(key), &JsValue::from_str(value));
        };
        set("type", kind);
        for (key, value) in fields {
            set(key, value);
        }
        if let Err(error) = self.post.call1(&JsValue::NULL, &message) {
            console_error(&error);
        }
    }
}

#[cfg(feature = "explorer-module")]
impl ExplorerPage for BrowserExplorerPage {
    fn open_schema(&self, schema: String) {
        self.post("bobcat-explorer-open-schema", &[("schema", &schema)]);
    }

    fn store_preference(&self, key: String, value: String) {
        self.post("bobcat-explorer-store", &[("key", &key), ("value", &value)]);
    }

    fn notice(&self, notice: ExplorerNotice) {
        self.post("bobcat-explorer-notice", &[("notice", notice.name())]);
    }

    fn log(&self, level: LogLevel, message: &str) {
        let message = JsValue::from_str(message);
        match level {
            LogLevel::Info => console_info(&message),
            LogLevel::Warn => console_warn(&message),
        }
    }
}

fn post_performance(
    callback: Option<&js_sys::Function>,
    json: &mut String,
    entry: &PerformanceEntry,
    origin: &TimeOrigin,
) {
    let Some(callback) = callback else {
        return;
    };
    json.clear();
    entry.write_lynx_json(origin, json);
    if let Err(error) = callback.call1(&JsValue::NULL, &JsValue::from_str(json)) {
        console_error(&error);
    }
}

fn wall_clock(unix_millis: f64) -> Option<SystemTime> {
    if !unix_millis.is_finite() || unix_millis < 0.0 {
        return None;
    }
    let since_epoch = Duration::try_from_secs_f64(unix_millis / 1000.0).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(since_epoch)
}

fn warn_notes(resources: &Resources<BrowserPlatform>) {
    for note in resources.take_notes() {
        console_warn(&JsValue::from(format!("bobcat-resources: {note}")));
    }
}

fn measured_screen(pixel_ratio: f32, pixel_width: f32, pixel_height: f32) -> Option<ScreenMetrics> {
    (pixel_width.is_finite() && pixel_height.is_finite() && pixel_width > 0.0 && pixel_height > 0.0)
        .then_some(ScreenMetrics {
            pixel_ratio,
            pixel_width,
            pixel_height,
        })
}

fn set_canvas_size(canvas: &OffscreenCanvas, size: FrameSize) {
    canvas.set_width(size.width);
    canvas.set_height(size.height);
}

fn js_error(error: impl fmt::Display) -> JsValue {
    js_sys::Error::new(&error.to_string()).into()
}

fn fatal_error(event: EngineEvent) -> JsValue {
    match event {
        EngineEvent::StartupFailed(error) => js_error(error),
        EngineEvent::Panicked(error) => js_error(error),
        event => js_error(format!("{event:?}")),
    }
}

fn text_input_fields(state: &TextInputState) -> JsValue {
    let fields = Object::new();
    let set = |key: &str, value: &JsValue| {
        let _ = Reflect::set(&fields, &JsValue::from_str(key), value);
    };
    set("host", &JsValue::from_bool(state.host.is_some()));
    set(
        "inputType",
        &JsValue::from_str(input_type_name(state.input_type)),
    );
    set(
        "inputMode",
        &JsValue::from_str(input_mode_name(state.input_mode)),
    );
    set(
        "enterKeyHint",
        &JsValue::from_str(enter_key_hint_name(state.enter_key_hint)),
    );
    set("value", &JsValue::from_str(&state.value));
    set("selectionStart", &JsValue::from(state.selection.start));
    set("selectionEnd", &JsValue::from(state.selection.end));
    set(
        "composition",
        &state.composition.as_ref().map_or(JsValue::NULL, |range| {
            let composition = Object::new();
            let _ = Reflect::set(&composition, &"start".into(), &range.start.into());
            let _ = Reflect::set(&composition, &"end".into(), &range.end.into());
            composition.into()
        }),
    );
    set("caret", &rect_fields(&state.caret));
    set("editableBounds", &rect_fields(&state.editable_bounds));
    let bounds = Array::new();
    for rect in &state.composition_bounds {
        bounds.push(&rect_fields(rect));
    }
    set("compositionBounds", &bounds);
    fields.into()
}

fn rect_fields(rect: &Rect<f32>) -> JsValue {
    let fields = Object::new();
    for (key, value) in [
        ("x", rect.origin.x),
        ("y", rect.origin.y),
        ("width", rect.size.width),
        ("height", rect.size.height),
    ] {
        let _ = Reflect::set(&fields, &JsValue::from_str(key), &JsValue::from(value));
    }
    fields.into()
}
