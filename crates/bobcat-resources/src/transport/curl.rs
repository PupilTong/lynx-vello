//! HTTP(S) through the platform's own libcurl, loaded at runtime.
//!
//! macOS ships `libcurl.4.dylib` and every desktop Linux ships
//! `libcurl.so.4`, and both carry the platform's TLS stack, proxy settings
//! and certificate trust. Using them through `dlopen` gives this crate a
//! real HTTP client with no HTTP or TLS code of its own and no build-time
//! link — the `linux-cli` CI job, which installs no curl headers, still
//! compiles it — and a host without the library gets a precise
//! [`TransportError::Unavailable`] instead of a load failure at startup.
//!
//! Only the "easy" interface is used, one blocking transfer per call, on
//! whatever worker thread the fetcher runs IO on.

#![expect(
    unsafe_code,
    reason = "a C library loaded at runtime is reached only through FFI; every call site \
              states the invariant it relies on"
)]

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use http::StatusCode;
use http::header::{HeaderMap, HeaderName, HeaderValue, LOCATION};
use libloading::Library;

/// The platform's libcurl, loaded once per process and shared.
#[derive(Clone)]
pub struct Curl {
    inner: Arc<Inner>,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("libcurl is not available: {0}")]
    Unavailable(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("could not resolve the host")]
    Dns,
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("TLS failure: {0}")]
    Tls(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("the response exceeded {limit} bytes")]
    TooLarge { limit: usize },
    #[error("transfer timed out")]
    Timeout,
    #[error("curl error {code}: {message}")]
    Curl { code: i32, message: String },
}

/// One GET.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// The whole transfer, connect included.
    pub timeout: Duration,
    /// Past this many body bytes the transfer is aborted.
    pub max_body: usize,
    pub max_redirects: u32,
    pub user_agent: String,
}

impl HttpRequest {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            timeout: Duration::from_secs(30),
            max_body: 64 * 1024 * 1024,
            max_redirects: 10,
            user_agent: concat!("bobcat-resources/", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }
}

/// The final response of a transfer, after any redirects.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: StatusCode,
    /// The final response's headers only; a redirect's block is discarded
    /// when the next status line arrives.
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    /// The URL the final response came from.
    pub effective_url: String,
    /// The `Location` of every redirect followed, in order.
    pub redirects: Vec<String>,
    pub timing: HttpTiming,
}

/// Transfer phases as libcurl measured them.
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpTiming {
    pub name_lookup: Option<Duration>,
    pub connect: Option<Duration>,
    pub start_transfer: Option<Duration>,
    pub total: Option<Duration>,
}

type Handle = *mut c_void;
type EasyInit = unsafe extern "C" fn() -> Handle;
type EasyCleanup = unsafe extern "C" fn(Handle);
type EasySetopt = unsafe extern "C" fn(Handle, c_int, ...) -> c_int;
type EasyPerform = unsafe extern "C" fn(Handle) -> c_int;
type EasyGetinfo = unsafe extern "C" fn(Handle, c_int, ...) -> c_int;
type EasyStrerror = unsafe extern "C" fn(c_int) -> *const c_char;
type GlobalInit = unsafe extern "C" fn(c_long) -> c_int;
type VersionFn = unsafe extern "C" fn() -> *const c_char;
type SlistAppend = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;
type SlistFreeAll = unsafe extern "C" fn(*mut c_void);
type DataCallback = unsafe extern "C" fn(*mut c_char, usize, usize, *mut c_void) -> usize;

struct Inner {
    // Declared before the library so it drops first: the symbols are only
    // valid while the library is mapped.
    symbols: Symbols,
    _library: Library,
}

struct Symbols {
    easy_init: EasyInit,
    easy_cleanup: EasyCleanup,
    easy_setopt: EasySetopt,
    easy_perform: EasyPerform,
    easy_getinfo: EasyGetinfo,
    easy_strerror: EasyStrerror,
    version: VersionFn,
    slist_append: SlistAppend,
    slist_free_all: SlistFreeAll,
}

const CURL_GLOBAL_DEFAULT: c_long = 3;
const CURLOPT_WRITEDATA: c_int = 10001;
const CURLOPT_URL: c_int = 10002;
const CURLOPT_ERRORBUFFER: c_int = 10010;
const CURLOPT_USERAGENT: c_int = 10018;
const CURLOPT_HTTPHEADER: c_int = 10023;
const CURLOPT_HEADERDATA: c_int = 10029;
const CURLOPT_WRITEFUNCTION: c_int = 20011;
const CURLOPT_HEADERFUNCTION: c_int = 20079;
const CURLOPT_FOLLOWLOCATION: c_int = 52;
const CURLOPT_MAXREDIRS: c_int = 68;
const CURLOPT_NOSIGNAL: c_int = 99;
const CURLOPT_ACCEPT_ENCODING: c_int = 10102;
const CURLOPT_TIMEOUT_MS: c_int = 155;
const CURLOPT_CONNECTTIMEOUT_MS: c_int = 156;
const CURLOPT_NOPROXY: c_int = 10177;
const CURLOPT_PROTOCOLS: c_int = 181;
const CURLOPT_REDIR_PROTOCOLS: c_int = 182;
const CURLOPT_PROTOCOLS_STR: c_int = 10318;
const CURLOPT_REDIR_PROTOCOLS_STR: c_int = 10319;
const CURLPROTO_HTTP_HTTPS: c_long = 1 | 2;
const CURLINFO_EFFECTIVE_URL: c_int = 0x0010_0001;
const CURLINFO_RESPONSE_CODE: c_int = 0x0020_0002;
const CURLINFO_OFF_T: c_int = 0x0060_0000;
const CURLINFO_TOTAL_TIME_T: c_int = CURLINFO_OFF_T + 50;
const CURLINFO_NAMELOOKUP_TIME_T: c_int = CURLINFO_OFF_T + 51;
const CURLINFO_CONNECT_TIME_T: c_int = CURLINFO_OFF_T + 52;
const CURLINFO_STARTTRANSFER_TIME_T: c_int = CURLINFO_OFF_T + 54;
const CURL_ERROR_SIZE: usize = 256;

const CURLE_OK: c_int = 0;
const CURLE_UNSUPPORTED_PROTOCOL: c_int = 1;
const CURLE_URL_MALFORMAT: c_int = 3;
const CURLE_COULDNT_RESOLVE_HOST: c_int = 6;
const CURLE_COULDNT_CONNECT: c_int = 7;
const CURLE_WRITE_ERROR: c_int = 23;
const CURLE_OPERATION_TIMEDOUT: c_int = 28;
const CURLE_TOO_MANY_REDIRECTS: c_int = 47;

#[cfg(target_os = "macos")]
const LIBRARY_CANDIDATES: &[&str] = &[
    "/usr/lib/libcurl.4.dylib",
    "libcurl.4.dylib",
    "libcurl.dylib",
];
#[cfg(all(unix, not(target_os = "macos")))]
const LIBRARY_CANDIDATES: &[&str] = &[
    "libcurl.so.4",
    "libcurl.so",
    "libcurl-gnutls.so.4",
    "libcurl-nss.so.4",
];
#[cfg(not(unix))]
const LIBRARY_CANDIDATES: &[&str] = &[];

static LOADED: OnceLock<Result<Curl, String>> = OnceLock::new();

impl std::fmt::Debug for Curl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Curl")
            .field("version", &self.version())
            .finish()
    }
}

impl Curl {
    /// The platform libcurl, loaded and globally initialised on first use.
    /// A host without one gets [`TransportError::Unavailable`] every time,
    /// never a panic.
    pub fn load() -> Result<Self, TransportError> {
        LOADED
            .get_or_init(|| Self::load_uncached().map_err(|error| error.to_string()))
            .clone()
            .map_err(TransportError::Unavailable)
    }

    fn load_uncached() -> Result<Self, TransportError> {
        let mut failures = Vec::new();
        for candidate in LIBRARY_CANDIDATES {
            // SAFETY: libcurl's initialisation routines run in its constructor
            // and are safe to run in any process; nothing here executes
            // untrusted code beyond loading the system library by its
            // well-known name.
            match unsafe { Library::new(candidate) } {
                Ok(library) => return Self::from_library(library),
                Err(error) => failures.push(format!("{candidate}: {error}")),
            }
        }
        Err(TransportError::Unavailable(if failures.is_empty() {
            "no libcurl candidate exists for this platform".to_owned()
        } else {
            failures.join("; ")
        }))
    }

    fn from_library(library: Library) -> Result<Self, TransportError> {
        macro_rules! symbol {
            ($name:literal) => {{
                // SAFETY: the symbol is declared with the signature libcurl's
                // public header gives it, and it is copied out of the
                // `Symbol` into a struct that keeps the library mapped for as
                // long as the pointer can be called.
                let symbol = unsafe { library.get::<$crate::transport::curl::Raw<_>>($name) }
                    .map_err(|error| {
                        TransportError::Unavailable(format!(
                            "{} is missing: {error}",
                            String::from_utf8_lossy(&$name[..$name.len() - 1])
                        ))
                    })?;
                *symbol
            }};
        }
        let symbols = Symbols {
            easy_init: symbol!(b"curl_easy_init\0"),
            easy_cleanup: symbol!(b"curl_easy_cleanup\0"),
            easy_setopt: symbol!(b"curl_easy_setopt\0"),
            easy_perform: symbol!(b"curl_easy_perform\0"),
            easy_getinfo: symbol!(b"curl_easy_getinfo\0"),
            easy_strerror: symbol!(b"curl_easy_strerror\0"),
            version: symbol!(b"curl_version\0"),
            slist_append: symbol!(b"curl_slist_append\0"),
            slist_free_all: symbol!(b"curl_slist_free_all\0"),
        };
        let global_init: GlobalInit = symbol!(b"curl_global_init\0");
        // SAFETY: called exactly once per process, from inside the `OnceLock`
        // initialiser, before any easy handle exists — the one ordering
        // libcurl documents as required.
        let code = unsafe { global_init(CURL_GLOBAL_DEFAULT) };
        if code != CURLE_OK {
            return Err(TransportError::Unavailable(format!(
                "curl_global_init failed with code {code}"
            )));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                symbols,
                _library: library,
            }),
        })
    }

    /// `curl_version()`.
    #[must_use]
    pub fn version(&self) -> String {
        // SAFETY: `curl_version` returns a pointer to a static NUL-terminated
        // string owned by libcurl.
        unsafe { CStr::from_ptr((self.inner.symbols.version)()) }
            .to_string_lossy()
            .into_owned()
    }

    /// Performs one blocking GET, following redirects, returning whatever
    /// final response the server gave — a `404` is a response here, not an
    /// error. Protocols are restricted to HTTP and HTTPS, on the first hop
    /// and on every redirect.
    pub fn get(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let symbols = &self.inner.symbols;
        // SAFETY: plain constructor call; a null return is checked below.
        let handle = unsafe { (symbols.easy_init)() };
        if handle.is_null() {
            return Err(TransportError::Curl {
                code: -1,
                message: "curl_easy_init returned null".to_owned(),
            });
        }
        let guard = HandleGuard {
            handle,
            symbols,
            header_list: std::ptr::null_mut(),
        };
        let mut transfer = Transfer {
            collector: HeaderCollector::default(),
            body: Vec::new(),
            max_body: request.max_body,
            overflowed: false,
        };
        let mut error_buffer = [0_u8; CURL_ERROR_SIZE];
        let mut guard = guard;
        let strings = Self::configure(&mut guard, request, &mut transfer, &mut error_buffer)?;

        // SAFETY: the handle is fully configured and every pointer it holds
        // — the strings, the error buffer, the transfer state — is live for
        // the duration of this call.
        let code = unsafe { (symbols.easy_perform)(handle) };
        drop(strings);
        if code != CURLE_OK {
            let message = CStr::from_bytes_until_nul(&error_buffer)
                .ok()
                .map(|message| message.to_string_lossy().into_owned())
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| self.strerror(code));
            return Err(map_error(
                code,
                message,
                transfer.overflowed,
                request.max_body,
            ));
        }

        let mut response_code: c_long = 0;
        let mut effective_url: *const c_char = std::ptr::null();
        // SAFETY: `getinfo` writes exactly one value of the documented type
        // through each out-pointer; the effective URL points into the
        // handle and is copied before the guard frees it.
        unsafe {
            let getinfo = symbols.easy_getinfo;
            getinfo(handle, CURLINFO_RESPONSE_CODE, &raw mut response_code);
            getinfo(handle, CURLINFO_EFFECTIVE_URL, &raw mut effective_url);
        }
        let effective_url = if effective_url.is_null() {
            request.url.clone()
        } else {
            // SAFETY: a non-null effective URL is a NUL-terminated string
            // owned by the still-live handle.
            unsafe { CStr::from_ptr(effective_url) }
                .to_string_lossy()
                .into_owned()
        };
        let status = u16::try_from(response_code)
            .ok()
            .and_then(|code| StatusCode::from_u16(code).ok())
            .ok_or_else(|| TransportError::Curl {
                code: -1,
                message: format!("libcurl reported an impossible status {response_code}"),
            })?;
        let timing = HttpTiming {
            name_lookup: self.timing(handle, CURLINFO_NAMELOOKUP_TIME_T),
            connect: self.timing(handle, CURLINFO_CONNECT_TIME_T),
            start_transfer: self.timing(handle, CURLINFO_STARTTRANSFER_TIME_T),
            total: self.timing(handle, CURLINFO_TOTAL_TIME_T),
        };
        drop(guard);
        let Transfer {
            collector, body, ..
        } = transfer;
        let (headers, redirects) = collector.finish();
        Ok(HttpResponse {
            status,
            headers,
            body,
            effective_url,
            redirects,
            timing,
        })
    }

    /// Sets every option a transfer needs, returning the C strings the
    /// handle now points at — they must outlive `easy_perform`.
    fn configure(
        guard: &mut HandleGuard<'_>,
        request: &HttpRequest,
        transfer: &mut Transfer,
        error_buffer: &mut [u8; CURL_ERROR_SIZE],
    ) -> Result<Vec<CString>, TransportError> {
        let symbols = guard.symbols;
        let handle = guard.handle;
        let url = c_string(&request.url)?;
        let user_agent = c_string(&request.user_agent)?;
        let accept_encoding = c_string("")?;
        let no_proxy = c_string("localhost,127.0.0.1,::1")?;
        let protocols = c_string("http,https")?;
        for (name, value) in &request.headers {
            let line = c_string(&format!("{name}: {value}"))?;
            // SAFETY: `slist_append` copies the string; the list head is
            // freed by the guard, and a null head is what an empty list is.
            guard.header_list = unsafe { (symbols.slist_append)(guard.header_list, line.as_ptr()) };
        }

        // SAFETY: for every `setopt` below, `handle` is a live easy handle owned
        // by `guard`, each option is passed exactly the C type libcurl
        // documents for it, and every pointer handed over — the CStrings,
        // the error buffer, the transfer state — outlives `easy_perform`,
        // after which libcurl no longer touches them.
        unsafe {
            let setopt = symbols.easy_setopt;
            setopt(handle, CURLOPT_URL, url.as_ptr());
            setopt(handle, CURLOPT_USERAGENT, user_agent.as_ptr());
            setopt(handle, CURLOPT_ACCEPT_ENCODING, accept_encoding.as_ptr());
            setopt(handle, CURLOPT_NOPROXY, no_proxy.as_ptr());
            setopt(handle, CURLOPT_FOLLOWLOCATION, 1 as c_long);
            setopt(
                handle,
                CURLOPT_MAXREDIRS,
                c_long::from(request.max_redirects),
            );
            setopt(handle, CURLOPT_NOSIGNAL, 1 as c_long);
            setopt(handle, CURLOPT_TIMEOUT_MS, millis(request.timeout));
            setopt(
                handle,
                CURLOPT_CONNECTTIMEOUT_MS,
                millis(request.timeout.min(Duration::from_secs(30))),
            );
            // The string form is the current API; a libcurl older than 7.85
            // answers it with an unknown-option code, and the numeric form
            // it does know says the same thing.
            if setopt(handle, CURLOPT_PROTOCOLS_STR, protocols.as_ptr()) != CURLE_OK {
                setopt(handle, CURLOPT_PROTOCOLS, CURLPROTO_HTTP_HTTPS);
            }
            if setopt(handle, CURLOPT_REDIR_PROTOCOLS_STR, protocols.as_ptr()) != CURLE_OK {
                setopt(handle, CURLOPT_REDIR_PROTOCOLS, CURLPROTO_HTTP_HTTPS);
            }
            setopt(handle, CURLOPT_ERRORBUFFER, error_buffer.as_mut_ptr());
            setopt(
                handle,
                CURLOPT_WRITEFUNCTION,
                write_callback as DataCallback,
            );
            setopt(
                handle,
                CURLOPT_WRITEDATA,
                std::ptr::from_mut(transfer).cast::<c_void>(),
            );
            setopt(
                handle,
                CURLOPT_HEADERFUNCTION,
                header_callback as DataCallback,
            );
            setopt(
                handle,
                CURLOPT_HEADERDATA,
                std::ptr::from_mut(transfer).cast::<c_void>(),
            );
            if !guard.header_list.is_null() {
                setopt(handle, CURLOPT_HTTPHEADER, guard.header_list);
            }
        }
        Ok(vec![url, user_agent, accept_encoding, no_proxy, protocols])
    }

    fn timing(&self, handle: Handle, info: c_int) -> Option<Duration> {
        let mut microseconds: i64 = 0;
        // SAFETY: the `*_TIME_T` infos write one `curl_off_t` (a 64-bit
        // integer) through the out-pointer; a libcurl too old to know them
        // returns an error and writes nothing.
        let code =
            unsafe { (self.inner.symbols.easy_getinfo)(handle, info, &raw mut microseconds) };
        (code == CURLE_OK)
            .then(|| u64::try_from(microseconds).ok())
            .flatten()
            .map(Duration::from_micros)
    }

    fn strerror(&self, code: c_int) -> String {
        // SAFETY: `curl_easy_strerror` returns a static string for any code.
        unsafe { CStr::from_ptr((self.inner.symbols.easy_strerror)(code)) }
            .to_string_lossy()
            .into_owned()
    }
}

/// A raw symbol type marker: `libloading` hands back `Symbol<T>` for the `T`
/// requested, and this alias keeps the request site to one generic.
pub(crate) type Raw<T> = T;

fn c_string(value: &str) -> Result<CString, TransportError> {
    CString::new(value)
        .map_err(|_| TransportError::InvalidRequest("a value contains a NUL byte".to_owned()))
}

fn millis(duration: Duration) -> c_long {
    c_long::try_from(duration.as_millis()).unwrap_or(c_long::MAX)
}

/// Frees the easy handle and its header list however `get` exits.
struct HandleGuard<'a> {
    handle: Handle,
    symbols: &'a Symbols,
    header_list: *mut c_void,
}

impl Drop for HandleGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by `easy_init` and is cleaned up
        // exactly once, after which nothing references it; the header list
        // is either null or the head `slist_append` returned, and the handle
        // that referenced it is already gone.
        unsafe {
            (self.symbols.easy_cleanup)(self.handle);
            if !self.header_list.is_null() {
                (self.symbols.slist_free_all)(self.header_list);
            }
        }
    }
}

/// The per-transfer state both callbacks write.
struct Transfer {
    collector: HeaderCollector,
    body: Vec<u8>,
    max_body: usize,
    overflowed: bool,
}

/// Accumulates header lines across a redirect chain, keeping only the last
/// response's block and the `Location` of every block before it.
#[derive(Debug, Default)]
pub(crate) struct HeaderCollector {
    current: HeaderMap,
    redirects: Vec<String>,
    /// The `Location` of the block being collected, moved into `redirects`
    /// when a later status line proves the block was a redirect.
    location: Option<String>,
}

impl HeaderCollector {
    /// Feeds one header line as libcurl delivers it: a status line, a
    /// `name: value` line, or the blank terminator.
    pub(crate) fn line(&mut self, line: &[u8]) {
        let trimmed = line.trim_ascii_end();
        if trimmed.len() >= 5 && trimmed[..5].eq_ignore_ascii_case(b"HTTP/") {
            if let Some(location) = self.location.take() {
                self.redirects.push(location);
            }
            self.current.clear();
            return;
        }
        if trimmed.is_empty() {
            return;
        }
        let Some(colon) = trimmed.iter().position(|byte| *byte == b':') else {
            return;
        };
        let name = trimmed[..colon].trim_ascii();
        let value = trimmed[colon + 1..].trim_ascii();
        let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name), HeaderValue::from_bytes(value))
        else {
            return;
        };
        if name == LOCATION {
            self.location = Some(String::from_utf8_lossy(value.as_bytes()).into_owned());
        }
        self.current.append(name, value);
    }

    /// The final response's headers and the redirects that led to it.
    pub(crate) fn finish(self) -> (HeaderMap, Vec<String>) {
        (self.current, self.redirects)
    }
}

unsafe extern "C" fn write_callback(
    ptr: *mut c_char,
    size: usize,
    nmemb: usize,
    userdata: *mut c_void,
) -> usize {
    let total = size.saturating_mul(nmemb);
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: libcurl passes the `Transfer` registered as `WRITEDATA`,
        // which outlives the transfer, and `total` bytes readable at `ptr`.
        let (transfer, chunk) = unsafe {
            (
                &mut *userdata.cast::<Transfer>(),
                std::slice::from_raw_parts(ptr.cast::<u8>(), total),
            )
        };
        if transfer.body.len().saturating_add(chunk.len()) > transfer.max_body {
            transfer.overflowed = true;
            return 0;
        }
        transfer.body.extend_from_slice(chunk);
        total
    }))
    .unwrap_or(0)
}

unsafe extern "C" fn header_callback(
    ptr: *mut c_char,
    size: usize,
    nmemb: usize,
    userdata: *mut c_void,
) -> usize {
    let total = size.saturating_mul(nmemb);
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: as for `write_callback`, with `HEADERDATA`.
        let (transfer, line) = unsafe {
            (
                &mut *userdata.cast::<Transfer>(),
                std::slice::from_raw_parts(ptr.cast::<u8>(), total),
            )
        };
        transfer.collector.line(line);
        total
    }))
    .unwrap_or(0)
}

/// Maps a `CURLcode` to the transport's vocabulary.
pub(crate) fn map_error(
    code: c_int,
    message: String,
    overflowed: bool,
    limit: usize,
) -> TransportError {
    match code {
        CURLE_WRITE_ERROR if overflowed => TransportError::TooLarge { limit },
        CURLE_UNSUPPORTED_PROTOCOL | CURLE_URL_MALFORMAT => TransportError::InvalidRequest(message),
        CURLE_COULDNT_RESOLVE_HOST => TransportError::Dns,
        CURLE_COULDNT_CONNECT => TransportError::Connect(message),
        CURLE_OPERATION_TIMEDOUT => TransportError::Timeout,
        CURLE_TOO_MANY_REDIRECTS => TransportError::TooManyRedirects,
        35 | 51 | 53 | 54 | 58 | 59 | 60 | 77 | 80 | 82 | 83 | 90 | 91 | 98 => {
            TransportError::Tls(message)
        }
        other => TransportError::Curl {
            code: other,
            message,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};

    use super::*;

    #[test]
    fn header_lines_keep_the_last_block_and_every_redirect() {
        let mut collector = HeaderCollector::default();
        for line in [
            &b"HTTP/1.1 302 Found\r\n"[..],
            b"Location: /second\r\n",
            b"Set-Cookie: a=1\r\n",
            b"\r\n",
            b"HTTP/1.1 301 Moved\r\n",
            b"location: http://final.test/x\r\n",
            b"\r\n",
            b"HTTP/1.1 200 OK\r\n",
            b"Content-Type: image/png\r\n",
            b"garbage line without colon\r\n",
            b"Bad Name!: x\r\n",
            b"X-Dup: 1\r\n",
            b"X-Dup: 2\r\n",
            b"\r\n",
        ] {
            collector.line(line);
        }
        let (headers, redirects) = collector.finish();
        assert_eq!(redirects, ["/second", "http://final.test/x"]);
        assert_eq!(headers.get("content-type").unwrap(), "image/png");
        assert!(
            headers.get("set-cookie").is_none(),
            "a redirect's headers are dropped"
        );
        assert_eq!(headers.get_all("x-dup").iter().count(), 2);
        assert!(headers.get("location").is_none());
    }

    #[test]
    fn curl_codes_map_to_the_transport_vocabulary() {
        assert!(matches!(
            map_error(6, String::new(), false, 1),
            TransportError::Dns
        ));
        assert!(matches!(
            map_error(7, String::new(), false, 1),
            TransportError::Connect(_)
        ));
        assert!(matches!(
            map_error(28, String::new(), false, 1),
            TransportError::Timeout
        ));
        assert!(matches!(
            map_error(47, String::new(), false, 1),
            TransportError::TooManyRedirects
        ));
        assert!(matches!(
            map_error(60, String::new(), false, 1),
            TransportError::Tls(_)
        ));
        assert!(matches!(
            map_error(23, String::new(), true, 9),
            TransportError::TooLarge { limit: 9 }
        ));
        assert!(matches!(
            map_error(23, String::new(), false, 9),
            TransportError::Curl { code: 23, .. }
        ));
        assert!(matches!(
            map_error(3, String::new(), false, 1),
            TransportError::InvalidRequest(_)
        ));
    }

    /// A tiny HTTP/1.1 server: one request per connection, routed by path.
    fn serve() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                std::thread::spawn(move || handle(stream));
            }
        });
        format!("http://{address}")
    }

    fn handle(mut stream: TcpStream) {
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("request line");
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .to_owned();
        let mut request_headers = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header");
            if line.trim().is_empty() {
                break;
            }
            request_headers.push(line.trim().to_owned());
        }
        let response: Vec<u8> = match path.as_str() {
            "/ok" => {
                let body = b"pixels";
                let mut response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                response.extend_from_slice(body);
                response
            }
            "/redirect" => b"HTTP/1.1 302 Found\r\nLocation: /redirect2\r\nSet-Cookie: hop=1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            "/redirect2" => b"HTTP/1.1 301 Moved Permanently\r\nLocation: /ok\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            "/echo" => {
                let echoed = request_headers
                    .iter()
                    .find(|header| header.to_ascii_lowercase().starts_with("if-none-match:"))
                    .cloned()
                    .unwrap_or_default();
                let body = echoed.into_bytes();
                let mut response = format!(
                    "HTTP/1.1 304 Not Modified\r\nX-Echo-Length: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                response.extend_from_slice(&[]);
                response
            }
            "/big" => {
                let body = vec![b'x'; 200_000];
                let mut response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                response.extend_from_slice(&body);
                response
            }
            _ => b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found".to_vec(),
        };
        stream.write_all(&response).expect("write");
        stream.flush().expect("flush");
        // Let the client read everything before the socket closes.
        let mut sink = Vec::new();
        let _ = reader.read_to_end(&mut sink);
    }

    fn curl() -> Curl {
        Curl::load().expect("this platform's libcurl loads")
    }

    #[test]
    fn the_platform_libcurl_loads_and_reports_a_version() {
        let version = curl().version();
        assert!(version.starts_with("libcurl/"), "{version}");
    }

    #[test]
    fn a_plain_get_returns_status_headers_body_and_timing() {
        let base = serve();
        let response = curl()
            .get(&HttpRequest::new(format!("{base}/ok")))
            .expect("get");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, b"pixels");
        assert_eq!(response.headers.get("content-type").unwrap(), "image/png");
        assert_eq!(response.headers.get("etag").unwrap(), "\"v1\"");
        assert_eq!(response.effective_url, format!("{base}/ok"));
        assert!(response.redirects.is_empty());
        assert!(response.timing.total.is_some());
    }

    #[test]
    fn redirects_are_followed_and_recorded_and_only_the_final_headers_survive() {
        let base = serve();
        let response = curl()
            .get(&HttpRequest::new(format!("{base}/redirect")))
            .expect("get");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, b"pixels");
        assert_eq!(response.redirects, ["/redirect2", "/ok"]);
        assert_eq!(response.effective_url, format!("{base}/ok"));
        assert!(response.headers.get("set-cookie").is_none());
        assert!(response.headers.get("location").is_none());

        let mut capped = HttpRequest::new(format!("{base}/redirect"));
        capped.max_redirects = 1;
        assert!(matches!(
            curl().get(&capped),
            Err(TransportError::TooManyRedirects)
        ));
    }

    #[test]
    fn a_404_is_a_response_and_request_headers_reach_the_server() {
        let base = serve();
        let response = curl()
            .get(&HttpRequest::new(format!("{base}/missing")))
            .expect("get");
        assert_eq!(response.status, StatusCode::NOT_FOUND);
        assert_eq!(response.body, b"not found");

        let mut conditional = HttpRequest::new(format!("{base}/echo"));
        conditional
            .headers
            .push(("If-None-Match".to_owned(), "\"v1\"".to_owned()));
        let response = curl().get(&conditional).expect("get");
        assert_eq!(response.status, StatusCode::NOT_MODIFIED);
        assert_eq!(
            response.headers.get("x-echo-length").unwrap(),
            "19",
            "`if-none-match: \"v1\"` is 19 bytes, so the header arrived"
        );
    }

    #[test]
    fn a_body_past_the_limit_aborts_the_transfer() {
        let base = serve();
        let mut request = HttpRequest::new(format!("{base}/big"));
        request.max_body = 1000;
        assert!(matches!(
            curl().get(&request),
            Err(TransportError::TooLarge { limit: 1000 })
        ));
        assert!(
            curl()
                .get(&HttpRequest::new(format!("{base}/big")))
                .expect("get")
                .body
                .len()
                == 200_000
        );
    }

    #[test]
    fn unsupported_schemes_and_dead_hosts_are_precise_errors() {
        assert!(matches!(
            curl().get(&HttpRequest::new("ftp://127.0.0.1:1/x")),
            Err(TransportError::InvalidRequest(_) | TransportError::Curl { .. })
        ));
        let mut request = HttpRequest::new("http://127.0.0.1:9/closed");
        request.timeout = Duration::from_secs(5);
        assert!(matches!(
            curl().get(&request),
            Err(TransportError::Connect(_) | TransportError::Timeout)
        ));
    }
}
