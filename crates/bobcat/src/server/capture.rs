//! Bounded admission and the dedicated thread that owns Bobcat screenshot
//! capture.
//!
//! `LynxView` is deliberately `!Send`: its handle and private painter stay on
//! the embedder thread that constructed them, while the engine's Lynx-main
//! thread owns the document and realm. HTTP tasks therefore enqueue plain
//! request data and receive only an owned RGBA screenshot back.

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use bobcat_core::{DrawTarget, EngineEvent, LynxView, NoWakeup, Screenshot};
use bobcat_resources::{Resources, ResourcesConfig, ViewResources};
use bobcat_source::PageSource;
use reqwest::Client;
use tokio::sync::oneshot;
use url::Url;

const MAX_QUEUED_CAPTURES: usize = 8;
const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const VIEWPORT_WIDTH: f32 = 800.0;
const VIEWPORT_HEIGHT: f32 = 600.0;
const DEVICE_PIXEL_RATIO: f32 = 1.0;

#[derive(Clone, Debug)]
pub(crate) struct CaptureRequest {
    pub(crate) screenshot_settle: Duration,
    pub(crate) timeout: Duration,
    pub(crate) url: Url,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CaptureFailure {
    #[error("headless {operation} timed out after {timeout_ms} ms")]
    Timeout {
        operation: &'static str,
        timeout_ms: u128,
    },
    #[error("could not read input `{url}`: {source}")]
    ReadFile {
        url: Url,
        #[source]
        source: io::Error,
    },
    #[error("could not fetch input `{url}`: {source}")]
    Fetch {
        url: Url,
        #[source]
        source: reqwest::Error,
    },
    #[error("could not fetch input `{url}`: HTTP {status}")]
    HttpStatus {
        url: Url,
        status: reqwest::StatusCode,
    },
    #[error("could not load input `{url}`: {source}")]
    Source {
        url: Url,
        #[source]
        source: Box<bobcat_source::SourceError>,
    },
    #[error("could not start input `{url}`: {source}")]
    StartView {
        url: Url,
        #[source]
        source: Box<bobcat_core::LynxViewError>,
    },
    #[error("could not render input `{url}`: {source}")]
    Render {
        url: Url,
        #[source]
        source: Box<bobcat_core::EngineError>,
    },
    #[error("could not run input `{url}`: {message}")]
    Script { url: Url, message: String },
}

impl CaptureFailure {
    fn timeout(operation: &'static str, timeout: Duration) -> Self {
        Self::Timeout {
            operation,
            timeout_ms: timeout.as_millis(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CaptureQueueError {
    #[error("The Bobcat capture queue is full; retry the request later.")]
    QueueFull,
    #[error("The Bobcat headless worker is unavailable.")]
    Unavailable,
    #[error("The Bobcat headless worker is shutting down.")]
    ShuttingDown,
    #[error("The Bobcat headless worker stopped before returning a result.")]
    Stopped,
}

#[derive(Debug, thiserror::Error)]
#[error("Bobcat headless worker panicked")]
pub(crate) struct WorkerPanicked;

pub(crate) struct CaptureJob {
    pub(crate) request: CaptureRequest,
    pub(crate) response: oneshot::Sender<Result<Screenshot, CaptureFailure>>,
}

#[derive(Debug)]
struct LoadedInput {
    bytes: Vec<u8>,
    url: Url,
}

/// One owner thread behind a bounded FIFO. Bobcat itself has no native Lynx
/// process-global renderer, but serial admission bounds GPU contexts and keeps
/// the `!Send` view handle's ownership obvious.
pub(crate) struct CaptureExecutor {
    failure_receiver: Mutex<Option<oneshot::Receiver<()>>>,
    healthy: Arc<AtomicBool>,
    jobs: Arc<Mutex<Receiver<CaptureJob>>>,
    sender: Arc<Mutex<Option<SyncSender<CaptureJob>>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl CaptureExecutor {
    pub(crate) fn new() -> io::Result<Self> {
        Self::with_worker_main(run_capture_worker)
    }

    pub(crate) fn with_worker_main<F>(worker_main: F) -> io::Result<Self>
    where
        F: FnOnce(&Mutex<Receiver<CaptureJob>>) + Send + 'static,
    {
        Self::start(worker_main)
    }

    fn start<F>(worker_main: F) -> io::Result<Self>
    where
        F: FnOnce(&Mutex<Receiver<CaptureJob>>) + Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(MAX_QUEUED_CAPTURES);
        let receiver = Arc::new(Mutex::new(receiver));
        let sender = Arc::new(Mutex::new(Some(sender)));
        let (failure_sender, failure_receiver) = oneshot::channel();
        let healthy = Arc::new(AtomicBool::new(true));

        let worker_receiver = Arc::clone(&receiver);
        let panic_receiver = Arc::clone(&receiver);
        let worker_healthy = Arc::clone(&healthy);
        let worker_sender = Arc::clone(&sender);
        let worker = thread::Builder::new()
            .name("bobcat-server-headless".to_owned())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| worker_main(&worker_receiver)));
                if let Err(payload) = result {
                    worker_healthy.store(false, Ordering::Release);
                    close_and_discard_queue(&worker_sender, &panic_receiver);
                    let _ = failure_sender.send(());
                    resume_unwind(payload);
                }
            })?;

        Ok(Self {
            failure_receiver: Mutex::new(Some(failure_receiver)),
            healthy,
            jobs: receiver,
            sender,
            worker: Mutex::new(Some(worker)),
        })
    }

    pub(crate) fn take_failure_receiver(&self) -> Result<oneshot::Receiver<()>, CaptureQueueError> {
        if !self.is_healthy() {
            return Err(CaptureQueueError::Unavailable);
        }
        self.failure_receiver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or(CaptureQueueError::Unavailable)
    }

    pub(crate) fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }

    pub(crate) fn submit(
        &self,
        request: CaptureRequest,
    ) -> Result<oneshot::Receiver<Result<Screenshot, CaptureFailure>>, CaptureQueueError> {
        let (response, response_receiver) = oneshot::channel();
        let job = CaptureJob { request, response };
        let sender = self.sender.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(sender) = sender.as_ref() else {
            return Err(CaptureQueueError::ShuttingDown);
        };
        match sender.try_send(job) {
            Ok(()) => Ok(response_receiver),
            Err(TrySendError::Full(job)) => {
                self.retry_after_dropping_cancelled(sender, job)?;
                Ok(response_receiver)
            }
            Err(TrySendError::Disconnected(_)) => Err(CaptureQueueError::Unavailable),
        }
    }

    fn retry_after_dropping_cancelled(
        &self,
        sender: &SyncSender<CaptureJob>,
        job: CaptureJob,
    ) -> Result<(), CaptureQueueError> {
        let jobs = match self.jobs.try_lock() {
            Ok(jobs) => jobs,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(CaptureQueueError::QueueFull),
        };
        let mut live_jobs = Vec::new();
        while let Ok(queued) = jobs.try_recv() {
            if !queued.response.is_closed() {
                live_jobs.push(queued);
            }
        }
        for queued in live_jobs {
            let requeued = sender.try_send(queued);
            assert!(
                requeued.is_ok(),
                "requeuing a drained live capture cannot exceed capacity"
            );
        }
        match sender.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(CaptureQueueError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(CaptureQueueError::Unavailable),
        }
    }

    pub(crate) async fn capture(
        &self,
        request: CaptureRequest,
    ) -> Result<Result<Screenshot, CaptureFailure>, CaptureQueueError> {
        self.submit(request)?
            .await
            .map_err(|_| CaptureQueueError::Stopped)
    }

    pub(crate) fn shutdown(&self) -> Result<(), WorkerPanicked> {
        self.healthy.store(false, Ordering::Release);
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(sender);
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            worker.join().map_err(|_| WorkerPanicked)?;
        }
        Ok(())
    }
}

impl Drop for CaptureExecutor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn close_and_discard_queue(
    sender: &Mutex<Option<SyncSender<CaptureJob>>>,
    jobs: &Mutex<Receiver<CaptureJob>>,
) {
    let sender = sender.lock().unwrap_or_else(PoisonError::into_inner).take();
    drop(sender);
    let jobs = jobs.lock().unwrap_or_else(PoisonError::into_inner);
    while jobs.try_recv().is_ok() {}
}

fn run_capture_worker(jobs: &Mutex<Receiver<CaptureJob>>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the Bobcat capture runtime must initialize");
    let client = Client::new();
    loop {
        let job = {
            let jobs = jobs.lock().unwrap_or_else(PoisonError::into_inner);
            jobs.recv()
        };
        let Ok(job) = job else { return };
        if job.response.is_closed() {
            continue;
        }
        let result = runtime.block_on(capture_page(&client, &job.request));
        let _ = job.response.send(result);
    }
}

async fn capture_page(
    client: &Client,
    request: &CaptureRequest,
) -> Result<Screenshot, CaptureFailure> {
    let loaded = tokio::time::timeout(request.timeout, load_input(client, &request.url))
        .await
        .map_err(|_| CaptureFailure::timeout("input load", request.timeout))??;
    let page = PageSource::from_bytes(&loaded.url, &loaded.bytes).map_err(|source| {
        CaptureFailure::Source {
            url: loaded.url.clone(),
            source: Box::new(source),
        }
    })?;
    drop(loaded.bytes);
    for warning in page.compatibility_warnings() {
        eprintln!("bobcat-server: warning: {warning}");
    }
    let resources = Resources::new(resources_config(&page, request.timeout), || {});
    page.register_with(&resources);
    for note in resources.take_notes() {
        eprintln!("bobcat-server: warning: {note}");
    }
    let sources = page.view_sources();
    drop(page);

    let mut view = tokio::time::timeout(
        request.timeout,
        LynxView::new(
            Arc::new(NoWakeup),
            VIEWPORT_WIDTH,
            VIEWPORT_HEIGHT,
            DEVICE_PIXEL_RATIO,
            DrawTarget::Offscreen,
            resources.builder(),
            sources,
        ),
    )
    .await
    .map_err(|_| CaptureFailure::timeout("page startup", request.timeout))?
    .map_err(|source| CaptureFailure::StartView {
        url: request.url.clone(),
        source: Box::new(source),
    })?;

    view.tick(true).map_err(|source| CaptureFailure::Render {
        url: request.url.clone(),
        source: Box::new(source),
    })?;
    check_events(&mut view, &request.url)?;

    // UI Judge treats settle time as an explicit post-readiness delay rather
    // than part of `timeoutMs`; preserve that observable behavior.
    settle(&mut view, &request.url, request.screenshot_settle).await?;

    let screenshot = view.capture().map_err(|source| CaptureFailure::Render {
        url: request.url.clone(),
        source: Box::new(source),
    })?;
    check_events(&mut view, &request.url)?;
    drop(view);
    Ok(screenshot)
}

fn resources_config(page: &PageSource, timeout: Duration) -> ResourcesConfig {
    ResourcesConfig {
        base_url: Some(page.input_url().clone()),
        request_timeout: timeout,
        ..ResourcesConfig::default()
    }
}

async fn load_input(client: &Client, url: &Url) -> Result<LoadedInput, CaptureFailure> {
    match url.scheme() {
        "file" => {
            let path = url.to_file_path().map_err(|()| CaptureFailure::ReadFile {
                url: url.clone(),
                source: io::Error::new(io::ErrorKind::InvalidInput, "invalid local file URL"),
            })?;
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|source| CaptureFailure::ReadFile {
                    url: url.clone(),
                    source,
                })?;
            Ok(LoadedInput {
                bytes,
                url: url.clone(),
            })
        }
        "http" | "https" => {
            let response =
                client
                    .get(url.clone())
                    .send()
                    .await
                    .map_err(|source| CaptureFailure::Fetch {
                        url: url.clone(),
                        source,
                    })?;
            let effective_url = response.url().clone();
            if !response.status().is_success() {
                return Err(CaptureFailure::HttpStatus {
                    url: effective_url,
                    status: response.status(),
                });
            }
            let bytes = response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|source| CaptureFailure::Fetch {
                    url: effective_url.clone(),
                    source,
                })?;
            Ok(LoadedInput {
                bytes,
                url: effective_url,
            })
        }
        _ => unreachable!("the HTTP boundary validates supported URL schemes"),
    }
}

async fn settle(
    view: &mut LynxView<ViewResources>,
    url: &Url,
    mut remaining: Duration,
) -> Result<(), CaptureFailure> {
    while !remaining.is_zero() {
        let step = remaining.min(FRAME_INTERVAL);
        tokio::time::sleep(step).await;
        view.tick(false).map_err(|source| CaptureFailure::Render {
            url: url.clone(),
            source: Box::new(source),
        })?;
        check_events(view, url)?;
        remaining = remaining.saturating_sub(step);
    }
    Ok(())
}

fn check_events(view: &mut LynxView<ViewResources>, url: &Url) -> Result<(), CaptureFailure> {
    for event in view.pump() {
        match event {
            EngineEvent::ScriptFinished => {}
            EngineEvent::ScriptRunError(error) => {
                return Err(CaptureFailure::Script {
                    url: url.clone(),
                    message: error.to_string(),
                });
            }
            EngineEvent::ListenerFailed(error) => {
                eprintln!("bobcat-server: event listener failed: {error}");
            }
            EngineEvent::TimerFailed(error) => {
                eprintln!("bobcat-server: timer callback failed: {error}");
            }
            EngineEvent::RenderFailed(source) => {
                return Err(CaptureFailure::Render {
                    url: url.clone(),
                    source: Box::new(source),
                });
            }
            _ => eprintln!("bobcat-server: ignored an unknown engine event"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::mpsc::Sender;

    use super::*;

    struct BlockedExecutor {
        executor: CaptureExecutor,
        release: Option<Sender<()>>,
    }

    impl BlockedExecutor {
        fn new() -> Self {
            let (release, released) = mpsc::channel();
            let executor = CaptureExecutor::with_worker_main(move |_jobs| {
                let _ = released.recv();
            })
            .expect("start blocked worker");
            Self {
                executor,
                release: Some(release),
            }
        }

        fn release_and_shutdown(mut self) {
            let release = self.release.take().expect("worker is still blocked");
            let _ = release.send(());
            self.executor.shutdown().expect("stop worker");
        }
    }

    impl Drop for BlockedExecutor {
        fn drop(&mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.send(());
            }
        }
    }

    fn request() -> CaptureRequest {
        CaptureRequest {
            screenshot_settle: Duration::ZERO,
            timeout: Duration::from_secs(1),
            url: Url::parse("file:///tmp/card.web.bundle").unwrap(),
        }
    }

    #[tokio::test]
    async fn a_redirected_input_uses_its_final_url_as_the_resource_base() {
        const PAGE: &[u8] =
            b"<lynx engine-version=\"4.2\"><script thread=\"main\">export {};</script></lynx>";

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind redirect fixture");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            for (path, response) in [
                (
                    "/old/card.lynx.xml",
                    "HTTP/1.1 302 Found\r\nLocation: /new/card.lynx.xml\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                ),
                (
                    "/new/card.lynx.xml",
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        PAGE.len(),
                        std::str::from_utf8(PAGE).expect("ASCII page fixture")
                    ),
                ),
            ] {
                let (mut stream, _) = listener.accept().expect("accept fixture request");
                let mut request = [0_u8; 2048];
                let count = stream.read(&mut request).expect("read fixture request");
                let request = std::str::from_utf8(&request[..count]).expect("HTTP is ASCII");
                assert!(
                    request.starts_with(&format!("GET {path} HTTP/1.1")),
                    "unexpected request: {request}"
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write fixture response");
            }
        });

        let original =
            Url::parse(&format!("http://{address}/old/card.lynx.xml")).expect("fixture URL");
        let loaded = load_input(&Client::new(), &original)
            .await
            .expect("follow redirect");
        assert_eq!(loaded.url.path(), "/new/card.lynx.xml");
        let page = PageSource::from_bytes(&loaded.url, &loaded.bytes).expect("decode final page");
        assert_eq!(
            resources_config(&page, Duration::from_secs(1)).base_url,
            Some(loaded.url)
        );
        server.join().expect("redirect fixture stays healthy");
    }

    #[test]
    fn a_full_queue_is_reported_instead_of_blocking() {
        let blocked = BlockedExecutor::new();

        let accepted = (0..MAX_QUEUED_CAPTURES)
            .map(|_| {
                blocked
                    .executor
                    .submit(request())
                    .expect("queue accepts request")
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            blocked.executor.submit(request()),
            Err(CaptureQueueError::QueueFull)
        ));
        drop(accepted);
        blocked.release_and_shutdown();
    }

    #[test]
    fn cancelled_jobs_release_queue_capacity() {
        let blocked = BlockedExecutor::new();

        let cancelled = (0..MAX_QUEUED_CAPTURES)
            .map(|_| blocked.executor.submit(request()).expect("fill queue"))
            .collect::<Vec<_>>();
        drop(cancelled);
        let replacement = blocked
            .executor
            .submit(request())
            .expect("cancelled requests release capacity");
        drop(replacement);
        blocked.release_and_shutdown();
    }

    #[tokio::test]
    async fn shutdown_stops_admission() {
        let executor =
            CaptureExecutor::with_worker_main(|_jobs| {}).expect("start short-lived worker");
        executor.shutdown().expect("join worker");
        let error = executor
            .capture(request())
            .await
            .expect_err("shutdown rejects a request");
        assert!(matches!(error, CaptureQueueError::ShuttingDown));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_real_web_bundle_renders_to_the_fixed_bmp_contract() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bobcat-source/tests/fixtures/basic-class-selector.web.bundle");
        let url = Url::from_file_path(&path).expect("absolute fixture URL");
        let executor = CaptureExecutor::new().expect("start capture owner thread");
        let result = executor
            .capture(CaptureRequest {
                screenshot_settle: Duration::ZERO,
                timeout: Duration::from_secs(30),
                url,
            })
            .await
            .expect("capture queue remains available");
        executor.shutdown().expect("stop capture owner thread");

        let screenshot = result.expect("decode, boot, and render the web bundle");
        let bmp = crate::server::bmp::encode(&screenshot).expect("encode captured BMP");
        assert_eq!(&bmp[..2], b"BM");
        let image = image::load_from_memory(&bmp)
            .expect("decode captured BMP")
            .to_rgb8();
        assert_eq!(image.dimensions(), (800, 600));
        assert!(
            image
                .pixels()
                .any(|pixel| pixel.0.iter().any(|channel| *channel < 245)),
            "the bundle's pink element must survive rendering and BMP encoding"
        );
    }
}
