//! One Lynx view: the embedder's handle, and the vocabulary of its one
//! thread boundary.
//!
//! A view has two owners. The embedder's own thread — whichever one called
//! [`LynxView::new`] — holds the view and, inside it, the private painter:
//! it captures input, creates the surface, routes, composes, presents, and
//! drains lifecycle events, all inside the calls the embedder makes. The
//! Lynx main thread owns the document and the script realm. The sibling
//! `paint` and `main` modules mirror those two owners; this module holds the
//! handle that joins them and the link that crosses between them.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use dom::input::InputEvent;
use dom::{CommittedFrame, FontBlob, NodeId, Vector2D};

#[cfg(target_arch = "wasm32")]
pub use crate::main::configure_wasm_workers;
#[cfg(test)]
use crate::main::tree::LynxDocument;
use crate::main::tree::PageConfig;
use crate::main::{MainLink, MainThreadHome, ToPainterSender, spawn_main_thread};
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
    /// The same computation [`LynxView::new`] and [`LynxView::resize`] make,
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
/// There is no attaching a target later: [`LynxView::new`] builds it, on the
/// thread that will draw into it, before the view exists.
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
    /// Boot failures are returned by [`LynxView::new`] instead.
    ScriptRunError(ScriptError),
    /// A listener threw while an event was being delivered to it.
    ListenerFailed(ScriptError),
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
/// source, a Worker's signal — and [`LynxView::new`] is generic over it, so
/// the wake is a direct call rather than a virtual one. The Lynx main thread
/// holds the only handle to it, and calls it whenever it has published
/// something the embedder's next [`LynxView::pump`] would find: a committed
/// frame, a lifecycle event. It must never call back into the view, which it
/// could not anyway — a view never leaves the thread that built it.
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

/// Everything `bobcat-main` needs before the entry module starts.
///
/// It carries no resource system, and has no field that could hold one: the
/// host's fetcher belongs to the painter, is passed to [`LynxView::new`]
/// separately, and stays on that thread. That absence is why this is `Send`,
/// and why "the Lynx main thread never sees a store" is a property of the
/// type rather than of the call site — this struct *is* the startup message.
#[derive(Debug)]
pub struct ViewSources {
    pub config: PageConfig,
    pub fonts: Vec<FontBlob>,
    pub default_font_family: Option<String>,
    pub style_sheets: Vec<String>,
    pub entry: String,
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

/// A running Lynx view: a window's worth of Lynx, and the one engine-owned
/// thread behind it.
///
/// The view stays on the thread that built it, and that thread is where it
/// paints: it owns its one draw target, the gesture router, the scroll
/// intents and the composition outright, so an embedder chooses the painting
/// thread by choosing where it calls [`LynxView::new`]. The target is chosen
/// there too, and never afterwards. Nothing here is a queue and
/// nothing here draws by itself — every call applies immediately, and the
/// frame those calls owe is produced by the next [`LynxView::pump`], which
/// is also the turn that hands back what the realm had to say. A host parked
/// on its own event loop therefore takes a turn after it hands a fact in;
/// facts from the Lynx main thread arrive with the construction-time
/// [`EventRequester`] wakeup.
pub struct LynxView<F> {
    painter: Painter<F>,
    main: MainThreadHome,
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
        // Goodbye first, join second. The painter holds the only sender on
        // the FIFO `bobcat-main` parks on, so a shutdown that is not sent
        // before the join is a shutdown that never arrives. The draw target
        // goes with the painter, in the drop glue that runs the moment this
        // returns — still on this thread, and still before the embedder's
        // next statement, which is what lets it drop the window handle
        // straight afterwards on a platform where only its own thread may
        // destroy one.
        self.painter.shutdown();
        self.main.shutdown();
    }
}

impl<F: ResourceFetcher> LynxView<F> {
    /// Starts `bobcat-main`, builds the draw target on the calling thread,
    /// and waits asynchronously until the main thread has created its
    /// document, loaded and mounted every source, and booted the entry
    /// module.
    ///
    /// The target is an argument rather than something attached afterwards:
    /// a view that exists has somewhere to put a frame, so nothing has to
    /// describe — or handle — a view that has run but cannot draw. Its GPU
    /// objects are built while `bobcat-main` is already fetching, and the
    /// thread that builds them is the thread that owns them, which on macOS
    /// is the only thread allowed to create a surface at all.
    ///
    /// Dropping this future before it resolves cancels pending resource work
    /// or stops startup before `QuickJS` begins, releases the target, and
    /// joins `bobcat-main`. If synchronous startup JavaScript is already
    /// running, teardown waits for that work to return before the main thread
    /// can be joined.
    pub async fn new<R: EventRequester, B>(
        event_requester: Arc<R>,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        target: DrawTarget,
        resources: B,
        sources: ViewSources,
    ) -> Result<Self, LynxViewError>
    where
        B: FnOnce(dom::ImageReports) -> F,
    {
        let frame_size = FrameSize::for_viewport(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let (painter_link, main_link) = main_link(event_requester);
        // `sources` is the startup message itself, and the resource system
        // is not in it: it stays here, on the painter's thread.
        let main = spawn_main_thread(viewport, sources, main_link)?;
        // The link goes into the guard before the first await, so every exit
        // path has a real goodbye to send — including the one where the draw
        // target failed and there is no painter yet.
        let mut startup = ViewStartup {
            link: Some(painter_link),
            painter: None,
            main: Some(main),
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
        // Serving `bobcat-main`'s source requests *is* the wait for its
        // startup message: one loop over one inbox, so there is no arm to
        // forget and no second thing to wait on.
        startup.serve().await?;
        Ok(startup.finish())
    }

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
/// [`LynxView::new`].
struct ViewStartup<F> {
    /// Held only until the painter exists, so a draw target that fails still
    /// leaves something able to say goodbye to `bobcat-main`.
    link: Option<PainterLink>,
    painter: Option<Painter<F>>,
    main: Option<MainThreadHome>,
}

impl<F: ResourceFetcher> ViewStartup<F> {
    async fn serve(&mut self) -> Result<(), LynxViewError> {
        self.painter
            .as_mut()
            .expect("the painter exists before startup is served")
            .serve_startup()
            .await
    }

    fn finish(mut self) -> LynxView<F> {
        LynxView {
            painter: self.painter.take().expect("startup owns the painter"),
            main: self.main.take().expect("startup owns the Lynx main thread"),
        }
    }
}

impl<F> Drop for ViewStartup<F> {
    fn drop(&mut self) {
        // Cancellation first: `bobcat-main` checks the flag at every gate
        // between sources, so a source that lands in the same instant cannot
        // carry boot onward into QuickJS.
        if let Some(main) = self.main.as_ref() {
            main.cancel();
        }
        // Then the goodbye, which must precede the join exactly as it does in
        // `Drop for LynxView`: `bobcat-main` parks on the FIFO whose only
        // sender lives on this side, so a shutdown not sent is a shutdown
        // that never arrives. Either the painter holds that sender, or — if
        // the draw target failed before one existed — the bare link still
        // does. Pending resource futures die with the painter, on this
        // thread, which is the thread that created them.
        if let Some(painter) = self.painter.as_mut() {
            painter.shutdown();
        } else if let Some(link) = self.link.as_ref() {
            link.send(ToMain::Shutdown);
        }
        if let Some(main) = self.main.as_mut() {
            main.shutdown();
        }
    }
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
    /// One startup source, answering exactly one [`ToPainter::FetchSource`].
    ///
    /// Only ever a success: a fetch that fails is the startup failure, and the
    /// painter is the side that already holds it, so it returns from
    /// construction rather than sending the error across and waiting to be
    /// told back what it just decided. `None` is reserved for a source the
    /// painter abandoned, which today cannot happen.
    SourceLoaded {
        slot: SourceSlot,
        source: Option<LoadedSource>,
    },
    Shutdown,
    #[cfg(test)]
    Probe(Box<dyn FnOnce(&mut LynxDocument) + Send>),
}

/// Which of the view's sources an answer is for.
///
/// `StyleSheet(i)` indexes [`ViewSources::style_sheets`], so cascade order is
/// a property of the reply rather than a convention about the order answers
/// happen to arrive in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceSlot {
    StyleSheet(usize),
    Entry,
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
    /// A source `bobcat-main` cannot fetch itself, because it owns no
    /// fetcher.
    ///
    /// Every one a view will ever ask for is sent before that thread parks
    /// for the first time; afterwards it holds no specifier at all, which is
    /// why no post-startup fetch is constructible.
    FetchSource {
        slot: SourceSlot,
        specifier: String,
    },
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

/// A wake carrying nothing, for the one wait the painter makes before it has
/// a host turn to be woken into.
///
/// `ToPainter` itself stays a `std::sync::mpsc` FIFO, because
/// `wait_begin_frame` needs `recv_timeout` and a `Receiver` is no `Future`.
/// So the message goes on that FIFO and a tick goes here, and the painter
/// awaits the tick and then drains the FIFO. The tick is only a nudge: every
/// fact is a durable queue entry, never a bare waker edge.
pub(crate) type StartupWakeSender = tokio::sync::mpsc::UnboundedSender<()>;
pub(crate) type StartupWakeReceiver = tokio::sync::mpsc::UnboundedReceiver<()>;

/// Builds the view's one link: the painter's end and the Lynx main thread's.
pub(crate) fn main_link<R: EventRequester>(requester: Arc<R>) -> (PainterLink, MainLink<R>) {
    let (commands, command_receiver) = flume::unbounded();
    let (notifications, notification_receiver) = flume::unbounded();
    let (wake, wake_receiver) = tokio::sync::mpsc::unbounded_channel();
    let frames = Arc::new(FrameHub::new(None));
    let painter = PainterLink::new(
        commands,
        notification_receiver,
        Arc::clone(&frames),
        wake_receiver,
    );
    let main = MainLink::new(
        command_receiver,
        ToPainterSender::new(notifications, frames, requester, wake),
    );
    (painter, main)
}
