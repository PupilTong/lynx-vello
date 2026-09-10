// Copyright 2026 The Lynx Authors. All rights reserved.
// Licensed under the Apache License, Version 2.0.
//! Multipart contract adapted from lynx-stack ui-judge at 83ad08f44eb5.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use axum::extract::multipart::{Multipart, MultipartError};
use axum::extract::{DefaultBodyLimit, FromRequest, Request};
use axum::http::StatusCode;
use axum::http::header::CONTENT_LENGTH;
use serde_json::Value;

use super::http::ApiError;

const DEFAULT_SCREENSHOT_SETTLE_MS: u64 = 16;
const DEFAULT_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_SCREENSHOT_WIDTH: usize = 800;
const DEFAULT_SCREENSHOT_HEIGHT: usize = 600;
const MAX_SCREENSHOT_DIMENSION: usize = 8192;
const MAX_SCREENSHOT_PIXELS: usize = 10 * 1024 * 1024 / 4;
const LYNXML_UPLOAD_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SCREENSHOT_FORM_BYTES: usize = 10 * 1024 * 1024;
const MAX_SCREENSHOT_REQUEST_BYTES: usize = MAX_SCREENSHOT_FORM_BYTES + 64 * 1024;
const MAX_REMOTE_URL_BYTES: usize = 8 * 1024;

#[derive(Debug)]
pub(super) struct ScreenshotForm {
    pub(super) entry: ScreenshotEntry,
    pub(super) viewport: ScreenshotViewport,
    pub(super) global_props: Option<Value>,
    pub(super) init_data: Option<Value>,
    pub(super) source: Vec<u8>,
    pub(super) screenshot_settle: Duration,
    pub(super) timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScreenshotViewport {
    pub(super) height: usize,
    pub(super) width: usize,
}

impl ScreenshotViewport {
    fn new(width: usize, height: usize) -> Result<Self, ApiError> {
        if width == 0 || height == 0 {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "width and height must be greater than zero.",
            ));
        }
        let pixels = width
            .checked_mul(height)
            .ok_or_else(invalid_screenshot_dimensions)?;
        if width > MAX_SCREENSHOT_DIMENSION
            || height > MAX_SCREENSHOT_DIMENSION
            || pixels > MAX_SCREENSHOT_PIXELS
        {
            return Err(invalid_screenshot_dimensions());
        }
        Ok(Self { height, width })
    }
}

fn invalid_screenshot_dimensions() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        format!(
            "width and height must each be at most {MAX_SCREENSHOT_DIMENSION} and describe no more than {MAX_SCREENSHOT_PIXELS} pixels.",
        ),
    )
}

#[derive(Debug)]
pub(super) struct ScreenshotEntry {
    pub(super) path: PathBuf,
    pub(super) url: String,
}

impl ScreenshotEntry {
    fn parse(input: &str) -> Result<Self, ApiError> {
        let input = input.trim();
        let relative = match input.split_once("://") {
            Some((scheme, path)) if scheme.eq_ignore_ascii_case("zip") => {
                path.trim_start_matches('/')
            }
            Some(_) => return Err(invalid_screenshot_entry()),
            None => input,
        };
        if relative.is_empty()
            || relative.len() > 4096
            || relative.starts_with('/')
            || relative.contains(['\0', '\\', '?', '#'])
            || is_windows_absolute_path(relative)
        {
            return Err(invalid_screenshot_entry());
        }
        let relative = percent_decode_entry(relative).ok_or_else(invalid_screenshot_entry)?;
        if relative.contains(['\0', '\\']) || is_windows_absolute_path(&relative) {
            return Err(invalid_screenshot_entry());
        }
        let mut path = PathBuf::new();
        let mut url = reqwest::Url::parse("zip:///").expect("the internal ZIP URL is valid");
        let mut segment_count = 0;
        {
            let mut url_segments = url
                .path_segments_mut()
                .expect("the internal ZIP URL supports path segments");
            url_segments.pop_if_empty();
            for component in Path::new(&relative).components() {
                let Component::Normal(component) = component else {
                    return Err(invalid_screenshot_entry());
                };
                let component = component.to_str().ok_or_else(invalid_screenshot_entry)?;
                if component.is_empty() || component.len() > 255 {
                    return Err(invalid_screenshot_entry());
                }
                segment_count += 1;
                if segment_count > 20 {
                    return Err(invalid_screenshot_entry());
                }
                path.push(component);
                url_segments.push(component);
            }
        }
        if path.as_os_str().is_empty() {
            return Err(invalid_screenshot_entry());
        }
        Ok(Self {
            path,
            url: url.into(),
        })
    }
}

fn invalid_screenshot_entry() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "entry must identify a safe relative file path inside the staged source.",
    )
}

fn is_windows_absolute_path(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn percent_decode_entry(input: &str) -> Option<String> {
    let input = input.as_bytes();
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] != b'%' {
            output.push(input[index]);
            index += 1;
            continue;
        }
        if index + 2 >= input.len() {
            return None;
        }
        let high = hex_value(input[index + 1])?;
        let low = hex_value(input[index + 2])?;
        output.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(output).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub(super) async fn read(
    mut request: Request,
    source_name: &str,
    page_options: bool,
) -> Result<ScreenshotForm, ApiError> {
    if request.uri().query().is_some_and(|query| !query.is_empty()) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Screenshot parameters must be multipart fields; query parameters are not supported.",
        ));
    }
    if let Some(length) = request.headers().get(CONTENT_LENGTH) {
        let length = length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| {
                ApiError::new(StatusCode::BAD_REQUEST, "Invalid Content-Length header.")
            })?;
        if length > MAX_SCREENSHOT_REQUEST_BYTES as u64 {
            return Err(screenshot_form_too_large());
        }
    }
    request
        .extensions_mut()
        .insert(DefaultBodyLimit::max(MAX_SCREENSHOT_REQUEST_BYTES));
    let multipart = Multipart::from_request(request, &())
        .await
        .map_err(|error| ApiError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, error.to_string()))?;
    read_screenshot_form_with_deadline(
        parse_screenshot_form(multipart, source_name, page_options),
        LYNXML_UPLOAD_TIMEOUT,
    )
    .await
}

async fn read_screenshot_form_with_deadline(
    read: impl std::future::Future<Output = Result<ScreenshotForm, ApiError>>,
    timeout: Duration,
) -> Result<ScreenshotForm, ApiError> {
    tokio::time::timeout(timeout, read).await.map_err(|_| {
        ApiError::new(
            StatusCode::REQUEST_TIMEOUT,
            "The screenshot request body timed out.",
        )
    })?
}

async fn parse_screenshot_form(
    mut multipart: Multipart,
    source_name: &str,
    page_options: bool,
) -> Result<ScreenshotForm, ApiError> {
    let mut entry = None;
    let mut width = None;
    let mut height = None;
    let mut global_props = None;
    let mut init_data = None;
    let mut screenshot_settle_ms = None;
    let mut timeout_ms = None;
    let mut source = None;
    let mut total_bytes = 0_usize;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| multipart_error(&error))?
    {
        let name = field.name().unwrap_or_default().to_string();
        let slot = match name.as_str() {
            "entry" => &mut entry,
            "width" => &mut width,
            "height" => &mut height,
            "globalProps" => &mut global_props,
            "initData" => &mut init_data,
            "screenshotSettleMs" if page_options => &mut screenshot_settle_ms,
            "timeoutMs" if page_options => &mut timeout_ms,
            name if name == source_name => &mut source,
            _ => {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("Unexpected multipart field {name:?}."),
                ));
            }
        };
        if slot.is_some() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("{name} must be provided exactly once."),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| multipart_error(&error))?
        {
            total_bytes = total_bytes.saturating_add(chunk.len());
            if total_bytes > MAX_SCREENSHOT_FORM_BYTES {
                return Err(screenshot_form_too_large());
            }
            if name == "url" && bytes.len().saturating_add(chunk.len()) > MAX_REMOTE_URL_BYTES {
                return Err(remote_url_too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        *slot = Some(bytes);
    }
    let entry = ScreenshotEntry::parse(form_text("entry", &required_form_field("entry", entry)?)?)?;
    let viewport = ScreenshotViewport::new(
        form_dimension("width", width, DEFAULT_SCREENSHOT_WIDTH)?,
        form_dimension("height", height, DEFAULT_SCREENSHOT_HEIGHT)?,
    )?;
    let global_props = form_page_data("globalProps", global_props)?;
    let init_data = form_page_data("initData", init_data)?;
    if entry.path.to_string_lossy().ends_with(".lynxml") && global_props.is_some() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "globalProps is not supported for LynXML; use initData or a compiled template.",
        ));
    }
    let source = required_form_field(source_name, source)?;
    let timeout_ms = form_duration("timeoutMs", timeout_ms, DEFAULT_TIMEOUT_MS)?;
    if timeout_ms == 0 {
        return Err(ApiError::bad_request(
            "timeoutMs must be greater than zero.",
        ));
    }
    Ok(ScreenshotForm {
        entry,
        viewport,
        global_props,
        init_data,
        source,
        screenshot_settle: Duration::from_millis(form_duration(
            "screenshotSettleMs",
            screenshot_settle_ms,
            if page_options {
                DEFAULT_SCREENSHOT_SETTLE_MS
            } else {
                500
            },
        )?),
        timeout: Duration::from_millis(timeout_ms),
    })
}

fn required_form_field(name: &str, value: Option<Vec<u8>>) -> Result<Vec<u8>, ApiError> {
    value.ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("Missing {name} multipart field."),
        )
    })
}

fn form_text<'a>(name: &str, bytes: &'a [u8]) -> Result<&'a str, ApiError> {
    std::str::from_utf8(bytes).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("{name} must be valid UTF-8."),
        )
    })
}

fn form_dimension(name: &str, bytes: Option<Vec<u8>>, default: usize) -> Result<usize, ApiError> {
    match bytes {
        Some(bytes) => form_text(name, &bytes)?.parse().map_err(|_| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("{name} must be a positive integer."),
            )
        }),
        None => Ok(default),
    }
}

fn form_duration(name: &str, bytes: Option<Vec<u8>>, default: u64) -> Result<u64, ApiError> {
    match bytes {
        Some(bytes) => form_text(name, &bytes)?.parse().map_err(|_| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("{name} must be a non-negative integer."),
            )
        }),
        None => Ok(default),
    }
}

fn form_page_data(name: &str, bytes: Option<Vec<u8>>) -> Result<Option<Value>, ApiError> {
    match bytes {
        Some(bytes) => {
            let value = serde_json::from_slice(&bytes).map_err(|_| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("{name} must be a JSON object."),
                )
            })?;
            if !matches!(value, Value::Object(_)) {
                return Err(ApiError::bad_request(format!(
                    "{name} must be a JSON object."
                )));
            }
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

fn screenshot_form_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "Screenshot multipart fields exceed the {MAX_SCREENSHOT_FORM_BYTES}-byte limit (64 KiB additional framing allowed)."
        ),
    )
}

fn remote_url_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("Remote URL exceeds the {MAX_REMOTE_URL_BYTES}-byte limit."),
    )
}

fn multipart_error(error: &MultipartError) -> ApiError {
    ApiError::new(
        error.status(),
        format!("Invalid multipart request: {}", error.body_text()),
    )
}

#[cfg(test)]
pub(super) mod tests {
    use axum::body::Body;
    use axum::http::header::CONTENT_TYPE;
    use axum::response::IntoResponse;

    use super::*;

    pub(crate) fn multipart_request(path: &str, fields: &[(&str, &[u8])]) -> Request {
        let mut body = Vec::new();
        for (name, bytes) in fields {
            body.extend_from_slice(
                format!("--capture\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n")
                    .as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(b"--capture--\r\n");
        Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "multipart/form-data; boundary=capture")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn matches_defaults_and_page_options() {
        for (source_name, entry) in [("url", "template.js"), ("source", "pages/index.lynxml")] {
            let base = [
                ("entry", entry.as_bytes()),
                (source_name, b"source".as_slice()),
            ];
            let form = read(multipart_request("/", &base), source_name, true)
                .await
                .unwrap();
            assert_eq!((form.viewport.width, form.viewport.height), (800, 600));
            assert_eq!(form.screenshot_settle, Duration::from_millis(16));
            assert_eq!(form.timeout, Duration::from_mins(1));
            assert_eq!(form.entry.url, format!("zip:///{entry}"));
            let mut fields = base.to_vec();
            fields.extend_from_slice(&[
                ("width", b"375"),
                ("height", b"812"),
                ("initData", br#"{"ready":true}"#),
                ("screenshotSettleMs", b"0"),
                ("timeoutMs", b"2345"),
            ]);
            let form = read(multipart_request("/", &fields), source_name, true)
                .await
                .unwrap();
            assert_eq!((form.viewport.width, form.viewport.height), (375, 812));
            assert!(form.screenshot_settle.is_zero());
            assert_eq!(form.timeout, Duration::from_millis(2345));
            assert_eq!(form.init_data, Some(serde_json::json!({"ready":true})));
        }
        let form = read(
            multipart_request(
                "/",
                &[
                    ("entry", b"template.js"),
                    ("file", b"zip"),
                    ("globalProps", b"{}"),
                ],
            ),
            "file",
            false,
        )
        .await
        .unwrap();
        assert_eq!(form.screenshot_settle, Duration::from_millis(500));
        assert_eq!(form.timeout, Duration::from_mins(1));
        assert_eq!(form.global_props, Some(serde_json::json!({})));
    }

    #[tokio::test]
    async fn rejects_invalid_duplicate_and_old_json_fields() {
        let base = [
            ("entry", b"template.js".as_slice()),
            ("url", b"https://example.com/template.js".as_slice()),
        ];
        for (key, value) in [
            ("width", "0"),
            ("height", "8193"),
            ("height", "12.5"),
            ("height", "8192"),
            ("timeoutMs", "0"),
            ("timeoutMs", "-1"),
            ("screenshotSettleMs", "18446744073709551616"),
            ("initData", "[]"),
            ("initData", "null"),
            ("globalProps", "false"),
            ("task", "capture"),
            ("steps", "[]"),
            ("initialData", "{}"),
            ("screenshot_settle_ms", "0"),
        ] {
            let mut fields = base.to_vec();
            fields.push((key, value.as_bytes()));
            assert_eq!(
                read(multipart_request("/", &fields), "url", true)
                    .await
                    .unwrap_err()
                    .into_response()
                    .status(),
                StatusCode::BAD_REQUEST,
                "{key}={value}"
            );
        }
        for key in ["entry", "url", "width", "initData", "timeoutMs"] {
            let mut fields = base.to_vec();
            fields.extend_from_slice(&[(key, b"1"), (key, b"1")]);
            assert_eq!(
                read(multipart_request("/", &fields), "url", true)
                    .await
                    .unwrap_err()
                    .into_response()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        for fields in [
            vec![("entry", b"index.lynxml".as_slice())],
            vec![("source", b"source".as_slice())],
            vec![
                ("entry", b"index.lynxml".as_slice()),
                ("source", b"source"),
                ("globalProps", b"{}"),
            ],
        ] {
            assert_eq!(
                read(multipart_request("/", &fields), "source", true)
                    .await
                    .unwrap_err()
                    .into_response()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        for name in ["screenshotSettleMs", "timeoutMs"] {
            let fields = [
                ("entry", b"index.lynxml".as_slice()),
                ("file", b"zip"),
                (name, b"1"),
            ];
            assert_eq!(
                read(multipart_request("/", &fields), "file", false)
                    .await
                    .unwrap_err()
                    .into_response()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
    }

    #[test]
    fn matches_entry_path_and_pixel_bounds() {
        assert_eq!(
            ScreenshotEntry::parse("ZIP:///pages/a%20b.lynxml")
                .unwrap()
                .url,
            "zip:///pages/a%20b.lynxml"
        );
        for entry in [
            "",
            "/tmp/page",
            "../page",
            "a/../page",
            "a/%2e%2e/page",
            "%2ftmp/page",
            "file:///tmp/page",
            "C:/page",
            "a\\page",
            "a%00b",
            "a%ZZ",
            "a?b",
            "a#b",
        ] {
            assert!(ScreenshotEntry::parse(entry).is_err(), "{entry}");
        }
        assert!(ScreenshotViewport::new(8192, 320).is_ok());
        assert!(ScreenshotViewport::new(8192, 321).is_err());
        assert!(ScreenshotViewport::new(usize::MAX, usize::MAX).is_err());
    }

    #[tokio::test]
    async fn bounds_body_upload_and_remote_url() {
        let fields = [("entry", b"index.lynxml".as_slice()), ("source", b"source")];
        let mut request = multipart_request("/", &fields);
        request.headers_mut().insert(
            CONTENT_LENGTH,
            (MAX_SCREENSHOT_REQUEST_BYTES + 1)
                .to_string()
                .parse()
                .unwrap(),
        );
        assert_eq!(
            read(request, "source", true)
                .await
                .unwrap_err()
                .into_response()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let oversized = vec![b'x'; MAX_SCREENSHOT_FORM_BYTES];
        let fields = [
            ("entry", b"index.lynxml".as_slice()),
            ("source", oversized.as_slice()),
        ];
        assert_eq!(
            read(multipart_request("/", &fields), "source", true)
                .await
                .unwrap_err()
                .into_response()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let oversized = vec![b'x'; MAX_REMOTE_URL_BYTES + 1];
        let fields = [
            ("entry", b"template.js".as_slice()),
            ("url", oversized.as_slice()),
        ];
        assert_eq!(
            read(multipart_request("/", &fields), "url", true)
                .await
                .unwrap_err()
                .into_response()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let error =
            read_screenshot_form_with_deadline(std::future::pending(), Duration::from_millis(1))
                .await
                .unwrap_err();
        assert_eq!(error.into_response().status(), StatusCode::REQUEST_TIMEOUT);
    }
}
