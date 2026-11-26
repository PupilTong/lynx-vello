//! Image identity and the seam the compose path reads pixels through.
//!
//! Nothing on the document's thread holds decoded pixels, and nothing this
//! crate publishes carries them either. The split is:
//!
//! - [`ImageRegistry`] lives on the document. It is a name table keyed by the raw source string the
//!   page wrote, holding each image's load state and its own intrinsic dimensions. No
//!   [`ImageData`], no [`Blob`](crate::peniko::Blob), no store — and no identity to mint, because
//!   nothing per-image is stored behind a name.
//! - [`FrameImages`] is the pixel seam. A committed frame's image draws name a source, and whoever
//!   composes that frame asks the store what each draws as.
//!
//! Producing those pixels is not this crate's business at all: the embedder's
//! resource system owns bytes, decoding, residency and eviction, and this
//! crate only ever names images and asks for them.
//!
//! # Nothing on the painter's thread waits for pixels
//!
//! Composition holds `&mut DisplayList` and an open clip stack; it can neither
//! suspend nor leave a layer unbalanced. So every question the painter asks a
//! store is one non-suspending call, and none of them may block: the painter
//! never decodes and never waits for a decode or for I/O.
//!
//! The painter asks what a source draws as ([`FrameImages::describe`]): the
//! bitmap's [`ImageDescriptor`], with its pixels when the store holds them. A
//! store that dropped the pixels answers the descriptor alone. When the
//! rasterizer holds no upload of that bitmap either, the frame **draws
//! nothing** for the source, and the painter tells the store the source is
//! wanted ([`FrameImages::want`]). The store restores it off the painter's
//! thread and wakes its host the way a finished load does; the painter asks
//! again at its next adoption, and the next frame it composes draws it.
//!
//! A picture one element owns ([`PictureKey::Id`]) is asked about the same
//! way, and answers a generation that changes exactly when its pixels do; its
//! frames are not lent but taken out of the slot its producer publishes into
//! ([`FrameImages::take_frame`], `render::picture`), and written into the
//! picture's own texture.
//!
//! What a store lends or hands over is a [`PictureFrame`]: CPU pixels, or a
//! frame of the embedder's own type ([`FrameImages::PlatformFrame`]) that
//! writes itself into a texture the painter created
//! ([`FrameWriter`](crate::render::picture::FrameWriter)). The painter writes
//! a lent platform frame at once and releases it, since nothing but its
//! upload can draw it.
//!
//! The document never learns of any of this. There is no way to report that
//! an image stopped being resident or came back, so a document's image state
//! can never regress from loaded back to pending, and its natural size,
//! layout and `load`/`error` events do not move.
//!
//! # One copy of a bitmap
//!
//! The rasterizer keys its upload of a bitmap on its descriptor alone, one
//! upload for every draw of a source whatever its sampler, so once it holds
//! that upload the painter tells the store ([`FrameImages::release`]) and the
//! store may drop its copy. A store that keeps every bitmap it answers is
//! never asked for one it dropped, and is correct with an empty
//! [`FrameImages::want`].
//!
//! # What the engine will not draw
//!
//! The rasterizer uploads a bitmap into a texture of its own, and no texture
//! can be longer than the device's `max_texture_dimension_2d`, which is
//! [`MAX_RENDERABLE_DIMENSION`] on every device this crate creates. Creating
//! a longer one is a wgpu validation error, not a draw that degrades.
//!
//! So [`is_renderable`] refuses such a bitmap before it reaches the
//! rasterizer. It is
//! tested against the pixels [`FrameImages::describe`] actually answers, and
//! never against an image's intrinsic size: **the intrinsic size is a CSS
//! input and is not bounded at all**. A host may report a 12000x6000 image
//! and decode it to 6000x3000; that lays out at its true size and ratio and
//! draws correctly, because an image draw carries its anchor and extent
//! unmultiplied and divides by the real bitmap dimensions at encode time.
//!
//! # The URL image service
//!
//! The document offers what it knows of a URL to whoever embeds it
//! ([`Document::request_image`](crate::tree::document::Document::request_image),
//! [`Document::image_status`](crate::tree::document::Document::image_status)
//! and the settled sources
//! [`Document::apply_image_events`](crate::tree::document::Document::apply_image_events)
//! answers): asked for once, then pending, loaded with its natural size as a
//! bitmap or as a vector document, or failed, and never moving again once
//! settled. Nothing here names an element: an element that presents an
//! image, and which of several it presents, is its embedder's, which puts
//! the picture on the replaced box itself
//! ([`Document::set_picture`](crate::tree::document::Document::set_picture)).
//!
//! # Vector images
//!
//! An SVG document is not pixels, and the engine draws it itself. The host
//! that fetched it parses it, on whatever thread it decodes bitmaps on,
//! with the engine's own parser, reached from outside this crate only
//! through [`ImageEvent::parse_document`]: the result is a
//! [`VectorDocument`], the parsed markup and its natural size, which the
//! host reports through [`ImageReports::parsed_document`] and which travels
//! to the document thread inside [`ImageEvent::ParsedDocument`]. A document
//! that does not parse is reported with [`ImageReports::failed`]. Applying a
//! parsed document builds it as an isolated document cascaded by stylo,
//! records it, shapes its text with the document's own `TextContext` and
//! paints the recording's plan into one display list, on the document
//! thread, into a [`VectorImage`] (`crate::svg::image`, the
//! function the document hands [`ImageRegistry::apply`]); the registry keeps
//! that in the loaded entry. A vector image is never an image
//! draw: [`FrameImages`] is never asked for it, and no bitmap budget applies.
//!
//! # Documents the page hands over
//!
//! A document need not be fetched. The Lynx `<svg content>` hands the
//! document its markup directly
//! ([`Document::insert_image_document`](crate::tree::document::Document::insert_image_document)),
//! and the registry files it under a *synthetic source*:
//! [`SYNTHETIC_SOURCE_PREFIX`] followed by the 32 hex digits of a 128-bit
//! hash of the bytes (SipHash-1-3 with the zero key, [`synthetic_source`]).
//! Identical markup is therefore one entry, one parse, one scene and one
//! raster texture however many elements draw it. An unknown synthetic source
//! is created `Pending` and its bytes queued as a *document request*
//! ([`ImageRegistry::take_document_requests`]), which the embedder hands to
//! the host with the bytes: the host parses them exactly as it parses a
//! fetched document and reports the result for that source. A known one
//! binds and settles at once. No walk and no bind ever asks the host to
//! fetch a synthetic source, and its entry is forgotten when the last
//! replaced box presenting it lets go ([`ImageRegistry::unbind`]): the one
//! removal the registry makes, a host source still never regressing.

use std::cell::{Cell, RefCell};
use std::convert::Infallible;
use std::rc::Rc;
use std::sync::Arc;

use bytes::Bytes;
use rustc_hash::FxHashMap;
use vant::image::DecodedImage;
use vant::image::residency::{
    Decoded, Described, Refinement, Requested, Residency, ResidencyConfig, ViewKey,
};

use crate::clock::ClockInstant;
use crate::paint::display_list::DisplayList;
use crate::peniko::ImageData;
use crate::render::gpu::SharedGpu;
use crate::render::picture::{
    FrameWriter, PictureFrame, PictureId, PictureKey, PlaybackReport, TakenFrame,
};
use crate::svg::image::VectorDocument;

#[derive(Clone)]
pub(crate) struct VectorImage {
    scene: Arc<DisplayList>,
    viewport: (f32, f32),
    aspect: AspectRatio,
    key: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum AspectAlign {
    Min,
    #[default]
    Mid,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AspectRatio {
    pub(crate) align: Option<(AspectAlign, AspectAlign)>,
    pub(crate) slice: bool,
}

impl Default for AspectRatio {
    fn default() -> Self {
        Self {
            align: Some((AspectAlign::Mid, AspectAlign::Mid)),
            slice: false,
        }
    }
}

static NEXT_VECTOR_KEY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<VectorDocument>();
    assert_send_sync::<DisplayList>();
    assert_send_sync::<VectorImage>();
};

impl VectorImage {
    #[must_use]
    pub(crate) fn new(scene: DisplayList, viewport: (f32, f32), aspect: AspectRatio) -> Self {
        Self {
            scene: Arc::new(scene),
            viewport,
            aspect,
            key: NEXT_VECTOR_KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    #[cfg(test)]
    pub(crate) fn parse_sealed(svg: &[u8]) -> Result<Self, crate::svg::image::SvgError> {
        let document = VectorDocument::parse(svg)?;
        Ok(crate::svg::image::vector_image(&document, &mut None))
    }

    #[must_use]
    pub(crate) fn key(&self) -> u64 {
        self.key
    }

    #[must_use]
    pub(crate) fn aspect(&self) -> AspectRatio {
        self.aspect
    }

    #[must_use]
    pub(crate) fn viewport(&self) -> (f32, f32) {
        self.viewport
    }

    #[must_use]
    pub(crate) fn scene(&self) -> &Arc<DisplayList> {
        &self.scene
    }
}

impl std::fmt::Debug for VectorImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VectorImage")
            .field("viewport", &self.viewport)
            .field("aspect", &self.aspect)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

pub const MAX_RENDERABLE_DIMENSION: u32 = SharedGpu::LIMITS.max_texture_dimension_2d;

#[must_use]
pub fn is_renderable(data: &ImageData) -> bool {
    renders_at(data.width, data.height)
}

pub(crate) const fn renders_at(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_RENDERABLE_DIMENSION
        && height <= MAX_RENDERABLE_DIMENSION
}

pub use vant::image::{ImageDescriptor, ImageSizeHint};

#[derive(Clone, Debug, PartialEq)]
pub struct FrameImage<P = Infallible> {
    pub descriptor: ImageDescriptor,
    pub pixels: Option<PictureFrame<P>>,
    pub generation: u64,
    pub next_change: Option<ClockInstant>,
}

impl<P> FrameImage<P> {
    #[must_use]
    pub fn resident(image: ImageData) -> Self {
        Self {
            descriptor: ImageDescriptor::of(&image),
            pixels: Some(PictureFrame::Pixels(image)),
            generation: 0,
            next_change: None,
        }
    }

    #[must_use]
    pub const fn held(frame: PictureFrame<P>, descriptor: ImageDescriptor) -> Self {
        Self {
            descriptor,
            pixels: Some(frame),
            generation: 0,
            next_change: None,
        }
    }

    #[must_use]
    pub const fn released(descriptor: ImageDescriptor) -> Self {
        Self {
            descriptor,
            pixels: None,
            generation: 0,
            next_change: None,
        }
    }
}

pub trait FrameImages {
    type PlatformFrame: FrameWriter;

    fn describe(
        &self,
        picture: &PictureKey,
        hint: ImageSizeHint,
    ) -> Option<FrameImage<Self::PlatformFrame>>;

    fn take_frame(&self, picture: PictureId) -> Option<TakenFrame<Self::PlatformFrame>> {
        let _ = picture;
        None
    }

    fn want(&self, picture: &PictureKey, hint: ImageSizeHint);

    fn retain(&self, drawn: &[PictureKey]);

    fn release(&self, pictures: &[PictureKey]) {
        let _ = pictures;
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoImages;

impl<T: FrameImages + ?Sized> FrameImages for Rc<T> {
    type PlatformFrame = T::PlatformFrame;

    fn describe(
        &self,
        picture: &PictureKey,
        hint: ImageSizeHint,
    ) -> Option<FrameImage<Self::PlatformFrame>> {
        (**self).describe(picture, hint)
    }

    fn take_frame(&self, picture: PictureId) -> Option<TakenFrame<Self::PlatformFrame>> {
        (**self).take_frame(picture)
    }

    fn want(&self, picture: &PictureKey, hint: ImageSizeHint) {
        (**self).want(picture, hint);
    }

    fn retain(&self, drawn: &[PictureKey]) {
        (**self).retain(drawn);
    }

    fn release(&self, pictures: &[PictureKey]) {
        (**self).release(pictures);
    }
}

impl FrameImages for NoImages {
    type PlatformFrame = Infallible;

    fn describe(&self, _picture: &PictureKey, _hint: ImageSizeHint) -> Option<FrameImage> {
        None
    }

    fn want(&self, _picture: &PictureKey, _hint: ImageSizeHint) {}

    fn retain(&self, _drawn: &[PictureKey]) {}
}

#[derive(Debug, Default)]
struct ImageQueue {
    events: RefCell<Vec<ImageEvent>>,
    detached: Cell<bool>,
}

#[derive(Clone, Debug)]
pub struct ImageReports {
    queue: Rc<ImageQueue>,
}

impl ImageReports {
    pub fn loaded(&self, source: &str, width: u32, height: u32) {
        self.post(ImageEvent::Loaded {
            source: Arc::from(source),
            width,
            height,
            animated: false,
        });
    }

    pub fn loaded_animated(&self, source: &str, width: u32, height: u32) {
        self.post(ImageEvent::Loaded {
            source: Arc::from(source),
            width,
            height,
            animated: true,
        });
    }

    pub fn playback(&self, picture: PictureId, report: PlaybackReport) {
        self.post(ImageEvent::Playback { picture, report });
    }

    pub fn parsed_document(&self, source: &str, document: Arc<VectorDocument>) {
        self.post(ImageEvent::ParsedDocument {
            source: Arc::from(source),
            document,
        });
    }

    pub fn failed(&self, source: &str) {
        self.post(ImageEvent::Failed {
            source: Arc::from(source),
        });
    }

    fn post(&self, event: ImageEvent) {
        if self.queue.detached.get() {
            return;
        }
        self.queue.events.borrow_mut().push(event);
    }
}

#[derive(Debug)]
pub struct ImageInbox {
    queue: Rc<ImageQueue>,
}

impl ImageInbox {
    #[must_use]
    pub fn new() -> (ImageReports, Self) {
        let queue = Rc::new(ImageQueue::default());
        (
            ImageReports {
                queue: Rc::clone(&queue),
            },
            Self { queue },
        )
    }

    #[must_use]
    pub fn drain(&self) -> Vec<ImageEvent> {
        std::mem::take(&mut *self.queue.events.borrow_mut())
    }

    pub fn detach(&self) {
        self.queue.detached.set(true);
    }
}

#[derive(Debug)]
pub struct ImageResidency<P = Infallible> {
    residency: Residency<ImageReports, P>,
}

impl<P: Clone> ImageResidency<P> {
    #[must_use]
    pub fn new(budget_bytes: usize, initial_bound: u32, downsample_ratio: f32) -> Self {
        Self {
            residency: Residency::new(ResidencyConfig {
                budget_bytes,
                initial_bound,
                max_renderable: MAX_RENDERABLE_DIMENSION,
                downsample_ratio,
            }),
        }
    }

    pub fn new_view(&mut self) -> ViewKey {
        self.residency.new_view()
    }

    pub fn request(&mut self, source: &str, reports: &ImageReports) -> Option<(u32, u32)> {
        match self.residency.request(source, reports.clone()) {
            Requested::Loaded {
                intrinsic,
                animated,
            } => {
                report_loaded(reports, source, intrinsic, animated);
                None
            }
            Requested::Failed => {
                reports.failed(source);
                None
            }
            Requested::Joined => None,
            Requested::Start { bound } => Some(bound),
        }
    }

    pub fn request_document(&mut self, source: Arc<str>, reports: &ImageReports) {
        self.residency.request_document(source, reports.clone());
    }

    pub fn complete_loaded(&mut self, source: &Arc<str>, decoded: Decoded<P>) {
        let animated = decoded.animated;
        if let Some((waiters, intrinsic)) = self.residency.loaded(source, decoded) {
            for reports in waiters {
                report_loaded(&reports, source, intrinsic, animated);
            }
        }
    }

    pub fn complete_parsed(&mut self, source: &str, document: &Arc<VectorDocument>) {
        if let Some(waiters) = self.residency.parsed(source) {
            for reports in waiters {
                reports.parsed_document(source, Arc::clone(document));
            }
        }
    }

    pub fn complete_requested_document(
        &mut self,
        source: &str,
        parsed: Result<Arc<VectorDocument>, String>,
    ) -> Option<String> {
        let reports = self.residency.requested_document_done(source)?;
        match parsed {
            Ok(document) => {
                reports.parsed_document(source, document);
                None
            }
            Err(message) => {
                reports.failed(source);
                Some(message)
            }
        }
    }

    pub fn complete_failed(&mut self, source: &str) -> bool {
        let Some(waiters) = self.residency.failed(source) else {
            return false;
        };
        for reports in waiters {
            reports.failed(source);
        }
        true
    }

    pub fn refined(
        &mut self,
        source: &Arc<str>,
        target: (u32, u32),
        bitmap: impl Into<DecodedImage<P>>,
    ) {
        self.residency.refined(source, target, bitmap);
    }

    pub fn refine_failed(&mut self, source: &str) {
        self.residency.refine_failed(source);
    }

    pub fn describe(&mut self, source: &str, hint: ImageSizeHint) -> Described<P> {
        self.residency.describe(source, hint)
    }

    pub fn release(&mut self, sources: &[Arc<str>]) {
        self.residency.release(sources);
    }

    pub fn want(&mut self, source: &str, hint: ImageSizeHint) -> Option<Refinement> {
        self.residency.want(source, hint)
    }

    pub fn retain(&mut self, view: ViewKey, frame: &[Arc<str>]) {
        self.residency.retain(view, frame);
    }

    pub fn release_view(&mut self, view: ViewKey) {
        self.residency.release_view(view);
    }

    #[must_use]
    pub fn budget(&self) -> usize {
        self.residency.budget()
    }

    pub fn set_budget(&mut self, budget: usize) {
        self.residency.set_budget(budget);
    }

    #[must_use]
    pub fn bitmap_bytes(&self) -> usize {
        self.residency.bitmap_bytes()
    }

    #[must_use]
    pub fn encoded_bytes(&self) -> usize {
        self.residency.encoded_bytes()
    }

    #[must_use]
    pub fn knows(&self, source: &str) -> bool {
        self.residency.knows(source)
    }

    #[must_use]
    pub fn is_resident(&self, source: &str) -> bool {
        self.residency.is_resident(source)
    }

    #[must_use]
    pub fn resident_size(&self, source: &str) -> Option<(u32, u32)> {
        self.residency.resident_size(source)
    }
}

fn report_loaded(reports: &ImageReports, source: &str, intrinsic: (u32, u32), animated: bool) {
    if animated {
        reports.loaded_animated(source, intrinsic.0, intrinsic.1);
    } else {
        reports.loaded(source, intrinsic.0, intrinsic.1);
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Svg,
}

#[derive(Clone, Debug)]
pub enum ImageEvent {
    Loaded {
        source: Arc<str>,
        width: u32,
        height: u32,
        animated: bool,
    },
    ParsedDocument {
        source: Arc<str>,
        document: Arc<VectorDocument>,
    },
    Failed {
        source: Arc<str>,
    },
    Playback {
        picture: PictureId,
        report: PlaybackReport,
    },
}

impl ImageEvent {
    #[must_use]
    pub fn parse_document(source: Arc<str>, bytes: &[u8], kind: DocumentKind) -> Self {
        let parsed = match kind {
            DocumentKind::Svg => VectorDocument::parse(bytes),
        };
        match parsed {
            Ok(document) => Self::ParsedDocument {
                source,
                document: Arc::new(document),
            },
            Err(_) => Self::Failed { source },
        }
    }

    pub(crate) fn source(&self) -> Option<&Arc<str>> {
        match self {
            Self::Loaded { source, .. }
            | Self::ParsedDocument { source, .. }
            | Self::Failed { source } => Some(source),
            Self::Playback { .. } => None,
        }
    }
}

#[derive(Debug, Default)]
enum ImageState {
    #[default]
    Pending,
    Ready {
        width: u32,
        height: u32,
        kind: ImageKind,
    },
    Failed,
}

impl ImageState {
    fn status(&self) -> ImageStatus {
        match self {
            Self::Pending => ImageStatus::Pending,
            Self::Ready {
                width,
                height,
                kind: ImageKind::Raster { animated },
            } => ImageStatus::Bitmap {
                width: *width,
                height: *height,
                animated: *animated,
            },
            Self::Ready {
                width,
                height,
                kind: ImageKind::Vector(_),
            } => ImageStatus::Vector {
                width: *width,
                height: *height,
            },
            Self::Failed => ImageStatus::Failed,
        }
    }
}

#[derive(Debug)]
enum ImageKind {
    Raster { animated: bool },
    Vector(VectorImage),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageStatus {
    Pending,
    Bitmap {
        width: u32,
        height: u32,
        animated: bool,
    },
    Vector {
        width: u32,
        height: u32,
    },
    Failed,
}

impl ImageStatus {
    #[must_use]
    pub const fn natural_size(self) -> Option<(u32, u32)> {
        match self {
            Self::Bitmap { width, height, .. } | Self::Vector { width, height } => {
                Some((width, height))
            }
            Self::Pending | Self::Failed => None,
        }
    }
}

pub(crate) type Resolved<'a> = (Arc<str>, (f64, f64), Option<&'a VectorImage>);

pub(crate) const SYNTHETIC_SOURCE_PREFIX: &str = "svg-content:";

pub(crate) fn is_synthetic_source(source: &str) -> bool {
    source.starts_with(SYNTHETIC_SOURCE_PREFIX)
}

pub(crate) fn synthetic_source(bytes: &[u8], kind: DocumentKind) -> String {
    let hash = siphasher::sip128::SipHasher13::new().hash(bytes).as_u128();
    match kind {
        DocumentKind::Svg => format!("{SYNTHETIC_SOURCE_PREFIX}{hash:032x}"),
    }
}

pub type DocumentRequest = (Arc<str>, Bytes, DocumentKind);

#[derive(Debug, Default)]
struct Entry {
    state: ImageState,
    presenters: u32,
}

#[derive(Default)]
pub(crate) struct ImageRegistry {
    entries: FxHashMap<Arc<str>, Entry>,
    wanted: RefCell<Vec<Arc<str>>>,
    document_requests: Vec<DocumentRequest>,
}

impl std::fmt::Debug for ImageRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageRegistry")
            .field("entries", &self.entries.len())
            .field("document_requests", &self.document_requests.len())
            .finish_non_exhaustive()
    }
}

impl ImageRegistry {
    pub(crate) fn resolve(&self, source: &str) -> Option<Resolved<'_>> {
        let (key, entry) = self.sight(source)?;
        match &entry.state {
            ImageState::Ready {
                width,
                height,
                kind,
            } => Some((
                Arc::clone(key),
                (f64::from(*width), f64::from(*height)),
                match kind {
                    ImageKind::Raster { .. } => None,
                    ImageKind::Vector(image) => Some(image),
                },
            )),
            ImageState::Pending | ImageState::Failed => None,
        }
    }

    fn sight(&self, source: &str) -> Option<(&Arc<str>, &Entry)> {
        let found = self.entries.get_key_value(source);
        if found.is_none() && !is_synthetic_source(source) {
            let mut wanted = self.wanted.borrow_mut();
            if !wanted.iter().any(|pending| &**pending == source) {
                wanted.push(Arc::from(source));
            }
        }
        found
    }

    pub(crate) fn request(&mut self, source: &str) -> (Arc<str>, ImageStatus) {
        if let Some((key, entry)) = self.entries.get_key_value(source) {
            return (Arc::clone(key), entry.state.status());
        }
        if is_synthetic_source(source) {
            return (Arc::from(source), ImageStatus::Pending);
        }
        let wanted = self.wanted.get_mut();
        let key = if let Some(pending) = wanted.iter().find(|pending| &***pending == source) {
            Arc::clone(pending)
        } else {
            let key = Arc::<str>::from(source);
            wanted.push(Arc::clone(&key));
            key
        };
        self.entries.insert(Arc::clone(&key), Entry::default());
        (key, ImageStatus::Pending)
    }

    pub(crate) fn status(&self, source: &str) -> ImageStatus {
        self.entries
            .get(source)
            .map_or(ImageStatus::Pending, |entry| entry.state.status())
    }

    pub(crate) fn bind(&mut self, source: &str) -> Arc<str> {
        let (key, _) = self.request(source);
        if let Some(entry) = self.entries.get_mut(source) {
            entry.presenters += 1;
        }
        key
    }

    pub(crate) fn unbind(&mut self, source: &str) {
        let Some(entry) = self.entries.get_mut(source) else {
            return;
        };
        entry.presenters = entry.presenters.saturating_sub(1);
        if entry.presenters == 0 && is_synthetic_source(source) {
            self.entries.remove(source);
            self.document_requests
                .retain(|(requested, ..)| &**requested != source);
        }
    }

    pub(crate) fn insert_document(&mut self, bytes: &[u8], kind: DocumentKind) -> Arc<str> {
        let source = synthetic_source(bytes, kind);
        if let Some((known, _)) = self.entries.get_key_value(source.as_str()) {
            return Arc::clone(known);
        }
        let source = Arc::<str>::from(source);
        self.entries.insert(Arc::clone(&source), Entry::default());
        self.document_requests
            .push((Arc::clone(&source), Bytes::copy_from_slice(bytes), kind));
        source
    }

    pub(crate) fn take_document_requests(&mut self) -> Vec<DocumentRequest> {
        std::mem::take(&mut self.document_requests)
    }

    pub(crate) fn take_wanted(&mut self) -> Vec<Arc<str>> {
        let wanted = std::mem::take(self.wanted.get_mut());
        for source in &wanted {
            self.entries.entry(Arc::clone(source)).or_default();
        }
        wanted
    }

    pub(crate) fn apply(
        &mut self,
        event: &ImageEvent,
        draw: impl FnOnce(&VectorDocument) -> VectorImage,
    ) -> Option<ImageStatus> {
        let source = event.source()?;
        if is_synthetic_source(source) && !self.entries.contains_key(&**source) {
            return None;
        }
        let entry = self.entry_for(source);
        if !matches!(entry.state, ImageState::Pending) {
            return None;
        }
        entry.state = match event {
            ImageEvent::Loaded {
                width,
                height,
                animated,
                ..
            } if *width > 0 && *height > 0 => ImageState::Ready {
                width: *width,
                height: *height,
                kind: ImageKind::Raster {
                    animated: *animated,
                },
            },
            ImageEvent::ParsedDocument { document, .. } => {
                let (width, height) = document.natural;
                ImageState::Ready {
                    width,
                    height,
                    kind: ImageKind::Vector(draw(document)),
                }
            }
            ImageEvent::Loaded { .. } | ImageEvent::Failed { .. } => ImageState::Failed,
            ImageEvent::Playback { .. } => unreachable!("a playback report names no source"),
        };
        Some(entry.state.status())
    }

    pub(crate) fn presenters(&self, source: &str) -> u32 {
        self.entries.get(source).map_or(0, |entry| entry.presenters)
    }

    pub(crate) fn vector_key(&self, source: &str) -> Option<u64> {
        match &self.entries.get(source)?.state {
            ImageState::Ready {
                kind: ImageKind::Vector(image),
                ..
            } => Some(image.key()),
            _ => None,
        }
    }

    pub(crate) fn replace_vector(&mut self, source: &str, image: VectorImage) {
        if let Some(Entry {
            state:
                ImageState::Ready {
                    kind: kind @ ImageKind::Vector(_),
                    ..
                },
            ..
        }) = self.entries.get_mut(source)
        {
            *kind = ImageKind::Vector(image);
        }
    }

    pub(crate) fn knows(&self, source: &str) -> bool {
        self.entries.contains_key(source)
    }

    fn entry_for(&mut self, source: &str) -> &mut Entry {
        if !self.entries.contains_key(source) {
            self.entries.insert(Arc::from(source), Entry::default());
        }
        self.entries
            .get_mut(source)
            .expect("the entry was just ensured")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn requesting_a_source_asks_the_host_for_it() {
        let mut registry = ImageRegistry::default();
        assert_eq!(registry.request("app:///a.png").1, ImageStatus::Pending);

        assert!(
            registry.resolve("app:///a.png").is_none(),
            "a bound source has no pixels yet"
        );
        assert_eq!(
            registry.take_wanted(),
            vec![Arc::<str>::from("app:///a.png")],
            "the request asks the host for the source"
        );
        assert!(registry.take_wanted().is_empty(), "and asks exactly once");
    }

    #[test]
    fn requesting_a_source_a_walk_already_met_asks_once() {
        let mut registry = ImageRegistry::default();
        assert!(registry.resolve("app:///a.png").is_none());
        let (name, _) = registry.request("app:///a.png");
        let bound = registry.bind("app:///a.png");

        assert!(Arc::ptr_eq(&name, &bound), "one name, one allocation");
        assert_eq!(registry.take_wanted().len(), 1, "one source, one request");
    }

    #[test]
    fn two_requests_of_one_source_ask_once() {
        let mut registry = ImageRegistry::default();
        let _ = registry.request("app:///a.png");
        let _ = registry.request("app:///a.png");

        assert_eq!(registry.take_wanted().len(), 1);
    }

    fn draw(document: &crate::svg::image::VectorDocument) -> super::VectorImage {
        crate::svg::image::vector_image(document, &mut None)
    }

    use super::{ImageInbox, ImageReports};

    #[test]
    fn reports_are_drained_once_in_the_order_they_were_made() {
        let (reports, inbox) = ImageInbox::new();

        reports.loaded("app:///a.png", 4, 4);
        reports.failed("app:///b.png");

        let drained = inbox.drain();
        assert!(
            matches!(
                drained.as_slice(),
                [
                    super::ImageEvent::Loaded {
                        source: loaded,
                        width: 4,
                        height: 4,
                        animated: false,
                    },
                    super::ImageEvent::Failed { source: failed },
                ] if &**loaded == "app:///a.png" && &**failed == "app:///b.png"
            ),
            "{drained:?}"
        );
        assert!(inbox.drain().is_empty(), "a drain empties the inbox");
    }

    #[test]
    fn a_detached_inbox_takes_nothing() {
        let (reports, inbox) = ImageInbox::new();

        inbox.detach();
        reports.loaded("app:///a.png", 4, 4);

        assert!(inbox.drain().is_empty());
    }

    const _: () = {
        const fn assert_not_send<T>() {}
        assert_not_send::<ImageReports>();
    };

    use std::sync::Arc;

    use super::{
        ImageEvent, ImageRegistry, ImageSizeHint, ImageStatus, MAX_RENDERABLE_DIMENSION, NoImages,
        is_renderable,
    };
    use crate::render::image::FrameImages;
    use crate::render::picture::PictureKey;

    fn loaded(source: &str, width: u32, height: u32) -> ImageEvent {
        ImageEvent::Loaded {
            source: Arc::from(source),
            width,
            height,
            animated: false,
        }
    }

    fn failed(source: &str) -> ImageEvent {
        ImageEvent::Failed {
            source: Arc::from(source),
        }
    }

    fn image(width: u32, height: u32) -> crate::peniko::ImageData {
        use crate::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
        ImageData {
            data: Blob::from(vec![0_u8; 4]),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        }
    }

    #[test]
    fn an_image_report_crosses_to_the_thread_that_owns_the_tree() {
        const fn assert_send<T: Send>() {}
        assert_send::<ImageEvent>();
    }

    #[test]
    fn a_source_is_asked_for_once_however_often_it_is_drawn() {
        let mut registry = ImageRegistry::default();
        for _ in 0..5 {
            assert!(registry.resolve("app:///a.png").is_none());
        }
        assert_eq!(
            registry.take_wanted(),
            vec![Arc::from("app:///a.png")],
            "however many draws named it, the host is asked once"
        );

        assert!(registry.resolve("app:///a.png").is_none(), "still pending");
        assert!(
            registry.take_wanted().is_empty(),
            "and a source already asked for is never asked again"
        );
    }

    #[test]
    fn only_a_loaded_image_resolves_and_it_carries_its_own_dimensions() {
        let mut registry = ImageRegistry::default();
        let _ = registry.resolve("app:///a.png");
        registry.take_wanted();
        assert!(registry.resolve("app:///a.png").is_none());

        registry.apply(&loaded("app:///a.png", 40, 20), draw);
        let (source, dimensions, _) = registry
            .resolve("app:///a.png")
            .expect("a loaded image resolves");
        assert_eq!(source.as_ref(), "app:///a.png");
        assert!((dimensions.0 - 40.0).abs() < f64::EPSILON);
        assert!((dimensions.1 - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn an_intrinsic_size_past_the_texture_bound_still_lays_out_and_draws() {
        let mut registry = ImageRegistry::default();
        let _ = registry.resolve("app:///huge.png");
        registry.take_wanted();
        registry.apply(&loaded("app:///huge.png", 12_000, 6_000), draw);

        let (_, dimensions, _) = registry
            .resolve("app:///huge.png")
            .expect("a huge image is drawable — it is the bitmap that is bounded, not this");
        assert!((dimensions.0 - 12_000.0).abs() < f64::EPSILON);

        assert!(is_renderable(&image(6_000, 3_000)));
        assert!(!is_renderable(&image(12_000, 6_000)));
    }

    #[test]
    fn the_texture_bound_is_inclusive_and_a_zero_axis_is_refused() {
        assert!(is_renderable(&image(1, 1)));
        assert!(is_renderable(&image(
            MAX_RENDERABLE_DIMENSION,
            MAX_RENDERABLE_DIMENSION
        )));
        assert!(!is_renderable(&image(MAX_RENDERABLE_DIMENSION + 1, 1)));
        assert!(!is_renderable(&image(1, MAX_RENDERABLE_DIMENSION + 1)));
        assert!(!is_renderable(&image(0, 4)));
        assert!(!is_renderable(&image(4, 0)));
    }

    #[test]
    fn a_zero_intrinsic_axis_is_a_load_failure() {
        for (width, height) in [(0, 4), (4, 0)] {
            let mut registry = ImageRegistry::default();
            let _ = registry.resolve("app:///bad.png");
            registry.take_wanted();
            registry.apply(&loaded("app:///bad.png", width, height), draw);
            assert!(
                registry.resolve("app:///bad.png").is_none(),
                "{width}x{height} has no usable intrinsic size"
            );
        }
    }

    #[test]
    fn a_loaded_image_never_regresses() {
        let mut registry = ImageRegistry::default();
        let _ = registry.resolve("app:///a.png");
        registry.take_wanted();
        assert!(
            registry
                .apply(&loaded("app:///a.png", 10, 10), draw)
                .is_some()
        );

        assert!(
            registry.apply(&failed("app:///a.png"), draw).is_none(),
            "a late failure on a loaded image moves nothing"
        );
        assert!(registry.resolve("app:///a.png").is_some(), "still drawable");

        assert!(
            registry
                .apply(&loaded("app:///a.png", 99, 99), draw)
                .is_none(),
            "and a repeated report moves nothing"
        );
        let (_, dimensions, _) = registry.resolve("app:///a.png").expect("still drawable");
        assert!(
            (dimensions.0 - 10.0).abs() < f64::EPSILON,
            "the first content is the content"
        );
    }

    #[test]
    fn a_failed_image_is_terminal() {
        let mut registry = ImageRegistry::default();
        let _ = registry.resolve("app:///a.png");
        registry.take_wanted();
        registry.apply(&failed("app:///a.png"), draw);
        assert!(registry.resolve("app:///a.png").is_none());
        assert!(
            registry
                .apply(&loaded("app:///a.png", 4, 4), draw)
                .is_none(),
            "a failure is terminal: pixels arriving later change nothing"
        );
        assert!(registry.resolve("app:///a.png").is_none());
    }

    #[test]
    fn a_report_answers_the_status_it_settled() {
        let mut registry = ImageRegistry::default();
        let _ = registry.request("app:///a.png");
        assert_eq!(
            registry.apply(&loaded("app:///a.png", 12, 6), draw),
            Some(ImageStatus::Bitmap {
                width: 12,
                height: 6,
                animated: false,
            })
        );
        assert_eq!(
            registry.apply(&failed("app:///b.png"), draw),
            Some(ImageStatus::Failed),
            "a failure settles too, requested or not"
        );
    }

    #[test]
    fn a_zero_axis_load_settles_as_a_failure() {
        let mut registry = ImageRegistry::default();
        let _ = registry.request("app:///bad.png");
        assert_eq!(
            registry.apply(&loaded("app:///bad.png", 0, 4), draw),
            Some(ImageStatus::Failed)
        );
        assert_eq!(registry.status("app:///bad.png"), ImageStatus::Failed);
    }

    #[test]
    fn a_settled_source_answers_a_later_request() {
        let mut registry = ImageRegistry::default();
        let _ = registry.request("app:///a.png");
        assert_eq!(registry.request("app:///a.png").1, ImageStatus::Pending);

        registry.apply(&loaded("app:///a.png", 12, 6), draw);
        registry.apply(&failed("app:///b.png"), draw);
        assert_eq!(
            registry.request("app:///a.png").1,
            ImageStatus::Bitmap {
                width: 12,
                height: 6,
                animated: false,
            }
        );
        assert_eq!(registry.request("app:///b.png").1, ImageStatus::Failed);
        let _ = registry.take_wanted();
        assert_eq!(
            registry.status("app:///never-seen.png"),
            ImageStatus::Pending,
            "a source nothing has asked for has settled on nothing"
        );
        assert!(
            registry.take_wanted().is_empty(),
            "and reading a status asks for nothing"
        );
    }

    #[test]
    fn a_status_names_the_natural_size_of_a_loaded_source_only() {
        assert_eq!(
            ImageStatus::Bitmap {
                width: 4,
                height: 2,
                animated: false,
            }
            .natural_size(),
            Some((4, 2))
        );
        assert_eq!(
            ImageStatus::Vector {
                width: 8,
                height: 4
            }
            .natural_size(),
            Some((8, 4))
        );
        assert_eq!(ImageStatus::Pending.natural_size(), None);
        assert_eq!(ImageStatus::Failed.natural_size(), None);
    }

    #[test]
    fn the_empty_pixel_source_draws_nothing() {
        assert!(
            NoImages
                .describe(
                    &PictureKey::Url(Arc::from("app:///a.png")),
                    ImageSizeHint::UNBOUNDED
                )
                .is_none()
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod vector_tests {

    use std::sync::Arc;

    use seldon::text::TextContext;

    use super::{DocumentKind, ImageEvent, ImageInbox, ImageRegistry, VectorImage};

    #[test]
    fn a_document_the_host_parsed_and_reported_settles_as_a_vector_image() {
        let ImageEvent::ParsedDocument { document, .. } = ImageEvent::parse_document(
            Arc::from("app:///icon.svg"),
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="6" height="4" fill="#dc2626"/></svg>"##,
            DocumentKind::Svg,
        ) else {
            panic!("the document parses");
        };
        let (reports, inbox) = ImageInbox::new();
        reports.parsed_document("app:///icon.svg", Arc::clone(&document));
        let drained = inbox.drain();
        let [
            ImageEvent::ParsedDocument {
                source,
                document: reported,
            },
        ] = drained.as_slice()
        else {
            panic!("one parsed document: {drained:?}");
        };
        assert_eq!(&**source, "app:///icon.svg");
        assert!(Arc::ptr_eq(reported, &document), "shared, not copied");

        let mut registry = ImageRegistry::default();
        let mut context = Some(Box::new(TextContext::without_system_fonts()));
        let applied = registry.apply(&drained[0], |document| {
            crate::svg::image::vector_image(document, &mut context)
        });
        assert_eq!(
            applied,
            Some(super::ImageStatus::Vector {
                width: 10,
                height: 10
            })
        );
        let (_, _, vector) = registry.resolve("app:///icon.svg").expect("loaded");
        let vector = vector.expect("a vector image");
        assert!(!vector.scene().is_empty());
        assert_eq!(vector.viewport(), (10.0, 10.0));
    }

    #[test]
    fn an_unparsable_document_fails_its_source() {
        let event = ImageEvent::parse_document(
            Arc::from("app:///bad.svg"),
            b"<svg xmlns='http://www.w3.org/2000/svg'><rect></svg>",
            DocumentKind::Svg,
        );
        assert!(matches!(event, ImageEvent::Failed { .. }));
        let mut registry = ImageRegistry::default();
        let mut context = Some(Box::new(TextContext::without_system_fonts()));
        assert_eq!(
            registry.apply(&event, |document| {
                crate::svg::image::vector_image(document, &mut context)
            }),
            Some(super::ImageStatus::Failed)
        );
        assert!(registry.resolve("app:///bad.svg").is_none());
    }

    #[test]
    fn a_markup_error_reports_failed() {
        for bytes in [
            &b"<svg \xff/>"[..],
            b"<html xmlns='http://www.w3.org/1999/xhtml'/>",
        ] {
            let event =
                ImageEvent::parse_document(Arc::from("app:///bad.svg"), bytes, DocumentKind::Svg);
            assert!(matches!(event, ImageEvent::Failed { .. }), "{bytes:?}");
        }
    }

    #[test]
    fn the_isolated_document_is_gone_after_apply() {
        fn sendable<T: Send + Sync>(_: &T) {}
        let event = ImageEvent::parse_document(
            Arc::from("app:///icon.svg"),
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4"/></svg>"#,
            DocumentKind::Svg,
        );
        let mut registry = ImageRegistry::default();
        registry.apply(&event, |document| {
            crate::svg::image::vector_image(document, &mut None)
        });
        let entry = registry.entries.get("app:///icon.svg").expect("filed");
        let super::ImageState::Ready {
            kind: super::ImageKind::Vector(image),
            ..
        } = &entry.state
        else {
            panic!("a settled vector image: {:?}", entry.state);
        };
        sendable(image);
        assert!(!image.scene().is_empty());
    }

    #[test]
    fn keys_are_unique_per_image_and_shared_by_clones() {
        let first = VectorImage::parse_sealed(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#,
        )
        .expect("parses");
        let second = VectorImage::parse_sealed(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#,
        )
        .expect("parses");
        assert_ne!(first.key(), second.key());
        assert_eq!(first.clone().key(), first.key());
        assert!(format!("{first:?}").starts_with("VectorImage"));
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod synthetic_tests {

    use std::sync::Arc;

    use seldon::text::TextContext;

    use super::{
        DocumentKind, ImageEvent, ImageRegistry, ImageStatus, SYNTHETIC_SOURCE_PREFIX,
        is_synthetic_source, synthetic_source,
    };

    const ICON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4"/>"#;
    const OTHER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="6" height="6"/>"#;

    #[test]
    fn the_source_names_the_markup() {
        let source = synthetic_source(ICON, DocumentKind::Svg);
        let digits = source
            .strip_prefix(SYNTHETIC_SOURCE_PREFIX)
            .expect("the prefix");
        assert_eq!(digits.len(), 32, "{source}");
        assert!(
            digits
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{source}"
        );
        assert!(is_synthetic_source(&source));
        assert_eq!(source, synthetic_source(ICON, DocumentKind::Svg));
        assert_ne!(source, synthetic_source(OTHER, DocumentKind::Svg));
        assert_ne!(
            source,
            synthetic_source(&ICON[..ICON.len() - 1], DocumentKind::Svg),
            "a prefix of the markup is another document"
        );
    }

    #[test]
    fn identical_markup_is_one_entry_and_one_parse() {
        let mut registry = ImageRegistry::default();
        let first = registry.insert_document(ICON, DocumentKind::Svg);
        registry.bind(&first);
        let second = registry.insert_document(ICON, DocumentKind::Svg);
        registry.bind(&second);

        assert!(Arc::ptr_eq(&first, &second), "one name, one allocation");
        let requests = registry.take_document_requests();
        assert_eq!(requests.len(), 1, "one parse");
        let (source, bytes, kind) = requests.into_iter().next().expect("one document");
        assert!(Arc::ptr_eq(&source, &first));
        assert_eq!(&*bytes, ICON);
        assert_eq!(kind, DocumentKind::Svg);

        let mut context = Some(Box::new(TextContext::without_system_fonts()));
        assert_eq!(
            registry.apply(
                &ImageEvent::parse_document(source, &bytes, kind),
                |document| { crate::svg::image::vector_image(document, &mut context) }
            ),
            Some(ImageStatus::Vector {
                width: 8,
                height: 4
            })
        );

        let third = registry.insert_document(ICON, DocumentKind::Svg);
        registry.bind(&third);
        assert_eq!(registry.take_document_requests(), []);
        assert!(registry.status(&third).natural_size().is_some());
    }

    #[test]
    fn document_requests_drain_once() {
        let mut registry = ImageRegistry::default();
        let icon = registry.insert_document(ICON, DocumentKind::Svg);
        registry.bind(&icon);
        assert_eq!(registry.take_document_requests().len(), 1);
        assert_eq!(registry.take_document_requests(), []);

        let other = registry.insert_document(OTHER, DocumentKind::Svg);
        registry.bind(&other);
        let requests = registry.take_document_requests();
        assert_eq!(requests.len(), 1);
        assert!(Arc::ptr_eq(&requests[0].0, &other));
    }

    #[test]
    fn a_synthetic_source_is_never_asked_of_the_host() {
        let mut registry = ImageRegistry::default();
        let icon = registry.insert_document(ICON, DocumentKind::Svg);
        registry.bind(&icon);
        assert!(registry.resolve(&icon).is_none(), "pending");
        assert!(
            registry
                .resolve(&synthetic_source(OTHER, DocumentKind::Svg))
                .is_none()
        );
        assert_eq!(registry.take_wanted(), []);
    }

    #[test]
    fn a_synthetic_source_named_by_a_page_files_nothing() {
        let mut registry = ImageRegistry::default();
        let named = synthetic_source(ICON, DocumentKind::Svg);
        assert_eq!(registry.request(&named).1, ImageStatus::Pending);
        registry.bind(&named);
        assert!(!registry.knows(&named));
        assert_eq!(registry.take_wanted(), []);

        let icon = registry.insert_document(ICON, DocumentKind::Svg);
        assert_eq!(&*icon, named);
        assert_eq!(registry.take_document_requests().len(), 1, "still parsed");
    }

    #[test]
    fn the_last_unbind_forgets_a_synthetic_source() {
        let mut registry = ImageRegistry::default();
        let icon = registry.insert_document(ICON, DocumentKind::Svg);
        registry.bind(&icon);
        registry.bind(&icon);

        registry.unbind(&icon);
        assert!(registry.knows(&icon), "one binder is left");
        registry.unbind(&icon);
        assert!(!registry.knows(&icon), "forgotten with the last binder");
        assert!(
            registry.take_document_requests().is_empty(),
            "and its bytes are never parsed"
        );

        registry.bind("app:///a.svg");
        registry.unbind("app:///a.svg");
        assert!(registry.knows("app:///a.svg"), "a host source never goes");
    }

    #[test]
    fn a_parse_for_a_forgotten_source_moves_nothing() {
        let mut registry = ImageRegistry::default();
        let icon = registry.insert_document(ICON, DocumentKind::Svg);
        registry.bind(&icon);
        let (source, bytes, kind) = registry
            .take_document_requests()
            .pop()
            .expect("one document");
        registry.unbind(&icon);

        let mut context = Some(Box::new(TextContext::without_system_fonts()));
        let event = ImageEvent::parse_document(source, &bytes, kind);
        assert!(
            registry
                .apply(&event, |document| {
                    crate::svg::image::vector_image(document, &mut context)
                })
                .is_none()
        );
        assert!(!registry.knows(&icon));

        let again = registry.insert_document(ICON, DocumentKind::Svg);
        registry.bind(&again);
        assert_eq!(registry.take_document_requests().len(), 1);
        assert!(
            registry
                .apply(&event, |document| {
                    crate::svg::image::vector_image(document, &mut context)
                })
                .is_some(),
            "the new entry is pending"
        );
        assert!(
            registry
                .apply(&event, |document| {
                    crate::svg::image::vector_image(document, &mut context)
                })
                .is_none(),
            "a second report for the settled source moves nothing"
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod residency_tests {

    use std::sync::Arc;

    use vant::image::Bitmap;
    use vant::image::residency::Decoded;
    use vant::mime::ImageFormat;

    use super::{
        DocumentKind, ImageEvent, ImageInbox, ImageResidency, MAX_RENDERABLE_DIMENSION,
        VectorDocument,
    };

    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4"/>"#;

    fn residency() -> ImageResidency {
        ImageResidency::new(1 << 20, 2048, 2.0)
    }

    fn decoded(intrinsic: (u32, u32)) -> Decoded {
        Decoded {
            url: url::Url::parse("app:///a.png").expect("url"),
            bitmap: Bitmap {
                width: 1,
                height: 1,
                source_width: intrinsic.0,
                source_height: intrinsic.1,
                premultiplied: false,
                rgba: vec![0; 4],
            }
            .into(),
            format: ImageFormat::Png,
            header: None,
            encoded: None,
            animated: false,
        }
    }

    fn document() -> Arc<VectorDocument> {
        match ImageEvent::parse_document(Arc::from("icon.svg"), SVG, DocumentKind::Svg) {
            ImageEvent::ParsedDocument { document, .. } => document,
            other => panic!("not a document: {other:?}"),
        }
    }

    fn loaded(events: &[ImageEvent]) -> Vec<(&str, u32, u32)> {
        events
            .iter()
            .map(|event| match event {
                ImageEvent::Loaded {
                    source,
                    width,
                    height,
                    ..
                } => (&**source, *width, *height),
                other => panic!("not a load: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_load_reports_once_to_every_view_that_asked() {
        let mut residency = residency();
        let (first, first_inbox) = ImageInbox::new();
        let (second, second_inbox) = ImageInbox::new();
        assert_eq!(residency.request("a.png", &first), Some((2048, 2048)));
        assert_eq!(residency.request("a.png", &second), None, "joined");
        assert!(first_inbox.drain().is_empty(), "nothing to report yet");

        let source = Arc::from("a.png");
        residency.complete_loaded(&source, decoded((40, 20)));
        assert_eq!(loaded(&first_inbox.drain()), [("a.png", 40, 20)]);
        assert_eq!(loaded(&second_inbox.drain()), [("a.png", 40, 20)]);

        residency.complete_loaded(&source, decoded((1, 1)));
        assert!(
            first_inbox.drain().is_empty(),
            "a stray completion reports nothing"
        );

        let (third, third_inbox) = ImageInbox::new();
        assert_eq!(residency.request("a.png", &third), None);
        assert_eq!(
            loaded(&third_inbox.drain()),
            [("a.png", 40, 20)],
            "a loaded source reports at once"
        );
        assert!(first_inbox.drain().is_empty());
    }

    #[test]
    fn an_animated_load_says_so_to_every_view_and_the_status() {
        let mut residency = residency();
        let (first, first_inbox) = ImageInbox::new();
        residency.request("a.png", &first);
        residency.complete_loaded(
            &Arc::from("a.png"),
            Decoded {
                animated: true,
                ..decoded((40, 20))
            },
        );
        let (second, second_inbox) = ImageInbox::new();
        residency.request("a.png", &second);
        for inbox in [&first_inbox, &second_inbox] {
            let events = inbox.drain();
            assert!(
                matches!(
                    events.as_slice(),
                    [ImageEvent::Loaded { animated: true, .. }]
                ),
                "{events:?}"
            );
        }

        let mut registry = super::ImageRegistry::default();
        let _ = registry.request("a.png");
        let picture = crate::render::picture::PictureId::new();
        let playback = ImageEvent::Playback {
            picture,
            report: crate::render::picture::PlaybackReport::LoopComplete,
        };
        assert_eq!(
            registry.apply(&playback, |document| crate::svg::image::vector_image(
                document, &mut None
            )),
            None
        );
        let (reports, inbox) = ImageInbox::new();
        reports.loaded_animated("a.png", 40, 20);
        for event in inbox.drain() {
            assert_eq!(
                registry.apply(&event, |document| crate::svg::image::vector_image(
                    document, &mut None
                )),
                Some(super::ImageStatus::Bitmap {
                    width: 40,
                    height: 20,
                    animated: true,
                })
            );
        }
        assert_eq!(registry.status("a.png").natural_size(), Some((40, 20)));
    }

    #[test]
    fn the_first_decode_is_bounded_by_the_engine() {
        let mut residency: ImageResidency = ImageResidency::new(0, u32::MAX, 2.0);
        let (reports, _inbox) = ImageInbox::new();
        assert_eq!(
            residency.request("a.png", &reports),
            Some((MAX_RENDERABLE_DIMENSION, MAX_RENDERABLE_DIMENSION))
        );
    }

    #[test]
    fn a_failure_reports_to_every_view_and_then_at_once() {
        let mut residency = residency();
        let (first, first_inbox) = ImageInbox::new();
        let (second, second_inbox) = ImageInbox::new();
        residency.request("a.png", &first);
        residency.request("a.png", &second);
        assert!(residency.complete_failed("a.png"));
        for inbox in [&first_inbox, &second_inbox] {
            assert!(matches!(
                inbox.drain().as_slice(),
                [ImageEvent::Failed { source }] if &**source == "a.png"
            ));
        }
        assert!(
            !residency.complete_failed("a.png"),
            "no load in flight: nothing reported, nothing to note"
        );
        assert!(first_inbox.drain().is_empty());
        assert_eq!(residency.request("a.png", &first), None);
        assert!(matches!(
            first_inbox.drain().as_slice(),
            [ImageEvent::Failed { .. }]
        ));
    }

    #[test]
    fn a_fetched_document_reports_its_parse_to_every_view_that_asked() {
        let mut residency = residency();
        let (first, first_inbox) = ImageInbox::new();
        let (second, second_inbox) = ImageInbox::new();
        residency.request("icon.svg", &first);
        residency.request("icon.svg", &second);
        residency.complete_parsed("icon.svg", &document());
        for inbox in [&first_inbox, &second_inbox] {
            assert!(matches!(
                inbox.drain().as_slice(),
                [ImageEvent::ParsedDocument { source, .. }] if &**source == "icon.svg"
            ));
        }
        residency.complete_parsed("icon.svg", &document());
        assert!(first_inbox.drain().is_empty());
        assert_eq!(
            residency.request("icon.svg", &first),
            Some((2048, 2048)),
            "a parsed document is fetched again"
        );
    }

    #[test]
    fn a_document_request_reports_to_the_view_that_asked_once() {
        let mut residency = residency();
        let (first, first_inbox) = ImageInbox::new();
        let (second, second_inbox) = ImageInbox::new();
        residency.request_document(Arc::from("svg-content:a"), &first);
        residency.request_document(Arc::from("svg-content:b"), &second);

        assert_eq!(
            residency.complete_requested_document("svg-content:a", Ok(document())),
            None
        );
        assert!(matches!(
            first_inbox.drain().as_slice(),
            [ImageEvent::ParsedDocument { source, .. }] if &**source == "svg-content:a"
        ));
        assert!(second_inbox.drain().is_empty());

        assert_eq!(
            residency.complete_requested_document("svg-content:b", Err("broken".to_owned())),
            Some("broken".to_owned()),
            "the failure comes back for the host's note"
        );
        assert!(matches!(
            second_inbox.drain().as_slice(),
            [ImageEvent::Failed { source }] if &**source == "svg-content:b"
        ));
        assert_eq!(
            residency.complete_requested_document("svg-content:b", Err("again".to_owned())),
            None,
            "no request left: nothing reported, nothing to note"
        );
        assert!(second_inbox.drain().is_empty());
    }
}
