//! User-thread ownership: the embedder handle and presenter driver.
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
//! handle's API is the same either way. Construction on this thread stops at
//! channel setup: source loading, document initialization, and script boot
//! all belong to `bobcat-main`.

use std::fmt;
use std::marker::PhantomData;
#[cfg(not(target_arch = "wasm32"))]
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use dom::input::InputEvent;
use dom::{FontBlob, ImageStore};
use tokio::sync::oneshot;

use crate::main::tree::{PageConfig, Viewport};
#[cfg(not(target_arch = "wasm32"))]
use crate::main::{InboxWakeup, panic_payload};
use crate::main::{
    MainThreadHome, StartupRequest, StartupResult, StartupSuccess, spawn_main_thread,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::paint::PresenterLink;
use crate::paint::{Painter, PainterLink, WindowGraphics};
use crate::resource::ResourceFetcher;
#[cfg(not(target_arch = "wasm32"))]
use crate::view::Screenshot;
use crate::view::{
    EngineError, EngineEvent, EventRequester, FrameSize, LynxViewError, NoWakeup, ToPainter,
    WindowTarget, frame_size, main_link, paint_link,
};

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

/// The user thread's end of its link to the painter.
pub(crate) struct HostLink {
    commands: mpsc::Sender<ToPainter>,
    events: mpsc::Receiver<EngineEvent>,
    /// Whether the painter owed the timeline another frame as of its last
    /// turn — the one fact the user thread reads without asking for it.
    animating: Arc<AtomicBool>,
}

impl HostLink {
    pub(crate) fn new(
        commands: mpsc::Sender<ToPainter>,
        events: mpsc::Receiver<EngineEvent>,
        animating: Arc<AtomicBool>,
    ) -> Self {
        Self {
            commands,
            events,
            animating,
        }
    }

    pub(crate) fn send(&self, command: ToPainter) {
        let _ = self.commands.send(command);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn wakeup(&self) -> InboxWakeup {
        InboxWakeup::new(self.commands.clone())
    }

    pub(crate) fn take_events(&self) -> Vec<EngineEvent> {
        self.events.try_iter().collect()
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.animating.load(Ordering::Relaxed)
    }
}

impl std::fmt::Debug for HostLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostLink")
            .field("animating", &self.is_animating())
            .finish_non_exhaustive()
    }
}

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
        if self.running {
            self.running = serve_pending(&mut self.painter, &self.link);
        }
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
    main: MainThreadHome,
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
        self.main.shutdown();
    }
}

impl<R: EventRequester> LynxView<R> {
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

/// A half-built view whose destructor is the cancellation protocol for
/// `LynxView::new`.
struct ViewStartup<R: EventRequester> {
    link: Option<HostLink>,
    home: Option<PainterHome<R>>,
    main: Option<MainThreadHome>,
    started: Option<oneshot::Receiver<StartupResult>>,
    viewport: Viewport,
    frame_size: FrameSize,
}

impl<R: EventRequester> ViewStartup<R> {
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

    fn finish(mut self, success: StartupSuccess) -> LynxView<R> {
        self.started.take();
        LynxView {
            link: self.link.take().expect("startup owns the host link"),
            home: self.home.take().expect("startup owns the presenter"),
            main: self.main.take().expect("startup owns the Lynx main thread"),
            image_store: success.image_store,
            viewport: self.viewport,
            frame_size: self.frame_size,
            thread_bound: PhantomData,
        }
    }
}

impl<R: EventRequester> Drop for ViewStartup<R> {
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
        if let Some(link) = self.link.as_ref() {
            link.send(ToPainter::Shutdown);
        }
        // The presenter must go first: dropping its PresenterLink closes the
        // command path a successfully booted main thread may be parked on.
        if let Some(home) = self.home.as_mut() {
            home.shutdown();
        }
        if let Some(main) = self.main.as_mut() {
            main.shutdown();
        }
    }
}

/// Starts the native presenter over the already-created main-thread link.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_presenter<H: EventRequester>(
    viewport: Viewport,
    frame_size: FrameSize,
    presenter: PresenterLink,
    link: PainterLink<H>,
) -> Result<std::thread::JoinHandle<()>, EngineError> {
    std::thread::Builder::new()
        .name("bobcat-present".to_owned())
        .spawn(move || {
            let painter = Painter::new(viewport, frame_size, presenter);
            run_presenter(painter, &link);
        })
        .map_err(|error| EngineError::Thread {
            name: "presenter",
            message: error.to_string(),
        })
}

impl<R: EventRequester> LynxView<R> {
    /// Starts both engine-owned threads and waits asynchronously until
    /// `bobcat-main` has created its document, loaded and mounted every
    /// source, and booted the entry module.
    ///
    /// Dropping this future before it resolves cancels pending resource work
    /// or stops startup before `QuickJS` begins, shuts down the presenter, and
    /// joins both engine-owned threads. If synchronous startup JavaScript is
    /// already running, teardown waits for that work to return before the main
    /// thread can be joined.
    pub async fn new(
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

        // The host's wakeup is always the embedder's. Whose the painter's is
        // depends on where it runs: its own inbox natively, and this same
        // requester where the presenter shares the host's thread.
        #[cfg(not(target_arch = "wasm32"))]
        let (link, painter_link) = paint_link(event_requester);
        #[cfg(target_arch = "wasm32")]
        let (link, painter_link) = paint_link(Arc::clone(&event_requester));

        #[cfg(not(target_arch = "wasm32"))]
        let (presenter, main_link) = main_link(Arc::new(link.wakeup()));
        #[cfg(target_arch = "wasm32")]
        let (presenter, main_link) = main_link(event_requester);

        let main = spawn_main_thread(
            StartupRequest::new(config, viewport, sources),
            main_link,
            started_sender,
        )?;
        let mut startup = ViewStartup {
            link: Some(link),
            home: None,
            main: Some(main),
            started: Some(started),
            viewport,
            frame_size,
        };

        #[cfg(not(target_arch = "wasm32"))]
        {
            startup.home = Some(PainterHome {
                thread: Some(spawn_presenter(
                    viewport,
                    frame_size,
                    presenter,
                    painter_link,
                )?),
                requester: PhantomData,
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            startup.home = Some(PainterHome {
                painter: Painter::new(viewport, frame_size, presenter),
                link: painter_link,
                running: true,
            });
        }

        let success = startup.wait().await?;
        Ok(startup.finish(success))
    }
}
