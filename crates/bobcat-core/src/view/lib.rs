//! Lynx views and the groups they share a thread with: the embedder's
//! handles, and the vocabulary of the one thread boundary they cross.
//!
//! A view has two owners. The embedder's own thread — whichever one created
//! the [`LynxGroup`] — holds the view and, inside it, the private painter: it
//! captures input, creates the surface, routes, composes, presents, and
//! drains lifecycle events, all inside the calls the embedder makes. The Lynx
//! main thread owns each document and each script realm, and belongs to the
//! group rather than to any one view. The sibling `paint` and `main` modules
//! mirror those two owners; this module holds the handles that join them and
//! the link that crosses between them.

use std::fmt;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use dom::input::InputEvent;
use dom::{FontBlob, StylePool};
use tokio::sync::{mpsc, oneshot, watch};

use crate::background::WorkerHome;
use crate::link::{Published, ToMain, ViewCancel, ViewNotice};
#[cfg(target_arch = "wasm32")]
pub use crate::main::configure_wasm_workers;
use crate::main::tree::PageConfig;
use crate::main::{GroupHome, GroupLink, spawn_group};
pub use crate::paint::WindowTarget;
use crate::paint::{Output, Painter, PainterLink};
use crate::resource::ResourceFetcher;
use crate::script::ScriptError;

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
    /// The same computation [`LynxGroup::create_lynx_view`] and
    /// [`LynxView::resize`] make,
    /// exposed because a host that owns the surface's backing store — a
    /// browser canvas — has to size it before it hands the view a target.
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

/// Where a view's pixels go, named once and kept for the view's whole life.
///
/// There is no attaching a target later: [`LynxGroup::create_lynx_view`]
/// builds it, on the thread that will draw into it, before the view exists.
pub enum DrawTarget {
    /// A window's presentation stack, built from a `'static` surface target —
    /// a shared window handle or an owned canvas.
    Window(WindowTarget),
    /// A texture the view owns and nothing displays. [`LynxView::tick`]
    /// renders into it and [`LynxView::capture`] reads it back.
    ///
    /// Native only in practice: building one blocks the calling thread on a
    /// device request, and in a browser that thread is the one whose event
    /// loop would answer it — so a Wasm view is refused this target at
    /// construction rather than hanging on it.
    Offscreen,
    /// Nowhere at all. Test-only, so an in-crate test that exercises routing,
    /// events or timers pays for no GPU device; production has exactly the
    /// two targets an embedder can name.
    #[cfg(test)]
    None,
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
            #[cfg(test)]
            Self::None => "DrawTarget::None",
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
    #[error("this view presents into a window; `tick` advances an offscreen view")]
    NotOffscreen,
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
    /// The entry MTS module and Bobcat boot completed successfully.
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
    /// The painter could not produce a frame. Fatal for the draw target:
    /// nothing further will reach the screen, so an embedder reports it and
    /// takes the window down.
    RenderFailed(EngineError),
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
/// group: its views paint on the thread that created it, and so wake one
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
/// one: the host's fetcher belongs to the painter, is passed to
/// [`LynxGroup::create_lynx_view`] separately, and stays on that thread.
/// Construction splits this in two — the document inputs cross to
/// `bobcat-main`, including the specifiers it requests from the painter's
/// resource fetcher as startup proceeds.
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
    /// Initial page data. `None` becomes JavaScript `undefined`; JSON `null`
    /// remains `null`. Converted in the view's realm, but not yet passed to boot.
    /// Numbers use JavaScript `Number` semantics, so large integers can round.
    pub init_data: Option<serde_json::Value>,
    /// Initial global properties, converted like [`Self::init_data`].
    /// Installing these on `lynx` is not yet wired.
    pub global_props: Option<serde_json::Value>,
}

/// The document half of [`ViewSources`]: what crosses to `bobcat-main` for
/// this view in particular, as opposed to the group's script runtime and
/// style pool, which every document on that thread shares.
pub(crate) struct MainSources {
    pub(crate) config: PageConfig,
    pub(crate) fonts: Vec<FontBlob>,
    pub(crate) default_font_family: Option<String>,
    pub(crate) style_sheets: Vec<String>,
    pub(crate) entry: String,
    pub(crate) background_entry: Option<String>,
    pub(crate) init_data: Option<serde_json::Value>,
    pub(crate) global_props: Option<serde_json::Value>,
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
/// that creates a group is the thread every view in it paints on. That is
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
struct GroupInner {
    /// The group's inbox. `Option` only so the goodbye — dropping the last
    /// sender — can be said before the join below waits for it.
    attach: Option<mpsc::UnboundedSender<GroupCommand>>,
    home: GroupHome,
    /// The group's other thread, held here rather than by `bobcat-main`:
    /// `bobcat-workers` is a runtime of the group's own, and all `bobcat-main`
    /// is given of it is one sender.
    workers: WorkerHome,
}

impl Drop for GroupInner {
    fn drop(&mut self) {
        // Goodbye first, join second. Closing the inbox is what ends the
        // group's task, and this runs only once every view built from the
        // group has already been dropped, so there is nothing left on the
        // thread to end.
        drop(self.attach.take());
        self.home.join();
        // Main first, because its view tasks and its `WorkerFactory` hold
        // senders on the worker thread's channel. Once `bobcat-main` has
        // returned, the group's own sender is the last one, and
        // `WorkerHome::join` drops it and waits.
        self.workers.join();
    }
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
        // leaves this local to end the worker thread — `WorkerHome`'s own
        // `Drop` joins it — rather than a thread parked on a channel nobody
        // holds.
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
                attach: Some(attach),
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

    /// Builds the draw target and returns a loading view on the calling thread.
    ///
    /// The view's task requests each stylesheet in cascade order, then the entry
    /// module. Ordinary [`LynxView::pump`] turns dispatch requests to the fetcher,
    /// which resolves, loads and decodes them and completes the request it was
    /// handed, waking whichever task was awaiting that source. Boot
    /// completion is [`EngineEvent::ScriptFinished`], and loading, configuration,
    /// or boot failure is [`EngineEvent::StartupFailed`].
    ///
    /// Dropping the unresolved constructor releases its target and attachment.
    /// Dropping a loading view marks its source work cancelled and prevents boot
    /// from entering `QuickJS`. An IO operation or synchronous JavaScript already
    /// executing may finish; late source results are discarded. Other views continue.
    /// The fetcher needs neither `Send`, `Sync`, nor `'static`; only the concrete
    /// source completion and the fetcher's own job inputs leave this thread.
    ///
    /// # Errors
    ///
    /// [`LynxViewError`] if metrics are invalid, the draw target cannot be built,
    /// or the group's main thread cannot accept the attachment.
    pub async fn create_lynx_view<F, B>(
        &self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        target: DrawTarget,
        resources: B,
        sources: ViewSources,
    ) -> Result<LynxView<F>, LynxViewError>
    where
        F: ResourceFetcher,
        B: FnOnce(dom::ImageReports) -> F,
    {
        let frame_size = FrameSize::for_viewport(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        // Main owns source ordering; the painter owns the fetcher.
        let ViewSources {
            config,
            fonts,
            default_font_family,
            style_sheets,
            entry,
            background_entry,
            init_data,
            global_props,
        } = sources;
        // One view, one set of channels: nothing here is shared with a
        // sibling, so nothing has to be addressed or deferred.
        let cancel = ViewCancel::default();
        let (commands, command_receiver) = mpsc::unbounded_channel();
        let (notices, notice_receiver) = mpsc::unbounded_channel();
        let (frames, frame_receiver) = watch::channel(Published::default());
        let painter_link =
            PainterLink::new(commands, notice_receiver, frame_receiver, cancel.clone());
        self.inner
            .attach
            .as_ref()
            .expect("a group hands out attachments until it is dropped")
            .send(GroupCommand::Attach(Box::new(ViewAttachment {
                viewport,
                sources: MainSources {
                    config,
                    fonts,
                    default_font_family,
                    style_sheets,
                    entry,
                    background_entry,
                    init_data,
                    global_props,
                },
                commands: command_receiver,
                notices,
                frames,
                cancel: cancel.clone(),
            })))
            .map_err(|_| EngineError::Thread {
                name: "script",
                message: "the group's Lynx main thread is gone".to_owned(),
            })?;
        // The link goes into the guard before the first await, so every exit
        // path has a real goodbye to send — including the one where the draw
        // target failed and there is no painter yet.
        let mut startup = ViewStartup {
            link: Some(painter_link),
            painter: None,
            group: Some(Rc::clone(&self.inner)),
            cancel,
        };
        let output = Output::build(target, frame_size).await?;
        // The store is built here, on the thread that owns the painter and
        // always will, out of the sink it reports through — one sink, one
        // store, one view. Nothing about it ever crosses a thread, which is
        // why it needs neither `Send` nor `Sync`, and why it is a type rather
        // than a trait object.
        startup.painter = Some(Painter::with_output(
            viewport,
            frame_size,
            startup
                .link
                .take()
                .expect("the link is held until the painter is"),
            output,
            resources,
        ));
        Ok(startup.finish())
    }
}

/// A loading or running Lynx view: a window's worth of Lynx, on a thread its
/// [`LynxGroup`] owns.
///
/// The view stays on the thread that built it, and that thread is where it
/// paints: it owns its one draw target, the gesture router, the scroll
/// intents and the composition outright, so an embedder chooses the painting
/// thread by choosing where it creates the group. The target is chosen at
/// construction too, and never afterwards. Nothing here is a queue and
/// nothing here draws by itself — every call applies immediately, and the
/// frame those calls owe is produced by the next [`LynxView::pump`], which
/// is also the turn that hands back what the realm had to say. A host parked
/// on its own event loop therefore takes a turn after it hands a fact in;
/// facts from the Lynx main thread arrive with the construction-time
/// [`EventRequester`] wakeup.
pub struct LynxView<F> {
    painter: Painter<F>,
    /// The group whose thread carries this view. Held rather than read: it is
    /// what keeps that thread — and the runtime and pool on it — alive for as
    /// long as any view built from the group is, in whatever order the
    /// embedder drops them.
    ///
    /// **Last field, and it must stay last.** Fields drop in declaration
    /// order, so the painter — and with it this view's command sender, which
    /// is what ends its task — goes first, and the handle that joins the
    /// thread goes after there is nothing left on it.
    #[expect(dead_code, reason = "held to keep the group's thread alive")]
    group: Rc<GroupInner>,
}

impl<F> fmt::Debug for LynxView<F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LynxView")
            .field("painter", &self.painter)
            .finish_non_exhaustive()
    }
}

impl<F> Drop for LynxView<F> {
    fn drop(&mut self) {
        // Goodbye, and no join: the thread is the group's and carries the
        // group's other views. `bobcat-main` answers this by releasing this
        // view's document and realm and going on serving its siblings, and
        // the group is what joins the thread once the last of them is gone.
        // The draw target goes with the painter, in the drop glue that runs
        // the moment this returns — still on this thread, and still before
        // the embedder's next statement, which is what lets it drop the
        // window handle straight afterwards on a platform where only its own
        // thread may destroy one.
        self.painter.shutdown();
    }
}

impl<F: ResourceFetcher> LynxView<F> {
    /// Routes one normalized OS input event against the frame the painter
    /// last read.
    pub fn dispatch_input(&mut self, event: InputEvent) {
        self.painter.dispatch_input(event);
    }

    /// Applies new device metrics, if they moved at all.
    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), EngineError> {
        self.painter.resize(width, height, device_pixel_ratio)
    }

    /// Asks for a frame nothing else would have asked for.
    pub fn refresh(&self) {
        self.painter.refresh();
    }

    /// Reports whether the window is visible. An occluded one is not drawn,
    /// and the frame it owed is produced when it comes back.
    pub fn set_occluded(&mut self, occluded: bool) {
        self.painter.set_occluded(occluded);
    }

    /// Runs one turn — draw the frame the view owes, then hand back every
    /// lifecycle event the engine has produced since the last call.
    ///
    /// This is where a windowed view draws, so a host calls it at the point
    /// in its own turn where a wait for the display is acceptable, and once
    /// per turn.
    #[must_use]
    pub fn pump(&mut self) -> Vec<EngineEvent> {
        self.painter.serve()
    }

    /// Whether the view has a frame to put on its window.
    ///
    /// Read it at the end of a turn: while it holds, the host owes the view
    /// another [`Self::pump`] at its own next display frame — a
    /// `requestAnimationFrame`, a display link, whatever that host's display
    /// clock is. The engine names no interval, because it owns no clock: a
    /// running animation, a swap chain that had no image to give, and a
    /// frame a [`Self::refresh`] left owed are one answer here, and one
    /// answer is all a vsync-driven host needs.
    ///
    /// Only a visible window ever answers `true`; an offscreen view's frames
    /// are the host's to ask for through [`Self::tick`].
    #[must_use]
    pub fn owes_frame(&self) -> bool {
        self.painter.owes_frame()
    }

    /// How long a host may park before this view needs a turn of its own.
    ///
    /// Always `None`: nothing the engine owes itself is the host's to wait
    /// out any more. A realm's timers come due on `bobcat-main`, which waits
    /// them out itself and wakes this thread through its
    /// [`EventRequester`] like any other publication.
    #[must_use]
    pub const fn next_wakeup(&self) -> Option<Duration> {
        None
    }

    /// Whether the engine owed the timeline another frame as of the last
    /// turn.
    ///
    /// Narrower than [`Self::owes_frame`] and answered for any target: this
    /// is the animation itself, which an offscreen host — with no display to
    /// pace against and no window to owe — asks about directly.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.painter.is_animating()
    }

    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.painter.frame_size()
    }

    /// Advances an offscreen view by one frame, answering whether it drew.
    ///
    /// The one call that blocks this thread on `bobcat-main`, which is why
    /// only an offscreen view has it — and why a browser view, which cannot
    /// have an offscreen target at all, can never reach it.
    ///
    /// # Errors
    ///
    /// [`EngineError::NotOffscreen`] if this view presents into a window —
    /// its frames come from [`Self::pump`], on the host's own clock.
    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        self.painter.tick(force)
    }

    /// Reads back what the view last rendered.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        self.painter.capture()
    }

    /// Warms `sources` in the store, ahead of any paint walk meeting them.
    ///
    /// There is no matching "load and tell me when it is done": discovery is
    /// automatic. The paint walk reports every source it meets, the painter
    /// names it against the store, and the document relayouts when the pixels
    /// and their intrinsic size arrive. This only moves that work earlier.
    ///
    /// Applies immediately, like every other call here — the painter is this
    /// thread.
    pub fn prefetch_images<I, S>(&mut self, sources: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<Arc<str>>,
    {
        self.painter
            .prefetch_images(sources.into_iter().map(Into::into).collect());
    }
}

/// What an in-crate test reaches a live view through.
///
/// Every one of these is an observation the engine already makes somewhere;
/// none of them is a second way to drive a view. They are here rather than in
/// the harness because the painter is private to the view.
#[cfg(test)]
impl<F: ResourceFetcher> LynxView<F> {
    /// The painting half, so a test can pin the frame clock or read the
    /// gesture arena the way `pump` and `tick` do.
    pub(crate) const fn painter(&mut self) -> &mut Painter<F> {
        &mut self.painter
    }

    /// This view's cancellation flag, which is what a source completion the
    /// host still holds is answered against.
    pub(crate) const fn cancel(&self) -> &ViewCancel {
        self.painter.view_cancel()
    }

    /// Runs `probe` against the document on the thread that owns it,
    /// answering `None` if that thread never got to it.
    pub(crate) fn probe_document<T: Send + 'static>(
        &mut self,
        probe: impl FnOnce(&mut crate::main::tree::LynxDocument) -> T + Send + 'static,
    ) -> Option<T> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.painter.send(ToMain::Probe(Box::new(move |document| {
            let _ = sender.send(probe(document));
        })));
        receiver.recv_timeout(Duration::from_secs(30)).ok()
    }

    /// The newest committed frame this view has published.
    pub(crate) fn published_frame(&mut self) -> Option<Arc<dom::CommittedFrame>> {
        self.painter.published_frame()
    }
}

/// A half-built view whose destructor is the cancellation protocol for
/// [`LynxGroup::create_lynx_view`].
struct ViewStartup<F> {
    /// Held only until the painter exists, so a draw target that fails still
    /// leaves something able to say goodbye to `bobcat-main`.
    link: Option<PainterLink>,
    painter: Option<Painter<F>>,
    /// `None` once the view has been handed over. While it is `Some`, what
    /// this guards is a view that does not exist yet, and dropping one
    /// cancels it.
    group: Option<Rc<GroupInner>>,
    cancel: ViewCancel,
}

impl<F: ResourceFetcher> ViewStartup<F> {
    fn finish(mut self) -> LynxView<F> {
        LynxView {
            painter: self.painter.take().expect("startup owns the painter"),
            group: self.group.take().expect("startup owns the group handle"),
        }
    }
}

impl<F> Drop for ViewStartup<F> {
    fn drop(&mut self) {
        if self.group.take().is_none() {
            return;
        }
        // Cancellation first: the view's task checks the flag at every gate
        // between its sources, so a source that lands in the same instant
        // cannot carry its boot onward into QuickJS. It is this view's flag
        // alone — the group's other views go on booting.
        self.cancel.cancel();
        // Then the goodbye, which is dropping the command sender — either
        // the painter's or, if the draw target failed before one existed,
        // the bare link's. The fetcher sees cancellation before the painter
        // releases it; any completion still owned by an IO job discards its
        // late result.
        if let Some(painter) = self.painter.as_mut() {
            painter.shutdown();
        }
        drop(self.link.take());
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
    pub(crate) sources: MainSources,
    pub(crate) commands: mpsc::UnboundedReceiver<ToMain>,
    pub(crate) notices: mpsc::UnboundedSender<ViewNotice>,
    pub(crate) frames: watch::Sender<Published>,
    pub(crate) cancel: ViewCancel,
}

#[cfg(test)]
mod tests;
