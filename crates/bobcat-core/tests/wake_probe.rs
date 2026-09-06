//! TEMPORARY diagnostic (review scratch): where does the WorkerFailed event
//! actually get consumed, and when does the fetch wake arrive?
mod support;

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
use support::solo_view;
use url::Url;

const ENTRY_URL: &str = "app:///main.js";
const WORKER_URL: &str = "app:///worker.js";

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

static START: Mutex<Option<Instant>> = Mutex::new(None);

fn ms() -> u128 {
    let mut guard = START.lock().unwrap();
    let start = *guard.get_or_insert_with(Instant::now);
    start.elapsed().as_millis()
}

#[derive(Debug, Default)]
struct PendingTurn {
    flag: AtomicBool,
    wakes: AtomicUsize,
}

impl PendingTurn {
    fn take(&self) -> bool {
        self.flag.swap(false, Ordering::AcqRel)
    }
}

impl EventRequester for PendingTurn {
    fn request_event(&self) {
        self.wakes.fetch_add(1, Ordering::Relaxed);
        self.flag.store(true, Ordering::Release);
    }
}

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
            eprintln!("[{}ms] Later polled -> READY", ms());
            return Poll::Ready(());
        }
        if !self.armed {
            self.armed = true;
            eprintln!("[{}ms] Later polled -> PENDING, arming", ms());
            let ready = Arc::clone(&self.ready);
            let waker = context.waker().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                ready.store(true, Ordering::Release);
                eprintln!("[{}ms] Later thread: waker.wake()", ms());
                waker.wake();
            });
        }
        Poll::Pending
    }
}

#[derive(Debug)]
struct Routes {
    entry: Vec<u8>,
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
        eprintln!("[{}ms] fetch_resource {url}", ms());
        let bytes = if url == WORKER_URL {
            Later::new().await;
            eprintln!("[{}ms] worker fetch resolving to NotFound", ms());
            return Err(ResourceError {
                request_id: Some(request.context.id),
                kind: ResourceErrorKind::NotFound,
                phase: ResourceErrorPhase::ReceiveHeaders,
                locator: Some(request.resource.resource.specifier.clone()),
                status: None,
                message: "no such worker script".into(),
                retry: RetryAdvice::Never,
            });
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

#[tokio::test]
async fn probe() {
    let wakeup = Arc::new(PendingTurn::default());
    let routes = Rc::new(Routes {
        entry: ENTRY.as_bytes().to_vec(),
    });
    let handed = Rc::clone(&routes);
    eprintln!("[{}ms] creating view", ms());
    let mut view: LynxView<Rc<Routes>> = solo_view(
        Arc::clone(&wakeup),
        16.0,
        12.0,
        1.0,
        DrawTarget::Offscreen,
        move |_sink| handed,
        ViewSources::new(ENTRY_URL),
    )
    .await
    .expect("view");
    eprintln!("[{}ms] view built", ms());

    // The shipped `wait_for_script`: pumps unconditionally every 1ms and
    // DISCARDS every event that is not a script outcome.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut finished_at = None;
    loop {
        for event in view.pump() {
            eprintln!("[{}ms] BOOT-LOOP saw {event:?}", ms());
            if matches!(event, EngineEvent::ScriptFinished) {
                finished_at = Some(ms());
            }
        }
        if finished_at.is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "script never finished");
        std::thread::sleep(Duration::from_millis(1));
    }
    eprintln!(
        "[{}ms] boot loop ended (ScriptFinished at {:?}), wakes so far {}",
        ms(),
        finished_at,
        wakeup.wakes.load(Ordering::Relaxed)
    );

    // Now the shipped `drive`: only pumps when the engine asked.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut failure = None;
    while Instant::now() < deadline {
        if !wakeup.take() && !view.owes_frame() {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        for event in view.pump() {
            eprintln!("[{}ms] DRIVE saw {event:?}", ms());
            if let EngineEvent::WorkerFailed(error) = event {
                failure = Some(error.to_string());
            }
        }
        if failure.is_some() {
            break;
        }
    }
    eprintln!(
        "[{}ms] drive ended, failure={:?}, total wakes {}",
        ms(),
        failure,
        wakeup.wakes.load(Ordering::Relaxed)
    );
}
