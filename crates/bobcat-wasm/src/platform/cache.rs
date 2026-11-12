//! [`CacheStore`]: the code cache's [`AsyncStore`] over one cache of the
//! web Cache API (`caches.open(name)`).
//!
//! The Cache API is available in a Window and in every kind of Worker, is
//! shared by every tab of the origin, writes each entry atomically, and is
//! listed in the browser's developer tools. Every call answers with a
//! Promise, which is why the code cache drives it through `vant`'s
//! [`AsyncArtifactCache`](vant::artifact::AsyncArtifactCache). All of the
//! tier's policy is `vant`'s; this file only moves bytes.
//!
//! # Entries
//!
//! A store key is the request URL `https://bobcat.invalid/<key>`, each `/`
//! separated segment of the key percent-encoded. The scheme is `https`
//! because `Cache.put` refuses any scheme other than `http` and `https`; the
//! host is in the `.invalid` top-level domain, so nothing is ever fetched
//! from it. An entry's response carries the entry's bytes as its body, and
//! its [`Meta`] as headers: the full key, the body's length, `last_used` in
//! whole Unix seconds, and the record as percent-encoded JSON (a header
//! value is a byte string).
//!
//! Recency is the `last_used` header alone. Nothing depends on the order in
//! which `keys()` or `matchAll()` list entries, or on whether a second `put`
//! of a key moves it in that order.
//!
//! Every body crosses into and out of Wasm memory through a fresh,
//! non-shared `Uint8Array`, as the decoder's job bytes and `fetch`'s bodies
//! do: on threaded Wasm the linear memory is a `SharedArrayBuffer`, which a
//! `Response` body cannot be built over, and a view over it would be
//! detached by a memory growth during the call.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use js_sys::{Array, ArrayBuffer, Reflect, Uint8Array};
use vant::{AsyncStore, Estimate, Meta, StoreError, Volatility};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Cache, CacheStorage, Headers, Response, ResponseInit};

/// The origin every request URL is under: never fetched from, since the
/// `.invalid` top-level domain resolves nowhere.
const ORIGIN: &str = "https://bobcat.invalid/";

const KEY_HEADER: &str = "x-bobcat-key";
const LENGTH_HEADER: &str = "x-bobcat-length";
const LAST_USED_HEADER: &str = "x-bobcat-last-used";
const RECORD_HEADER: &str = "x-bobcat-record";

/// One cache of the origin's `CacheStorage`, opened once.
#[derive(Debug)]
pub struct CacheStore {
    cache: Cache,
}

impl CacheStore {
    /// Opens (creating if absent) the cache `name` of this scope's
    /// `caches`. Works in a Window and in a Worker: `caches` is read from
    /// the global scope.
    ///
    /// # Errors
    ///
    /// When the scope has no `caches` (a context that is not secure) or
    /// `caches.open` rejects (some browsers' private browsing).
    pub async fn open(name: &str) -> Result<Self, String> {
        let caches =
            Reflect::get(&js_sys::global(), &"caches".into()).map_err(|error| describe(&error))?;
        if caches.is_undefined() || caches.is_null() {
            return Err("this scope has no Cache API (it is not a secure context)".to_owned());
        }
        let caches = caches.unchecked_into::<CacheStorage>();
        let cache = JsFuture::from(caches.open(name))
            .await
            .map_err(|error| describe(&error))?
            .dyn_into::<Cache>()
            .map_err(|_| "`caches.open` answered something other than a cache".to_owned())?;
        Ok(Self { cache })
    }
}

impl AsyncStore for CacheStore {
    /// Every entry's [`Meta`], read from its headers by `matchAll()`; no body
    /// is read. A response from `matchAll()` does not say which request it
    /// is filed under, so when one's headers do not parse, the entries are
    /// listed again by `keys()` and every entry whose headers do not parse is
    /// deleted by its request URL.
    async fn scan(&self) -> Result<Vec<Meta>, StoreError> {
        let responses = list(self.cache.match_all()).await?;
        let mut metas = Vec::with_capacity(responses.length() as usize);
        let mut broken = false;
        for response in responses.iter() {
            match response
                .dyn_into::<Response>()
                .ok()
                .and_then(|response| meta_of(&response.headers()))
            {
                Some(meta) => metas.push(meta),
                None => broken = true,
            }
        }
        if broken {
            self.delete_broken().await?;
        }
        Ok(metas)
    }

    async fn get(&self, key: &str) -> Result<Option<(Meta, Bytes)>, StoreError> {
        let Some(response) = self.matched(key).await? else {
            return Ok(None);
        };
        let Some(meta) = meta_of(&response.headers()) else {
            return Err(StoreError::Corrupt(format!(
                "the headers of `{key}` do not parse"
            )));
        };
        let body = JsFuture::from(response.array_buffer().map_err(store_error)?)
            .await
            .map_err(store_error)?
            .dyn_into::<ArrayBuffer>()
            .map_err(|_| {
                StoreError::Other("`arrayBuffer` answered something other than bytes".to_owned())
            })?;
        Ok(Some((meta, Bytes::from(Uint8Array::new(&body).to_vec()))))
    }

    async fn put(&self, meta: &Meta, body: Bytes) -> Result<(), StoreError> {
        let copy = Uint8Array::from(&body[..]);
        let response = Response::new_with_opt_buffer_source_and_init(Some(&*copy), &init(meta)?)
            .map_err(store_error)?;
        JsFuture::from(self.cache.put_with_str(&request_url(&meta.key), &response))
            .await
            .map_err(store_error)?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        JsFuture::from(self.cache.delete_with_str(&request_url(key)))
            .await
            .map_err(store_error)?;
        Ok(())
    }

    /// Puts the entry again with a new `last_used` header. The stored body
    /// is handed to the new response as its stream, so it is never copied
    /// into Wasm memory. A key with no entry is left absent.
    async fn touch(&self, key: &str, now: SystemTime) -> Result<(), StoreError> {
        let Some(response) = self.matched(key).await? else {
            return Ok(());
        };
        let Some(mut meta) = meta_of(&response.headers()) else {
            return Err(StoreError::Corrupt(format!(
                "the headers of `{key}` do not parse"
            )));
        };
        meta.last_used = now;
        let touched = Response::new_with_opt_readable_stream_and_init(
            response.body().as_ref(),
            &init(&meta)?,
        )
        .map_err(store_error)?;
        JsFuture::from(self.cache.put_with_str(&request_url(key), &touched))
            .await
            .map_err(store_error)?;
        Ok(())
    }

    /// `navigator.storage.estimate()` for the whole origin, when the scope
    /// has it; otherwise nothing is known.
    async fn estimate(&self) -> Estimate {
        let Some(estimate) = storage_estimate().await else {
            return Estimate::default();
        };
        let number = |name: &str| {
            Reflect::get(&estimate, &name.into())
                .ok()
                .and_then(|value| value.as_f64())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|value| {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "a finite, non-negative byte count"
                    )]
                    let value = value as u64;
                    value
                })
        };
        Estimate {
            used: number("usage").unwrap_or(0),
            quota: number("quota"),
        }
    }

    /// The browser may clear the origin's storage at any time.
    fn volatility(&self) -> Volatility {
        Volatility::EvictableAnytime
    }
}

impl CacheStore {
    /// Deletes every entry whose headers do not parse, found through
    /// `keys()`, which names each entry's request.
    async fn delete_broken(&self) -> Result<(), StoreError> {
        for request in list(self.cache.keys()).await?.iter() {
            let Ok(request) = request.dyn_into::<web_sys::Request>() else {
                continue;
            };
            let url = request.url();
            let answer = JsFuture::from(self.cache.match_with_str(&url))
                .await
                .map_err(store_error)?;
            let parses = answer
                .dyn_into::<Response>()
                .ok()
                .and_then(|response| meta_of(&response.headers()))
                .is_some();
            if !parses {
                let _ = JsFuture::from(self.cache.delete_with_str(&url)).await;
            }
        }
        Ok(())
    }

    /// The response stored under `key`, if there is one.
    async fn matched(&self, key: &str) -> Result<Option<Response>, StoreError> {
        let answer = JsFuture::from(self.cache.match_with_str(&request_url(key)))
            .await
            .map_err(store_error)?;
        Ok(answer.dyn_into::<Response>().ok())
    }
}

/// The list a `matchAll()` or `keys()` Promise answers.
async fn list(promise: js_sys::Promise) -> Result<Array, StoreError> {
    JsFuture::from(promise)
        .await
        .map_err(store_error)?
        .dyn_into::<Array>()
        .map_err(|_| StoreError::Other("the cache answered something other than a list".to_owned()))
}

/// `navigator.storage.estimate()`'s answer, or `None` when the scope has no
/// `navigator.storage` or the call fails.
async fn storage_estimate() -> Option<JsValue> {
    let navigator = Reflect::get(&js_sys::global(), &"navigator".into()).ok()?;
    if navigator.is_undefined() || navigator.is_null() {
        return None;
    }
    let storage = Reflect::get(&navigator, &"storage".into()).ok()?;
    let has_estimate = !storage.is_undefined()
        && !storage.is_null()
        && Reflect::get(&storage, &"estimate".into()).is_ok_and(|value| value.is_function());
    if !has_estimate {
        return None;
    }
    let storage = storage.unchecked_into::<web_sys::StorageManager>();
    JsFuture::from(storage.estimate().ok()?).await.ok()
}

/// The request URL a store key is filed under.
fn request_url(key: &str) -> String {
    let path: Vec<String> = key.split('/').map(percent_encode).collect();
    format!("{ORIGIN}{}", path.join("/"))
}

/// The response options carrying `meta` as headers.
fn init(meta: &Meta) -> Result<ResponseInit, StoreError> {
    let headers = Headers::new().map_err(store_error)?;
    let last_used = meta
        .last_used
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    for (name, value) in [
        (KEY_HEADER, percent_encode(&meta.key)),
        (LENGTH_HEADER, meta.len.to_string()),
        (LAST_USED_HEADER, last_used.to_string()),
        (RECORD_HEADER, percent_encode(&meta.record.to_string())),
    ] {
        headers.set(name, &value).map_err(store_error)?;
    }
    let init = ResponseInit::new();
    init.set_headers(headers.as_ref());
    Ok(init)
}

/// The [`Meta`] an entry's headers carry, or `None` when one is missing or
/// does not parse.
fn meta_of(headers: &Headers) -> Option<Meta> {
    let header = |name: &str| headers.get(name).ok().flatten();
    let key = percent_decode(&header(KEY_HEADER)?)?;
    let len = header(LENGTH_HEADER)?.parse().ok()?;
    let last_used = UNIX_EPOCH + Duration::from_secs(header(LAST_USED_HEADER)?.parse().ok()?);
    let record = serde_json::from_str(&percent_decode(&header(RECORD_HEADER)?)?).ok()?;
    Some(Meta {
        key,
        len,
        last_used,
        record,
    })
}

/// `text` with every byte other than an ASCII letter, digit, `-`, `.`, `_`
/// or `~` written as `%XX`.
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0xF)]));
        }
    }
    encoded
}

/// The text [`percent_encode`] wrote, or `None` for a malformed escape or a
/// result that is not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "it is the error a `map_err` hands over"
)]
fn store_error(error: JsValue) -> StoreError {
    StoreError::Other(describe(&error))
}

fn describe(error: &JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"))
}
