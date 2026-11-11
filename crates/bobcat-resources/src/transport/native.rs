//! [`NativeFetch`]: the [`Fetch`] of macOS and Linux — `file:` URLs from the
//! local filesystem and `http(s)` through libcurl loaded at runtime.

use std::time::Duration;

use bobcat_core::resource::{ResourceErrorKind, ResourceErrorPhase, RetryAdvice};
use bytes::Bytes;
use http::HeaderMap;
use url::Url;
use vant::error::Failure;
use vant::{Fetch, Request, Response};

use super::{curl, file};
use crate::file_body::Body;

/// The native transport. It holds nothing: libcurl is loaded once per
/// process, on the first `http(s)` request, and a host without it gets a
/// precise `Unavailable` for every such request.
///
/// It applies no HTTP cache semantics of its own, so the resource system
/// puts the byte tier in front of it when the platform has a
/// [`Store`](vant::Store).
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFetch;

impl Fetch for NativeFetch {
    fn fetch_blocking(&self, request: &Request) -> Option<Result<Response, Failure>> {
        Some(match request.url.scheme() {
            "file" => file::read(&request.url, Body::Read).map(|body| Response {
                url: request.url.clone(),
                status: 200,
                headers: HeaderMap::new(),
                body,
            }),
            "http" | "https" => http(request),
            other => Err(Failure::new(
                ResourceErrorKind::UnsupportedScheme,
                ResourceErrorPhase::Open,
                format!("no transport serves `{other}:` URLs"),
            )),
        })
    }

    fn owns_http_cache(&self) -> bool {
        false
    }

    fn map_file(&self, url: &Url) -> Option<Result<Bytes, Failure>> {
        (url.scheme() == "file").then(|| file::read(url, Body::Mapped))
    }
}

/// One `http(s)` request through libcurl.
fn http(request: &Request) -> Result<Response, Failure> {
    let mut curl_request = curl::HttpRequest::new(request.url.as_str());
    curl_request.timeout = request.timeout;
    curl_request.max_body = request.max_body;
    curl_request.max_redirects = request.max_redirects;
    curl_request.user_agent.clone_from(&request.user_agent);
    for (name, value) in &request.headers {
        if let Ok(value) = value.to_str() {
            curl_request
                .headers
                .push((name.to_string(), value.to_owned()));
        }
    }
    let response = curl::Curl::load()
        .and_then(|curl| curl.get(&curl_request))
        .map_err(|error| transport_failure(&error))?;
    Ok(Response {
        url: Url::parse(&response.effective_url).unwrap_or_else(|_| request.url.clone()),
        status: response.status.as_u16(),
        headers: response.headers,
        body: Bytes::from(response.body),
    })
}

fn transport_failure(error: &curl::TransportError) -> Failure {
    use curl::TransportError;

    let (kind, phase) = match error {
        TransportError::Unavailable(_) => {
            (ResourceErrorKind::Unavailable, ResourceErrorPhase::Open)
        }
        TransportError::InvalidRequest(_) => {
            (ResourceErrorKind::InvalidRequest, ResourceErrorPhase::Open)
        }
        TransportError::Dns => (ResourceErrorKind::Dns, ResourceErrorPhase::Connect),
        TransportError::Connect(_) => (ResourceErrorKind::Connect, ResourceErrorPhase::Connect),
        TransportError::Tls(_) => (ResourceErrorKind::Tls, ResourceErrorPhase::Connect),
        TransportError::TooManyRedirects => (
            ResourceErrorKind::TooManyRedirects,
            ResourceErrorPhase::ReceiveHeaders,
        ),
        TransportError::TooLarge { .. } => (
            ResourceErrorKind::ResponseTooLarge,
            ResourceErrorPhase::ReadBody,
        ),
        TransportError::Timeout => (ResourceErrorKind::Io, ResourceErrorPhase::ReadBody),
        TransportError::Curl { .. } => {
            (ResourceErrorKind::Protocol, ResourceErrorPhase::SendRequest)
        }
    };
    let retry = match error {
        TransportError::Dns | TransportError::Connect(_) | TransportError::Timeout => {
            RetryAdvice::After(Duration::from_secs(1))
        }
        _ => RetryAdvice::Never,
    };
    Failure::new(kind, phase, error.to_string()).with_retry(retry)
}
