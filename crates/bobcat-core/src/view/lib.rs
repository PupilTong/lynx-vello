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

use std::cell::Cell;
use std::fmt;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};

use dom::input::InputEvent;
use dom::{CommittedFrame, FontBlob, NodeId, StylePool, Vector2D};

#[cfg(target_arch = "wasm32")]
pub use crate::main::configure_wasm_workers;
#[cfg(test)]
use crate::main::tree::LynxDocument;
use crate::main::tree::PageConfig;
use crate::main::{GroupHome, GroupLink, StartupControl, ToPainterSender, spawn_group};
pub use crate::paint::WindowTarget;
use crate::paint::{Output, Painter, PainterLink};
use crate::resource::ResourceFetcher;
use crate::script::ScriptError;
use crate::style::PreparsedStyleSheet;

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

/// A construction failure. No [`LynxView`] exists for any of these errors:
/// source acquisition, document configuration, and script boot all complete
/// on `bobcat-main` before the startup result is answered.
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
    /// The script runtime failed fatally during owner-thread work after startup.
    /// Boot failures are returned by [`LynxGroup::create_lynx_view`] instead.
    ScriptRunError(ScriptError),
    /// A listener threw while an event was being delivered to it.
    ListenerFailed(ScriptError),
    /// A `setTimeout` or `setInterval` callback threw when it came due.
    /// Not fatal either: only the timer that threw is affected, a repeating
    /// one stays armed, and the realm goes on.
    TimerFailed(ScriptError),
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
/// `bobcat-main`, while the specifiers stay with the painter that fetches
/// them, so that thread never holds a specifier and no fetch of its asking
/// is even constructible.
#[derive(Debug)]
pub struct ViewSources {
    pub config: PageConfig,
    pub fonts: Vec<FontBlob>,
    pub default_font_family: Option<String>,
    pub style_sheets: Vec<String>,
    pub entry: String,
}

/// The document half of [`ViewSources`]: what crosses to `bobcat-main` for
/// this view in particular, as opposed to the group's script runtime and
/// style pool, which every document on that thread shares.
pub(crate) struct MainSources {
    pub(crate) config: PageConfig,
    pub(crate) fonts: Vec<FontBlob>,
    pub(crate) default_font_family: Option<String>,
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
/// Views in a group take turns rather than run at once. The thread serves
/// one command round for one view at a time, so a second view costs no
/// second heap, no second module graph and no second set of Stylo workers,
/// at the price of the two never restyling in parallel. What buys that is
/// the assumption that a person drives one view at a time; a host that needs
/// two pages genuinely parallel gives them a group each, on a thread each.
///
/// Dropping the group does not end its views: the thread is joined once the
/// group and the last view built from it are both gone.
pub struct LynxGroup {
    inner: Rc<GroupInner>,
}

/// What a group owns, and what its views hold it alive by.
struct GroupInner {
    /// Every view's commands, and every attachment, on one FIFO.
    commands: flume::Sender<GroupCommand>,
    /// The next view's id. Ids are never reused, so a command still in
    /// flight for a view that has ended cannot find a later one wearing its
    /// name.
    next_view: Cell<u64>,
    home: GroupHome,
}

impl GroupInner {
    fn next_id(&self) -> ViewId {
        let id = self.next_view.get();
        self.next_view.set(id + 1);
        ViewId(id)
    }
}

impl Drop for GroupInner {
    fn drop(&mut self) {
        // Goodbye first, join second, exactly as a view's shutdown is. The
        // group holds a sender on the FIFO `bobcat-main` parks on, so a close
        // that is not sent is a close that never arrives — and this runs only
        // once every view built from the group has already been dropped, so
        // there is nothing left on the thread to end.
        let _ = self.commands.send(GroupCommand::Close);
        self.home.join();
    }
}

impl fmt::Debug for LynxGroup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("LynxGroup").finish_non_exhaustive()
    }
}

impl LynxGroup {
    /// Starts this group's `bobcat-main` and waits until the script runtime
    /// and style pool it shares are up.
    ///
    /// Both are built before any view exists, which is what lets a view's own
    /// construction overlap them — and what makes workers that cannot start a
    /// failure to build the *group*, named here, rather than a failure of
    /// whichever view happened to be first.
    ///
    /// # Errors
    ///
    /// [`LynxViewError::Engine`] if `bobcat-main` or a style worker will not
    /// start — asking for more workers than Stylo indexes is one such
    /// refusal — and [`LynxViewError::Script`] if the shared `QuickJS`
    /// runtime cannot be created.
    pub async fn new<R: EventRequester>(
        event_requester: Arc<R>,
        style_threads: StyleThreads,
    ) -> Result<Self, LynxViewError> {
        let (commands, command_receiver) = flume::unbounded();
        let (ready, started) = flume::bounded(1);
        let home = spawn_group(
            style_threads,
            GroupLink {
                commands: command_receiver,
                requester: event_requester,
                ready,
            },
        )?;
        // Into the handle before the first await, so every exit path from
        // here on closes the thread and joins it — including this one.
        let group = Self {
            inner: Rc::new(GroupInner {
                commands,
                next_view: Cell::new(0),
                home,
            }),
        };
        match started.recv_async().await {
            Ok(Ok(())) => Ok(group),
            Ok(Err(error)) => Err(error),
            Err(flume::RecvError::Disconnected) => Err(EngineError::Thread {
                name: "script",
                message: "the Lynx main thread ended before it reported startup".to_owned(),
            }
            .into()),
        }
    }

    /// Builds one view on this group's thread, and waits asynchronously until
    /// that thread has created its document, loaded and mounted every source,
    /// and booted the entry module.
    ///
    /// The target is an argument rather than something attached afterwards: a
    /// view that exists has somewhere to put a frame, so nothing has to
    /// describe — or handle — a view that has run but cannot draw. Its GPU
    /// objects are built while `bobcat-main` is already fetching, and the
    /// thread that builds them is the thread that owns them, which on macOS
    /// is the only thread allowed to create a surface at all.
    ///
    /// Dropping this future before it resolves cancels pending resource work
    /// or stops this view's boot before `QuickJS` begins, releases the
    /// target, and takes the half-built view off the group's thread — leaving
    /// the group, and every other view on it, running. If synchronous startup
    /// JavaScript is already executing, cancellation takes effect when that
    /// call returns; nothing interrupts a realm mid-call.
    ///
    /// # Errors
    ///
    /// [`LynxViewError`] if the draw target cannot be built, a source cannot
    /// be fetched or decoded, the document refuses one, or the entry module
    /// fails to boot.
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
        // The split: document inputs cross to `bobcat-main`, specifiers stay
        // with the thread that owns the fetcher. Neither side ever asks the
        // other for what it already holds.
        let ViewSources {
            config,
            fonts,
            default_font_family,
            style_sheets,
            entry,
        } = sources;
        let view = self.inner.next_id();
        let control = Arc::new(StartupControl::default());
        let (painter_link, notifications, frames) = view_link(view, &self.inner.commands);
        // The attachment goes first and the sources follow it on the same
        // FIFO, so the thread has this view's document before the first
        // source it must mount on one.
        self.inner
            .commands
            .send(GroupCommand::Attach(Box::new(Attachment {
                view,
                viewport,
                sources: MainSources {
                    config,
                    fonts,
                    default_font_family,
                },
                notifications,
                frames,
                control: Arc::clone(&control),
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
            control,
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
        // Pushing the sources *is* the wait for this view's startup message:
        // one loop over one inbox, so there is no arm to forget and no second
        // thing to wait on.
        startup.serve(style_sheets, entry).await?;
        Ok(startup.finish())
    }
}

/// A running Lynx view: a window's worth of Lynx, on a thread its
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
    control: Arc<StartupControl>,
}

impl<F: ResourceFetcher> ViewStartup<F> {
    async fn serve(
        &mut self,
        style_sheets: Vec<String>,
        entry: String,
    ) -> Result<(), LynxViewError> {
        self.painter
            .as_mut()
            .expect("the painter exists before startup is served")
            .serve_startup(style_sheets, entry)
            .await
    }

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
        // Cancellation first: `bobcat-main` checks the flag at every gate
        // between this view's sources, so a source that lands in the same
        // instant cannot carry its boot onward into QuickJS. It is this
        // view's flag alone — the group's other views go on booting.
        self.control.cancel();
        // Then the goodbye, which the group's thread answers by releasing
        // this view and nothing else. Either the painter holds the sender,
        // or — if the draw target failed before one existed — the bare link
        // still does. Pending resource futures die with the painter, on this
        // thread, which is the thread that created them.
        if let Some(painter) = self.painter.as_mut() {
            painter.shutdown();
        } else if let Some(link) = self.link.as_ref() {
            link.send(ToMain::Shutdown);
        }
    }
}

/// Which view on a group's thread a command is for.
///
/// Every view in a group sends on one FIFO, so every command names its view;
/// a group hands the ids out and never reuses one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewId(pub(crate) u64);

/// A view for a group's thread to adopt, and everything that view needs
/// which is not already the group's.
///
/// Nothing here is generic over the embedder's [`EventRequester`]. The one
/// part of a view's link that knows it is the requester itself, and that is
/// the group's — every view in a group paints on the thread that created the
/// group, and so wakes one event loop. That is what lets attachments and
/// commands share a single channel instead of needing a select over two.
pub(crate) struct Attachment {
    pub(crate) view: ViewId,
    pub(crate) viewport: Viewport,
    pub(crate) sources: MainSources,
    pub(crate) notifications: flume::Sender<ToPainter>,
    pub(crate) frames: Arc<FrameHub>,
    pub(crate) control: Arc<StartupControl>,
}

/// Everything that reaches a group's `bobcat-main`, from every view on it.
pub(crate) enum GroupCommand {
    /// A view to adopt, on the script runtime and style pool this thread
    /// already holds.
    Attach(Box<Attachment>),
    /// One carried view's command.
    View { view: ViewId, command: ToMain },
    /// The group handle is gone, and every view built from it with it.
    Close,
}

/// Painter → Lynx main: every fact the document must see.
pub(crate) enum ToMain {
    DispatchEvent {
        target: NodeId,
        name: &'static str,
        detail: String,
    },
    Resize {
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    },
    BeginFrame {
        now: f64,
        seq: u64,
    },
    Refill {
        offsets: Vec<(NodeId, Vector2D<f32>)>,
    },
    /// The host's image reports: completed or failed loads. No variant can
    /// carry pixels, which is what makes "`ImageData` never crosses a
    /// channel" a property of the type.
    ImageEvents(Vec<dom::ImageEvent>),
    /// One startup source, pushed by the painter unasked.
    ///
    /// The order of these sends is the protocol: stylesheets in cascade
    /// order, the entry last, so `bobcat-main` mounts in arrival order and
    /// the entry's arrival is what completes its wait. Only ever a success:
    /// a fetch that fails is the startup failure, and the painter is the
    /// side that already holds it, so it returns from construction rather
    /// than sending the error across and waiting to be told back what it
    /// just decided.
    SourceLoaded {
        source: LoadedSource,
    },
    Shutdown,
    #[cfg(test)]
    Probe(Box<dyn FnOnce(&mut LynxDocument) + Send>),
}

/// A stylesheet in the one shape the document mounts.
///
/// Text arrives as `String` rather than bytes: UTF-8 validation happens on
/// the painter, where the resolved URL the error has to name is in hand.
#[derive(Debug)]
pub(crate) enum StyleSheetSource {
    Preparsed(Arc<PreparsedStyleSheet>),
    Text(String),
}

/// A source, resolved and decoded by the thread that owns the fetcher.
#[derive(Debug)]
pub(crate) enum LoadedSource {
    StyleSheet(StyleSheetSource),
    Entry { source: String, url: String },
}

/// Lynx main → painter: everything the main thread has to say back.
#[derive(Debug)]
pub(crate) enum ToPainter {
    /// The frame mailbox holds something the painter has not read.
    FrameChanged,
    Engine(EngineEvent),
    ListenerAvailable(Arc<str>),
    ListenerUnavailable(Arc<str>),
    BeginFrameServiced(u64),
    /// Sources the last paint walk met that the store has not been asked for.
    RequestImages(Vec<Arc<str>>),
    /// How startup went — the message that replaces the startup oneshot.
    ///
    /// It rides this FIFO *behind* `ScriptFinished` and `FrameChanged`, so a
    /// painter that has seen it has already adopted boot's frame and buffered
    /// boot's lifecycle event for the host's first `pump`.
    Started(Result<(), LynxViewError>),
}

/// The latest committed frame, and only ever the latest.
pub(crate) type FrameHub = Mutex<Option<Arc<CommittedFrame>>>;

pub(crate) fn frame_slot(hub: &FrameHub) -> MutexGuard<'_, Option<Arc<CommittedFrame>>> {
    hub.lock()
        .unwrap_or_else(|error| panic!("the frame mailbox is poisoned: {error}"))
}

/// Builds one view's half of its group's link: the painter's end, and the
/// two pieces of the main thread's end that cross in its attachment.
///
/// The commands go the other way on a channel the group already owns, which
/// is why only this direction is built here.
fn view_link(
    view: ViewId,
    commands: &flume::Sender<GroupCommand>,
) -> (PainterLink, flume::Sender<ToPainter>, Arc<FrameHub>) {
    let (notifications, notification_receiver) = flume::unbounded();
    let frames = Arc::new(FrameHub::new(None));
    let painter = PainterLink::new(
        view,
        commands.clone(),
        notification_receiver,
        Arc::clone(&frames),
    );
    (painter, notifications, frames)
}

/// Both ends of one view's link, for a caller that is itself the far end: the
/// crate's benchmarks and the unit tests that drive a document in place
/// rather than over a group's thread.
pub(crate) struct DetachedLink<R: EventRequester> {
    /// What the painter sent, still tagged with the view a group's thread
    /// would have routed it to. Only the unit tests that play that thread
    /// read it; a benchmark needs the reporting half alone.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only a test plays the far end of a link")
    )]
    pub(crate) commands: flume::Receiver<GroupCommand>,
    /// Everything the main thread's side has to say back.
    pub(crate) notify: ToPainterSender<R>,
}

#[cfg(test)]
impl<R: EventRequester> DetachedLink<R> {
    /// The next command the painter sent, with the view tag stripped off.
    ///
    /// A detached link carries exactly one view and no group control, so
    /// there is nothing else the tag could have selected.
    pub(crate) fn try_recv(&self) -> Result<ToMain, flume::TryRecvError> {
        self.commands.try_recv().map(|message| match message {
            GroupCommand::View { command, .. } => command,
            GroupCommand::Attach(_) | GroupCommand::Close => {
                unreachable!("a detached link carries no group control")
            }
        })
    }
}

/// Builds a link with nothing on the far end of it.
pub(crate) fn detached_link<R: EventRequester>(
    requester: Arc<R>,
) -> (PainterLink, DetachedLink<R>) {
    let (commands, command_receiver) = flume::unbounded();
    let (painter, notifications, frames) = view_link(DETACHED_VIEW, &commands);
    // The local sender goes here: the painter holds the only clone, so the
    // receiver still reports a disconnect when that painter is dropped.
    drop(commands);
    (
        painter,
        DetachedLink {
            commands: command_receiver,
            notify: ToPainterSender::new(notifications, frames, requester),
        },
    )
}

/// The one view a [`detached_link`] carries, and the one
/// [`crate::main::spawn_test_main_thread`] serves.
pub(crate) const DETACHED_VIEW: ViewId = ViewId(0);
