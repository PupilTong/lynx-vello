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

use std::fmt;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;

use dom::{FontBlob, FrameImages, ImageInbox, StylePool};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::background::WorkerHome;
use crate::clock::ClockInstant;
use crate::link::{Published, ToMain, ViewNotice, ViewSeat};
#[cfg(target_arch = "wasm32")]
pub use crate::main::configure_wasm_workers;
use crate::main::tree::PageConfig;
use crate::main::{GroupLink, spawn_group};
pub use crate::paint::WindowTarget;
use crate::resource::ResourceFetcher;
use crate::script::ScriptError;
use crate::threads::ThreadJoin;

/// View metrics, copied across the view's one thread boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Viewport {
    /// Viewport width in CSS pixels.
    pub width: f32,
    /// Viewport height in CSS pixels.
    pub height: f32,
    /// Physical pixels per CSS pixel.
    pub device_pixel_ratio: f32,
}

impl Viewport {
    /// Creates a viewport with a device-pixel ratio of 1.
    #[must_use]
    pub(crate) const fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio: 1.0,
        }
    }

    /// Returns this viewport with a new device-pixel ratio.
    #[must_use]
    pub(crate) const fn with_device_pixel_ratio(mut self, device_pixel_ratio: f32) -> Self {
        self.device_pixel_ratio = device_pixel_ratio;
        self
    }

    pub(crate) fn device(self) -> dom::Device {
        dom::Device::new(self.width, self.height, self.device_pixel_ratio)
    }
}

/// The physical pixel size of the render target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

impl FrameSize {
    /// The physical target a CSS viewport at this device scale needs.
    ///
    /// The same computation [`Painter::new`](crate::Painter::new) and
    /// [`Painter::resize`](crate::Painter::resize) make, exposed because a
    /// host that owns the surface's backing store — a browser canvas — has to
    /// size it before it hands a painter a target.
    ///
    /// # Errors
    ///
    /// [`EngineError::Viewport`] if the metrics are not finite and positive,
    /// or if the physical target would exceed 16384 pixels on either axis.
    pub fn for_viewport(
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, EngineError> {
        if !width.is_finite()
            || !height.is_finite()
            || !device_pixel_ratio.is_finite()
            || width <= 0.0
            || height <= 0.0
            || device_pixel_ratio <= 0.0
        {
            return Err(EngineError::Viewport(format!(
                "CSS size and device-pixel ratio must be finite and positive, got \
                 {width}\u{d7}{height} at {device_pixel_ratio}\u{d7}"
            )));
        }

        let physical_width = f64::from(width) * f64::from(device_pixel_ratio);
        let physical_height = f64::from(height) * f64::from(device_pixel_ratio);
        if physical_width > f64::from(MAX_RENDER_DIMENSION)
            || physical_height > f64::from(MAX_RENDER_DIMENSION)
        {
            return Err(EngineError::Viewport(format!(
                "the physical render target may not exceed \
                 {MAX_RENDER_DIMENSION}\u{d7}{MAX_RENDER_DIMENSION}, got \
                 {physical_width:.0}\u{d7}{physical_height:.0}"
            )));
        }

        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "finite positive values were bounded to 16384 immediately above"
        )]
        Ok(Self {
            width: physical_width.round().max(1.0) as u32,
            height: physical_height.round().max(1.0) as u32,
        })
    }
}

/// Where a painter's pixels go, named once and kept for its whole life.
///
/// There is no attaching a target later: [`Painter::new`](crate::Painter::new)
/// builds it, on the thread that will draw into it, before the painter
/// exists — and a painter then shows whichever views it attaches to through
/// that one target.
pub enum DrawTarget {
    /// A window's presentation stack, built from a `'static` surface target —
    /// a shared window handle or an owned canvas.
    Window(WindowTarget),
    /// A texture the painter owns and nothing displays.
    /// [`Painter::tick`](crate::Painter::tick) renders into it and
    /// [`Painter::capture`](crate::Painter::capture) reads it back.
    ///
    /// Native only in practice: building one blocks the calling thread on a
    /// device request, and in a browser that thread is the one whose event
    /// loop would answer it — so a Wasm painter is refused this target at
    /// construction rather than hanging on it.
    Offscreen,
}

impl DrawTarget {
    /// A window target, from anything a surface can be built out of.
    #[must_use]
    pub fn window(target: impl Into<WindowTarget>) -> Self {
        Self::Window(target.into())
    }
}

// Hand-written: a `SurfaceTarget` carries a window handle or a canvas, and
// neither is `Debug`. Which target this is, is the whole of what a formatter
// can honestly say.
impl fmt::Debug for DrawTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Window(_) => "DrawTarget::Window",
            Self::Offscreen => "DrawTarget::Offscreen",
        })
    }
}

const MAX_RENDER_DIMENSION: u32 = 16_384;

/// What a rendered target is identified by: the commit it came from, and
/// the scroll generation it was composed at.
///
/// Images need no term of their own. A load that changes what a frame draws
/// dirties the document, and every rebuild takes a new commit id.
pub(crate) type ComposeKey = (u64, u64);

#[derive(Debug, thiserror::Error)]
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
    #[error("this painter presents into a window; `tick` advances an offscreen one")]
    NotOffscreen,
    /// One view has at most one interactive painter, and one painter observes
    /// at most one *live* view. A link whose view has been released is not an
    /// attachment and never has to be detached by hand: attaching releases it.
    /// Two live pairings are what this refuses, and detaching whichever one is
    /// in the way is what clears it.
    #[error("a painter is already attached")]
    PainterAttached,
}

/// A view construction or startup failure. Construction reports target and
/// attachment errors directly; loading and boot report through
/// [`EngineEvent::StartupFailed`] on the returned view.
#[derive(Debug, thiserror::Error)]
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
    /// MTS boot and the configured BTS entry completed successfully.
    /// `LynxView::pump` records readiness before returning this notification.
    ScriptFinished,
    /// Source loading, document configuration, or entry boot failed.
    StartupFailed(LynxViewError),
    /// The script runtime failed fatally during owner-thread work after startup.
    /// Boot failures arrive as [`EngineEvent::StartupFailed`].
    ScriptRunError(ScriptError),
    /// A listener threw while an event was being delivered to it.
    ListenerFailed(ScriptError),
    /// A `setTimeout` or `setInterval` callback threw when it came due.
    /// Not fatal either: only the timer that threw is affected, a repeating
    /// one stays armed, and the realm goes on.
    TimerFailed(ScriptError),
    /// A worker failed to load or threw. The owning view remains usable.
    WorkerFailed(ScriptError),
    /// An application reported an error through `lynx.reportError`, or the
    /// runtime reported a recoverable operation failure.
    /// Reporting does not throw into its caller or end the view.
    ScriptReported { level: String, message: String },
    /// A realm's console output, delivered to the embedder that owns the view.
    ConsoleMessage { level: String, message: String },
}

/// One captured frame: tightly packed RGBA8 pixels at size.
#[derive(Clone)]
pub struct Screenshot {
    pub size: FrameSize,
    pub pixels: Vec<u8>,
}

impl fmt::Debug for Screenshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Screenshot")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Wakes the embedder's thread, which parks on an event loop the engine does
/// not own.
///
/// One implementation per platform — a winit event-loop proxy, an `AppKit`
/// source, a Worker's signal — and [`LynxGroup::new`] is generic over it, so
/// the wake is a direct call rather than a virtual one. One serves a whole
/// group: its views live on the thread that created it, and so wake one
/// event loop. The Lynx main thread holds the only handle to it, and calls it
/// whenever it has published something a view's next [`LynxView::pump`] would
/// find: a committed frame, a lifecycle event. It must never call back into a
/// view, which it could not anyway — a view never leaves the thread that
/// built it.
pub trait EventRequester: Send + Sync + 'static {
    fn request_event(&self);
}

/// The requester for a host with no event loop to wake: an offscreen view
/// driven by its own `tick`, a benchmark, a test.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoWakeup;

impl EventRequester for NoWakeup {
    fn request_event(&self) {}
}

/// How large a style pool one [`LynxGroup`]'s traversals get, `bobcat-main`
/// included: it is index zero of that pool, so a group starts one fewer
/// thread than the count says.
///
/// The threads are the group's, shared by every view in it and by no view
/// outside it. Two groups restyle at the same time; two views in one group
/// never do, because the single thread that drives them both is already
/// inside whichever traversal is running. A host multiplies workers by
/// groups rather than by views, which is why the count is a group's
/// construction-time choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StyleThreads {
    /// Stylo's own heuristic for this machine, under [`dom::MAX_STYLE_THREADS`].
    /// A machine whose parallelism the platform cannot answer for — Wasm,
    /// where the embedder passes it to [`StyleThreads::for_parallelism`]
    /// instead — resolves to [`StyleThreads::Sequential`].
    #[default]
    Auto,
    /// Exactly this many threads, `bobcat-main` included, so `Fixed(3)` is
    /// the group's `bobcat-main` and two more. More than [`dom::MAX_STYLE_THREADS`] is a
    /// construction error rather than a silent clamp: Stylo indexes its
    /// per-traversal thread-local storage by rayon thread index into an
    /// array that long.
    Fixed(NonZeroUsize),
    /// No pool: traversal runs on `bobcat-main` alone.
    Sequential,
}

impl StyleThreads {
    /// The policy a machine with this much parallelism gets, by the same
    /// heuristic and the same arithmetic [`StyleThreads::Auto`] applies to a
    /// machine that can answer for its own.
    ///
    /// For an embedder on a target where the standard library cannot answer —
    /// Wasm, which has `navigator.hardwareConcurrency` instead. Passing the
    /// raw number here rather than a count worked out by hand is what keeps a
    /// Wasm view and a native view on comparable hardware on the same pool.
    #[must_use]
    pub fn for_parallelism(available: usize) -> Self {
        StylePool::thread_count_for(available).map_or(Self::Sequential, Self::Fixed)
    }

    /// The thread count this policy resolves to on this machine, the flushing
    /// thread included.
    pub(crate) fn resolve(self) -> Option<NonZeroUsize> {
        match self {
            Self::Auto => StylePool::default_thread_count(),
            Self::Fixed(threads) => Some(threads),
            Self::Sequential => None,
        }
    }
}

/// Everything one view is built from.
///
/// Everything *shared* is the group's instead: the script runtime, the style
/// pool and the thread they live on are named once, at
/// [`LynxGroup::new`], and no field here could name them a second time.
///
/// It carries no resource system either, and has no field that could hold
/// one: the host's fetcher belongs to the view, is passed to
/// [`LynxGroup::create_lynx_view`] separately, and stays on that thread.
/// So every field here crosses to `bobcat-main`, and construction sends the
/// whole value: the view's task there stages the document inputs and requests
/// the specifiers from the view's resource fetcher as startup proceeds.
#[derive(Debug)]
pub struct ViewSources {
    pub config: PageConfig,
    pub fonts: Vec<FontBlob>,
    pub default_font_family: Option<String>,
    pub style_sheets: Vec<String>,
    pub entry: String,
    /// Optional BTS application module specifier imported by `bobcat:bts`.
    /// The view always starts a BTS context; without this it runs only the
    /// built-in environment. Application module loading is not implemented yet.
    pub background_entry: Option<String>,
    /// Initial page data, as JSON text. The engine hands it to the view's
    /// realm unread, as a plain string; `bobcat:runtime` parses it there and
    /// boot passes it to `processData`. `None` is `{}`, which is what web-core
    /// gives a page that was handed none, and text that is not JSON fails boot
    /// with [`EngineEvent::StartupFailed`].
    pub init_data: Option<String>,
    /// Initial global properties, as JSON text handed over like
    /// [`Self::init_data`]: the realm parses it into `lynx.__globalProps` and
    /// the entry's `__globalProps` before the entry loads.
    pub global_props: Option<String>,
}

impl ViewSources {
    #[must_use]
    pub fn new(entry: impl Into<String>) -> Self {
        Self {
            config: PageConfig::default(),
            fonts: Vec::new(),
            default_font_family: None,
            style_sheets: Vec::new(),
            entry: entry.into(),
            background_entry: None,
            init_data: None,
            global_props: None,
        }
    }
}

/// The views that share one Lynx main thread.
///
/// A group owns that thread, and with it the one `QuickJS` runtime every
/// view's realm is opened on and the one Stylo pool every view's document
/// restyles with. [`Self::create_lynx_view`] is the only way to build a
/// view, because naming the group is the only way to say which thread a
/// view runs on.
///
/// One group per thread, and one thread per group. The handle is `!Send` and
/// `!Sync` — it hands out `Rc`s of what it owns — so the embedder thread
/// that creates a group is the thread every view in it lives on. That is
/// also why one [`EventRequester`] serves the whole group rather than one
/// per view: its views wake one event loop, the one belonging to the thread
/// they were all created on.
///
/// Views in a group take turns rather than run at once. Every entry into a
/// realm is synchronous on that one thread, so a second view costs no second
/// heap, no second module graph and no second set of Stylo workers, at the
/// price of the two never restyling in parallel. What buys that is the
/// assumption that a person drives one view at a time; a host that needs two
/// pages genuinely parallel gives them a group each, on a thread each.
///
/// Dropping the group does not end its views: its threads are joined once the
/// group and the last view built from it are both gone.
pub struct LynxGroup {
    inner: Rc<GroupInner>,
}

/// What a group owns, and what its views hold it alive by.
///
/// **The field order is the teardown, and it must stay in this order.**
/// `bobcat-main` is waited for before the group's own worker sender closes,
/// because its view tasks and its `WorkerFactory` hold senders on the worker
/// thread's channel and it takes them with it.
struct GroupInner {
    attach: mpsc::UnboundedSender<GroupCommand>,
    #[expect(dead_code, reason = "held to wait for bobcat-main on drop")]
    home: ThreadJoin,
    /// The group's other thread, held here rather than by `bobcat-main`:
    /// `bobcat-workers` is a runtime of the group's own, and all `bobcat-main`
    /// is given of it is one sender.
    #[expect(dead_code, reason = "held to end and wait for bobcat-workers on drop")]
    workers: WorkerHome,
}

impl fmt::Debug for LynxGroup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("LynxGroup").finish_non_exhaustive()
    }
}

impl LynxGroup {
    /// Starts this group's two threads — `bobcat-main` and `bobcat-workers` —
    /// and waits until the script runtime and style pool `bobcat-main` shares
    /// out are up.
    ///
    /// All of it is ready before views attach. A thread or a style worker that
    /// cannot start fails the group here, rather than whichever view or
    /// `Worker` happened to ask for it first.
    ///
    /// # Errors
    ///
    /// [`LynxViewError::Engine`] if `bobcat-main`, `bobcat-workers` or a style
    /// worker will not start — asking for more workers than Stylo indexes is
    /// one such refusal — and [`LynxViewError::Script`] if the shared
    /// `QuickJS` runtime cannot be created.
    pub async fn new<R: EventRequester>(
        event_requester: Arc<R>,
        style_threads: StyleThreads,
    ) -> Result<Self, LynxViewError> {
        // The group's second runtime, started here beside `bobcat-main`
        // rather than by it: `bobcat-main` is handed one sender on it and
        // nothing else. First, because a `bobcat-main` that will not spawn
        // leaves this local to end the worker thread — its sender drops before
        // its own `ThreadJoin` — rather than a thread parked on a channel
        // nobody holds.
        let workers = WorkerHome::start()?;
        let (attach, attachments) = mpsc::unbounded_channel();
        let (ready, started) = oneshot::channel();
        let home = spawn_group(
            style_threads,
            GroupLink {
                attach: attachments,
                requester: event_requester,
                ready,
                workers: workers.commands(),
            },
        )?;
        // Into the handle before the first await, so every exit path from
        // here on closes both threads and joins them — including this one.
        let group = Self {
            inner: Rc::new(GroupInner {
                attach,
                home,
                workers,
            }),
        };
        match started.await {
            Ok(Ok(())) => Ok(group),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(EngineError::Thread {
                name: "script",
                message: "the Lynx main thread ended before it reported startup".to_owned(),
            }
            .into()),
        }
    }

    /// Returns a loading view on the calling thread.
    ///
    /// Synchronous, and it builds nothing that could block: the view is a set
    /// of channels, the host's resource system, and the group handle that
    /// keeps its thread alive. Where its pixels go is a separate question,
    /// answered by attaching a [`Painter`](crate::Painter) to it — which may
    /// be before boot, after it, or never.
    ///
    /// The view's task requests each stylesheet in cascade order, then the entry
    /// module. Ordinary [`LynxView::pump`] turns dispatch requests to the fetcher,
    /// which resolves, loads and decodes them and completes the request it was
    /// handed, waking whichever task was awaiting that source. Boot
    /// completion is [`EngineEvent::ScriptFinished`], and loading, configuration,
    /// or boot failure is [`EngineEvent::StartupFailed`].
    ///
    /// Dropping a loading view cancels this view's own cancellation token,
    /// which marks its source work cancelled and prevents boot from entering
    /// `QuickJS`. An IO operation or synchronous JavaScript already
    /// executing may finish; late source results are discarded. Other views continue.
    /// The fetcher needs neither `Send` nor `Sync`; only the concrete source
    /// completion and the fetcher's own job inputs leave this thread.
    ///
    /// # Errors
    ///
    /// [`LynxViewError`] if the metrics are invalid, or the group's main
    /// thread cannot accept the attachment.
    pub fn create_lynx_view<F, B>(
        &self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        resources: B,
        sources: ViewSources,
    ) -> Result<LynxView<F>, LynxViewError>
    where
        F: ResourceFetcher + 'static,
        B: FnOnce(dom::ImageReports) -> F,
    {
        // Validated here even though no target is built from it: these are the
        // metrics the document lays out against, and a painter that later
        // attaches imposes its own.
        FrameSize::for_viewport(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        // One view, one set of channels and one end signal: nothing here is
        // shared with a sibling, so nothing has to be addressed or deferred.
        // The token is minted here because the embedder's own thread is where
        // a release happens, and it is the parent of every token the view's
        // realm mints for a worker.
        let cancel = CancellationToken::new();
        let (commands, command_receiver) = mpsc::unbounded_channel();
        let (notices, notice_receiver) = mpsc::unbounded_channel();
        let (frames, frame_receiver) = watch::channel(Published::default());
        self.inner
            .attach
            .send(GroupCommand::Attach(Box::new(ViewAttachment {
                viewport,
                // Main owns source ordering; the view owns the fetcher.
                sources,
                commands: command_receiver,
                notices,
                frames,
                cancel: cancel.clone(),
            })))
            .map_err(|_| EngineError::Thread {
                name: "script",
                message: "the group's Lynx main thread is gone".to_owned(),
            })?;
        // The sink comes first and the store is built *from* it, so a store
        // without its report channel is unrepresentable and the two are paired
        // by construction. That pairing is per view: a host whose registry
        // outlives the view returns a per-view value holding a shared handle
        // on it, and that value — not the registry — is what carries the sink.
        // A load in flight when a view is replaced therefore reports to the
        // queue it was started for, which teardown has already detached,
        // rather than into its successor's document.
        let (reports, inbox) = ImageInbox::new();
        let fetcher = Rc::new(resources(reports));
        Ok(LynxView {
            cancel,
            // The seat a painter observes this view through: both halves of it
            // are this view's, and the view is the only holder of a strong
            // reference to it.
            seat: Rc::new(ViewSeat {
                commands,
                images: Rc::clone(&fetcher) as Rc<dyn FrameImages>,
            }),
            notices: notice_receiver,
            frames: frame_receiver,
            inbox,
            fetcher,
            state: ViewState::Loading,
            timeline_epoch: ClockInstant::now(),
            group: Rc::clone(&self.inner),
        })
    }
}

/// A loading or running Lynx view: a page's worth of Lynx, on a thread its
/// [`LynxGroup`] owns.
///
/// The view stays on the thread that built it. It owns the host's resource
/// system — sources and images both — and services it in [`Self::pump`], the
/// one call that advances the resource protocol at all. Nothing here is a
/// queue: every call applies immediately, and the facts the realm has to hand
/// back arrive on the [`EventRequester`] wakeup the group was built with.
///
/// It owns no pixels and no draw target. A [`Painter`](crate::Painter)
/// attached to it observes the frames it publishes and reads images out of
/// its fetcher; dropping the view leaves that painter showing the last frame
/// it drew, and dropping the painter leaves the view running with nothing
/// watching it.
pub struct LynxView<F> {
    /// This view's end signal, cancelled the instant it is released, before
    /// anything else is, so a host still holding one of its source completions
    /// sees it cancelled without waiting for a turn of its own. The view's
    /// task holds a clone, and every worker its realm creates holds a child of
    /// it.
    cancel: CancellationToken,
    /// What a painter observes this view through, and the goodbye with it:
    /// dropping this closes the view's task's inbox, which is what ends it —
    /// so it is declared before the fetcher whose completions that task may
    /// still be holding, and before the share of that fetcher the seat itself
    /// carries for the painter.
    ///
    /// The only strong reference there is. `Painter::attach` downgrades it,
    /// and a live weak handle on it is what "this view already has an
    /// interactive painter" means.
    seat: Rc<ViewSeat>,
    notices: mpsc::UnboundedReceiver<ViewNotice>,
    frames: watch::Receiver<Published>,
    /// Where the host's completed image loads land. Detached in `Drop` before
    /// the fetcher goes, so a loader still in flight finds it closed rather
    /// than queueing into a released view.
    inbox: ImageInbox,
    /// The host's whole resource system, as this view itself speaks to it. An
    /// `Rc` because the seat above carries a second, erased handle on it for
    /// an attached painter to read pixels through — so this view dropping
    /// both is what releases it.
    fetcher: Rc<F>,
    /// Updated by pump from boot/failure notices on the host thread.
    state: ViewState,
    /// When this view's document started, which is the epoch its animations
    /// are timed against. A painter adopts it at `attach`, so a painter that
    /// changes views does not restart the new one's timeline.
    timeline_epoch: ClockInstant,
    /// The group whose thread carries this view. Held rather than read: it is
    /// what keeps that thread — and the runtime and pool on it — alive for as
    /// long as any view built from the group is, in whatever order the
    /// embedder drops them.
    ///
    /// **Last field, and it must stay last.** Fields drop in declaration
    /// order, so this view's command sender — which is what ends its task —
    /// goes first, and the handle that joins the thread goes after there is
    /// nothing left on it.
    #[expect(dead_code, reason = "held to keep the group's thread alive")]
    group: Rc<GroupInner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewState {
    Loading,
    Ready,
    Failed,
}

impl<F> LynxView<F> {
    /// Whether pump has observed successful MTS boot and configured BTS entry
    /// completion, and this view has not ended. Keep calling pump while loading.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state == ViewState::Ready && !self.cancel.is_cancelled()
    }
}

impl<F> fmt::Debug for LynxView<F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LynxView")
            .field("painter_attached", &(Rc::weak_count(&self.seat) > 0))
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl<F> Drop for LynxView<F> {
    fn drop(&mut self) {
        // Cancellation first: a host still holding one of this view's source
        // completions must see it before the fetcher that holds it is
        // released. It is this view's token alone — the group's other views go
        // on booting. Cancelling before the command sender below drops is also
        // what gets a burst queued behind this release discarded: such a
        // command wakes the view's command consumer ahead of this cancel
        // waking its owner, and the consumer reads the token at that wake, so
        // what it finds there has to already say released.
        self.cancel.cancel();
        // Then the sink, before the store it reports into drops: a loader
        // still in flight must find it detached rather than queue into a
        // released view.
        self.inbox.detach();
        // The fields then drop in declaration order — the seat first, and
        // inside it the command sender whose closing is the goodbye, then the
        // fetcher whose other handle that seat carried. `bobcat-main`
        // answers that by releasing this view's document and realm and going
        // on serving its siblings, and the group is what joins the thread
        // once the last of them is gone.
    }
}

impl<F: ResourceFetcher + 'static> LynxView<F> {
    /// Deliver a native global event. `arguments` is the listener argument list.
    /// Call after pump returns `ScriptFinished`, or when `is_ready()` is true.
    ///
    /// # Errors
    ///
    /// Returns `EngineError::NotReady` before readiness is observed or after
    /// the view ends. Rejected events are not queued for later delivery.
    pub fn send_global_event(
        &self,
        name: impl Into<String>,
        arguments: Vec<serde_json::Value>,
    ) -> Result<(), EngineError> {
        if !self.is_ready() {
            return Err(EngineError::NotReady);
        }
        self.seat
            .commands
            .send(ToMain::PageUpdate(crate::link::PageUpdate::GlobalEvent {
                name: name.into(),
                arguments,
            }))
            .map_err(|_| EngineError::NotReady)
    }

    /// Runs one view turn: hand the host's resource system everything the
    /// document asked for, take back what it has finished, and hand back
    /// every lifecycle event the engine has produced since the last call.
    ///
    /// **This is the only call that advances the resource protocol.** A
    /// painter observing this view asks the host for nothing — it draws what
    /// has already been published and reads pixels the fetcher already holds
    /// — so a host that wants an image to arrive takes this turn.
    #[must_use]
    pub fn pump(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        let mut image_requests = Vec::new();
        while let Ok(notice) = self.notices.try_recv() {
            match notice {
                ViewNotice::Engine(event) => {
                    // A fatal event ends the view the same way its release
                    // does, and by the same signal: the token this view was
                    // built with, which its own task is waiting on and every
                    // worker its realm created holds a child of.
                    if matches!(
                        event,
                        EngineEvent::StartupFailed(_) | EngineEvent::ScriptRunError(_)
                    ) {
                        self.state = ViewState::Failed;
                        self.cancel.cancel();
                    }
                    if matches!(event, EngineEvent::ScriptFinished) {
                        self.state = ViewState::Ready;
                    }
                    events.push(event);
                }
                ViewNotice::RequestImages(sources) => image_requests.extend(sources),
                // A view that failed or was released asks its host for
                // nothing more: the completion is dropped instead, which is
                // what tells whoever was awaiting it that no source is coming.
                ViewNotice::RequestSource {
                    request,
                    completion,
                } => {
                    if self.state != ViewState::Failed && !completion.is_cancelled() {
                        self.fetcher.request_source(request, completion);
                    }
                }
            }
        }
        // The host's own moment in the turn comes before the sources this
        // turn discovered are named, so a load that finished between turns is
        // reported whether or not this turn asked for anything.
        //
        // A view that has failed asks its host for nothing at all, images
        // included: the document those pixels were for is finished with, and
        // the same rule already governs the source requests above.
        if self.state != ViewState::Failed {
            self.fetcher.service_images();
            for source in image_requests {
                self.fetcher.request_image(source.as_ref());
            }
        }
        // Drained either way, so what a host reported before the failure is
        // discarded here rather than left to accumulate behind a view that
        // will never commit again.
        let reports = self.inbox.drain();
        if self.state != ViewState::Failed && !reports.is_empty() {
            let _ = self.seat.commands.send(ToMain::ImageEvents(reports));
        }
        events
    }

    /// Warms `sources` in the store, ahead of any paint walk meeting them.
    ///
    /// There is no matching "load and tell me when it is done": discovery is
    /// automatic. The paint walk reports every source it meets, this view
    /// names it against the store, and the document relayouts when the pixels
    /// and their intrinsic size arrive. This only moves that work earlier.
    ///
    /// Applies immediately, like every other call here — the fetcher is on
    /// this thread.
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

/// What a painter reaches a view through. None of it is a second way to drive
/// a view: every one is something the view already publishes, handed over
/// without a copy.
impl<F: ResourceFetcher + 'static> LynxView<F> {
    /// A second receiver on this view's publication watch.
    pub(crate) fn frames(&self) -> watch::Receiver<Published> {
        self.frames.clone()
    }

    /// The seat a painter observes this view from: its command sender and its
    /// handle on the host's resource system, as one thing.
    ///
    /// The view owns the only strong reference, so the `Weak`
    /// [`Painter::attach`](crate::Painter::attach) takes of it can never keep
    /// a released view's task alive or hold its store open — and the weak
    /// count here is what says whether a painter has taken the seat.
    pub(crate) const fn seat(&self) -> &Rc<ViewSeat> {
        &self.seat
    }

    /// When this view's document started, which is the epoch its animations
    /// are timed against.
    pub(crate) const fn timeline_epoch(&self) -> ClockInstant {
        self.timeline_epoch
    }
}

/// What an in-crate test reaches a live view through.
///
/// Every one of these is an observation the engine already makes somewhere;
/// none of them is a second way to drive a view.
#[cfg(test)]
impl<F: ResourceFetcher + 'static> LynxView<F> {
    /// This view's end signal, which is what a source completion the host
    /// still holds is answered against.
    pub(crate) const fn cancel(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Runs `probe` against the document on the thread that owns it,
    /// answering `None` if that thread never got to it.
    pub(crate) fn probe_document<T: Send + 'static>(
        &mut self,
        probe: impl FnOnce(&mut crate::main::tree::LynxDocument) -> T + Send + 'static,
    ) -> Option<T> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let _ = self
            .seat
            .commands
            .send(ToMain::Probe(Box::new(move |document| {
                let _ = sender.send(probe(document));
            })));
        receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .ok()
    }

    /// The newest committed frame this view has published.
    pub(crate) fn published_frame(&mut self) -> Option<Arc<dom::CommittedFrame>> {
        self.frames.borrow_and_update().frame.clone()
    }
}

/// Everything a group's thread is ever told.
pub(crate) enum GroupCommand {
    /// Serve one more view on the group's runtime and style pool.
    Attach(Box<ViewAttachment>),
}

/// A view for a group's thread to adopt: everything that view needs which is
/// not already the group's, and both ends of the link it will speak through.
///
/// Nothing here is generic over the embedder's [`EventRequester`]. The one
/// part of a view's link that knows it is the requester itself, and that is
/// the group's — every view in a group paints on the thread that created the
/// group, and so wakes one event loop.
pub(crate) struct ViewAttachment {
    pub(crate) viewport: Viewport,
    pub(crate) sources: ViewSources,
    pub(crate) commands: mpsc::UnboundedReceiver<ToMain>,
    pub(crate) notices: mpsc::UnboundedSender<ViewNotice>,
    pub(crate) frames: watch::Sender<Published>,
    /// The view's end signal, minted on the embedder's thread. The task that
    /// serves this view ends on it, and cancels it again on every exit.
    pub(crate) cancel: CancellationToken,
}

#[cfg(test)]
mod tests;
