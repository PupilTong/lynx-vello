//! Lynx views and the groups they share a thread with: the embedder's
//! handles, and the vocabulary of the one thread boundary they cross.
//!
//! A view has two owners. The embedder's own thread — whichever one created
//! the [`LynxGroup`] — holds the view: it owns the host's resource system,
//! services it, and drains lifecycle events, all inside the calls the
//! embedder makes. The Lynx main thread owns each document and each script
//! realm, and belongs to the group rather than to any one view.
//!
//! Pixels are a third party. A [`Painter`](crate::Painter) is built
//! separately, on the same embedder thread, and observes a view's frames for
//! as long as it is attached to it; a view can outlive its painter and a
//! painter can outlive its view. The sibling `paint` and `main` modules hold
//! those two, and this one holds the handles that join them.

use std::cell::RefCell;
use std::fmt;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;

use terminus::link::clock::alignment_channel;
pub(crate) use terminus::link::painter::Viewport;
pub use terminus::link::painter::{DrawTarget, FrameSize, PainterError, Screenshot, WindowTarget};
use terminus::link::{FontBlob, FrameImages, ImageInbox, StylePool};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::background::{WorkerHome, WorkerKey};
use crate::clock::{ALIGNMENT_PATIENCE, ClockInstant};
use crate::link::{Published, ScrollMailbox, SourceAnswer, ToMain, ViewNotice, ViewSeat};
#[cfg(target_arch = "wasm32")]
pub use crate::main::configure_wasm_workers;
use crate::main::tree::PageConfig;
use crate::main::{GroupLink, spawn_group};
use crate::native_module::{NativeModule, NativeModuleTable};
use crate::performance::{
    EntryTypes, ExtraTiming, JsBlockingConfig, LongTaskConfig, MemoryConfig, MonitorTask,
    PerformanceEntry, PipelineRegistry, TimeOrigin,
};
use crate::resource::{ResourceFetcher, SourceCompletion, SourceRequest};
use crate::script::ScriptError;
use crate::threads::ThreadJoin;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenMetrics {
    pub pixel_ratio: f32,
    pub pixel_width: f32,
    pub pixel_height: f32,
}

impl ScreenMetrics {
    #[must_use]
    pub const fn for_viewport(width: f32, height: f32, device_pixel_ratio: f32) -> Self {
        Self {
            pixel_ratio: device_pixel_ratio,
            pixel_width: width * device_pixel_ratio,
            pixel_height: height * device_pixel_ratio,
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    #[error("the view is not ready")]
    NotReady,
    #[error("invalid viewport: {0}")]
    Viewport(String),
    #[error("GPU operation failed: {0}")]
    Gpu(String),
    #[error("rendering failed: {0}")]
    Render(String),
    #[error("could not start the {name} thread: {message}")]
    Thread { name: &'static str, message: String },
    #[error("no registered or system font family is named `{0}`")]
    UnknownFontFamily(String),
    #[error("`{0}` is not a BCP-47 language tag")]
    InvalidLocale(String),
    #[error("`{0}` is not a script whose font fallback can be preloaded")]
    UnknownScript(String),
    #[error("cannot resolve `{url}`: {message}")]
    InvalidUrl { url: String, message: String },
    #[error(
        "this painter presents into a window; `post_frame` and `draw_posted` advance an offscreen one"
    )]
    NotOffscreen,
    #[error("a painter is already attached")]
    PainterAttached,
    #[error("two native modules are named `{0}`")]
    DuplicateNativeModule(String),
    #[error("the ES module of native module `{0}` is not bytecode this build of the engine wrote")]
    InvalidNativeModuleEsm(String),
    #[cfg(feature = "devtools")]
    #[error("this view was created without `ViewSources::devtools`")]
    DevtoolsDisabled,
    #[cfg(feature = "devtools")]
    #[error("a devtools session is already attached to this view")]
    DevtoolsAttached,
}

impl From<PainterError> for EngineError {
    fn from(error: PainterError) -> Self {
        match error {
            PainterError::Viewport(message) => Self::Viewport(message),
            PainterError::Gpu(message) => Self::Gpu(message),
            PainterError::Render(message) => Self::Render(message),
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LynxViewError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Resource(#[from] crate::resource::ResourceError),
    #[error(transparent)]
    Script(#[from] ScriptError),
    #[error("script `{url}` is not valid UTF-8: {message}")]
    InvalidScriptEncoding { url: String, message: String },
    #[error("stylesheet `{url}` is not valid UTF-8: {message}")]
    InvalidStyleSheetEncoding { url: String, message: String },
}

#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    ScriptFinished,
    StartupFailed(LynxViewError),
    ScriptRunError(ScriptError),
    Panicked(ScriptError),
    ListenerFailed(ScriptError),
    TimerFailed(ScriptError),
    WorkerThrew {
        source: ScriptSource,
        error: ScriptError,
    },
    WorkerEnded {
        source: ScriptSource,
        error: ScriptError,
    },
    ScriptReported {
        source: ScriptSource,
        level: String,
        message: String,
    },
    TextInputStateChanged(Box<terminus::link::editing::TextInputState>),
    ConsoleMessage {
        source: ScriptSource,
        level: String,
        message: String,
    },
    Performance(PerformanceEntry),
}

impl EngineEvent {
    #[must_use]
    pub const fn is_fatal(&self) -> bool {
        matches!(self, Self::StartupFailed(_) | Self::Panicked(_))
    }

    pub(crate) const fn from_panic(error: ScriptError) -> Self {
        Self::Panicked(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ScriptSource {
    Main,
    Worker { id: WorkerId, url: Arc<str> },
}

impl fmt::Display for ScriptSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Main => formatter.write_str("main"),
            Self::Worker { id, url } => write!(formatter, "worker {id} {url}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkerId(u64);

impl From<WorkerKey> for WorkerId {
    fn from(key: WorkerKey) -> Self {
        Self(key.get())
    }
}

impl fmt::Display for WorkerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

pub trait EventRequester: Send + Sync + 'static {
    fn request_event(&self);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoWakeup;

impl EventRequester for NoWakeup {
    fn request_event(&self) {}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StyleThreads {
    #[default]
    Auto,
    Fixed(NonZeroUsize),
    Sequential,
}

impl StyleThreads {
    #[must_use]
    pub fn for_parallelism(available: usize) -> Self {
        StylePool::thread_count_for(available).map_or(Self::Sequential, Self::Fixed)
    }

    pub(crate) fn resolve(self) -> Option<NonZeroUsize> {
        match self {
            Self::Auto => StylePool::default_thread_count(),
            Self::Fixed(threads) => Some(threads),
            Self::Sequential => None,
        }
    }
}

#[derive(Debug)]
pub struct ViewSources {
    pub base_url: String,
    pub config: PageConfig,
    pub fonts: Vec<FontBlob>,
    pub default_font_family: Option<String>,
    pub fallback_fonts: Vec<FontBlob>,
    pub locale: String,
    pub style_sheets: Vec<String>,
    pub entry: String,
    pub card_url: Option<String>,
    pub background_entry: Option<String>,
    pub init_data: Option<String>,
    pub global_props: Option<String>,
    pub initial_processor: String,
    pub screen: ScreenMetrics,
    #[cfg(feature = "devtools")]
    pub devtools: bool,
}

impl ViewSources {
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        entry: impl Into<String>,
        screen: ScreenMetrics,
        locale: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            config: PageConfig::default(),
            fonts: Vec::new(),
            default_font_family: None,
            fallback_fonts: Vec::new(),
            locale: locale.into(),
            style_sheets: Vec::new(),
            entry: entry.into(),
            card_url: None,
            background_entry: None,
            init_data: None,
            global_props: None,
            initial_processor: String::new(),
            screen,
            #[cfg(feature = "devtools")]
            devtools: false,
        }
    }
}

pub struct LynxGroup {
    inner: Rc<GroupInner>,
}

struct GroupInner {
    attach: mpsc::UnboundedSender<GroupCommand>,
    #[expect(dead_code, reason = "held to wait for bobcat-main on drop")]
    home: ThreadJoin,
    #[expect(dead_code, reason = "held to end and wait for bobcat-workers on drop")]
    workers: WorkerHome,
}

impl fmt::Debug for LynxGroup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("LynxGroup").finish_non_exhaustive()
    }
}

impl LynxGroup {
    pub async fn new<R: EventRequester>(
        event_requester: Arc<R>,
        style_threads: StyleThreads,
    ) -> Result<Self, LynxViewError> {
        #[cfg(target_arch = "wasm32")]
        let alarm_clock = crate::alarm::start()?;
        #[cfg(target_arch = "wasm32")]
        {
            let requester = Arc::clone(&event_requester);
            terminus::link::render::canvas::set_waker(Arc::new(move || {
                requester.request_event();
            }));
        }
        let (workers_clock, workers_clock_end) = alignment_channel();
        let workers = WorkerHome::start(workers_clock_end)?;
        let (attach, attachments) = mpsc::unbounded_channel();
        let (ready, started) = oneshot::channel();
        let (main_clock, main_clock_end) = alignment_channel();
        let home = spawn_group(
            style_threads,
            GroupLink {
                attach: attachments,
                requester: event_requester,
                ready,
                workers: workers.commands(),
                workers_trapped: workers.trapped(),
                clock: main_clock_end,
            },
        )?;
        let group = Self {
            inner: Rc::new(GroupInner {
                attach,
                home,
                workers,
            }),
        };
        #[cfg(target_arch = "wasm32")]
        if let Some(answer) = alarm_clock {
            answer.await;
        }
        workers_clock.answer(ALIGNMENT_PATIENCE).await;
        main_clock.answer(ALIGNMENT_PATIENCE).await;
        match started.await {
            Ok(Ok(())) => Ok(group),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Err(EngineError::Thread {
                name: "script",
                message: "the Lynx main thread ended before it reported startup".to_owned(),
            }
            .into()),
        }
    }

    pub fn preload_text(&self, locale: &str, scripts: &[&str]) -> Result<(), LynxViewError> {
        if !terminus::link::TextContext::is_locale(locale) {
            return Err(EngineError::InvalidLocale(locale.to_owned()).into());
        }
        if let Some(script) = scripts
            .iter()
            .find(|script| !terminus::link::TextContext::is_script(script))
        {
            return Err(EngineError::UnknownScript((*script).to_owned()).into());
        }
        terminus::link::TextContext::preload_system_fonts();
        let preloaded = terminus::link::TextContext::preload_fallbacks(locale, scripts);
        debug_assert!(preloaded, "the locale and the scripts were checked above");
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one construction, read top to bottom"
    )]
    pub fn create_lynx_view<F, B>(
        &self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        resources: B,
        native_modules: Vec<Box<dyn NativeModule>>,
        mut sources: ViewSources,
    ) -> Result<LynxView<F>, LynxViewError>
    where
        F: ResourceFetcher + 'static,
        B: FnOnce(terminus::link::ImageReports) -> F,
    {
        let time_origin = TimeOrigin::now();
        let registry = Arc::new(PipelineRegistry::new(time_origin));
        let (table, native_esms) = read_native_modules(&native_modules)?;
        let base_url = Arc::new(resolve_startup_urls(&mut sources)?);
        #[cfg(feature = "devtools")]
        let devtools = sources.devtools.then(RefCell::default);
        let worker_fonts = Arc::new(WorkerFonts::of(&sources));
        let default_font_family = sources.default_font_family.take();
        let text_context = stage_text_context(
            std::mem::take(&mut sources.fonts),
            default_font_family.as_deref(),
            std::mem::take(&mut sources.fallback_fonts),
            &sources.locale,
        )?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let cancel = CancellationToken::new();
        let (commands, command_receiver) = mpsc::unbounded_channel();
        let (notices, notice_receiver) = mpsc::unbounded_channel();
        let (frames, frame_receiver) = watch::channel(Published::default());
        let (metrics, metric_receiver) = watch::channel(None);
        let scroll = Arc::new(ScrollMailbox::default());
        let canvases = Arc::new(crate::paint::CanvasPictures::default());
        let to_main = commands.downgrade();
        let (reports, inbox) = ImageInbox::new();
        let fetcher = Rc::new(resources(reports));
        let sheets: Vec<StartupSource> = std::mem::take(&mut sources.style_sheets)
            .into_iter()
            .map(|url| {
                let answer = request_startup_source(
                    &*fetcher,
                    &cancel,
                    SourceRequest::StyleSheet(url.clone()),
                );
                StartupSource { url, answer }
            })
            .collect();
        let entry_url = std::mem::take(&mut sources.entry);
        let entry = StartupSource {
            answer: request_startup_source(
                &*fetcher,
                &cancel,
                SourceRequest::Module(entry_url.clone()),
            ),
            url: entry_url,
        };
        self.inner
            .attach
            .send(GroupCommand::Attach(Box::new(ViewAttachment {
                viewport,
                sources,
                text_context,
                worker_fonts,
                startup: StartupSources { sheets, entry },
                native_modules: crate::native_module::encode_table(&table),
                native_esms: Arc::new(native_esms),
                commands: command_receiver,
                metrics: metric_receiver,
                scroll: Arc::clone(&scroll),
                notices,
                frames,
                cancel: cancel.clone(),
                fetch_probe: fetcher.fetch_probe(),
                style_sheet_probe: fetcher.style_sheet_probe(),
                base_url,
                registry: Arc::clone(&registry),
                canvases: Arc::clone(&canvases),
                to_main,
            })))
            .map_err(|_| EngineError::Thread {
                name: "script",
                message: "the group's Lynx main thread is gone".to_owned(),
            })?;
        registry.set_root_create_lynx(time_origin.instant().instant(), ClockInstant::now());
        Ok(LynxView {
            cancel,
            seat: Rc::new(ViewSeat {
                frame_demand: RefCell::new(crate::link::FrameDemand::new(Arc::clone(&registry))),
                commands: crate::link::CommandSender::new(commands),
                metrics,
                images: Rc::clone(&fetcher)
                    as Rc<dyn FrameImages<PlatformFrame = F::PlatformFrame>>,
                canvases,
                scroll,
                registry,
            }),
            notices: notice_receiver,
            frames: frame_receiver,
            inbox,
            fetcher,
            native_modules: native_modules
                .into_iter()
                .zip(table.into_iter().map(|module| module.methods))
                .collect(),
            state: ViewState::Loading,
            timeline_epoch: time_origin.instant().instant(),
            time_origin,
            performance_generation: 0,
            #[cfg(feature = "devtools")]
            devtools,
            group: Rc::clone(&self.inner),
        })
    }
}

fn read_native_modules(
    native_modules: &[Box<dyn NativeModule>],
) -> Result<(NativeModuleTable, crate::native_module::NativeModuleEsms), EngineError> {
    let mut table = NativeModuleTable::with_capacity(native_modules.len());
    let mut esms = Vec::new();
    for module in native_modules {
        let name = module.name();
        if table.iter().any(|existing| existing.name == name) {
            return Err(EngineError::DuplicateNativeModule(name.to_owned()));
        }
        let esm = module.esm();
        if let Some(esm) = &esm {
            if !quickjs_rust_bridge::is_framed_bytecode(&esm.bytecode) {
                return Err(EngineError::InvalidNativeModuleEsm(name.to_owned()));
            }
            esms.push((
                crate::native_module::native_module_esm_specifier(name),
                esm.bytecode.clone(),
            ));
        }
        table.push(crate::native_module::TableEntry {
            esm: crate::native_module::EsmLoad::of(esm.as_ref()),
            ..crate::native_module::TableEntry::new(name, module.methods())
        });
    }
    Ok((table, crate::native_module::NativeModuleEsms::new(esms)))
}

pub struct LynxView<F: ResourceFetcher> {
    cancel: CancellationToken,
    seat: Rc<ViewSeat<F::PlatformFrame>>,
    notices: mpsc::UnboundedReceiver<ViewNotice>,
    frames: watch::Receiver<Published>,
    inbox: ImageInbox,
    fetcher: Rc<F>,
    native_modules: Vec<(Box<dyn NativeModule>, Vec<String>)>,
    state: ViewState,
    timeline_epoch: ClockInstant,
    time_origin: TimeOrigin,
    performance_generation: u64,
    #[cfg(feature = "devtools")]
    devtools: Option<RefCell<std::sync::Weak<crate::devtools::Outlet>>>,
    #[expect(dead_code, reason = "held to keep the group's thread alive")]
    group: Rc<GroupInner>,
}

#[derive(Default)]
struct ImageRequests {
    images: Vec<Arc<str>>,
    documents: Vec<terminus::link::DocumentRequest>,
    playback: Vec<terminus::link::PlaybackRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewState {
    Loading,
    Ready,
    Failed,
}

impl<F: ResourceFetcher> LynxView<F> {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state == ViewState::Ready && !self.cancel.is_cancelled()
    }
}

impl<F: ResourceFetcher> fmt::Debug for LynxView<F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LynxView")
            .field("painter_attached", &(Rc::weak_count(&self.seat) > 0))
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl<F: ResourceFetcher> Drop for LynxView<F> {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.inbox.detach();
    }
}

impl<F: ResourceFetcher + 'static> LynxView<F> {
    pub fn reload(&self, data: String, processor_name: String) -> Result<(), EngineError> {
        self.send_page_update(crate::link::PageUpdate::Reload {
            data,
            processor_name,
        })
    }

    pub fn update_data(&self, data: String, processor_name: String) -> Result<(), EngineError> {
        self.send_page_update(crate::link::PageUpdate::Data {
            data,
            processor_name,
            reset: false,
        })
    }

    pub fn reset_data(&self, data: String, processor_name: String) -> Result<(), EngineError> {
        self.send_page_update(crate::link::PageUpdate::Data {
            data,
            processor_name,
            reset: true,
        })
    }

    pub fn update_global_props(&self, data: String) -> Result<(), EngineError> {
        self.send_page_update(crate::link::PageUpdate::GlobalProps(data))
    }

    pub fn send_global_event(
        &self,
        name: impl Into<String>,
        arguments: String,
    ) -> Result<(), EngineError> {
        self.send_page_update(crate::link::PageUpdate::GlobalEvent {
            name: name.into(),
            arguments,
        })
    }

    pub fn edit(&self, command: terminus::link::editing::Command) -> Result<(), EngineError> {
        if !self.is_ready() {
            return Err(EngineError::NotReady);
        }
        self.seat
            .commands
            .send(ToMain::Edit(command))
            .map_err(|_| EngineError::NotReady)
    }

    pub fn set_text_input_source(
        &self,
        source: crate::editing::TextInputSource,
    ) -> Result<(), EngineError> {
        if self.cancel.is_cancelled() {
            return Err(EngineError::NotReady);
        }
        self.seat
            .commands
            .send(ToMain::TextInputSource(source))
            .map_err(|_| EngineError::NotReady)
    }

    #[cfg(feature = "devtools")]
    pub fn devtools_attach(
        &self,
        sink: Arc<dyn crate::devtools::DevtoolsSink>,
    ) -> Result<crate::devtools::DevtoolsSession, EngineError> {
        let current = self
            .devtools
            .as_ref()
            .ok_or(EngineError::DevtoolsDisabled)?;
        if self.cancel.is_cancelled() {
            return Err(EngineError::NotReady);
        }
        if current
            .borrow()
            .upgrade()
            .is_some_and(|outlet| outlet.is_open())
        {
            return Err(EngineError::DevtoolsAttached);
        }
        let (session, attach) = crate::devtools::DevtoolsSession::new(sink);
        if let Err(refused) = self.seat.commands.send(ToMain::Devtools(attach)) {
            drop(session);
            drop(refused);
            return Err(EngineError::NotReady);
        }
        *current.borrow_mut() = session.outlet();
        Ok(session)
    }

    fn send_page_update(&self, update: crate::link::PageUpdate) -> Result<(), EngineError> {
        if !self.is_ready() {
            return Err(EngineError::NotReady);
        }
        self.seat
            .commands
            .send(ToMain::PageUpdate(update))
            .map_err(|_| EngineError::NotReady)
    }

    #[must_use]
    pub fn pump(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        let mut requests = ImageRequests::default();
        while let Ok(notice) = self.notices.try_recv() {
            match notice {
                ViewNotice::WorkerCreated { key, messages } => {
                    self.seat
                        .frame_demand
                        .borrow_mut()
                        .register_worker(key, messages);
                }
                ViewNotice::ScriptFrameDemand { worker, pending } => {
                    if self.state != ViewState::Failed && !self.cancel.is_cancelled() {
                        self.seat.frame_demand.borrow_mut().set(worker, pending);
                    }
                }
                ViewNotice::Engine(event) => {
                    if event.is_fatal() {
                        self.state = ViewState::Failed;
                        *self.seat.frame_demand.borrow_mut() =
                            crate::link::FrameDemand::new(Arc::clone(&self.seat.registry));
                        self.cancel.cancel();
                    }
                    if matches!(event, EngineEvent::ScriptFinished) {
                        self.state = ViewState::Ready;
                    }
                    events.push(event);
                }
                ViewNotice::PreloadSource(request) => {
                    if self.state != ViewState::Failed && !self.cancel.is_cancelled() {
                        self.fetcher.preload_source(request);
                    }
                }
                ViewNotice::TextInputState(state) => {
                    if self.state != ViewState::Failed && !self.cancel.is_cancelled() {
                        events.push(EngineEvent::TextInputStateChanged(state));
                    }
                }
                ViewNotice::RequestImages(sources) => requests.images.extend(sources),
                ViewNotice::RequestDocuments(documents) => requests.documents.extend(documents),
                ViewNotice::Playback(playback) => requests.playback.extend(playback),
                ViewNotice::RequestSource {
                    request,
                    completion,
                } => {
                    if self.state != ViewState::Failed && !completion.is_cancelled() {
                        self.fetcher.request_source(request, completion);
                    }
                }
                ViewNotice::NativeModuleCall {
                    caller,
                    call,
                    module,
                    method,
                    arguments,
                    callbacks,
                } => {
                    if self.state != ViewState::Failed && !self.cancel.is_cancelled() {
                        let reply = self
                            .seat
                            .frame_demand
                            .borrow()
                            .reply(caller, &self.seat.commands);
                        if let Some(reply) = reply {
                            let call = crate::native_module::ModuleCall::assemble(
                                call, method, arguments, &callbacks, &reply,
                            );
                            match self
                                .native_modules
                                .iter()
                                .find(|(candidate, _)| candidate.name() == module)
                            {
                                Some((native_module, methods))
                                    if methods.contains(&call.method) =>
                                {
                                    native_module.invoke(call);
                                }
                                _ => drop(call),
                            }
                        }
                    }
                }
            }
        }
        self.serve_images(requests);
        self.take_performance_entries(&mut events);
        let reports = self.inbox.drain();
        if self.state != ViewState::Failed && !reports.is_empty() {
            let _ = self.seat.commands.send(ToMain::ImageEvents(reports));
        }
        events
    }

    fn serve_images(&self, requests: ImageRequests) {
        if self.state == ViewState::Failed {
            return;
        }
        self.fetcher.service_images();
        for source in requests.images {
            self.fetcher.request_image(source.as_ref());
        }
        for (source, bytes, kind) in requests.documents {
            self.fetcher.request_document(&source, bytes, kind);
        }
        for request in requests.playback {
            self.fetcher.request_playback(request);
        }
    }

    fn take_performance_entries(&mut self, events: &mut Vec<EngineEvent>) {
        let registry = &self.seat.registry;
        let generation = registry.generation();
        if generation == self.performance_generation {
            return;
        }
        self.performance_generation = generation;
        let wakes = registry.take_wakes();
        if !wakes.is_empty() {
            let demand = self.seat.frame_demand.borrow();
            for key in wakes {
                demand.notify_performance(key);
            }
        }
        let frame_loads = registry.take_frame_loads();
        if self.state != ViewState::Failed {
            for (card, load) in frame_loads {
                let mut json = String::new();
                PerformanceEntry::LoadBundle(Box::new(load))
                    .write_lynx_json(&self.time_origin, &mut json);
                let _ = self
                    .seat
                    .commands
                    .send(ToMain::FrameLoadMetrics { card, json });
            }
            events.extend(
                registry
                    .take_completed_for_rust()
                    .into_iter()
                    .map(EngineEvent::Performance),
            );
        }
    }

    pub fn observe_performance(&self, types: EntryTypes) {
        self.seat.registry.observe_rust(types);
    }

    pub fn set_extra_timing(&self, timing: ExtraTiming) -> Result<(), EngineError> {
        if self.cancel.is_cancelled() {
            return Err(EngineError::NotReady);
        }
        self.seat.registry.set_extra_timing(&timing);
        Ok(())
    }

    pub fn set_long_task_monitor(&self, config: Option<LongTaskConfig>) {
        self.seat
            .registry
            .set_long_task_threshold(config.map(|config| config.threshold));
    }

    pub fn set_js_blocking_monitor(
        &self,
        config: Option<JsBlockingConfig>,
    ) -> Result<(), EngineError> {
        if self.cancel.is_cancelled() {
            return Err(EngineError::NotReady);
        }
        let now = crate::clock::ClockInstant::now();
        let epoch = self.seat.registry.set_js_blocking_monitor(config, now);
        if let (Some(epoch), Some(config)) = (epoch, config) {
            let _ =
                self.seat
                    .commands
                    .send(ToMain::StartMonitorTask(MonitorTask::JsBlockingReports {
                        epoch,
                        interval: config.report_interval,
                    }));
        }
        Ok(())
    }

    pub fn set_memory_monitor(&self, config: Option<MemoryConfig>) -> Result<(), EngineError> {
        if self.cancel.is_cancelled() {
            return Err(EngineError::NotReady);
        }
        let epoch = self.seat.registry.set_memory_monitor(config);
        if let (Some(epoch), Some(config)) = (epoch, config) {
            let _ = self
                .seat
                .commands
                .send(ToMain::StartMonitorTask(MonitorTask::MemorySamples {
                    epoch,
                    config,
                }));
        }
        Ok(())
    }

    #[must_use]
    pub const fn time_origin(&self) -> TimeOrigin {
        self.time_origin
    }

    pub fn prefetch_images<I, S>(&mut self, sources: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<Arc<str>>,
    {
        for source in sources {
            self.fetcher.request_image(source.into().as_ref());
        }
    }
}

impl<F: ResourceFetcher + 'static> LynxView<F> {
    pub(crate) fn frames(&self) -> watch::Receiver<Published> {
        self.frames.clone()
    }

    pub(crate) const fn seat(&self) -> &Rc<ViewSeat<F::PlatformFrame>> {
        &self.seat
    }

    pub(crate) const fn timeline_epoch(&self) -> ClockInstant {
        self.timeline_epoch
    }
}

#[cfg(test)]
impl<F: ResourceFetcher + 'static> LynxView<F> {
    pub(crate) const fn cancel(&self) -> &CancellationToken {
        &self.cancel
    }

    pub(crate) fn probe_document<T: Send + 'static>(
        &mut self,
        probe: impl FnOnce(&mut crate::main::tree::LynxDocument) -> T + Send + 'static,
    ) -> Option<T> {
        self.probe_document_within(std::time::Duration::from_secs(30), probe)
    }

    pub(crate) fn probe_document_within<T: Send + 'static>(
        &mut self,
        patience: std::time::Duration,
        probe: impl FnOnce(&mut crate::main::tree::LynxDocument) -> T + Send + 'static,
    ) -> Option<T> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let _ = self
            .seat
            .commands
            .send(ToMain::Probe(Box::new(move |document| {
                let _ = sender.send(probe(document));
            })));
        receiver.recv_timeout(patience).ok()
    }

    pub(crate) fn published_frame(&mut self) -> Option<Arc<terminus::link::CommittedFrame>> {
        self.frames.borrow_and_update().frame.clone()
    }
}

pub(crate) enum GroupCommand {
    Attach(Box<ViewAttachment>),
}

pub(crate) struct ViewAttachment {
    pub(crate) viewport: Viewport,
    pub(crate) sources: ViewSources,
    pub(crate) text_context: terminus::link::TextContext,
    pub(crate) worker_fonts: Arc<WorkerFonts>,
    pub(crate) startup: StartupSources,
    pub(crate) native_modules: String,
    pub(crate) native_esms: Arc<crate::native_module::NativeModuleEsms>,
    pub(crate) commands: mpsc::UnboundedReceiver<ToMain>,
    pub(crate) metrics: watch::Receiver<Option<Viewport>>,
    pub(crate) scroll: Arc<ScrollMailbox>,
    pub(crate) notices: mpsc::UnboundedSender<ViewNotice>,
    pub(crate) frames: watch::Sender<Published>,
    pub(crate) cancel: CancellationToken,
    pub(crate) fetch_probe: Option<crate::resource::FetchProbe>,
    pub(crate) style_sheet_probe: Option<crate::resource::StyleSheetProbe>,
    pub(crate) base_url: Arc<url::Url>,
    pub(crate) registry: Arc<PipelineRegistry>,
    pub(crate) canvases: Arc<crate::paint::CanvasPictures>,
    pub(crate) to_main: mpsc::WeakUnboundedSender<ToMain>,
}

pub(crate) struct StartupSources {
    pub(crate) sheets: Vec<StartupSource>,
    pub(crate) entry: StartupSource,
}

pub(crate) struct StartupSource {
    pub(crate) url: String,
    pub(crate) answer: SourceAnswer,
}

fn request_startup_source<F: ResourceFetcher>(
    fetcher: &F,
    cancel: &CancellationToken,
    request: SourceRequest,
) -> SourceAnswer {
    let (completion, answer) = SourceCompletion::new(cancel.clone());
    fetcher.request_source(request, completion);
    answer
}

pub(crate) fn resolve_startup_urls(sources: &mut ViewSources) -> Result<url::Url, EngineError> {
    let invalid = |url: &str, error: url::ParseError| EngineError::InvalidUrl {
        url: url.to_owned(),
        message: error.to_string(),
    };
    let base =
        url::Url::parse(&sources.base_url).map_err(|error| invalid(&sources.base_url, error))?;
    let resolve = |entry: &mut String| -> Result<(), EngineError> {
        *entry = base
            .join(entry)
            .map_err(|error| invalid(entry, error))?
            .into();
        Ok(())
    };
    resolve(&mut sources.entry)?;
    if let Some(background_entry) = &mut sources.background_entry {
        resolve(background_entry)?;
    }
    if let Some(card_url) = &mut sources.card_url {
        resolve(card_url)?;
    }
    Ok(base)
}

pub fn preload_system_fonts() {
    terminus::link::TextContext::preload_system_fonts();
}

pub(crate) fn stage_text_context(
    fonts: Vec<FontBlob>,
    default_font_family: Option<&str>,
    fallback_fonts: Vec<FontBlob>,
    locale: &str,
) -> Result<terminus::link::TextContext, LynxViewError> {
    let mut text = terminus::link::TextContext::new();
    for font in fonts {
        text.register_fonts(font);
    }
    for font in fallback_fonts {
        text.register_fallback_fonts(font);
    }
    if let Some(family) = default_font_family
        && !text.set_default_font_family(family)
    {
        return Err(EngineError::UnknownFontFamily(family.to_owned()).into());
    }
    if !text.set_locale(locale) {
        return Err(EngineError::InvalidLocale(locale.to_owned()).into());
    }
    Ok(text)
}

#[derive(Debug, Default)]
pub(crate) struct WorkerFonts {
    fonts: Vec<FontBlob>,
    default_font_family: Option<String>,
    fallback_fonts: Vec<FontBlob>,
    locale: String,
}

impl WorkerFonts {
    fn of(sources: &ViewSources) -> Self {
        Self {
            fonts: sources.fonts.clone(),
            default_font_family: sources.default_font_family.clone(),
            fallback_fonts: sources.fallback_fonts.clone(),
            locale: sources.locale.clone(),
        }
    }

    pub(crate) fn text_context(&self) -> terminus::link::TextContext {
        stage_text_context(
            self.fonts.clone(),
            self.default_font_family.as_deref(),
            self.fallback_fonts.clone(),
            &self.locale,
        )
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod performance_tests;
#[cfg(test)]
mod tests;
