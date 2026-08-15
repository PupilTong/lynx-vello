//! Shared-memory browser composition exported through `wasm-bindgen`.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bobcat_core::resource::{
    BufferedResourceRequest, CacheStatus, HttpRequest, HttpResponse, PrefetchReceipt,
    PrefetchRequest, RequestId, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceFuture, ResourceKind,
    ResourceLocality, ResourceMetadata, ResourcePath, ResourceRequest, ResourceResponse,
    ResourceSource, ResourceStream, ResourceTiming, RetryAdvice,
};
use bobcat_core::script::{
    HostCallback, HostValue, ScriptEngine, ScriptEngineFactory, ScriptError, ScriptErrorKind,
    ScriptErrorPhase, ScriptSourceLocation,
};
use bobcat_core::{
    EngineEvent, FrameRequester, FrameSize, LynxView, PageConfig, Window, WindowTarget,
    configure_wasm_workers,
};
use http::HeaderMap;
use js_sys::{Array, Function, JsString, Object, Reflect};
use url::Url;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
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
#[derive(Debug, Default)]
struct BrowserResources {
    scripts: Mutex<HashMap<String, Arc<[u8]>>>,
}

impl BrowserResources {
    fn register_script(&self, url: &str, bytes: Vec<u8>) -> Result<String, JsValue> {
        let url = Url::parse(url)
            .map_err(|error| js_error(format!("the script URL `{url}` is invalid: {error}")))?;
        let normalized = url.to_string();
        self.scripts
            .lock()
            .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
            .insert(normalized.clone(), Arc::from(bytes));
        Ok(normalized)
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
        if !matches!(request.resource.kind, ResourceKind::ExternalJs) {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::UnsupportedKind,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "the browser source registry only contains external scripts",
            );
        }

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
                "the script locator is not a valid URL",
            );
        };
        let present = self
            .scripts
            .lock()
            .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
            .contains_key(url.as_str());
        if !present {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Resolve,
                Some(locator),
                "the Render Worker has not registered this script URL",
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

        let source = self
            .scripts
            .lock()
            .unwrap_or_else(|error| panic!("the browser resource map is poisoned: {error}"))
            .get(request.request.resource.url.as_str())
            .cloned();
        let Some(source) = source else {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::NotFound,
                ResourceErrorPhase::Open,
                Some(locator),
                "the registered script disappeared before it was loaded",
            );
        };
        let content_length = source.len() as u64;
        if content_length > request.max_bytes {
            return Self::error(
                Some(request_id),
                ResourceErrorKind::ResponseTooLarge,
                ResourceErrorPhase::ReadBody,
                Some(locator),
                "the registered script exceeds Bobcat's buffered-script limit",
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
                    media_type: Some(Arc::from("text/javascript; charset=utf-8")),
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

#[derive(Debug)]
struct BrowserScriptEngineFactory;

impl ScriptEngineFactory for BrowserScriptEngineFactory {
    fn create(&self) -> Result<Box<dyn ScriptEngine>, ScriptError> {
        Ok(Box::new(BrowserScriptEngine::default()))
    }
}

type HostDispatcher = Closure<dyn FnMut(Array) -> Array>;

#[derive(Default)]
struct BrowserScriptEngine {
    dispatchers: Vec<HostDispatcher>,
}

impl fmt::Debug for BrowserScriptEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrowserScriptEngine")
            .field("host_function_count", &self.dispatchers.len())
            .finish_non_exhaustive()
    }
}

impl ScriptEngine for BrowserScriptEngine {
    #[allow(
        clippy::too_many_lines,
        reason = "namespace setup, exact-arity wrapper creation, and callback retention form one atomic VM operation"
    )]
    fn register_host_function(
        &mut self,
        namespace: &str,
        name: &str,
        arity: u8,
        mut callback: HostCallback,
    ) -> Result<(), ScriptError> {
        let global = js_sys::global();
        let namespace_key = JsValue::from_str(namespace);
        let namespace_object = match Reflect::get(&global, &namespace_key)
            .map_err(|error| script_error_from_js(ScriptErrorPhase::RegisterHostFunction, &error))?
        {
            value if value.is_undefined() => {
                let object = Object::new();
                let installed =
                    Reflect::set(&global, &namespace_key, &object).map_err(|error| {
                        script_error_from_js(ScriptErrorPhase::RegisterHostFunction, &error)
                    })?;
                if !installed {
                    return Err(script_error(
                        ScriptErrorKind::EvaluationDenied,
                        ScriptErrorPhase::RegisterHostFunction,
                        format!("globalThis.{namespace} is not writable"),
                    ));
                }
                object.into()
            }
            value if value.is_object() => value,
            _ => {
                return Err(script_error(
                    ScriptErrorKind::InvalidBoundaryValue,
                    ScriptErrorPhase::RegisterHostFunction,
                    format!("globalThis.{namespace} exists but is not an object"),
                ));
            }
        };

        let dispatcher = Closure::wrap(Box::new(move |arguments: Array| -> Array {
            let response = Array::new();
            let result = arguments
                .iter()
                .map(|value| host_value_from_js(&value))
                .collect::<Result<Vec<_>, _>>()
                .and_then(|arguments| callback(&arguments))
                .and_then(host_value_to_js);
            match result {
                Ok(value) => {
                    response.push(&JsValue::TRUE);
                    response.push(&value);
                }
                Err(message) => {
                    response.push(&JsValue::FALSE);
                    response.push(&JsValue::from_str(&message));
                }
            }
            response
        }) as Box<dyn FnMut(Array) -> Array>);

        let parameters = (0..arity)
            .map(|index| format!("arg{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let body = format!(
            "\"use strict\"; let active = false; return function({parameters}) {{ \
             if (active) throw new Error('a Bobcat host function cannot be invoked re-entrantly'); \
             active = true; try {{ \
               const result = dispatch(Array.from(arguments)); \
               if (!result[0]) throw new Error(result[1]); \
               return result[1]; \
             }} finally {{ active = false; }} }};"
        );
        let function_constructor = Reflect::get(&global, &JsValue::from_str("Function"))
            .and_then(|value| {
                value
                    .dyn_into::<Function>()
                    .map_err(|_| JsValue::from_str("global Function is not callable"))
            })
            .map_err(|error| {
                script_error_from_js(ScriptErrorPhase::RegisterHostFunction, &error)
            })?;
        let constructor_arguments = Array::new();
        constructor_arguments.push(&JsValue::from_str("dispatch"));
        constructor_arguments.push(&JsValue::from_str(&body));
        let factory = Reflect::construct(&function_constructor, &constructor_arguments)
            .map_err(|error| script_error_from_js(ScriptErrorPhase::RegisterHostFunction, &error))?
            .dyn_into::<Function>()
            .map_err(|_| {
                script_error(
                    ScriptErrorKind::Other,
                    ScriptErrorPhase::RegisterHostFunction,
                    "the Function constructor did not return a function",
                )
            })?;
        let member = factory
            .call1(&JsValue::UNDEFINED, dispatcher.as_ref())
            .map_err(|error| {
                script_error_from_js(ScriptErrorPhase::RegisterHostFunction, &error)
            })?;
        let installed = Reflect::set(&namespace_object, &JsValue::from_str(name), &member)
            .map_err(|error| {
                script_error_from_js(ScriptErrorPhase::RegisterHostFunction, &error)
            })?;
        if !installed {
            return Err(script_error(
                ScriptErrorKind::EvaluationDenied,
                ScriptErrorPhase::RegisterHostFunction,
                format!("{namespace}.{name} is not writable"),
            ));
        }
        self.dispatchers.push(dispatcher);
        Ok(())
    }

    fn execute_script(&mut self, source: &str, source_name: &str) -> Result<(), ScriptError> {
        let source_name = source_name.replace(['\n', '\r'], "");
        let source = format!("{source}\n//# sourceURL={source_name}\n");
        js_sys::eval(&source).map(|_| ()).map_err(|error| {
            let mut error = script_error_from_js(ScriptErrorPhase::Execute, &error);
            match &mut error.location {
                Some(location) => location.source = Some(Arc::from(source_name)),
                None => {
                    error.location = Some(ScriptSourceLocation {
                        source: Some(Arc::from(source_name)),
                        line: None,
                        column: None,
                    });
                }
            }
            error
        })
    }

    fn collect_garbage(&mut self) -> Result<(), ScriptError> {
        // Browsers do not expose a synchronous GC hook. This synchronous
        // entry-script adapter therefore has no explicit collection operation.
        Ok(())
    }
}

/// A complete browser embedder, permanently owned by the explicit Render
/// Worker that constructs it. The document, element tree, VM, and engine stay
/// behind the opaque `LynxView` facade.
#[wasm_bindgen]
pub struct BobcatRenderer {
    view: LynxView<'static, BrowserWindow>,
    resources: Arc<BrowserResources>,
    canvas: OffscreenCanvas,
    frames: FrameSignal,
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
            let script_engine: Arc<dyn ScriptEngineFactory> = Arc::new(BrowserScriptEngineFactory);
            let config = PageConfig {
                default_display_linear,
                default_overflow_visible,
                enable_css_selector,
            };
            let mut view: LynxView<'static, BrowserWindow> = LynxView::new(
                config,
                resource_fetcher,
                script_engine,
                width,
                height,
                device_pixel_ratio,
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

    /// Start the registered main-thread script through `LynxView`'s resource
    /// boundary. The Render Worker polls completion independently from drawing.
    #[wasm_bindgen(js_name = executeScript)]
    pub async fn execute_script(&mut self, url: String) -> Result<(), JsValue> {
        self.ensure_running()?;
        self.view.execute_script(&url).await.map_err(js_error)
    }

    /// Forward the reserved URL-based stylesheet entry. Core intentionally
    /// reports this as unsupported until the stylesheet pipeline exists.
    #[wasm_bindgen(js_name = loadStyleSheet)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "wasm-bindgen owns JavaScript string arguments"
    )]
    pub fn load_style_sheet(&mut self, url: String) -> Result<(), JsValue> {
        self.ensure_running()?;
        self.view.load_style_sheet(&url).map_err(js_error)
    }

    /// Drain the script lifecycle channel independently of animation frames.
    #[wasm_bindgen(js_name = pollScript)]
    pub fn poll_script(&mut self) -> Result<bool, JsValue> {
        self.ensure_running()?;
        let mut finished = false;
        for event in self.view.pump() {
            match event {
                EngineEvent::ScriptFinished(Ok(())) => finished = true,
                EngineEvent::ScriptFinished(Err(error)) => return Err(js_error(error)),
                _ => {}
            }
        }
        Ok(finished)
    }

    /// Present a requested frame without exposing the engine or document to
    /// the browser host.
    #[wasm_bindgen(js_name = renderIfRequested)]
    pub fn render_if_requested(&mut self) -> Result<bool, JsValue> {
        self.ensure_running()?;
        if !self.frames.take() {
            return Ok(false);
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

fn host_value_from_js(value: &JsValue) -> Result<HostValue, String> {
    if value.is_undefined() {
        Ok(HostValue::Undefined)
    } else if value.is_null() {
        Ok(HostValue::Null)
    } else if let Some(value) = value.as_bool() {
        Ok(HostValue::Boolean(value))
    } else if let Some(value) = value.as_f64() {
        Ok(HostValue::Number(value))
    } else if value.is_string() {
        let string = value.unchecked_ref::<JsString>();
        if !string.is_valid_utf16() {
            return Err("ill-formed UTF-16 cannot cross Bobcat's host boundary".to_owned());
        }
        Ok(HostValue::String(Arc::from(
            value
                .as_string()
                .expect("a valid JavaScript string converts to Rust"),
        )))
    } else {
        Err("this JavaScript value cannot cross Bobcat's host boundary".to_owned())
    }
}

fn host_value_to_js(value: HostValue) -> Result<JsValue, String> {
    Ok(match value {
        HostValue::Undefined => JsValue::UNDEFINED,
        HostValue::Null => JsValue::NULL,
        HostValue::Boolean(value) => JsValue::from_bool(value),
        HostValue::Number(value) => JsValue::from_f64(value),
        HostValue::String(value) => JsValue::from_str(&value),
        _ => return Err("this Bobcat host value cannot cross into JavaScript".to_owned()),
    })
}

fn script_error_from_js(phase: ScriptErrorPhase, error: &JsValue) -> ScriptError {
    let name = Reflect::get(error, &JsValue::from_str("name"))
        .ok()
        .and_then(|value| value.as_string());
    let kind = match name.as_deref() {
        Some("SyntaxError") => ScriptErrorKind::Syntax,
        Some("EvalError") => ScriptErrorKind::EvaluationDenied,
        _ => ScriptErrorKind::Exception,
    };
    let line = numeric_error_property(error, "lineNumber");
    let column = numeric_error_property(error, "columnNumber");
    ScriptError {
        kind,
        phase,
        message: Arc::from(js_exception_message(error)),
        location: (line.is_some() || column.is_some()).then_some(ScriptSourceLocation {
            source: None,
            line,
            column,
        }),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the preceding bounds check makes the browser's line number fit u32"
)]
fn numeric_error_property(error: &JsValue, name: &str) -> Option<u32> {
    Reflect::get(error, &JsValue::from_str(name))
        .ok()
        .and_then(|value| value.as_f64())
        .filter(|value| value.is_finite() && *value >= 0.0 && *value <= f64::from(u32::MAX))
        .map(|value| value as u32)
}

fn script_error(
    kind: ScriptErrorKind,
    phase: ScriptErrorPhase,
    message: impl Into<Arc<str>>,
) -> ScriptError {
    ScriptError {
        kind,
        phase,
        message: message.into(),
        location: None,
    }
}

fn js_exception_message(error: &JsValue) -> String {
    if let Some(message) = error.as_string() {
        return message;
    }
    let name = Reflect::get(error, &JsValue::from_str("name"))
        .ok()
        .and_then(|value| value.as_string());
    let message = Reflect::get(error, &JsValue::from_str("message"))
        .ok()
        .and_then(|value| value.as_string());
    match (name, message) {
        (Some(name), Some(message)) if !message.is_empty() => format!("{name}: {message}"),
        (Some(name), _) => name,
        (_, Some(message)) => message,
        _ => "the browser JavaScript VM threw an opaque value".to_owned(),
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
