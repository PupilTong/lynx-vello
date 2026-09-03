//! Transports: how bytes are moved for each kind of URL, and the cache tiers
//! they pass through on the way.
//!
//! Order of precedence is fixed. A URL the embedder registered is served
//! from the registry whatever its scheme. `data:` URLs are their own bytes.
//! `file:` URLs read the local filesystem (native only). `http` and `https`
//! go through the disk cache's HTTP semantics and then the platform's own
//! client — libcurl loaded at runtime natively, `fetch` in the browser.
//! Anything else is refused at resolution, before a request exists.

#[cfg(target_arch = "wasm32")]
pub(crate) mod browser;
#[cfg(not(target_arch = "wasm32"))]
pub mod curl;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod file;

use std::time::Duration;

use bobcat_core::resource::{
    CachePolicy, CacheStatus, ResourceErrorKind, ResourceErrorPhase, ResourceLocality,
    ResourceSource, ResourceTiming, RetryAdvice,
};
use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use url::Url;

use crate::data_url;
use crate::error::Failure;
use crate::mime::MediaType;
use crate::registry::{Registered, Registry};

/// What the HTTP transports are configured with.
///
/// The browser's `fetch` owns its own timeouts, redirect limit and user
/// agent, so only the body bound reaches it there.
#[derive(Clone, Debug)]
pub(crate) struct HttpSettings {
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "fetch times out on its own")
    )]
    pub timeout: Duration,
    pub max_body: usize,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "fetch sends the browser's")
    )]
    pub user_agent: String,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "fetch follows redirects itself")
    )]
    pub max_redirects: u32,
}

/// Bytes a transport produced, with everything the response metadata is
/// built from.
#[derive(Clone, Debug)]
pub(crate) struct Fetched {
    pub bytes: Bytes,
    /// The type the transport could label the bytes with: a `Content-Type`,
    /// a `data:` media type, a registration's type, or a file's extension.
    pub media_type: Option<MediaType>,
    /// The URL the bytes finally came from, after redirects.
    pub url: Url,
    pub redirects: Vec<Url>,
    pub source: ResourceSource,
    pub cache_status: CacheStatus,
    pub headers: HeaderMap,
    pub timing: ResourceTiming,
    /// Whether the transport can hand these bytes back again synchronously
    /// and without the network — a file, or a response the disk tier holds.
    /// The image pipeline keeps the bytes of anything that cannot, since a
    /// read after eviction has to come from somewhere.
    pub restorable: bool,
}

/// Every transport, behind one dispatch.
pub(crate) struct Transports {
    pub registry: Registry,
    pub http: HttpSettings,
    #[cfg(not(target_arch = "wasm32"))]
    pub disk: Option<crate::cache::disk::DiskCache>,
}

impl std::fmt::Debug for Transports {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Transports")
            .field("registered", &self.registry.len())
            .finish_non_exhaustive()
    }
}

impl Transports {
    /// Resolves `specifier` against `base`, refusing what nothing here could
    /// serve — so the failure is at resolution, where the protocol reports
    /// it, and never a request that was doomed from the start.
    pub(crate) fn resolve(&self, specifier: &str, base: Option<&Url>) -> Result<Url, Failure> {
        let url = Url::parse(specifier)
            .or_else(|_| {
                base.ok_or(url::ParseError::RelativeUrlWithoutBase)
                    .and_then(|base| base.join(specifier))
            })
            .map_err(|error| {
                Failure::new(
                    ResourceErrorKind::InvalidUrl,
                    ResourceErrorPhase::Resolve,
                    format!("`{specifier}` is not a URL: {error}"),
                )
            })?;
        if self.registry.contains(&url) || Self::serves_scheme(url.scheme()) {
            Ok(url)
        } else {
            Err(Failure::new(
                ResourceErrorKind::UnsupportedScheme,
                ResourceErrorPhase::Resolve,
                format!("no transport serves `{}:` URLs", url.scheme()),
            ))
        }
    }

    fn serves_scheme(scheme: &str) -> bool {
        matches!(scheme, "http" | "https" | "data")
            || (cfg!(not(target_arch = "wasm32")) && scheme == "file")
    }

    pub(crate) fn locality(url: &Url) -> ResourceLocality {
        match url.scheme() {
            "http" | "https" => ResourceLocality::Remote,
            _ => ResourceLocality::Local,
        }
    }

    /// Resources that are already in hand: registered contents and `data:`
    /// URLs. `None` when the URL needs a transport.
    pub(crate) fn local(&self, url: &Url) -> Option<Result<Fetched, Failure>> {
        if let Some(registered) = self.registry.get(url) {
            return Some(match registered {
                Registered::Bytes { bytes, media_type } => Ok(Fetched {
                    bytes,
                    media_type,
                    url: url.clone(),
                    redirects: Vec::new(),
                    source: ResourceSource::PackagedAsset,
                    cache_status: CacheStatus::NotApplicable,
                    headers: HeaderMap::new(),
                    timing: ResourceTiming::default(),
                    // A registration may be cleared later, and its bytes are
                    // shared rather than copied, so keeping them costs nothing.
                    restorable: false,
                }),
                Registered::StyleSheet(_) => Err(Failure::new(
                    ResourceErrorKind::UnsupportedOperation,
                    ResourceErrorPhase::Open,
                    "the URL is registered as a pre-parsed stylesheet, which has no bytes",
                )),
            });
        }
        if url.scheme() == "data" {
            return Some(
                data_url::parse(url)
                    .map(|decoded| Fetched {
                        bytes: Bytes::from(decoded.bytes),
                        media_type: Some(decoded.media_type),
                        url: url.clone(),
                        redirects: Vec::new(),
                        source: ResourceSource::DataUrl,
                        cache_status: CacheStatus::NotApplicable,
                        headers: HeaderMap::new(),
                        timing: ResourceTiming::default(),
                        restorable: false,
                    })
                    .map_err(|error| {
                        Failure::new(
                            ResourceErrorKind::InvalidUrl,
                            ResourceErrorPhase::Open,
                            error.to_string(),
                        )
                    }),
            );
        }
        None
    }

    /// Moves the bytes for `url`, blocking the calling thread — an IO
    /// worker's, never the painter's.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn fetch_blocking(
        &self,
        url: &Url,
        policy: CachePolicy,
        headers: &HeaderMap,
    ) -> Result<Fetched, Failure> {
        if let Some(local) = self.local(url) {
            return local;
        }
        match url.scheme() {
            "file" => file::read(url).map(|(bytes, media_type)| Fetched {
                bytes,
                media_type,
                url: url.clone(),
                redirects: Vec::new(),
                source: ResourceSource::FileSystem,
                cache_status: CacheStatus::NotApplicable,
                headers: HeaderMap::new(),
                timing: ResourceTiming::default(),
                restorable: true,
            }),
            "http" | "https" => self.http_blocking(url, policy, headers),
            other => Err(Failure::new(
                ResourceErrorKind::UnsupportedScheme,
                ResourceErrorPhase::Open,
                format!("no transport serves `{other}:` URLs"),
            )),
        }
    }

    /// The HTTP path natively: the disk cache decides whether a request is
    /// needed, libcurl makes it, and a storable answer goes back to disk.
    #[cfg(not(target_arch = "wasm32"))]
    fn http_blocking(
        &self,
        url: &Url,
        policy: CachePolicy,
        headers: &HeaderMap,
    ) -> Result<Fetched, Failure> {
        use std::time::SystemTime;

        use crate::cache::http::{Plan, StoredResponse, plan};

        let key = url.as_str();
        let now = SystemTime::now();
        let stored = self.disk.as_ref().and_then(|disk| disk.entry(key));
        let mut plan = plan(policy, stored.as_ref().map(|entry| &entry.response), now);
        if matches!(plan, Plan::UseStored) {
            match self.disk.as_ref().and_then(|disk| disk.get(key)) {
                Some((entry, bytes)) => {
                    return Self::stored_response(url, entry, bytes, CacheStatus::HitDisk);
                }
                // The record was there but the body was not: fetch instead.
                None => plan = Plan::Fetch,
            }
        }
        if matches!(plan, Plan::Unavailable) {
            return Err(Failure::new(
                ResourceErrorKind::Unavailable,
                ResourceErrorPhase::Open,
                "only-if-cached: the resource is not in the cache",
            ));
        }
        let revalidating = matches!(plan, Plan::Revalidate);
        let conditional = stored
            .as_ref()
            .filter(|_| revalidating)
            .map(|entry| entry.response.revalidation_headers());
        let request = self.http_request(url, headers, conditional.as_ref());
        let response = curl::Curl::load()
            .and_then(|curl| curl.get(&request))
            .map_err(|error| transport_failure(&error))?;

        if revalidating
            && response.status == StatusCode::NOT_MODIFIED
            && let (Some(disk), Some(entry)) = (self.disk.as_ref(), stored.as_ref())
        {
            let refreshed = entry.response.refreshed_by(&response.headers, now);
            let _ = disk.update_response(key, &refreshed);
            if let Some((entry, bytes)) = disk.get(key) {
                return Self::stored_response(url, entry, bytes, CacheStatus::Revalidated);
            }
        }
        let record = StoredResponse {
            status: response.status.as_u16(),
            headers: response.headers.clone(),
            stored_at: now,
        };
        let media_type = response
            .headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(MediaType::parse)
            .or_else(|| crate::mime::from_extension(url.path()));
        let cache_status = if matches!(plan, Plan::FetchNoStore | Plan::Fetch)
            && !matches!(policy, CachePolicy::Default)
        {
            CacheStatus::Bypassed
        } else {
            CacheStatus::Miss
        };
        let mut stored_on_disk = false;
        if !matches!(plan, Plan::FetchNoStore)
            && record.is_storable()
            && let Some(disk) = self.disk.as_ref()
        {
            stored_on_disk = disk
                .put(
                    key,
                    media_type.as_ref().map(ToString::to_string).as_deref(),
                    &record,
                    &response.body,
                )
                .is_ok();
        }
        if let Some(failure) = status_failure(response.status) {
            return Err(failure);
        }
        let redirects = response
            .redirects
            .iter()
            .filter_map(|location| Url::parse(location).ok())
            .collect();
        Ok(Fetched {
            bytes: Bytes::from(response.body),
            media_type,
            url: Url::parse(&response.effective_url).unwrap_or_else(|_| url.clone()),
            redirects,
            source: ResourceSource::Network,
            cache_status,
            headers: response.headers,
            timing: ResourceTiming {
                resolve: response.timing.name_lookup,
                connect: response.timing.connect,
                time_to_first_byte: response.timing.start_transfer,
                transfer: None,
                total: response.timing.total,
            },
            restorable: stored_on_disk,
        })
    }

    /// The libcurl request for `url`: the caller's headers plus any
    /// conditional ones a revalidation adds.
    #[cfg(not(target_arch = "wasm32"))]
    fn http_request(
        &self,
        url: &Url,
        headers: &HeaderMap,
        conditional: Option<&HeaderMap>,
    ) -> curl::HttpRequest {
        let mut request = curl::HttpRequest::new(url.as_str());
        request.timeout = self.http.timeout;
        request.max_body = self.http.max_body;
        request.max_redirects = self.http.max_redirects;
        request.user_agent.clone_from(&self.http.user_agent);
        for (name, value) in headers.iter().chain(conditional.into_iter().flatten()) {
            if let Ok(value) = value.to_str() {
                request.headers.push((name.to_string(), value.to_owned()));
            }
        }
        request
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn stored_response(
        url: &Url,
        entry: crate::cache::disk::DiskEntry,
        bytes: Vec<u8>,
        cache_status: CacheStatus,
    ) -> Result<Fetched, Failure> {
        let status = StatusCode::from_u16(entry.response.status).ok();
        if let Some(failure) = status.and_then(status_failure) {
            return Err(failure);
        }
        let media_type = entry
            .media_type
            .as_deref()
            .and_then(MediaType::parse)
            .or_else(|| {
                entry
                    .response
                    .headers
                    .get(http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(MediaType::parse)
            })
            .or_else(|| crate::mime::from_extension(url.path()));
        Ok(Fetched {
            bytes: Bytes::from(bytes),
            media_type,
            url: url.clone(),
            redirects: Vec::new(),
            source: ResourceSource::DiskCache,
            cache_status,
            headers: entry.response.headers,
            timing: ResourceTiming::default(),
            restorable: true,
        })
    }

    /// Moves the bytes for `url` in the browser: registered and `data:`
    /// resources synchronously, everything else through `fetch`.
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn fetch(
        &self,
        url: &Url,
        policy: CachePolicy,
        headers: &HeaderMap,
    ) -> Result<Fetched, Failure> {
        if let Some(local) = self.local(url) {
            return local;
        }
        match url.scheme() {
            "http" | "https" => browser::fetch(url, policy, headers, &self.http).await,
            other => Err(Failure::new(
                ResourceErrorKind::UnsupportedScheme,
                ResourceErrorPhase::Open,
                format!("no transport serves `{other}:` URLs"),
            )),
        }
    }
}

/// The failure an HTTP status is, if it is one: every non-success status
/// after redirects were followed.
pub(crate) fn status_failure(status: StatusCode) -> Option<Failure> {
    if status.is_success() {
        return None;
    }
    let kind = match status.as_u16() {
        404 | 410 => ResourceErrorKind::NotFound,
        401 | 403 | 407 => ResourceErrorKind::PermissionDenied,
        408 | 429 | 500..=599 => ResourceErrorKind::Unavailable,
        _ => ResourceErrorKind::Protocol,
    };
    let retry = match status.as_u16() {
        408 | 429 | 502 | 503 | 504 => RetryAdvice::After(Duration::from_secs(1)),
        _ => RetryAdvice::Never,
    };
    Some(
        Failure::new(
            kind,
            ResourceErrorPhase::ReceiveHeaders,
            format!("the server answered {status}"),
        )
        .with_status(status)
        .with_retry(retry),
    )
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn transports() -> Transports {
        Transports {
            registry: Registry::default(),
            http: HttpSettings {
                timeout: Duration::from_secs(5),
                max_body: 1024,
                user_agent: "test".to_owned(),
                max_redirects: 2,
            },
            #[cfg(not(target_arch = "wasm32"))]
            disk: None,
        }
    }

    #[test]
    fn resolution_joins_against_the_base_and_refuses_unknown_schemes() {
        let transports = transports();
        let base = Url::parse("https://cards.test/app/index.html").expect("a URL");
        assert_eq!(
            transports
                .resolve("../img/a.png", Some(&base))
                .unwrap()
                .as_str(),
            "https://cards.test/img/a.png"
        );
        assert_eq!(
            transports.resolve("data:,x", None).unwrap().scheme(),
            "data"
        );
        let failure = transports
            .resolve("relative.png", None)
            .expect_err("no base");
        assert_eq!(failure.kind, ResourceErrorKind::InvalidUrl);
        let failure = transports
            .resolve("ftp://x.test/a", None)
            .expect_err("unsupported");
        assert_eq!(failure.kind, ResourceErrorKind::UnsupportedScheme);
        let registered = Url::parse("bobcat-memory://bundle/root.js").expect("a URL");
        transports.registry.insert(
            &registered,
            Registered::Bytes {
                bytes: Bytes::from_static(b"1"),
                media_type: None,
            },
        );
        assert!(
            transports
                .resolve("bobcat-memory://bundle/root.js", None)
                .is_ok(),
            "any registered scheme resolves"
        );
        assert_eq!(Transports::locality(&registered), ResourceLocality::Local);
        assert_eq!(
            Transports::locality(&Url::parse("https://x.test/").unwrap()),
            ResourceLocality::Remote
        );
    }

    #[test]
    fn registered_and_data_resources_need_no_transport() {
        let transports = transports();
        let url = Url::parse("app:///main.js").expect("a URL");
        transports.registry.insert(
            &url,
            Registered::Bytes {
                bytes: Bytes::from_static(b"let a = 1;"),
                media_type: MediaType::parse("text/javascript"),
            },
        );
        let fetched = transports.local(&url).expect("registered").expect("bytes");
        assert_eq!(&fetched.bytes[..], b"let a = 1;");
        assert_eq!(fetched.source, ResourceSource::PackagedAsset);
        assert_eq!(fetched.media_type.unwrap().essence(), "text/javascript");

        let data = Url::parse("data:text/css,a%7B%7D").expect("a URL");
        let fetched = transports.local(&data).expect("data").expect("bytes");
        assert_eq!(&fetched.bytes[..], b"a{}");
        assert_eq!(fetched.source, ResourceSource::DataUrl);

        let sheet = Url::parse("app:///style.css").expect("a URL");
        transports.registry.insert(
            &sheet,
            Registered::StyleSheet(std::sync::Arc::new(
                bobcat_core::PreparsedStyleSheet::default(),
            )),
        );
        let failure = transports
            .local(&sheet)
            .expect("registered")
            .expect_err("no bytes");
        assert_eq!(failure.kind, ResourceErrorKind::UnsupportedOperation);
        assert!(
            transports
                .local(&Url::parse("https://x.test/a").unwrap())
                .is_none()
        );
    }

    #[test]
    fn statuses_map_to_kinds_and_retry_advice() {
        assert!(status_failure(StatusCode::OK).is_none());
        assert!(status_failure(StatusCode::NO_CONTENT).is_none());
        let not_found = status_failure(StatusCode::NOT_FOUND).unwrap();
        assert_eq!(not_found.kind, ResourceErrorKind::NotFound);
        assert_eq!(not_found.status, Some(StatusCode::NOT_FOUND));
        assert_eq!(
            status_failure(StatusCode::FORBIDDEN).unwrap().kind,
            ResourceErrorKind::PermissionDenied
        );
        let unavailable = status_failure(StatusCode::SERVICE_UNAVAILABLE).unwrap();
        assert_eq!(unavailable.kind, ResourceErrorKind::Unavailable);
        assert!(matches!(unavailable.retry, RetryAdvice::After(_)));
        assert_eq!(
            status_failure(StatusCode::IM_A_TEAPOT).unwrap().kind,
            ResourceErrorKind::Protocol
        );
    }
}
