//! Axum boundary for the UI Judge-compatible screenshot route.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::{NonZeroU16, ParseIntError};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use url::Url;

use crate::capture::{
    CaptureExecutor, CaptureFailure, CaptureQueueError, CaptureRequest, WorkerPanicked,
};

const DEFAULT_SCREENSHOT_SETTLE_MS: u64 = 16;
const DEFAULT_TIMEOUT_MS: u64 = 60_000;
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

/// The UI Judge request shape is retained so an existing screenshot caller
/// can target Bobcat. Scoring-only fields are parsed and ignored by this
/// route; inputs that need runtime or `DevTools` support Bobcat does not yet
/// expose are rejected explicitly rather than rendered with missing data.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HttpScreenshotRequest {
    #[serde(default, alias = "global_props")]
    global_props: Option<Value>,
    #[serde(default, alias = "include_screenshot")]
    include_screenshot: bool,
    #[serde(default, alias = "initial_data")]
    initial_data: Option<Value>,
    #[serde(default, alias = "include_geqi")]
    include_geqi: bool,
    #[serde(default)]
    reference: Option<String>,
    #[serde(default, alias = "reference_image")]
    reference_image: Option<String>,
    #[serde(default, alias = "screenshot_settle_ms")]
    screenshot_settle_ms: Option<u64>,
    #[serde(default)]
    steps: Vec<String>,
    task: String,
    #[serde(default, alias = "timeout_ms")]
    timeout_ms: Option<u64>,
    url: String,
}

impl HttpScreenshotRequest {
    fn into_capture_request(self) -> Result<CaptureRequest, ApiError> {
        let Self {
            global_props,
            include_screenshot,
            initial_data,
            include_geqi,
            reference,
            reference_image,
            screenshot_settle_ms,
            steps,
            task,
            timeout_ms,
            url,
        } = self;
        drop((include_screenshot, include_geqi, reference, reference_image));

        let timeout_ms = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        if timeout_ms == 0 {
            return Err(ApiError::bad_request(
                "timeoutMs must be greater than zero.",
            ));
        }

        let global_props = page_data("globalProps", global_props)?;
        let initial_data = page_data("initialData", initial_data)?;
        if global_props.as_ref().is_some_and(|value| !value.is_empty()) {
            return Err(ApiError::unprocessable(
                "bobcat-server does not yet support non-empty globalProps.",
            ));
        }
        if initial_data.as_ref().is_some_and(|value| !value.is_empty()) {
            return Err(ApiError::unprocessable(
                "bobcat-server does not yet support non-empty initialData.",
            ));
        }
        if steps.iter().any(|step| !step.trim().is_empty()) {
            return Err(ApiError::unprocessable(
                "bobcat-server does not yet support screenshot interaction steps.",
            ));
        }

        let url = url.trim();
        if url.is_empty() {
            return Err(ApiError::unprocessable(
                "screenshot requires a non-empty URL.",
            ));
        }
        if !["file://", "http://", "https://"]
            .iter()
            .any(|prefix| url.starts_with(prefix))
        {
            return Err(ApiError::unprocessable(
                "screenshot URL must use file://, http://, or https://.",
            ));
        }
        let url = Url::parse(url).map_err(|_| {
            ApiError::unprocessable("screenshot URL must use file://, http://, or https://.")
        })?;
        if !matches!(url.scheme(), "file" | "http" | "https")
            || (url.scheme() == "file" && url.to_file_path().is_err())
        {
            return Err(ApiError::unprocessable(
                "screenshot URL must use file://, http://, or https://.",
            ));
        }
        if task.trim().is_empty() {
            return Err(ApiError::unprocessable(
                "screenshot requires a non-empty task.",
            ));
        }

        Ok(CaptureRequest {
            screenshot_settle: Duration::from_millis(
                screenshot_settle_ms.unwrap_or(DEFAULT_SCREENSHOT_SETTLE_MS),
            ),
            timeout: Duration::from_millis(timeout_ms),
            url,
        })
    }
}

fn page_data(name: &str, value: Option<Value>) -> Result<Option<Map<String, Value>>, ApiError> {
    match value {
        Some(Value::Object(value)) => Ok(Some(value)),
        Some(_) => Err(ApiError::bad_request(format!(
            "{name} must be a JSON object."
        ))),
        None => Ok(None),
    }
}

#[derive(Debug)]
struct ApiError {
    message: String,
    status: StatusCode,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
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
        Self::unprocessable(error.to_string())
    }
}

impl From<crate::bmp::BmpError> for ApiError {
    fn from(error: crate::bmp::BmpError) -> Self {
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
        .route("/screenshot", post(screenshot))
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

async fn screenshot(
    State(state): State<AppState>,
    Json(request): Json<HttpScreenshotRequest>,
) -> Result<Response, ApiError> {
    let request = request.into_capture_request()?;
    let screenshot = state.headless.capture(request).await??;
    let bmp = tokio::task::spawn_blocking(move || crate::bmp::encode(&screenshot))
        .await
        .map_err(|error| ApiError::internal(format!("BMP worker failed: {error}")))??;
    Ok(([(CONTENT_TYPE, "image/bmp")], bmp).into_response())
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
    use axum::http::{Method, Request};
    use bobcat_core::{FrameSize, Screenshot};
    use tower::ServiceExt;

    use super::*;
    use crate::capture::CaptureJob;

    fn http_request(url: &str) -> HttpScreenshotRequest {
        HttpScreenshotRequest {
            global_props: None,
            include_screenshot: false,
            initial_data: None,
            include_geqi: false,
            reference: None,
            reference_image: None,
            screenshot_settle_ms: None,
            steps: Vec::new(),
            task: "Render the page".to_owned(),
            timeout_ms: None,
            url: url.to_owned(),
        }
    }

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

    #[test]
    fn defaults_match_the_ui_judge_screenshot_contract() {
        let request = http_request("file:///tmp/card.web.bundle")
            .into_capture_request()
            .expect("valid request");
        assert_eq!(request.screenshot_settle, Duration::from_millis(16));
        assert_eq!(request.timeout, Duration::from_mins(1));
    }

    #[test]
    fn accepts_snake_case_aliases_and_empty_page_data() {
        let request = serde_json::from_value::<HttpScreenshotRequest>(json!({
            "global_props": {},
            "initial_data": {},
            "include_screenshot": true,
            "include_geqi": true,
            "reference_image": null,
            "screenshot_settle_ms": 0,
            "task": "Capture",
            "timeout_ms": 1,
            "url": "https://example.test/card.web.bundle"
        }))
        .expect("deserialize aliases")
        .into_capture_request()
        .expect("empty page data need no unsupported runtime feature");
        assert!(request.screenshot_settle.is_zero());
        assert_eq!(request.timeout, Duration::from_millis(1));
    }

    #[test]
    fn rejects_unsupported_runtime_inputs_explicitly() {
        let mut with_data = http_request("file:///tmp/card.web.bundle");
        with_data.initial_data = Some(json!({ "message": "hello" }));
        assert_eq!(
            with_data
                .into_capture_request()
                .expect_err("initial data is not wired")
                .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let mut with_steps = http_request("file:///tmp/card.web.bundle");
        with_steps.steps = vec!["Tap Save".to_owned()];
        assert_eq!(
            with_steps
                .into_capture_request()
                .expect_err("DOM automation is not exposed")
                .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn rejects_invalid_request_values_with_reference_statuses() {
        let mut zero_timeout = http_request("file:///tmp/card.web.bundle");
        zero_timeout.timeout_ms = Some(0);
        assert_eq!(
            zero_timeout
                .into_capture_request()
                .expect_err("zero timeout")
                .status,
            StatusCode::BAD_REQUEST
        );

        let mut invalid_data = http_request("file:///tmp/card.web.bundle");
        invalid_data.global_props = Some(json!([]));
        assert_eq!(
            invalid_data
                .into_capture_request()
                .expect_err("page data must be an object")
                .status,
            StatusCode::BAD_REQUEST
        );

        let invalid_url = http_request("card.web.bundle");
        assert_eq!(
            invalid_url
                .into_capture_request()
                .expect_err("bare paths are rejected")
                .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let uppercase_scheme = http_request("HTTP://example.test/card.web.bundle");
        assert_eq!(
            uppercase_scheme
                .into_capture_request()
                .expect_err("schemes are case-sensitive in UI Judge")
                .status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn screenshot_returns_raw_bmp() {
        let frame = Screenshot {
            size: FrameSize {
                width: 8,
                height: 8,
            },
            pixels: [20, 40, 60, 255].repeat(64),
        };
        let bmp = crate::bmp::encode(&frame).expect("encode expected BMP");
        let headless = scripted_executor(frame);
        let response = router(Arc::clone(&headless))
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/screenshot")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "task": "Render the page",
                            "url": "file:///tmp/card.web.bundle"
                        })
                        .to_string(),
                    ))
                    .expect("valid HTTP request"),
            )
            .await
            .expect("route screenshot response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "image/bmp");
        let body = to_bytes(response.into_body(), bmp.len() + 1)
            .await
            .expect("read response");
        assert_eq!(body.as_ref(), bmp);
        headless.shutdown().expect("stop worker");
    }

    #[tokio::test]
    async fn screenshot_keeps_axums_json_extractor_statuses() {
        let headless = scripted_executor(Screenshot {
            size: FrameSize {
                width: 1,
                height: 1,
            },
            pixels: vec![0, 0, 0, 0],
        });
        let app = router(Arc::clone(&headless));

        let missing_content_type = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/screenshot")
                    .body(Body::from(r#"{"task":"capture","url":"file:///tmp/card"}"#))
                    .expect("valid HTTP request"),
            )
            .await
            .expect("route response");
        assert_eq!(
            missing_content_type.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );

        let malformed_json = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/screenshot")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{"))
                    .expect("valid HTTP request"),
            )
            .await
            .expect("route response");
        assert_eq!(malformed_json.status(), StatusCode::BAD_REQUEST);

        let missing_field = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/screenshot")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"url":"file:///tmp/card"}"#))
                    .expect("valid HTTP request"),
            )
            .await
            .expect("route response");
        assert_eq!(missing_field.status(), StatusCode::UNPROCESSABLE_ENTITY);

        headless.shutdown().expect("stop worker");
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
