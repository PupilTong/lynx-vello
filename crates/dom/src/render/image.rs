//! Image identity and the seam the compose path reads pixels through.
//!
//! Nothing on the document's thread holds decoded pixels, and nothing this
//! crate publishes carries them either. The split is:
//!
//! - [`ImageRegistry`] lives on the document. It is a name table keyed by the raw source string the
//!   page wrote, holding each image's load state and its own intrinsic dimensions. No
//!   [`ImageData`], no [`Blob`](vello::peniko::Blob), no store — and no identity to mint, because
//!   nothing per-image is stored behind a name.
//! - [`FrameImages`] is the read seam. A committed frame's image draws name a source, and whoever
//!   composes that frame supplies the pixels for it.
//!
//! Producing those pixels is not this crate's business at all: the embedder's
//! resource system owns bytes, decoding, residency and eviction, and this
//! crate only ever names images and reads them back.
//!
//! # Why the read is synchronous and may block
//!
//! Composition holds `&mut Scene` and an open clip stack; it can neither
//! suspend nor leave a layer unbalanced. So the pixel read is one
//! non-suspending call: [`FrameImages::read`].
//!
//! Unlike the non-blocking residency probe this replaces, `read` is not
//! allowed to answer "not resident". Once a store has reported an image
//! loaded, `read` must produce its pixels — restoring them from the store's
//! own backing store inside the call if its memory cache dropped them.
//! Blocking there is accepted: the caller is the painter, and the engine
//! calls it outside the window in which a swap-chain image is held.
//!
//! That is what makes eviction invisible above this module. There is no way
//! to report that an image stopped being resident, so a document's image
//! state can never regress from loaded back to pending.
//!
//! # What the engine will not draw
//!
//! Vello packs every scene image into one shared square atlas that grows by
//! doubling to [`MAX_RENDERABLE_DIMENSION`]. An image longer than that on
//! either axis never fits at any atlas size: the resolve pass leaves it
//! unallocated and zeroes the draw's dimensions, so it renders as nothing
//! with no error anywhere — after growing the shared atlas to its maximum, a
//! texture that never shrinks, and re-uploading every image already in it.
//!
//! So [`is_renderable`] refuses such a bitmap before it reaches vello. It is
//! tested against the pixels [`FrameImages::read`] actually returns, and
//! never against an image's intrinsic size: **the intrinsic size is a CSS
//! input and is not bounded at all**. A host may report a 12000x6000 image
//! and decode it to 6000x3000; that lays out at its true size and ratio and
//! draws correctly, because an image draw carries its anchor and extent
//! unmultiplied and divides by the real bitmap dimensions at encode time.

use std::cell::RefCell;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use vello::peniko::ImageData;

use crate::NodeId;

/// Largest per-axis pixel count vello can render.
///
/// Vello's image atlas starts at 1024 px and doubles until an image fits,
/// stopping at `vello_encoding`'s `MAX_ATLAS_SIZE` of 8192. An image within
/// this bound may still fail to allocate transiently when the atlas is full of
/// other images — vello resolves that itself by evicting or growing — but an
/// image past it can never be placed at all.
pub const MAX_RENDERABLE_DIMENSION: u32 = 8192;

/// Whether vello can place this bitmap in its image atlas.
///
/// Tested against the bitmap a frame actually reads, never against an image's
/// intrinsic size. A zero axis is refused for a second reason: the brush
/// transform divides the draw's extent by these dimensions, and dividing by
/// zero encodes a non-finite transform that vello accepts without complaint.
#[must_use]
pub fn is_renderable(data: &ImageData) -> bool {
    data.width > 0
        && data.height > 0
        && data.width <= MAX_RENDERABLE_DIMENSION
        && data.height <= MAX_RENDERABLE_DIMENSION
}

/// The synchronous pixel source a frame's image draws resolve against.
///
/// The embedder's resource system is one implementation; the painter's
/// per-commit resolved set is another; a test double is a third. This is the
/// only image-shaped type the compose path knows.
pub trait FrameImages {
    /// The pixels for `source`, or `None` when there are none to draw.
    ///
    /// May block. See the module docs for the contract a real store owes
    /// here: after a successful load, this must not miss.
    ///
    /// `source` is the raw string the page wrote, and it is only ever one the
    /// host has already reported loaded. The bitmap need not match the
    /// intrinsic size that was reported with it: a reduced-scale decode
    /// composes correctly.
    fn read(&self, source: &str) -> Option<ImageData>;
}

/// Composes every image draw as nothing: the pixel source for a scene built
/// without an embedder, and for the many call sites that render pages with no
/// images at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoImages;

/// A shared handle serves whatever it points at, so an embedder holding its
/// resource system behind an `Arc` needs no forwarding wrapper of its own.
impl<T: FrameImages + ?Sized> FrameImages for std::sync::Arc<T> {
    fn read(&self, source: &str) -> Option<ImageData> {
        (**self).read(source)
    }
}

impl FrameImages for NoImages {
    fn read(&self, _source: &str) -> Option<ImageData> {
        None
    }
}

/// How a store reports progress. Implemented by the painter; callable from
/// whatever thread the store loads on.
///
/// Every method is non-blocking and must not re-enter the store.
pub trait ImageSink: Send + Sync {
    /// `source`'s bytes are readable, and the image's own full-resolution
    /// size is `width` x `height`.
    ///
    /// Reported once per source: one URL has one content, so a second report
    /// for a source already resolved changes nothing. There is deliberately
    /// no way to say that an image stopped being loaded — eviction is
    /// invisible above this trait, which is what keeps a document's state
    /// from regressing.
    ///
    /// Neither dimension is bounded here. This is the image's intrinsic size,
    /// which layout uses; what a host may actually decode to is bounded, and
    /// checked where the pixels are read.
    fn loaded(&self, source: &str, width: u32, height: u32);

    /// `source` will not produce pixels. Terminal; the engine does not retry.
    fn failed(&self, source: &str);
}

/// One report from the store, on its way to the document.
///
/// `Send` by construction: an `Arc<str>` and integers. There is no variant
/// that could carry pixels, which is what makes "`ImageData` never crosses a
/// channel" a property of the type rather than a rule to remember.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageEvent {
    /// Pixels exist for this source, with these intrinsic dimensions.
    Loaded {
        source: Arc<str>,
        width: u32,
        height: u32,
    },
    /// This source will never produce pixels.
    Failed { source: Arc<str> },
}

/// What the document knows about one image. Never any pixels.
///
/// `Pending` is the only state with outgoing edges; both others are sinks,
/// which is the whole of "the document's image state never regresses".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ImageState {
    /// Asked for; no pixels yet.
    #[default]
    Pending,
    /// The only drawable state — so a frame naming an image that is not
    /// loaded is unrepresentable rather than filtered out later.
    Ready { width: u32, height: u32 },
    /// Terminal.
    Failed,
}

/// What the registry holds for one source.
#[derive(Debug, Default)]
struct Entry {
    state: ImageState,
    /// Replaced nodes presenting this source, so a completed load knows whose
    /// natural size to set. A `background-image` user is not here: it has no
    /// natural size, and the load invalidates the frame anyway.
    nodes: SmallVec<[NodeId; 1]>,
}

/// The document's whole image state: a name table keyed by the raw source
/// string the page wrote.
///
/// Holds no pixels and no store. One source is one entry with one content —
/// there is no per-image identity to mint and no generation to track, because
/// nothing per-image is stored behind a name. Two specifiers a host
/// canonicalises to one resource stay two entries, and that duplication costs
/// an `Arc<str>` and a few words.
///
/// The invariant that makes it work: **an entry exists exactly when the
/// source has been asked for.** There is no window in which a source is known
/// but has no key, because the key *is* the source.
#[derive(Default)]
pub(crate) struct ImageRegistry {
    entries: FxHashMap<Arc<str>, Entry>,
    /// Sources met by a walk that have not been requested yet.
    ///
    /// `RefCell` because the paint walk takes the document shared, and the
    /// walk is exactly where sources are discovered.
    wanted: RefCell<Vec<Arc<str>>>,
}

impl std::fmt::Debug for ImageRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageRegistry")
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl ImageRegistry {
    /// The draw name and intrinsic dimensions for `source`, requesting it if
    /// this is the registry's first sighting.
    ///
    /// `None` means "paint nothing this frame": pending, or failed. A pending
    /// image is the one-frame gap between a source appearing and its pixels
    /// arriving, which is what a browser shows for a not-yet-loaded image.
    ///
    /// The name handed back is the map's own key, so every draw of one source
    /// in one frame shares one allocation.
    pub(crate) fn resolve(&self, source: &str) -> Option<(Arc<str>, (f64, f64))> {
        let Some((key, entry)) = self.entries.get_key_value(source) else {
            // First sighting: ask for it. Taking `&self` is what lets this run
            // inside the walk, the only place that knows which sources a frame
            // actually needs.
            //
            // Deduplicated here rather than on the way out: a list of 200 rows
            // sharing one `url(...)` resolves it 200 times on its first
            // commit, and allocating a copy of the URL per *draw* to request
            // one image is the wrong shape on the most latency-sensitive
            // commit there is. The list is tiny, so the scan beats a set.
            let mut wanted = self.wanted.borrow_mut();
            if !wanted.iter().any(|pending| &**pending == source) {
                wanted.push(Arc::from(source));
            }
            return None;
        };
        match entry.state {
            ImageState::Ready { width, height } => {
                Some((Arc::clone(key), (f64::from(width), f64::from(height))))
            }
            ImageState::Pending | ImageState::Failed => None,
        }
    }

    /// Records that `node` presents `source`, so a completed load knows whose
    /// natural size to update.
    pub(crate) fn bind_node(&mut self, source: &str, node: NodeId) {
        let entry = self.entry_for(source);
        if !entry.nodes.contains(&node) {
            entry.nodes.push(node);
        }
    }

    /// Drops `node`'s claim on `source`.
    pub(crate) fn unbind_node(&mut self, source: &str, node: NodeId) {
        if let Some(entry) = self.entries.get_mut(source) {
            entry.nodes.retain(|held| *held != node);
        }
    }

    /// The sources discovered since the last drain, for the painter to ask
    /// the host for. Already deduplicated — `resolve` never queues one twice.
    ///
    /// Recording each one here is what makes a source asked-for exactly once:
    /// its entry exists from this moment, so `resolve` finds it and never
    /// queues it again.
    pub(crate) fn take_wanted(&mut self) -> Vec<Arc<str>> {
        let wanted = std::mem::take(self.wanted.get_mut());
        for source in &wanted {
            self.entries.entry(Arc::clone(source)).or_default();
        }
        wanted
    }

    /// Applies one report from the host.
    ///
    /// `None` when nothing moved — a source reported twice, which one URL
    /// with one content makes a no-op. `Some` carries the replaced nodes
    /// whose natural size the caller must now set, empty when the load names
    /// none.
    pub(crate) fn apply(&mut self, event: &ImageEvent) -> Option<SmallVec<[NodeId; 1]>> {
        match event {
            ImageEvent::Loaded {
                source,
                width,
                height,
            } => {
                // Well-formedness, not the atlas bound: an image with a zero
                // axis has neither an intrinsic size nor an aspect ratio, so
                // it would stretch an unknown bitmap over the whole content
                // box. `MAX_RENDERABLE_DIMENSION` is deliberately not tested
                // here — see `is_renderable`, which tests the bitmap instead.
                if *width == 0 || *height == 0 {
                    return self.apply(&ImageEvent::Failed {
                        source: Arc::clone(source),
                    });
                }
                let entry = self.entry_for(source);
                if entry.state != ImageState::Pending {
                    return None;
                }
                entry.state = ImageState::Ready {
                    width: *width,
                    height: *height,
                };
                Some(entry.nodes.clone())
            }
            ImageEvent::Failed { source } => {
                let entry = self.entry_for(source);
                if entry.state != ImageState::Pending {
                    return None;
                }
                entry.state = ImageState::Failed;
                Some(SmallVec::new())
            }
        }
    }

    /// The intrinsic dimensions already known for `source`, if it has loaded.
    pub(crate) fn dimensions_of(&self, source: &str) -> Option<(u32, u32)> {
        match self.entries.get(source)?.state {
            ImageState::Ready { width, height } => Some((width, height)),
            ImageState::Pending | ImageState::Failed => None,
        }
    }

    fn entry_for(&mut self, source: &str) -> &mut Entry {
        // `raw_entry` is unstable, so a miss costs one allocation of a string
        // the registry is about to own anyway.
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
    use std::sync::Arc;

    use super::{ImageEvent, ImageRegistry, MAX_RENDERABLE_DIMENSION, NoImages, is_renderable};
    use crate::render::image::FrameImages;

    fn loaded(source: &str, width: u32, height: u32) -> ImageEvent {
        ImageEvent::Loaded {
            source: Arc::from(source),
            width,
            height,
        }
    }

    fn failed(source: &str) -> ImageEvent {
        ImageEvent::Failed {
            source: Arc::from(source),
        }
    }

    fn image(width: u32, height: u32) -> vello::peniko::ImageData {
        use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
        // Declares the size without allocating it: every check under test
        // reads the declared dimensions, not the buffer.
        ImageData {
            data: Blob::from(vec![0_u8; 4]),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        }
    }

    /// A document must still cross threads with a registry on it: it is built
    /// on one thread and run on another.
    #[test]
    fn a_document_carrying_a_registry_still_crosses_threads() {
        const fn assert_send<T: Send>() {}
        assert_send::<crate::Document<()>>();
    }

    #[test]
    fn a_source_is_asked_for_once_however_often_it_is_drawn() {
        let mut registry = ImageRegistry::default();
        // A page whose every box shares one background image resolves the
        // same source once per draw.
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

        registry.apply(&loaded("app:///a.png", 40, 20));
        let (source, dimensions) = registry
            .resolve("app:///a.png")
            .expect("a loaded image resolves");
        assert_eq!(source.as_ref(), "app:///a.png");
        assert!((dimensions.0 - 40.0).abs() < f64::EPSILON);
        assert!((dimensions.1 - 20.0).abs() < f64::EPSILON);
    }

    /// The whole of issue 3: an intrinsic size is a CSS input and is not
    /// bounded. A host may report a huge image and decode it small.
    #[test]
    fn an_intrinsic_size_past_the_atlas_bound_still_lays_out_and_draws() {
        let mut registry = ImageRegistry::default();
        let _ = registry.resolve("app:///huge.png");
        registry.take_wanted();
        registry.apply(&loaded("app:///huge.png", 12_000, 6_000));

        let (_, dimensions) = registry
            .resolve("app:///huge.png")
            .expect("a huge image is drawable — it is the bitmap that is bounded, not this");
        assert!((dimensions.0 - 12_000.0).abs() < f64::EPSILON);

        // And the bitmap the host actually decoded is what the atlas bound
        // is tested against.
        assert!(is_renderable(&image(6_000, 3_000)));
        assert!(!is_renderable(&image(12_000, 6_000)));
    }

    #[test]
    fn the_atlas_bound_is_inclusive_and_a_zero_axis_is_refused() {
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

    /// A zero axis is not an atlas question: such an image has neither an
    /// intrinsic size nor a ratio, so it is refused as malformed.
    #[test]
    fn a_zero_intrinsic_axis_is_a_load_failure() {
        for (width, height) in [(0, 4), (4, 0)] {
            let mut registry = ImageRegistry::default();
            let _ = registry.resolve("app:///bad.png");
            registry.take_wanted();
            registry.apply(&loaded("app:///bad.png", width, height));
            assert!(
                registry.resolve("app:///bad.png").is_none(),
                "{width}x{height} has no usable intrinsic size"
            );
        }
    }

    /// One URL, one content: a host repeating itself changes nothing, and a
    /// late failure cannot un-load an image that already resolved.
    #[test]
    fn a_loaded_image_never_regresses() {
        let mut registry = ImageRegistry::default();
        let _ = registry.resolve("app:///a.png");
        registry.take_wanted();
        assert!(registry.apply(&loaded("app:///a.png", 10, 10)).is_some());

        assert!(
            registry.apply(&failed("app:///a.png")).is_none(),
            "a late failure on a loaded image moves nothing"
        );
        assert!(registry.resolve("app:///a.png").is_some(), "still drawable");

        assert!(
            registry.apply(&loaded("app:///a.png", 99, 99)).is_none(),
            "and a repeated report moves nothing"
        );
        let (_, dimensions) = registry.resolve("app:///a.png").expect("still drawable");
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
        registry.apply(&failed("app:///a.png"));
        assert!(registry.resolve("app:///a.png").is_none());
        assert!(
            registry.apply(&loaded("app:///a.png", 4, 4)).is_none(),
            "a failure is terminal: pixels arriving later change nothing"
        );
        assert!(registry.resolve("app:///a.png").is_none());
    }

    #[test]
    fn a_load_reports_the_nodes_whose_natural_size_must_change() {
        let mut registry = ImageRegistry::default();
        let mut document = crate::Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.create_element("view", ());
        registry.bind_node("app:///a.png", root);
        let nodes = registry
            .apply(&loaded("app:///a.png", 12, 6))
            .expect("the load moved the entry");
        assert_eq!(nodes.as_slice(), [root], "the bound node relayouts");
    }

    #[test]
    fn the_empty_pixel_source_draws_nothing() {
        assert!(NoImages.read("app:///a.png").is_none());
    }
}
