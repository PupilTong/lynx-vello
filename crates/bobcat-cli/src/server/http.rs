//! Axum boundary for the UI Judge-compatible screenshot route.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::{NonZeroU16, ParseIntError};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{Value, json};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use url::Url;

use crate::server::capture::{
    CaptureExecutor, CaptureFailure, CaptureInput, CaptureQueueError, CaptureRequest,
    WorkerPanicked,
};

const MAX_REQUEST_BYTES: usize = 20 * 1024 * 1024 + 64 * 1024;
const TCP_BACKLOG: i32 = 1_024;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("LYNX_USE_PORT must be an integer from 1 through 65535, got {port:?}: {source}")]
    InvalidPort {
        port: String,
        #[source]
        source: ParseIntError,
    },
    #[error("Bobcat headless worker panicked")]
    HeadlessWorkerPanicked,
    #[error("Bobcat headless worker is unavailable: {0}")]
    HeadlessWorkerUnavailable(String),
    #[error("Bobcat server I/O failed: {0}")]
    Io(#[from] io::Error),
}

impl From<WorkerPanicked> for ServerError {
    fn from(_: WorkerPanicked) -> Self {
        Self::HeadlessWorkerPanicked
    }
}

#[derive(Clone)]
struct AppState {
    headless: Arc<CaptureExecutor>,
}

#[derive(Debug)]
pub(super) struct ApiError {
    message: String,
    status: StatusCode,
}

impl ApiError {
    pub(super) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status,
        }
    }

    pub(super) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn unprocessable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl From<CaptureQueueError> for ApiError {
    fn from(error: CaptureQueueError) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, error.to_string())
    }
}

impl From<CaptureFailure> for ApiError {
    fn from(error: CaptureFailure) -> Self {
        let status = match &error {
            CaptureFailure::Timeout { .. } => StatusCode::REQUEST_TIMEOUT,
            CaptureFailure::Zip { source, .. }
                if !matches!(source.as_ref(), bobcat_source::ZipSourceError::Page(_)) =>
            {
                StatusCode::BAD_REQUEST
            }
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        Self::new(status, error.to_string())
    }
}

impl From<crate::server::bmp::BmpError> for ApiError {
    fn from(error: crate::server::bmp::BmpError) -> Self {
        Self::internal(error.to_string())
    }
}

#[derive(Serialize)]
struct ApiErrorBody {
    error: ApiErrorMessage,
}

#[derive(Serialize)]
struct ApiErrorMessage {
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                error: ApiErrorMessage {
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

/// Runs the screenshot server on IPv4 and IPv6 unspecified addresses.
/// Requests remain concurrent at the HTTP layer; capture crosses one bounded
/// queue to the thread that owns the non-`Send` Bobcat view handle.
pub async fn serve(port: &str) -> Result<(), ServerError> {
    let port = parse_port(port)?;
    let (ipv4_listener, ipv6_listener) = bind_listeners(port)?;
    let headless = Arc::new(CaptureExecutor::new()?);
    let worker_failure = headless
        .take_failure_receiver()
        .map_err(|error| ServerError::HeadlessWorkerUnavailable(error.to_string()))?;
    let app = router(Arc::clone(&headless));
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let worker_failure_task = tokio::spawn(trigger_shutdown_on_worker_failure(
        worker_failure,
        shutdown_sender.clone(),
    ));
    let signal_task = tokio::spawn(async move {
        if let Err(error) = shutdown_signal().await {
            eprintln!("[bobcat-server] failed to listen for shutdown: {error}");
        }
        let _ = shutdown_sender.send(true);
    });

    println!("Bobcat server listening on 0.0.0.0:{port} and [::]:{port}");
    let ipv4_server = axum::serve(ipv4_listener, app.clone())
        .with_graceful_shutdown(wait_for_shutdown(shutdown_receiver.clone()));
    let ipv6_server = axum::serve(ipv6_listener, app)
        .with_graceful_shutdown(wait_for_shutdown(shutdown_receiver));
    let result = tokio::try_join!(ipv4_server, ipv6_server);

    signal_task.abort();
    let _ = signal_task.await;
    let worker_result = headless.shutdown();
    let _ = worker_failure_task.await;
    worker_result?;
    result.map(|_| ()).map_err(ServerError::from)
}

fn router(headless: Arc<CaptureExecutor>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/screenshot/template", post(screenshot_template))
        .route("/screenshot/lynxml", post(screenshot_lynxml))
        .route("/screenshot/template/url", post(screenshot_template_url))
        .route("/screenshot/zip/upload", post(screenshot_zip_upload))
        .route("/screenshot/zip/url", post(screenshot_zip_url))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(AppState { headless })
}

fn parse_port(port: &str) -> Result<u16, ServerError> {
    port.parse::<NonZeroU16>()
        .map(NonZeroU16::get)
        .map_err(|source| ServerError::InvalidPort {
            port: port.to_owned(),
            source,
        })
}

fn bind_listeners(port: u16) -> io::Result<(TcpListener, TcpListener)> {
    let ipv4 = bind_listener(
        Domain::IPV4,
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
    )?;
    let ipv6 = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    ipv6.set_only_v6(true)?;
    let ipv6 = configure_listener(ipv6, SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)))?;
    Ok((ipv4, ipv6))
}

fn bind_listener(domain: Domain, address: SocketAddr) -> io::Result<TcpListener> {
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    configure_listener(socket, address)
}

fn configure_listener(socket: Socket, address: SocketAddr) -> io::Result<TcpListener> {
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SockAddr::from(address))?;
    socket.listen(TCP_BACKLOG)?;
    TcpListener::from_std(socket.into())
}

async fn health(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    if state.headless.is_healthy() {
        Ok(Json(json!({ "status": "ok" })))
    } else {
        Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The Bobcat headless worker is unavailable.",
        ))
    }
}

async fn screenshot_template(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    screenshot_remote(state, request, true, false).await
}

async fn screenshot_template_url(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    screenshot_remote(state, request, false, false).await
}

async fn screenshot_zip_url(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    screenshot_remote(state, request, false, true).await
}

async fn screenshot_remote(
    state: AppState,
    request: Request,
    page_options: bool,
    zip: bool,
) -> Result<Response, ApiError> {
    let mut form = super::form::read(request, "url", page_options).await?;
    if !zip && !form.entry.path.to_string_lossy().ends_with(".js") {
        return Err(ApiError::bad_request(
            "entry must identify a template.js file.",
        ));
    }
    let url = std::str::from_utf8(&form.source)
        .map_err(|_| ApiError::bad_request("The remote URL must be valid UTF-8."))?
        .trim();
    if url.is_empty() {
        return Err(ApiError::bad_request("The remote URL must not be empty."));
    }
    form.source =
        super::remote::fetch_http_resource(url, 10 * 1024 * 1024, Duration::from_secs(10))
            .await
            .map_err(|error| remote_fetch_api_error(&error))?
            .bytes;
    if !zip && form.source.is_empty() {
        return Err(ApiError::unprocessable("The remote template is empty."));
    }
    capture_form(state, form, zip).await
}

async fn screenshot_lynxml(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    let form = super::form::read(request, "source", true).await?;
    if !form.entry.path.to_string_lossy().ends_with(".lynxml") {
        return Err(ApiError::bad_request("entry must identify a .lynxml file."));
    }
    if form.source.is_empty() {
        return Err(ApiError::bad_request("LynXML source must not be empty."));
    }
    if std::str::from_utf8(&form.source).is_err() {
        return Err(ApiError::bad_request("LynXML source must be valid UTF-8."));
    }
    capture_form(state, form, false).await
}

async fn screenshot_zip_upload(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, ApiError> {
    let form = super::form::read(request, "file", false).await?;
    capture_form(state, form, true).await
}

async fn capture_form(
    state: AppState,
    form: super::form::ScreenshotForm,
    zip: bool,
) -> Result<Response, ApiError> {
    if zip && form.source.is_empty() {
        return Err(ApiError::bad_request("ZIP upload must not be empty."));
    }
    // Core converts these values, but does not yet deliver them to page boot.
    // Keep this limitation explicit until the runtime implements that seam.
    for (name, value) in [
        ("globalProps", &form.global_props),
        ("initData", &form.init_data),
    ] {
        if value
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|object| !object.is_empty())
        {
            return Err(ApiError::unprocessable(format!(
                "bobcat-server does not yet support non-empty {name}."
            )));
        }
    }
    let request = CaptureRequest {
        url: Url::parse(&form.entry.url).expect("validated staged entry URL"),
        width: u16::try_from(form.viewport.width).expect("viewport is bounded to 8192"),
        height: u16::try_from(form.viewport.height).expect("viewport is bounded to 8192"),
        screenshot_settle: form.screenshot_settle,
        timeout: form.timeout,
        input: if zip {
            CaptureInput::Zip(form.source)
        } else {
            CaptureInput::Bytes(form.source)
        },
    };
    let screenshot = state.headless.capture(request).await??;
    let bmp = tokio::task::spawn_blocking(move || crate::server::bmp::encode(&screenshot))
        .await
        .map_err(|error| ApiError::internal(format!("BMP worker failed: {error}")))??;
    Ok((
        [(CONTENT_TYPE, "image/bmp"), (CACHE_CONTROL, "no-store")],
        bmp,
    )
        .into_response())
}

fn remote_fetch_api_error(error: &super::remote::HttpFetchError) -> ApiError {
    use super::remote::HttpFetchError;
    let status = match error {
        HttpFetchError::InvalidUrl | HttpFetchError::Credentials => StatusCode::BAD_REQUEST,
        HttpFetchError::NonPublicAddress => StatusCode::FORBIDDEN,
        HttpFetchError::TimedOut => StatusCode::GATEWAY_TIMEOUT,
        HttpFetchError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
        HttpFetchError::Resolution | HttpFetchError::Request | HttpFetchError::Status(_) => {
            StatusCode::BAD_GATEWAY
        }
    };
    ApiError::new(status, error.to_string())
}

async fn wait_for_shutdown(mut receiver: watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() || receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn trigger_shutdown_on_worker_failure(
    worker_failure: oneshot::Receiver<()>,
    shutdown_sender: watch::Sender<bool>,
) {
    if worker_failure.await.is_ok() {
        let _ = shutdown_sender.send(true);
    }
}

#[cfg(unix)]
async fn shutdown_signal() -> io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;
    use std::sync::{MutexGuard, PoisonError};

    use axum::body::{Body, to_bytes};
    use axum::http::Method;
    use bobcat_core::{FrameSize, Screenshot};
    use tower::ServiceExt;

    use super::*;
    use crate::server::capture::CaptureJob;

    fn scripted_executor(screenshot: Screenshot) -> Arc<CaptureExecutor> {
        Arc::new(
            CaptureExecutor::with_worker_main(
                move |jobs: &std::sync::Mutex<Receiver<CaptureJob>>| loop {
                    let job = {
                        let jobs: MutexGuard<'_, Receiver<CaptureJob>> =
                            jobs.lock().unwrap_or_else(PoisonError::into_inner);
                        jobs.recv()
                    };
                    let Ok(job) = job else { return };
                    let _ = job.response.send(Ok(screenshot.clone()));
                },
            )
            .expect("start scripted worker"),
        )
    }

    use crate::server::form::tests::multipart_request;

    #[tokio::test]
    async fn screenshot_returns_bmp_bytes_and_reference_headers() {
        let frame = Screenshot {
            size: FrameSize {
                width: 2,
                height: 2,
            },
            pixels: [20, 40, 60, 128].repeat(4),
        };
        let expected = crate::server::bmp::encode(&frame).unwrap();
        let headless = scripted_executor(frame);
        for (path, source_name, entry, source) in [
            (
                "/screenshot/lynxml",
                "source",
                "pages/index.lynxml",
                b"<lynx/>".as_slice(),
            ),
            (
                "/screenshot/zip/upload",
                "file",
                "index.lynxml",
                b"zip".as_slice(),
            ),
        ] {
            let response = router(Arc::clone(&headless))
                .oneshot(multipart_request(
                    path,
                    &[
                        ("entry", entry.as_bytes()),
                        (source_name, source),
                        ("width", b"2"),
                        ("height", b"2"),
                    ],
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[CONTENT_TYPE], "image/bmp");
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
            assert_eq!(
                response.headers()[axum::http::header::CONTENT_LENGTH],
                expected.len().to_string()
            );
            let body = to_bytes(response.into_body(), expected.len())
                .await
                .unwrap();
            assert_eq!(body.as_ref(), expected);
        }
        headless.shutdown().unwrap();
    }

    #[tokio::test]
    async fn multipart_options_and_owned_source_reach_the_capture_worker() {
        let (observed, requests) = std::sync::mpsc::channel();
        let headless = Arc::new(
            CaptureExecutor::with_worker_main(move |jobs| {
                loop {
                    let job = jobs.lock().unwrap().recv();
                    let Ok(job) = job else { return };
                    observed.send(job.request.clone()).unwrap();
                    let _ = job.response.send(Ok(Screenshot {
                        size: FrameSize {
                            width: 1,
                            height: 1,
                        },
                        pixels: vec![1, 2, 3, 4],
                    }));
                }
            })
            .unwrap(),
        );
        let source = b"<lynx/>";
        let response = router(Arc::clone(&headless))
            .oneshot(multipart_request(
                "/screenshot/lynxml",
                &[
                    ("source", source),
                    ("timeoutMs", b"2345"),
                    ("screenshotSettleMs", b"0"),
                    ("entry", b"zip:///pages/index.lynxml"),
                    ("width", b"375"),
                    ("height", b"812"),
                    ("initData", b"{}"),
                ],
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let request = requests.try_recv().unwrap();
        assert_eq!(request.url.as_str(), "zip:///pages/index.lynxml");
        assert_eq!((request.width, request.height), (375, 812));
        assert_eq!(request.timeout, Duration::from_millis(2345));
        assert!(request.screenshot_settle.is_zero());
        assert!(matches!(request.input, CaptureInput::Bytes(bytes) if bytes == source));
        headless.shutdown().unwrap();
    }

    #[tokio::test]
    async fn old_route_json_queries_and_invalid_sources_are_rejected() {
        let headless = scripted_executor(Screenshot {
            size: FrameSize {
                width: 1,
                height: 1,
            },
            pixels: vec![0; 4],
        });
        let app = router(Arc::clone(&headless));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/screenshot")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"url":"file:///tmp/card","task":"capture"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        for path in [
            "/screenshot/lynxml",
            "/screenshot/template",
            "/screenshot/template/url",
            "/screenshot/zip/upload",
            "/screenshot/zip/url",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "{path}"
            );
        }
        for (path, fields, status) in [
            (
                "/screenshot/lynxml?width=1",
                vec![
                    ("entry", b"index.lynxml".as_slice()),
                    ("source", b"<lynx/>"),
                ],
                StatusCode::BAD_REQUEST,
            ),
            (
                "/screenshot/lynxml",
                vec![("entry", b"index.js".as_slice()), ("source", b"<lynx/>")],
                StatusCode::BAD_REQUEST,
            ),
            (
                "/screenshot/lynxml",
                vec![("entry", b"index.lynxml".as_slice()), ("source", b"")],
                StatusCode::BAD_REQUEST,
            ),
            (
                "/screenshot/lynxml",
                vec![("entry", b"index.lynxml".as_slice()), ("source", b"\xff")],
                StatusCode::BAD_REQUEST,
            ),
            (
                "/screenshot/lynxml",
                vec![
                    ("entry", b"index.lynxml".as_slice()),
                    ("source", b"<lynx/>"),
                    ("initData", br#"{"ready":true}"#),
                ],
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/screenshot/zip/upload",
                vec![("entry", b"index.lynxml".as_slice()), ("file", b"")],
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(multipart_request(path, &fields))
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{path}");
            assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            let error: Value = serde_json::from_slice(&body).unwrap();
            assert!(error["error"]["message"].is_string());
        }
        headless.shutdown().unwrap();
    }

    #[tokio::test]
    async fn remote_routes_share_url_validation() {
        let headless = scripted_executor(Screenshot {
            size: FrameSize {
                width: 1,
                height: 1,
            },
            pixels: vec![0; 4],
        });
        for path in [
            "/screenshot/template",
            "/screenshot/template/url",
            "/screenshot/zip/url",
        ] {
            for (url, status) in [
                ("file:///tmp/private", StatusCode::BAD_REQUEST),
                (
                    "https://user:secret@example.com/private",
                    StatusCode::BAD_REQUEST,
                ),
                ("http://127.0.0.1/private", StatusCode::FORBIDDEN),
                ("HTTP://[::1]/private", StatusCode::FORBIDDEN),
            ] {
                let response = router(Arc::clone(&headless))
                    .oneshot(multipart_request(
                        path,
                        &[("entry", b"template.js"), ("url", url.as_bytes())],
                    ))
                    .await
                    .unwrap();
                assert_eq!(response.status(), status, "{path}: {url}");
            }
        }
        headless.shutdown().unwrap();
    }

    #[tokio::test]
    async fn real_multipart_xml_and_zip_captures_use_the_requested_viewport() {
        use std::io::{Cursor, Write};
        let headless = Arc::new(CaptureExecutor::new().unwrap());
        let xml = br#"<lynx engine-version="4.2">
          <script thread="main">
            globalThis.renderPage = function () {
              const page = __CreatePage('0', 0);
              const view = __CreateView(0);
              __SetInlineStyles(view, 'width:100%;height:100%;background-color:#00ff00');
              __AppendElement(page, view);
              __FlushElementTree(page);
            };
          </script>
        </lynx>"#;
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "pages/index.lynxml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(xml).unwrap();
        let archive = archive.finish().unwrap().into_inner();
        for (path, source_name, source) in [
            ("/screenshot/lynxml", "source", xml.as_slice()),
            ("/screenshot/zip/upload", "file", archive.as_slice()),
        ] {
            let response = router(Arc::clone(&headless))
                .oneshot(multipart_request(
                    path,
                    &[
                        ("entry", b"pages/index.lynxml"),
                        (source_name, source),
                        ("width", b"37"),
                        ("height", b"23"),
                    ],
                ))
                .await
                .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(
                status,
                StatusCode::OK,
                "{path}: {}",
                String::from_utf8_lossy(&bytes)
            );
            let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
            assert_eq!(image.dimensions(), (37, 23));
            assert_eq!(image.get_pixel(18, 11).0, [0, 255, 0, 255]);
        }
        headless.shutdown().unwrap();
    }

    #[tokio::test]
    async fn health_reports_worker_readiness() {
        let headless = scripted_executor(Screenshot {
            size: FrameSize {
                width: 1,
                height: 1,
            },
            pixels: vec![0, 0, 0, 0],
        });
        let response = health(State(AppState {
            headless: Arc::clone(&headless),
        }))
        .await
        .expect("healthy executor");
        assert_eq!(response.0, json!({ "status": "ok" }));
        headless.shutdown().expect("stop worker");
    }

    #[test]
    fn rejects_port_zero() {
        assert!(matches!(
            parse_port("0"),
            Err(ServerError::InvalidPort { .. })
        ));
    }
}
