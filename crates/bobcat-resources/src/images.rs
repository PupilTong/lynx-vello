//! The image pipeline's IO: the jobs that fetch, preprocess, decode and
//! parse, and the completions they feed back.
//!
//! What is known about every source, who waits for it, the decoded-bitmap
//! tier and its pins, and when a frame's describe refines or a want restores
//! are not decided here: they are [`vant::image::residency::Residency`]'s,
//! held as the engine's [`ImageResidency`], which also sends every report.
//! This module asks it what a request, a describe or a want means and does
//! the IO it is told to do.
//!
//! A request resolves the source, then a job — one task on the fetcher's own
//! runtime natively, a local task in the browser — fetches the bytes and runs
//! them through the preprocessing pipeline on a blocking thread, takes a
//! decode permit, and decodes them with the platform decoder at a bounded
//! size on a blocking thread. Its completion lands in a channel the painter
//! drains in its next turn, which is when the load is reported and the bitmap
//! enters the memory tier. From then on the frame describes the source with
//! the size it draws it at, and three things can happen:
//!
//! - the resident bitmap is about the right size, and is returned;
//! - the resident bitmap is far larger (or smaller, with more to give) than the draw, and is
//!   returned while a refinement job at the drawn size runs off the painter's thread and replaces
//!   it — the downsampling the size hint exists for;
//! - the bitmap was released or evicted, and its descriptor is returned without pixels.
//!
//! In the last case a painter whose rasterizer holds no upload of the bitmap
//! draws nothing for it and says it is wanted ([`want`]). The restore is a
//! refinement job like any other — off the painter's thread, under a decode
//! permit, from the encoded bytes kept in memory when nothing else holds
//! them, read back from the byte tier or the file when something does — and
//! its completion lands in the same channel and wakes the host the same way
//! a load's does. Nothing on the painter's thread decodes or waits for I/O,
//! and the document is never told: the source stays loaded throughout.
//!
//! # Animated images
//!
//! A load of a container that may animate also asks the decoder what its
//! frames are, in the same blocking closure under the same permit. One with
//! more than one frame keeps its encoded bytes and gets a playhead
//! ([`crate::animation`]); from then on a describe of it answers the frame
//! its playhead is on, and the frames ahead of it, its timer and its
//! restores are jobs started here like any other.
//!
//! # SVG documents
//!
//! An SVG document is parsed here, where a bitmap would be decoded, with the
//! engine's own parser ([`ImageEvent::parse_document`]). When preprocessing
//! says the bytes are [`ImageFormat::Svg`], the job hands them to the parser
//! instead of the platform decoder: natively under a decode permit, in a
//! closure of its own on the blocking pool; in the browser inside the load's
//! local task on the Render Worker, whose executor has no permits. The
//! browser's `createImageBitmap` never sees an SVG, so every target parses
//! the same bytes the same way.
//!
//! A document the page wrote itself (the Lynx `<svg content>`) arrives as a
//! document request instead ([`request_document`]): its bytes and kind under
//! a synthetic source the engine named, with nothing to fetch, parsed by the
//! same job half, natively under a permit of its own.
//!
//! Either is reported through [`ImageReports::parsed_document`], or through
//! [`ImageReports::failed`] when it does not parse. **Documents are parsed
//! and reported, never retained** (the residency's module docs say what that
//! means for its entries): nothing enters the memory tier or
//! [`Resources::memory_used_bytes`],
//! [`FrameImages::describe`](bobcat_core::FrameImages::describe) answers
//! `None` (the engine draws the document itself and never asks), and no
//! refinement and no restore can start for it.

use std::sync::Arc;
use std::time::Duration;

use bobcat_core::{
    ClockInstant, DocumentKind, FrameImage, ImageEvent, ImageReports, ImageResidency,
    ImageSizeHint, PictureId, PlaybackRequest, VectorDocument,
};
use bytes::Bytes;
use url::Url;
use vant::Platform;
use vant::image::DecodedImage;
use vant::image::animation::ImageFrames;
use vant::image::residency::{Decoded, Described, Refinement, ViewKey};

use crate::animation::{Animations, FrameDecode, Job};
use crate::image_header::ImageHeader;
use crate::mime::ImageFormat;
use crate::preprocess::{self, Payload};
use crate::{FrameOf, Resources, Shared, SharedHandle};

type LoadedImage<F> = (
    DecodedImage<F>,
    ImageFormat,
    Option<ImageHeader>,
    Option<Bytes>,
    Option<ImageFrames>,
);

enum LoadOutcome<F> {
    Raster(LoadedImage<F>),
    Vector(Arc<VectorDocument>),
}

impl<F> LoadOutcome<F> {
    fn completion(self, source: Arc<str>, url: Url) -> Completion<F> {
        match self {
            Self::Raster((bitmap, format, header, encoded, frames)) => Completion::Loaded {
                source,
                url,
                bitmap,
                format,
                header,
                encoded,
                frames,
            },
            Self::Vector(document) => Completion::ParsedDocument { source, document },
        }
    }
}

fn parse(
    source: &Arc<str>,
    bytes: &[u8],
    kind: DocumentKind,
) -> Result<Arc<VectorDocument>, String> {
    match ImageEvent::parse_document(Arc::clone(source), bytes, kind) {
        ImageEvent::ParsedDocument { document, .. } => Ok(document),
        ImageEvent::Loaded { .. } | ImageEvent::Failed { .. } | ImageEvent::Playback { .. } => {
            Err(format!("`{source}` is not a document the engine can draw"))
        }
    }
}

pub(crate) struct ImageState<F> {
    pub(crate) residency: ImageResidency<F>,
    pub(crate) animations: Animations<F>,
    completions: tokio::sync::mpsc::UnboundedReceiver<Completion<F>>,
}

pub(crate) enum Completion<F> {
    Loaded {
        source: Arc<str>,
        url: Url,
        bitmap: DecodedImage<F>,
        format: ImageFormat,
        header: Option<ImageHeader>,
        encoded: Option<Bytes>,
        frames: Option<ImageFrames>,
    },
    ParsedDocument {
        source: Arc<str>,
        document: Arc<VectorDocument>,
    },
    RequestedDocument {
        source: Arc<str>,
        parsed: Result<Arc<VectorDocument>, String>,
    },
    Refined {
        source: Arc<str>,
        target: (u32, u32),
        bitmap: DecodedImage<F>,
    },
    Failed {
        source: Arc<str>,
        message: String,
    },
    RefineFailed {
        source: Arc<str>,
        message: String,
    },
    Frame {
        source: Arc<str>,
        index: usize,
        bitmap: DecodedImage<F>,
    },
    FrameFailed {
        source: Arc<str>,
        index: usize,
        message: String,
    },
    Due {
        source: Arc<str>,
        due: ClockInstant,
    },
    MediaSource {
        picture: PictureId,
        source: Result<vant::media::MediaSource, String>,
    },
}

impl<F: Clone> ImageState<F> {
    pub(crate) fn new(
        residency: ImageResidency<F>,
        origin: ClockInstant,
        completions: tokio::sync::mpsc::UnboundedReceiver<Completion<F>>,
    ) -> Self {
        Self {
            residency,
            animations: Animations::new(origin),
            completions,
        }
    }
}

pub(crate) fn request<P: Platform>(resources: &Resources<P>, source: &str, reports: &ImageReports) {
    let mut state = resources.local.borrow_mut();
    let Some(bound) = state.residency.request(source, reports) else {
        return;
    };
    let base = resources.base_url();
    let url = match resources.shared.transports.resolve(source, base.as_ref()) {
        Ok(url) => url,
        Err(failure) => {
            state.residency.complete_failed(source);
            drop(state);
            resources.note(format!("image `{source}` cannot be resolved: {failure}"));
            return;
        }
    };
    drop(state);
    spawn_load(resources, Arc::from(source), url, bound);
}

pub(crate) fn request_document<P: Platform>(
    resources: &Resources<P>,
    source: &str,
    bytes: Bytes,
    kind: DocumentKind,
    reports: &ImageReports,
) {
    let source: Arc<str> = Arc::from(source);
    resources
        .local
        .borrow_mut()
        .residency
        .request_document(Arc::clone(&source), reports);
    spawn_parse(resources, source, bytes, kind);
}

pub(crate) fn service<P: Platform>(resources: &Resources<P>) {
    let completions: Vec<Completion<FrameOf<P>>> = {
        let mut state = resources.local.borrow_mut();
        let mut drained = Vec::new();
        while let Ok(completion) = state.completions.try_recv() {
            drained.push(completion);
        }
        drained
    };
    let any = !completions.is_empty();
    for completion in completions {
        apply(resources, completion);
    }
    if any {
        let mut jobs = Vec::new();
        {
            let mut state = resources.local.borrow_mut();
            let ImageState {
                residency,
                animations,
                ..
            } = &mut *state;
            animations.plan(residency, resources.now(), &mut jobs);
        }
        start(resources, jobs);
    }
}

fn start<P: Platform>(resources: &Resources<P>, jobs: Vec<Job>) {
    for job in jobs {
        match job {
            Job::Frame { source, decode } => spawn_frame(resources, source, decode),
            Job::Refine { source, refinement } => spawn_refine(resources, source, refinement),
            Job::Timer { source, due, wait } => spawn_timer(resources, source, due, wait),
        }
    }
}

fn apply<P: Platform>(resources: &Resources<P>, completion: Completion<FrameOf<P>>) {
    let mut state = resources.local.borrow_mut();
    match completion {
        Completion::Loaded {
            source,
            url,
            bitmap,
            format,
            header,
            encoded,
            frames,
        } => {
            let animation = frames
                .zip(encoded.clone())
                .map(|(frames, bytes)| (frames, bytes, bitmap.size()));
            state.residency.complete_loaded(
                &source,
                Decoded {
                    url,
                    bitmap,
                    format,
                    header,
                    encoded,
                    animated: animation.is_some(),
                },
            );
            if let Some((frames, bytes, size)) = animation {
                state
                    .animations
                    .insert(source, &frames, bytes, format, header, size);
            }
        }
        Completion::ParsedDocument { source, document } => {
            state.residency.complete_parsed(&source, &document);
        }
        Completion::RequestedDocument { source, parsed } => {
            if let Some(message) = state.residency.complete_requested_document(&source, parsed) {
                drop(state);
                resources.note(format!("image `{source}` failed to load: {message}"));
            }
        }
        Completion::Refined {
            source,
            target,
            bitmap,
        } => state.residency.refined(&source, target, bitmap),
        Completion::Failed { source, message } => {
            if state.residency.complete_failed(&source) {
                drop(state);
                resources.note(format!("image `{source}` failed to load: {message}"));
            }
        }
        Completion::RefineFailed { source, message } => {
            state.residency.refine_failed(&source);
            drop(state);
            resources.note(format!(
                "image `{source}` could not be re-decoded: {message}"
            ));
        }
        Completion::Frame {
            source,
            index,
            bitmap,
        } => state.animations.decoded(&source, index, bitmap),
        Completion::FrameFailed {
            source,
            index,
            message,
        } => {
            let bytes = state
                .animations
                .decode_failed(&source, index, resources.now());
            drop(state);
            forget_frames(resources, bytes);
            resources.note(format!(
                "frame {index} of image `{source}` could not be decoded: {message}"
            ));
        }
        Completion::Due { source, due } => state.animations.fired(&source, due),
        Completion::MediaSource { picture, source } => {
            drop(state);
            resources
                .media
                .borrow_mut()
                .fetched(&resources.shared.media, picture, source);
        }
    }
}

pub(crate) fn retain<P: Platform>(
    resources: &Resources<P>,
    view: ViewKey,
    frame: &[Arc<str>],
    pictures: &[PictureId],
) {
    let mut state = resources.local.borrow_mut();
    if pictures.is_empty() || state.animations.is_empty() {
        state.residency.retain(view, frame);
    } else {
        let mut pinned = frame.to_vec();
        pinned.extend(state.animations.sources_of(pictures));
        state.residency.retain(view, &pinned);
    }
    if !state.animations.is_empty() {
        let stopped = state
            .animations
            .retain(view, frame, pictures, resources.now());
        drop(state);
        forget_frames(resources, stopped);
    }
}

pub(crate) fn describe_picture<P: Platform>(
    resources: &Resources<P>,
    picture: PictureId,
    hint: ImageSizeHint,
) -> Option<FrameImage<FrameOf<P>>> {
    let mut jobs = Vec::new();
    let (image, wake) = {
        let mut state = resources.local.borrow_mut();
        let ImageState {
            residency,
            animations,
            ..
        } = &mut *state;
        animations.describe_picture(residency, picture, hint, resources.now(), &mut jobs)
    };
    start(resources, jobs);
    if wake {
        resources.wake();
    }
    image
}

pub(crate) fn want_picture<P: Platform>(
    resources: &Resources<P>,
    picture: PictureId,
    hint: ImageSizeHint,
) {
    let mut jobs = Vec::new();
    {
        let mut state = resources.local.borrow_mut();
        let ImageState {
            residency,
            animations,
            ..
        } = &mut *state;
        animations.want_picture(residency, picture, hint, resources.now(), &mut jobs);
    }
    start(resources, jobs);
}

pub(crate) fn take_frame<P: Platform>(
    resources: &Resources<P>,
    picture: PictureId,
) -> Option<bobcat_core::TakenFrame<FrameOf<P>>> {
    resources.local.borrow().animations.take_frame(picture)
}

pub(crate) fn playback<P: Platform>(
    resources: &Resources<P>,
    view: ViewKey,
    reports: &ImageReports,
    request: PlaybackRequest,
) {
    match &request {
        PlaybackRequest::Open { .. } | PlaybackRequest::Media { .. } => {
            crate::media::request(resources, view, reports, request);
            return;
        }
        PlaybackRequest::End { picture } if resources.media.borrow().contains(*picture) => {
            crate::media::request(resources, view, reports, request);
            return;
        }
        PlaybackRequest::Play { .. }
        | PlaybackRequest::Control { .. }
        | PlaybackRequest::End { .. } => {}
    }
    let mut jobs = Vec::new();
    let mut stopped = None;
    let wake = {
        let mut state = resources.local.borrow_mut();
        let ImageState {
            residency,
            animations,
            ..
        } = &mut *state;
        match request {
            PlaybackRequest::Play {
                picture,
                source,
                policy,
            } => animations.play(
                residency,
                picture,
                &source,
                policy,
                reports.clone(),
                view,
                resources.now(),
                &mut jobs,
            ),
            PlaybackRequest::Control { picture, control } => {
                animations.control(picture, control, resources.now())
            }
            PlaybackRequest::End { picture } => {
                stopped = animations.end(picture);
                false
            }
            PlaybackRequest::Open { .. } | PlaybackRequest::Media { .. } => false,
        }
    };
    start(resources, jobs);
    forget_frames(resources, stopped);
    if wake {
        resources.wake();
    }
}

pub(crate) fn forget_frames<P: Platform>(
    resources: &Resources<P>,
    stopped: impl IntoIterator<Item = Bytes>,
) {
    for bytes in stopped {
        vant::ImageDecoder::forget_frames(&*resources.shared.decoder, &bytes);
    }
}

pub(crate) fn describe<P: Platform>(
    resources: &Resources<P>,
    source: &str,
    hint: ImageSizeHint,
) -> Option<FrameImage<FrameOf<P>>> {
    let mut state = resources.local.borrow_mut();
    if state.animations.contains(source) {
        let source: Arc<str> = Arc::from(source);
        let source = &source;
        let mut jobs = Vec::new();
        let ImageState {
            residency,
            animations,
            ..
        } = &mut *state;
        let image = animations.describe(residency, source, hint, resources.now(), &mut jobs);
        drop(state);
        start(resources, jobs);
        return image;
    }
    let described = state.residency.describe(source, hint);
    drop(state);
    let (image, refine) = match described {
        Described::Unavailable => return None,
        Described::Resident {
            image,
            descriptor,
            refine,
        } => (FrameImage::held(image, descriptor), refine),
        Described::Absent { descriptor, refine } => (FrameImage::released(descriptor), refine),
    };
    if let Some(refinement) = refine {
        spawn_refine(resources, Arc::from(source), refinement);
    }
    Some(image)
}

pub(crate) fn release<P: Platform>(resources: &Resources<P>, sources: &[Arc<str>]) {
    let mut state = resources.local.borrow_mut();
    let sources = state.animations.release(sources);
    state.residency.release(&sources);
}

pub(crate) fn want<P: Platform>(resources: &Resources<P>, source: &str, hint: ImageSizeHint) {
    let source: Arc<str> = Arc::from(source);
    let source = &source;
    let mut jobs = Vec::new();
    let restore = {
        let mut state = resources.local.borrow_mut();
        let ImageState {
            residency,
            animations,
            ..
        } = &mut *state;
        if animations.want(residency, source, hint, resources.now(), &mut jobs) {
            None
        } else {
            residency.want(source, hint)
        }
    };
    if let Some(restore) = restore {
        spawn_refine(resources, Arc::clone(source), restore);
    }
    start(resources, jobs);
}

#[cfg(not(target_arch = "wasm32"))]
enum Prepared {
    Raster {
        format: ImageFormat,
        header: Option<ImageHeader>,
        bytes: Bytes,
        restorable: bool,
    },
    Document {
        bytes: Bytes,
        kind: DocumentKind,
    },
}

#[cfg(not(target_arch = "wasm32"))]
fn prepare<P: Platform>(shared: &Shared<P>, source: &str, url: &Url) -> Result<Prepared, String> {
    let fetched = shared
        .fetch_job(
            url,
            crate::CachePolicy::Default,
            &http::HeaderMap::new(),
            crate::file_body::Body::Read,
        )
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

#[cfg(not(target_arch = "wasm32"))]
async fn load<P: Platform>(
    shared: &SharedHandle<P>,
    handle: &tokio::runtime::Handle,
    permits: &Arc<tokio::sync::Semaphore>,
    source: &Arc<str>,
    url: &Url,
    bound: (u32, u32),
) -> Result<LoadOutcome<FrameOf<P>>, String> {
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
    let (bitmap, frames) = {
        let shared = SharedHandle::clone(shared);
        let bytes = encoded.clone();
        crate::executor::blocking(handle, "decode", move || {
            let _permit = permit;
            let bitmap = shared.decode_job(&bytes, format, header, bound)?;
            Ok::<_, crate::decode::DecodeError>((bitmap, shared.frames_job(&bytes, format, header)))
        })
        .await?
        .map_err(|error| error.to_string())?
    };
    let encoded = (!restorable || frames.is_some()).then_some(encoded);
    Ok(LoadOutcome::Raster((
        bitmap.into(),
        format,
        header,
        encoded,
        frames,
    )))
}

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
fn spawn_load<P: Platform>(
    resources: &Resources<P>,
    source: Arc<str>,
    url: Url,
    bound: (u32, u32),
) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    let permits = resources.executor.decode_permits();
    resources.executor.spawn(async move {
        let completion = match load(&shared, &handle, &permits, &source, &url, bound).await {
            Ok(outcome) => outcome.completion(source, url),
            Err(message) => Completion::Failed { source, message },
        };
        shared.complete(completion);
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_parse<P: Platform>(
    resources: &Resources<P>,
    source: Arc<str>,
    bytes: Bytes,
    kind: DocumentKind,
) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    let permits = resources.executor.decode_permits();
    resources.executor.spawn(async move {
        let parsed = parse_job(&handle, &permits, &source, bytes, kind).await;
        shared.complete(Completion::RequestedDocument { source, parsed });
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn_parse<P: Platform>(
    resources: &Resources<P>,
    source: Arc<str>,
    bytes: Bytes,
    kind: DocumentKind,
) {
    let shared = SharedHandle::clone(&resources.shared);
    wasm_bindgen_futures::spawn_local(async move {
        let parsed = parse(&source, &bytes, kind);
        shared.complete(Completion::RequestedDocument { source, parsed });
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn_load<P: Platform>(
    resources: &Resources<P>,
    source: Arc<str>,
    url: Url,
    bound: (u32, u32),
) {
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
async fn load_async<P: Platform>(
    shared: &Shared<P>,
    source: &Arc<str>,
    url: &Url,
    fetched: Result<crate::transport::Fetched, vant::error::Failure>,
    bound: (u32, u32),
) -> Result<LoadOutcome<FrameOf<P>>, String> {
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
    if format == ImageFormat::Svg {
        return parse(source, &preprocessed.bytes, DocumentKind::Svg).map(LoadOutcome::Vector);
    }
    let bitmap = shared
        .decode_bytes_async(preprocessed.bytes.clone(), format, header, bound)
        .await
        .map_err(|error| error.to_string())?;
    let frames = shared
        .frames_async(preprocessed.bytes.clone(), format, header)
        .await;
    let encoded = (!restorable || frames.is_some()).then_some(preprocessed.bytes);
    Ok(LoadOutcome::Raster((
        bitmap, format, header, encoded, frames,
    )))
}

#[cfg(not(target_arch = "wasm32"))]
async fn refine<P: Platform>(
    shared: &SharedHandle<P>,
    handle: &tokio::runtime::Handle,
    permits: &Arc<tokio::sync::Semaphore>,
    refinement: Refinement,
) -> Result<DecodedImage<FrameOf<P>>, String> {
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
    .map(DecodedImage::from)
    .map_err(|error| error.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_refine<P: Platform>(resources: &Resources<P>, source: Arc<str>, refinement: Refinement) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    let permits = resources.executor.decode_permits();
    let target = refinement.target;
    resources.executor.spawn(async move {
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
fn spawn_refine<P: Platform>(resources: &Resources<P>, source: Arc<str>, refinement: Refinement) {
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
                .decode_bytes_async(bytes, format, header, target)
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

#[cfg(not(target_arch = "wasm32"))]
fn spawn_frame<P: Platform>(resources: &Resources<P>, source: Arc<str>, decode: FrameDecode) {
    let shared = SharedHandle::clone(&resources.shared);
    let handle = resources.executor.handle();
    let permits = resources.executor.decode_permits();
    let index = decode.index;
    resources.executor.spawn(async move {
        let decoded = match acquire_decode(&permits).await {
            Ok(permit) => {
                let shared = SharedHandle::clone(&shared);
                crate::executor::blocking(&handle, "frame decode", move || {
                    let _permit = permit;
                    shared.decode_frame_job(&decode)
                })
                .await
                .and_then(|decoded| decoded.map_err(|error| error.to_string()))
            }
            Err(message) => Err(message),
        };
        shared.complete(match decoded {
            Ok(bitmap) => Completion::Frame {
                source,
                index,
                bitmap: bitmap.into(),
            },
            Err(message) => Completion::FrameFailed {
                source,
                index,
                message,
            },
        });
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn_frame<P: Platform>(resources: &Resources<P>, source: Arc<str>, decode: FrameDecode) {
    let shared = SharedHandle::clone(&resources.shared);
    let index = decode.index;
    wasm_bindgen_futures::spawn_local(async move {
        let decoded = vant::ImageDecoder::decode_frame(
            &*shared.decoder,
            decode.bytes,
            decode.format,
            decode.header,
            decode.index,
            decode.max,
        )
        .await;
        shared.complete(match decoded {
            Ok(bitmap) => Completion::Frame {
                source,
                index,
                bitmap,
            },
            Err(error) => Completion::FrameFailed {
                source,
                index,
                message: error.to_string(),
            },
        });
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_timer<P: Platform>(
    resources: &Resources<P>,
    source: Arc<str>,
    due: ClockInstant,
    wait: Duration,
) {
    let shared = SharedHandle::clone(&resources.shared);
    resources.executor.spawn(async move {
        tokio::time::sleep(wait).await;
        shared.complete(Completion::Due { source, due });
    });
}

#[cfg(target_arch = "wasm32")]
fn spawn_timer<P: Platform>(
    resources: &Resources<P>,
    source: Arc<str>,
    due: ClockInstant,
    wait: Duration,
) {
    let shared = SharedHandle::clone(&resources.shared);
    wasm_bindgen_futures::spawn_local(async move {
        crate::executor::sleep(wait).await;
        shared.complete(Completion::Due { source, due });
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn restore_for_job<P: Platform>(shared: &Shared<P>, url: &Url) -> Result<Bytes, String> {
    shared
        .fetch_job(
            url,
            crate::CachePolicy::ForceCache,
            &http::HeaderMap::new(),
            crate::file_body::Body::Read,
        )
        .map(|fetched| fetched.bytes)
        .map_err(|failure| failure.to_string())
}

#[cfg(target_arch = "wasm32")]
fn restore_bytes<P: Platform>(shared: &Shared<P>, url: &Url) -> Result<Bytes, String> {
    shared
        .transports
        .local(url)
        .unwrap_or_else(|| {
            Err(vant::error::Failure::new(
                bobcat_core::resource::ResourceErrorKind::Unavailable,
                bobcat_core::resource::ResourceErrorPhase::Open,
                "the bytes were not retained and cannot be fetched again",
            ))
        })
        .map(|fetched| fetched.bytes)
        .map_err(|failure| failure.to_string())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod job_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use bobcat_core::resource::ResourceFetcher;
    use bobcat_core::{DocumentKind, FrameImages, ImageEvent, ImageInbox, ImageSizeHint};

    use crate::decode::Bitmap;
    use crate::{Resources, ResourcesConfig};

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
    fn a_panicking_refinement_keeps_the_loaded_entry_drawable() {
        let resources = Resources::new(
            quiet(ResourcesConfig {
                worker_threads: 2,
                initial_decode_bound: 2048,
                downsample_ratio: 2.0,
                ..ResourcesConfig::default()
            }),
            || {},
        );
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

        assert!(
            super::describe(&resources, "app:///big.png", ImageSizeHint::new(125, 125))
                .is_some_and(|image| image.pixels.is_some()),
            "the describe answers with what is resident"
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

    fn load(resources: &Resources, source: &str) {
        let (reports, inbox) = ImageInbox::new();
        resources
            .register(source, tiny_png(), None)
            .expect("register");
        super::request(resources, source, &reports);
        settle(resources, "the load never reported", || {
            !inbox.drain().is_empty()
        });
    }

    #[test]
    fn a_wanted_source_is_restored_once_off_the_calling_thread_and_wakes_the_host() {
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
        let caller = std::thread::current().id();
        let decoded_on = Arc::new(std::sync::Mutex::new(Vec::new()));
        let open = Arc::new(AtomicBool::new(true));
        resources.shared.set_decode_hook(Arc::new({
            let decoded_on = Arc::clone(&decoded_on);
            let open = Arc::clone(&open);
            move |_bytes, _format, _header, _max| {
                decoded_on
                    .lock()
                    .expect("decode log")
                    .push(std::thread::current().id());
                while !open.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok(pixel(1, 1, (1, 1)))
            }
        }));
        let (reports, inbox) = ImageInbox::new();
        let view = resources.for_view(reports);
        load(&resources, "app:///a.png");
        let source = bobcat_core::PictureKey::Url(Arc::from("app:///a.png"));
        view.release(std::slice::from_ref(&source));
        let hint = ImageSizeHint::new(1, 1);
        assert!(
            view.describe(&source, hint)
                .is_some_and(|image| image.pixels.is_none()),
            "a released bitmap is described without its pixels"
        );

        open.store(false, Ordering::SeqCst);
        let wakeups_before = wakeups.load(Ordering::SeqCst);
        for _ in 0..3 {
            view.want(&source, hint);
            super::service(&resources);
            assert!(
                view.describe(&source, hint)
                    .is_some_and(|image| image.pixels.is_none()),
                "nothing is drawn from until the restore lands"
            );
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while decoded_on.lock().expect("decode log").len() < 2 {
            assert!(Instant::now() < deadline, "the restore never decoded");
            std::thread::sleep(Duration::from_millis(2));
        }
        open.store(true, Ordering::SeqCst);
        settle(&resources, "the restore never landed", || {
            view.describe(&source, hint)
                .is_some_and(|image| image.pixels.is_some())
        });

        let decoded_on = decoded_on.lock().expect("decode log").clone();
        assert_eq!(decoded_on.len(), 2, "the load and one restore");
        assert!(
            decoded_on.iter().all(|thread| *thread != caller),
            "no decode ran on the thread that wanted the source"
        );
        assert!(
            wakeups.load(Ordering::SeqCst) > wakeups_before,
            "the restore woke the host"
        );
        assert!(
            inbox.drain().is_empty(),
            "the document is told nothing of a restore"
        );
    }

    #[test]
    fn a_failed_restore_leaves_the_source_with_nothing_to_draw() {
        let resources = Resources::new(
            quiet(ResourcesConfig {
                worker_threads: 2,
                ..ResourcesConfig::default()
            }),
            || {},
        );
        let calls = Arc::new(AtomicUsize::new(0));
        resources.shared.set_decode_hook(Arc::new({
            let calls = Arc::clone(&calls);
            move |_bytes, _format, _header, _max| {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(pixel(1, 1, (1, 1)))
                } else {
                    Err(crate::decode::DecodeError::Malformed("gone".to_owned()))
                }
            }
        }));
        let (reports, inbox) = ImageInbox::new();
        let view = resources.for_view(reports);
        load(&resources, "app:///a.png");
        let source = bobcat_core::PictureKey::Url(Arc::from("app:///a.png"));
        view.release(std::slice::from_ref(&source));
        let hint = ImageSizeHint::new(1, 1);
        view.want(&source, hint);
        settle(&resources, "the restore never failed", || {
            view.describe(&source, hint).is_none()
        });
        view.want(&source, hint);
        super::service(&resources);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "nothing retries it");
        assert!(
            resources.knows_image("app:///a.png"),
            "the entry stays loaded"
        );
        assert!(inbox.drain().is_empty());
    }

    #[test]
    fn two_views_of_one_scope_keep_both_working_sets_pinned() {
        let resources = Resources::new(
            quiet(ResourcesConfig {
                worker_threads: 2,
                ..ResourcesConfig::default()
            }),
            || {},
        );
        resources
            .shared
            .set_decode_hook(Arc::new(|_bytes, _format, _header, _max| {
                Ok(pixel(1, 1, (1, 1)))
            }));
        let (first_reports, first_inbox) = ImageInbox::new();
        let (second_reports, second_inbox) = ImageInbox::new();
        let first = resources.for_view(first_reports);
        let second = resources.for_view(second_reports);
        for (source, view) in [("app:///a.png", &first), ("app:///b.png", &second)] {
            resources
                .register(source, tiny_png(), None)
                .expect("register");
            view.request_image(source);
        }
        let mut seen = 0;
        settle(&resources, "the two images never reported", || {
            seen += first_inbox.drain().len() + second_inbox.drain().len();
            seen >= 2
        });

        first.retain(&[bobcat_core::PictureKey::Url(Arc::from("app:///a.png"))]);
        second.retain(&[bobcat_core::PictureKey::Url(Arc::from("app:///b.png"))]);
        resources.set_memory_budget_bytes(0);
        assert!(
            resources.is_resident("app:///a.png"),
            "the second view's frame does not unpin the first view's"
        );
        assert!(resources.is_resident("app:///b.png"));

        drop(first);
        assert!(
            !resources.is_resident("app:///a.png"),
            "a dropped view's working set stops counting"
        );
        assert!(resources.is_resident("app:///b.png"));
    }

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
