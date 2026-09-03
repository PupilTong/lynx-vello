//! The image pipeline: what happens between the painter naming a source and
//! the frame reading its pixels.
//!
//! A request resolves the source, then a job — a worker thread natively, a
//! local task in the browser — fetches the bytes, runs them through the
//! preprocessing pipeline, and decodes them with the platform decoder at a
//! bounded size. Its completion lands in a channel the painter drains in
//! its next turn, which is when the load is reported and the bitmap enters
//! the memory tier. From then on the frame reads the source with the size
//! it draws it at, and three things can happen:
//!
//! - the resident bitmap is about the right size, and is returned;
//! - the resident bitmap is far larger (or smaller, with more to give) than the draw, and is
//!   returned while a refinement decode at the drawn size runs in the background and replaces it —
//!   the downsampling the size hint exists for;
//! - the bitmap was evicted, and is restored synchronously from the encoded bytes — kept in memory
//!   when nothing else holds them, read back from the disk tier or the file when something does —
//!   because a read after a reported load must not miss.
//!
//! The document's state never regresses, and neither does this one: an
//! entry that failed stays failed, and one that loaded stays loaded whatever
//! the memory tier holds for it.

use std::sync::Arc;

use bobcat_core::vello::peniko::ImageData;
use bobcat_core::{ImageReports, ImageSizeHint, MAX_RENDERABLE_DIMENSION};
use bytes::Bytes;
use rustc_hash::FxHashMap;
use url::Url;

use crate::cache::memory::MemoryCache;
use crate::decode::{Bitmap, DecodeError};
use crate::image_header::ImageHeader;
use crate::mime::ImageFormat;
use crate::preprocess::{self, Payload};
use crate::{Resources, Shared, SharedHandle};

/// What a load job hands back: the bitmap, what the bytes were, and the
/// bytes themselves when nothing else could restore them.
type LoadedImage = (Bitmap, ImageFormat, Option<ImageHeader>, Option<Bytes>);

/// The painter-thread half: what is known about every source asked for.
pub(crate) struct ImageState {
    entries: FxHashMap<Arc<str>, Entry>,
    bitmaps: MemoryCache<ImageData>,
    completions: flume::Receiver<Completion>,
}

enum Entry {
    /// Asked for; the job has not completed. Every view that asked is told
    /// when it does.
    Loading {
        waiters: Vec<ImageReports>,
    },
    Loaded(Loaded),
    Failed,
}

struct Loaded {
    url: Url,
    intrinsic: (u32, u32),
    /// The container, for a decoder that wants to be told what it is given.
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
    pub(crate) fn new(budget: usize, completions: flume::Receiver<Completion>) -> Self {
        Self {
            entries: FxHashMap::default(),
            bitmaps: MemoryCache::new(budget),
            completions,
        }
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
                _ => None,
            })
            .sum()
    }

    pub(crate) fn set_budget(&mut self, budget: usize) {
        self.bitmaps.set_budget(budget);
    }

    /// Whether `source` has been asked for at all.
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
/// knows it, otherwise starts the load and remembers who asked.
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
        None => {}
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
    spawn_load(&resources.shared, source, url, (bound, bound));
}

/// Applies every completion queued since the last turn.
pub(crate) fn service(resources: &Resources) {
    let completions: Vec<Completion> = {
        let state = resources.local.borrow();
        state.completions.try_iter().collect()
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
    let Some(Entry::Loaded(loaded)) = state.entries.get_mut(source) else {
        return None;
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
                &resources.shared,
                Arc::from(source),
                loaded.url.clone(),
                loaded.encoded.clone(),
                loaded.format,
                loaded.header,
                target,
            );
        }
        return Some(image);
    }
    // Evicted: restore now. The contract is that a reported load never
    // misses, and blocking here is the accepted price.
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

/// The job behind a request: fetch, preprocess, decode at the bound.
#[cfg(not(target_arch = "wasm32"))]
fn load(
    shared: &Shared,
    source: &Arc<str>,
    url: &Url,
    fetched: Result<crate::transport::Fetched, crate::error::Failure>,
    bound: (u32, u32),
) -> Result<LoadedImage, String> {
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
    let bitmap = shared
        .decode_bytes(&preprocessed.bytes, format, header, bound)
        .map_err(|error| error.to_string())?;
    let encoded = (!restorable).then_some(preprocessed.bytes);
    Ok((bitmap, format, header, encoded))
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_load(shared: &SharedHandle, source: Arc<str>, url: Url, bound: (u32, u32)) {
    let shared = SharedHandle::clone(shared);
    shared.clone().executor.run(move || {
        let fetched = shared.transports.fetch_blocking(
            &url,
            bobcat_core::resource::CachePolicy::Default,
            &http::HeaderMap::new(),
        );
        let completion = match load(&shared, &source, &url, fetched, bound) {
            Ok((bitmap, format, header, encoded)) => Completion::Loaded {
                source,
                url,
                bitmap,
                format,
                header,
                encoded,
            },
            Err(message) => Completion::Failed { source, message },
        };
        shared.complete(completion);
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn_load(shared: &SharedHandle, source: Arc<str>, url: Url, bound: (u32, u32)) {
    let shared = SharedHandle::clone(shared);
    wasm_bindgen_futures::spawn_local(async move {
        let fetched = shared
            .transports
            .fetch(
                &url,
                bobcat_core::resource::CachePolicy::Default,
                &http::HeaderMap::new(),
            )
            .await;
        let completion = match load_async(&shared, &source, &url, fetched, bound).await {
            Ok((bitmap, format, header, encoded)) => Completion::Loaded {
                source,
                url,
                bitmap,
                format,
                header,
                encoded,
            },
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
) -> Result<LoadedImage, String> {
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
    let bitmap = shared
        .decode_bytes_async(&preprocessed.bytes, format, header, bound)
        .await
        .map_err(|error| error.to_string())?;
    let encoded = (!restorable).then_some(preprocessed.bytes);
    Ok((bitmap, format, header, encoded))
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_refine(
    shared: &SharedHandle,
    source: Arc<str>,
    url: Url,
    encoded: Option<Bytes>,
    format: ImageFormat,
    header: Option<ImageHeader>,
    target: (u32, u32),
) {
    let shared = SharedHandle::clone(shared);
    shared.clone().executor.run(move || {
        let bytes = match encoded {
            Some(bytes) => Ok(bytes),
            None => restore_bytes(&shared, &url),
        };
        let completion = match bytes.and_then(|bytes| {
            shared
                .decode_bytes(&bytes, format, header, target)
                .map_err(|error| error.to_string())
        }) {
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
fn spawn_refine(
    shared: &SharedHandle,
    source: Arc<str>,
    url: Url,
    encoded: Option<Bytes>,
    format: ImageFormat,
    header: Option<ImageHeader>,
    target: (u32, u32),
) {
    let shared = SharedHandle::clone(shared);
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

/// Encoded bytes for an image whose bytes were not kept: the disk tier or
/// the file natively, only what is already in hand in the browser.
fn restore_bytes(shared: &Shared, url: &Url) -> Result<Bytes, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        shared
            .transports
            .fetch_blocking(
                url,
                bobcat_core::resource::CachePolicy::ForceCache,
                &http::HeaderMap::new(),
            )
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

fn decode_blocking(
    shared: &Shared,
    bytes: &[u8],
    format: ImageFormat,
    header: Option<ImageHeader>,
    target: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    shared.decode_bytes(bytes, format, header, target)
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
