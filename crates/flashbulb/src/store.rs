//! An in-memory [`dom::ImageStore`] for tests and benchmarks.
//!
//! Production stores fetch, decode, cache and evict; this one does none of
//! that. It holds exactly the pixels a test put in it, so a render either
//! draws them or proves it looked the image up under the wrong source.
//!
//! It mints ids itself and reports every load inline from
//! [`ImageStore::request`], so a test needs no async runtime at all. What it
//! does honour is the identity contract: one source keeps one [`ImageId`] for
//! the store's life, and every [`FrameImages::read`] of one [`ImageRef`]
//! returns a clone sharing the same `Blob`, which is what vello keys its atlas
//! on.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use dom::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use dom::{Document, FrameImages, ImageEvent, ImageId, ImageRef, ImageSink, ImageStore};

/// One named image: its id, the generation of the pixels currently published
/// for it, and those pixels.
#[derive(Debug, Clone)]
struct Entry {
    id: ImageId,
    generation: u32,
    image: Option<ImageData>,
}

/// Decoded images keyed by the source string the paint walk asks for.
#[derive(Default)]
pub struct TestImages {
    entries: Mutex<HashMap<String, Entry>>,
    /// Ids handed out so far, so one source keeps one id for the store's life.
    next_id: Mutex<u32>,
    /// Where completed loads are reported. Absent until the painter installs
    /// one, which lets a test publish images before the view exists.
    sink: Mutex<Option<Arc<dyn ImageSink>>>,
    /// Every `retain` hint received, so a test can assert on the working set.
    retained: Mutex<Vec<Vec<ImageRef>>>,
    /// Reports not yet drained by [`pump_images`], for tests driving the
    /// protocol by hand instead of through a painter.
    pending: Mutex<Vec<ImageEvent>>,
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

    /// Publishes `image` under `source`, replacing any previous entry and
    /// bumping its generation.
    ///
    /// Takes `&self` because a store is shared behind a handle and a test
    /// still has to change what it answers afterwards. If a sink is
    /// installed, the load is reported through it immediately.
    pub fn insert(&self, source: impl Into<String>, image: ImageData) {
        let source = source.into();
        let (id, generation, width, height) = {
            let mut entries = self.entries();
            let entry = entries.entry(source).or_insert_with(|| Entry {
                id: self.mint(),
                generation: 0,
                image: None,
            });
            entry.generation += 1;
            let (width, height) = (image.width, image.height);
            entry.image = Some(image);
            (entry.id, entry.generation, width, height)
        };
        self.report_loaded(id, generation, width, height);
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

    /// Drops the pixels for `source`, so later reads miss.
    ///
    /// Deliberately keeps the id: a real store's eviction does not retract an
    /// id either, and nothing above the store may observe residency.
    pub fn remove(&self, source: &str) {
        if let Some(entry) = self.entries().get_mut(source) {
            entry.image = None;
        }
    }

    /// Installs the sink completed loads report through, and replays every
    /// image already published so a store warmed before the view still
    /// reports its contents.
    pub fn attach(&self, sink: Arc<dyn ImageSink>) {
        let published: Vec<(ImageId, u32, u32, u32)> = self
            .entries()
            .values()
            .filter_map(|entry| {
                entry
                    .image
                    .as_ref()
                    .map(|image| (entry.id, entry.generation, image.width, image.height))
            })
            .collect();
        *self.sink.lock().expect("test image sink") = Some(sink);
        for (id, generation, width, height) in published {
            self.report_loaded(id, generation, width, height);
        }
    }

    /// The working sets reported through [`ImageStore::retain`], in order.
    #[must_use]
    pub fn retained(&self) -> Vec<Vec<ImageRef>> {
        self.retained.lock().expect("test image retain log").clone()
    }

    /// The id this store gave `source`, if it has been asked for one.
    #[must_use]
    pub fn id_of(&self, source: &str) -> Option<ImageId> {
        self.entries().get(source).map(|entry| entry.id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries()
            .values()
            .filter(|entry| entry.image.is_some())
            .count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().expect("test image store")
    }

    fn mint(&self) -> ImageId {
        let mut next = self.next_id.lock().expect("test image ids");
        *next += 1;
        ImageId(NonZeroU32::new(*next).expect("ids start at one"))
    }

    fn report_loaded(&self, id: ImageId, generation: u32, width: u32, height: u32) {
        self.pending
            .lock()
            .expect("test image reports")
            .push(ImageEvent::Loaded {
                id,
                generation,
                width,
                height,
            });
        if let Some(sink) = self.sink.lock().expect("test image sink").as_ref() {
            sink.loaded(id, generation, width, height);
        }
    }

    /// Takes the reports made since the last drain.
    pub fn drain_events(&self) -> Vec<ImageEvent> {
        std::mem::take(&mut *self.pending.lock().expect("test image reports"))
    }
}

impl FrameImages for TestImages {
    /// Returns a clone that shares the published `Blob`, so every read of one
    /// `ImageRef` carries the same `Blob::id()` — the identity vello keys its
    /// atlas on.
    fn read(&self, image: ImageRef) -> Option<ImageData> {
        self.entries()
            .values()
            .find(|entry| entry.id == image.id && entry.generation == image.generation)
            .and_then(|entry| entry.image.clone())
    }
}

impl ImageStore for TestImages {
    fn request(&self, source: &str) -> ImageId {
        let (id, load) = {
            let mut entries = self.entries();
            let entry = entries.entry(source.to_owned()).or_insert_with(|| Entry {
                id: self.mint(),
                generation: 0,
                image: None,
            });
            let load = entry
                .image
                .as_ref()
                .map(|image| (entry.generation, image.width, image.height));
            (entry.id, load)
        };
        // Single-flight is trivial here: one entry per source, and a source
        // already holding pixels starts no work.
        if let Some((generation, width, height)) = load {
            self.report_loaded(id, generation, width, height);
        }
        id
    }

    fn retain(&self, frame: &[ImageRef]) {
        self.retained
            .lock()
            .expect("test image retain log")
            .push(frame.to_vec());
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

/// Lets a test keep an `Arc` handle on the store while the painter owns its
/// own reference to the same state: the painter's handle is an `Rc`, because
/// a store never leaves the painter thread and needs neither `Send` nor
/// `Sync`, but a test still has to publish images and read the retain log.
#[derive(Debug)]
pub struct SharedTestImages(Arc<TestImages>);

impl FrameImages for SharedTestImages {
    fn read(&self, image: ImageRef) -> Option<ImageData> {
        self.0.read(image)
    }
}

impl ImageStore for SharedTestImages {
    fn request(&self, source: &str) -> ImageId {
        self.0.request(source)
    }

    fn retain(&self, frame: &[ImageRef]) {
        self.0.retain(frame);
    }

    fn release(&self, id: ImageId) {
        self.0.release(id);
    }
}

/// The painter-owned handle for a store a test also holds.
#[must_use]
pub fn shared(images: &Arc<TestImages>) -> std::rc::Rc<dyn ImageStore> {
    std::rc::Rc::new(SharedTestImages(Arc::clone(images)))
}

/// Drives one round of the document-to-store image protocol, the same loop a
/// painter runs: request every source the last walk discovered, then apply
/// whatever the store reported.
///
/// Returns whether anything moved, so a caller can loop to quiescence.
pub fn pump_images<T>(document: &mut Document<T>, store: &TestImages) -> bool {
    let mut events: Vec<ImageEvent> = document
        .take_wanted_images()
        .into_iter()
        .map(|source| {
            let id = store.request(&source);
            ImageEvent::Bound { source, id }
        })
        .collect();
    events.extend(store.drain_events());
    if events.is_empty() {
        return false;
    }
    document.apply_image_events(&events);
    true
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
