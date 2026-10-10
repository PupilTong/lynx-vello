//! The image pipeline: what happens between the painter naming a source and
//! the frame reading its pixels.
//!
//! A request resolves the source, then a job — one task on the fetcher's own
//! runtime natively, a local task in the browser — fetches the bytes and runs
//! them through the preprocessing pipeline on a blocking thread, takes a
//! decode permit, and decodes them with the platform decoder at a bounded
//! size on a blocking thread. Its completion lands in a channel the painter
//! drains in its next turn, which is when the load is reported and the bitmap
//! enters the memory tier. From then on the frame reads the source with the
//! size it draws it at, and three things can happen:
//!
//! - the resident bitmap is about the right size, and is returned;
//! - the resident bitmap is far larger (or smaller, with more to give) than the draw, and is
//!   returned while a refinement job at the drawn size runs off the painter's thread and replaces
//!   it — the downsampling the size hint exists for;
//! - the bitmap was evicted, and is restored on the painter's thread, inside the read, from the
//!   encoded bytes — kept in memory when nothing else holds them, read back from the disk tier or
//!   the file when something does — because a read after a reported load must not miss. That
//!   restore fetches and decodes synchronously and takes no decode permit: it must never wait for a
//!   background decode.
//!
//! # SVG documents
//!
//! An SVG document is parsed here, where a bitmap would be decoded, with the
//! engine's own parser ([`ImageEvent::parse_document`]). When preprocessing
//! says the bytes are [`ImageFormat::Svg`], the job hands them to the parser
//! instead of the platform decoder: natively under a decode permit, in a
//! closure of its own on the blocking pool; in the browser inside the load's
//! local task on the Render Worker, whose executor has no permits. The
//! browser's `Image` element never sees an SVG, so every target parses the
//! same bytes the same way.
//!
//! A document the page wrote itself (the Lynx `<svg content>`) arrives as a
//! document request instead ([`request_document`]): its bytes and kind under
//! a synthetic source the engine named, with nothing to fetch, parsed by the
//! same job half, natively under a permit of its own.
//!
//! Either is reported through [`ImageReports::parsed_document`], or through
//! [`ImageReports::failed`] when it does not parse. **Documents are parsed
//! and reported, never retained**: the engine owns the scene it encodes from
//! the report, and this pipeline keeps nothing of the document. Nothing
//! enters the memory tier or [`Resources::memory_used_bytes`],
//! [`FrameImages::read`](bobcat_core::FrameImages::read) answers `None` (the
//! engine draws the document itself and never asks), and no refinement and
//! no restore can start for it. A document request files no entry at all,
//! before, during or after its parse: two requests for one source are two
//! parses and two reports, and the engine applies the first that finds the
//! source pending. A fetched document leaves [`Entry::Parsed`], a marker
//! that holds nothing and keeps the source known; a later request for it
//! fetches and parses it again.
//!
//! The document's state never regresses, and a bitmap's does not here: an
//! entry that failed stays failed, and one that loaded as a bitmap stays
//! loaded whatever the memory tier holds for it. The parsed marker is the
//! one entry a later request starts over.

use std::sync::Arc;

use bobcat_core::vello::peniko::ImageData;
use bobcat_core::{
    DocumentKind, ImageEvent, ImageReports, ImageSizeHint, MAX_RENDERABLE_DIMENSION, VectorDocument,
};
use bytes::Bytes;
use rustc_hash::FxHashMap;
use url::Url;

use crate::cache::memory::MemoryCache;
use crate::decode::{Bitmap, DecodeError};
use crate::image_header::ImageHeader;
use crate::mime::ImageFormat;
use crate::preprocess::{self, Payload};
use crate::{Resources, Shared, SharedHandle};

/// What a raster load hands back: the bitmap, what the bytes were, and the
/// bytes themselves when nothing else could restore them.
type LoadedImage = (Bitmap, ImageFormat, Option<ImageHeader>, Option<Bytes>);

/// What a load job hands back: a decoded bitmap, or a parsed document.
enum LoadOutcome {
    Raster(LoadedImage),
    Vector(Arc<VectorDocument>),
}

impl LoadOutcome {
    /// The completion that reports this outcome for `source`.
    fn completion(self, source: Arc<str>, url: Url) -> Completion {
        match self {
            Self::Raster((bitmap, format, header, encoded)) => Completion::Loaded {
                source,
                url,
                bitmap,
                format,
                header,
                encoded,
            },
            Self::Vector(document) => Completion::ParsedDocument { source, document },
        }
    }
}

/// Parses `bytes`, a document of `kind` for `source`, with the engine's
/// parser: the job half that takes the place of a decode for a document. A
/// document that does not parse is the job's failure.
fn parse(
    source: &Arc<str>,
    bytes: &[u8],
    kind: DocumentKind,
) -> Result<Arc<VectorDocument>, String> {
    match ImageEvent::parse_document(Arc::clone(source), bytes, kind) {
        ImageEvent::ParsedDocument { document, .. } => Ok(document),
        ImageEvent::Loaded { .. } | ImageEvent::Failed { .. } => {
            Err(format!("`{source}` is not a document the engine can draw"))
        }
    }
}

/// The painter-thread half: what is known about every source asked for.
pub(crate) struct ImageState {
    entries: FxHashMap<Arc<str>, Entry>,
    bitmaps: MemoryCache<ImageData>,
    /// Document requests whose parse is in flight, each with the view that
    /// asked: where a parse's report goes, since a view's [`ImageReports`]
    /// cannot travel with the job. Not an entry: two requests for one source
    /// are two of these, two parses and two reports, and each leaves the
    /// list when a parse of its source completes.
    requested: Vec<(Arc<str>, ImageReports)>,
    /// The painter is the only consumer, and drains without blocking.
    completions: tokio::sync::mpsc::UnboundedReceiver<Completion>,
}

enum Entry {
    /// Asked for; the job has not completed. Every view that asked is told
    /// when it does.
    Loading {
        waiters: Vec<ImageReports>,
    },
    /// A decoded raster image.
    Loaded(Loaded),
    /// A fetched document that parsed and was reported to every view that
    /// asked. Nothing of it is kept, since the engine owns the scene it
    /// encodes; the marker keeps the source known, and a later request
    /// fetches and parses it again. It has no bitmap, so the memory tier,
    /// refinement and restore never see it.
    Parsed,
    Failed,
}

struct Loaded {
    url: Url,
    intrinsic: (u32, u32),
    /// The container, for a decoder that wants to be told what it is given.
    /// Never [`ImageFormat::Svg`]: an SVG document completes as
    /// [`Entry::Parsed`], so no restore or refinement ever decodes one.
    format: ImageFormat,
    header: Option<ImageHeader>,
    /// The encoded bytes, kept when no other tier can hand them back: a
    /// registration that may be cleared, a `data:` URL, a response the disk
    /// did not store, and everything in the browser.
    encoded: Option<Bytes>,
    /// A refinement decode in flight, and the size it targets.
    refining: Option<(u32, u32)>,
}

/// What a job sends back.
pub(crate) enum Completion {
    Loaded {
        source: Arc<str>,
        url: Url,
        bitmap: Bitmap,
        format: ImageFormat,
        header: Option<ImageHeader>,
        encoded: Option<Bytes>,
    },
    /// A fetched document, parsed.
    ParsedDocument {
        source: Arc<str>,
        document: Arc<VectorDocument>,
    },
    /// A document request's parse: the document, or why it did not parse.
    RequestedDocument {
        source: Arc<str>,
        parsed: Result<Arc<VectorDocument>, String>,
    },
    Refined {
        source: Arc<str>,
        target: (u32, u32),
        bitmap: Bitmap,
    },
    Failed {
        source: Arc<str>,
        message: String,
    },
    RefineFailed {
        source: Arc<str>,
        message: String,
    },
}

impl ImageState {
    pub(crate) fn new(
        budget: usize,
        completions: tokio::sync::mpsc::UnboundedReceiver<Completion>,
    ) -> Self {
        Self {
            entries: FxHashMap::default(),
            bitmaps: MemoryCache::new(budget),
            requested: Vec::new(),
            completions,
        }
    }

    pub(crate) fn budget(&self) -> usize {
        self.bitmaps.budget()
    }

    /// Bytes held by decoded bitmaps.
    pub(crate) fn bitmap_bytes(&self) -> usize {
        self.bitmaps.used_bytes()
    }

    /// Bytes held by encoded images nothing else can restore.
    pub(crate) fn encoded_bytes(&self) -> usize {
        self.entries
            .values()
            .filter_map(|entry| match entry {
                Entry::Loaded(loaded) => loaded.encoded.as_ref().map(Bytes::len),
                Entry::Loading { .. } | Entry::Parsed | Entry::Failed => None,
            })
            .sum()
    }

    pub(crate) fn set_budget(&mut self, budget: usize) {
        self.bitmaps.set_budget(budget);
    }

    /// Whether `source` has been asked for at all. A document request does
    /// not count: it files no entry.
    pub(crate) fn knows(&self, source: &str) -> bool {
        self.entries.contains_key(source)
    }

    /// Whether `source` has a resident bitmap.
    pub(crate) fn is_resident(&self, source: &str) -> bool {
        self.bitmaps.contains(source)
    }

    /// The resident bitmap's size, if any.
    pub(crate) fn resident_size(&self, source: &str) -> Option<(u32, u32)> {
        self.bitmaps
            .peek(source)
            .map(|image| (image.width, image.height))
    }
}

/// Names `source` for `reports`: answers at once if the pipeline already
/// knows it as a bitmap or a failure, joins a load in flight, and otherwise
/// starts the load and remembers who asked. A document parsed before is
/// fetched and parsed again.
pub(crate) fn request(resources: &Resources, source: &str, reports: &ImageReports) {
    let mut state = resources.local.borrow_mut();
    match state.entries.get_mut(source) {
        Some(Entry::Loaded(loaded)) => {
            reports.loaded(source, loaded.intrinsic.0, loaded.intrinsic.1);
            return;
        }
        Some(Entry::Failed) => {
            reports.failed(source);
            return;
        }
        Some(Entry::Loading { waiters }) => {
            waiters.push(reports.clone());
            return;
        }
        Some(Entry::Parsed) | None => {}
    }
    let base = resources.base_url();
    let url = match resources.shared.transports.resolve(source, base.as_ref()) {
        Ok(url) => url,
        Err(failure) => {
            state.entries.insert(Arc::from(source), Entry::Failed);
            drop(state);
            resources.note(format!("image `{source}` cannot be resolved: {failure}"));
            reports.failed(source);
            return;
        }
    };
    let source: Arc<str> = Arc::from(source);
    state.entries.insert(
        Arc::clone(&source),
        Entry::Loading {
            waiters: vec![reports.clone()],
        },
    );
    drop(state);
    let bound = resources
        .shared
        .initial_decode_bound
        .min(MAX_RENDERABLE_DIMENSION);
    spawn_load(resources, source, url, (bound, bound));
}

/// Hands `bytes`, a document of `kind` a page wrote itself, to a parse of
/// its own under `source`, the synthetic name the engine filed it as, whose
/// outcome is reported to `reports`. Nothing is resolved or fetched, no
/// entry is filed and none is consulted: every request is one parse and one
/// report.
pub(crate) fn request_document(
    resources: &Resources,
    source: &str,
    bytes: Bytes,
    kind: DocumentKind,
    reports: &ImageReports,
) {
    let source: Arc<str> = Arc::from(source);
    resources
        .local
        .borrow_mut()
        .requested
        .push((Arc::clone(&source), reports.clone()));
    spawn_parse(resources, source, bytes, kind);
}

/// Applies every completion queued since the last turn.
pub(crate) fn service(resources: &Resources) {
    let completions: Vec<Completion> = {
        let mut state = resources.local.borrow_mut();
        let mut drained = Vec::new();
        // Empty and disconnected both end the drain: a scope whose senders
        // are all gone has nothing more to report.
        while let Ok(completion) = state.completions.try_recv() {
            drained.push(completion);
        }
        drained
    };
    for completion in completions {
        apply(resources, completion);
    }
}

fn apply(resources: &Resources, completion: Completion) {
    let mut state = resources.local.borrow_mut();
    match completion {
        Completion::Loaded {
            source,
            url,
            bitmap,
            format,
            header,
            encoded,
        } => {
            let Some(Entry::Loading { waiters }) = state.entries.remove(&source) else {
                return;
            };
            let intrinsic = (bitmap.source_width, bitmap.source_height);
            let bytes = bitmap.byte_len();
            let image = bitmap.into_image_data();
            state.bitmaps.insert(Arc::clone(&source), image, bytes);
            state.entries.insert(
                Arc::clone(&source),
                Entry::Loaded(Loaded {
                    url,
                    intrinsic,
                    format,
                    header,
                    encoded,
                    refining: None,
                }),
            );
            drop(state);
            for reports in waiters {
                reports.loaded(&source, intrinsic.0, intrinsic.1);
            }
        }
        Completion::ParsedDocument { source, document } => {
            let Some(Entry::Loading { waiters }) = state.entries.remove(&source) else {
                return;
            };
            state.entries.insert(Arc::clone(&source), Entry::Parsed);
            drop(state);
            for reports in waiters {
                reports.parsed_document(&source, Arc::clone(&document));
            }
        }
        Completion::RequestedDocument { source, parsed } => {
            // Any request for this source will do: the source names its
            // markup, so every parse of it is the same document.
            let Some(index) = state
                .requested
                .iter()
                .position(|(requested, _)| *requested == source)
            else {
                return;
            };
            let (_, reports) = state.requested.swap_remove(index);
            drop(state);
            match parsed {
                Ok(document) => reports.parsed_document(&source, document),
                Err(message) => {
                    resources.note(format!("image `{source}` failed to load: {message}"));
                    reports.failed(&source);
                }
            }
        }
        Completion::Refined {
            source,
            target,
            bitmap,
        } => {
            let Some(Entry::Loaded(loaded)) = state.entries.get_mut(&source) else {
                return;
            };
            if loaded.refining != Some(target) {
                return;
            }
            loaded.refining = None;
            let bytes = bitmap.byte_len();
            state
                .bitmaps
                .insert(Arc::clone(&source), bitmap.into_image_data(), bytes);
        }
        Completion::Failed { source, message } => {
            let Some(Entry::Loading { waiters }) = state.entries.remove(&source) else {
                return;
            };
            state.entries.insert(Arc::clone(&source), Entry::Failed);
            drop(state);
            resources.note(format!("image `{source}` failed to load: {message}"));
            for reports in waiters {
                reports.failed(&source);
            }
        }
        Completion::RefineFailed { source, message } => {
            if let Some(Entry::Loaded(loaded)) = state.entries.get_mut(&source) {
                loaded.refining = None;
            }
            drop(state);
            resources.note(format!(
                "image `{source}` could not be re-decoded: {message}"
            ));
        }
    }
}

/// Records the frame's working set: those bitmaps are never evicted.
pub(crate) fn retain(resources: &Resources, frame: &[Arc<str>]) {
    let mut state = resources.local.borrow_mut();
    let _ = state.bitmaps.pin(frame);
}

/// The pixels for `source`, for a draw of `hint`.
pub(crate) fn read(resources: &Resources, source: &str, hint: ImageSizeHint) -> Option<ImageData> {
    let mut state = resources.local.borrow_mut();
    let state = &mut *state;
    let loaded = match state.entries.get_mut(source)? {
        Entry::Loaded(loaded) => loaded,
        // The engine draws a document itself and never reads it here;
        // answering `None` is what keeps every restore and refinement below
        // unreachable for an SVG document.
        Entry::Parsed | Entry::Loading { .. } | Entry::Failed => return None,
    };
    let target = bounded(hint.fit(loaded.intrinsic.0, loaded.intrinsic.1));
    if let Some(image) = state.bitmaps.get(source) {
        let image = image.clone();
        if loaded.refining.is_none()
            && wants_refinement(
                (image.width, image.height),
                target,
                loaded.intrinsic,
                resources.shared.downsample_ratio,
            )
        {
            loaded.refining = Some(target);
            spawn_refine(
                resources,
                Arc::from(source),
                Refinement {
                    url: loaded.url.clone(),
                    encoded: loaded.encoded.clone(),
                    format: loaded.format,
                    header: loaded.header,
                    target,
                },
            );
        }
        return Some(image);
    }
    // Evicted: restore now, on this thread, with no decode permit. The
    // contract is that a reported load never misses, and blocking the
    // painter here is the accepted price.
    let bytes = match loaded.encoded.clone() {
        Some(bytes) => bytes,
        None => match restore_bytes(&resources.shared, &loaded.url) {
            Ok(bytes) => bytes,
            Err(message) => {
                resources.note(format!("image `{source}` could not be restored: {message}"));
                return None;
            }
        },
    };
    match decode_blocking(
        &resources.shared,
        &bytes,
        loaded.format,
        loaded.header,
        target,
    ) {
        Ok(bitmap) => {
            let bytes = bitmap.byte_len();
            let image = bitmap.into_image_data();
            state
                .bitmaps
                .insert(Arc::from(source), image.clone(), bytes);
            Some(image)
        }
        Err(error) => {
            resources.note(format!("image `{source}` could not be re-decoded: {error}"));
            None
        }
    }
}

/// Never ask a decoder for more than vello can draw.
fn bounded(target: (u32, u32)) -> (u32, u32) {
    ImageSizeHint::new(MAX_RENDERABLE_DIMENSION, MAX_RENDERABLE_DIMENSION).fit(target.0, target.1)
}

/// Whether a resident bitmap of `resident` should be replaced by one decoded
/// for `target`: it is at least `ratio` times too large on both axes, or it
/// is smaller than the draw while the image itself has more pixels to give.
pub(crate) fn wants_refinement(
    resident: (u32, u32),
    target: (u32, u32),
    intrinsic: (u32, u32),
    ratio: f32,
) -> bool {
    #[expect(
        clippy::cast_precision_loss,
        reason = "pixel counts are compared approximately, by ratio"
    )]
    let too_large = resident.0 as f32 >= target.0 as f32 * ratio
        && resident.1 as f32 >= target.1 as f32 * ratio;
    let too_small = (resident.0 < target.0 || resident.1 < target.1)
        && (resident.0 < intrinsic.0 || resident.1 < intrinsic.1);
    too_large || too_small
}

/// What a load's transport-and-preprocess step hands on.
#[cfg(not(target_arch = "wasm32"))]
enum Prepared {
    /// Encoded raster bytes, for the decode.
    Raster {
        format: ImageFormat,
        header: Option<ImageHeader>,
        bytes: Bytes,
        /// Whether another tier can hand these bytes back, so the entry need
        /// not keep them.
        restorable: bool,
    },
    /// A document, for the engine's parser rather than the decoder.
    Document { bytes: Bytes, kind: DocumentKind },
}

/// The blocking half of a load: fetch the bytes and run them through the
/// preprocessing pipeline. Runs on a blocking-pool thread.
#[cfg(not(target_arch = "wasm32"))]
fn prepare(shared: &Shared, source: &str, url: &Url) -> Result<Prepared, String> {
    let fetched = shared
        .fetch_job(url, crate::CachePolicy::Default, &http::HeaderMap::new())
        .map_err(|failure| failure.to_string())?;
    let restorable = fetched.restorable;
    let preprocessed =
        preprocess::preprocess(fetched.bytes, fetched.media_type.as_ref(), Some(url))
            .map_err(|error| error.to_string())?;
    let Payload::Image { format, header } = preprocessed.payload else {
        return Err(format!(
            "`{source}` is {}, not an image",
            preprocessed.media_type
        ));
    };
    if format == ImageFormat::Svg {
        return Ok(Prepared::Document {
            bytes: preprocessed.bytes,
            kind: DocumentKind::Svg,
        });
    }
    Ok(Prepared::Raster {
        format,
        header,
        bytes: preprocessed.bytes,
        restorable,
    })
}

/// The job behind a request: prepare on the pool, take a decode permit, then
/// decode on the pool. The permit is held from before the decode closure is
/// submitted until it returns, so a decode that has to wait holds no thread.
/// A document is parsed in the decode's place, under the same permit.
#[cfg(not(target_arch = "wasm32"))]
async fn load(
    shared: &SharedHandle,
    handle: &tokio::runtime::Handle,
    permits: &Arc<tokio::sync::Semaphore>,
    source: &Arc<str>,
    url: &Url,
    bound: (u32, u32),
) -> Result<LoadOutcome, String> {
    let prepared = {
        let shared = SharedHandle::clone(shared);
        let source = Arc::clone(source);
        let url = url.clone();
        crate::executor::blocking(handle, "load", move || prepare(&shared, &source, &url)).await??
    };
    let (format, header, encoded, restorable) = match prepared {
        Prepared::Document { bytes, kind } => {
            return parse_job(handle, permits, source, bytes, kind)
                .await
                .map(LoadOutcome::Vector);
        }
        Prepared::Raster {
            format,
            header,
            bytes,
            restorable,
        } => (format, header, bytes, restorable),
    };
    let permit = acquire_decode(permits).await?;
    let bitmap = {
        let shared = SharedHandle::clone(shared);
        let bytes = encoded.clone();
        crate::executor::blocking(handle, "decode", move || {
            let _permit = permit;
            shared.decode_job(&bytes, format, header, bound)
        })
        .await?
        .map_err(|error| error.to_string())?
    };
    let encoded = (!restorable).then_some(encoded);
    Ok(LoadOutcome::Raster((bitmap, format, header, encoded)))
}

/// The parse a document takes in place of a decode: a decode permit, taken
/// before the closure is submitted and held until it returns, then the
/// engine's parser on the blocking pool. A parse that panicked is the
/// job's failure, as a decode's is.
#[cfg(not(target_arch = "wasm32"))]
async fn parse_job(
    handle: &tokio::runtime::Handle,
    permits: &Arc<tokio::sync::Semaphore>,
    source: &Arc<str>,
    bytes: Bytes,
    kind: DocumentKind,
) -> Result<Arc<VectorDocument>, String> {
    let permit = acquire_decode(permits).await?;
    let source = Arc::clone(source);
    crate::executor::blocking(handle, "parse", move || {
        let _permit = permit;
        parse(&source, &bytes, kind)
    })
    .await?
}

/// One decode permit, taken before any decode closure is submitted.
#[cfg(not(target_arch = "wasm32"))]
async fn acquire_decode(
    permits: &Arc<tokio::sync::Semaphore>,
) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
    Arc::clone(permits)
        .acquire_owned()
        .await
        .map_err(|_| "the decode permits were closed".to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_load(resources: &Resources, source: Arc<str>, url: Url, bound: (u32, u32)) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    let permits = resources.executor.decode_permits();
    resources.executor.spawn(async move {
        // The completion is sent here, in the task, outside every unwinding
        // region: a panicking closure is a `JoinError` this maps, not a
        // second completion.
        let completion = match load(&shared, &handle, &permits, &source, &url, bound).await {
            Ok(outcome) => outcome.completion(source, url),
            Err(message) => Completion::Failed { source, message },
        };
        shared.complete(completion);
    });
}

/// Starts the parse of a document the page handed over: [`parse_job`] as a
/// task of its own, with no transport and no preprocessing in front of it.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_parse(resources: &Resources, source: Arc<str>, bytes: Bytes, kind: DocumentKind) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    let permits = resources.executor.decode_permits();
    resources.executor.spawn(async move {
        let parsed = parse_job(&handle, &permits, &source, bytes, kind).await;
        shared.complete(Completion::RequestedDocument { source, parsed });
    });
}

/// Starts the parse of a document the page handed over, in a local task on
/// the Render Worker, as a fetched document's parse runs in its load's.
#[cfg(target_arch = "wasm32")]
fn spawn_parse(resources: &Resources, source: Arc<str>, bytes: Bytes, kind: DocumentKind) {
    let shared = SharedHandle::clone(&resources.shared);
    wasm_bindgen_futures::spawn_local(async move {
        let parsed = parse(&source, &bytes, kind);
        shared.complete(Completion::RequestedDocument { source, parsed });
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn_load(resources: &Resources, source: Arc<str>, url: Url, bound: (u32, u32)) {
    let shared = SharedHandle::clone(&resources.shared);
    wasm_bindgen_futures::spawn_local(async move {
        let fetched = shared
            .transports
            .fetch(&url, crate::CachePolicy::Default, &http::HeaderMap::new())
            .await;
        let completion = match load_async(&shared, &source, &url, fetched, bound).await {
            Ok(outcome) => outcome.completion(source, url),
            Err(message) => Completion::Failed { source, message },
        };
        shared.complete(completion);
    });
}

#[cfg(target_arch = "wasm32")]
async fn load_async(
    shared: &Shared,
    source: &Arc<str>,
    url: &Url,
    fetched: Result<crate::transport::Fetched, crate::error::Failure>,
    bound: (u32, u32),
) -> Result<LoadOutcome, String> {
    let fetched = fetched.map_err(|failure| failure.to_string())?;
    let restorable = fetched.restorable;
    let preprocessed =
        preprocess::preprocess(fetched.bytes, fetched.media_type.as_ref(), Some(url))
            .map_err(|error| error.to_string())?;
    let Payload::Image { format, header } = preprocessed.payload else {
        return Err(format!(
            "`{}` is {}, not an image",
            source, preprocessed.media_type
        ));
    };
    // An SVG document is parsed here, in this task, with the engine's
    // parser: it never reaches the main thread's `Image` element.
    if format == ImageFormat::Svg {
        return parse(source, &preprocessed.bytes, DocumentKind::Svg).map(LoadOutcome::Vector);
    }
    let bitmap = shared
        .decode_bytes_async(&preprocessed.bytes, format, header, bound)
        .await
        .map_err(|error| error.to_string())?;
    let encoded = (!restorable).then_some(preprocessed.bytes);
    Ok(LoadOutcome::Raster((bitmap, format, header, encoded)))
}

/// What a refinement re-decodes, and at what size.
pub(crate) struct Refinement {
    url: Url,
    /// The entry's own bytes, when it kept them; otherwise another tier is
    /// asked for them again.
    encoded: Option<Bytes>,
    format: ImageFormat,
    header: Option<ImageHeader>,
    target: (u32, u32),
}

/// The refinement job: recover the bytes if the entry did not keep them,
/// take a decode permit, decode at the drawn size.
#[cfg(not(target_arch = "wasm32"))]
async fn refine(
    shared: &SharedHandle,
    handle: &tokio::runtime::Handle,
    permits: &Arc<tokio::sync::Semaphore>,
    refinement: Refinement,
) -> Result<Bitmap, String> {
    let Refinement {
        url,
        encoded,
        format,
        header,
        target,
    } = refinement;
    let bytes = if let Some(bytes) = encoded {
        bytes
    } else {
        let shared = SharedHandle::clone(shared);
        crate::executor::blocking(handle, "restore", move || restore_for_job(&shared, &url))
            .await??
    };
    let permit = acquire_decode(permits).await?;
    let shared = SharedHandle::clone(shared);
    crate::executor::blocking(handle, "decode", move || {
        let _permit = permit;
        shared.decode_job(&bytes, format, header, target)
    })
    .await?
    .map_err(|error| error.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_refine(resources: &Resources, source: Arc<str>, refinement: Refinement) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    let permits = resources.executor.decode_permits();
    let target = refinement.target;
    // Only a task is enqueued here; the painter creates no thread, and this
    // call is made from inside compose.
    resources.executor.spawn(async move {
        // A refinement that fails leaves the `Loaded` entry alone: its arm
        // in `apply` only clears `refining`.
        let completion = match refine(&shared, &handle, &permits, refinement).await {
            Ok(bitmap) => Completion::Refined {
                source,
                target,
                bitmap,
            },
            Err(message) => Completion::RefineFailed { source, message },
        };
        shared.complete(completion);
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn_refine(resources: &Resources, source: Arc<str>, refinement: Refinement) {
    let shared = SharedHandle::clone(&resources.shared);
    let Refinement {
        url,
        encoded,
        format,
        header,
        target,
    } = refinement;
    wasm_bindgen_futures::spawn_local(async move {
        let bytes = match encoded {
            Some(bytes) => Ok(bytes),
            None => restore_bytes(&shared, &url),
        };
        let completion = match bytes {
            Ok(bytes) => match shared
                .decode_bytes_async(&bytes, format, header, target)
                .await
            {
                Ok(bitmap) => Completion::Refined {
                    source,
                    target,
                    bitmap,
                },
                Err(error) => Completion::RefineFailed {
                    source,
                    message: error.to_string(),
                },
            },
            Err(message) => Completion::RefineFailed { source, message },
        };
        shared.complete(completion);
    });
}

/// Encoded bytes for a refinement whose bytes were not kept: the same tiers
/// as [`restore_bytes`], reached through the background job's transport seam
/// on a blocking-pool thread.
#[cfg(not(target_arch = "wasm32"))]
fn restore_for_job(shared: &Shared, url: &Url) -> Result<Bytes, String> {
    shared
        .fetch_job(url, crate::CachePolicy::ForceCache, &http::HeaderMap::new())
        .map(|fetched| fetched.bytes)
        .map_err(|failure| failure.to_string())
}

/// Encoded bytes for an image whose bytes were not kept: the disk tier or
/// the file natively, only what is already in hand in the browser.
///
/// Natively this is the painter's own restore, called inside
/// [`FrameImages::read`](bobcat_core::FrameImages::read).
fn restore_bytes(shared: &Shared, url: &Url) -> Result<Bytes, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        shared
            .transports
            .fetch_blocking(url, crate::CachePolicy::ForceCache, &http::HeaderMap::new())
            .map(|fetched| fetched.bytes)
            .map_err(|failure| failure.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        shared
            .transports
            .local(url)
            .unwrap_or_else(|| {
                Err(crate::error::Failure::new(
                    bobcat_core::resource::ResourceErrorKind::Unavailable,
                    bobcat_core::resource::ResourceErrorPhase::Open,
                    "the bytes were not retained and cannot be re-fetched synchronously",
                ))
            })
            .map(|fetched| fetched.bytes)
            .map_err(|failure| failure.to_string())
    }
}

/// The painter's own decode, for a restore inside a read: the platform
/// decoder directly, with no permit and no test seam.
fn decode_blocking(
    shared: &Shared,
    bytes: &[u8],
    format: ImageFormat,
    header: Option<ImageHeader>,
    target: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    shared.decode_bytes(bytes, format, header, target)
}

/// The executor's seams, pinned where the test can reach the decoder and the
/// transport: how many decodes may run at once, what a panicking closure
/// reports, and what a drop of the last `Resources` leaves behind.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod job_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use bobcat_core::{DocumentKind, ImageEvent, ImageInbox, ImageSizeHint};

    use crate::decode::Bitmap;
    use crate::{Resources, ResourcesConfig};

    /// A 1x1 RGBA PNG: enough for preprocessing to call it an image, while a
    /// hooked decoder answers for the platform one.
    fn tiny_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&[1, 2, 3, 255]).expect("png data");
        }
        bytes
    }

    /// What a hooked decode hands back, with `source` as the image's own size.
    fn pixel(width: u32, height: u32, source: (u32, u32)) -> Bitmap {
        Bitmap {
            width,
            height,
            source_width: source.0,
            source_height: source.1,
            premultiplied: false,
            rgba: [1_u8, 2, 3, 255].repeat(width as usize * height as usize),
        }
    }

    fn quiet(config: ResourcesConfig) -> ResourcesConfig {
        ResourcesConfig {
            log_to_stderr: false,
            ..config
        }
    }

    /// Services turns until `done` holds, or the deadline says the job never
    /// arrived.
    fn settle(resources: &Resources, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            super::service(resources);
            if done() {
                return;
            }
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn one_decode_permit_lets_only_one_decode_run_at_a_time() {
        let live = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let resources = Resources::new(
            quiet(ResourcesConfig {
                worker_threads: 4,
                decode_parallelism: Some(1),
                ..ResourcesConfig::default()
            }),
            || {},
        );
        resources.shared.set_decode_hook(Arc::new({
            let live = Arc::clone(&live);
            let max = Arc::clone(&max);
            let calls = Arc::clone(&calls);
            move |_bytes, _format, _header, _max| {
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                calls.fetch_add(1, Ordering::SeqCst);
                // A window wide enough for a second decode to be seen, and
                // one that never waits on another decode: with one permit no
                // rendezvous could ever complete.
                std::thread::sleep(Duration::from_millis(50));
                live.fetch_sub(1, Ordering::SeqCst);
                Ok(pixel(1, 1, (1, 1)))
            }
        }));
        let (reports, inbox) = ImageInbox::new();
        let sources: Vec<String> = (0..4).map(|index| format!("app:///{index}.png")).collect();
        for source in &sources {
            resources
                .register(source, tiny_png(), None)
                .expect("register");
            super::request(&resources, source, &reports);
        }
        let mut seen = 0;
        settle(&resources, "four images never reported", || {
            seen += inbox.drain().len();
            seen >= 4
        });
        assert_eq!(calls.load(Ordering::SeqCst), 4, "every image was decoded");
        assert_eq!(
            max.load(Ordering::SeqCst),
            1,
            "the permit is taken before the decode closure is submitted"
        );
    }

    /// An SVG document is parsed in the decode's place: it never reaches the
    /// decoder, but its parse waits for a decode permit like a decode does,
    /// so nothing reports while the only permit is held, and the parsed
    /// document reports once it is let go. The same holds for a document a
    /// page handed over, which has nothing to fetch.
    #[test]
    fn an_svg_is_parsed_under_a_decode_permit_without_the_decoder() {
        const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12"/>"#;
        let calls = Arc::new(AtomicUsize::new(0));
        let resources = Resources::new(
            quiet(ResourcesConfig {
                worker_threads: 2,
                decode_parallelism: Some(1),
                ..ResourcesConfig::default()
            }),
            || {},
        );
        resources.shared.set_decode_hook(Arc::new({
            let calls = Arc::clone(&calls);
            move |_bytes, _format, _header, _max| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(pixel(1, 1, (1, 1)))
            }
        }));
        let held = resources
            .executor
            .decode_permits()
            .try_acquire_owned()
            .expect("the only decode permit");
        let (reports, inbox) = ImageInbox::new();
        resources
            .register("app:///icon.svg", SVG.to_vec(), None)
            .expect("register");
        super::request(&resources, "app:///icon.svg", &reports);
        super::request_document(
            &resources,
            "svg-content:icon",
            bytes::Bytes::from_static(SVG),
            DocumentKind::Svg,
            &reports,
        );
        // Long enough for the fetch and the preprocessing to finish: the
        // parse is what waits.
        std::thread::sleep(Duration::from_millis(200));
        super::service(&resources);
        assert!(
            inbox.drain().is_empty(),
            "no parse runs while the permit is held"
        );

        drop(held);
        let mut events = Vec::new();
        settle(&resources, "the SVGs never reported", || {
            events.extend(inbox.drain());
            events.len() >= 2
        });
        let mut sources: Vec<&str> = events
            .iter()
            .map(|event| match event {
                ImageEvent::ParsedDocument { source, .. } => &**source,
                other => panic!("not a parsed document: {other:?}"),
            })
            .collect();
        sources.sort_unstable();
        assert_eq!(sources, ["app:///icon.svg", "svg-content:icon"]);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "the decoder was never asked"
        );
        assert_eq!(
            resources.memory_used_bytes(),
            0,
            "no bitmap, and nothing kept of either document"
        );
    }

    #[test]
    fn a_panicking_decode_fails_that_image_and_leaves_the_next_one_alone() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resources = Resources::new(
            quiet(ResourcesConfig {
                worker_threads: 2,
                ..ResourcesConfig::default()
            }),
            || {},
        );
        resources.shared.set_decode_hook(Arc::new({
            let calls = Arc::clone(&calls);
            move |_bytes, _format, _header, _max| {
                assert!(
                    calls.fetch_add(1, Ordering::SeqCst) != 0,
                    "the decoder gave up"
                );
                Ok(pixel(1, 1, (1, 1)))
            }
        }));
        let (reports, inbox) = ImageInbox::new();
        resources
            .register("app:///first.png", tiny_png(), None)
            .expect("register");
        super::request(&resources, "app:///first.png", &reports);
        let mut events = Vec::new();
        settle(&resources, "the panicking load never reported", || {
            events.extend(inbox.drain());
            !events.is_empty()
        });
        assert!(
            matches!(events.as_slice(), [ImageEvent::Failed { source }] if &**source == "app:///first.png"),
            "a panic is reported as a failure: {events:?}"
        );
        assert!(
            resources
                .take_notes()
                .iter()
                .any(|note| note.contains("the decode panicked: the decoder gave up")),
            "the panic's own message is what the failure carries"
        );

        resources
            .register("app:///second.png", tiny_png(), None)
            .expect("register");
        super::request(&resources, "app:///second.png", &reports);
        events.clear();
        settle(&resources, "the next image never reported", || {
            events.extend(inbox.drain());
            !events.is_empty()
        });
        assert!(
            matches!(events.as_slice(), [ImageEvent::Loaded { source, .. }] if &**source == "app:///second.png"),
            "the pool's thread survived the panic: {events:?}"
        );
    }

    #[test]
    fn a_panicking_refinement_keeps_the_loaded_entry_readable() {
        let resources = Resources::new(
            quiet(ResourcesConfig {
                worker_threads: 2,
                initial_decode_bound: 2048,
                downsample_ratio: 2.0,
                ..ResourcesConfig::default()
            }),
            || {},
        );
        // The initial decode is asked for the bound; a refinement is asked
        // for the drawn size, and that one panics.
        resources
            .shared
            .set_decode_hook(Arc::new(|_bytes, _format, _header, max| {
                assert_eq!(max, (2048, 2048), "the refinement panics instead");
                Ok(pixel(1000, 1000, (4000, 4000)))
            }));
        let (reports, inbox) = ImageInbox::new();
        resources
            .register("app:///big.png", tiny_png(), None)
            .expect("register");
        super::request(&resources, "app:///big.png", &reports);
        let mut events = Vec::new();
        settle(&resources, "the load never reported", || {
            events.extend(inbox.drain());
            !events.is_empty()
        });
        assert!(matches!(events.as_slice(), [ImageEvent::Loaded { .. }]));

        // Drawn eight times smaller: a refinement starts and its decode panics.
        assert!(
            super::read(&resources, "app:///big.png", ImageSizeHint::new(125, 125)).is_some(),
            "the read answers with what is resident"
        );
        settle(&resources, "the refinement never reported", || {
            resources
                .take_notes()
                .iter()
                .any(|note| note.contains("could not be re-decoded"))
        });
        assert!(
            resources.is_resident("app:///big.png"),
            "a failed refinement never removes the loaded bitmap"
        );
        assert!(
            resources.knows_image("app:///big.png"),
            "the entry stays loaded"
        );
    }

    /// The gate a test holds a blocking closure at.
    struct Gate {
        entered: Arc<AtomicBool>,
        open: Arc<AtomicBool>,
    }

    impl Gate {
        fn new() -> Self {
            Self {
                entered: Arc::new(AtomicBool::new(false)),
                open: Arc::new(AtomicBool::new(false)),
            }
        }

        fn hook(&self) -> crate::FetchHook {
            let entered = Arc::clone(&self.entered);
            let open = Arc::clone(&self.open);
            Arc::new(move |_url: &url::Url| {
                entered.store(true, Ordering::SeqCst);
                while !open.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(2));
                }
            })
        }

        fn wait_for_entry(&self) {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !self.entered.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "the job never reached the gate");
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        fn release(&self) {
            self.open.store(true, Ordering::SeqCst);
        }
    }

    /// Drops the last `Resources` with a blocking closure held at a gate,
    /// and reports how long the drop took and whether the wakeup moved after
    /// it returned.
    fn drop_with_a_job_in_flight() -> Duration {
        let wakeups = Arc::new(AtomicUsize::new(0));
        let resources = Resources::new(
            quiet(ResourcesConfig {
                worker_threads: 2,
                ..ResourcesConfig::default()
            }),
            {
                let wakeups = Arc::clone(&wakeups);
                move || {
                    wakeups.fetch_add(1, Ordering::SeqCst);
                }
            },
        );
        let gate = Gate::new();
        resources.shared.set_fetch_hook(gate.hook());
        let (reports, _inbox) = ImageInbox::new();
        resources
            .register("app:///gated.png", tiny_png(), None)
            .expect("register");
        super::request(&resources, "app:///gated.png", &reports);
        gate.wait_for_entry();

        let started = Instant::now();
        drop(resources);
        let elapsed = started.elapsed();
        let after_drop = wakeups.load(Ordering::SeqCst);

        gate.release();
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(
            wakeups.load(Ordering::SeqCst),
            after_drop,
            "no job wakes the embedder once the drop has returned"
        );
        elapsed
    }

    #[test]
    fn dropping_the_last_resources_returns_promptly_and_wakes_nothing_after() {
        let elapsed = drop_with_a_job_in_flight();
        assert!(
            elapsed < Duration::from_secs(2),
            "the drop waited for the running closure: {elapsed:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_same_drop_is_legal_inside_another_runtime() {
        let elapsed = drop_with_a_job_in_flight();
        assert!(
            elapsed < Duration::from_secs(2),
            "the drop waited for the running closure: {elapsed:?}"
        );
    }

    /// What makes "no wakeup after the drop returns" a guarantee rather than
    /// a race: the completion runs on the driver thread and the drop joins
    /// it, so a wakeup already running finishes first.
    #[test]
    fn the_drop_waits_for_a_wakeup_already_running_on_the_driver() {
        let entered = Arc::new(AtomicBool::new(false));
        let wakeups = Arc::new(AtomicUsize::new(0));
        let resources = Resources::new(
            quiet(ResourcesConfig {
                worker_threads: 2,
                ..ResourcesConfig::default()
            }),
            {
                let entered = Arc::clone(&entered);
                let wakeups = Arc::clone(&wakeups);
                move || {
                    entered.store(true, Ordering::SeqCst);
                    // A slow embedder: the count moves only at the end, so a
                    // drop that did not wait would read it before it moved.
                    std::thread::sleep(Duration::from_millis(400));
                    wakeups.fetch_add(1, Ordering::SeqCst);
                }
            },
        );
        resources
            .shared
            .set_decode_hook(Arc::new(|_bytes, _format, _header, _max| {
                Ok(pixel(1, 1, (1, 1)))
            }));
        let (reports, _inbox) = ImageInbox::new();
        resources
            .register("app:///slow-wakeup.png", tiny_png(), None)
            .expect("register");
        super::request(&resources, "app:///slow-wakeup.png", &reports);

        let deadline = Instant::now() + Duration::from_secs(30);
        while !entered.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the wakeup never ran");
            std::thread::sleep(Duration::from_millis(2));
        }
        drop(resources);
        assert_eq!(
            wakeups.load(Ordering::SeqCst),
            1,
            "the drop returned while the driver was still inside the wakeup"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::wants_refinement;

    #[test]
    fn refinement_triggers_on_a_bitmap_far_too_large_or_too_small() {
        // A 2048-wide initial decode drawn at 200px: twice too large.
        assert!(wants_refinement(
            (2048, 1536),
            (200, 150),
            (4000, 3000),
            2.0
        ));
        // Drawn at 1100px: under the ratio, kept as is.
        assert!(!wants_refinement(
            (2048, 1536),
            (1100, 825),
            (4000, 3000),
            2.0
        ));
        // Drawn larger than the resident bitmap while the image has more.
        assert!(wants_refinement((200, 150), (400, 300), (4000, 3000), 2.0));
        // Drawn larger than the resident bitmap, which is already everything.
        assert!(!wants_refinement((200, 150), (400, 300), (200, 150), 2.0));
        // Exactly the drawn size.
        assert!(!wants_refinement((200, 150), (200, 150), (4000, 3000), 2.0));
    }
}
