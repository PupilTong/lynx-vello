//! The capabilities an embedder provides: a transport ([`Fetch`]),
//! persistent bytes ([`Store`], or [`AsyncStore`] for the artifact tier of a
//! host whose storage answers asynchronously), platform pixels
//! ([`ImageDecoder`]) and a video player ([`Media`](crate::media::Media)),
//! bundled as one [`Platform`].
//!
//! They are shaped after the web platform's Fetch and Cache APIs. [`Store`]
//! adds what this engine needs and the Cache API lacks: synchronous storage,
//! a body mapped read-only, and an LRU signal. [`AsyncStore`] is the subset
//! of [`Store`] the artifact tier uses, answered asynchronously, as the
//! Cache API answers. Every policy decision — what to cache, for how long,
//! what a request sends — stays in this crate; an implementation only moves
//! bytes (`docs/resource-cache-design.md`).
//!
//! Capabilities are traits with associated types, never `dyn`, so a host
//! that names its platform pays no dispatch for it.

use std::future::Future;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::HeaderMap;
use url::Url;

use crate::CachePolicy;
use crate::error::{Failure, ResourceErrorKind, ResourceErrorPhase};
use crate::image::animation::ImageFrames;
use crate::image::{Bitmap, DecodeError, DecodedImage};
use crate::image_header::ImageHeader;
use crate::mime::ImageFormat;

#[cfg(not(target_arch = "wasm32"))]
pub trait Threaded: Send + Sync + 'static {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + 'static> Threaded for T {}

#[cfg(target_arch = "wasm32")]
pub trait Threaded: 'static {}

#[cfg(target_arch = "wasm32")]
impl<T: 'static> Threaded for T {}

#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + ?Sized> MaybeSend for T {}

#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}

#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

#[derive(Clone, Debug)]
pub struct Request {
    pub url: Url,
    pub policy: CachePolicy,
    pub headers: HeaderMap,
    pub timeout: Duration,
    pub max_body: usize,
    pub max_redirects: u32,
    pub user_agent: String,
}

#[derive(Clone, Debug)]
pub struct Response {
    pub url: Url,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub trait Fetch: Threaded {
    fn fetch_blocking(&self, request: &Request) -> Option<Result<Response, Failure>>;

    fn fetch(&self, request: Request) -> impl Future<Output = Result<Response, Failure>> {
        async move {
            self.fetch_blocking(&request).unwrap_or_else(|| {
                Err(Failure::new(
                    ResourceErrorKind::UnsupportedOperation,
                    ResourceErrorPhase::Open,
                    "this transport cannot fetch synchronously",
                ))
            })
        }
    }

    fn owns_http_cache(&self) -> bool;

    fn map_file(&self, url: &Url) -> Option<Result<Bytes, Failure>> {
        let _ = url;
        None
    }
}

#[derive(Clone, Debug)]
pub struct Meta {
    pub key: String,
    pub len: u64,
    pub last_used: SystemTime,
    pub record: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Estimate {
    pub used: u64,
    pub quota: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Volatility {
    Durable,
    PurgedWhenIdle,
    EvictableAnytime,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store IO failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("store entry is corrupt: {0}")]
    Corrupt(String),
    #[error("store failed: {0}")]
    Other(String),
}

pub trait Store: Threaded {
    fn scan(&self) -> Result<Vec<Meta>, StoreError>;
    fn get(&self, key: &str) -> Result<Option<(Meta, Bytes)>, StoreError>;
    fn get_meta(&self, key: &str) -> Result<Option<Meta>, StoreError>;
    fn put(&self, meta: &Meta, body: &[u8]) -> Result<(), StoreError>;
    fn put_meta(&self, meta: &Meta) -> Result<(), StoreError>;
    fn delete(&self, key: &str) -> Result<(), StoreError>;
    fn touch(&self, key: &str, now: SystemTime) -> Result<(), StoreError>;
    fn map_body(&self, key: &str) -> Option<Bytes> {
        let _ = key;
        None
    }
    fn estimate(&self) -> Estimate;
    fn volatility(&self) -> Volatility;
}

#[derive(Clone, Copy, Debug)]
pub enum NoStore {}

impl Store for NoStore {
    fn scan(&self) -> Result<Vec<Meta>, StoreError> {
        match *self {}
    }

    fn get(&self, _key: &str) -> Result<Option<(Meta, Bytes)>, StoreError> {
        match *self {}
    }

    fn get_meta(&self, _key: &str) -> Result<Option<Meta>, StoreError> {
        match *self {}
    }

    fn put(&self, _meta: &Meta, _body: &[u8]) -> Result<(), StoreError> {
        match *self {}
    }

    fn put_meta(&self, _meta: &Meta) -> Result<(), StoreError> {
        match *self {}
    }

    fn delete(&self, _key: &str) -> Result<(), StoreError> {
        match *self {}
    }

    fn touch(&self, _key: &str, _now: SystemTime) -> Result<(), StoreError> {
        match *self {}
    }

    fn map_body(&self, _key: &str) -> Option<Bytes> {
        match *self {}
    }

    fn estimate(&self) -> Estimate {
        match *self {}
    }

    fn volatility(&self) -> Volatility {
        match *self {}
    }
}

pub trait AsyncStore: Threaded {
    fn scan(&self) -> impl Future<Output = Result<Vec<Meta>, StoreError>> + MaybeSend;
    fn get(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<(Meta, Bytes)>, StoreError>> + MaybeSend;
    fn put(
        &self,
        meta: &Meta,
        body: Bytes,
    ) -> impl Future<Output = Result<(), StoreError>> + MaybeSend;
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), StoreError>> + MaybeSend;
    fn touch(
        &self,
        key: &str,
        now: SystemTime,
    ) -> impl Future<Output = Result<(), StoreError>> + MaybeSend;
    fn estimate(&self) -> impl Future<Output = Estimate> + MaybeSend;
    fn volatility(&self) -> Volatility;
}

impl AsyncStore for NoStore {
    fn scan(&self) -> impl Future<Output = Result<Vec<Meta>, StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn get(
        &self,
        _key: &str,
    ) -> impl Future<Output = Result<Option<(Meta, Bytes)>, StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn put(
        &self,
        _meta: &Meta,
        _body: Bytes,
    ) -> impl Future<Output = Result<(), StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn delete(&self, _key: &str) -> impl Future<Output = Result<(), StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn touch(
        &self,
        _key: &str,
        _now: SystemTime,
    ) -> impl Future<Output = Result<(), StoreError>> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn estimate(&self) -> impl Future<Output = Estimate> + MaybeSend {
        let never = *self;
        async move { match never {} }
    }

    fn volatility(&self) -> Volatility {
        match *self {}
    }
}

pub trait ImageDecoder: Threaded {
    type Frame: Clone + std::fmt::Debug + Threaded;

    fn decode_blocking(
        &self,
        bytes: &[u8],
        format: ImageFormat,
        header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError>;

    fn decode(
        &self,
        bytes: Bytes,
        format: ImageFormat,
        header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> impl Future<Output = Result<DecodedImage<Self::Frame>, DecodeError>> {
        async move {
            self.decode_blocking(&bytes, format, header, max)
                .map(DecodedImage::from)
        }
    }

    fn available(&self) -> Result<(), DecodeError>;

    fn frames_blocking(
        &self,
        bytes: &[u8],
        format: ImageFormat,
        header: Option<ImageHeader>,
    ) -> Result<ImageFrames, DecodeError> {
        let _ = (bytes, format, header);
        Ok(ImageFrames::still())
    }

    fn frames(
        &self,
        bytes: Bytes,
        format: ImageFormat,
        header: Option<ImageHeader>,
    ) -> impl Future<Output = Result<ImageFrames, DecodeError>> {
        async move { self.frames_blocking(&bytes, format, header) }
    }

    fn decode_frame_blocking(
        &self,
        bytes: &Bytes,
        format: ImageFormat,
        header: Option<ImageHeader>,
        index: usize,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        if index == 0 {
            return self.decode_blocking(bytes, format, header, max);
        }
        Err(DecodeError::Unsupported(format!(
            "this decoder decodes one frame, not frame {index}"
        )))
    }

    fn forget_frames(&self, bytes: &Bytes) {
        let _ = bytes;
    }

    fn decode_frame(
        &self,
        bytes: Bytes,
        format: ImageFormat,
        header: Option<ImageHeader>,
        index: usize,
        max: (u32, u32),
    ) -> impl Future<Output = Result<DecodedImage<Self::Frame>, DecodeError>> {
        async move {
            if index == 0 {
                return self.decode(bytes, format, header, max).await;
            }
            Err(DecodeError::Unsupported(format!(
                "this decoder decodes one frame, not frame {index}"
            )))
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Tier<S> {
    pub store: S,
    pub budget: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum ArtifactStore<S, A> {
    Blocking(S),
    Async(A),
}

#[derive(Debug)]
pub struct Parts<F, S, A, D, M = crate::media::NoMedia> {
    pub fetch: F,
    pub decoder: D,
    pub media: M,
    pub bytes: Option<Tier<S>>,
    pub artifacts: Option<Tier<ArtifactStore<S, A>>>,
}

pub trait Platform: 'static {
    type Fetch: Fetch;
    type Store: Store;
    type AsyncStore: AsyncStore;
    type Decoder: ImageDecoder;
    type Media: crate::media::Media<<Self::Decoder as ImageDecoder>::Frame>;

    fn into_parts(self) -> PartsOf<Self>;
}

pub type PartsOf<P> = Parts<
    <P as Platform>::Fetch,
    <P as Platform>::Store,
    <P as Platform>::AsyncStore,
    <P as Platform>::Decoder,
    <P as Platform>::Media,
>;
