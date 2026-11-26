//! An in-memory image source for tests and benchmarks: a [`terminus::link::FrameImages`]
//! that answers from whatever a test published into it.
//!
//! Production stores fetch, decode, cache and evict; this one does none of
//! that. It holds exactly the pixels a test put in it, so a render either
//! draws them or proves it looked the image up under the wrong source.
//!
//! It reports every load inline through the [`terminus::link::ImageReports`] a view
//! handed it, so a test needs no async runtime at all. What it does honour is
//! the identity contract: every [`FrameImages::describe`] of one source
//! answers a clone sharing the same `Blob`, which is what the rasterizer
//! keys its bitmap uploads on.
//!
//! An SVG document is parsed here, the way a production host parses one:
//! with the engine's own parser ([`ImageEvent::parse_document`]), and
//! reported as the parsed document, or as a failure when it does not parse.
//! A document is published through [`TestImages::insert_svg`], or handed
//! over by the page as markup, which reaches the store as a document request
//! ([`TestImages::request_document`], driven by [`pump_images`]). A
//! document request keeps nothing, as a production host keeps nothing:
//! every one is parsed and reported on its own. [`FrameImages::describe`]
//! never answers for either, because the engine never asks.
//!
//! By default the store keeps every bitmap it answers: a painter's
//! [`FrameImages::release`] is logged and changes nothing, and every
//! describe answers the pixels. A store built with [`TestImages::releasing`]
//! behaves as a production host does instead: a release drops the pixels — a
//! copy of the bytes stays behind so a later [`FrameImages::want`] can
//! restore them, as a host restores from its encoded bytes, into a new blob
//! — and a describe answers the bitmap's descriptor alone until then. The
//! restore is made inside the want, since this store has nothing to decode,
//! so the painter's next describe finds the pixels.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use terminus::link::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use terminus::link::{
    Document, DocumentKind, FrameImage, FrameImages, ImageDescriptor, ImageEvent, ImageReports,
    ImageSizeHint, NodeId, PictureKey, VectorDocument,
};

#[derive(Default)]
enum Entry {
    #[default]
    Pending,
    Ready(ImageData),
    Released {
        descriptor: ImageDescriptor,
        bytes: Vec<u8>,
    },
    Svg(Arc<VectorDocument>),
    Failed,
}

impl Entry {
    fn parsed(source: &str, bytes: &[u8], kind: DocumentKind) -> Self {
        match ImageEvent::parse_document(Arc::from(source), bytes, kind) {
            ImageEvent::ParsedDocument { document, .. } => Entry::Svg(document),
            ImageEvent::Loaded { .. } | ImageEvent::Failed { .. } | ImageEvent::Playback { .. } => {
                Entry::Failed
            }
        }
    }

    fn event(&self, source: &str) -> Option<ImageEvent> {
        let source = Arc::from(source);
        match self {
            Entry::Pending => None,
            Entry::Ready(image) => Some(ImageEvent::Loaded {
                source,
                width: image.width,
                height: image.height,
                animated: false,
            }),
            Entry::Released { descriptor, .. } => Some(ImageEvent::Loaded {
                source,
                width: descriptor.width,
                height: descriptor.height,
                animated: false,
            }),
            Entry::Svg(document) => Some(ImageEvent::ParsedDocument {
                source,
                document: Arc::clone(document),
            }),
            Entry::Failed => Some(ImageEvent::Failed { source }),
        }
    }
}

#[derive(Default)]
pub struct TestImages {
    entries: Mutex<HashMap<String, Entry>>,
    sink: RefCell<Option<ImageReports>>,
    retained: Mutex<Vec<Vec<Arc<str>>>>,
    pending: Mutex<Vec<ImageEvent>>,
    reads: Mutex<Vec<(String, ImageSizeHint)>>,
    releasing: Cell<bool>,
    described: Mutex<Vec<String>>,
    released: Mutex<Vec<Vec<Arc<str>>>>,
}

impl std::fmt::Debug for TestImages {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TestImages")
            .field("entries", &self.entries.lock().map(|map| map.len()).ok())
            .finish_non_exhaustive()
    }
}

impl TestImages {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn releasing() -> Self {
        let store = Self::default();
        store.releasing.set(true);
        store
    }

    #[must_use]
    pub fn described(&self) -> Vec<String> {
        self.described
            .lock()
            .expect("test image describe log")
            .clone()
    }

    #[must_use]
    pub fn released(&self) -> Vec<Vec<Arc<str>>> {
        self.released
            .lock()
            .expect("test image release log")
            .clone()
    }

    #[must_use]
    pub fn holds_pixels(&self, source: &str) -> bool {
        matches!(self.entries().get(source), Some(Entry::Ready(_)))
    }

    pub fn insert(&self, source: impl Into<String>, image: ImageData) {
        self.publish(source.into(), Entry::Ready(image));
    }

    pub fn insert_rgba8(
        &self,
        source: impl Into<String>,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) {
        assert_eq!(
            pixels.len(),
            width as usize * height as usize * 4,
            "an RGBA8 buffer must be width * height * 4 bytes"
        );
        self.insert(source, rgba8(width, height, pixels));
    }

    pub fn insert_svg(&self, source: impl Into<String>, svg: &str) {
        let source = source.into();
        let entry = Entry::parsed(&source, svg.as_bytes(), DocumentKind::Svg);
        self.publish(source, entry);
    }

    pub fn fail(&self, source: impl Into<String>) {
        self.publish(source.into(), Entry::Failed);
    }

    pub fn remove(&self, source: &str) {
        if let Some(entry) = self.entries().get_mut(source) {
            *entry = Entry::Pending;
        }
    }

    pub fn attach(&self, sink: ImageReports) {
        *self.sink.borrow_mut() = Some(sink);
        for (source, entry) in self.entries().iter() {
            self.report(source, entry);
        }
    }

    #[must_use]
    pub fn retained(&self) -> Vec<Vec<Arc<str>>> {
        self.retained.lock().expect("test image retain log").clone()
    }

    #[must_use]
    pub fn reads(&self) -> Vec<(String, ImageSizeHint)> {
        self.reads.lock().expect("test image read log").clone()
    }

    #[must_use]
    pub fn was_asked_for(&self, source: &str) -> bool {
        self.entries().contains_key(source)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries()
            .values()
            .filter(|entry| {
                matches!(
                    entry,
                    Entry::Ready(_) | Entry::Released { .. } | Entry::Svg(_)
                )
            })
            .count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().expect("test image store")
    }

    fn publish(&self, source: String, entry: Entry) {
        self.report(&source, &entry);
        self.entries().insert(source, entry);
    }

    fn report(&self, source: &str, entry: &Entry) {
        let Some(event) = entry.event(source) else {
            return;
        };
        if let Some(sink) = self.sink.borrow().as_ref() {
            match &event {
                ImageEvent::Loaded {
                    source,
                    width,
                    height,
                    ..
                } => sink.loaded(source, *width, *height),
                ImageEvent::ParsedDocument { source, document } => {
                    sink.parsed_document(source, Arc::clone(document));
                }
                ImageEvent::Failed { source } => sink.failed(source),
                ImageEvent::Playback { picture, report } => sink.playback(*picture, report.clone()),
            }
        }
        self.pending.lock().expect("test image reports").push(event);
    }

    fn log_read(&self, source: &str, hint: ImageSizeHint) {
        self.reads
            .lock()
            .expect("test image read log")
            .push((source.to_owned(), hint));
    }

    pub fn drain_events(&self) -> Vec<ImageEvent> {
        std::mem::take(&mut *self.pending.lock().expect("test image reports"))
    }
}

fn urls(pictures: &[PictureKey]) -> Vec<Arc<str>> {
    pictures
        .iter()
        .filter_map(|picture| match picture {
            PictureKey::Url(url) => Some(Arc::clone(url)),
            PictureKey::Id(_) => None,
        })
        .collect()
}

impl FrameImages for TestImages {
    type PlatformFrame = std::convert::Infallible;

    fn describe(&self, picture: &PictureKey, hint: ImageSizeHint) -> Option<FrameImage> {
        let source = picture.url()?;
        if self.releasing.get() {
            self.described
                .lock()
                .expect("test image describe log")
                .push(source.to_owned());
        } else {
            self.log_read(source, hint);
        }
        match self.entries().get(source)? {
            Entry::Ready(image) => Some(FrameImage::resident(image.clone())),
            Entry::Released { descriptor, .. } => Some(FrameImage::released(*descriptor)),
            Entry::Pending | Entry::Svg(_) | Entry::Failed => None,
        }
    }

    fn want(&self, picture: &PictureKey, hint: ImageSizeHint) {
        let Some(source) = picture.url() else {
            return;
        };
        let mut entries = self.entries();
        let Some(entry) = entries.get_mut(source) else {
            return;
        };
        let Entry::Released { descriptor, bytes } = entry else {
            return;
        };
        let image = ImageData {
            data: Blob::from(std::mem::take(bytes)),
            format: descriptor.format,
            alpha_type: descriptor.alpha_type,
            width: descriptor.width,
            height: descriptor.height,
        };
        *entry = Entry::Ready(image);
        drop(entries);
        self.log_read(source, hint);
    }

    fn release(&self, pictures: &[PictureKey]) {
        let sources = urls(pictures);
        self.released
            .lock()
            .expect("test image release log")
            .push(sources.clone());
        if !self.releasing.get() {
            return;
        }
        let mut entries = self.entries();
        for source in &sources {
            if let Some(entry) = entries.get_mut(&**source)
                && let Entry::Ready(image) = entry
            {
                *entry = Entry::Released {
                    descriptor: ImageDescriptor::of(image),
                    bytes: image.data.data().to_vec(),
                };
            }
        }
    }

    fn retain(&self, drawn: &[PictureKey]) {
        self.retained
            .lock()
            .expect("test image retain log")
            .push(urls(drawn));
    }
}

impl TestImages {
    pub fn request(&self, source: &str) {
        let mut entries = self.entries();
        let entry = entries.entry(source.to_owned()).or_default();
        self.report(source, entry);
    }

    pub fn request_document(&self, source: &str, bytes: &[u8], kind: DocumentKind) {
        self.report(source, &Entry::parsed(source, bytes, kind));
    }
}

#[must_use]
pub fn rgba8(width: u32, height: u32, pixels: Vec<u8>) -> ImageData {
    ImageData {
        data: Blob::from(pixels),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    }
}

pub fn pump_images<T>(document: &mut Document<T>, store: &TestImages) -> bool {
    for source in document.take_wanted_images() {
        store.request(&source);
    }
    for (source, bytes, kind) in document.take_document_requests() {
        store.request_document(&source, &bytes, kind);
    }
    let events = store.drain_events();
    let _settled = document.apply_image_events(&events);
    !events.is_empty()
}

pub fn present_urls<T>(document: &mut Document<T>, presented: &[(NodeId, &str)]) {
    for &(node, url) in presented {
        let natural = document.request_image(url).natural_size();
        document.set_picture(node, Some((PictureKey::Url(Arc::from(url)), natural)));
    }
}

pub fn render_presenting<T: Sync>(
    document: &mut Document<T>,
    store: &TestImages,
    presented: &[(NodeId, &str)],
) {
    present_urls(document, presented);
    for _ in 0..8 {
        document.render();
        let moved = pump_images(document, store);
        present_urls(document, presented);
        if !moved {
            return;
        }
    }
    document.render();
}

pub fn render_with_images<T: Sync>(document: &mut Document<T>, store: &TestImages) {
    for _ in 0..8 {
        document.render();
        if !pump_images(document, store) {
            return;
        }
    }
    document.render();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use terminus::link::{
        Device, Document, DocumentKind, ImageEvent, ImageStatus, NodeId, PictureKey,
    };

    use super::{TestImages, pump_images};

    const ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="6"/>"#;
    const BROKEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg"><rect></svg>"#;

    fn page() -> (Document<()>, NodeId) {
        let mut document = Document::new(Device::new(200.0, 200.0, 1.0), "page", ());
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);
        (document, image)
    }

    #[test]
    fn a_published_svg_is_reported_parsed_and_a_malformed_one_failed() {
        let store = TestImages::new();
        store.insert_svg("app:///icon.svg", ICON);
        store.insert_svg("app:///broken.svg", BROKEN);

        let events = store.drain_events();
        assert!(
            matches!(
                events.as_slice(),
                [
                    ImageEvent::ParsedDocument { source: parsed, .. },
                    ImageEvent::Failed { source: failed },
                ] if &**parsed == "app:///icon.svg" && &**failed == "app:///broken.svg"
            ),
            "{events:?}"
        );
        assert_eq!(store.len(), 1, "only the parsed document is held");
        assert!(
            terminus::link::FrameImages::describe(
                &store,
                &terminus::link::PictureKey::Url(std::sync::Arc::from("app:///icon.svg")),
                terminus::link::ImageSizeHint::UNBOUNDED
            )
            .is_none(),
            "the engine draws a document itself and never asks for it"
        );
    }

    #[test]
    fn pump_images_answers_a_document_request_by_parsing_it() {
        let store = TestImages::new();
        let (mut document, image) = page();
        let source = document.insert_image_document(ICON.as_bytes(), DocumentKind::Svg);
        document.set_picture(image, Some((PictureKey::Url(source.clone()), None)));
        assert_eq!(
            document.image_status(&source),
            ImageStatus::Pending,
            "pending until the store reports"
        );

        assert!(
            pump_images(&mut document, &store),
            "the request was answered"
        );
        assert!(
            !store.was_asked_for(&source),
            "a document request files nothing"
        );
        assert!(!pump_images(&mut document, &store), "nothing more moves");

        let again = document.insert_image_document(ICON.as_bytes(), DocumentKind::Svg);
        assert_eq!(again, source, "the same markup is the same source");
        assert_eq!(
            document.request_image(&again),
            ImageStatus::Vector {
                width: 12,
                height: 6,
            },
            "the store's report settled the shared source"
        );
    }

    #[test]
    fn every_document_request_is_parsed_and_reported() {
        let store = TestImages::new();
        store.request_document("svg-content:1", ICON.as_bytes(), DocumentKind::Svg);
        store.request_document("svg-content:1", ICON.as_bytes(), DocumentKind::Svg);
        store.request_document("svg-content:2", BROKEN.as_bytes(), DocumentKind::Svg);

        let events = store.drain_events();
        let [
            ImageEvent::ParsedDocument {
                document: first, ..
            },
            ImageEvent::ParsedDocument {
                document: again, ..
            },
            ImageEvent::Failed { source },
        ] = events.as_slice()
        else {
            panic!("two parsed documents and a failure: {events:?}");
        };
        assert!(!Arc::ptr_eq(first, again), "parsed twice");
        assert_eq!(&**source, "svg-content:2");
        assert!(store.is_empty(), "nothing is kept");
    }
}
