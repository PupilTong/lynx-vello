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
//! An SVG document published through [`TestImages::insert_svg`] is parsed
//! here with `usvg` and reported as a [`VectorImage`]; [`FrameImages::read`]
//! never answers for it, because the engine never asks.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dom::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use dom::{Document, FrameImages, ImageEvent, ImageReports, ImageSizeHint, VectorImage};

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
    /// A parsed SVG document.
    Vector(VectorImage),
    /// Named as one that will never produce pixels.
    Failed,
}

impl Entry {
    /// The report this entry settles as, or `None` while it is pending.
    fn settled(&self, source: &str) -> Option<ImageEvent> {
        let source = Arc::from(source);
        match self {
            Entry::Pending => None,
            Entry::Ready(image) => Some(ImageEvent::Loaded {
                source,
                width: image.width,
                height: image.height,
            }),
            Entry::Vector(image) => Some(ImageEvent::LoadedVector {
                source,
                image: image.clone(),
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

    /// Publishes the SVG document `svg` under `source`, parsed with `usvg`,
    /// and reports it the way [`Self::insert`] reports a bitmap.
    ///
    /// The natural size and viewport follow the design's four cases
    /// ([`svg_sizes`]). Parsing reads no file: an `<image>` inside the
    /// document that names anything but a `data:` URL resolves to nothing.
    ///
    /// # Panics
    ///
    /// If `svg` does not parse.
    pub fn insert_svg(&self, source: impl Into<String>, svg: &str) {
        let source = source.into();
        let image = parse_svg(svg).unwrap_or_else(|error| panic!("{source}: {error}"));
        self.entries()
            .insert(source.clone(), Entry::Vector(image.clone()));
        self.report(ImageEvent::LoadedVector {
            source: Arc::from(source.as_str()),
            image,
        });
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
        let published: Vec<ImageEvent> = self
            .entries()
            .iter()
            .filter_map(|(source, entry)| entry.settled(source))
            .collect();
        *self.sink.borrow_mut() = Some(sink);
        for event in published {
            self.report(event);
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
            .filter(|entry| matches!(entry, Entry::Ready(_) | Entry::Vector(_)))
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
                ImageEvent::LoadedVector { source, image } => sink.loaded_vector(&source, image),
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
            Entry::Pending | Entry::Vector(_) | Entry::Failed => None,
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
        let settled = self
            .entries()
            .entry(source.to_owned())
            .or_default()
            .settled(source);
        if let Some(event) = settled {
            self.report(event);
        }
    }
}

/// Parses an SVG document the way a host does: one XML parse, the root's
/// `width`, `height` and `viewBox` read for [`svg_sizes`], then the `usvg`
/// tree built from that same parse with no filesystem access.
fn parse_svg(svg: &str) -> Result<VectorImage, String> {
    let document = usvg::roxmltree::Document::parse_with_options(
        svg,
        usvg::roxmltree::ParsingOptions {
            allow_dtd: true,
            ..usvg::roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|error| error.to_string())?;
    let root = document.root_element();
    let options = usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            ..usvg::ImageHrefResolver::default()
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_xmltree(&document, &options).map_err(|error| error.to_string())?;
    let (natural, viewport) = svg_sizes(
        root.attribute("width").and_then(absolute_length),
        root.attribute("height").and_then(absolute_length),
        root.attribute("viewBox").and_then(view_box),
        (tree.size().width(), tree.size().height()),
    );
    Ok(VectorImage::new(Arc::new(tree), natural, viewport))
}

/// The natural size and viewport of an SVG document, from its root's
/// absolute `width` and `height`, its `viewBox` size and the parsed tree's
/// `size()`.
///
/// A test-only copy of the host's rule (`docs/svg-vector-images-design.md`):
///
/// 1. `width` and `height` both absolute: that size.
/// 2. One absolute plus a `viewBox`: the other from the `viewBox` ratio.
/// 3. A `viewBox` only: the largest size with its ratio that fits 300x150.
/// 4. Otherwise: the absolute axis if there is one, 300 wide and 150 high for the others.
///
/// The natural size is rounded to whole px, at least 1. The viewport is the
/// tree's `size()` wherever a `viewBox` or both dimensions are known, which
/// is what usvg maps the content into; in case 4 usvg leaves user units 1:1
/// and overwrites `size()` with the content bounds, so the viewport is the
/// natural size.
fn svg_sizes(
    width: Option<f32>,
    height: Option<f32>,
    view_box: Option<(f32, f32)>,
    tree_size: (f32, f32),
) -> ((u32, u32), (f32, f32)) {
    const DEFAULT: (f32, f32) = (300.0, 150.0);
    let (natural, from_tree) = match (width, height, view_box) {
        (Some(width), Some(height), _) => ((width, height), true),
        (Some(width), None, Some((box_width, box_height))) => {
            ((width, width * box_height / box_width), true)
        }
        (None, Some(height), Some((box_width, box_height))) => {
            ((height * box_width / box_height, height), true)
        }
        (None, None, Some((box_width, box_height))) => {
            let scale = (DEFAULT.0 / box_width).min(DEFAULT.1 / box_height);
            ((box_width * scale, box_height * scale), true)
        }
        (width, height, None) => (
            (width.unwrap_or(DEFAULT.0), height.unwrap_or(DEFAULT.1)),
            false,
        ),
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded and clamped to at least one before the cast"
    )]
    let whole = |length: f32| length.round().max(1.0) as u32;
    let rounded = (whole(natural.0), whole(natural.1));
    let viewport = if from_tree {
        tree_size
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a natural size is far below f32's exact integer range"
        )]
        let viewport = (rounded.0 as f32, rounded.1 as f32);
        viewport
    };
    (rounded, viewport)
}

/// A root `width` or `height` that is an absolute length: a number with no
/// unit or `px`. A percentage, or any other unit, counts as absent here.
fn absolute_length(value: &str) -> Option<f32> {
    let value = value.trim();
    let number = value.strip_suffix("px").unwrap_or(value);
    number
        .parse::<f32>()
        .ok()
        .filter(|length| length.is_finite() && *length > 0.0)
}

/// A `viewBox`'s width and height, when both are positive.
fn view_box(value: &str) -> Option<(f32, f32)> {
    let numbers: Vec<f32> = value
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|part| !part.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    match numbers.as_slice() {
        [_, _, width, height] if *width > 0.0 && *height > 0.0 => Some((*width, *height)),
        _ => None,
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
    // The outcomes go nowhere: they are what an embedder turns into `load`
    // and `error` events, and a screenshot has no realm to dispatch one in.
    let _outcomes = document.apply_image_events(&events);
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
