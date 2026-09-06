//! `Worker`, through the public facade: a real view, a real painter fetch,
//! and the group's second `QuickJS` runtime on the far side of it.
//!
//! The realm-level behaviour — what a `Worker` object does, what a worker's
//! global scope has, what happens when one closes or throws — is asserted
//! inside the realm by `bobcat-core`'s own unit tests. What only a whole view
//! can show is asserted here: that a script named by `new Worker(...)` is
//! fetched by the thread that owns the fetcher, that a fetch finishing between
//! turns wakes the host, and that what the worker answers reaches the
//! document.

mod support;

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    CacheStatus, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceErrorKind, ResourceErrorPhase, ResourceFetcher, ResourceLocality, ResourceMetadata,
    ResourceRequest, ResourceResponse, ResourceSource, ResourceTiming, RetryAdvice,
};
use bobcat_core::{
    DrawTarget, EngineEvent, EventRequester, ImageSizeHint, LynxView, ViewSources, vello,
};
use bytes::Bytes;
use support::{solo_view, wait_for_script};
use url::Url;

const ENTRY_URL: &str = "app:///main.js";
const WORKER_URL: &str = "app:///worker.js";

/// The entry: it constructs one worker, asks it for a colour, and paints the
/// page with whatever comes back.
const ENTRY: &str = r#"
globalThis.renderPage = function renderPage() {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'background-color:#ff0000');
  const worker = new Worker("app:///worker.js");
  worker.onmessage = function (event) {
    __SetInlineStyles(page, 'background-color:' + event.data.colour);
    __FlushElementTree();
  };
  worker.postMessage({ want: 'colour' });
};
"#;

/// The worker: an ordinary ESM, with no import preamble and no document.
const WORKER: &str = r"
onmessage = (event) => {
  postMessage({ colour: event.data.want === 'colour' ? '#00ff00' : '#0000ff' });
};
";

/// A host event loop, reduced to the one bit an event loop is: a turn has
/// been asked for, and has not been taken yet.
///
/// A flag rather than a counter, for the same reason a real event loop is
/// one — a wakeup that arrives while the host is mid-turn must still be
/// there when that turn ends. It exists at all because of the one thing a
/// synchronous fetcher could never show: a worker script that finishes
/// loading *between* turns has to reach a turn somehow, and this is the only
/// wakeup the engine has.
#[derive(Debug, Default)]
struct PendingTurn(AtomicBool);

impl PendingTurn {
    /// Takes the pending turn, if one was asked for.
    fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

impl EventRequester for PendingTurn {
    fn request_event(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// A fetch that is not ready the first time it is polled.
///
/// It answers `Pending`, then completes on a thread of its own and wakes the
/// waker it was given — which is exactly what a real host's transport does,
/// and the only shape that exercises the painter's off-turn path.
struct Later {
    ready: Arc<AtomicBool>,
    armed: bool,
}

impl Later {
    fn new() -> Self {
        Self {
            ready: Arc::new(AtomicBool::new(false)),
            armed: false,
        }
    }
}

impl Future for Later {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.ready.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        if !self.armed {
            self.armed = true;
            let ready = Arc::clone(&self.ready);
            let waker = context.waker().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                ready.store(true, Ordering::Release);
                waker.wake();
            });
        }
        Poll::Pending
    }
}

/// Serves a fixed set of URLs, and makes the worker's script arrive late.
#[derive(Debug)]
struct Routes {
    entry: Vec<u8>,
    worker: Option<Vec<u8>>,
    /// Every URL `fetch_resource` was asked for, in order.
    fetched: Mutex<Vec<String>>,
}

impl Routes {
    fn new(worker: Option<&str>) -> Self {
        Self {
            entry: ENTRY.as_bytes().to_vec(),
            worker: worker.map(|source| source.as_bytes().to_vec()),
            fetched: Mutex::new(Vec::new()),
        }
    }

    fn fetched(&self) -> Vec<String> {
        self.fetched.lock().expect("the fetch log").clone()
    }
}

impl ResourceFetcher for Routes {
    fn supports_capability(&self, capability: ResourceCapability) -> bool {
        capability == ResourceCapability::BufferedResource
    }

    async fn resolve_locator(
        &self,
        request: ResolveRequest,
    ) -> Result<ResolvedLocator, ResourceError> {
        let url = Url::parse(&request.resource.specifier).expect("an absolute test URL");
        Ok(ResolvedLocator {
            resource: request.resource,
            url,
            rewrite_chain: Vec::new(),
            locality: ResourceLocality::Remote,
            cache_key: None,
        })
    }

    async fn fetch_resource(
        &self,
        request: ResourceRequest,
    ) -> Result<ResourceResponse, ResourceError> {
        let url = request.resource.url.to_string();
        self.fetched
            .lock()
            .expect("the fetch log")
            .push(url.clone());
        let bytes = if url == WORKER_URL {
            // Not ready yet: the painter must come back for it.
            Later::new().await;
            self.worker.clone().ok_or_else(|| ResourceError {
                request_id: Some(request.context.id),
                kind: ResourceErrorKind::NotFound,
                phase: ResourceErrorPhase::ReceiveHeaders,
                locator: Some(request.resource.resource.specifier.clone()),
                status: None,
                message: "no such worker script".into(),
                retry: RetryAdvice::Never,
            })?
        } else {
            self.entry.clone()
        };
        Ok(ResourceResponse {
            metadata: ResourceMetadata {
                request_id: request.context.id,
                resource: request.resource,
                headers: http::HeaderMap::new(),
                content_length: Some(bytes.len() as u64),
                media_type: None,
                source: ResourceSource::Custom,
                cache_status: CacheStatus::Miss,
                timing: ResourceTiming::default(),
            },
            bytes: Bytes::from(bytes),
        })
    }
}

impl bobcat_core::FrameImages for Routes {
    fn read(&self, _source: &str, _hint: ImageSizeHint) -> Option<vello::peniko::ImageData> {
        None
    }
}

async fn booted(wakeup: &Arc<PendingTurn>, routes: &Rc<Routes>) -> LynxView<Rc<Routes>> {
    let routes = Rc::clone(routes);
    let mut view = solo_view(
        Arc::clone(wakeup),
        16.0,
        12.0,
        1.0,
        DrawTarget::Offscreen,
        move |_sink| routes,
        ViewSources::new(ENTRY_URL),
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    view
}

/// Runs the view the way a host does — a turn when the engine asks for one,
/// and a turn while it still owes a frame — until `done` is satisfied.
///
/// Pumping on a timer instead would hide the thing this file exists to check:
/// a fetch that completes off-turn has to ask for the turn that applies it,
/// and nothing else here would ever ask.
fn drive(
    view: &mut LynxView<Rc<Routes>>,
    wakeup: &PendingTurn,
    mut done: impl FnMut(&mut LynxView<Rc<Routes>>, &[EngineEvent]) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            Instant::now() < deadline,
            "the engine never asked for the turn that would finish this"
        );
        if !wakeup.take() && !view.owes_frame() {
            // Sleeping rather than spinning: this thread shares its cores
            // with the group's own, and a spin here starves the very threads
            // it is waiting on.
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let events = view.pump();
        if done(view, &events) {
            return;
        }
    }
}

#[tokio::test]
async fn a_worker_script_is_fetched_by_the_painter_and_its_answer_reaches_the_document() {
    let wakeup = Arc::new(PendingTurn::default());
    let routes = Rc::new(Routes::new(Some(WORKER)));
    let mut view = booted(&wakeup, &routes).await;

    // Red until the worker answers: the entry painted the page before it
    // constructed one, and the script had not even been fetched yet.
    let before = view.capture().expect("capture the un-answered page");
    assert_eq!(count(&before.pixels, [255, 0, 0, 255]), 16 * 12);

    drive(&mut view, &wakeup, |view, _events| {
        let shot = view.capture().expect("capture");
        count(&shot.pixels, [0, 255, 0, 255]) == 16 * 12
    });

    assert_eq!(
        routes.fetched(),
        vec![ENTRY_URL.to_owned(), WORKER_URL.to_owned()],
        "the entry and the worker script are both the painter's fetches"
    );
}

#[tokio::test]
async fn a_worker_script_that_cannot_be_fetched_is_reported_and_the_view_goes_on() {
    let wakeup = Arc::new(PendingTurn::default());
    let routes = Rc::new(Routes::new(None));
    let mut view = booted(&wakeup, &routes).await;

    let mut failure = None;
    drive(&mut view, &wakeup, |_view, events| {
        for event in events {
            if let EngineEvent::WorkerFailed(error) = event {
                failure = Some(error.to_string());
            }
            assert!(
                !matches!(event, EngineEvent::ScriptRunError(_)),
                "a worker that will not load is not a failure of the view"
            );
        }
        failure.is_some()
    });

    let failure = failure.expect("the worker reported its failure");
    assert!(failure.contains("no such worker script"), "{failure}");

    // The page the entry painted before it constructed the worker is
    // untouched: nothing about the failure reached the document.
    let shot = view.capture().expect("capture the surviving page");
    assert_eq!(count(&shot.pixels, [255, 0, 0, 255]), 16 * 12);
}

fn count(pixels: &[u8], wanted: [u8; 4]) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|pixel| *pixel == wanted)
        .count()
}
