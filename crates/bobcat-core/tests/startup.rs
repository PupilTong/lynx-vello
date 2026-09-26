//! Startup ownership and cancellation across a view's two threads: the one
//! that constructed it, which paints, and `bobcat-main`.

mod support;

use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobcat_core::resource::{ResourceFetcher, SourceCompletion, SourceRequest};
use bobcat_core::{DrawTarget, EngineEvent, EventRequester, NoWakeup, ViewSources};
use support::{FetcherDouble, solo_view, wait_for_script};

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

struct HostWakeup(flume::Sender<()>);
impl EventRequester for HostWakeup {
    fn request_event(&self) {
        let _ = self.0.send(());
    }
}

/// Identifies where a source is requested, completed or cancelled.
fn thread_tag() -> String {
    format!("{:?}", std::thread::current().id())
}

struct ThreadedFetcher {
    resume: flume::Receiver<()>,
    base: FetcherDouble,
    records: Arc<Mutex<Vec<(String, String)>>>,
}

impl ThreadedFetcher {
    fn record(&self, phase: &str) {
        self.records
            .lock()
            .expect("thread records")
            .push((phase.to_owned(), thread_tag()));
    }
}

impl bobcat_core::FrameImages for ThreadedFetcher {
    fn read(
        &self,
        source: &str,
        hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::vello::peniko::ImageData> {
        self.base.read(source, hint)
    }

    fn retain(&self, frame: &[Arc<str>]) {
        self.base.retain(frame);
    }
}

impl ResourceFetcher for ThreadedFetcher {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        self.record("request");
        let result = self.base.load_source(request);
        let resume = self.resume.clone();
        let records = Arc::clone(&self.records);
        std::thread::spawn(move || {
            resume.recv().expect("host releases IO");
            records
                .lock()
                .expect("thread records")
                .push(("complete".to_owned(), thread_tag()));
            completion.complete(result);
        });
    }
}

#[tokio::test]
async fn resource_completion_reaches_main_without_another_painter_turn() {
    let (wake, awakened) = flume::unbounded();
    let (release, resume) = flume::bounded(1);
    let records = Arc::new(Mutex::new(Vec::new()));
    let fetcher = Rc::new(ThreadedFetcher {
        resume,
        base: FetcherDouble::new(Vec::new()).resolving_to("app:///main.js"),
        records: Arc::clone(&records),
    });
    let (mut view, _painter) = solo_view(
        Arc::new(HostWakeup(wake)),
        393.0,
        727.0,
        1.0,
        DrawTarget::Offscreen,
        |_reports| fetcher,
        ViewSources::new("app:///", "app:///main.js", SCREEN),
    )
    .await
    .expect("startup completes");

    assert_eq!(
        records.lock().expect("thread records").len(),
        1,
        "creation hands the entry to the fetcher, on this thread"
    );
    assert!(view.pump().is_empty(), "the IO is still held pending");
    assert!(
        awakened.try_recv().is_err(),
        "and nothing of main's can wake the next turn: the request never \
         crossed a channel"
    );
    for _ in 0..64 {
        assert!(view.pump().is_empty());
    }
    assert_eq!(
        records.lock().expect("thread records").len(),
        1,
        "idle pumps never poll or restart IO"
    );
    release
        .send(())
        .expect("release the resource on its IO thread");
    loop {
        awakened
            .recv_timeout(HANG_BUDGET)
            .expect("main or resource completion wakes the host");
        let mut finished = false;
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => finished = true,
                EngineEvent::StartupFailed(error) => panic!("boot failed: {error}"),
                EngineEvent::Panicked(error) => panic!("the engine panicked: {error}"),
                _ => {}
            }
        }
        if finished {
            break;
        }
    }
    let records = records.lock().expect("thread records");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0], ("request".to_owned(), thread_tag()));
    assert_eq!(records[1].0, "complete");
    assert_ne!(
        records[1].1,
        thread_tag(),
        "completion runs on the fetcher's IO thread"
    );
}

struct PendingFetcher {
    base: FetcherDouble,
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    dropped: Mutex<Option<flume::Sender<String>>>,
    pending: Mutex<Option<SourceCompletion>>,
}

impl bobcat_core::FrameImages for PendingFetcher {
    fn read(
        &self,
        source: &str,
        hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::vello::peniko::ImageData> {
        self.base.read(source, hint)
    }

    fn retain(&self, frame: &[Arc<str>]) {
        self.base.retain(frame);
    }
}

impl ResourceFetcher for PendingFetcher {
    fn request_source(&self, _request: SourceRequest, completion: SourceCompletion) {
        *self.pending.lock().expect("pending completion") = Some(completion);
        if let Some(started) = self.started.lock().expect("start signal").take() {
            let _ = started.send(());
        }
    }
}

impl Drop for PendingFetcher {
    fn drop(&mut self) {
        if let Some(completion) = self.pending.get_mut().expect("pending completion").take() {
            assert!(
                completion.is_cancelled(),
                "view cancellation precedes releasing the fetcher"
            );
        }
        if let Some(dropped) = self.dropped.get_mut().expect("drop signal").take() {
            let _ = dropped.send(thread_tag());
        }
    }
}

/// How long a startup test may run before it is treated as hung.
///
/// A hang detector, **not** a performance assertion. It exists so a genuine
/// deadlock fails with a message instead of blocking the suite until the CI
/// job's own timeout, and nothing about it should discriminate a fast machine
/// from a slow one.
///
/// The number is chosen against measurement rather than taste. Reaching the
/// first fetch means building an offscreen GPU target and starting
/// `bobcat-main`: ~5.4s warm on an M-series laptop, ~14.4s cold, and longer
/// again when the whole workspace suite is competing for the machine. The
/// budget these tests used to carry was 10s, which sits *between* the warm and
/// cold figures — so it failed on a cold cache or a busy machine and passed
/// otherwise, which is the definition of a flaky test rather than a slow one.
/// Eight times the observed cold path leaves no plausible load that crosses
/// it while still failing a real deadlock in under two minutes.
///
/// It is spent in two different shapes, and they see different things.
/// [`hang_budget`] wraps an async test in `tokio::time::timeout`, which cannot
/// expire inside a synchronously blocking poll. The one blocking use —
/// `recv_timeout` around `dropping_the_group_joins_both_of_its_threads`, whose
/// whole body is a pair of blocking joins — does measure synchronous work, and
/// the budget is what fails that test rather than letting it hang. Anything
/// that waits *inside* that test's own thread takes a smaller budget, so a
/// step that never finishes is reported as that step rather than as a
/// teardown that would not return.
const HANG_BUDGET: Duration = Duration::from_mins(2);

/// Fails `future` with a clear message if it has not finished within
/// [`HANG_BUDGET`].
///
/// Every wall-clock deadline the async tests in this file carry lives here,
/// once, around a whole test. Per-step deadlines are the thing to avoid: they
/// turn "this step was slower than I guessed" into a failure, and there is no
/// step here whose duration is a property worth asserting.
///
/// What it can and cannot see is worth stating, because it is what makes this
/// safe. `timeout` polls the inner future before it consults the clock, so
/// time spent inside one *synchronously blocking* poll — building the
/// offscreen GPU target, which is most of a startup test's wall clock — is
/// invisible to it and cannot expire it. An async stall is not: a future
/// parked on a signal that never arrives yields, the runtime reaches the
/// timer, and this fires. That asymmetry is the whole point. It detects the
/// deadlock it is for and structurally cannot fail a machine that was only
/// slow. Verified by shrinking the budget to 1ms: the tests that go through
/// this wrapper still passed, and only an added async stall tripped it. That
/// experiment says nothing about the blocking `recv_timeout` below, which has
/// no such blind spot.
async fn hang_budget<F: Future<Output = ()>>(future: F) {
    tokio::time::timeout(HANG_BUDGET, future)
        .await
        .expect("startup hung: no progress within the hang budget");
}

#[derive(Debug)]
struct DropObservedRequester;

impl EventRequester for DropObservedRequester {
    fn request_event(&self) {}
}

#[tokio::test]
async fn dropping_loading_view_cancels_resource_and_reaps_main() {
    hang_budget(async {
        dropping_loading_view_cancels_resource_and_reaps_main_body().await;
    })
    .await;
}

async fn dropping_loading_view_cancels_resource_and_reaps_main_body() {
    let (started_sender, mut started) = tokio::sync::oneshot::channel();
    let (dropped_sender, dropped) = flume::unbounded();
    let fetcher = Rc::new(PendingFetcher {
        base: FetcherDouble::new(Vec::new()).resolving_to("app:///main.js"),
        started: Mutex::new(Some(started_sender)),
        dropped: Mutex::new(Some(dropped_sender)),
        pending: Mutex::new(None),
    });
    let fetcher_weak = Rc::downgrade(&fetcher);
    let requester = Arc::new(DropObservedRequester);
    let requester_weak = Arc::downgrade(&requester);
    let (mut view, mut painter) = solo_view(
        requester,
        393.0,
        727.0,
        1.0,
        DrawTarget::Offscreen,
        |_reports| fetcher,
        ViewSources::new("app:///", "app:///main.js", SCREEN),
    )
    .await
    .expect("creation returns a loading view even when the fetch never answers");
    // The entry request went out inside `create_lynx_view`, so it is already
    // held by the time the view exists.
    started
        .try_recv()
        .expect("creation handed the entry to the fetcher");
    assert!(view.pump().is_empty());
    // Public operations are legal during loading, including offscreen's
    // main-thread acknowledgement, which must not wait for the entry fetch.
    painter
        .resize(400.0, 800.0, 1.0)
        .expect("resize while loading");
    painter.tick(false).expect("tick while loading");
    drop(view);

    // Cancellation is visible before the view releases its concrete fetcher —
    // and dropping the view alone releases it, because the painter still
    // attached to it holds nothing but a weak handle.
    assert_eq!(
        dropped
            .recv()
            .expect("cancellation releases the source completion"),
        thread_tag()
    );
    assert!(
        fetcher_weak.upgrade().is_none(),
        "the view released its owned fetcher"
    );
    assert!(
        requester_weak.upgrade().is_none(),
        "bobcat-main exited and released its requester"
    );
    drop(painter);
}

/// Metrics that arrive while the entry fetch is outstanding reach the
/// document through the seat's watch rather than through a command, and the
/// document the boot module creates is the resized one: `createDocument`
/// reads that watch, so a painter that bound before the realm's first
/// statement has already named the metrics.
///
/// Asserted through the pixels, because the document is what an integration
/// test cannot name: the UA sheet gives `page` `width: 100%; height: 100%`, so
/// a document created at the resized viewport and device-pixel ratio paints
/// the painter's whole target, while one still carrying the metrics the view
/// was built at would cover a 32x24 corner of it.
#[tokio::test]
async fn metrics_that_arrive_before_the_document_are_what_it_is_created_at() {
    hang_budget(async {
        let (started_sender, mut started) = tokio::sync::oneshot::channel();
        let fetcher = Rc::new(PendingFetcher {
            base: FetcherDouble::new(Vec::new()),
            started: Mutex::new(Some(started_sender)),
            dropped: Mutex::new(None),
            pending: Mutex::new(None),
        });
        let (mut view, mut painter) = solo_view(
            Arc::new(NoWakeup),
            32.0,
            24.0,
            1.0,
            DrawTarget::Offscreen,
            |_| Rc::clone(&fetcher),
            ViewSources::new("app:///", "app:///main.js", SCREEN),
        )
        .await
        .expect("creation returns a loading view");
        // Held from creation: the entry was requested there, not in a turn.
        started
            .try_recv()
            .expect("creation handed the entry to the fetcher");

        // The painter owns device metrics, and this one is resized while the
        // view it is attached to is still loading.
        painter
            .resize(400.0, 800.0, 2.0)
            .expect("resize while loading");
        // A `BeginFrame` behind it, waited out: commands are a FIFO and the
        // loading task acknowledges this one itself, so the acknowledgement is
        // proof that the resize ahead of it has already been staged — which is
        // what lets the entry arrive afterwards rather than racing it.
        painter.tick(false).expect("tick while loading");
        let completion = fetcher
            .pending
            .lock()
            .expect("pending completion")
            .take()
            .expect("the entry fetch is outstanding");
        completion.complete(Ok(bobcat_core::resource::LoadedSource::Module {
            source: "globalThis.renderPage = function () {
               __SetInlineStyles(__CreatePage('card', 0), 'background-color:rgb(255,0,0)');
             };"
            .into(),
            url: "app:///main.js".into(),
        }));
        wait_for_script(&mut view).expect("the entry boots once its source arrives");

        let shot = painter.capture().expect("capture the committed page");
        assert_eq!((shot.size.width, shot.size.height), (800, 1600));
        let at = |x: u32, y: u32| {
            let start = ((y * shot.size.width + x) * 4) as usize;
            <[u8; 4]>::try_from(&shot.pixels[start..start + 4]).expect("an RGBA frame")
        };
        assert_eq!(
            at(shot.size.width - 8, shot.size.height - 8),
            [255, 0, 0, 255],
            "the document the boot module created covers the resized viewport"
        );
    })
    .await;
}

/// A default family nothing provides is a construction failure, not a
/// lifecycle event: the check runs inside `create_lynx_view`, ahead of the
/// startup requests that same call issues, so there is no view and nothing
/// was asked of the host.
#[tokio::test]
async fn an_unknown_font_family_fails_construction_without_fetching() {
    hang_budget(async {
        let fetcher = Rc::new(FetcherDouble::new(Vec::new()));
        let error = solo_view(
            Arc::new(NoWakeup),
            32.0,
            24.0,
            1.0,
            DrawTarget::Offscreen,
            |_| fetcher.clone(),
            ViewSources {
                default_font_family: Some("no-such-family".to_owned()),
                ..ViewSources::new("app:///", "app:///main.js", SCREEN)
            },
        )
        .await
        .expect_err("an unknown family is refused where the view is built");
        assert!(
            matches!(
                error,
                bobcat_core::LynxViewError::Engine(
                    bobcat_core::EngineError::UnknownFontFamily(ref family)
                ) if family == "no-such-family"
            ),
            "{error}"
        );
        assert_eq!(fetcher.resolve_count(), 0);
        assert_eq!(fetcher.fetch_count(), 0);
    })
    .await;
}

/// A view's URLs are resolved inside `create_lynx_view`, ahead of the
/// startup requests, so a base that is not a URL and an entry or BTS entry
/// that does not resolve against it are construction failures of the same
/// kind as an unknown font family: no view, and nothing asked of the host.
/// Each error names the string that failed.
#[tokio::test]
async fn an_unresolvable_url_fails_construction_without_fetching() {
    hang_budget(async {
        let cases = [
            (
                "not a URL",
                ViewSources::new("not a URL", "main.js", SCREEN),
            ),
            ("http://[", ViewSources::new("app:///", "http://[", SCREEN)),
            (
                "http://[",
                ViewSources {
                    background_entry: Some("http://[".to_owned()),
                    ..ViewSources::new("app:///", "main.js", SCREEN)
                },
            ),
        ];
        for (unresolvable, sources) in cases {
            let fetcher = Rc::new(FetcherDouble::new(Vec::new()));
            let error = solo_view(
                Arc::new(NoWakeup),
                32.0,
                24.0,
                1.0,
                DrawTarget::Offscreen,
                |_| fetcher.clone(),
                sources,
            )
            .await
            .expect_err("an unresolvable URL is refused where the view is built");
            assert!(
                matches!(
                    error,
                    bobcat_core::LynxViewError::Engine(
                        bobcat_core::EngineError::InvalidUrl { ref url, .. }
                    ) if url == unresolvable
                ),
                "{error}"
            );
            assert_eq!(fetcher.resolve_count(), 0);
            assert_eq!(fetcher.fetch_count(), 0);
        }
    })
    .await;
}

/// A resolution failure is one event, however many of the answers the same
/// call asked for failed.
///
/// All three startup sources — two sheets and the entry — are requested
/// inside `create_lynx_view`, so the host has resolved all three before the
/// view's own boot has read any of them, and every one of them fails here.
/// What stops at the first failure is the *reading*. The entry is read first:
/// the view's entry task reads its answer before boot can go on to render,
/// and the listed sheets are read only by boot's first `__FlushElementTree`,
/// which a boot whose entry failed never reaches. So one `StartupFailed` is
/// reported — the fetcher's own `Resource` error, which is what the entry
/// task was answered with, naming the entry as its locator and the reason in
/// its message — and the sheets' answers are dropped unread.
#[tokio::test]
async fn a_resolution_failure_is_one_event_whatever_else_failed() {
    let fetcher = Rc::new(FetcherDouble::new(Vec::new()).resolving_to("not a URL"));
    let (mut view, _painter) = solo_view(
        Arc::new(NoWakeup),
        32.0,
        24.0,
        1.0,
        DrawTarget::Offscreen,
        |_| fetcher.clone(),
        ViewSources {
            style_sheets: vec!["first.css".into(), "second.css".into()],
            ..ViewSources::new("app:///", "app:///main.js", SCREEN)
        },
    )
    .await
    .expect("resource failure does not prevent construction");
    assert_eq!(
        fetcher.resolve_count(),
        3,
        "creation hands over both sheets and the entry, before any turn"
    );
    let error = wait_for_script(&mut view).expect_err("the entry cannot be resolved");
    let bobcat_core::LynxViewError::Resource(error) = error else {
        panic!("the fetcher's own error: {error}");
    };
    assert_eq!(error.locator.as_deref(), Some("app:///main.js"), "{error}");
    assert!(
        error.to_string().contains("relative URL without a base"),
        "{error}"
    );
    assert_eq!(
        fetcher.resolve_count(),
        3,
        "and the failure asks for nothing further"
    );
    assert_eq!(fetcher.fetch_count(), 0);
    assert!(view.pump().is_empty(), "failure is delivered once");
}

/// A view whose entry is still in flight holds up nothing.
///
/// The boot module imports the entry by its URL, and nothing parks for it:
/// the import stays pending until a task of that view's owner completes the
/// module from the answer. So the pending view's job has already
/// returned, and a sibling view in the same group opens its realm, boots and
/// paints while the first view's fetch is outstanding.
#[tokio::test]
async fn a_pending_view_does_not_block_a_sibling_in_the_same_group() {
    hang_budget(async {
        let group =
            bobcat_core::LynxGroup::new(Arc::new(NoWakeup), bobcat_core::StyleThreads::Sequential)
                .await
                .expect("group");
        let (started_sender, mut started) = tokio::sync::oneshot::channel();
        let (dropped_sender, dropped) = flume::unbounded();
        let fetcher = Rc::new(PendingFetcher {
            base: FetcherDouble::new(Vec::new()),
            started: Mutex::new(Some(started_sender)),
            dropped: Mutex::new(Some(dropped_sender)),
            pending: Mutex::new(None),
        });
        // No painter: this view's entry never arrives, so it never reaches
        // the first flush that would wait for one.
        let mut pending = group
            .create_lynx_view(
                32.0,
                24.0,
                1.0,
                |_| Rc::clone(&fetcher),
                Vec::new(),
                ViewSources::new("app:///", "app:///pending.js", SCREEN),
            )
            .expect("pending view");
        // Issued inside the construction above, so the fetcher is already
        // holding it and no turn of this view's is needed to get there.
        started
            .try_recv()
            .expect("creation handed the entry to the fetcher");
        assert!(pending.pump().is_empty());
        let mut sibling = group
            .create_lynx_view(
                32.0,
                24.0,
                1.0,
                |_| FetcherDouble::new(Vec::new()),
                Vec::new(),
                ViewSources::new("app:///", "app:///sibling.js", SCREEN),
            )
            .expect("sibling view");
        let mut sibling_painter = bobcat_core::Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
            .await
            .expect("the sibling's painter is built");
        sibling_painter
            .attach(&sibling)
            .expect("a fresh view takes a painter");
        wait_for_script(&mut sibling).expect("sibling boots while first fetch stays pending");
        let completion = fetcher.pending.lock().unwrap().take().unwrap();
        drop(pending);
        assert!(completion.is_cancelled());
        completion.complete(Ok(bobcat_core::resource::LoadedSource::Module {
            source: "throw new Error('late source must not run')".into(),
            url: "app:///late.js".into(),
        }));
        drop(fetcher);
        assert_eq!(
            dropped.recv().expect("pending fetch cancelled"),
            thread_tag()
        );
        sibling_painter
            .tick(true)
            .expect("sibling still runs after cancellation");
    })
    .await;
}

/// A host that answers worker scripts with something other than the entry,
/// which the shared double — one payload for every source — cannot do.
struct TwoScriptFetcher {
    base: FetcherDouble,
    worker: &'static str,
}

impl bobcat_core::FrameImages for TwoScriptFetcher {
    fn read(
        &self,
        source: &str,
        hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::vello::peniko::ImageData> {
        self.base.read(source, hint)
    }

    fn retain(&self, frame: &[Arc<str>]) {
        self.base.retain(frame);
    }
}

impl ResourceFetcher for TwoScriptFetcher {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        // The entry is a `Module` request too, so the worker script is told
        // apart by the URL Rust joined `./worker.js` to: the entry's response
        // URL is `app:///main.js`.
        if matches!(&request, SourceRequest::Module(url) if url == "app:///worker.js") {
            completion.complete(Ok(bobcat_core::resource::LoadedSource::Module {
                source: self.worker.to_owned(),
                url: "app:///worker.js".to_owned(),
            }));
            return;
        }
        completion.complete(self.base.load_source(request));
    }
}

/// The entry the view in the test below boots: one worker whose interval
/// throws, so that the worker's liveness is observable from the embedder.
const WORKER_ENTRY: &str = "import { Worker } from 'bobcat-internal';
     globalThis.worker = new Worker('./worker.js');
     globalThis.renderPage = function () { __CreatePage('card', 0); };";

/// Pumps until a worker of this view reports a throw carrying `message`,
/// which for the test below is its interval callback throwing: proof that the
/// worker booted and that its timer is armed and firing, rather than that some
/// other worker of the view's went wrong. A throw rather than an end, because
/// the worker that threw is still running.
///
/// A fraction of [`HANG_BUDGET`], because this runs inside the thread the
/// budget is watching: a worker that never boots has to fail here, naming the
/// worker, rather than run the outer wait out and be reported as a teardown
/// that would not return.
fn wait_for_worker_error<F: ResourceFetcher + 'static>(
    view: &mut bobcat_core::LynxView<F>,
    message: &str,
) {
    let deadline = std::time::Instant::now() + HANG_BUDGET / 4;
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::WorkerThrew { error, .. } => {
                    assert!(
                        error.to_string().contains(message),
                        "unexpected worker failure: {error}"
                    );
                    return;
                }
                EngineEvent::WorkerEnded { error, .. } => panic!("the worker ended: {error}"),
                _ => {}
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the worker's interval never fired"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// What a panic on the teardown thread said, so the failure the test reports
/// is the one that happened rather than the wait that noticed it.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<String>().map_or_else(
        || {
            payload
                .downcast_ref::<&str>()
                .map_or_else(|| "a panic with no message".to_owned(), ToString::to_string)
        },
        Clone::clone,
    )
}

/// A group is two threads, and dropping the group handle joins both of them
/// within the deadline while a worker with an armed interval exists.
///
/// That is all this checks. The worker arms an interval that throws, which is
/// what makes its liveness observable from the embedder: every tick is a
/// nonfatal `WorkerThrew`, so reaching the drops means a worker is running
/// and would go on running. Why its task then ends — the `Terminate` its realm
/// sends, or the channel closing behind that message — is not something the
/// two joins returning can tell apart.
///
/// A plain `#[test]` on a thread of its own because both joins are blocking:
/// a deadline around a future cannot fire inside one, so the wait that fails
/// this test has to be on a different thread from the joins it is watching.
/// That is also why the body catches its own panic and sends the message
/// across: an assertion inside the thread would otherwise reach the test only
/// as a channel that never delivered, which reads as a teardown hang.
#[test]
fn dropping_the_group_joins_both_of_its_threads() {
    let (finished, teardown) = flume::bounded(1);
    std::thread::Builder::new()
        .name("group-teardown".to_owned())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("a per-thread runtime");
                runtime.block_on(async {
                    let group = bobcat_core::LynxGroup::new(
                        Arc::new(NoWakeup),
                        bobcat_core::StyleThreads::Sequential,
                    )
                    .await
                    .expect("the group starts");
                    let mut view = group
                        .create_lynx_view(
                            32.0,
                            24.0,
                            1.0,
                            |_| {
                                Rc::new(TwoScriptFetcher {
                                    base: FetcherDouble::new(WORKER_ENTRY.as_bytes().to_vec())
                                        .resolving_to("app:///main.js"),
                                    worker: "import 'bobcat:worker'; import 'bobcat:timers'; \
                                             setInterval(() => { throw new Error('tick'); }, 10);",
                                })
                            },
                            Vec::new(),
                            ViewSources::new("app:///", "app:///main.js", SCREEN),
                        )
                        .expect("the view is created");
                    // Boot's first flush waits for a painter to bind the
                    // view, so this teardown needs one before it can wait for
                    // the entry.
                    let mut painter =
                        bobcat_core::Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
                            .await
                            .expect("the painter is built");
                    painter.attach(&view).expect("a fresh view takes a painter");
                    wait_for_script(&mut view).expect("the entry boots");
                    wait_for_worker_error(&mut view, "tick");
                    drop(painter);
                    drop(view);
                    drop(group);
                });
            });
            let _ = finished.send(outcome.map_err(|payload| panic_text(payload.as_ref())));
        })
        .expect("the teardown thread starts");
    match teardown.recv_timeout(HANG_BUDGET) {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => panic!("the teardown thread failed before its joins: {panic}"),
        Err(error) => panic!("dropping the group did not join both of its threads: {error}"),
    }
}
