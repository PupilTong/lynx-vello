//! Startup ownership and cancellation across a view's two threads: the one
//! that constructed it, which paints, and `bobcat-main`.

mod support;

use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobcat_core::resource::{
    ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError, ResourceFetcher,
    ResourceRequest, ResourceResponse, SourceCompletion, SourceRequest,
};
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
}

impl ResourceFetcher for ThreadedFetcher {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        self.base.supports_capability(capability)
    }

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

    async fn resolve_locator(&self, _: ResolveRequest) -> Result<ResolvedLocator, ResourceError> {
        panic!("core must not resolve sources")
    }

    async fn fetch_resource(&self, _: ResourceRequest) -> Result<ResourceResponse, ResourceError> {
        panic!("core must not poll resource futures")
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
    let mut view = solo_view(
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
}

impl ResourceFetcher for PendingFetcher {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        self.base.supports_capability(capability)
    }

    fn request_source(&self, _request: SourceRequest, completion: SourceCompletion) {
        *self.pending.lock().expect("pending completion") = Some(completion);
        if let Some(started) = self.started.lock().expect("start signal").take() {
            let _ = started.send(());
        }
    }

    async fn resolve_locator(&self, _: ResolveRequest) -> Result<ResolvedLocator, ResourceError> {
        panic!("core must not resolve sources")
    }

    async fn fetch_resource(&self, _: ResourceRequest) -> Result<ResourceResponse, ResourceError> {
        panic!("core must not poll resource futures")
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
const HANG_BUDGET: Duration = Duration::from_mins(2);

/// Fails `future` with a clear message if it has not finished within
/// [`HANG_BUDGET`].
///
/// Every wall-clock deadline in this file lives here, once, around a whole
/// test. Per-step deadlines are the thing to avoid: they turn "this step was
/// slower than I guessed" into a failure, and there is no step here whose
/// duration is a property worth asserting.
///
/// What it can and cannot see is worth stating, because it is what makes this
/// safe. `timeout` polls the inner future before it consults the clock, so
/// time spent inside one *synchronously blocking* poll — building the
/// offscreen GPU target, which is most of a startup test's wall clock — is
/// invisible to it and cannot expire it. An async stall is not: a future
/// parked on a signal that never arrives yields, the runtime reaches the
/// timer, and this fires. That asymmetry is the whole point. It detects the
/// deadlock it is for and structurally cannot fail a machine that was only
/// slow. Verified by shrinking the budget to 1ms: the tests still passed,
/// and only an added async stall tripped it.
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
    let mut view = solo_view(
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
    view.resize(400.0, 800.0, 1.0)
        .expect("resize while loading");
    view.tick(false).expect("tick while loading");
    drop(view);

    // Cancellation is visible before the painter releases its concrete fetcher.
    assert_eq!(
        dropped
            .recv()
            .expect("cancellation releases the source completion"),
        thread_tag()
    );
    assert!(
        fetcher_weak.upgrade().is_none(),
        "the painter released its owned fetcher"
    );
    assert!(
        requester_weak.upgrade().is_none(),
        "bobcat-main exited and released its requester"
    );
}

/// Configuration errors are lifecycle events even when no source was requested.
#[tokio::test]
async fn an_unknown_font_family_reports_failure_without_fetching() {
    hang_budget(async {
        let fetcher = Rc::new(FetcherDouble::new(Vec::new()));
        let mut view = solo_view(
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
    let mut view = solo_view(
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
                DrawTarget::Offscreen,
                |_| Rc::clone(&fetcher),
                ViewSources::new("pending.js"),
            )
            .await
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
                DrawTarget::Offscreen,
                |_| FetcherDouble::new(Vec::new()),
                ViewSources::new("sibling.js"),
            )
            .await
            .expect("sibling view");
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
        sibling
            .tick(true)
            .expect("sibling still runs after cancellation");
    })
    .await;
}
