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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    CacheStatus, ResolveRequest, ResolvedLocator, ResourceCapability, ResourceError,
    ResourceFetcher, ResourceLocality, ResourceMetadata, ResourceRequest, ResourceResponse,
    ResourceSource, ResourceTiming, ScriptReports, ScriptRequest, ScriptRequestId,
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

/// A miniature of a real host: it serves the entry through the awaiting half
/// of the protocol, which is what construction uses, and worker scripts
/// through the pushing half, which is what everything after construction uses.
///
/// Deliberately faithful about the part that matters — the load finishes on a
/// thread of its own, *between* the painter's turns, and this host rings the
/// view's wakeup itself. Nothing in the engine polls it and nothing in the
/// engine wakes for it.
struct Routes {
    entry: Vec<u8>,
    worker: Option<Vec<u8>>,
    /// Every URL the host was asked to load, in order.
    fetched: Mutex<Vec<String>>,
    /// Where finished script loads are handed to the engine, drained in the
    /// host's own moment in a painter turn.
    scripts: ScriptReports,
    completions: (flume::Sender<ScriptDone>, flume::Receiver<ScriptDone>),
    /// The same wakeup the view was built with. A host that reports between
    /// turns and does not ring this has reported into a void.
    wakeup: Arc<PendingTurn>,
}

struct ScriptDone {
    id: ScriptRequestId,
    result: Result<(String, Vec<u8>), String>,
}

impl Routes {
    fn new(worker: Option<&str>, scripts: ScriptReports, wakeup: Arc<PendingTurn>) -> Self {
        Self {
            entry: ENTRY.as_bytes().to_vec(),
            worker: worker.map(|source| source.as_bytes().to_vec()),
            fetched: Mutex::new(Vec::new()),
            scripts,
            completions: flume::unbounded(),
            wakeup,
        }
    }

    fn fetched(&self) -> Vec<String> {
        self.fetched.lock().expect("the fetch log").clone()
    }

    fn log(&self, url: &str) {
        self.fetched
            .lock()
            .expect("the fetch log")
            .push(url.to_owned());
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

    /// The entry, during construction, driven by the embedder's own executor.
    async fn fetch_resource(
        &self,
        request: ResourceRequest,
    ) -> Result<ResourceResponse, ResourceError> {
        let url = request.resource.url.to_string();
        self.log(&url);
        let bytes = self.entry.clone();
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

    /// A worker script, at whatever moment its `Worker` was constructed.
    fn request_script(&self, request: ScriptRequest) {
        let url = request.specifier.to_string();
        self.log(&url);
        let id = request.id;
        let body = self.worker.clone();
        let completions = self.completions.0.clone();
        let wakeup = Arc::clone(&self.wakeup);
        std::thread::spawn(move || {
            // Long enough that the turn which asked has certainly ended: the
            // answer lands between turns, which is the case worth testing.
            std::thread::sleep(Duration::from_millis(20));
            let result = body
                .map(|bytes| (url, bytes))
                .ok_or_else(|| "no such worker script".to_owned());
            let _ = completions.send(ScriptDone { id, result });
            wakeup.request_event();
        });
    }

    fn service_loads(&self) {
        for done in self.completions.1.try_iter() {
            match done.result {
                Ok((url, bytes)) => self.scripts.loaded(done.id, &url, Bytes::from(bytes)),
                Err(message) => self.scripts.failed(done.id, &message),
            }
        }
    }
}

impl bobcat_core::FrameImages for Routes {
    fn read(&self, _source: &str, _hint: ImageSizeHint) -> Option<vello::peniko::ImageData> {
        None
    }
}

/// Builds the view, and hands back the host it was built with — which cannot
/// exist before the view does, since it is built from the view's own sinks.
async fn booted(
    wakeup: &Arc<PendingTurn>,
    worker: Option<&'static str>,
) -> (LynxView<Rc<Routes>>, Rc<Routes>) {
    let built: Rc<RefCell<Option<Rc<Routes>>>> = Rc::new(RefCell::new(None));
    let out = Rc::clone(&built);
    let wake = Arc::clone(wakeup);
    let mut view = solo_view(
        Arc::clone(wakeup),
        16.0,
        12.0,
        1.0,
        DrawTarget::Offscreen,
        move |reports| {
            let routes = Rc::new(Routes::new(worker, reports.scripts, wake));
            *out.borrow_mut() = Some(Rc::clone(&routes));
            routes
        },
        ViewSources::new(ENTRY_URL),
    )
    .await
    .expect("view");
    wait_for_script(&mut view).expect("script execution");
    let routes = built.borrow_mut().take().expect("the host was built");
    (view, routes)
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
    let (mut view, routes) = booted(&wakeup, Some(WORKER)).await;

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
    let (mut view, _routes) = booted(&wakeup, None).await;

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
