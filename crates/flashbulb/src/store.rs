//! An in-memory image source for tests and benchmarks: a [`dom::FrameImages`]
//! that answers from whatever a test published into it.
//!
//! Production stores fetch, decode, cache and evict; this one does none of
//! that. It holds exactly the pixels a test put in it, so a render either
//! draws them or proves it looked the image up under the wrong source.
//!
//! It reports every load inline through the [`dom::ImageReports`] a view
//! handed it, so a test needs no async runtime at all. What it does honour is
//! the identity contract: every [`FrameImages::read`] of one
//! source returns a clone sharing the same `Blob`, which is what vello keys
//! its atlas on.
//!
//! An SVG document is parsed here, the way a production host parses one:
//! with the engine's own parser ([`ImageEvent::parse_document`]), and
//! reported as the parsed document, or as a failure when it does not parse.
//! A document is published through [`TestImages::insert_svg`], or handed
//! over by the page as markup, which reaches the store as a document request
//! ([`TestImages::request_document`], driven by [`pump_images`]). A
//! document request keeps nothing, as a production host keeps nothing:
//! every one is parsed and reported on its own. [`FrameImages::read`] never
//! answers for either, because the engine never asks.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dom::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use dom::{
    Document, DocumentKind, FrameImages, ImageEvent, ImageReports, ImageSizeHint, VectorDocument,
};

/// What this store answers for one source.
///
/// The three states a real host has, so a test can hold one source pending
/// while another has pixels and a third will never produce any — and so that
/// "failed" is a state rather than the absence of one.
#[derive(Default)]
enum Entry {
    /// Asked for; no pixels published.
    #[default]
    Pending,
    Ready(ImageData),
    /// An SVG document a test published, parsed at publication: what the
    /// store answers its source with, as [`Entry::Ready`] holds a test's
    /// pixels.
    Svg(Arc<VectorDocument>),
    /// Named as one that will never produce pixels.
    Failed,
}

impl Entry {
    /// What `bytes`, a document of `kind` for `source`, settle as: the
    /// document the engine's parser makes of them, or a failure.
    fn parsed(source: &str, bytes: &[u8], kind: DocumentKind) -> Self {
        match ImageEvent::parse_document(Arc::from(source), bytes, kind) {
            ImageEvent::ParsedDocument { document, .. } => Entry::Svg(document),
            ImageEvent::Loaded { .. } | ImageEvent::Failed { .. } => Entry::Failed,
        }
    }

    /// The report this entry settles as, or `None` while it is pending.
    fn event(&self, source: &str) -> Option<ImageEvent> {
        let source = Arc::from(source);
        match self {
            Entry::Pending => None,
            Entry::Ready(image) => Some(ImageEvent::Loaded {
                source,
                width: image.width,
                height: image.height,
            }),
            Entry::Svg(document) => Some(ImageEvent::ParsedDocument {
                source,
                document: Arc::clone(document),
            }),
            Entry::Failed => Some(ImageEvent::Failed { source }),
        }
    }
}

/// Decoded images keyed by the source string the paint walk asks for.
#[derive(Default)]
pub struct TestImages {
    /// One source, one content — the same shape the engine's own registry has.
    entries: Mutex<HashMap<String, Entry>>,
    /// Where completed loads are reported. Absent until the painter installs
    /// one, which lets a test publish images before the view exists.
    ///
    /// A `RefCell` rather than a `Mutex` because [`ImageReports`] is
    /// thread-bound — which is also why this store, and anything holding it,
    /// is `!Sync`.
    sink: RefCell<Option<ImageReports>>,
    /// Every `retain` hint received, so a test can assert on the working set.
    retained: Mutex<Vec<Vec<Arc<str>>>>,
    /// Reports not yet drained by [`pump_images`], for tests driving the
    /// protocol by hand instead of through a painter.
    pending: Mutex<Vec<ImageEvent>>,
    /// Every read, with the size hint the frame passed for it, so a test can
    /// assert on what a store would have been told to decode.
    reads: Mutex<Vec<(String, ImageSizeHint)>>,
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

    /// Publishes `image` under `source`, replacing any previous entry.
    ///
    /// Takes `&self` because a store is shared behind a handle and a test
    /// still has to change what it answers afterwards. If a sink is
    /// installed, the load is reported through it immediately.
    pub fn insert(&self, source: impl Into<String>, image: ImageData) {
        self.publish(source.into(), Entry::Ready(image));
    }

    /// Publishes tightly packed, row-major, straight-alpha RGBA8 pixels.
    ///
    /// # Panics
    ///
    /// If `pixels` is not exactly `width * height * 4` bytes.
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

    /// Publishes the SVG document `svg` under `source`: parses it with the
    /// engine's parser, as a production host does, and reports the document
    /// through [`ImageReports::parsed_document`], the way [`Self::insert`]
    /// reports a bitmap, or reports a failure when it does not parse. The
    /// published document is what a later request for `source` is answered
    /// with, as published pixels are.
    pub fn insert_svg(&self, source: impl Into<String>, svg: &str) {
        let source = source.into();
        let entry = Entry::parsed(&source, svg.as_bytes(), DocumentKind::Svg);
        self.publish(source, entry);
    }

    /// Names `source` as one that will never produce pixels, and reports the
    /// failure the way [`Self::insert`] reports a load.
    ///
    /// Terminal, as it is for a real host: later requests for the source
    /// answer the same failure, which is how a test failing a source before
    /// the view exists still fails it for the bind that comes later.
    pub fn fail(&self, source: impl Into<String>) {
        self.publish(source.into(), Entry::Failed);
    }

    /// Drops the pixels for `source`, so later reads miss.
    ///
    /// Deliberately keeps the id: a real store's eviction does not retract an
    /// id either, and nothing above the store may observe residency.
    pub fn remove(&self, source: &str) {
        if let Some(entry) = self.entries().get_mut(source) {
            *entry = Entry::Pending;
        }
    }

    /// Installs the sink completed loads report through, and replays every
    /// image already published so a store warmed before the view still
    /// reports its contents.
    pub fn attach(&self, sink: ImageReports) {
        *self.sink.borrow_mut() = Some(sink);
        for (source, entry) in self.entries().iter() {
            self.report(source, entry);
        }
    }

    /// The working sets reported through [`FrameImages::retain`], in order.
    #[must_use]
    pub fn retained(&self) -> Vec<Vec<Arc<str>>> {
        self.retained.lock().expect("test image retain log").clone()
    }

    /// Every read so far with its size hint, in order — the sizes a decoding
    /// store would have been asked for.
    #[must_use]
    pub fn reads(&self) -> Vec<(String, ImageSizeHint)> {
        self.reads.lock().expect("test image read log").clone()
    }

    /// Whether this store has been asked for `source` at all.
    #[must_use]
    pub fn was_asked_for(&self, source: &str) -> bool {
        self.entries().contains_key(source)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries()
            .values()
            .filter(|entry| matches!(entry, Entry::Ready(_) | Entry::Svg(_)))
            .count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().expect("test image store")
    }

    /// Reports `entry` for `source`, then makes it the entry for `source`,
    /// replacing any previous one.
    fn publish(&self, source: String, entry: Entry) {
        self.report(&source, &entry);
        self.entries().insert(source, entry);
    }

    /// Logs the report `entry` settles as for [`Self::drain_events`] and
    /// makes it through the sink if one is installed. A pending entry reports
    /// nothing.
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
                } => sink.loaded(source, *width, *height),
                ImageEvent::ParsedDocument { source, document } => {
                    sink.parsed_document(source, Arc::clone(document));
                }
                ImageEvent::Failed { source } => sink.failed(source),
            }
        }
        self.pending.lock().expect("test image reports").push(event);
    }

    /// Takes the reports made since the last drain.
    pub fn drain_events(&self) -> Vec<ImageEvent> {
        std::mem::take(&mut *self.pending.lock().expect("test image reports"))
    }
}

impl FrameImages for TestImages {
    /// Returns a clone that shares the published `Blob`, so every read of one
    /// source carries the same `Blob::id()` — the identity vello keys its
    /// atlas on. The hint is recorded and otherwise ignored: this store
    /// decodes nothing, so there is nothing to size.
    fn read(&self, source: &str, hint: ImageSizeHint) -> Option<ImageData> {
        self.reads
            .lock()
            .expect("test image read log")
            .push((source.to_owned(), hint));
        match self.entries().get(source)? {
            Entry::Ready(image) => Some(image.clone()),
            Entry::Pending | Entry::Svg(_) | Entry::Failed => None,
        }
    }

    /// Records the working set a resolve pass reported.
    fn retain(&self, frame: &[Arc<str>]) {
        self.retained
            .lock()
            .expect("test image retain log")
            .push(frame.to_vec());
    }
}

impl TestImages {
    /// Names `source` and reports it immediately if pixels are published.
    ///
    /// Inherent rather than a trait impl: the embedder-facing resource trait
    /// lives in `bobcat-core`, which this crate deliberately does not depend
    /// on. A `bobcat-core` test wraps this in its own adapter.
    pub fn request(&self, source: &str) {
        // Single-flight is trivial here: one entry per source, and a source
        // that has already settled either way starts no work.
        let mut entries = self.entries();
        let entry = entries.entry(source.to_owned()).or_default();
        self.report(source, entry);
    }

    /// Answers a document request: `bytes`, a document of `kind` a page
    /// handed over as markup, filed by the engine under `source`. Parses
    /// them with the engine's parser, as a production host does, and
    /// reports the document, or a failure when it does not parse.
    ///
    /// Keeps nothing, as a production host keeps nothing: every request is
    /// a parse and a report of its own, and the source is not asked for
    /// ([`Self::was_asked_for`]). Inherent for the reason [`Self::request`]
    /// is.
    pub fn request_document(&self, source: &str, bytes: &[u8], kind: DocumentKind) {
        self.report(source, &Entry::parsed(source, bytes, kind));
    }
}

/// Wraps tightly packed, row-major, straight-alpha RGBA8 pixels as the
/// `peniko` image a store hands the paint walk.
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

/// Drives one round of the document-to-host image protocol, the same loop a
/// painter runs: request every source the last walk discovered, hand the
/// store every document the page handed over as markup
/// ([`Document::set_image_document`], drained through
/// [`Document::take_document_requests`] as the runtime drains it), then
/// apply whatever the store reported. The store parses each such document as
/// a host does ([`TestImages::request_document`]); nothing parses inside the
/// document.
///
/// Returns whether anything moved, so a caller can loop to quiescence.
pub fn pump_images<T>(document: &mut Document<T>, store: &TestImages) -> bool {
    for source in document.take_wanted_images() {
        store.request(&source);
    }
    for (source, bytes, kind) in document.take_document_requests() {
        store.request_document(&source, &bytes, kind);
    }
    let events = store.drain_events();
    // The outcomes go nowhere: they are what an embedder turns into `load`
    // and `error` events, and a screenshot has no realm to dispatch one in.
    let _outcomes = document.apply_image_events(&events);
    !events.is_empty()
}

/// Renders until every image the page needs has been requested, reported and
/// laid out — at most a few rounds, since each one can only discover sources
/// the previous round's layout made visible.
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

    use dom::{Device, Document, DocumentKind, ImageEvent, ImageOutcome, ImageRole, NodeId};

    use super::{TestImages, pump_images};

    const ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="6"/>"#;
    const BROKEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg"><rect></svg>"#;

    /// A document with one `image` element under its root.
    fn page() -> (Document<()>, NodeId) {
        let mut document = Document::new(Device::new(200.0, 200.0, 1.0), "page", ());
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);
        (document, image)
    }

    /// The store parses a published document as a host does: a well-formed
    /// one is reported as the parsed document, which no read answers for,
    /// and one that does not parse is reported as a failure.
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
            dom::FrameImages::read(&store, "app:///icon.svg", dom::ImageSizeHint::UNBOUNDED)
                .is_none(),
            "the engine draws a document itself and never reads it"
        );
    }

    /// Markup a page handed over reaches the store as a document request in
    /// `pump_images`, the store parses it, and its report settles the
    /// element; a second round has nothing to do.
    #[test]
    fn pump_images_answers_a_document_request_by_parsing_it() {
        let store = TestImages::new();
        let (mut document, image) = page();
        assert_eq!(
            document.set_image_document(
                image,
                ImageRole::Source,
                ICON.as_bytes(),
                DocumentKind::Svg
            ),
            None,
            "pending until the store reports"
        );
        let source = document
            .image_source(image, ImageRole::Source)
            .expect("a synthetic source")
            .to_owned();

        assert!(
            pump_images(&mut document, &store),
            "the request was answered"
        );
        assert!(
            !store.was_asked_for(&source),
            "a document request files nothing"
        );
        assert!(!pump_images(&mut document, &store), "nothing more moves");

        let root = document.document_element().id();
        let second = document.create_element("image", ());
        document.append_child(root, second);
        assert_eq!(
            document.set_image_document(
                second,
                ImageRole::Source,
                ICON.as_bytes(),
                DocumentKind::Svg
            ),
            Some(ImageOutcome::Loaded {
                node: second,
                width: 12,
                height: 6,
            }),
            "the store's report settled the shared source"
        );
    }

    /// Every document request is a parse and a report of its own: two for
    /// one source are two documents, and markup that does not parse is
    /// reported as a failure.
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
