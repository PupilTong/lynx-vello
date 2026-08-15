//! Shared-memory browser composition exported through `wasm-bindgen`.

use std::cell::RefCell;
use std::fmt;
use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, Once};
use std::task::{Poll, Waker};
use std::time::Duration;

use bobcat_core::dom::{FontBlob, Node, StylesheetOrigin};
use bobcat_core::engine::{
    Engine, FrameRequester, FrameSize, MainThreadDocument, Window, WindowTarget,
};
use bobcat_core::tree::PageConfig;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, future_to_promise};
use web_sys::OffscreenCanvas;

const MAX_RENDER_DIMENSION: f64 = 16_384.0;
const MAX_STYLE_THREADS: u32 = 6;
const DOM_STARTUP_TIMEOUT_MS: f64 = 10_000.0;

thread_local! {
    /// Set only on a `bobcat-main` worker. Rust's panic hook still runs with
    /// `panic=abort`, so its owning Render Worker can stop accepting commands
    /// instead of polling a dead worker forever.
    static DOM_WORKER_FAILED: RefCell<Option<(Arc<AtomicBool>, Arc<ResponseSignal>)>> =
        const { RefCell::new(None) };
}

static INSTALL_PANIC_HOOK: Once = Once::new();

#[derive(Clone, Debug, Default)]
struct FrameSignal {
    requested: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
struct ResponseSignal {
    pending: std::sync::atomic::AtomicU32,
    waiters: Mutex<Vec<Waker>>,
}

impl ResponseSignal {
    fn reserve(&self) {
        let previous = self.pending.fetch_add(1, Ordering::AcqRel);
        assert_ne!(previous, u32::MAX, "too many pending DOM responses");
    }

    fn cancel(&self) {
        let previous = self.pending.fetch_sub(1, Ordering::AcqRel);
        assert!(
            previous > 0,
            "a DOM response reservation was cancelled twice"
        );
    }

    fn notify(&self) {
        let mut guard = self
            .waiters
            .lock()
            .unwrap_or_else(|error| panic!("the response signal is poisoned: {error}"));
        let waiters = std::mem::take(&mut *guard);
        drop(guard);
        for waiter in waiters {
            waiter.wake();
        }
    }

    fn take(&self) {
        let previous = self.pending.fetch_sub(1, Ordering::AcqRel);
        assert!(
            previous > 0,
            "a DOM response was taken without a reservation"
        );
    }

    async fn wait(self: Arc<Self>, failed: Arc<AtomicBool>) {
        poll_fn(move |context| {
            if self.pending.load(Ordering::Acquire) > 0 || failed.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            let mut waiters = self
                .waiters
                .lock()
                .unwrap_or_else(|error| panic!("the response signal is poisoned: {error}"));
            if self.pending.load(Ordering::Acquire) > 0 || failed.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            if !waiters
                .iter()
                .any(|waiter| waiter.will_wake(context.waker()))
            {
                waiters.push(context.waker().clone());
            }
            Poll::Pending
        })
        .await;
    }
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

/// Browser marker for an engine whose owned surface target lives in a Render
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

#[derive(Debug)]
struct DomCommand {
    request: u32,
    kind: DomCommandKind,
}

#[derive(Debug)]
enum DomMessage {
    Command(DomCommand),
    Shutdown,
}

#[derive(Debug)]
enum DomCommandKind {
    AddAuthorStylesheet(String),
    AppendElement { parent: u32, child: u32 },
    CreatePage,
    CreateView,
    DropElement(u32),
    Flush,
    RegisterFonts(Vec<u8>),
}

#[derive(Debug)]
struct DomResponse {
    request: u32,
    result: Result<DomValue, String>,
}

struct DomWorker {
    main_thread: MainThreadDocument,
    receiver: Receiver<DomMessage>,
    responses: Sender<DomResponse>,
    response_signal: Arc<ResponseSignal>,
    frames: FrameSignal,
    started: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    style_thread_count: usize,
    style_worker_url: String,
}

#[derive(Debug)]
enum DomValue {
    Node(u32),
    Unit,
}

/// A complete browser embedder, permanently owned by the explicit Render
/// Worker that constructs it.
///
/// The `Engine`, `OffscreenCanvas`, and every WebGPU object remain on that
/// Worker. Only the engine-created [`MainThreadDocument`] and Rust channel
/// endpoints cross into the `wasm_thread`-owned Lynx main Worker.
#[wasm_bindgen]
pub struct BobcatRenderer {
    engine: Engine<'static, BrowserWindow>,
    canvas: OffscreenCanvas,
    frames: FrameSignal,
    commands: Option<Sender<DomMessage>>,
    responses: Receiver<DomResponse>,
    response_signal: Arc<ResponseSignal>,
    dom_thread: Option<wasm_thread::JoinHandle<()>>,
    dom_failed: Arc<AtomicBool>,
}

impl fmt::Debug for BobcatRenderer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BobcatRenderer")
            .field("engine", &self.engine)
            .field("dom_running", &self.dom_thread.is_some())
            .finish_non_exhaustive()
    }
}

#[wasm_bindgen]
impl BobcatRenderer {
    /// Construct the entire browser embedder on the explicit Render Worker,
    /// attach its `OffscreenCanvas`, then move the unique DOM mutation owner to
    /// a permanent `wasm_thread` Worker.
    #[wasm_bindgen(js_name = create)]
    pub async fn create(
        canvas: OffscreenCanvas,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        dom_worker_url: String,
        style_thread_count: u32,
    ) -> Result<BobcatRenderer, JsValue> {
        validate_metrics(width, height, device_pixel_ratio)?;
        if dom_worker_url.is_empty() {
            return Err(js_error("the DOM worker URL must not be empty"));
        }
        if !(1..=MAX_STYLE_THREADS).contains(&style_thread_count) {
            return Err(js_error(format!(
                "the style thread count must be between 1 and {MAX_STYLE_THREADS}"
            )));
        }

        let mut engine: Engine<'static, BrowserWindow> =
            Engine::new(PageConfig::default(), width, height, device_pixel_ratio)
                .map_err(js_error)?;
        set_canvas_size(&canvas, engine.frame_size());

        let frames = FrameSignal::default();
        let target: WindowTarget<'static> = WindowTarget::OffscreenCanvas(canvas.clone());
        engine
            .attach_target(target, frames.clone(), engine.frame_size())
            .await
            .map_err(js_error)?;

        let main_thread = engine.take_main_thread_document().map_err(js_error)?;
        let (commands, command_receiver) = mpsc::channel();
        let (response_sender, responses) = mpsc::channel();
        let response_signal = Arc::new(ResponseSignal::default());
        let dom_started = Arc::new(AtomicBool::new(false));
        let dom_failed = Arc::new(AtomicBool::new(false));
        let worker_frames = frames.clone();
        let worker_responses = Arc::clone(&response_signal);
        let worker_started = Arc::clone(&dom_started);
        let worker_failed = Arc::clone(&dom_failed);
        let style_worker_url = dom_worker_url.clone();
        let dom_thread = wasm_thread::Builder::new()
            .name("bobcat-main".to_owned())
            .worker_script_url(dom_worker_url)
            .spawn(move || {
                run_dom_worker(DomWorker {
                    main_thread,
                    receiver: command_receiver,
                    responses: response_sender,
                    response_signal: worker_responses,
                    frames: worker_frames,
                    started: worker_started,
                    failed: worker_failed,
                    style_thread_count: style_thread_count as usize,
                    style_worker_url,
                });
            })
            .map_err(|error| js_error(format!("could not start bobcat-main: {error}")))?;
        wait_for_dom_start(&dom_thread, &dom_started, &dom_failed).await?;

        Ok(Self {
            engine,
            canvas,
            frames,
            commands: Some(commands),
            responses,
            response_signal,
            dom_thread: Some(dom_thread),
            dom_failed,
        })
    }

    /// Enqueue an author stylesheet mutation on the Lynx main Worker.
    #[wasm_bindgen(js_name = addAuthorStylesheet)]
    pub fn add_author_stylesheet(&self, request: u32, css: String) -> Result<(), JsValue> {
        self.enqueue(request, DomCommandKind::AddAuthorStylesheet(css))
    }

    /// Enqueue a DOM insertion on the Lynx main Worker.
    #[wasm_bindgen(js_name = appendElement)]
    pub fn append_element(&self, request: u32, parent: u32, child: u32) -> Result<(), JsValue> {
        self.enqueue(request, DomCommandKind::AppendElement { parent, child })
    }

    /// Resolve the permanent page element on the Lynx main Worker.
    #[wasm_bindgen(js_name = createPage)]
    pub fn create_page(&self, request: u32) -> Result<(), JsValue> {
        self.enqueue(request, DomCommandKind::CreatePage)
    }

    /// Create a Lynx `view` element on the Lynx main Worker.
    #[wasm_bindgen(js_name = createView)]
    pub fn create_view(&self, request: u32) -> Result<(), JsValue> {
        self.enqueue(request, DomCommandKind::CreateView)
    }

    /// Drop one detached or attached element on the Lynx main Worker.
    #[wasm_bindgen(js_name = dropElement)]
    pub fn drop_element(&self, request: u32, element: u32) -> Result<(), JsValue> {
        self.enqueue(request, DomCommandKind::DropElement(element))
    }

    /// Commit the open Element-PAPI batch and request presentation.
    #[wasm_bindgen(js_name = flushElementTree)]
    pub fn flush_element_tree(&self, request: u32) -> Result<(), JsValue> {
        self.enqueue(request, DomCommandKind::Flush)
    }

    /// Register shared font bytes on the Lynx main Worker.
    #[wasm_bindgen(js_name = registerFonts)]
    pub fn register_fonts(&self, request: u32, bytes: Vec<u8>) -> Result<(), JsValue> {
        self.enqueue(request, DomCommandKind::RegisterFonts(bytes))
    }

    /// Poll the next completed DOM request without blocking this presenting
    /// Worker. Returns `undefined` while no response is ready, otherwise
    /// `[request, ok, valueOrError]`.
    #[wasm_bindgen(js_name = pollResponse)]
    pub fn poll_response(&mut self) -> Result<JsValue, JsValue> {
        match self.responses.try_recv() {
            Ok(response) => {
                self.response_signal.take();
                Ok(response.into_js())
            }
            Err(TryRecvError::Empty) => {
                self.ensure_dom_running()?;
                Ok(JsValue::UNDEFINED)
            }
            Err(TryRecvError::Disconnected) => Err(js_error(
                "bobcat-main stopped before its response channel was drained",
            )),
        }
    }

    /// Resolve once at least one DOM response is ready. Unlike presentation,
    /// this control-plane wakeup is not tied to Worker animation frames, which
    /// browsers may pause for background documents.
    #[wasm_bindgen(js_name = waitForResponse)]
    pub fn wait_for_response(&self) -> js_sys::Promise {
        let signal = Arc::clone(&self.response_signal);
        let failed = Arc::clone(&self.dom_failed);
        future_to_promise(async move {
            signal.wait(failed).await;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// Present a requested frame without ever waiting for the DOM worker.
    #[wasm_bindgen(js_name = renderIfRequested)]
    pub fn render_if_requested(&mut self) -> Result<bool, JsValue> {
        if !self.frames.take() {
            return Ok(false);
        }
        self.engine.notify_redraw().map_err(js_error)?;
        Ok(true)
    }

    /// Apply device metrics and resize the worker-owned surface. `Engine`
    /// defers the document-side mutation itself if an Element-PAPI batch is
    /// currently open; there is no second resize command to the DOM Worker.
    #[wasm_bindgen(js_name = resize)]
    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), JsValue> {
        validate_metrics(width, height, device_pixel_ratio)?;
        self.engine
            .resize(width, height, device_pixel_ratio)
            .map_err(js_error)?;
        set_canvas_size(&self.canvas, self.engine.frame_size());
        Ok(())
    }

    /// Stop and join the permanent Lynx main Worker. This method is called on
    /// the dedicated Render Worker, never on the browser UI thread.
    #[wasm_bindgen(js_name = dispose)]
    pub fn dispose(&mut self) -> Result<(), JsValue> {
        self.shutdown_dom().map_err(js_error)
    }
}

impl BobcatRenderer {
    fn enqueue(&self, request: u32, kind: DomCommandKind) -> Result<(), JsValue> {
        if request == 0 {
            return Err(js_error("DOM request id zero is reserved"));
        }
        self.ensure_dom_running()?;
        self.commands
            .as_ref()
            .ok_or_else(|| js_error("the Bobcat renderer is disposed"))?
            .send(DomMessage::Command(DomCommand { request, kind }))
            .map_err(|_| js_error("bobcat-main stopped before receiving the command"))
    }

    fn ensure_dom_running(&self) -> Result<(), JsValue> {
        if self.dom_failed.load(Ordering::Acquire) {
            return Err(js_error(
                "bobcat-main trapped while executing a DOM command",
            ));
        }
        let thread = self
            .dom_thread
            .as_ref()
            .ok_or_else(|| js_error("the Bobcat renderer is disposed"))?;
        if thread.is_finished() {
            return Err(js_error("bobcat-main exited unexpectedly"));
        }
        Ok(())
    }

    fn shutdown_dom(&mut self) -> Result<(), String> {
        let Some(thread) = self.dom_thread.take() else {
            return Ok(());
        };
        let send_result = self
            .commands
            .take()
            .ok_or_else(|| "the DOM command sender was already disposed".to_owned())?
            .send(DomMessage::Shutdown);

        // A panic-abort Worker cannot complete wasm_thread's join packet. Wait
        // only while the thread is making observable progress, checking the
        // hook's shared failure flag before calling the blocking `join`.
        while !thread.is_finished() {
            if self.dom_failed.load(Ordering::Acquire) {
                drop(thread);
                return Err("bobcat-main trapped before shutdown".to_owned());
            }
            // This runs on a Worker, where Wasm atomic wait is legal. Keep the
            // native blocking model without burning a core while retaining the
            // panic-abort escape check that wasm_thread's unbounded join lacks.
            std::thread::sleep(Duration::from_millis(1));
        }

        let joined = thread
            .join()
            .map_err(|payload| panic_message(payload.as_ref()));
        if send_result.is_err() {
            return Err("bobcat-main exited before receiving shutdown".to_owned());
        }
        joined
    }
}

impl Drop for BobcatRenderer {
    fn drop(&mut self) {
        let _ = self.shutdown_dom();
    }
}

impl DomResponse {
    fn into_js(self) -> JsValue {
        let response = js_sys::Array::new();
        response.push(&JsValue::from(self.request));
        match self.result {
            Ok(DomValue::Unit) => {
                response.push(&JsValue::TRUE);
                response.push(&JsValue::NULL);
            }
            Ok(DomValue::Node(node)) => {
                response.push(&JsValue::TRUE);
                response.push(&JsValue::from(node));
            }
            Err(error) => {
                response.push(&JsValue::FALSE);
                response.push(&JsValue::from(error));
            }
        }
        response.into()
    }
}

fn run_dom_worker(worker: DomWorker) {
    let DomWorker {
        mut main_thread,
        receiver,
        responses,
        response_signal,
        frames,
        started,
        failed,
        style_thread_count,
        style_worker_url,
    } = worker;
    install_panic_hook();
    DOM_WORKER_FAILED.with(|slot| {
        *slot.borrow_mut() = Some((Arc::clone(&failed), Arc::clone(&response_signal)));
    });

    let style_pool = build_style_thread_pool(style_thread_count, style_worker_url)
        .unwrap_or_else(|error| panic!("could not start Stylo's Rayon pool: {error}"));
    bobcat_core::dom::install_style_thread_pool(style_pool)
        .unwrap_or_else(|_| panic!("Stylo's embedder thread pool was installed twice"));
    started.store(true, Ordering::Release);

    while let Ok(message) = receiver.recv() {
        let DomMessage::Command(command) = message else {
            break;
        };
        let response = DomResponse {
            request: command.request,
            result: execute_dom_command(&mut main_thread, &frames, command.kind),
        };
        response_signal.reserve();
        if responses.send(response).is_err() {
            response_signal.cancel();
            break;
        }
        response_signal.notify();
    }

    DOM_WORKER_FAILED.with(|slot| {
        slot.borrow_mut().take();
    });
}

fn build_style_thread_pool(
    thread_count: usize,
    worker_script_url: String,
) -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(thread_count)
        .use_current_thread()
        .thread_name(|index| format!("StyleThread#{index}"))
        .start_handler(|_| {
            bobcat_core::dom::stylo::thread_state::initialize_layout_worker_thread();
        })
        .stack_size(bobcat_core::dom::stylo::parallel::STYLE_THREAD_STACK_SIZE_KB * 1024)
        .spawn_handler(move |thread| {
            let mut builder =
                wasm_thread::Builder::new().worker_script_url(worker_script_url.clone());
            if let Some(name) = thread.name() {
                builder = builder.name(name.to_owned());
            }
            if let Some(stack_size) = thread.stack_size() {
                builder = builder.stack_size(stack_size);
            }
            builder.spawn(move || thread.run()).map(|_| ())
        })
        .build()
}

async fn wait_for_dom_start(
    thread: &wasm_thread::JoinHandle<()>,
    started: &AtomicBool,
    failed: &AtomicBool,
) -> Result<(), JsValue> {
    let deadline = js_sys::Date::now() + DOM_STARTUP_TIMEOUT_MS;
    loop {
        if started.load(Ordering::Acquire) {
            return Ok(());
        }
        if failed.load(Ordering::Acquire) {
            return Err(js_error("bobcat-main trapped during startup"));
        }
        if thread.is_finished() {
            return Err(js_error("bobcat-main exited during startup"));
        }
        if js_sys::Date::now() >= deadline {
            return Err(js_error(format!(
                "bobcat-main did not start within {DOM_STARTUP_TIMEOUT_MS:.0} ms"
            )));
        }
        worker_delay(1).await?;
    }
}

async fn worker_delay(milliseconds: i32) -> Result<(), JsValue> {
    let scope: web_sys::WorkerGlobalScope = js_sys::global().unchecked_into();
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        if let Err(error) =
            scope.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, milliseconds)
        {
            let _ = reject.call1(&JsValue::UNDEFINED, &error);
        }
    });
    JsFuture::from(promise).await.map(|_| ())
}

fn execute_dom_command(
    main_thread: &mut MainThreadDocument,
    frames: &FrameSignal,
    command: DomCommandKind,
) -> Result<DomValue, String> {
    match command {
        DomCommandKind::AddAuthorStylesheet(css) => {
            main_thread
                .document()
                .add_stylesheet(&css, StylesheetOrigin::Author);
            main_thread.flush();
            frames.request_frame();
            Ok(DomValue::Unit)
        }
        DomCommandKind::AppendElement { parent, child } => {
            let tree = main_thread.document();
            let parent_id = parent as usize;
            let child_id = child as usize;
            if !tree.get(parent_id).is_some_and(Node::is_element) {
                return Err(format!("append parent {parent} is not a live element"));
            }
            if tree.get(child_id).is_none() {
                return Err(format!("append child {child} is not a live node"));
            }
            if child_id == tree.root_node().id()
                || child_id == tree.document_element().id()
                || child_id == parent_id
                || tree.is_ancestor(child_id, parent_id)
            {
                return Err(format!(
                    "appending node {child} under {parent} would violate the document tree"
                ));
            }
            tree.insert_before(parent_id, child_id, None);
            Ok(DomValue::Node(child))
        }
        DomCommandKind::CreatePage => {
            let node = main_thread.document().document_element().id();
            Ok(DomValue::Node(u32::try_from(node).map_err(|error| {
                format!("page node id does not fit the browser protocol: {error}")
            })?))
        }
        DomCommandKind::CreateView => {
            let node = main_thread.document().create_element("view", ());
            Ok(DomValue::Node(u32::try_from(node).map_err(|error| {
                format!("view node id does not fit the browser protocol: {error}")
            })?))
        }
        DomCommandKind::DropElement(element) => {
            let tree = main_thread.document();
            let id = element as usize;
            if tree.get(id).is_none() {
                return Err(format!("drop target {element} is not a live node"));
            }
            if id == tree.root_node().id() || id == tree.document_element().id() {
                return Err(format!(
                    "the permanent root node {element} cannot be dropped"
                ));
            }
            tree.drop_element(id);
            Ok(DomValue::Unit)
        }
        DomCommandKind::Flush => {
            main_thread.flush();
            frames.request_frame();
            Ok(DomValue::Unit)
        }
        DomCommandKind::RegisterFonts(bytes) => {
            let registered = main_thread.document().register_fonts(FontBlob::new(bytes));
            main_thread.flush();
            if registered > 0 {
                frames.request_frame();
            }
            Ok(DomValue::Node(u32::try_from(registered).map_err(
                |error| format!("font count does not fit the browser protocol: {error}"),
            )?))
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

fn install_panic_hook() {
    INSTALL_PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            let _ = DOM_WORKER_FAILED.try_with(|slot| {
                if let Ok(slot) = slot.try_borrow()
                    && let Some((failed, responses)) = slot.as_ref()
                {
                    failed.store(true, Ordering::Release);
                    responses.notify();
                }
            });
            previous(panic);
        }));
    });
}

fn set_canvas_size(canvas: &OffscreenCanvas, size: FrameSize) {
    canvas.set_width(size.width);
    canvas.set_height(size.height);
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        format!("bobcat-main panicked: {message}")
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        format!("bobcat-main panicked: {message}")
    } else {
        "bobcat-main panicked with a non-string payload".to_owned()
    }
}

fn js_error(error: impl fmt::Display) -> JsValue {
    js_sys::Error::new(&error.to_string()).into()
}
