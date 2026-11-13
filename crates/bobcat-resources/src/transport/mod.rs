//! Transports: how bytes are moved for each kind of URL, and the byte tier
//! they pass through on the way.
//!
//! Order of precedence is fixed. A URL the embedder registered is served
//! from the registry whatever its scheme. `data:` URLs are their own bytes.
//! Everything else goes to the platform's [`Fetch`]: `file:` URLs (native
//! only) straight there, and `http` and `https` through `vant`'s
//! [`ByteCache`] over the platform's [`Store`](vant::Store) first, when it
//! has one and the transport does not apply HTTP cache semantics itself.
//! Anything else is refused at resolution, before a request exists.
//!
//! The adapters this crate ships are here too: [`NativeFetch`] (libcurl
//! loaded at runtime, and the local filesystem) natively, and
//! [`BrowserFetch`] (the Render Worker's `fetch`) on wasm32.

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(not(target_arch = "wasm32"))]
pub mod curl;
#[cfg(not(target_arch = "wasm32"))]
mod file;
#[cfg(not(target_arch = "wasm32"))]
mod native;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bobcat_core::resource::{ResourceErrorKind, ResourceErrorPhase, RetryAdvice};
#[cfg(target_arch = "wasm32")]
pub use browser::BrowserFetch;
use bytes::Bytes;
use http::{HeaderMap, StatusCode};
#[cfg(not(target_arch = "wasm32"))]
pub use native::NativeFetch;
use url::Url;
use vant::byte_cache::{ByteCache, Entry, Lookup, Read};
use vant::error::Failure;
use vant::http::{Plan, plan};
use vant::{Fetch, Platform, Request, Response};

#[cfg(not(target_arch = "wasm32"))]
use crate::file_body::Body;
use crate::mime::MediaType;
use crate::registry::{Registered, Registry};
use crate::{CachePolicy, Handle, data_url};

/// What every request this system makes is sent with.
#[derive(Clone, Debug)]
pub(crate) struct HttpSettings {
    pub timeout: Duration,
    pub max_body: usize,
    pub user_agent: String,
    pub max_redirects: u32,
}

/// Bytes a transport produced, with what the pipeline behind it needs.
#[derive(Clone, Debug)]
pub(crate) struct Fetched {
    pub bytes: Bytes,
    /// The type the transport could label the bytes with: a `Content-Type`,
    /// a `data:` media type, a registration's type, or a file's extension.
    pub media_type: Option<MediaType>,
    /// The URL the bytes finally came from, after redirects.
    pub url: Url,
    /// Whether the transport can hand these bytes back again synchronously
    /// and without the network — a file, or a response the byte tier holds.
    /// The image pipeline keeps the bytes of anything that cannot, since a
    /// read after eviction has to come from somewhere.
    pub restorable: bool,
}

/// Whether bytes a transport hands over can be asked for again inside a
/// read. The browser's restore reads only what is in hand
/// (`images::restore_bytes`), so nothing its transport fetched is.
const RESTORES: bool = cfg!(not(target_arch = "wasm32"));

/// `specifier` resolved by URL rules: joined to `base` when there is one,
/// and parsed on its own when there is none.
///
/// With a base, the join is the whole algorithm, as it is where the engine
/// resolves a script URL against the view's base. Parsing first and joining
/// only on failure gives a different URL for one kind of input: a reference
/// that repeats the base's own special scheme, such as `https:x` against an
/// `https:` base, which a join reads as a relative path and a parse as the
/// host `x`.
pub(crate) fn join(specifier: &str, base: Option<&Url>) -> Result<Url, url::ParseError> {
    base.map_or_else(|| Url::parse(specifier), |base| base.join(specifier))
}

/// Every transport, behind one dispatch: the registry, `data:` URLs, the
/// byte tier and the platform's [`Fetch`].
pub(crate) struct Transports<P: Platform> {
    /// Its own `Arc` so that
    /// [`ResourceFetcher::style_sheet_probe`](bobcat_core::resource::ResourceFetcher::style_sheet_probe)
    /// can hand a reader of it to `bobcat-main`, as the fetch index is.
    pub registry: std::sync::Arc<Registry>,
    pub http: HttpSettings,
    pub fetch: Handle<P::Fetch>,
    /// The HTTP byte tier: `None` when the platform has no store, or when
    /// its transport applies HTTP cache semantics itself.
    pub bytes: Option<Handle<ByteCache<P::Store>>>,
}

impl<P: Platform> std::fmt::Debug for Transports<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Transports")
            .field("registered", &self.registry.len())
            .field("byte_tier", &self.bytes.is_some())
            .finish_non_exhaustive()
    }
}

/// What the byte tier decided about an `http(s)` request.
enum Planned {
    /// Answered without a request: stored bytes, or `only-if-cached` with
    /// nothing stored.
    Answered(Result<Fetched, Failure>),
    /// A request to make, and what to do with its answer.
    Fetch {
        request: Request,
        /// Whether a `304` refreshes the stored entry.
        revalidating: bool,
        /// Whether the answer may be stored.
        store: bool,
    },
}

impl<P: Platform> Transports<P> {
    /// The scope's transports over the platform's `fetch` and byte tier.
    pub(crate) fn new(
        http: HttpSettings,
        fetch: Handle<P::Fetch>,
        bytes: Option<Handle<ByteCache<P::Store>>>,
    ) -> Self {
        Self {
            registry: std::sync::Arc::default(),
            http,
            fetch,
            bytes,
        }
    }

    /// Transports for a new scope: a fresh registry, the same platform parts.
    pub(crate) fn for_new_scope(&self) -> Self {
        Self::new(
            self.http.clone(),
            Handle::clone(&self.fetch),
            self.bytes.clone(),
        )
    }

    /// Resolves `specifier` against `base`, refusing what nothing here could
    /// serve — so the failure is at resolution, where the protocol reports
    /// it, and never a request that was doomed from the start.
    pub(crate) fn resolve(&self, specifier: &str, base: Option<&Url>) -> Result<Url, Failure> {
        let url = join(specifier, base).map_err(|error| {
            Failure::new(
                ResourceErrorKind::InvalidUrl,
                ResourceErrorPhase::Resolve,
                format!("`{specifier}` is not a URL: {error}"),
            )
        })?;
        if self.registry.contains(&url)
            || self.registry.install_covering(&url).is_some()
            || Self::serves_scheme(url.scheme())
        {
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

    /// Resources that are already in hand: registered contents and `data:`
    /// URLs. `None` when the URL needs a transport.
    pub(crate) fn local(&self, url: &Url) -> Option<Result<Fetched, Failure>> {
        if let Some(registered) = self.registry.get(url) {
            return Some(match registered {
                Registered::Bytes { bytes, media_type } => Ok(Fetched {
                    bytes,
                    media_type,
                    url: url.clone(),
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

    /// Moves the bytes for `url`, blocking the calling thread: a
    /// blocking-pool thread for a background job, and the painter's own for
    /// the synchronous restore inside `FrameImages::read`, which has to
    /// answer within the call and so cannot await anything.
    ///
    /// `body` is how bytes read from a file — the `file:` URL's, or the byte
    /// tier's copy of a response — are handed over. Registered and `data:`
    /// bytes are already in memory and come as they are.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn fetch_blocking(
        &self,
        url: &Url,
        policy: CachePolicy,
        headers: &HeaderMap,
        body: Body,
    ) -> Result<Fetched, Failure> {
        if let Some(local) = self.local(url) {
            return local;
        }
        let read = match body {
            Body::Read => Read::Owned,
            Body::Mapped => Read::Mapped,
        };
        match url.scheme() {
            "file" => {
                let mapped = (body == Body::Mapped)
                    .then(|| self.fetch.map_file(url))
                    .flatten()
                    .map(|mapped| {
                        mapped.map(|bytes| Response {
                            url: url.clone(),
                            status: 200,
                            headers: HeaderMap::new(),
                            body: bytes,
                        })
                    });
                let response = mapped
                    .unwrap_or_else(|| self.blocking(&self.request(url, policy, headers, None)))?;
                Self::answered(url, response, true)
            }
            "http" | "https" => {
                let now = now();
                match self.plan_http(url, policy, headers, read, now) {
                    Planned::Answered(answer) => answer,
                    Planned::Fetch {
                        request,
                        revalidating,
                        store,
                    } => {
                        let response = self.blocking(&request);
                        self.finish_http(url, response, revalidating, store, read, now)
                    }
                }
            }
            other => Err(unsupported_scheme(other)),
        }
    }

    /// Moves the bytes for `url` in the browser: registered and `data:`
    /// resources synchronously, everything else through the platform's
    /// asynchronous [`Fetch::fetch`].
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
            "http" | "https" => {
                let now = now();
                match self.plan_http(url, policy, headers, Read::Owned, now) {
                    Planned::Answered(answer) => answer,
                    Planned::Fetch {
                        request,
                        revalidating,
                        store,
                    } => {
                        let response = self.fetch.fetch(request).await;
                        self.finish_http(url, response, revalidating, store, Read::Owned, now)
                    }
                }
            }
            other => Err(unsupported_scheme(other)),
        }
    }

    /// The platform's blocking fetch, or the failure a transport that
    /// cannot block answers with.
    #[cfg(not(target_arch = "wasm32"))]
    fn blocking(&self, request: &Request) -> Result<Response, Failure> {
        self.fetch.fetch_blocking(request).unwrap_or_else(|| {
            Err(Failure::new(
                ResourceErrorKind::UnsupportedOperation,
                ResourceErrorPhase::Open,
                "this transport cannot fetch synchronously",
            ))
        })
    }

    /// The request for `url`: the caller's headers plus any conditional
    /// ones a revalidation adds.
    fn request(
        &self,
        url: &Url,
        policy: CachePolicy,
        headers: &HeaderMap,
        conditional: Option<&HeaderMap>,
    ) -> Request {
        let mut headers = headers.clone();
        for (name, value) in conditional.into_iter().flatten() {
            headers.append(name.clone(), value.clone());
        }
        Request {
            url: url.clone(),
            policy,
            headers,
            timeout: self.http.timeout,
            max_body: self.http.max_body,
            max_redirects: self.http.max_redirects,
            user_agent: self.http.user_agent.clone(),
        }
    }

    /// The first half of an `http(s)` load: the byte tier decides whether a
    /// request is needed. Without a byte tier in front of a transport that
    /// keeps no cache either, nothing is ever stored, so `only-if-cached`
    /// cannot be served.
    fn plan_http(
        &self,
        url: &Url,
        policy: CachePolicy,
        headers: &HeaderMap,
        read: Read,
        now: SystemTime,
    ) -> Planned {
        let Some(bytes) = self.bytes.as_deref() else {
            // With nothing stored the plan does not read the clock.
            if !self.fetch.owns_http_cache()
                && matches!(plan(policy, None, UNIX_EPOCH), Plan::Unavailable)
            {
                return Planned::Answered(Err(not_cached()));
            }
            return Planned::Fetch {
                request: self.request(url, policy, headers, None),
                revalidating: false,
                store: false,
            };
        };
        match bytes.lookup(url.as_str(), policy, now, read) {
            Lookup::UseStored(entry, body) => {
                Planned::Answered(Self::stored_response(url, &entry, body))
            }
            Lookup::Unavailable => Planned::Answered(Err(not_cached())),
            Lookup::Fetch {
                conditional,
                revalidating,
                store,
            } => Planned::Fetch {
                request: self.request(url, policy, headers, conditional.as_ref()),
                revalidating,
                store,
            },
        }
    }

    /// The second half of an `http(s)` load: a `304` refreshes what is
    /// stored, a storable answer is stored, and a failure status fails.
    fn finish_http(
        &self,
        url: &Url,
        response: Result<Response, Failure>,
        revalidating: bool,
        store: bool,
        read: Read,
        now: SystemTime,
    ) -> Result<Fetched, Failure> {
        let response = response?;
        let key = url.as_str();
        if revalidating
            && response.status == StatusCode::NOT_MODIFIED.as_u16()
            && let Some(bytes) = self.bytes.as_deref()
            && let Some((entry, body)) = bytes.refreshed(key, &response.headers, now, read)
        {
            return Self::stored_response(url, &entry, body);
        }
        let media_type = media_type(&response.headers, url);
        let stored = store
            && self.bytes.as_deref().is_some_and(|bytes| {
                bytes.store(
                    key,
                    media_type.as_ref().map(ToString::to_string).as_deref(),
                    &response,
                    now,
                )
            });
        if let Some(failure) = status_failure(status_code(response.status)?) {
            return Err(failure);
        }
        // Bytes to be retained are handed over as the stored copy's mapping
        // once there is one, so the download's buffer is dropped here rather
        // than held as long as the bytes are.
        let mapped = (stored && read == Read::Mapped)
            .then(|| self.bytes.as_deref()?.map_body(key))
            .flatten();
        Ok(Fetched {
            bytes: mapped.unwrap_or(response.body),
            media_type,
            url: response.url,
            restorable: stored && RESTORES,
        })
    }

    /// A response that needs no byte tier — a file's — as the pipeline
    /// takes it.
    #[cfg(not(target_arch = "wasm32"))]
    fn answered(url: &Url, response: Response, restorable: bool) -> Result<Fetched, Failure> {
        if let Some(failure) = status_failure(status_code(response.status)?) {
            return Err(failure);
        }
        Ok(Fetched {
            media_type: media_type(&response.headers, url),
            bytes: response.body,
            url: response.url,
            restorable,
        })
    }

    fn stored_response(url: &Url, entry: &Entry, bytes: Bytes) -> Result<Fetched, Failure> {
        let status = StatusCode::from_u16(entry.response.status).ok();
        if let Some(failure) = status.and_then(status_failure) {
            return Err(failure);
        }
        let media_type = entry
            .media_type
            .as_deref()
            .and_then(MediaType::parse)
            .or_else(|| media_type(&entry.response.headers, url));
        Ok(Fetched {
            bytes,
            media_type,
            url: url.clone(),
            restorable: RESTORES,
        })
    }
}

/// The type a response labels its body with, or else the URL's extension.
fn media_type(headers: &HeaderMap, url: &Url) -> Option<MediaType> {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(MediaType::parse)
        .or_else(|| crate::mime::from_extension(url.path()))
}

fn status_code(status: u16) -> Result<StatusCode, Failure> {
    StatusCode::from_u16(status).map_err(|_| {
        Failure::new(
            ResourceErrorKind::Protocol,
            ResourceErrorPhase::ReceiveHeaders,
            format!("impossible status {status}"),
        )
    })
}

fn unsupported_scheme(scheme: &str) -> Failure {
    Failure::new(
        ResourceErrorKind::UnsupportedScheme,
        ResourceErrorPhase::Open,
        format!("no transport serves `{scheme}:` URLs"),
    )
}

fn not_cached() -> Failure {
    Failure::new(
        ResourceErrorKind::Unavailable,
        ResourceErrorPhase::Open,
        "only-if-cached: the resource is not in the cache",
    )
}

/// The wall clock the byte tier's freshness and the code cache's last-use
/// stamps are measured on. `std`'s has no source on
/// `wasm32-unknown-unknown`, so the browser's `Date` stands in.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn now() -> SystemTime {
    SystemTime::now()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs_f64(js_sys::Date::now().max(0.0) / 1000.0)
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
        .with_retry(retry),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transports() -> Transports<crate::DefaultPlatform> {
        Transports::new(
            HttpSettings {
                timeout: Duration::from_secs(5),
                max_body: 1024,
                user_agent: "test".to_owned(),
                max_redirects: 2,
            },
            Handle::default(),
            None,
        )
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
        // With a base, the join is the whole algorithm, as the engine's is: a
        // reference that repeats the base's own special scheme is relative.
        assert_eq!(
            transports
                .resolve("https:img/a.png", Some(&base))
                .unwrap()
                .as_str(),
            "https://cards.test/app/img/a.png"
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
        assert_eq!(fetched.media_type.unwrap().essence(), "text/javascript");

        let data = Url::parse("data:text/css,a%7B%7D").expect("a URL");
        let fetched = transports.local(&data).expect("data").expect("bytes");
        assert_eq!(&fetched.bytes[..], b"a{}");
        assert_eq!(
            fetched.media_type.unwrap().essence(),
            "text/css",
            "a `data:` URL labels its own bytes"
        );

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
        assert!(not_found.message.contains("404"));
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

/// The dispatch over a scripted [`Fetch`]: the byte tier in front of it, the
/// `file:` path, and the answers a transport that cannot block gets.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod dispatch_tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use http::HeaderValue;
    use http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};

    use super::*;
    use crate::decode::NativeDecoder;
    use crate::store::FsStore;

    /// Answers every request with the next scripted response, and counts
    /// what it was asked.
    #[derive(Default)]
    struct Scripted {
        answers: Mutex<Vec<Response>>,
        requests: Mutex<Vec<Request>>,
        blocks: bool,
    }

    impl Fetch for Scripted {
        fn fetch_blocking(&self, request: &Request) -> Option<Result<Response, Failure>> {
            if !self.blocks {
                return None;
            }
            self.requests.lock().unwrap().push(request.clone());
            Some(Ok(self.answers.lock().unwrap().remove(0)))
        }

        fn owns_http_cache(&self) -> bool {
            false
        }
    }

    struct Test;

    impl Platform for Test {
        type Fetch = Scripted;
        type Store = FsStore;
        type Decoder = NativeDecoder;

        fn into_parts(self) -> vant::Parts<Scripted, FsStore, NativeDecoder> {
            unreachable!("the tests build their transports from parts")
        }
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bobcat-resources-dispatch-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn settings() -> HttpSettings {
        HttpSettings {
            timeout: Duration::from_secs(5),
            max_body: 1024,
            user_agent: "test".to_owned(),
            max_redirects: 2,
        }
    }

    fn transports(fetch: Scripted, dir: Option<&TempDir>) -> Transports<Test> {
        let bytes = dir.map(|dir| {
            Handle::new(ByteCache::open(
                FsStore::open(&dir.0).expect("open"),
                u64::MAX,
            ))
        });
        Transports::new(settings(), Handle::new(fetch), bytes)
    }

    fn answer(
        url: &Url,
        status: u16,
        headers: &[(http::HeaderName, &str)],
        body: &[u8],
    ) -> Response {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(name.clone(), HeaderValue::from_str(value).unwrap());
        }
        Response {
            url: url.clone(),
            status,
            headers: map,
            body: Bytes::copy_from_slice(body),
        }
    }

    fn fetch(
        transports: &Transports<Test>,
        url: &Url,
        policy: CachePolicy,
    ) -> Result<Fetched, Failure> {
        transports.fetch_blocking(url, policy, &HeaderMap::new(), Body::Read)
    }

    #[test]
    fn a_stored_response_is_served_without_a_request_and_revalidated_with_a_304() {
        let dir = TempDir::new();
        let url = Url::parse("https://cards.test/a.png").unwrap();
        let scripted = Scripted {
            blocks: true,
            ..Scripted::default()
        };
        scripted.answers.lock().unwrap().extend([
            answer(
                &url,
                200,
                &[
                    (CONTENT_TYPE, "image/png"),
                    (ETAG, "\"1\""),
                    (CACHE_CONTROL, "max-age=3600"),
                ],
                b"pixels",
            ),
            answer(&url, 304, &[(CACHE_CONTROL, "max-age=3600")], b""),
        ]);
        let transports = transports(scripted, Some(&dir));

        let first = fetch(&transports, &url, CachePolicy::Default).expect("fetched");
        assert_eq!(&first.bytes[..], b"pixels");
        assert!(first.restorable, "the byte tier holds it");
        assert_eq!(first.media_type.unwrap().essence(), "image/png");

        let again = transports
            .fetch_blocking(&url, CachePolicy::Default, &HeaderMap::new(), Body::Mapped)
            .expect("stored");
        assert_eq!(&again.bytes[..], b"pixels");
        assert_eq!(again.media_type.unwrap().essence(), "image/png");
        assert_eq!(transports.fetch.requests.lock().unwrap().len(), 1);

        let revalidated = fetch(&transports, &url, CachePolicy::NoCache).expect("revalidated");
        assert_eq!(
            &revalidated.bytes[..],
            b"pixels",
            "a 304 serves the stored body"
        );
        let requests = transports.fetch.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].headers.get(IF_NONE_MATCH).unwrap(), "\"1\"");
        assert_eq!(requests[1].user_agent, "test");
        assert_eq!(requests[1].max_body, 1024);
    }

    #[test]
    fn only_if_cached_with_nothing_stored_fails_without_a_request() {
        let url = Url::parse("https://cards.test/a.png").unwrap();
        for dir in [None, Some(TempDir::new())] {
            let transports = transports(
                Scripted {
                    blocks: true,
                    ..Scripted::default()
                },
                dir.as_ref(),
            );
            let failure = fetch(&transports, &url, CachePolicy::OnlyIfCached).expect_err("refused");
            assert_eq!(failure.kind, ResourceErrorKind::Unavailable);
            assert!(transports.fetch.requests.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn a_failure_status_is_stored_when_storable_and_still_fails() {
        let dir = TempDir::new();
        let url = Url::parse("https://cards.test/missing.png").unwrap();
        let scripted = Scripted {
            blocks: true,
            ..Scripted::default()
        };
        scripted.answers.lock().unwrap().push(answer(
            &url,
            404,
            &[(CACHE_CONTROL, "max-age=3600")],
            b"not found",
        ));
        let transports = transports(scripted, Some(&dir));
        for _ in 0..2 {
            let failure = fetch(&transports, &url, CachePolicy::Default).expect_err("404");
            assert_eq!(failure.kind, ResourceErrorKind::NotFound);
        }
        assert_eq!(
            transports.fetch.requests.lock().unwrap().len(),
            1,
            "the second answer is the stored 404"
        );
    }

    #[test]
    fn a_transport_that_cannot_block_fails_a_blocking_request() {
        let transports = transports(Scripted::default(), None);
        let url = Url::parse("https://cards.test/a.png").unwrap();
        let failure = fetch(&transports, &url, CachePolicy::Default).expect_err("cannot block");
        assert_eq!(failure.kind, ResourceErrorKind::UnsupportedOperation);
    }

    #[test]
    fn a_file_is_labelled_by_its_extension_and_restorable() {
        let dir = TempDir::new();
        std::fs::create_dir_all(&dir.0).expect("dir");
        let path = dir.0.join("style.css");
        std::fs::write(&path, b"a{}").expect("write");
        let url = Url::from_file_path(&path).unwrap();
        let native = Transports::<crate::DefaultPlatform>::new(settings(), Handle::default(), None);
        for body in [Body::Read, Body::Mapped] {
            let fetched = native
                .fetch_blocking(&url, CachePolicy::Default, &HeaderMap::new(), body)
                .expect("read");
            assert_eq!(&fetched.bytes[..], b"a{}");
            assert_eq!(fetched.media_type.unwrap().essence(), "text/css");
            assert!(fetched.restorable);
        }
    }
}
