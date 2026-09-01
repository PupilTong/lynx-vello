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
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::Duration;

use dom::input::InputEvent;
use dom::{CommittedFrame, FontBlob, ImageStore, NodeId, Vector2D};
use tokio::sync::oneshot;

#[cfg(target_arch = "wasm32")]
pub use crate::main::configure_wasm_workers;
#[cfg(test)]
use crate::main::tree::LynxDocument;
use crate::main::tree::PageConfig;
use crate::main::{
    MainLink, MainThreadHome, StartupRequest, StartupResult, StartupSuccess, ToPainterSender,
    spawn_main_thread,
};
pub use crate::paint::WindowTarget;
use crate::paint::{Painter, PainterLink};
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

const MAX_RENDER_DIMENSION: u32 = 16_384;

/// The commit id and scroll-intent generation composing one drawn frame.
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
    #[error("the engine has no draw target attached")]
    NoDrawTarget,
    #[error("the document is busy in a script batch; retry the repaint request")]
    ResourceUpdateBusy,
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
    #[error("the image store could not load `{image_source}`: {message}")]
    Image {
        image_source: String,
        message: String,
    },
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

/// Everything transferred to `bobcat-main` before the entry module starts.
#[derive(Clone)]
pub struct ViewSources {
    /// Owned by `bobcat-main` for the complete source-loading phase. The
    /// fetcher's own implementation decides where actual network or file IO
    /// runs; completion and document mutation resume on the main thread.
    pub resource_fetcher: Arc<dyn ResourceFetcher>,
    pub fonts: Vec<FontBlob>,
    pub default_font_family: Option<String>,
    pub image_store: Option<Arc<dyn ImageStore>>,
    pub style_sheets: Vec<String>,
    pub entry: String,
}

impl ViewSources {
    #[must_use]
    pub fn new(resource_fetcher: Arc<dyn ResourceFetcher>, entry: impl Into<String>) -> Self {
        Self {
            resource_fetcher,
            fonts: Vec::new(),
            default_font_family: None,
            image_store: None,
            style_sheets: Vec::new(),
            entry: entry.into(),
        }
    }
}

impl fmt::Debug for ViewSources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ViewSources")
            .field("style_sheets", &self.style_sheets)
            .field("entry", &self.entry)
            .finish_non_exhaustive()
    }
}

/// A running Lynx view: a window's worth of Lynx, and the one engine-owned
/// thread behind it.
///
/// The view stays on the thread that built it, and that thread is where it
/// paints: it owns the draw targets, the gesture router, the scroll intents
/// and the composition outright, so an embedder chooses the painting thread
/// by choosing where it calls [`LynxView::new`]. Nothing here is a queue and
/// nothing here draws by itself — every call applies immediately, and the
/// frame those calls owe is produced by the next [`LynxView::pump`], which
/// is also the turn that hands back what the realm had to say. A host parked
/// on its own event loop therefore takes a turn after it hands a fact in;
/// facts from the Lynx main thread arrive with the construction-time
/// [`EventRequester`] wakeup.
pub struct LynxView {
    painter: Painter,
    main: MainThreadHome,
    image_store: Arc<dyn ImageStore>,
}

impl fmt::Debug for LynxView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LynxView")
            .field("painter", &self.painter)
            .finish_non_exhaustive()
    }
}

impl Drop for LynxView {
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

impl LynxView {
    /// Starts `bobcat-main` and waits asynchronously until it has created its
    /// document, loaded and mounted every source, and booted the entry
    /// module. The painter is built here, on the calling thread, which owns
    /// it from now on.
    ///
    /// Dropping this future before it resolves cancels pending resource work
    /// or stops startup before `QuickJS` begins, releases the painter, and
    /// joins `bobcat-main`. If synchronous startup JavaScript is already
    /// running, teardown waits for that work to return before the main thread
    /// can be joined.
    pub async fn new<R: EventRequester>(
        config: PageConfig,
        event_requester: Arc<R>,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        sources: ViewSources,
    ) -> Result<Self, LynxViewError> {
        let frame_size = frame_size(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let (started_sender, started) = oneshot::channel();
        let (painter_link, main_link) = main_link(event_requester);
        let main = spawn_main_thread(
            StartupRequest::new(config, viewport, sources),
            main_link,
            started_sender,
        )?;
        let mut startup = ViewStartup {
            painter: Some(Painter::new(viewport, frame_size, painter_link)),
            main: Some(main),
            started: Some(started),
        };
        let success = startup.wait().await?;
        Ok(startup.finish(success))
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

    /// How long the host may sleep before offering the view another
    /// [`Self::pump`].
    ///
    /// `None` is nothing owed: park until an OS fact or the engine's wakeup
    /// opens the next turn. `Some(Duration::ZERO)` is a running animation —
    /// come straight back, because the swap chain's `AutoVsync` acquire
    /// inside the next draw is the pace, and on a target whose acquire does
    /// not wait (a browser canvas) it means one display frame instead.
    /// Anything else is a swap chain that had no image to give and said so
    /// without waiting for vsync, so the retry needs that delay or it becomes
    /// a spin.
    ///
    /// Only ever a visible window answers anything but `None`; an offscreen
    /// view's frames are the host's to ask for through [`Self::tick`].
    #[must_use]
    pub fn next_turn(&self) -> Option<Duration> {
        self.painter.next_turn()
    }

    /// Whether the engine owed the timeline another frame as of the last
    /// turn.
    ///
    /// A host that owns the display clock — a Worker driving
    /// `requestAnimationFrame` — reads it to decide whether to ask for
    /// another turn. Unlike [`Self::next_turn`] it answers for an offscreen
    /// view too, which has no display to pace against.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.painter.is_animating()
    }

    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.painter.frame_size()
    }

    /// Lends the view a draw target.
    ///
    /// The presentation stack is built on this thread, the one that owns the
    /// window, because creating a surface from a window handle is a
    /// main-thread-only call on macOS — and it is also the thread that will
    /// draw into it.
    ///
    /// It is configured at the view's own frame size rather than one the
    /// caller names, so the first draw cannot find a surface built for a
    /// different target than the one it is about to paint.
    pub async fn attach_target(
        &mut self,
        target: impl Into<WindowTarget>,
    ) -> Result<(), EngineError> {
        self.painter.attach_target(target).await
    }

    /// Gives the view a windowless GPU target of its own.
    pub fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        self.painter.attach_offscreen()
    }

    /// Advances an offscreen view by one frame, answering whether it drew.
    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        self.painter.tick(force)
    }

    /// Reads back what the view last rendered.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        self.painter.capture()
    }

    /// Resolves an image through the embedder's store and tells the document
    /// its pixels are resident.
    pub async fn load_image(&self, source: &str) -> Result<(), LynxViewError> {
        let store = Arc::clone(&self.image_store);
        store
            .get(source)
            .await
            .map_err(|error| LynxViewError::Image {
                image_source: source.to_owned(),
                message: error.to_string(),
            })?;
        self.painter.note_images_changed();
        Ok(())
    }

    pub fn prefetch_image(&self, source: &str) {
        self.image_store.prefetch(source);
    }
}

/// A half-built view whose destructor is the cancellation protocol for
/// [`LynxView::new`].
struct ViewStartup {
    painter: Option<Painter>,
    main: Option<MainThreadHome>,
    started: Option<oneshot::Receiver<StartupResult>>,
}

impl ViewStartup {
    async fn wait(&mut self) -> Result<StartupSuccess, LynxViewError> {
        self.started
            .as_mut()
            .expect("startup result is present until the view is built")
            .await
            .map_err(|_| EngineError::Thread {
                name: "script",
                message: "the Lynx main thread stopped before startup completed".to_owned(),
            })?
    }

    fn finish(mut self, success: StartupSuccess) -> LynxView {
        self.started.take();
        LynxView {
            painter: self.painter.take().expect("startup owns the painter"),
            main: self.main.take().expect("startup owns the Lynx main thread"),
            image_store: success.image_store,
        }
    }
}

impl Drop for ViewStartup {
    fn drop(&mut self) {
        // Publish cancellation before closing the result receiver, so a
        // resource that becomes ready in the same poll cannot proceed into
        // QuickJS before bobcat-main observes the flag.
        if let Some(main) = self.main.as_ref() {
            main.cancel();
        }
        // Closing wakes `Sender::closed`, which drops any pending
        // ResourceFuture on bobcat-main.
        if let Some(started) = self.started.as_mut() {
            started.close();
        }
        // The painter's goodbye must precede the join, exactly as it does in
        // `Drop for LynxView`: a successfully booted main thread is parked on
        // the FIFO the painter holds the only sender for.
        if let Some(painter) = self.painter.as_mut() {
            painter.shutdown();
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
    NoteImagesChanged,
    Shutdown,
    #[cfg(test)]
    Probe(Box<dyn FnOnce(&mut LynxDocument) + Send>),
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
}

/// The latest committed frame, and only ever the latest.
pub(crate) type FrameHub = Mutex<Option<Arc<CommittedFrame>>>;

pub(crate) fn frame_slot(hub: &FrameHub) -> MutexGuard<'_, Option<Arc<CommittedFrame>>> {
    hub.lock()
        .unwrap_or_else(|error| panic!("the frame mailbox is poisoned: {error}"))
}

/// Builds the view's one link: the painter's end and the Lynx main thread's.
pub(crate) fn main_link<R: EventRequester>(requester: Arc<R>) -> (PainterLink, MainLink<R>) {
    let (commands, command_receiver) = mpsc::channel();
    let (notifications, notification_receiver) = mpsc::channel();
    let frames = Arc::new(FrameHub::new(None));
    let painter = PainterLink::new(commands, notification_receiver, Arc::clone(&frames));
    let main = MainLink::new(
        command_receiver,
        ToPainterSender::new(notifications, frames, requester),
    );
    (painter, main)
}

pub(crate) fn frame_size(
    width: f32,
    height: f32,
    device_pixel_ratio: f32,
) -> Result<FrameSize, EngineError> {
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
    Ok(FrameSize {
        width: physical_width.round().max(1.0) as u32,
        height: physical_height.round().max(1.0) as u32,
    })
}
