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
        ViewSources::new("main.js"),
    )
    .await
    .expect("startup completes");

    assert!(
        records.lock().expect("thread records").is_empty(),
        "creation performs no fetch"
    );
    awakened
        .recv_timeout(HANG_BUDGET)
        .expect("main requests the entry");
    assert!(view.pump().is_empty(), "the IO is still held pending");
    assert!(
        awakened.try_recv().is_err(),
        "no other main notification can wake the next turn"
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
        ViewSources::new("main.js"),
    )
    .await
    .expect("creation returns a loading view even when the fetch never answers");
    assert!(matches!(
        started.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    loop {
        assert!(view.pump().is_empty());
        if started.try_recv().is_ok() {
            break;
        }
        tokio::task::yield_now().await;
    }
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

/// Metrics that arrive while the entry fetch is outstanding have no document
/// to write into — the boot module has not created one yet — so the view's
/// task writes them into the ingredients instead, and the document the boot
/// module then creates is the resized one.
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
            ViewSources::new("main.js"),
        )
        .await
        .expect("creation returns a loading view");
        loop {
            assert!(view.pump().is_empty());
            if started.try_recv().is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }

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
        completion.complete(Ok(bobcat_core::resource::LoadedSource::Entry {
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

/// Configuration errors are lifecycle events even when no source was requested.
#[tokio::test]
async fn an_unknown_font_family_reports_failure_without_fetching() {
    hang_budget(async {
        let fetcher = Rc::new(FetcherDouble::new(Vec::new()));
        let (mut view, _painter) = solo_view(
            Arc::new(NoWakeup),
            32.0,
            24.0,
            1.0,
            DrawTarget::Offscreen,
            |_| fetcher.clone(),
            ViewSources {
                default_font_family: Some("no-such-family".to_owned()),
                ..ViewSources::new("main.js")
            },
        )
        .await
        .expect("loading view exists");
        let error = wait_for_script(&mut view).expect_err("unknown family fails boot");
        assert!(error.to_string().contains("no-such-family"));
        assert_eq!(fetcher.fetch_count(), 0);
        assert!(view.pump().is_empty(), "failure is delivered once");
    })
    .await;
}

#[tokio::test]
async fn a_resource_resolution_failure_is_an_event_and_stops_further_sources() {
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
            ..ViewSources::new("main.js")
        },
    )
    .await
    .expect("resource failure does not prevent construction");
    assert_eq!(fetcher.resolve_count(), 0);
    assert!(matches!(
        wait_for_script(&mut view),
        Err(bobcat_core::LynxViewError::Resource(_))
    ));
    assert_eq!(
        fetcher.resolve_count(),
        1,
        "main stops requesting sources after failure"
    );
    assert_eq!(fetcher.fetch_count(), 0);
    assert!(view.pump().is_empty());
}

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
        let mut pending = group
            .create_lynx_view(
                32.0,
                24.0,
                1.0,
                |_| Rc::clone(&fetcher),
                ViewSources::new("pending.js"),
            )
            .expect("pending view");
        loop {
            assert!(pending.pump().is_empty());
            if started.try_recv().is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let mut sibling = group
            .create_lynx_view(
                32.0,
                24.0,
                1.0,
                |_| FetcherDouble::new(Vec::new()),
                ViewSources::new("sibling.js"),
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
        completion.complete(Ok(bobcat_core::resource::LoadedSource::Entry {
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
        if matches!(request, SourceRequest::Worker { .. }) {
            completion.complete(Ok(bobcat_core::resource::LoadedSource::Entry {
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

/// Pumps until a worker of this view reports a failure carrying `message`,
/// which for the test below is its interval callback throwing: proof that the
/// worker booted and that its timer is armed and firing, rather than that some
/// other worker of the view's went wrong.
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
            if let EngineEvent::WorkerFailed(error) = event {
                assert!(
                    error.to_string().contains(message),
                    "unexpected worker failure: {error}"
                );
                return;
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
/// nonfatal `WorkerFailed`, so reaching the drops means a worker is running
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
                                    worker: "setInterval(() => { throw new Error('tick'); }, 10);",
                                })
                            },
                            ViewSources::new("main.js"),
                        )
                        .expect("the view is created");
                    wait_for_script(&mut view).expect("the entry boots");
                    wait_for_worker_error(&mut view, "tick");
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
