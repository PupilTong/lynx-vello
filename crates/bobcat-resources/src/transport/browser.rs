//! `http(s)` in the browser: the Render Worker's own `fetch`, which applies
//! the page's origin, CORS, credentials and HTTP-cache policy for free.
//!
//! The browser's HTTP cache stands in for the disk tier here, driven by the
//! same [`CachePolicy`] through the request's cache mode; what it did is
//! opaque, so a response reports no cache status of its own.

use bobcat_core::resource::{
    CachePolicy, CacheStatus, ResourceErrorKind, ResourceErrorPhase, ResourceSource, ResourceTiming,
};
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use url::Url;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestCache, RequestInit, Response, WorkerGlobalScope};

use super::{Fetched, HttpSettings, status_failure};
use crate::error::Failure;
use crate::mime::MediaType;

pub(crate) async fn fetch(
    url: &Url,
    policy: CachePolicy,
    headers: &HeaderMap,
    settings: &HttpSettings,
) -> Result<Fetched, Failure> {
    let scope = js_sys::global()
        .dyn_into::<WorkerGlobalScope>()
        .map_err(|_| {
            Failure::new(
                ResourceErrorKind::Unavailable,
                ResourceErrorPhase::Open,
                "browser fetch is only available inside a Worker",
            )
        })?;
    let request_headers =
        Headers::new().map_err(|error| js_failure(ResourceErrorPhase::Open, &error))?;
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            let _ = request_headers.append(name.as_str(), value);
        }
    }
    let init = RequestInit::new();
    init.set_method("GET");
    init.set_headers(&request_headers);
    init.set_cache(match policy {
        CachePolicy::NoStore => RequestCache::NoStore,
        CachePolicy::Reload => RequestCache::Reload,
        CachePolicy::NoCache => RequestCache::NoCache,
        CachePolicy::ForceCache => RequestCache::ForceCache,
        CachePolicy::OnlyIfCached => RequestCache::OnlyIfCached,
        _ => RequestCache::Default,
    });
    let request = Request::new_with_str_and_init(url.as_str(), &init)
        .map_err(|error| js_failure(ResourceErrorPhase::Open, &error))?;
    let started = web_time::Instant::now();
    let response = JsFuture::from(scope.fetch_with_request(&request))
        .await
        .map_err(|error| js_failure(ResourceErrorPhase::Connect, &error))?
        .dyn_into::<Response>()
        .map_err(|_| {
            Failure::new(
                ResourceErrorKind::Protocol,
                ResourceErrorPhase::ReceiveHeaders,
                "fetch resolved with something other than a Response",
            )
        })?;
    let status = StatusCode::from_u16(response.status()).map_err(|_| {
        Failure::new(
            ResourceErrorKind::Protocol,
            ResourceErrorPhase::ReceiveHeaders,
            format!("impossible status {}", response.status()),
        )
    })?;
    let response_headers = collect_headers(&response.headers());
    if let Some(failure) = status_failure(status) {
        return Err(failure);
    }
    let buffer = JsFuture::from(
        response
            .array_buffer()
            .map_err(|error| js_failure(ResourceErrorPhase::ReadBody, &error))?,
    )
    .await
    .map_err(|error| js_failure(ResourceErrorPhase::ReadBody, &error))?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    if bytes.len() > settings.max_body {
        return Err(Failure::new(
            ResourceErrorKind::ResponseTooLarge,
            ResourceErrorPhase::ReadBody,
            format!("the response exceeded {} bytes", settings.max_body),
        ));
    }
    let media_type = response_headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(MediaType::parse)
        .or_else(|| crate::mime::from_extension(url.path()));
    let final_url = Url::parse(&response.url()).unwrap_or_else(|_| url.clone());
    Ok(Fetched {
        bytes: Bytes::from(bytes),
        media_type,
        redirects: if final_url == *url {
            Vec::new()
        } else {
            vec![final_url.clone()]
        },
        url: final_url,
        source: ResourceSource::Network,
        cache_status: CacheStatus::NotApplicable,
        headers: response_headers,
        timing: ResourceTiming {
            total: Some(started.elapsed()),
            ..ResourceTiming::default()
        },
        // The browser's HTTP cache cannot be read synchronously.
        restorable: false,
    })
}

fn collect_headers(headers: &Headers) -> HeaderMap {
    let mut map = HeaderMap::new();
    let Ok(iterator) = js_sys::try_iter(headers.entries().as_ref()) else {
        return map;
    };
    let Some(iterator) = iterator else {
        return map;
    };
    for entry in iterator.flatten() {
        let pair = js_sys::Array::from(&entry);
        let (Some(name), Some(value)) = (pair.get(0).as_string(), pair.get(1).as_string()) else {
            continue;
        };
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            map.append(name, value);
        }
    }
    map
}

fn js_failure(phase: ResourceErrorPhase, error: &wasm_bindgen::JsValue) -> Failure {
    let message = error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"));
    let kind = match phase {
        ResourceErrorPhase::Connect => ResourceErrorKind::Connect,
        ResourceErrorPhase::ReadBody => ResourceErrorKind::ResponseBody,
        _ => ResourceErrorKind::Other,
    };
    Failure::new(kind, phase, message)
}
