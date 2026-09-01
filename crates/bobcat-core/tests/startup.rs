//! Startup ownership and cancellation across a view's two threads: the one
//! that constructed it, which paints, and `bobcat-main`.

mod support;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use bobcat_core::resource::{
    HttpRequest, HttpResponse, ResolveRequest, ResolvedLocator, ResourceCapability,
    ResourceFetcher, ResourceFuture, ResourceRequest, ResourceResponse,
};
use bobcat_core::{DrawTarget, EventRequester, LynxView, NoWakeup, PageConfig, ViewSources};
use support::{FetcherDouble, wait_for_script};

fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned()
}

struct HopState {
    ready: AtomicBool,
    started: AtomicBool,
    waker: Mutex<Option<Waker>>,
    records: Arc<Mutex<Vec<(String, String)>>>,
}

struct ThreadHop {
    state: Arc<HopState>,
}

impl Future for ThreadHop {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.state
            .records
            .lock()
            .expect("thread records")
            .push(("poll".to_owned(), thread_name()));
        if self.state.ready.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        *self.state.waker.lock().expect("hop waker") = Some(context.waker().clone());
        if !self.state.started.swap(true, Ordering::AcqRel) {
            let state = Arc::clone(&self.state);
            std::thread::Builder::new()
                .name("fetch-io".to_owned())
                .spawn(move || {
                    state.ready.store(true, Ordering::Release);
                    if let Some(waker) = state.waker.lock().expect("hop waker").take() {
                        waker.wake();
                    }
                })
                .expect("IO worker starts");
        }
        Poll::Pending
    }
}

struct ThreadedFetcher {
    base: FetcherDouble,
    records: Arc<Mutex<Vec<(String, String)>>>,
}

impl ThreadedFetcher {
    fn record(&self, phase: &str) {
        self.records
            .lock()
            .expect("thread records")
            .push((phase.to_owned(), thread_name()));
    }
}

impl ResourceFetcher for ThreadedFetcher {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        self.base.supports_capability(capability)
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        self.record("resolve");
        self.base.resolve_locator(request)
    }

    fn fetch_resource(&self, request: ResourceRequest) -> ResourceFuture<'_, ResourceResponse> {
        self.record("fetch");
        let response = self.base.fetch_resource(request);
        let state = Arc::new(HopState {
            ready: AtomicBool::new(false),
            started: AtomicBool::new(false),
            waker: Mutex::new(None),
            records: Arc::clone(&self.records),
        });
        Box::pin(async move {
            ThreadHop { state }.await;
            response.await
        })
    }

    fn fetch_http(&self, request: HttpRequest) -> ResourceFuture<'_, HttpResponse> {
        self.base.fetch_http(request)
    }
}

#[tokio::test]
async fn resource_continuations_and_boot_stay_on_bobcat_main() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let fetcher = Arc::new(ThreadedFetcher {
        base: FetcherDouble::new(Vec::new()).resolving_to("app:///main.js"),
        records: Arc::clone(&records),
    });
    let sources = ViewSources::new(fetcher, "main.js");
    let mut view = LynxView::new(
        PageConfig::default(),
        Arc::new(NoWakeup),
        393.0,
        727.0,
        1.0,
        DrawTarget::Offscreen,
        sources,
    )
    .await
    .expect("startup completes");

    let records = records.lock().expect("thread records");
    assert!(
        records.iter().any(|(phase, _)| phase == "resolve"),
        "the locator was resolved"
    );
    assert!(
        records.iter().filter(|(phase, _)| phase == "poll").count() >= 2,
        "the resource future yielded and resumed"
    );
    assert!(
        records
            .iter()
            .all(|(_, owner)| owner.as_str() == "bobcat-main"),
        "every fetch call and continuation belongs to bobcat-main: {records:?}"
    );
    drop(records);
    wait_for_script(&mut view)
        .expect("new returns only after boot and preserves the successful lifecycle event");
}

struct PendingResource {
    started: Option<tokio::sync::oneshot::Sender<()>>,
    dropped: Option<mpsc::Sender<String>>,
}

impl Future for PendingResource {
    type Output = Result<ResourceResponse, bobcat_core::resource::ResourceError>;

    fn poll(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(started) = self.started.take() {
            let _ = started.send(());
        }
        Poll::Pending
    }
}

impl Drop for PendingResource {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(thread_name());
        }
    }
}

struct PendingFetcher {
    base: FetcherDouble,
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    dropped: Mutex<Option<mpsc::Sender<String>>>,
}

impl ResourceFetcher for PendingFetcher {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        self.base.supports_capability(capability)
    }

    fn resolve_locator(&self, request: ResolveRequest) -> ResourceFuture<'_, ResolvedLocator> {
        self.base.resolve_locator(request)
    }

    fn fetch_resource(&self, _request: ResourceRequest) -> ResourceFuture<'_, ResourceResponse> {
        Box::pin(PendingResource {
            started: self.started.lock().expect("start signal").take(),
            dropped: self.dropped.lock().expect("drop signal").take(),
        })
    }

    fn fetch_http(&self, request: HttpRequest) -> ResourceFuture<'_, HttpResponse> {
        self.base.fetch_http(request)
    }
}

#[derive(Debug)]
struct DropObservedRequester;

impl EventRequester for DropObservedRequester {
    fn request_event(&self) {}
}

#[tokio::test]
async fn cancelling_new_drops_the_resource_future_and_reaps_the_main_thread() {
    let (started_sender, started) = tokio::sync::oneshot::channel();
    let (dropped_sender, dropped) = mpsc::channel();
    let fetcher = Arc::new(PendingFetcher {
        base: FetcherDouble::new(Vec::new()).resolving_to("app:///main.js"),
        started: Mutex::new(Some(started_sender)),
        dropped: Mutex::new(Some(dropped_sender)),
    });
    let fetcher_weak = Arc::downgrade(&fetcher);
    let requester = Arc::new(DropObservedRequester);
    let requester_weak = Arc::downgrade(&requester);
    let mut construction = Box::pin(LynxView::new(
        PageConfig::default(),
        requester,
        393.0,
        727.0,
        1.0,
        DrawTarget::Offscreen,
        ViewSources::new(fetcher, "main.js"),
    ));

    tokio::select! {
        result = &mut construction => panic!("pending resource unexpectedly completed: {result:?}"),
        signal = started => signal.expect("the resource future is polled"),
        () = tokio::time::sleep(Duration::from_secs(10)) => panic!("resource startup timed out"),
    }
    drop(construction);

    assert_eq!(
        dropped
            .recv_timeout(Duration::from_secs(10))
            .expect("cancellation drops the resource future"),
        "bobcat-main"
    );
    assert!(
        fetcher_weak.upgrade().is_none(),
        "bobcat-main released its owned fetcher"
    );
    assert!(
        requester_weak.upgrade().is_none(),
        "bobcat-main exited and released its requester"
    );
}
