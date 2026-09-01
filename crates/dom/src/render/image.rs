//! Image identity and the seam the compose path reads pixels through.
//!
//! Nothing on the document's thread holds decoded pixels, and nothing this
//! crate publishes carries them either. The split is:
//!
//! - [`ImageRegistry`] lives on the document. It is a name table — source string to [`ImageId`],
//!   plus each image's load state and its own intrinsic dimensions. No [`ImageData`], no
//!   [`Blob`](vello::peniko::Blob), no store.
//! - [`FrameImages`] is the read seam. A committed frame's image draws name an [`ImageRef`], and
//!   whoever composes that frame supplies the pixels for it.
//! - [`ImageStore`] is the embedder's resource system, owned by the painter. It is the only thing
//!   in the process that holds bytes, and the only thing that decides caching, residency and
//!   eviction.
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
//! # What the store must not report
//!
//! Vello packs every scene image into one shared square atlas that grows by
//! doubling to [`MAX_RENDERABLE_DIMENSION`]. An image longer than that on
//! either axis never fits at any atlas size: the resolve pass leaves it
//! unallocated and zeroes the draw's dimensions, so it renders as nothing
//! with no error anywhere. Such a report is turned into a load failure here,
//! at the one place it enters the engine, so an unrenderable image is never
//! laid out and never handed to vello.

use std::cell::RefCell;
use std::num::NonZeroU32;
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

/// A store's name for one image source, unique for that store's life.
///
/// Minted by the store, so two sources it canonicalises to one resource share
/// an id — and therefore one load, one decode, one buffer and one atlas slot.
/// Opaque to this crate and to the document's thread: compared, never
/// interpreted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ImageId(pub NonZeroU32);

/// The identity of one *decoding*: an image plus the content generation
/// behind it.
///
/// A generation bump is a different bitmap and therefore a different key.
/// Nothing else in the system distinguishes them, which is what lets a store
/// replace an image's content without any consumer holding a stale buffer by
/// accident.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ImageRef {
    pub id: ImageId,
    pub generation: u32,
}

/// The synchronous pixel source a frame's image draws resolve against.
///
/// [`ImageStore`] is one implementation; the painter's per-commit resolved
/// set is another; a test double is a third. This is the only image-shaped
/// type the compose path knows.
pub trait FrameImages {
    /// The pixels for `image`, or `None` when there are none to draw.
    ///
    /// May block. See the module docs for the contract a real store owes
    /// here: after a successful load, this must not miss.
    fn read(&self, image: ImageRef) -> Option<ImageData>;
}

/// Composes every image draw as nothing: the pixel source for a scene built
/// without an embedder, and for the many call sites that render pages with no
/// images at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoImages;

impl FrameImages for NoImages {
    fn read(&self, _image: ImageRef) -> Option<ImageData> {
        None
    }
}

/// How a store reports progress. Implemented by the painter; callable from
/// whatever thread the store loads on.
///
/// Every method is non-blocking and must not re-enter the store.
pub trait ImageSink: Send + Sync {
    /// `id`'s bytes are readable at `generation`, and the image's own
    /// full-resolution size is `width` x `height`.
    ///
    /// Calling again with a greater generation replaces the content; a lesser
    /// or equal generation is ignored. There is deliberately no way to report
    /// that an image stopped being loaded — eviction is invisible above this
    /// trait, which is what keeps a document's state from regressing.
    fn loaded(&self, id: ImageId, generation: u32, width: u32, height: u32);

    /// `id` will not produce pixels. Terminal; the engine does not retry, and
    /// ignores this for an id that has already loaded.
    fn failed(&self, id: ImageId);
}

/// The embedder's image resource system: the only owner of bytes, decoding,
/// disk, memory and eviction policy in the whole stack.
///
/// Built on and owned by the painter thread, and never reachable from the
/// document's thread or the embedder's own afterwards. It is therefore free to
/// be neither `Send` nor `Sync`, and to hold `Rc`, `RefCell` or browser
/// objects. Only the loaders it starts itself cross threads, and they report
/// through [`ImageSink`], which is `Send + Sync`.
pub trait ImageStore: FrameImages {
    /// Names `source` and begins loading it. Non-blocking; returns the
    /// canonical id immediately.
    ///
    /// Idempotent and single-flight: repeated or concurrent requests for one
    /// source join one load, and a request for an already-loaded source starts
    /// nothing. Two sources the store canonicalises to one resource MUST
    /// return the same [`ImageId`] — that is the only place cross-source
    /// bitmap reuse can happen.
    ///
    /// For every id it hands out, the store eventually calls exactly one of
    /// [`ImageSink::loaded`] or [`ImageSink::failed`], unless the view is torn
    /// down first. It may call `loaded` again later with a greater generation
    /// when the bytes behind the id change.
    fn request(&self, source: &str) -> ImageId;

    /// The images the frame just encoded, deduplicated in paint order.
    ///
    /// Advisory: it informs residency and nothing else, and a store that
    /// ignores it is still correct. Called once per resolve pass.
    fn retain(&self, _frame: &[ImageRef]) {}

    /// No live node or style names `id` any more. Advisory; the store may keep
    /// the bitmap, and must answer a later [`Self::request`] for the same
    /// source as a fresh load if it did not.
    fn release(&self, _id: ImageId) {}
}

/// One report from the store, on its way to the document.
///
/// `Send` by construction: an `Arc<str>` and integers. There is no variant
/// that could carry pixels, which is what makes "`ImageData` never crosses a
/// channel" a property of the type rather than a rule to remember.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageEvent {
    /// The store's canonical id for a source the document asked about.
    Bound { source: Arc<str>, id: ImageId },
    /// Pixels exist, at this generation and these intrinsic dimensions.
    Loaded {
        id: ImageId,
        generation: u32,
        width: u32,
        height: u32,
    },
    /// This id will never produce pixels.
    Failed { id: ImageId },
}

/// What the document knows about one image. Never any pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageState {
    /// Requested; no pixels yet.
    Pending,
    /// The only drawable state, and the only one carrying a generation — so a
    /// frame naming an image that is not loaded is unrepresentable rather than
    /// filtered out later.
    Ready {
        generation: u32,
        width: u32,
        height: u32,
    },
    /// Terminal.
    Failed,
}

/// One source string's binding to a store id.
#[derive(Debug)]
enum Binding {
    /// Requested; the painter has not answered with an id yet.
    Unbound { nodes: SmallVec<[NodeId; 1]> },
    Bound {
        id: ImageId,
        nodes: SmallVec<[NodeId; 1]>,
    },
}

impl Binding {
    fn nodes_mut(&mut self) -> &mut SmallVec<[NodeId; 1]> {
        match self {
            Self::Unbound { nodes } | Self::Bound { nodes, .. } => nodes,
        }
    }
}

/// The document's whole image state: a name table.
///
/// Holds no pixels, no store and no buffers. Its entire job is to answer, for
/// a source string the paint walk just met, "what does a frame call this, and
/// how big is it" — and to request the ones it has never seen.
#[derive(Default)]
pub(crate) struct ImageRegistry {
    by_source: FxHashMap<Arc<str>, Binding>,
    slots: FxHashMap<ImageId, ImageState>,
    /// Sources met by a walk that have not been requested yet.
    ///
    /// `RefCell` because the paint walk takes the document shared, and the
    /// walk is exactly where sources are discovered. The document already
    /// holds a `RefCell` for its retained painter, so this costs no
    /// thread-safety property it had.
    wanted: RefCell<Vec<Arc<str>>>,
    /// Ids no live node or style names any more, awaiting a release message.
    released: Vec<ImageId>,
}

impl std::fmt::Debug for ImageRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageRegistry")
            .field("sources", &self.by_source.len())
            .field("slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

impl ImageRegistry {
    /// The draw key and intrinsic dimensions for `source`, requesting it if
    /// this is the first time the registry has seen it.
    ///
    /// `None` means "paint nothing this frame": the image is pending, failed,
    /// or not yet bound to an id. A pending image is the one-frame gap between
    /// a source appearing and its pixels arriving, which is what a browser
    /// shows for a not-yet-loaded image.
    pub(crate) fn resolve(&self, source: &str) -> Option<(ImageRef, (f64, f64))> {
        match self.by_source.get(source) {
            None => {
                // First sighting: ask for it. Taking `&self` is what lets this
                // run inside the walk, which is the only place that knows
                // which sources a frame actually needs.
                self.wanted.borrow_mut().push(Arc::from(source));
                None
            }
            Some(Binding::Unbound { .. }) => None,
            Some(Binding::Bound { id, .. }) => match self.slots.get(id) {
                Some(&ImageState::Ready {
                    generation,
                    width,
                    height,
                }) => Some((
                    ImageRef {
                        id: *id,
                        generation,
                    },
                    (f64::from(width), f64::from(height)),
                )),
                _ => None,
            },
        }
    }

    /// Records that `node` presents `source`, so a completed load knows whose
    /// natural size to update.
    pub(crate) fn bind_node(&mut self, source: &str, node: NodeId) {
        let key: Arc<str> = Arc::from(source);
        let entry = self.by_source.entry(key).or_insert_with(|| {
            self.wanted.get_mut().push(Arc::from(source));
            Binding::Unbound {
                nodes: SmallVec::new(),
            }
        });
        let nodes = entry.nodes_mut();
        if !nodes.contains(&node) {
            nodes.push(node);
        }
    }

    /// Drops `node`'s claim on `source`.
    pub(crate) fn unbind_node(&mut self, source: &str, node: NodeId) {
        if let Some(binding) = self.by_source.get_mut(source) {
            binding.nodes_mut().retain(|held| *held != node);
        }
    }

    /// The sources discovered since the last drain, deduplicated, for the
    /// painter to request. Empty on every commit that met no new image.
    ///
    /// A source is seen once per draw that names it, and the walk cannot
    /// record it in the table from a shared borrow, so the same string can
    /// arrive several times. Deduplicating here keeps that off the wire; the
    /// store's own single-flight would make it harmless anyway.
    pub(crate) fn take_wanted(&mut self) -> Vec<Arc<str>> {
        let mut wanted = std::mem::take(self.wanted.get_mut());
        wanted.sort_unstable();
        wanted.dedup();
        wanted
    }

    /// The ids no longer named by anything, for the painter to release.
    pub(crate) fn take_released(&mut self) -> Vec<ImageId> {
        std::mem::take(&mut self.released)
    }

    /// Applies one report from the store.
    ///
    /// Returns the nodes whose natural size the caller must now update — empty
    /// for everything except a load that lands on bound replaced nodes.
    pub(crate) fn apply(&mut self, event: &ImageEvent) -> SmallVec<[NodeId; 1]> {
        match event {
            ImageEvent::Bound { source, id } => {
                let nodes = match self.by_source.remove(source.as_ref()) {
                    Some(mut binding) => std::mem::take(binding.nodes_mut()),
                    None => SmallVec::new(),
                };
                self.by_source
                    .insert(Arc::clone(source), Binding::Bound { id: *id, nodes });
                self.slots.entry(*id).or_insert(ImageState::Pending);
                SmallVec::new()
            }
            ImageEvent::Loaded {
                id,
                generation,
                width,
                height,
            } => {
                // An image vello could never place is a failure, refused at
                // the one point it enters the engine: it is then never laid
                // out, never encoded, and never handed to the renderer.
                if *width == 0
                    || *height == 0
                    || *width > MAX_RENDERABLE_DIMENSION
                    || *height > MAX_RENDERABLE_DIMENSION
                {
                    return self.apply(&ImageEvent::Failed { id: *id });
                }
                let slot = self.slots.entry(*id).or_insert(ImageState::Pending);
                // Generations only move forward. A late report from a
                // superseded load cannot walk the content backwards.
                if let ImageState::Ready {
                    generation: current,
                    ..
                } = *slot
                    && *generation <= current
                {
                    return SmallVec::new();
                }
                *slot = ImageState::Ready {
                    generation: *generation,
                    width: *width,
                    height: *height,
                };
                self.nodes_for(*id)
            }
            ImageEvent::Failed { id } => {
                let slot = self.slots.entry(*id).or_insert(ImageState::Pending);
                // A load that already succeeded is not un-done by a late
                // failure: that would regress the document's state, which the
                // rest of the system is built on never happening.
                if !matches!(slot, ImageState::Ready { .. }) {
                    *slot = ImageState::Failed;
                }
                SmallVec::new()
            }
        }
    }

    /// The intrinsic dimensions already known for `source`, if it has loaded.
    pub(crate) fn dimensions_of(&self, source: &str) -> Option<(u32, u32)> {
        let Some(Binding::Bound { id, .. }) = self.by_source.get(source) else {
            return None;
        };
        match self.slots.get(id) {
            Some(&ImageState::Ready { width, height, .. }) => Some((width, height)),
            _ => None,
        }
    }

    fn nodes_for(&self, id: ImageId) -> SmallVec<[NodeId; 1]> {
        self.by_source
            .values()
            .find_map(|binding| match binding {
                Binding::Bound { id: bound, nodes } if *bound == id => Some(nodes.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use super::{ImageEvent, ImageId, ImageRegistry, MAX_RENDERABLE_DIMENSION, NoImages};
    use crate::render::image::FrameImages;

    fn id(raw: u32) -> ImageId {
        ImageId(NonZeroU32::new(raw).expect("a non-zero id"))
    }

    fn loaded(raw: u32, generation: u32, width: u32, height: u32) -> ImageEvent {
        ImageEvent::Loaded {
            id: id(raw),
            generation,
            width,
            height,
        }
    }

    fn bound(source: &str, raw: u32) -> ImageEvent {
        ImageEvent::Bound {
            source: Arc::from(source),
            id: id(raw),
        }
    }

    /// A document must still cross threads with a registry on it: the
    /// document is built on one thread and run on another.
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
            "however many draws named it, the store is asked once"
        );

        registry.apply(&bound("app:///a.png", 1));
        assert!(registry.resolve("app:///a.png").is_none(), "still pending");
        assert!(
            registry.take_wanted().is_empty(),
            "and a bound source is never asked for again"
        );
    }

    #[test]
    fn only_a_loaded_image_resolves_and_it_carries_its_own_dimensions() {
        let mut registry = ImageRegistry::default();
        let _ = registry.resolve("app:///a.png");
        registry.apply(&bound("app:///a.png", 7));
        assert!(registry.resolve("app:///a.png").is_none());
        registry.apply(&loaded(7, 1, 40, 20));
        let (image, dimensions) = registry
            .resolve("app:///a.png")
            .expect("a loaded image resolves");
        assert_eq!(image.id, id(7));
        assert_eq!(image.generation, 1);
        assert!((dimensions.0 - 40.0).abs() < f64::EPSILON);
        assert!((dimensions.1 - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn an_image_vello_could_never_place_is_a_load_failure() {
        for (width, height) in [
            (0, 4),
            (4, 0),
            (MAX_RENDERABLE_DIMENSION + 1, 4),
            (4, MAX_RENDERABLE_DIMENSION + 1),
        ] {
            let mut registry = ImageRegistry::default();
            let _ = registry.resolve("app:///huge.png");
            registry.apply(&bound("app:///huge.png", 1));
            registry.apply(&loaded(1, 1, width, height));
            assert!(
                registry.resolve("app:///huge.png").is_none(),
                "{width}x{height} must not reach vello"
            );
        }
    }

    #[test]
    fn a_loaded_image_never_regresses() {
        let mut registry = ImageRegistry::default();
        let _ = registry.resolve("app:///a.png");
        registry.apply(&bound("app:///a.png", 1));
        registry.apply(&loaded(1, 5, 10, 10));

        // A late failure from a superseded load does not un-load it.
        registry.apply(&ImageEvent::Failed { id: id(1) });
        assert!(registry.resolve("app:///a.png").is_some(), "still drawable");

        // Nor does a stale generation walk the content backwards.
        registry.apply(&loaded(1, 4, 99, 99));
        let (image, dimensions) = registry.resolve("app:///a.png").expect("still drawable");
        assert_eq!(image.generation, 5);
        assert!((dimensions.0 - 10.0).abs() < f64::EPSILON);

        // A newer generation does replace it.
        registry.apply(&loaded(1, 6, 20, 20));
        let (image, dimensions) = registry.resolve("app:///a.png").expect("still drawable");
        assert_eq!(image.generation, 6);
        assert!((dimensions.0 - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn two_sources_the_store_canonicalises_share_one_slot() {
        let mut registry = ImageRegistry::default();
        let _ = registry.resolve("app:///a.png");
        let _ = registry.resolve("app:///./a.png");
        registry.apply(&bound("app:///a.png", 3));
        registry.apply(&bound("app:///./a.png", 3));
        registry.apply(&loaded(3, 1, 8, 8));
        let first = registry.resolve("app:///a.png").expect("resolves");
        let second = registry.resolve("app:///./a.png").expect("resolves");
        assert_eq!(
            first.0, second.0,
            "one id and one generation means one decode and one atlas slot"
        );
    }

    #[test]
    fn a_load_reports_the_nodes_whose_natural_size_must_change() {
        let mut registry = ImageRegistry::default();
        let mut document = crate::Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.create_element("view", ());
        registry.bind_node("app:///a.png", root);
        registry.apply(&bound("app:///a.png", 1));
        let nodes = registry.apply(&loaded(1, 1, 12, 6));
        assert_eq!(nodes.as_slice(), [root], "the bound node relayouts");

        registry.unbind_node("app:///a.png", root);
        let nodes = registry.apply(&loaded(1, 2, 12, 6));
        assert!(nodes.is_empty(), "an unbound source relayouts nothing");
    }

    #[test]
    fn the_empty_pixel_source_draws_nothing() {
        assert!(
            NoImages
                .read(super::ImageRef {
                    id: id(1),
                    generation: 1,
                })
                .is_none()
        );
    }
}
