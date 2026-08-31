//! The embedder-facing Lynx view and the private link behind it.
//!
//! Source loading, Lynx-main-thread ownership, and presenting-thread work live
//! in focused submodules. This module keeps the public facade and the state
//! shared by those parts.

mod graphics;
mod loading;
mod main_thread;
mod presenter;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod animation_tests;
#[cfg(test)]
mod event_loop_tests;
#[cfg(test)]
mod tests;

use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

use dom::render::gpu::Headless;
use dom::vello::Scene;

use self::graphics::WindowGraphics;
pub use self::graphics::WindowTarget;
#[cfg(test)]
use self::loading::EntryModule;
pub use self::loading::{LynxViewError, ViewSources};
#[cfg(target_arch = "wasm32")]
pub use self::main_thread::configure_wasm_workers;
use self::presenter::ScrollIntents;
use crate::clock::FrameClock;
use crate::gesture::GestureRouter;
use crate::link::PresenterLink;
use crate::script::ScriptError;
use crate::tree::Viewport;

/// The physical pixel size of the render target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

const MAX_RENDER_DIMENSION: u32 = 16_384;

/// The commit id and scroll-intent generation composing one drawn frame.
type ComposeKey = (u64, u64);

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

#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    /// The entry MTS module and Bobcat boot completed successfully.
    ScriptFinished,
    /// The script runtime failed fatally during boot or later owner-thread work.
    ScriptRunError(ScriptError),
    /// A listener threw while an event was being delivered to it.
    ListenerFailed(ScriptError),
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

/// The draw target an embedder lends the engine.
pub trait Window {
    type Target<'window>: Into<WindowTarget<'window>>
    where
        Self: 'window;

    fn target(&self) -> Self::Target<'_>;
}

/// Wakes the host event loop for pending engine events or frames.
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

#[derive(Debug)]
pub enum NoWindow {}

impl Window for NoWindow {
    type Target<'window> = WindowTarget<'window>;

    fn target(&self) -> Self::Target<'_> {
        match *self {}
    }
}

enum Output<'window> {
    None,
    Offscreen(Box<Headless>),
    Window(Box<WindowGraphics<'window>>),
}

/// A running Lynx view with an engine-owned script thread.
///
/// Generic over the embedder's window capability. The windowless form is
/// [`OffscreenLynxView`]. A view remains on the thread that owns its presenter.
pub struct LynxView<'window, W: Window = NoWindow, R: EventRequester = NoWakeup> {
    // Keep first: dropping the link closes the sole command sender, which
    // wakes the main thread before any state it may still refer to is
    // released.
    link: PresenterLink<R>,
    #[cfg(test)]
    detached: bool,
    image_store: Arc<dyn dom::ImageStore>,
    viewport: Viewport,
    frame_size: FrameSize,
    output: Output<'window>,
    gesture: GestureRouter,
    clock: FrameClock,
    scroll_intents: ScrollIntents,
    composed: Option<ComposeKey>,
    composed_scene: Scene,
    refill_requested_for: Option<u64>,
    window: PhantomData<fn() -> W>,
    thread_bound: PhantomData<Rc<()>>,
}

/// The offscreen composition of [`LynxView`].
pub type OffscreenLynxView<R = NoWakeup> = LynxView<'static, NoWindow, R>;

impl<W: Window, R: EventRequester> fmt::Debug for LynxView<'_, W, R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LynxView")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .finish_non_exhaustive()
    }
}

fn frame_size(width: f32, height: f32, device_pixel_ratio: f32) -> Result<FrameSize, EngineError> {
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
