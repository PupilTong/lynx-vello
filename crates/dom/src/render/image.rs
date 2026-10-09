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
//!
//! # An element's two sources
//!
//! A replaced element names up to two of them: the picture it is for, and a
//! placeholder it draws until the first has pixels. They are independent
//! entries requested at the same time — a placeholder is not something the
//! element reaches for once its source fails. What the element draws is its
//! own source while that is loaded, the placeholder otherwise, and nothing
//! when neither is, so a loaded source suppresses the placeholder for good,
//! including one whose pixels land later. The element's natural size names
//! whichever bitmap that is, because `object-fit` resolves one against the
//! other.
//!
//! Only the element's own source produces an [`ImageOutcome`], which is what
//! an embedder turns into a `load` or an `error`. A placeholder is an interim
//! picture the page did not ask about, so neither of its endings is an event.
//!
//! # Vector images
//!
//! An SVG document is not pixels, and a host does not parse it. It reports
//! the fetched bytes and their [`DocumentKind`] through
//! [`ImageReports::loaded_document`], which travel to the document thread
//! inside [`ImageEvent::LoadedDocument`]. The engine parses them into a
//! [`VectorImage`] (a `usvg` tree plus its natural size and viewport) with
//! [`VectorImage::parse_sealed`], reached from outside this crate only
//! through [`ImageEvent::parse_document`]: natively `bobcat-core` parses on its
//! engine thread's blocking pool and applies the result as the
//! already-parsed [`ImageEvent::LoadedVector`]; where nothing parses first
//! (this crate's own tests, the wasm32 build),
//! [`Document::apply_image_events`](crate::Document::apply_image_events)
//! parses inline. The registry keeps the tree in the loaded entry, and a
//! document that does not parse marks its source failed. The paint walk
//! encodes the tree straight into the fragment scene, the way
//! it encodes a gradient, through the scene [`VectorImage::scene`] builds once
//! (`paint/svg.rs`). A vector image is therefore never an image draw:
//! [`FrameImages`] is never asked for it, and no bitmap budget applies.
//!
//! The document is the second producer of vector images: an inline `<svg>`
//! root's subtree is serialised and parsed through the same
//! [`ImageEvent::parse_document`] inline on the document thread, and filed
//! under a synthetic source ([`SYNTHETIC_SOURCE_PREFIX`]) the registry
//! creates already settled, so the host never sees it
//! (`tree/inline_svg.rs`). Synthetic entries are the only ones the registry
//! ever removes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use vello::peniko::ImageData;

use crate::NodeId;
use crate::vello::Scene;

/// A parsed SVG document, drawable at any size.
///
/// Built by the engine, with `VectorImage::parse_sealed` (reached through
/// [`ImageEvent::parse_document`]), from the bytes a host reported through
/// [`ImageReports::loaded_document`]. It carries two sizes, both computed
/// at the parse from the root element's `width`, `height` and `viewBox`:
///
/// - the **natural size**, in whole CSS px, which layout reads exactly as it reads a bitmap's
///   intrinsic size (CSS Images 3 default sizing; see `docs/svg-vector-images-design.md`);
/// - the **viewport**, the rectangle in tree units that a draw maps onto its destination rectangle.
///   A draw scales by `extent / viewport` per axis, never by `extent / tree.size()`: `usvg`
///   overwrites `size()` with the content bounding box for a root that has neither a `viewBox` nor
///   an absolute dimension.
///
/// The vello scene the tree encodes to is built on the first draw and kept
/// for the image's life, so every later draw is one `Scene::append`. Cloning
/// shares the tree; a clone made before the first draw builds its own scene.
#[derive(Clone)]
pub struct VectorImage {
    tree: Arc<usvg::Tree>,
    natural: (u32, u32),
    viewport: (f32, f32),
    aspect: AspectRatio,
    key: u64,
    scene: OnceLock<Arc<Scene>>,
}

/// One side of a `preserveAspectRatio` alignment: where the viewport sits
/// along an axis of a box with another ratio.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)] // The converter writes these once it parses `preserveAspectRatio`.
pub(crate) enum AspectAlign {
    Min,
    #[default]
    Mid,
    Max,
}

/// The root's `preserveAspectRatio` (SVG 2 §8.6): how the viewport maps
/// onto a destination box whose ratio differs. `align: None` is `none`, a
/// stretch per axis; otherwise the viewport is scaled uniformly to `meet`
/// (fit inside) or `slice` (cover) the box and aligned per axis. The
/// default is `xMidYMid meet`.
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

/// The source of every [`VectorImage::key`]: a counter, so two images
/// never share a key within a process.
static NEXT_VECTOR_KEY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The tree crosses from the thread that parsed it (natively a blocking-pool
/// thread) to the document's inside an [`ImageEvent`], and the cached scene
/// is published inside the registry, so both must be `Send + Sync`.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<usvg::Tree>();
    assert_send_sync::<Scene>();
    assert_send_sync::<VectorImage>();
};

impl VectorImage {
    /// A vector image over `tree`, laid out at `natural` CSS px and mapping
    /// the `viewport` rectangle of tree units onto each draw.
    #[must_use]
    pub(crate) fn new(tree: Arc<usvg::Tree>, natural: (u32, u32), viewport: (f32, f32)) -> Self {
        Self {
            tree,
            natural,
            viewport,
            aspect: AspectRatio::default(),
            key: NEXT_VECTOR_KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            scene: OnceLock::new(),
        }
    }

    /// A process-unique identity for this image's picture: the painter's
    /// raster cache keys its textures by it and a device size. Clones share
    /// it, because they share the picture.
    #[must_use]
    #[allow(dead_code)] // Read by the painter's raster cache once it exists.
    pub(crate) fn key(&self) -> u64 {
        self.key
    }

    /// How the viewport maps onto a destination box of another ratio.
    #[must_use]
    #[allow(dead_code)] // Read by the painter's raster cache once it exists.
    pub(crate) fn aspect(&self) -> AspectRatio {
        self.aspect
    }

    /// Whether drawing the picture opens a blend layer with no isolating
    /// layer of its own around it, so the layer a draw of the whole image
    /// opens must be a full `Normal` layer rather than a clip layer (vello
    /// [#1198](https://github.com/linebender/vello/issues/1198)).
    #[must_use]
    pub(crate) fn opens_blend(&self) -> bool {
        crate::paint::svg::opens_blend(self.tree.root())
    }

    /// Parses the SVG document `svg`, and computes its natural size and
    /// viewport from the root element's `width`, `height` and `viewBox`.
    ///
    /// The options read nothing outside the document: the `<image>` string
    /// resolver (`resolve_string`) returns `None`, so an `<image>` naming
    /// anything but a `data:` URL resolves to nothing, and `resources_dir` is
    /// `None`. Every other option is usvg's default; `data:` URLs still
    /// resolve. This is the parse the engine runs on every document a host
    /// reports through [`ImageReports::loaded_document`]; usvg's default
    /// string resolver would read the filesystem.
    ///
    /// One XML parse serves both sizes and the tree: the root's attributes
    /// are read from the same `roxmltree` document
    /// [`usvg::Tree::from_xmltree`] converts, because the tree keeps neither
    /// the `viewBox` nor the raw dimensions. The input is handled as
    /// [`usvg::Tree::from_data`] handles it without the `svgz` feature: gzip
    /// data fails with [`usvg::Error::SvgzFeatureNotEnabled`], anything that
    /// is not UTF-8 with [`usvg::Error::NotAnUtf8Str`], and a DTD is allowed.
    ///
    /// The sizes follow `docs/svg-vector-images-design.md`. A root `width`
    /// or `height` is absolute when it is a bare number or a length in one of
    /// the CSS absolute units `px`, `in`, `cm`, `mm`, `pt` or `pc`, converted
    /// to px at 96 px per inch. A font-relative length (`em`, `ex`), a
    /// percentage, any other unit, or a missing attribute counts as absent.
    ///
    /// - Both absolute: natural = that size, viewport = `tree.size()`.
    /// - One absolute plus a `viewBox`: the other axis from the `viewBox` ratio, viewport =
    ///   `tree.size()`.
    /// - A `viewBox` only: natural = the largest size with the `viewBox` ratio that fits 300x150,
    ///   viewport = `tree.size()`.
    /// - Neither: natural and viewport 300x150. usvg leaves user units 1:1 here and overwrites
    ///   `size()` with the content bounding box, so `size()` is not the viewport.
    /// - One absolute and no `viewBox`: that axis, with 300 wide or 150 high for the other; the
    ///   viewport is the natural size, for the same reason as the case above.
    ///
    /// The natural size is rounded to whole px, at least 1 on each axis.
    ///
    /// # Errors
    ///
    /// Whatever usvg reports for a document it cannot read.
    pub(crate) fn parse_sealed(svg: &[u8]) -> Result<Self, usvg::Error> {
        if svg.starts_with(&[0x1f, 0x8b]) {
            return Err(usvg::Error::SvgzFeatureNotEnabled);
        }
        let text = std::str::from_utf8(svg).map_err(|_| usvg::Error::NotAnUtf8Str)?;
        let document = usvg::roxmltree::Document::parse_with_options(
            text,
            usvg::roxmltree::ParsingOptions {
                allow_dtd: true,
                ..usvg::roxmltree::ParsingOptions::default()
            },
        )
        .map_err(usvg::Error::ParsingFailed)?;
        let root = document.root_element();
        let width = root.attribute("width").and_then(absolute_length);
        let height = root.attribute("height").and_then(absolute_length);
        let view_box = root.attribute("viewBox").and_then(view_box_size);
        let options = usvg::Options {
            resources_dir: None,
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_string: Box::new(|_, _| None),
                ..usvg::ImageHrefResolver::default()
            },
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_xmltree(&document, &options)?;
        let (natural, viewport) = vector_sizes(
            width,
            height,
            view_box,
            (tree.size().width(), tree.size().height()),
        );
        Ok(Self::new(Arc::new(tree), natural, viewport))
    }

    /// The size layout is told, in whole CSS px.
    #[must_use]
    pub(crate) fn natural_size(&self) -> (u32, u32) {
        self.natural
    }

    /// The rectangle in tree units a draw maps onto its destination.
    #[must_use]
    pub(crate) fn viewport(&self) -> (f32, f32) {
        self.viewport
    }

    /// The parsed document.
    #[allow(dead_code)] // Goes with `usvg` when the own converter lands.
    pub(crate) fn tree(&self) -> &usvg::Tree {
        &self.tree
    }

    /// The tree encoded as a vello scene in tree units, built on the first
    /// call and returned from the cache after that.
    #[must_use]
    pub(crate) fn scene(&self) -> &Arc<Scene> {
        self.scene.get_or_init(|| {
            let mut scene = Scene::new();
            crate::paint::svg::encode(&self.tree, &mut scene);
            Arc::new(scene)
        })
    }
}

/// The default object size of CSS Images 3, in CSS px.
const DEFAULT_OBJECT_SIZE: (f32, f32) = (300.0, 150.0);

/// The natural size and viewport of an SVG document, from its root's
/// absolute `width` and `height`, its `viewBox` size and the parsed tree's
/// `size()`. The rule is [`VectorImage::parse_sealed`]'s.
fn vector_sizes(
    width: Option<f32>,
    height: Option<f32>,
    view_box: Option<(f32, f32)>,
    tree_size: (f32, f32),
) -> ((u32, u32), (f32, f32)) {
    let (natural, viewport_is_tree) = match (width, height, view_box) {
        (Some(width), Some(height), _) => ((width, height), true),
        (Some(width), None, Some((box_width, box_height))) => {
            ((width, width * box_height / box_width), true)
        }
        (None, Some(height), Some((box_width, box_height))) => {
            ((height * box_width / box_height, height), true)
        }
        (None, None, Some((box_width, box_height))) => {
            let scale = (DEFAULT_OBJECT_SIZE.0 / box_width).min(DEFAULT_OBJECT_SIZE.1 / box_height);
            ((box_width * scale, box_height * scale), true)
        }
        (width, height, None) => (
            (
                width.unwrap_or(DEFAULT_OBJECT_SIZE.0),
                height.unwrap_or(DEFAULT_OBJECT_SIZE.1),
            ),
            false,
        ),
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded and clamped to at least one before the cast, which saturates"
    )]
    let whole = |length: f32| length.round().max(1.0) as u32;
    let rounded = (whole(natural.0), whole(natural.1));
    let viewport = if viewport_is_tree {
        tree_size
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a natural size with no viewBox is an authored px length or the default"
        )]
        let viewport = (rounded.0 as f32, rounded.1 as f32);
        viewport
    };
    (rounded, viewport)
}

/// A root `width` or `height` that is an absolute length, in px: a bare
/// number, or a number in one of the CSS absolute units (`px`; `in` = 96 px,
/// `cm` = 96/2.54 px, `mm` = 96/25.4 px, `pt` = 4/3 px, `pc` = 16 px),
/// finite and positive. Anything else, `em`, `ex` and `%` included, counts
/// as absent.
fn absolute_length(value: &str) -> Option<f32> {
    const UNITS: [(&str, f32); 6] = [
        ("px", 1.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("pt", 4.0 / 3.0),
        ("pc", 16.0),
    ];
    let value = value.trim();
    let (number, scale) = UNITS
        .iter()
        .find_map(|&(unit, scale)| value.strip_suffix(unit).map(|number| (number, scale)))
        .unwrap_or((value, 1.0));
    number
        .parse::<f32>()
        .ok()
        .map(|length| length * scale)
        .filter(|length| length.is_finite() && *length > 0.0)
}

/// A `viewBox`'s width and height, when it is four numbers and both of those
/// are finite and positive.
fn view_box_size(value: &str) -> Option<(f32, f32)> {
    let numbers: SmallVec<[f32; 4]> = value
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|part| !part.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    match numbers.as_slice() {
        [_, _, width, height]
            if width.is_finite() && height.is_finite() && *width > 0.0 && *height > 0.0 =>
        {
            Some((*width, *height))
        }
        _ => None,
    }
}

impl std::fmt::Debug for VectorImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VectorImage")
            .field("natural", &self.natural)
            .field("viewport", &self.viewport)
            .field("scene_built", &self.scene.get().is_some())
            .finish_non_exhaustive()
    }
}

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

/// How large a frame draws an image, in device pixels.
///
/// Per axis, the largest extent of one copy of the source across every draw
/// in the frame that names it — a tiled background counts one tile, a
/// `cover`-fitted replaced element the fitted rect — under the draw's own
/// transform, so a scaled or high-DPR draw asks for more pixels than its CSS
/// size says. It is the one fact a host needs to size a decode: a bitmap
/// larger than the draw is resampled down at composition and pays its memory
/// for nothing, while one smaller than the draw is upsampled and blurs. The
/// engine composes correctly against either, so the hint is advisory — a
/// host may decode at it, below it, or ignore it.
///
/// [`ImageSizeHint::UNBOUNDED`] names a read with no frame behind it, where
/// the only right answer is the image's own size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageSizeHint {
    pub width: u32,
    pub height: u32,
}

impl ImageSizeHint {
    /// No bound on either axis.
    pub const UNBOUNDED: Self = Self {
        width: u32::MAX,
        height: u32::MAX,
    };

    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Whether this hint bounds nothing.
    #[must_use]
    pub const fn is_unbounded(self) -> bool {
        self.width == u32::MAX && self.height == u32::MAX
    }

    /// The hint covering both this one and `other`: per-axis maximum, so a
    /// source drawn at two sizes in one frame is decoded for the larger.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            width: self.width.max(other.width),
            height: self.height.max(other.height),
        }
    }

    /// The size to decode a `width`x`height` image to under this hint: the
    /// largest size inside the hint that keeps the image's ratio, never
    /// larger than the image itself, never smaller than one pixel.
    ///
    /// This is the whole downsampling decision, kept beside the type so every
    /// host makes it the same way.
    #[must_use]
    pub fn fit(self, width: u32, height: u32) -> (u32, u32) {
        if self.is_unbounded() || (width <= self.width && height <= self.height) {
            return (width.max(1), height.max(1));
        }
        let scale = (f64::from(self.width) / f64::from(width.max(1)))
            .min(f64::from(self.height) / f64::from(height.max(1)));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the product is at most the image's own axis, a u32, and floored positive"
        )]
        let axis = |length: u32| ((f64::from(length) * scale).floor() as u32).max(1);
        (axis(width), axis(height))
    }
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
    /// host has already reported loaded. `hint` is how large the frame draws
    /// it; a store that decodes on demand sizes its decode from it, and one
    /// that already holds pixels may ignore it. The bitmap need not match
    /// the hint or the intrinsic size that was reported with the load: a
    /// reduced-scale decode composes correctly.
    fn read(&self, source: &str, hint: ImageSizeHint) -> Option<ImageData>;

    /// The sources the frame just resolved, deduplicated in paint order.
    ///
    /// Advisory: it informs residency and nothing else, and a store that
    /// ignores it is still correct. Called once per resolve pass, immediately
    /// after the reads that pass made.
    ///
    /// There is no default, deliberately. A store that keeps bitmaps has to
    /// decide what its working set is, and a silent no-op inherited from the
    /// trait is the one answer that cannot be reviewed — writing `{}` says
    /// the same thing where someone can see it.
    fn retain(&self, frame: &[Arc<str>]);
}

/// Composes every image draw as nothing: the pixel source for a scene built
/// without an embedder, and for the many call sites that render pages with no
/// images at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoImages;

/// A shared handle serves whatever it points at, so an embedder holding its
/// resource system behind an [`Rc`] needs no forwarding wrapper of its own.
///
/// `Rc` rather than `Arc` because a resource system never leaves the painter's
/// thread; an atomic count here would be paid on every clone and never used.
impl<T: FrameImages + ?Sized> FrameImages for Rc<T> {
    fn read(&self, source: &str, hint: ImageSizeHint) -> Option<ImageData> {
        (**self).read(source, hint)
    }

    fn retain(&self, frame: &[Arc<str>]) {
        (**self).retain(frame);
    }
}

impl FrameImages for NoImages {
    fn read(&self, _source: &str, _hint: ImageSizeHint) -> Option<ImageData> {
        None
    }

    /// Nothing is held, so there is no working set to narrow.
    fn retain(&self, _frame: &[Arc<str>]) {}
}

/// Completed loads waiting for the painter to take its next turn.
///
/// The buffer exists for re-entrancy and teardown, not for synchronisation: a
/// store may report from inside the `request_image` the painter is in the
/// middle of calling, so a report cannot land directly in the `&mut` path
/// that asked for it.
#[derive(Debug, Default)]
struct ImageQueue {
    events: RefCell<Vec<ImageEvent>>,
    detached: Cell<bool>,
}

/// The host's end: how a store says a load finished.
///
/// A concrete handle rather than a trait object, so reporting costs a direct
/// call and no vtable exists to dispatch through. It is thread-bound by
/// construction — an [`Rc`] cannot be moved to another thread — so a host
/// that decodes off-thread must marshal completions back to the painter's
/// thread itself, which costs it nothing it does not already have: it drives
/// the painter's turns, so it drains its own completions just before the next
/// one.
///
/// Waking the painter is the host's too. A report made between turns needs a
/// turn to be drained, and the host holds the wakeup it gave the view — it
/// knows whether it reported inside a turn, where the wake would be
/// wasted, or outside one, where it is needed.
#[derive(Clone, Debug)]
pub struct ImageReports {
    queue: Rc<ImageQueue>,
}

impl ImageReports {
    /// `source`'s bytes are readable, and the image's own full-resolution
    /// size is `width` x `height`.
    ///
    /// Reported once per source: one URL has one content, so a second report
    /// for a source already resolved changes nothing. There is deliberately
    /// no way to say that an image stopped being loaded — eviction is
    /// invisible here, which is what keeps a document's state from
    /// regressing.
    ///
    /// Neither dimension is bounded. This is the image's intrinsic size,
    /// which layout uses; what a host may actually decode to is bounded, and
    /// checked where the pixels are read.
    ///
    /// Non-blocking, and it must not re-enter the store.
    pub fn loaded(&self, source: &str, width: u32, height: u32) {
        self.post(ImageEvent::Loaded {
            source: Arc::from(source),
            width,
            height,
        });
    }

    /// `source` is a document of `kind` the engine draws itself, and `bytes`
    /// are its encoded bytes as fetched (after any preprocessing that leaves
    /// an image's bytes unchanged).
    ///
    /// The host does not parse: the engine does, and a document it cannot
    /// read becomes a failure of the source on the engine's side. Otherwise
    /// the same contract as [`ImageReports::loaded`]: reported once per
    /// source, never retracted. A host may answer a later request for the
    /// same source with the same bytes again; the document's registry makes
    /// the repeat a no-op.
    ///
    /// Non-blocking, and it must not re-enter the store.
    pub fn loaded_document(&self, source: &str, bytes: Bytes, kind: DocumentKind) {
        self.post(ImageEvent::LoadedDocument {
            source: Arc::from(source),
            bytes,
            kind,
        });
    }

    /// `source` will not produce pixels. Terminal; the engine does not retry.
    pub fn failed(&self, source: &str) {
        self.post(ImageEvent::Failed {
            source: Arc::from(source),
        });
    }

    fn post(&self, event: ImageEvent) {
        // A load completing against a torn-down view is dropped rather than
        // buffered into a painter that will never read it.
        if self.queue.detached.get() {
            return;
        }
        self.queue.events.borrow_mut().push(event);
    }
}

/// The painter's end: where reports are taken from.
///
/// Separate from [`ImageReports`] so each side holds only what it may do —
/// the host can report and not drain, the painter can drain and not report.
#[derive(Debug)]
pub struct ImageInbox {
    queue: Rc<ImageQueue>,
}

impl ImageInbox {
    /// The host end this inbox receives from, and the inbox itself.
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

    /// Takes everything reported since the last drain.
    #[must_use]
    pub fn drain(&self) -> Vec<ImageEvent> {
        std::mem::take(&mut *self.queue.events.borrow_mut())
    }

    /// Stops accepting reports, at teardown. The host may still hold its
    /// [`ImageReports`] and call it; the calls do nothing.
    ///
    /// The flag lives on the shared queue rather than on the host's handle
    /// because the painter cannot reach into the store it handed that handle
    /// to.
    pub fn detach(&self) {
        self.queue.detached.set(true);
    }
}

/// Which kind of document a host reported through
/// [`ImageReports::loaded_document`]: what the engine parses the bytes as.
///
/// Non-exhaustive so that a second engine-drawn format is one more variant
/// rather than a second protocol method.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    /// An SVG document (`image/svg+xml`), parsed by
    /// [`ImageEvent::parse_document`].
    Svg,
}

/// One report from the store, on its way to the document.
///
/// `Send` by construction: an `Arc<str>`, integers, [`Bytes`] and a
/// [`VectorImage`], whose tree is `Send + Sync`. Unlike the sink, this really
/// does cross a thread — the painter forwards a batch of these to the Lynx
/// main thread — and the `Arc`s are what make that legal. There is no variant
/// that could carry pixels (encoded document bytes and a parsed tree are not
/// pixels), which is what makes "`ImageData` never crosses a channel" a
/// property of the type rather than a rule to remember.
///
/// Not `PartialEq`: a [`VectorImage`] has no meaningful equality, and every
/// consumer matches events by pattern.
#[derive(Clone, Debug)]
pub enum ImageEvent {
    /// Pixels exist for this source, with these intrinsic dimensions.
    Loaded {
        source: Arc<str>,
        width: u32,
        height: u32,
    },
    /// This source is a document of `kind` the engine draws itself, and
    /// these are its encoded bytes. What a host reports
    /// ([`ImageReports::loaded_document`]); the engine parses it, and it ends
    /// as [`ImageEvent::LoadedVector`] or [`ImageEvent::Failed`].
    LoadedDocument {
        source: Arc<str>,
        bytes: Bytes,
        kind: DocumentKind,
    },
    /// This source is an SVG document, already parsed. Engine-internal: no
    /// host reports it. `bobcat-core` produces it from a
    /// [`ImageEvent::LoadedDocument`] it parsed off the document thread
    /// ([`ImageEvent::parse_document`]). Its natural size is the intrinsic
    /// size layout reads.
    LoadedVector {
        source: Arc<str>,
        image: VectorImage,
    },
    /// This source will never produce pixels.
    Failed { source: Arc<str> },
}

impl ImageEvent {
    /// The already-parsed event a [`ImageEvent::LoadedDocument`] for
    /// `source` with `bytes` of `kind` ends as: [`ImageEvent::LoadedVector`]
    /// when the document parses, [`ImageEvent::Failed`] when it does not.
    ///
    /// The parse is `VectorImage::parse_sealed`, which reads nothing
    /// outside the document. It runs on the calling thread and may take as
    /// long as the document is large, so a caller with a blocking pool runs
    /// it there.
    #[must_use]
    pub fn parse_document(source: Arc<str>, bytes: &[u8], kind: DocumentKind) -> Self {
        match parse_document(bytes, kind) {
            Some(image) => Self::LoadedVector { source, image },
            None => Self::Failed { source },
        }
    }

    /// The source this event reports on.
    fn source(&self) -> &Arc<str> {
        match self {
            Self::Loaded { source, .. }
            | Self::LoadedDocument { source, .. }
            | Self::LoadedVector { source, .. }
            | Self::Failed { source } => source,
        }
    }
}

/// Parses `bytes` as a document of `kind`; `None` for one that does not
/// parse. The error itself goes nowhere: the source's failure is what the
/// document records, and the engine has no log to write the message to.
fn parse_document(bytes: &[u8], kind: DocumentKind) -> Option<VectorImage> {
    match kind {
        DocumentKind::Svg => VectorImage::parse_sealed(bytes).ok(),
    }
}

/// The state a parsed vector image settles its source in. A zero axis is a
/// failure, as it is for a bitmap; [`VectorImage::parse_sealed`] never produces
/// one, so the check guards [`VectorImage::new`]'s callers.
fn vector_state(image: VectorImage) -> ImageState {
    let (width, height) = image.natural_size();
    if width > 0 && height > 0 {
        ImageState::Ready {
            width,
            height,
            kind: ImageKind::Vector(image),
        }
    } else {
        ImageState::Failed
    }
}

/// What the document knows about one image. Never any pixels.
///
/// `Pending` is the only state with outgoing edges; both others are sinks,
/// which is the whole of "the document's image state never regresses".
#[derive(Debug, Default)]
enum ImageState {
    /// Asked for; no pixels yet.
    #[default]
    Pending,
    /// The only drawable state — so a frame naming an image that is not
    /// loaded is unrepresentable rather than filtered out later.
    Ready {
        width: u32,
        height: u32,
        kind: ImageKind,
    },
    /// Terminal.
    Failed,
}

/// What a loaded image draws from.
#[derive(Debug)]
enum ImageKind {
    /// Pixels the host holds, read through [`FrameImages`] at compose time.
    Raster,
    /// A parsed SVG document, encoded into the frame on the document thread.
    Vector(VectorImage),
}

/// Which of a replaced element's two sources a binding is.
///
/// A node may hold one URL in both roles, so a binding is the pair and not
/// the node alone — and it is what
/// [`Document::set_image_source`](crate::Document::set_image_source) names to
/// say which of the two it is writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRole {
    /// The picture the element is for.
    Source,
    /// The picture it shows until [`ImageRole::Source`] has pixels.
    Placeholder,
}

/// One element's own image source settling, for the embedder to turn into a
/// `load` or an `error`.
///
/// A placeholder produces none: it is an interim picture the page did not ask
/// about, so neither of its outcomes is an event. That asymmetry is the type's
/// — there is no variant naming a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageOutcome {
    /// The element's source has loaded, as pixels or as a parsed vector
    /// document, with this intrinsic size.
    Loaded {
        node: NodeId,
        width: u32,
        height: u32,
    },
    /// The element's source will never produce pixels.
    Failed { node: NodeId },
}

/// What [`ImageRegistry::resolve`] hands a draw: the source's name (the
/// registry's own key), its intrinsic dimensions, and its parsed tree when it
/// is a vector image.
pub(crate) type Resolved<'a> = (Arc<str>, (f64, f64), Option<&'a VectorImage>);

/// What applying one report moved.
pub(crate) struct ImageApplied {
    /// The intrinsic size the source ended up with, or `None` for one that
    /// will never produce pixels. Not the reported size: a report with a zero
    /// axis is a failure.
    pub(crate) loaded: Option<(u32, u32)>,
    /// Every replaced node holding this source, in whichever role.
    pub(crate) nodes: SmallVec<[(NodeId, ImageRole); 1]>,
}

/// The prefix of every synthetic source: the one an inline SVG root is bound
/// to (`tree::inline_svg`), `inline-svg:<node>:<generation>`. The registry
/// creates such an entry already settled, so the host is never asked for
/// it, and removes it when the root rebinds or is freed.
///
/// A host URL that happened to start with this prefix would be forgotten
/// with the node that last presented it; no host scheme does.
pub(crate) const SYNTHETIC_SOURCE_PREFIX: &str = "inline-svg:";

/// Whether `source` is a synthetic source ([`SYNTHETIC_SOURCE_PREFIX`]).
pub(crate) fn is_synthetic_source(source: &str) -> bool {
    source.starts_with(SYNTHETIC_SOURCE_PREFIX)
}

/// What the registry holds for one source.
#[derive(Debug, Default)]
struct Entry {
    state: ImageState,
    /// Replaced nodes presenting this source, so a completed load knows whose
    /// natural size to recompute and whose element owes an event. A
    /// `background-image` user is not here: it has no natural size, and the
    /// load invalidates the frame anyway.
    nodes: SmallVec<[(NodeId, ImageRole); 1]>,
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
/// but has no key, because the key *is* the source. A synthetic source
/// ([`SYNTHETIC_SOURCE_PREFIX`]) is the one exception: its entry is created
/// settled by the document itself ([`ImageRegistry::insert_synthetic`]), and
/// existing is exactly what keeps it from ever being asked for.
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
    /// The draw name and intrinsic dimensions for `source`, and its parsed
    /// tree when it is a vector image, requesting it if this is the
    /// registry's first sighting.
    ///
    /// `None` means "paint nothing this frame": pending, or failed. A pending
    /// image is the one-frame gap between a source appearing and its pixels
    /// arriving, which is what a browser shows for a not-yet-loaded image.
    ///
    /// The name handed back is the map's own key, so every draw of one source
    /// in one frame shares one allocation. The vector image is a borrow of
    /// the entry's own, so a draw clones nothing.
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
                    ImageKind::Raster => None,
                    ImageKind::Vector(image) => Some(image),
                },
            )),
            ImageState::Pending | ImageState::Failed => None,
        }
    }

    /// What a replaced element holding these two sources draws: its own
    /// source once that has pixels, its placeholder until then, nothing when
    /// neither has any. Requests either on a first sighting, as
    /// [`ImageRegistry::resolve`] does.
    pub(crate) fn resolve_presented(
        &self,
        source: Option<&str>,
        placeholder: Option<&str>,
    ) -> Option<Resolved<'_>> {
        source
            .and_then(|source| self.resolve(source))
            .or_else(|| placeholder.and_then(|placeholder| self.resolve(placeholder)))
    }

    /// The entry for `source`, asking for it if this is the registry's first
    /// sighting.
    ///
    /// Taking `&self` is what lets this run inside the walk, the only place
    /// that knows which sources a frame actually needs.
    fn sight(&self, source: &str) -> Option<(&Arc<str>, &Entry)> {
        let found = self.entries.get_key_value(source);
        if found.is_none() {
            // Deduplicated here rather than on the way out: a list of 200 rows
            // sharing one `url(...)` resolves it 200 times on its first
            // commit, and allocating a copy of the URL per *draw* to request
            // one image is the wrong shape on the most latency-sensitive
            // commit there is. The list is tiny, so the scan beats a set.
            let mut wanted = self.wanted.borrow_mut();
            if !wanted.iter().any(|pending| &**pending == source) {
                wanted.push(Arc::from(source));
            }
        }
        found
    }

    /// Records that `node` presents `source` in `role`, asking for it if this
    /// is the registry's first sighting.
    ///
    /// Binding has to ask, not merely record. The entry it is about to create
    /// is exactly what [`ImageRegistry::resolve`] reads as "already asked
    /// for", so an entry created without a request would suppress the only
    /// request that source would ever get — and a replaced element binds its
    /// source in the same call that makes it replaced, always before any walk
    /// could have resolved it.
    pub(crate) fn bind_node(&mut self, source: &str, node: NodeId, role: ImageRole) {
        if !self.entries.contains_key(source) {
            // Deduplicated against a walk that met the same source first and
            // whose request has not been drained yet, the same way `resolve`
            // deduplicates against itself.
            let wanted = self.wanted.get_mut();
            if !wanted.iter().any(|pending| &**pending == source) {
                wanted.push(Arc::from(source));
            }
        }
        let entry = self.entry_for(source);
        if !entry.nodes.contains(&(node, role)) {
            entry.nodes.push((node, role));
        }
    }

    /// Drops `node`'s claim on `source` in `role`. Its claim in the other
    /// role, which an element naming one URL twice has, survives.
    pub(crate) fn unbind_node(&mut self, source: &str, node: NodeId, role: ImageRole) {
        if let Some(entry) = self.entries.get_mut(source) {
            entry.nodes.retain(|held| *held != (node, role));
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
    /// with one content makes a no-op.
    ///
    /// A [`ImageEvent::LoadedDocument`] is parsed here, on the calling
    /// thread, and only when its source is still pending: this is the path
    /// for a caller with no blocking pool to parse on first (this crate's
    /// tests, the wasm32 build). A document that does not parse is a
    /// failure, exactly as a [`ImageEvent::Failed`] report would be.
    pub(crate) fn apply(&mut self, event: &ImageEvent) -> Option<ImageApplied> {
        let entry = self.entry_for(event.source());
        if !matches!(entry.state, ImageState::Pending) {
            return None;
        }
        let state = match event {
            // Well-formedness, not the atlas bound: an image with a zero axis
            // has neither an intrinsic size nor an aspect ratio, so it would
            // stretch an unknown bitmap over the whole content box, and it is
            // refused as a failure. `MAX_RENDERABLE_DIMENSION` is deliberately
            // not tested here — see `is_renderable`, which tests the bitmap
            // instead.
            ImageEvent::Loaded { width, height, .. } if *width > 0 && *height > 0 => {
                ImageState::Ready {
                    width: *width,
                    height: *height,
                    kind: ImageKind::Raster,
                }
            }
            ImageEvent::LoadedVector { image, .. } => vector_state(image.clone()),
            ImageEvent::LoadedDocument { bytes, kind, .. } => {
                parse_document(bytes, *kind).map_or(ImageState::Failed, vector_state)
            }
            ImageEvent::Loaded { .. } | ImageEvent::Failed { .. } => ImageState::Failed,
        };
        let loaded = match &state {
            ImageState::Ready { width, height, .. } => Some((*width, *height)),
            ImageState::Pending | ImageState::Failed => None,
        };
        entry.state = state;
        Some(ImageApplied {
            loaded,
            nodes: entry.nodes.clone(),
        })
    }

    /// How `source` has already settled, as the outcome `node` binding to it
    /// now is owed.
    ///
    /// `None` while the source is still pending, where the report itself will
    /// carry the outcome. A source that settled before this bind is reported
    /// by nothing else ever again — one URL is reported once — so this is the
    /// only place a second mount of a known URL can learn what it got.
    pub(crate) fn outcome_for(&self, source: &str, node: NodeId) -> Option<ImageOutcome> {
        match &self.entries.get(source)?.state {
            ImageState::Pending => None,
            ImageState::Ready { width, height, .. } => Some(ImageOutcome::Loaded {
                node,
                width: *width,
                height: *height,
            }),
            ImageState::Failed => Some(ImageOutcome::Failed { node }),
        }
    }

    /// The intrinsic dimensions of that same bitmap, for the natural size the
    /// element carries between paints.
    ///
    /// It has to make [`ImageRegistry::resolve_presented`]'s choice and no
    /// other: `object-fit` reads the natural size against the bitmap actually
    /// drawn. Separate from it only because this one asks for nothing — it
    /// runs where a source change or a report is being settled, not where a
    /// frame is being built.
    pub(crate) fn presented_dimensions(
        &self,
        source: Option<&str>,
        placeholder: Option<&str>,
    ) -> Option<(u32, u32)> {
        source
            .and_then(|source| self.dimensions_of(source))
            .or_else(|| placeholder.and_then(|placeholder| self.dimensions_of(placeholder)))
    }

    /// The intrinsic dimensions already known for `source`, if it has loaded.
    fn dimensions_of(&self, source: &str) -> Option<(u32, u32)> {
        match &self.entries.get(source)?.state {
            ImageState::Ready { width, height, .. } => Some((*width, *height)),
            ImageState::Pending | ImageState::Failed => None,
        }
    }

    /// Files a synthetic source ([`SYNTHETIC_SOURCE_PREFIX`]) already
    /// settled by `event`, before any node binds it: an entry that exists
    /// is never queued as wanted, so no paint walk and no bind asks the host
    /// for it.
    pub(crate) fn insert_synthetic(&mut self, event: &ImageEvent) {
        debug_assert!(
            is_synthetic_source(event.source()),
            "only a synthetic source is inserted settled"
        );
        self.entries
            .insert(Arc::clone(event.source()), Entry::default());
        let _ = self.apply(event);
    }

    /// Removes a synthetic source's entry: a superseded generation of an
    /// inline SVG root, or the source of a freed one. The one removal the
    /// registry makes; a host source never regresses.
    pub(crate) fn forget_synthetic(&mut self, source: &str) {
        debug_assert!(
            is_synthetic_source(source),
            "only a synthetic source is ever removed: {source}"
        );
        self.entries.remove(source);
    }

    /// Whether the registry holds an entry for `source`.
    pub(crate) fn knows(&self, source: &str) -> bool {
        self.entries.contains_key(source)
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
    /// Regression: a replaced element binds its source in the same call that
    /// makes it replaced — before any walk can resolve it — so binding is the
    /// first sighting. If binding creates the entry without asking, `resolve`
    /// reads that entry as "already asked for" and the source is never
    /// requested from the host: the image never loads, with no error.
    #[test]
    fn binding_a_node_asks_for_its_source() {
        let mut registry = ImageRegistry::default();
        registry.bind_node("app:///a.png", node(1), ImageRole::Source);

        assert!(
            registry.resolve("app:///a.png").is_none(),
            "a bound source has no pixels yet"
        );
        assert_eq!(
            registry.take_wanted(),
            vec![Arc::<str>::from("app:///a.png")],
            "binding asks the host for the source"
        );
        assert!(registry.take_wanted().is_empty(), "and asks exactly once");
    }

    /// A walk that met the source first has already queued it; binding a node
    /// to it must not queue it twice.
    #[test]
    fn binding_a_source_a_walk_already_met_asks_once() {
        let mut registry = ImageRegistry::default();
        assert!(registry.resolve("app:///a.png").is_none());
        registry.bind_node("app:///a.png", node(1), ImageRole::Source);

        assert_eq!(registry.take_wanted().len(), 1, "one source, one request");
    }

    /// Two elements presenting one source ask for it once between them.
    #[test]
    fn two_nodes_on_one_source_ask_once() {
        let mut registry = ImageRegistry::default();
        registry.bind_node("app:///a.png", node(1), ImageRole::Source);
        registry.bind_node("app:///a.png", node(2), ImageRole::Source);

        assert_eq!(registry.take_wanted().len(), 1);
    }

    fn node(bits: u64) -> crate::NodeId {
        crate::NodeId::from_bits(bits).expect("a valid node id")
    }

    use super::{ImageInbox, ImageReports};

    /// Reports are readable in the order they were made, and a drain empties
    /// the inbox.
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
                        height: 4
                    },
                    super::ImageEvent::Failed { source: failed },
                ] if &**loaded == "app:///a.png" && &**failed == "app:///b.png"
            ),
            "{drained:?}"
        );
        assert!(inbox.drain().is_empty(), "a drain empties the inbox");
    }

    /// A host outliving its view must not keep buffering into a painter that
    /// will never read again.
    #[test]
    fn a_detached_inbox_takes_nothing() {
        let (reports, inbox) = ImageInbox::new();

        inbox.detach();
        reports.loaded("app:///a.png", 4, 4);

        assert!(inbox.drain().is_empty());
    }

    /// The host's handle is thread-bound by construction, so a store cannot
    /// hand it to a loader thread even by mistake.
    const _: () = {
        const fn assert_not_send<T>() {}
        assert_not_send::<ImageReports>();
    };

    use std::sync::Arc;

    use super::{
        ImageEvent, ImageOutcome, ImageRegistry, ImageRole, ImageSizeHint,
        MAX_RENDERABLE_DIMENSION, NoImages, is_renderable,
    };
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

    /// A decoded-image report must cross to the thread that owns the tree.
    ///
    /// This used to assert `Document<()>: Send`, because a document was built
    /// on one thread and moved to the one that ran it. A document does not
    /// move any more — `spawn_main_thread` hands `bobcat-main` the *sources*
    /// and the document is created there — and it is now structurally unable
    /// to (`Node`'s arena backpointer is a raw pointer). The registry itself
    /// therefore needs nothing.
    ///
    /// What still genuinely crosses is the report: a host answers an image
    /// load on whatever thread it likes, the painter forwards the batch as
    /// `ToMain::ImageEvents`, and `bobcat-main` applies it. That send is the
    /// requirement worth pinning here.
    #[test]
    fn an_image_report_crosses_to_the_thread_that_owns_the_tree() {
        const fn assert_send<T: Send>() {}
        assert_send::<ImageEvent>();
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
        let (source, dimensions, _) = registry
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

        let (_, dimensions, _) = registry
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
        registry.bind_node("app:///a.png", root, ImageRole::Source);
        let applied = registry
            .apply(&loaded("app:///a.png", 12, 6))
            .expect("the load moved the entry");
        assert_eq!(applied.loaded, Some((12, 6)));
        assert_eq!(
            applied.nodes.as_slice(),
            [(root, ImageRole::Source)],
            "the bound node relayouts"
        );
    }

    /// One element may name one URL in both roles, so a binding is the pair
    /// and not the node: unbinding one role must leave the other standing.
    #[test]
    fn one_url_in_both_roles_is_two_bindings() {
        let mut registry = ImageRegistry::default();
        let element = node(1);
        registry.bind_node("app:///a.png", element, ImageRole::Source);
        registry.bind_node("app:///a.png", element, ImageRole::Placeholder);
        assert_eq!(registry.take_wanted().len(), 1, "one source, one request");

        registry.unbind_node("app:///a.png", element, ImageRole::Source);
        let applied = registry
            .apply(&loaded("app:///a.png", 12, 6))
            .expect("the load moved the entry");
        assert_eq!(
            applied.nodes.as_slice(),
            [(element, ImageRole::Placeholder)]
        );
    }

    /// A failure names its nodes too — it is what an element owes an `error`
    /// for — where it used to report none at all.
    #[test]
    fn a_failure_reports_its_nodes() {
        let mut registry = ImageRegistry::default();
        let element = node(1);
        registry.bind_node("app:///a.png", element, ImageRole::Source);
        let applied = registry
            .apply(&failed("app:///a.png"))
            .expect("the failure moved the entry");
        assert_eq!(applied.loaded, None);
        assert_eq!(applied.nodes.as_slice(), [(element, ImageRole::Source)]);
    }

    /// A zero axis is a failure, and reports as one rather than as the load
    /// the host wrote.
    #[test]
    fn a_zero_axis_load_reports_as_a_failure() {
        let mut registry = ImageRegistry::default();
        let element = node(1);
        registry.bind_node("app:///bad.png", element, ImageRole::Source);
        let applied = registry
            .apply(&loaded("app:///bad.png", 0, 4))
            .expect("the report moved the entry");
        assert_eq!(applied.loaded, None);
        assert_eq!(applied.nodes.as_slice(), [(element, ImageRole::Source)]);
    }

    /// A source that settled before a node bound to it is never reported
    /// again, so the bind is the only place its outcome can be learnt.
    #[test]
    fn a_settled_source_answers_a_later_bind() {
        let mut registry = ImageRegistry::default();
        let element = node(1);
        registry.bind_node("app:///a.png", element, ImageRole::Source);
        assert_eq!(
            registry.outcome_for("app:///a.png", element),
            None,
            "a pending source owes nothing yet: the report will carry it"
        );

        registry.apply(&loaded("app:///a.png", 12, 6));
        registry.apply(&failed("app:///b.png"));
        assert_eq!(
            registry.outcome_for("app:///a.png", node(2)),
            Some(ImageOutcome::Loaded {
                node: node(2),
                width: 12,
                height: 6
            })
        );
        assert_eq!(
            registry.outcome_for("app:///b.png", node(2)),
            Some(ImageOutcome::Failed { node: node(2) })
        );
        assert_eq!(
            registry.outcome_for("app:///never-seen.png", node(2)),
            None,
            "a source nothing has asked for has settled on nothing"
        );
    }

    /// The whole src-over-placeholder rule, at the registry's level: the
    /// element's own source once it has pixels, the placeholder until then.
    #[test]
    fn the_presented_dimensions_prefer_the_source_over_the_placeholder() {
        let mut registry = ImageRegistry::default();
        let (source, placeholder) = (Some("app:///a.png"), Some("app:///p.png"));
        assert_eq!(registry.presented_dimensions(source, placeholder), None);

        registry.apply(&loaded("app:///p.png", 4, 4));
        assert_eq!(
            registry.presented_dimensions(source, placeholder),
            Some((4, 4)),
            "the placeholder is drawn while the source has nothing"
        );
        assert_eq!(
            registry.presented_dimensions(None, placeholder),
            Some((4, 4)),
            "and on its own, for an element with no source at all"
        );

        registry.apply(&loaded("app:///a.png", 12, 6));
        assert_eq!(
            registry.presented_dimensions(source, placeholder),
            Some((12, 6)),
            "a loaded source suppresses the placeholder"
        );
    }

    /// The paint walk's half of the same rule: it draws whichever source the
    /// dimensions came from, and asking still asks — a source first met there
    /// is requested exactly as a single-source `resolve` requests it.
    #[test]
    fn the_paint_walk_resolves_the_presented_source_and_asks_for_it() {
        let mut registry = ImageRegistry::default();
        let (source, placeholder) = (Some("app:///a.png"), Some("app:///p.png"));
        assert!(registry.resolve_presented(source, placeholder).is_none());
        let mut wanted = registry.take_wanted();
        wanted.sort();
        assert_eq!(
            wanted,
            vec![
                Arc::<str>::from("app:///a.png"),
                Arc::<str>::from("app:///p.png")
            ],
            "a first sighting of either asks for it"
        );

        registry.apply(&loaded("app:///p.png", 4, 4));
        let (drawn, dimensions, _) = registry
            .resolve_presented(source, placeholder)
            .expect("the placeholder draws while the source has nothing");
        assert_eq!(drawn.as_ref(), "app:///p.png");
        assert!((dimensions.0 - 4.0).abs() < f64::EPSILON);

        registry.apply(&loaded("app:///a.png", 12, 6));
        let (drawn, _, _) = registry
            .resolve_presented(source, placeholder)
            .expect("the source took over");
        assert_eq!(drawn.as_ref(), "app:///a.png");
        assert!(registry.take_wanted().is_empty(), "each asked exactly once");
    }

    #[test]
    fn the_empty_pixel_source_draws_nothing() {
        assert!(
            NoImages
                .read("app:///a.png", ImageSizeHint::UNBOUNDED)
                .is_none()
        );
    }

    /// The downsampling decision every host shares: inside the hint, keep
    /// the ratio, never grow, never vanish.
    #[test]
    fn a_hint_fits_an_image_without_growing_it_or_breaking_its_ratio() {
        assert_eq!(
            ImageSizeHint::UNBOUNDED.fit(4000, 1000),
            (4000, 1000),
            "no bound decodes the image as it is"
        );
        assert_eq!(
            ImageSizeHint::new(8000, 8000).fit(4000, 1000),
            (4000, 1000),
            "a hint larger than the image never upsamples"
        );
        assert_eq!(
            ImageSizeHint::new(500, 500).fit(4000, 1000),
            (500, 125),
            "the binding axis lands on the hint and the other keeps the ratio"
        );
        assert_eq!(ImageSizeHint::new(500, 200).fit(1000, 4000), (50, 200));
        assert_eq!(
            ImageSizeHint::new(0, 0).fit(1000, 4000),
            (1, 1),
            "a degenerate draw still asks for a pixel, never a zero-sized bitmap"
        );
        assert_eq!(ImageSizeHint::new(300, 300).fit(0, 0), (1, 1));
    }

    #[test]
    fn a_union_of_hints_covers_the_larger_draw_on_each_axis() {
        let hint = ImageSizeHint::new(100, 900).union(ImageSizeHint::new(800, 50));
        assert_eq!(hint, ImageSizeHint::new(800, 900));
        assert!(!hint.is_unbounded());
        assert!(hint.union(ImageSizeHint::UNBOUNDED).is_unbounded());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod vector_tests {
    use super::VectorImage;

    fn parse(attributes: &str) -> VectorImage {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" {attributes}><rect x="2" y="3" width="5" height="7"/></svg>"#
        );
        VectorImage::parse_sealed(svg.as_bytes())
            .unwrap_or_else(|error| panic!("<svg {attributes}>: {error}"))
    }

    /// The natural size follows CSS Images 3 default sizing, and the
    /// viewport is the tree's own size wherever the root gives usvg a size
    /// to map into.
    #[test]
    fn natural_size_and_viewport_follow_the_root_attributes() {
        let cases = [
            // (a) Both absolute, with or without a viewBox.
            (r#"width="40" height="30""#, (40, 30), (40.0, 30.0)),
            (r#"width="40" height="30px""#, (40, 30), (40.0, 30.0)),
            (
                r#"width="40px" height="30" viewBox="0 0 4 3""#,
                (40, 30),
                (40.0, 30.0),
            ),
            // (b) One absolute plus a viewBox.
            (r#"width="40" viewBox="0 0 20 10""#, (40, 20), (40.0, 20.0)),
            (r#"height="30" viewBox="0 0 20 10""#, (60, 30), (60.0, 30.0)),
            // (c) A viewBox only, fitted into 300x150.
            (r#"viewBox="0 0 10 10""#, (150, 150), (10.0, 10.0)),
            (r#"viewBox="0 0 40 10""#, (300, 75), (40.0, 10.0)),
            (r#"viewBox="0,0,40,10""#, (300, 75), (40.0, 10.0)),
            // (d) Neither: the default object size, which is also the viewport.
            ("", (300, 150), (300.0, 150.0)),
            (r#"width="50%" height="2em""#, (300, 150), (300.0, 150.0)),
            // (e) One absolute and no viewBox.
            (r#"width="50%" height="20""#, (300, 20), (300.0, 20.0)),
            (r#"width="50%" height="1in""#, (300, 96), (300.0, 96.0)),
            (r#"width="3pc" height="1ex""#, (48, 150), (48.0, 150.0)),
            (r#"width="40""#, (40, 150), (40.0, 150.0)),
            // Whole px, at least one.
            (r#"width="10.4" height="0.2""#, (10, 1), (10.4, 0.2)),
        ];
        for (attributes, natural, viewport) in cases {
            let image = parse(attributes);
            assert_eq!(image.natural_size(), natural, "<svg {attributes}> natural");
            assert_eq!(image.viewport(), viewport, "<svg {attributes}> viewport");
        }
    }

    /// Every CSS absolute unit converts at 96 px per inch, and the viewport
    /// is the size usvg converted the same lengths to.
    #[test]
    fn absolute_units_convert_to_px() {
        let image = parse(r#"width="10mm" height="10mm""#);
        assert_eq!(image.natural_size(), (38, 38));
        let size = image.tree().size();
        assert_eq!(image.viewport(), (size.width(), size.height()));
        for (length, px) in [
            ("1in", 96.0),
            ("2.54cm", 96.0),
            ("25.4mm", 96.0),
            ("72pt", 96.0),
            ("6pc", 96.0),
            ("96px", 96.0),
            ("96", 96.0),
        ] {
            let converted = super::absolute_length(length).expect(length);
            assert!((converted - px).abs() < 1e-3, "{length} is {converted} px");
        }
        for absent in ["1em", "1ex", "50%", "1rem", "1vw", "0in", "-2pt"] {
            assert_eq!(super::absolute_length(absent), None, "{absent}");
        }
    }

    /// The parse the engine runs on a reported document reads no file an
    /// `<image>` inside it names. A nested SVG document renders its own
    /// tree, so the same document reached through a `data:` URL draws while
    /// the file path draws nothing. Each document goes through the inline
    /// path, a [`super::ImageEvent::LoadedDocument`] applied to the registry.
    #[test]
    fn a_reported_document_reads_no_file_its_image_elements_name() {
        use std::sync::Arc;

        use super::{DocumentKind, ImageEvent, ImageRegistry};

        let image_of = |href: &str| {
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="{href}" width="10" height="10"/></svg>"#
            )
        };
        let inner = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect x="1" y="1" width="6" height="4" fill="#dc2626"/></svg>"##;
        let path =
            std::env::temp_dir().join(format!("dom-vector-{}-nested.svg", std::process::id()));
        std::fs::write(&path, inner).expect("write the temp file");
        let file = path.to_str().expect("a UTF-8 temp path").to_owned();
        let data_url = format!(
            "data:image/svg+xml;base64,{}",
            base64_encode(inner.as_bytes())
        );

        let mut registry = ImageRegistry::default();
        // A file every Unix has, the nested document by path, and the same
        // document as a `data:` URL.
        let mut drawn = Vec::new();
        for (source, href) in [
            ("app:///hosts.svg", "/etc/hosts"),
            ("app:///by-path.svg", file.as_str()),
            ("app:///by-data.svg", data_url.as_str()),
        ] {
            let applied = registry.apply(&ImageEvent::LoadedDocument {
                source: Arc::from(source),
                bytes: bytes::Bytes::from(image_of(href)),
                kind: DocumentKind::Svg,
            });
            assert_eq!(
                applied.and_then(|applied| applied.loaded),
                Some((10, 10)),
                "{source} loads"
            );
            let (_, _, vector) = registry.resolve(source).expect("loaded");
            let vector = vector.expect("a vector image");
            drawn.push(!vector.scene().encoding().is_empty());
        }
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            drawn,
            [false, false, true],
            "neither file was read; a data: URL is resolved, which is what makes \
             the empty scenes evidence"
        );
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut buffer = [0_u8; 3];
            buffer[..chunk.len()].copy_from_slice(chunk);
            let bits =
                u32::from(buffer[0]) << 16 | u32::from(buffer[1]) << 8 | u32::from(buffer[2]);
            for index in 0..4 {
                if index <= chunk.len() {
                    out.push(char::from(
                        ALPHABET[((bits >> (18 - 6 * index)) & 63) as usize],
                    ));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// Failures are usvg's own errors, as `usvg::Tree::from_data` reports
    /// them without the `svgz` feature.
    #[test]
    fn unreadable_documents_fail_with_the_usvg_error() {
        assert!(matches!(
            VectorImage::parse_sealed(b"<svg xmlns='http://www.w3.org/2000/svg'><rect></svg>"),
            Err(usvg::Error::ParsingFailed(_))
        ));
        assert!(matches!(
            VectorImage::parse_sealed(b"<svg \xff/>"),
            Err(usvg::Error::NotAnUtf8Str)
        ));
        assert!(matches!(
            VectorImage::parse_sealed(&[0x1f, 0x8b, 0x08, 0x00]),
            Err(usvg::Error::SvgzFeatureNotEnabled)
        ));
    }
}
