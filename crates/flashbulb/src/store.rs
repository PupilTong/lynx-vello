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

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dom::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use dom::{Document, FrameImages, ImageEvent, ImageReports, ImageSizeHint};

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
    /// Named as one that will never produce pixels.
    Failed,
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
        let source = source.into();
        let (width, height) = (image.width, image.height);
        self.entries().insert(source.clone(), Entry::Ready(image));
        self.report_loaded(&source, width, height);
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

    /// Names `source` as one that will never produce pixels, and reports the
    /// failure the way [`Self::insert`] reports a load.
    ///
    /// Terminal, as it is for a real host: later requests for the source
    /// answer the same failure, which is how a test failing a source before
    /// the view exists still fails it for the bind that comes later.
    pub fn fail(&self, source: impl Into<String>) {
        let source = source.into();
        self.entries().insert(source.clone(), Entry::Failed);
        self.report_failed(&source);
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
        let published: Vec<(String, Option<(u32, u32)>)> = self
            .entries()
            .iter()
            .filter_map(|(source, entry)| match entry {
                Entry::Pending => None,
                Entry::Ready(image) => Some((source.clone(), Some((image.width, image.height)))),
                Entry::Failed => Some((source.clone(), None)),
            })
            .collect();
        *self.sink.borrow_mut() = Some(sink);
        for (source, loaded) in published {
            match loaded {
                Some((width, height)) => self.report_loaded(&source, width, height),
                None => self.report_failed(&source),
            }
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
            .filter(|entry| matches!(entry, Entry::Ready(_)))
            .count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().expect("test image store")
    }

    fn report_loaded(&self, source: &str, width: u32, height: u32) {
        self.report(ImageEvent::Loaded {
            source: Arc::from(source),
            width,
            height,
        });
    }

    fn report_failed(&self, source: &str) {
        self.report(ImageEvent::Failed {
            source: Arc::from(source),
        });
    }

    fn report(&self, event: ImageEvent) {
        self.pending
            .lock()
            .expect("test image reports")
            .push(event.clone());
        if let Some(sink) = self.sink.borrow().as_ref() {
            match event {
                ImageEvent::Loaded {
                    source,
                    width,
                    height,
                } => sink.loaded(&source, width, height),
                ImageEvent::Failed { source } => sink.failed(&source),
            }
        }
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
            Entry::Pending | Entry::Failed => None,
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
        let settled = {
            let mut entries = self.entries();
            match entries.entry(source.to_owned()).or_default() {
                Entry::Pending => None,
                Entry::Ready(image) => Some(Some((image.width, image.height))),
                Entry::Failed => Some(None),
            }
        };
        match settled {
            Some(Some((width, height))) => self.report_loaded(source, width, height),
            Some(None) => self.report_failed(source),
            None => {}
        }
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
/// painter runs: request every source the last walk discovered, then apply
/// whatever the host reported.
///
/// Returns whether anything moved, so a caller can loop to quiescence.
pub fn pump_images<T>(document: &mut Document<T>, store: &TestImages) -> bool {
    for source in document.take_wanted_images() {
        store.request(&source);
    }
    let events = store.drain_events();
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
