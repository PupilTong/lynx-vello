//! Startup ownership and cancellation across a view's two threads: the one
//! that constructed it, which paints, and `bobcat-main`.

mod support;

use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobcat_core::resource::{ResourceFetcher, SourceCompletion, SourceRequest};
use bobcat_core::{DrawTarget, EngineEvent, EventRequester, NoWakeup, ViewSources};
use support::{FetcherDouble, solo_view, tick, wait_for_script};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

struct HostWakeup(flume::Sender<()>);
impl EventRequester for HostWakeup {
    fn request_event(&self) {
        let _ = self.0.send(());
    }
}

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
    type PlatformFrame = std::convert::Infallible;

    fn describe(
        &self,
        picture: &bobcat_core::PictureKey,
        hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::FrameImage> {
        self.base.describe(picture, hint)
    }

    fn want(&self, picture: &bobcat_core::PictureKey, hint: bobcat_core::ImageSizeHint) {
        self.base.want(picture, hint);
    }

    fn retain(&self, frame: &[bobcat_core::PictureKey]) {
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
        ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US"),
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
    type PlatformFrame = std::convert::Infallible;

    fn describe(
        &self,
        picture: &bobcat_core::PictureKey,
        hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::FrameImage> {
        self.base.describe(picture, hint)
    }

    fn want(&self, picture: &bobcat_core::PictureKey, hint: bobcat_core::ImageSizeHint) {
        self.base.want(picture, hint);
    }

    fn retain(&self, frame: &[bobcat_core::PictureKey]) {
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

const HANG_BUDGET: Duration = Duration::from_mins(2);

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
        ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US"),
    )
    .await
    .expect("creation returns a loading view even when the fetch never answers");
    started
        .try_recv()
        .expect("creation handed the entry to the fetcher");
    assert!(view.pump().is_empty());
    painter
        .resize(400.0, 800.0, 1.0)
        .expect("resize while loading");
    tick(&mut painter, false).expect("tick while loading");
    drop(view);

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
            ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US"),
        )
        .await
        .expect("creation returns a loading view");
        started
            .try_recv()
            .expect("creation handed the entry to the fetcher");

        painter
            .resize(400.0, 800.0, 2.0)
            .expect("resize while loading");
        tick(&mut painter, false).expect("tick while loading");
        let completion = fetcher
            .pending
            .lock()
            .expect("pending completion")
            .take()
            .expect("the entry fetch is outstanding");
        completion.complete(Ok(bobcat_core::resource::LoadedSource::Module {
            source: format!(
                "{}globalThis.renderPage = function () {{
               __SetInlineStyles(__CreatePage('card', 0), 'background-color:rgb(255,0,0)');
             }};",
                bobcat_core::MTS_CHUNK_PREAMBLE
            ),
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
                ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
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

#[tokio::test]
async fn an_unresolvable_url_fails_construction_without_fetching() {
    hang_budget(async {
        let cases = [
            (
                "not a URL",
                ViewSources::new("not a URL", "main.js", SCREEN, "en-US"),
            ),
            (
                "http://[",
                ViewSources::new("app:///", "http://[", SCREEN, "en-US"),
            ),
            (
                "http://[",
                ViewSources {
                    background_entry: Some("http://[".to_owned()),
                    ..ViewSources::new("app:///", "main.js", SCREEN, "en-US")
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
            ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
        },
    )
    .await
    .expect("resource failure does not prevent construction");
    assert_eq!(
        fetcher.resolve_count(),
        3,
        "creation hands over both sheets and the entry, before any turn"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut events = Vec::new();
    while !events.iter().any(EngineEvent::is_fatal) {
        assert!(
            std::time::Instant::now() < deadline,
            "the failure was never reported"
        );
        events.extend(view.pump());
        std::thread::sleep(Duration::from_millis(1));
    }
    let fatal = events
        .iter()
        .position(EngineEvent::is_fatal)
        .expect("the loop ran until one was there");
    let after = events.split_off(fatal + 1);
    assert!(after.is_empty(), "failure is delivered once: {after:?}");
    let Some(EngineEvent::StartupFailed(bobcat_core::LynxViewError::Resource(error))) =
        events.last()
    else {
        panic!("the fetcher's own error: {events:?}");
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
                Vec::new(),
                ViewSources::new("app:///", "app:///pending.js", SCREEN, "en-US"),
            )
            .expect("pending view");
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
                ViewSources::new("app:///", "app:///sibling.js", SCREEN, "en-US"),
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
        tick(&mut sibling_painter, true).expect("sibling still runs after cancellation");
    })
    .await;
}

struct TwoScriptFetcher {
    base: FetcherDouble,
    worker: &'static str,
}

impl bobcat_core::FrameImages for TwoScriptFetcher {
    type PlatformFrame = std::convert::Infallible;

    fn describe(
        &self,
        picture: &bobcat_core::PictureKey,
        hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::FrameImage> {
        self.base.describe(picture, hint)
    }

    fn want(&self, picture: &bobcat_core::PictureKey, hint: bobcat_core::ImageSizeHint) {
        self.base.want(picture, hint);
    }

    fn retain(&self, frame: &[bobcat_core::PictureKey]) {
        self.base.retain(frame);
    }
}

impl ResourceFetcher for TwoScriptFetcher {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
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

const WORKER_ENTRY: &str = "import { Worker } from 'bobcat-internal';
     globalThis.worker = new Worker('./worker.js');
     globalThis.renderPage = function () { __CreatePage('card', 0); };";

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
                                    base: FetcherDouble::card(WORKER_ENTRY)
                                        .resolving_to("app:///main.js"),
                                    worker: "import 'bobcat:worker'; import 'bobcat:timers'; \
                                             setInterval(() => { throw new Error('tick'); }, 10);",
                                })
                            },
                            Vec::new(),
                            ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US"),
                        )
                        .expect("the view is created");
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
