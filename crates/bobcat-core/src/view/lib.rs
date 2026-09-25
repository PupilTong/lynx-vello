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

use dom::{FontBlob, FrameImages, ImageInbox, StylePool};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::background::WorkerHome;
use crate::clock::ClockInstant;
use crate::link::{Published, ScrollMailbox, SourceAnswer, ToMain, ViewNotice, ViewSeat};
#[cfg(target_arch = "wasm32")]
pub use crate::main::configure_wasm_workers;
use crate::main::tree::PageConfig;
use crate::main::{GroupLink, spawn_group};
use crate::native_module::{NativeModule, NativeModuleTable};
pub use crate::paint::WindowTarget;
use crate::resource::{ResourceFetcher, SourceCompletion, SourceRequest};
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

/// The screen metrics `SystemInfo` reports: `pixelRatio`, `pixelWidth`,
/// `pixelHeight`.
///
/// A screen, not a view. What Lynx calls `SystemInfo` describes the display
/// the page is shown on — natively the process-wide physical screen size,
/// and in web-core `devicePixelRatio` with `screen.availWidth`/`availHeight`
/// multiplied by it — so a view's own viewport is not an answer to it. The
/// embedder measures it and names it in
/// [`ViewSources::screen`](ViewSources::screen).
///
/// Read once, as the realm opens, and never updated afterwards — the same
/// standing web-core gives the values it reads at module load. A view resized
/// later, or a painter that binds at other metrics, changes nothing here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenMetrics {
    /// Physical pixels per CSS pixel, reported as `pixelRatio`.
    pub pixel_ratio: f32,
    /// The screen's width in physical pixels, reported as `pixelWidth`.
    pub pixel_width: f32,
    /// The screen's height in physical pixels, reported as `pixelHeight`.
    pub pixel_height: f32,
}

impl ScreenMetrics {
    /// The metrics of a viewport `width`×`height` CSS pixels at
    /// `device_pixel_ratio`, in physical pixels: `pixel_ratio` is the ratio
    /// and the two sizes are the CSS size multiplied by it.
    ///
    /// Not a screen, and not meant to be one. It is what a host that has no
    /// screen to measure — a headless or offscreen capture — reports, named
    /// explicitly at the call that builds its [`ViewSources`]: nothing in the
    /// engine derives a screen on a host's behalf.
    #[must_use]
    pub const fn for_viewport(width: f32, height: f32, device_pixel_ratio: f32) -> Self {
        Self {
            pixel_ratio: device_pixel_ratio,
            pixel_width: width * device_pixel_ratio,
            pixel_height: height * device_pixel_ratio,
        }
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
    /// The default family a view named is one neither its own font
    /// containers nor the platform provides. A construction failure rather
    /// than a startup one: [`LynxGroup::create_lynx_view`] validates the
    /// fonts before it hands the fetcher anything, so a view that fails this
    /// is never built and never fetches.
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
    /// Two of a view's native modules answer to one `NativeModules` key. The
    /// realm's object has one property per name, so there is no second module
    /// for a name to reach and nothing to arbitrate between them; naming the
    /// clash where the view is built is the whole of the resolution.
    #[error("two native modules are named `{0}`")]
    DuplicateNativeModule(String),
}

/// A view construction or startup failure. Construction reports target,
/// font, native-module and attachment errors directly; loading and boot
/// report through [`EngineEvent::StartupFailed`] on the returned view.
///
/// **A startup source that fails to load reports as `Script`.** The boot
/// module is what reads a view's stylesheets and its entry, so what reaches
/// the embedder is the exception that reading threw, carrying the URL and the
/// host's own reason in its message. The three source variants below are what
/// a *fetcher* answers a request with, which is where they are produced and
/// where they read as themselves.
#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LynxViewError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Resource(#[from] crate::resource::ResourceError),
    #[error(transparent)]
    Script(#[from] ScriptError),
    /// What a fetcher answers a source request with when the bytes are not
    /// UTF-8. A realm reads it as the text of the exception its own read
    /// threw, never as this variant.
    #[error("script `{url}` is not valid UTF-8: {message}")]
    InvalidScriptEncoding { url: String, message: String },
    /// The same for a stylesheet.
    #[error("stylesheet `{url}` is not valid UTF-8: {message}")]
    InvalidStyleSheetEncoding { url: String, message: String },
}

#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    /// MTS boot completed: the entry module evaluated — its top-level await
    /// settled — and its first flush committed. The BTS Worker plays no part
    /// in it: it may still be importing its entry, may have thrown (reported
    /// separately as [`EngineEvent::WorkerFailed`]), or may have ended.
    /// `LynxView::pump` records the view as ready before returning this
    /// notification.
    ScriptFinished,
    /// Source loading, document configuration, or entry boot failed. The
    /// view's fonts are not among them: an unknown default family is refused
    /// by [`LynxGroup::create_lynx_view`] itself, before any source is
    /// requested.
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
    /// A worker failed to load or threw, the BTS Worker included. The owning
    /// view remains usable.
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
///
/// Four of these fields are spent where the value is handed over rather than
/// crossing to `bobcat-main`: [`Self::fonts`] and
/// [`Self::default_font_family`] become the view's text context, and
/// [`Self::style_sheets`] and [`Self::entry`] become the requests
/// `create_lynx_view` hands the fetcher before it returns. What crosses of
/// them is the built context and the answers. The rest crosses as it stands,
/// and the view's task on `bobcat-main` stages the document inputs out of it.
#[derive(Debug)]
pub struct ViewSources {
    pub config: PageConfig,
    pub fonts: Vec<FontBlob>,
    pub default_font_family: Option<String>,
    pub style_sheets: Vec<String>,
    /// The MTS entry, as an absolute URL. The fetcher is asked for it by this
    /// string, and the realm's boot module imports it by the same string, so
    /// a name the module normalizer refuses — a bare `main.js` — fails the
    /// boot with that refusal. The fetcher may answer from another URL, which
    /// becomes the entry's `import.meta.url`.
    pub entry: String,
    /// Optional BTS application module specifier imported by `bobcat:bts`.
    /// The view always starts a BTS context; without this it runs only the
    /// built-in environment. Its imports load through the view's resource fetcher.
    /// Neither MTS evaluation nor [`EngineEvent::ScriptFinished`] waits for it:
    /// a host update accepted while the BTS entry is still importing is
    /// forwarded to the Worker, which queues it behind that import. An entry
    /// that throws is reported as [`EngineEvent::WorkerFailed`], like any
    /// worker script, and leaves the view and the BTS Worker running.
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
    /// Processor selected for initial data; empty selects the default.
    pub initial_processor: String,
    /// The screen this view's `SystemInfo` describes, as the embedder
    /// measured it: the web-core algorithm in a browser, the monitor the
    /// window is on natively.
    ///
    /// Required: every host names one. A host with no screen to measure — a
    /// headless or offscreen capture — names
    /// [`ScreenMetrics::for_viewport`] of its capture size. Read once as the
    /// realm opens and never updated.
    pub screen: ScreenMetrics,
}

impl ViewSources {
    /// The sources of a view over `entry`, reporting `screen` as its
    /// `SystemInfo`, with every other field at its default.
    #[must_use]
    pub fn new(entry: impl Into<String>, screen: ScreenMetrics) -> Self {
        Self {
            config: PageConfig::default(),
            fonts: Vec::new(),
            default_font_family: None,
            style_sheets: Vec::new(),
            entry: entry.into(),
            background_entry: None,
            init_data: None,
            global_props: None,
            initial_processor: String::new(),
            screen,
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
    /// `width`, `height` and `device_pixel_ratio` are the **create-time
    /// viewport**: the metrics this view's document is built at and works at
    /// — styling, layout and the encoding of its first frame — until a
    /// painter binds to it. They are not validated here, because no draw
    /// target is built from them; an attached painter's metrics, which
    /// [`Painter::new`](crate::Painter::new) and
    /// [`Painter::resize`](crate::Painter::resize) do validate, supersede
    /// them. If the two differ, the first frame is discarded and recomputed
    /// at the painter's size, so a create-time viewport equal to the
    /// painter's is the one that costs nothing.
    ///
    /// **No frame is published before a painter binds**, and the first
    /// `__FlushElementTree` waits for that binding — so a view no painter
    /// ever attaches to never reports
    /// [`EngineEvent::ScriptFinished`] and never becomes ready.
    ///
    /// **The view's startup sources are handed to the fetcher here**, on this
    /// thread and before this returns: each author stylesheet in cascade
    /// order, then the entry module. The fetcher is built in this same call,
    /// resolves and loads on whatever executor it owns, and answers the
    /// one-shot each request was minted with — so its IO and `bobcat-main`'s
    /// boot run while the embedder builds this view's painter, which is what
    /// it does next. Every *later* request rides an ordinary
    /// [`LynxView::pump`] turn instead: imports, adopted stylesheets, worker
    /// scripts, fonts and plain fetches. Boot completion is
    /// [`EngineEvent::ScriptFinished`], and loading, configuration, or boot
    /// failure is [`EngineEvent::StartupFailed`].
    ///
    /// Dropping a loading view cancels this view's own cancellation token,
    /// which marks its source work cancelled and prevents boot from entering
    /// `QuickJS`. An IO operation or synchronous JavaScript already
    /// executing may finish; late source results are discarded. Other views continue.
    /// The fetcher needs neither `Send` nor `Sync`; only the concrete source
    /// completion and the fetcher's own job inputs leave this thread.
    ///
    /// The embedder's [`NativeModule`]s are injected here, beside its
    /// fetcher and for the same reason: both are host capabilities that stay
    /// on this thread and are served inside [`LynxView::pump`]. Their names
    /// and methods are read once, here, and cross to the realm as data — so a
    /// module is never asked a question while script is running, and
    /// `NativeModules` carries exactly the methods declared at this call.
    ///
    /// # Errors
    ///
    /// [`LynxViewError`] if two native modules answer to one name, if the
    /// default font family is one neither this view's containers nor the
    /// platform provides ([`EngineError::UnknownFontFamily`]), or if the
    /// group's main thread cannot accept the attachment. Both of the first
    /// two are decided before the fetcher is built and before anything is
    /// requested, so a view that fails either one fetches nothing.
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
        B: FnOnce(dom::ImageReports) -> F,
    {
        // Read once, before anything is sent: the table is what crosses, and
        // the modules themselves stay here.
        let mut table = NativeModuleTable::with_capacity(native_modules.len());
        for module in &native_modules {
            let name = module.name();
            if table.iter().any(|(existing, _)| existing == name) {
                return Err(EngineError::DuplicateNativeModule(name.to_owned()).into());
            }
            table.push((name.to_owned(), module.methods()));
        }
        // The fonts next, and before the fetcher exists: a view whose
        // containers cannot serve the family it named will never render, and
        // the requests below go out in this same call, so a check made
        // anywhere later would already be behind them. It needs no document —
        // fonts and the default family are a `dom::TextContext`'s business,
        // and a document only ever adopts a finished one.
        let default_font_family = sources.default_font_family.take();
        let text_context = stage_text_context(
            std::mem::take(&mut sources.fonts),
            default_font_family.as_deref(),
        )?;
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
        // `None` until a painter attaches, which is what "no painter has bound
        // this view yet" is: the document works at the create-time viewport
        // until then, and its first `__FlushElementTree` parks on this.
        let (metrics, metric_receiver) = watch::channel(None);
        let scroll = Arc::new(ScrollMailbox::default());
        // The sink comes first and the store is built *from* it, so a store
        // without its report channel is unrepresentable and the two are paired
        // by construction. That pairing is per view: a host whose registry
        // outlives the view returns a per-view value holding a shared handle
        // on it, and that value — not the registry — is what carries the sink.
        // A load in flight when a view is replaced therefore reports to the
        // queue it was started for, which teardown has already detached,
        // rather than into its successor's document.
        //
        // Built before the attachment goes out, because the attachment
        // carries one thing out of it: the fetcher's `fetch_probe`, the only
        // part of a host's resource system that crosses to an engine thread.
        let (reports, inbox) = ImageInbox::new();
        let fetcher = Rc::new(resources(reports));
        // The startup sources, issued rather than waited for: every author
        // stylesheet in the order the view listed them, then the entry. Nothing here waits —
        // the fetcher takes each request and answers the one-shot minted with
        // it — so the load overlaps whatever this thread does next, which is
        // building this view's painter. Only the receivers cross; the fetcher
        // stays on this thread, as it must.
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
                SourceRequest::Entry(entry_url.clone()),
            ),
            url: entry_url,
        };
        self.inner
            .attach
            .send(GroupCommand::Attach(Box::new(ViewAttachment {
                viewport,
                // What is left of the view's sources: the fonts, the sheets
                // and the entry were spent above.
                sources,
                text_context,
                startup: StartupSources { sheets, entry },
                native_modules: crate::native_module::encode_table(&table),
                commands: command_receiver,
                metrics: metric_receiver,
                scroll: Arc::clone(&scroll),
                notices,
                frames,
                cancel: cancel.clone(),
                fetch_probe: fetcher.fetch_probe(),
            })))
            .map_err(|_| EngineError::Thread {
                name: "script",
                message: "the group's Lynx main thread is gone".to_owned(),
            })?;
        Ok(LynxView {
            cancel,
            // The seat a painter observes this view through: both halves of it
            // are this view's, and the view is the only holder of a strong
            // reference to it.
            seat: Rc::new(ViewSeat {
                frame_demand: RefCell::default(),
                commands: crate::link::CommandSender::new(commands),
                metrics,
                images: Rc::clone(&fetcher) as Rc<dyn FrameImages>,
                scroll,
            }),
            notices: notice_receiver,
            frames: frame_receiver,
            inbox,
            fetcher,
            native_modules,
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
    /// The embedder's native modules, held rather than read: the realm was
    /// told their names and methods at construction, and these are what serves
    /// a call under one of those names. Called only from [`Self::pump`], on
    /// this thread, which is why they need be neither `Send` nor `Sync`.
    native_modules: Vec<Box<dyn NativeModule>>,
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
    /// Whether pump has observed successful MTS boot — the entry module
    /// evaluated and its first flush committed — and this view has not ended.
    /// The BTS Worker's own state is not part of it. Keep calling pump while
    /// loading.
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
    /// Reload the page without fetching or evaluating its entry again. The
    /// framework recreates component state; global properties remain unchanged.
    /// The embedder serializes `data` as JSON; JavaScript parses it on delivery.
    /// An empty `processor_name` selects the default processor.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::NotReady`] until [`Self::pump`] reports
    /// [`EngineEvent::ScriptFinished`] — MTS boot, not the BTS Worker's — or
    /// after the view ends. No update is queued.
    pub fn reload(&self, data: String, processor_name: String) -> Result<(), EngineError> {
        self.send_page_update(crate::link::PageUpdate::Reload {
            data,
            processor_name,
        })
    }

    /// Merge data through the MTS update entry and BTS `updateCardData` hook.
    /// The embedder serializes `data` as JSON; JavaScript parses it on delivery.
    /// An empty `processor_name` selects the default processor.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::NotReady`] until [`Self::is_ready`] is true —
    /// MTS boot, not the BTS Worker's startup.
    /// Rejected updates are not queued.
    pub fn update_data(&self, data: String, processor_name: String) -> Result<(), EngineError> {
        self.send_page_update(crate::link::PageUpdate::Data {
            data,
            processor_name,
            reset: false,
        })
    }

    /// Replace page data using RESET semantics. The framework owns merging,
    /// notification and React rerendering. The embedder serializes `data` as
    /// JSON; JavaScript parses it on delivery. An empty `processor_name`
    /// selects the default processor.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::NotReady`] until [`Self::is_ready`] is true —
    /// MTS boot, not the BTS Worker's startup.
    /// Rejected resets are not queued.
    pub fn reset_data(&self, data: String, processor_name: String) -> Result<(), EngineError> {
        self.send_page_update(crate::link::PageUpdate::Data {
            data,
            processor_name,
            reset: true,
        })
    }

    /// Merge literal top-level global-property keys into host values, then
    /// notify BTS before updating MTS bindings and invoking its current hook.
    /// Script mutations do not alter the host values.
    /// The embedder serializes `data` as a JSON object; JavaScript parses it.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::NotReady`] until [`Self::is_ready`] is true —
    /// MTS boot, not the BTS Worker's startup.
    /// Supply initial properties through [`ViewSources::global_props`]; rejected
    /// updates are not retained as initial properties or queued for replay.
    pub fn update_global_props(&self, data: String) -> Result<(), EngineError> {
        self.send_page_update(crate::link::PageUpdate::GlobalProps(data))
    }

    /// Deliver a global event. The embedder serializes the listener argument
    /// list as a JSON array in `arguments`; JavaScript parses it on delivery.
    /// Call after pump returns `ScriptFinished`, or when `is_ready()` is true.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::NotReady`] before MTS boot is observed or after
    /// the view ends. Rejected events are not queued for later delivery.
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

    fn send_page_update(&self, update: crate::link::PageUpdate) -> Result<(), EngineError> {
        if !self.is_ready() {
            return Err(EngineError::NotReady);
        }
        self.seat
            .commands
            .send(ToMain::PageUpdate(update))
            .map_err(|_| EngineError::NotReady)
    }

    /// Runs one view turn: hand the host's resource system everything the
    /// document asked for, take back what it has finished, and hand back
    /// every lifecycle event the engine has produced since the last call.
    ///
    /// **This is the only call that advances the resource protocol**, past
    /// the startup sources [`LynxGroup::create_lynx_view`] handed the fetcher
    /// as it built this view. A painter observing this view asks the host for
    /// nothing — it draws what has already been published and reads pixels
    /// the fetcher already holds — so a host that wants an image to arrive
    /// takes this turn.
    #[must_use]
    pub fn pump(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        let mut image_requests = Vec::new();
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
                    // A fatal event ends the view the same way its release
                    // does, and by the same signal: the token this view was
                    // built with, which its own task is waiting on and every
                    // worker its realm created holds a child of.
                    if matches!(
                        event,
                        EngineEvent::StartupFailed(_) | EngineEvent::ScriptRunError(_)
                    ) {
                        self.state = ViewState::Failed;
                        *self.seat.frame_demand.borrow_mut() = crate::link::FrameDemand::default();
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
                // Assembled here, because here is where the handle a callback
                // answers through already is: `WorkerCreated` registered it,
                // and it precedes every call that worker makes on this one
                // FIFO — so a sender this turn cannot find is a worker that
                // has since gone, and there is nobody left to answer.
                //
                // A module nothing here is named for is no error either: the
                // realm's `NativeModules` object never carried that name, so
                // such a call can only come from a script importing the host
                // member directly, and what it registered is its own affair.
                // Either way no call is built and nothing is answered — there
                // is nobody left to answer, or nobody was ever asked.
                ViewNotice::NativeModuleCall {
                    worker,
                    call,
                    module,
                    method,
                    arguments,
                    callbacks,
                } => {
                    if self.state != ViewState::Failed
                        && !self.cancel.is_cancelled()
                        && let Some(native_module) = self
                            .native_modules
                            .iter()
                            .find(|candidate| candidate.name() == module)
                        && let Some(reply) = self.seat.frame_demand.borrow().sender(worker)
                    {
                        native_module.invoke(crate::native_module::ModuleCall::assemble(
                            call, method, arguments, &callbacks, &reply,
                        ));
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
    /// This view's fonts and default family, already registered and
    /// validated on the embedder's thread. `None` is a view that named
    /// neither, which leaves the document's own lazy context alone.
    pub(crate) text_context: Option<dom::TextContext>,
    /// The answers to the requests `create_lynx_view` already made.
    pub(crate) startup: StartupSources,
    /// The embedder's native modules as the realm hears about them: one
    /// `<utf16Length>:<text>` record of names and comma-joined method lists.
    /// The modules themselves stay on the view, on the embedder's thread.
    pub(crate) native_modules: String,
    pub(crate) commands: mpsc::UnboundedReceiver<ToMain>,
    /// The reading end of the seat's metrics watch: what an attached painter
    /// names, `None` until one does.
    pub(crate) metrics: watch::Receiver<Option<Viewport>>,
    /// Main's handle on the seat's scroll mailbox.
    pub(crate) scroll: Arc<ScrollMailbox>,
    pub(crate) notices: mpsc::UnboundedSender<ViewNotice>,
    pub(crate) frames: watch::Sender<Published>,
    /// The view's end signal, minted on the embedder's thread. The task that
    /// serves this view ends on it, and cancels it again on every exit.
    pub(crate) cancel: CancellationToken,
    /// The one part of the host's resource system that leaves the embedder's
    /// thread: the fetcher's answer to "already fetched?", which every realm
    /// of this view asks before making a fetch. `None` for a host that gave
    /// none.
    pub(crate) fetch_probe: Option<crate::resource::FetchProbe>,
}

/// The answers to the requests [`LynxGroup::create_lynx_view`] made on the
/// embedder's thread.
///
/// The entry is read by a task of the view's owner when its answer arrives,
/// which completes the module boot imports by the entry's URL. The sheets go
/// to the realm's document slot, and the first `__FlushElementTree` waits
/// for each and mounts it, in the order the view listed them, before the
/// document is styled: order of *use* rather than of completion, so the
/// cascade order between several sheets is the listed order whatever order
/// the fetcher answered them in.
pub(crate) struct StartupSources {
    /// One per author stylesheet, in the order the view listed them.
    pub(crate) sheets: Vec<StartupSource>,
    pub(crate) entry: StartupSource,
}

/// One startup source: the URL the view named it by, and the answer to the
/// request [`LynxGroup::create_lynx_view`] already made for it.
///
/// The URL travels beside the answer because it is what a failure is named
/// by: a sheet whose load failed makes `__FlushElementTree` throw a message
/// naming it, which fails boot's own flush, and an entry whose load failed
/// rejects boot's `import` with a message naming the URL.
pub(crate) struct StartupSource {
    pub(crate) url: String,
    pub(crate) answer: SourceAnswer,
}

/// Hands the fetcher one startup request and keeps the answer.
///
/// The token is the view's own, minted a few statements above: a host holding
/// one of these completions reads the embedder's release without waiting for
/// a turn, exactly as it does for a request that left through
/// `ViewNotice::RequestSource`.
fn request_startup_source<F: ResourceFetcher>(
    fetcher: &F,
    cancel: &CancellationToken,
    request: SourceRequest,
) -> SourceAnswer {
    let (completion, answer) = SourceCompletion::new(cancel.clone());
    fetcher.request_source(request, completion);
    answer
}

/// Registers a view's fonts and selects its default family, before any
/// document exists and before anything has been fetched.
///
/// Neither needs a document: fonts and the default family are a
/// [`TextContext`](dom::TextContext)'s business, and a document only ever
/// adopts a finished one. Running it here, in `create_lynx_view`, is what
/// keeps a family nothing provides a zero-fetch failure — the startup
/// requests go out in that same call, so there is no later point at which
/// this check would still be ahead of them.
///
/// `None` is a view that named neither, which leaves the document's own lazy
/// context alone. `Err` is a default family neither the containers nor the
/// platform has, which is a failure to build the view rather than to run it.
fn stage_text_context(
    fonts: Vec<FontBlob>,
    default_font_family: Option<&str>,
) -> Result<Option<dom::TextContext>, LynxViewError> {
    if fonts.is_empty() && default_font_family.is_none() {
        return Ok(None);
    }
    let mut text = dom::TextContext::new();
    for font in fonts {
        text.register_fonts(font);
    }
    if let Some(family) = default_font_family
        && !text.set_default_font_family(family)
    {
        return Err(EngineError::UnknownFontFamily(family.to_owned()).into());
    }
    Ok(Some(text))
}

#[cfg(test)]
mod tests;
