//! The handle an embedder holds, and the thread it drives the painter on.
//!
//! Nothing here draws. [`LynxView`] lives on the embedder's own thread — the
//! one that owns the window, captures input, and is the only thread allowed
//! to create a surface — and every question it cannot answer from its own
//! replicas becomes one message on the link to the presenter.
//!
//! Who runs the painter is the one thing that differs by platform. Natively
//! it is an engine-owned thread parked on the link's inbox, so an `AppKit`
//! run loop never waits for vsync. On the web the presenter is the Render
//! Worker itself: `wgpu`'s handles are not `Send` under shared memory and an
//! `OffscreenCanvas` cannot be transferred again once a Worker holds it, so
//! the painter stays on the thread that owns the canvas and each turn runs
//! inside a `pump`. Both drivers run the same [`serve_pending`], and the
//! handle's API is the same either way.

use std::marker::PhantomData;
#[cfg(not(target_arch = "wasm32"))]
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::{Arc, mpsc};

use dom::input::InputEvent;

#[cfg(not(target_arch = "wasm32"))]
use super::Screenshot;
use super::graphics::WindowGraphics;
use super::loading::{EntryModule, LynxViewError};
#[cfg(not(target_arch = "wasm32"))]
use super::main_thread::panic_payload;
use super::painter::Painter;
use super::{
    EngineError, EngineEvent, EventRequester, FrameSize, NoWakeup, WindowTarget, frame_size,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::link::InboxWakeup;
use crate::link::{HostLink, PainterLink, ToPainter, painter_link};
use crate::tree::{LynxDocument, Viewport};

/// Applies everything the presenter has queued and ends the turn, answering
/// whether it is still running.
fn serve_pending<H: EventRequester>(painter: &mut Painter, link: &PainterLink<H>) -> bool {
    while let Some(command) = link.try_next() {
        if !painter.apply(command, link) {
            return false;
        }
    }
    painter.serve(link);
    true
}

/// The presenter thread's whole life.
///
/// One turn per burst of facts, and then whatever [`Painter::next_turn`] says
/// the frame it just produced implies: straight into the next one while an
/// animation owes the display frames, a short wait for a swap chain that had
/// nothing to give, or parked on the inbox until something arrives.
///
/// A panic here would otherwise be silent — the window would simply stop
/// updating and the host would go on waiting — so it is caught and reported
/// as the render failure it is, over a link that is still alive.
#[cfg(not(target_arch = "wasm32"))]
fn run_presenter<H: EventRequester>(painter: Painter, link: &PainterLink<H>) {
    let served = catch_unwind(AssertUnwindSafe(|| present_until_shutdown(painter, link)));
    if let Err(payload) = served {
        link.report(EngineEvent::RenderFailed(EngineError::Thread {
            name: "presenter",
            message: format!(
                "the presenter thread panicked: {}",
                panic_payload(payload.as_ref())
            ),
        }));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn present_until_shutdown<H: EventRequester>(mut painter: Painter, link: &PainterLink<H>) {
    while serve_pending(&mut painter, link) {
        let waited = match painter.next_turn() {
            Some(delay) if delay.is_zero() => continue,
            Some(delay) => link.next_within(delay),
            None => Some(link.next()),
        };
        if let Some(command) = waited
            && !painter.apply(command, link)
        {
            break;
        }
    }
}

/// Where a view's painter lives, and what a host turn owes it.
#[cfg(not(target_arch = "wasm32"))]
struct PainterHome<R: EventRequester> {
    /// Taken on the way out, so the join happens exactly once.
    thread: Option<std::thread::JoinHandle<()>>,
    requester: PhantomData<fn() -> R>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<R: EventRequester> PainterHome<R> {
    /// Nothing: sending on the link already woke the thread that serves it.
    #[allow(
        clippy::unused_self,
        reason = "the driver whose painter shares the host thread needs the receiver"
    )]
    const fn notify(&self) {}

    /// Nothing: the presenter serves its own turns.
    #[allow(
        clippy::unused_self,
        reason = "the driver whose painter shares the host thread needs the receiver"
    )]
    const fn serve(&mut self) {}

    const fn is_running(&self) -> bool {
        self.thread.is_some()
    }

    /// Waits for the presenter to release its draw target.
    ///
    /// The view outlives its surface by exactly this call, which is what
    /// lets an embedder drop its window handle straight afterwards on a
    /// platform where only the main thread may destroy one.
    fn shutdown(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The painter shares this thread, so the host owes it the turns.
#[cfg(target_arch = "wasm32")]
struct PainterHome<R: EventRequester> {
    painter: Painter,
    link: PainterLink<R>,
    running: bool,
}

#[cfg(target_arch = "wasm32")]
impl<R: EventRequester> PainterHome<R> {
    /// A command queued for the painter is a turn this thread owes itself,
    /// and the embedder's loop is what hands the turn back.
    fn notify(&self) {
        self.link.requester().request_event();
    }

    fn serve(&mut self) {
        if !self.running {
            return;
        }
        self.running = serve_pending(&mut self.painter, &self.link);
        // A swap chain that had nothing to give needs another turn, and this
        // host's turns come from its own signal. A running animation does
        // not: the embedder paces that against the display itself.
        if self.running
            && self
                .painter
                .next_turn()
                .is_some_and(|delay| !delay.is_zero())
        {
            self.link.requester().request_event();
        }
    }

    const fn is_running(&self) -> bool {
        self.running
    }

    fn shutdown(&mut self) {
        self.running = false;
    }
}

/// A running Lynx view: a window's worth of Lynx, and the two engine-owned
/// threads behind it.
///
/// The handle stays on the thread that built it. Everything an embedder
/// hands over — input, metrics, a draw target — crosses to the presenter in
/// order, and everything that comes back does so through [`Self::pump`].
pub struct LynxView<R: EventRequester = NoWakeup> {
    link: HostLink,
    home: PainterHome<R>,
    image_store: Arc<dyn dom::ImageStore>,
    viewport: Viewport,
    frame_size: FrameSize,
    thread_bound: PhantomData<Rc<()>>,
}

impl<R: EventRequester> std::fmt::Debug for LynxView<R> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LynxView")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .finish_non_exhaustive()
    }
}

impl<R: EventRequester> Drop for LynxView<R> {
    fn drop(&mut self) {
        self.link.send(ToPainter::Shutdown);
        self.home.shutdown();
    }
}

impl<R: EventRequester> LynxView<R> {
    /// Starts both engine threads over a loaded document: the IO-free half
    /// of construction.
    pub(super) fn start(
        document: LynxDocument,
        viewport: Viewport,
        frame_size: FrameSize,
        event_requester: Arc<R>,
        entry: EntryModule,
    ) -> Result<Self, EngineError> {
        let image_store = Arc::clone(document.image_store());
        // The host's wakeup is always the embedder's. Whose the painter's is
        // depends on where it runs: its own inbox natively, and this same
        // requester where the presenter shares the host's thread.
        #[cfg(not(target_arch = "wasm32"))]
        let (link, painter) = painter_link(event_requester);
        #[cfg(target_arch = "wasm32")]
        let (link, painter) = painter_link(Arc::clone(&event_requester));
        #[cfg(not(target_arch = "wasm32"))]
        let home = {
            let wakeup = Arc::new(link.wakeup());
            PainterHome {
                thread: Some(spawn_presenter(
                    document, viewport, frame_size, wakeup, entry, painter,
                )?),
                requester: PhantomData,
            }
        };
        #[cfg(target_arch = "wasm32")]
        let home = PainterHome {
            painter: Painter::start(document, viewport, frame_size, event_requester, entry)?,
            link: painter,
            running: true,
        };
        Ok(Self {
            link,
            home,
            image_store,
            viewport,
            frame_size,
            thread_bound: PhantomData,
        })
    }

    /// Hands one normalized OS input event to the presenter, which routes it
    /// against the frame it last read.
    pub fn dispatch_input(&mut self, event: InputEvent) {
        self.send(ToPainter::Input(event));
    }

    /// Applies new device metrics, if they moved at all.
    ///
    /// The size is validated here, on the thread that measured it, so the
    /// answer an embedder gets is immediate and the presenter is handed a
    /// target it can only render.
    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<(), EngineError> {
        let next_size = frame_size(width, height, device_pixel_ratio)?;
        let size_changed = self.viewport.width.to_bits() != width.to_bits()
            || self.viewport.height.to_bits() != height.to_bits();
        let scale_changed =
            self.viewport.device_pixel_ratio.to_bits() != device_pixel_ratio.to_bits();
        if !size_changed && !scale_changed {
            return Ok(());
        }
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        self.send(ToPainter::Resize {
            viewport: self.viewport,
            frame_size: next_size,
        });
        Ok(())
    }

    /// Asks for a frame nothing else would have asked for.
    pub fn refresh(&self) {
        self.send(ToPainter::Refresh);
    }

    /// Reports whether the window is visible. An occluded one is not drawn,
    /// and the frame it owed is produced when it comes back.
    pub fn set_occluded(&self, occluded: bool) {
        self.send(ToPainter::Occluded(occluded));
    }

    /// Every lifecycle event the engine has produced since the last call.
    #[must_use]
    pub fn pump(&mut self) -> Vec<EngineEvent> {
        self.home.serve();
        self.link.take_events()
    }

    /// Whether the engine owed the timeline another frame as of the
    /// presenter's last turn.
    ///
    /// The presenter paces a running animation itself, so a windowed
    /// embedder does not need this. A host that owns the display clock — a
    /// Worker driving `requestAnimationFrame` — reads it to decide whether
    /// to ask for another turn.
    #[must_use]
    pub fn is_animating(&self) -> bool {
        self.link.is_animating()
    }

    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    /// Lends the view a draw target.
    ///
    /// The presentation stack is built here, on the thread that owns the
    /// window, because creating a surface from a window handle is a
    /// main-thread-only call on macOS. It is then handed over, and the
    /// presenter owns it for the rest of the view's life.
    ///
    /// It is configured at the view's own frame size rather than one the
    /// caller names, so the first draw cannot find a surface built for a
    /// different target than the one it is about to paint.
    pub async fn attach_target(
        &mut self,
        target: impl Into<WindowTarget>,
    ) -> Result<(), EngineError> {
        let graphics = WindowGraphics::new(target, self.frame_size).await?;
        self.send(ToPainter::Attach(Box::new(graphics)));
        Ok(())
    }

    /// Gives the view a windowless GPU target of its own.
    pub fn attach_offscreen(&mut self) -> Result<(), EngineError> {
        self.ask(ToPainter::AttachOffscreen)?
    }

    /// Advances an offscreen view by one frame, answering whether it drew.
    pub fn tick(&mut self, force: bool) -> Result<bool, EngineError> {
        self.ask(|reply| ToPainter::Tick { force, reply })?
    }

    /// Reads back what the view last rendered.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self) -> Result<Screenshot, EngineError> {
        self.ask(ToPainter::Capture)?
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
        self.send(ToPainter::NoteImagesChanged);
        Ok(())
    }

    pub fn prefetch_image(&self, source: &str) {
        self.image_store.prefetch(source);
    }

    /// Queues one fact for the presenter, and gives it the turn if it shares
    /// this thread.
    fn send(&self, command: ToPainter) {
        self.link.send(command);
        self.home.notify();
    }

    /// Asks the presenter something only it can answer, and waits.
    ///
    /// The wait is unbounded on purpose: the reply channel closes the moment
    /// the presenter stops, so a lost answer is reported rather than slept
    /// through, and every wait the presenter itself makes is bounded.
    fn ask<T>(
        &mut self,
        request: impl FnOnce(mpsc::Sender<T>) -> ToPainter,
    ) -> Result<T, EngineError> {
        let gone = || EngineError::Thread {
            name: "presenter",
            message: "the presenter stopped before it answered".to_owned(),
        };
        if !self.home.is_running() {
            return Err(gone());
        }
        let (reply, answer) = mpsc::channel();
        self.link.send(request(reply));
        self.home.serve();
        answer.recv().map_err(|_| gone())
    }
}

/// Starts the presenter thread, which builds the painter — and with it the
/// Lynx main thread — before it serves anything.
///
/// Construction is reported back before the loop begins, so a view that
/// cannot start its threads fails where it was asked to start them.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_presenter<H: EventRequester>(
    document: LynxDocument,
    viewport: Viewport,
    frame_size: FrameSize,
    wakeup: Arc<InboxWakeup>,
    entry: EntryModule,
    link: PainterLink<H>,
) -> Result<std::thread::JoinHandle<()>, EngineError> {
    let (ready, started) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("bobcat-present".to_owned())
        .spawn(move || {
            let painter = match Painter::start(document, viewport, frame_size, wakeup, entry) {
                Ok(painter) => painter,
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            let _ = ready.send(Ok(()));
            run_presenter(painter, &link);
        })
        .map_err(|error| EngineError::Thread {
            name: "presenter",
            message: error.to_string(),
        })?;
    match started.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(EngineError::Thread {
            name: "presenter",
            message: "the presenter stopped before it started".to_owned(),
        }),
    }
}
