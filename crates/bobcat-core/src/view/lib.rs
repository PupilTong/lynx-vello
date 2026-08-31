//! Shared vocabulary and coordination for one Lynx view's three threads.
//!
//! A view spans three owners. The embedder's user thread holds [`LynxView`],
//! which is a handle and nothing else: it captures input, creates the
//! surface, and drains lifecycle events. The paint thread owns the
//! painter — every draw target, the gesture router, the scroll intents,
//! and the composition — and the Lynx main thread owns the document and the
//! script realm. The sibling `user`, `paint`, and `main` modules mirror those
//! owners; this module keeps only the vocabulary and links that cross them.

use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, mpsc};

use dom::input::InputEvent;
use dom::{CommittedFrame, NodeId, Vector2D};

#[cfg(target_arch = "wasm32")]
pub use crate::main::configure_wasm_workers;
#[cfg(test)]
use crate::main::tree::LynxDocument;
use crate::main::{MainLink, ToPresenterSender};
pub use crate::paint::WindowTarget;
use crate::paint::{PainterLink, PresenterLink, WindowGraphics};
use crate::script::ScriptError;
use crate::user::HostLink;
pub use crate::user::{LynxView, ViewSources};

/// View metrics copied across all three thread boundaries.
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
    /// The presenter could not produce a frame. Fatal for the draw target:
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

/// Wakes a thread that parks on an event loop the engine does not own.
///
/// One implementation per platform — a winit event-loop proxy, an `AppKit`
/// source, a Worker's signal — and the view is generic over it, so the wake
/// is a direct call rather than a virtual one. A view's link holds the only
/// handle to it.
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

/// Paint → Lynx main: every fact the document must see.
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

/// Lynx main → paint: everything the main thread has to say back.
#[derive(Debug)]
pub(crate) enum ToPresenter {
    /// The frame mailbox holds something the paint thread has not read.
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

/// Builds the link between the paint and Lynx main threads.
pub(crate) fn main_link<R: EventRequester>(requester: Arc<R>) -> (PresenterLink, MainLink<R>) {
    let (commands, command_receiver) = mpsc::channel();
    let (notifications, notification_receiver) = mpsc::channel();
    let frames = Arc::new(FrameHub::new(None));
    let presenter = PresenterLink::new(commands, notification_receiver, Arc::clone(&frames));
    let main = MainLink::new(
        command_receiver,
        ToPresenterSender::new(notifications, frames, requester),
    );
    (presenter, main)
}

/// User → paint: every fact the embedder's thread has to hand over.
pub(crate) enum ToPainter {
    Input(InputEvent),
    Resize {
        viewport: Viewport,
        frame_size: FrameSize,
    },
    Occluded(bool),
    Refresh,
    /// Built on the window-owning user thread, then owned by the painter.
    Attach(Box<WindowGraphics>),
    AttachOffscreen(mpsc::Sender<Result<(), EngineError>>),
    Tick {
        force: bool,
        reply: mpsc::Sender<Result<bool, EngineError>>,
    },
    #[cfg(not(target_arch = "wasm32"))]
    Capture(mpsc::Sender<Result<Screenshot, EngineError>>),
    NoteImagesChanged,
    #[cfg(not(target_arch = "wasm32"))]
    MainChanged,
    Shutdown,
}

/// Builds the link between the user and paint threads.
pub(crate) fn paint_link<R: EventRequester>(requester: Arc<R>) -> (HostLink, PainterLink<R>) {
    let (commands, command_receiver) = mpsc::channel();
    let (events, event_receiver) = mpsc::channel();
    let animating = Arc::new(AtomicBool::new(false));
    let host = HostLink::new(commands, event_receiver, Arc::clone(&animating));
    let painter = PainterLink::new(command_receiver, events, requester, animating);
    (host, painter)
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
