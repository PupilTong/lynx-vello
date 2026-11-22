//! HTML's `<canvas>` element (canvas.html, "The canvas element") as a
//! placeholder for an `OffscreenCanvas` ("The `OffscreenCanvas` interface").
//!
//! [`Document::define_html_elements`] defines its component. HTML gives the
//! element no UA rule of its own: it is a replaced element whose content is
//! its bitmap, and here it presents a picture of its own
//! ([`PictureKey::Id`], [`Document::set_picture`]), which makes its box a
//! replaced one that generates no boxes for its children.
//!
//! # State
//!
//! Each element keeps, from its construction until it is freed, one
//! [`PictureId`] and its *context mode*. Two of HTML's modes are built:
//!
//! - **none**, the initial mode: the element has no rendering context and its bitmap is transparent
//!   black, so the picture draws nothing. Its natural size is the numeric values of `width` and
//!   `height` (the rules for parsing non-negative integers), 300 and 150 when absent or unparsable,
//!   and follows every change of either attribute.
//! - **placeholder**, after [`transfer_control_to_offscreen`]: the element shows what its
//!   `OffscreenCanvas` commits through the [`CanvasFrames`] that call answered. A change of `width`
//!   or `height` does nothing (HTML's attribute table, "placeholder: Do nothing"). The natural size
//!   is the committed bitmap's: it changes when a bitmap of another size is committed, and then
//!   only on the document's thread, in [`update_placeholders`].
//!
//! `getContext()` and the `2d`, `bitmaprenderer`, `webgl` and `webgpu`
//! modes are not built on the element: a canvas draws only what an
//! `OffscreenCanvas` commits to it. The IDL `width` and `height` setters, which throw on a
//! placeholder, are the embedder's script binding's; this module sees the
//! content attributes only.
//!
//! # Commits
//!
//! The `OffscreenCanvas`'s bitmap is a GPU texture
//! ([`crate::render::canvas::Surface`]); its owner, on any thread, commits it
//! with `Surface::commit`. The commit copies the texture into the one the
//! picture is drawn from, which the link holds a lease on for the element's
//! life, and then publishes the new generation, so a painter never draws a
//! commit half copied and the painter's own release of textures never takes
//! this one. It answers what it owes: a wake of the host that draws, and,
//! for a bitmap whose size differs from the last committed, one call of
//! [`update_placeholders`] on the document's thread. How either is delivered
//! is the embedder's. Once the element is freed, a commit does nothing and
//! owes nothing. A placeholder nothing was committed to draws nothing.
//!
//! # The 2D rendering context
//!
//! [`OffscreenCanvas`] is the `OffscreenCanvas`'s side: its size and its
//! bitmap, committed here through [`OffscreenCanvas::commit`]. Its
//! `OffscreenCanvasRenderingContext2D` is the [`Context2d`] that
//! [`OffscreenCanvas::context_2d`] borrows ([`context2d`]), with `Path2D`,
//! `CanvasGradient`, `CanvasPattern` and the image it draws. Neither needs a
//! [`Document`], so they run on a thread that has none.

mod context2d;
mod offscreen;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use rustc_hash::FxHashMap;

pub use self::context2d::{
    CanvasError, CompositeOp, Context2d, Direction, FillRule, GenericFamily, Gradient, Image,
    ImageDataRead, LineCap, LineJoin, Path, Pattern, Repetition, ResolvedFamily, ResolvedFont,
    ResolvedFontStyle, SmoothingQuality, Style, TextAlign, TextBaseline, TextMetrics,
};
pub use self::offscreen::OffscreenCanvas;
pub use super::InvalidState;
use crate::render::image::{FrameImage, ImageDescriptor};
use crate::render::picture::{PictureId, PictureKey};
use crate::tree::custom::CustomElement;
use crate::tree::document::{Document, NodeId};

/// HTML's tag.
pub const CANVAS_TAG: &str = "canvas";
const WIDTH_ATTRIBUTE: &str = "width";
const HEIGHT_ATTRIBUTE: &str = "height";

/// The natural size of a canvas whose attributes give none.
const DEFAULT_SIZE: (u32, u32) = (300, 150);

/// A canvas element's context mode, of the two this module builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMode {
    None,
    Placeholder,
}

/// What [`transfer_control_to_offscreen`] answers: the size the
/// `OffscreenCanvas` is created at, and where it commits its bitmap.
#[derive(Debug)]
pub struct Placeholder {
    pub width: u32,
    pub height: u32,
    pub frames: CanvasFrames,
}

/// What one commit owes (`Surface::commit`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Committed {
    /// The host that draws has to be woken: it has described every earlier
    /// commit.
    pub wake: bool,
    /// The document's thread has to run [`update_placeholders`]: the bitmap's
    /// size differs from the last committed one, and no earlier call is
    /// still owed.
    pub resize: bool,
}

/// The shared half of a placeholder: what its picture shows, and the
/// committed size the document has not applied.
#[derive(Debug)]
pub(crate) struct Link {
    /// The lease owner holding the picture's texture
    /// (`render::sparse::SharedRaster::copy_to_picture`).
    owner: u64,
    shown: Mutex<Shown>,
    /// The last committed bitmap's size, packed by [`pack`].
    committed: AtomicU64,
    resize_owed: AtomicBool,
}

/// The newest commit a painter may draw.
#[derive(Debug, Default)]
struct Shown {
    /// The last generation handed out; zero before the first commit.
    issued: u64,
    /// The newest published commit, `None` before the first.
    published: Option<(u64, ImageDescriptor)>,
    /// A published commit the painter has not described yet.
    waiting: bool,
}

impl Link {
    fn shown(&self) -> std::sync::MutexGuard<'_, Shown> {
        self.shown.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn owner(&self) -> u64 {
        self.owner
    }

    /// The generation the next commit is published as.
    pub(crate) fn next_generation(&self) -> u64 {
        let mut shown = self.shown();
        shown.issued += 1;
        shown.issued
    }

    /// Publishes commit `generation`, which `descriptor` names, once its
    /// pixels are in the picture's texture (or, on wasm32, queued ahead of
    /// every later op), and answers what it owes.
    pub(crate) fn publish(&self, generation: u64, descriptor: ImageDescriptor) -> Committed {
        let size = pack(descriptor.width, descriptor.height);
        let resize = self.committed.swap(size, Ordering::AcqRel) != size
            && !self.resize_owed.swap(true, Ordering::AcqRel);
        let mut shown = self.shown();
        shown.published = Some((generation, descriptor));
        let wake = !std::mem::replace(&mut shown.waiting, true);
        Committed { wake, resize }
    }
}

impl Drop for Link {
    /// The element is freed: its picture's texture is let go.
    fn drop(&mut self) {
        crate::render::canvas::end_lease(self.owner);
    }
}

/// Where an `OffscreenCanvas` commits its bitmap for its placeholder canvas
/// element. `Send`, and cloned freely; it holds the element's state weakly,
/// so it keeps nothing alive once the element is freed.
#[derive(Clone, Debug)]
pub struct CanvasFrames {
    picture: PictureId,
    link: Weak<Link>,
}

impl CanvasFrames {
    /// The picture the placeholder presents.
    #[must_use]
    pub const fn picture(&self) -> PictureId {
        self.picture
    }

    pub(crate) fn link(&self) -> Option<Arc<Link>> {
        self.link.upgrade()
    }

    /// What the painter is told about the picture: the newest commit's
    /// descriptor and generation, with no pixels, which are in the
    /// picture's texture. `None` before the first commit and once the
    /// element is freed. Called on the painter's thread: the next commit
    /// owes a wake again.
    #[must_use]
    pub fn describe<P>(&self) -> Option<FrameImage<P>> {
        let link = self.link.upgrade()?;
        let mut shown = link.shown();
        shown.waiting = false;
        let (generation, descriptor) = shown.published?;
        Some(FrameImage {
            descriptor,
            pixels: None,
            generation,
            next_change: None,
        })
    }

    /// Whether the element is still alive.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.link.strong_count() > 0
    }
}

fn pack(width: u32, height: u32) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "unpacks what `pack` packed: each half is a `u32`"
)]
const fn unpack(size: u64) -> (u32, u32) {
    ((size >> 32) as u32, size as u32)
}

#[derive(Debug)]
enum Mode {
    None,
    Placeholder(Arc<Link>),
}

#[derive(Debug)]
struct Element {
    picture: PictureId,
    mode: Mode,
}

/// The document's canvas elements, shared by the document and the
/// component, which drops an element's state when it is freed.
#[derive(Clone, Debug, Default)]
pub(crate) struct Canvases(Rc<RefCell<FxHashMap<NodeId, Element>>>);

/// Installs the `canvas` component ([`Document::define_html_elements`]).
pub(super) fn define<T>(document: &mut Document<T>) {
    let canvases = document.canvases.clone();
    document.define(CANVAS_TAG, Box::new(CanvasElement(canvases)));
}

/// `canvas`'s context mode; `None` when it is not a live canvas element.
#[must_use]
pub fn context_mode<T>(document: &Document<T>, canvas: NodeId) -> Option<ContextMode> {
    Some(match document.canvases.0.borrow().get(&canvas)?.mode {
        Mode::None => ContextMode::None,
        Mode::Placeholder(_) => ContextMode::Placeholder,
    })
}

/// HTML's `transferControlToOffscreen()`: [`InvalidState`] unless the
/// context mode is none (or for a node that is not a live canvas element);
/// otherwise the mode becomes placeholder, and the answer is the
/// `OffscreenCanvas`'s size — the numeric values of `width` and `height`
/// now — and the link its bitmap is committed through.
///
/// # Errors
///
/// [`InvalidState`] as above.
pub fn transfer_control_to_offscreen<T>(
    document: &mut Document<T>,
    canvas: NodeId,
) -> Result<Placeholder, InvalidState> {
    let (width, height) = attribute_size(document, canvas);
    let mut canvases = document.canvases.0.borrow_mut();
    let element = canvases.get_mut(&canvas).ok_or(InvalidState)?;
    if !matches!(element.mode, Mode::None) {
        return Err(InvalidState);
    }
    let link = Arc::new(Link {
        owner: crate::render::canvas::next_owner(),
        shown: Mutex::default(),
        committed: AtomicU64::new(pack(width, height)),
        resize_owed: AtomicBool::new(false),
    });
    let frames = CanvasFrames {
        picture: element.picture,
        link: Arc::downgrade(&link),
    };
    element.mode = Mode::Placeholder(link);
    Ok(Placeholder {
        width,
        height,
        frames,
    })
}

/// Gives every placeholder canvas whose committed bitmap changed size the
/// committed size as its natural size: HTML's update of a placeholder's
/// natural size, on the document's thread. Cheap when nothing is owed.
pub fn update_placeholders<T>(document: &mut Document<T>) {
    let resized: Vec<(NodeId, PictureId, (u32, u32))> = document
        .canvases
        .0
        .borrow()
        .iter()
        .filter_map(|(node, element)| match &element.mode {
            Mode::Placeholder(link) if link.resize_owed.swap(false, Ordering::AcqRel) => Some((
                *node,
                element.picture,
                unpack(link.committed.load(Ordering::Acquire)),
            )),
            _ => None,
        })
        .collect();
    for (node, picture, size) in resized {
        document.set_picture(node, Some((PictureKey::Id(picture), Some(size))));
    }
}

/// The numeric values of `canvas`'s `width` and `height`, or their defaults.
fn attribute_size<T>(document: &Document<T>, canvas: NodeId) -> (u32, u32) {
    let Some(node) = document.get(canvas) else {
        return DEFAULT_SIZE;
    };
    let read = |name, default| {
        node.attribute(name)
            .and_then(|value| parse_non_negative_integer(&value))
            .unwrap_or(default)
    };
    (
        read(WIDTH_ATTRIBUTE, DEFAULT_SIZE.0),
        read(HEIGHT_ATTRIBUTE, DEFAULT_SIZE.1),
    )
}

/// HTML's rules for parsing non-negative integers (common-microsyntaxes):
/// leading ASCII whitespace, an optional sign, then the leading run of ASCII
/// digits; whatever follows them is ignored. No digits, or a value below
/// zero, is an error, and so is one past `u32`.
fn parse_non_negative_integer(input: &str) -> Option<u32> {
    let input = input.trim_start_matches(['\t', '\n', '\u{c}', '\r', ' ']);
    let (negative, digits) = match input.as_bytes().first()? {
        b'-' => (true, &input[1..]),
        b'+' => (false, &input[1..]),
        _ => (false, input),
    };
    let end = digits
        .bytes()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(digits.len());
    let value: u32 = digits[..end].parse().ok()?;
    (!negative || value == 0).then_some(value)
}

/// The `canvas` component: owns each element's picture and mode, and keeps
/// a mode-none canvas's natural size in step with its attributes.
struct CanvasElement(Canvases);

impl CanvasElement {
    /// Presents `canvas`'s picture at its attributes' size while its mode is
    /// none; a placeholder's size is its committed bitmap's.
    fn show_attributes<T>(&self, document: &mut Document<T>, canvas: NodeId) {
        let picture = match self.0.0.borrow().get(&canvas) {
            Some(Element {
                picture,
                mode: Mode::None,
            }) => *picture,
            _ => return,
        };
        let size = attribute_size(document, canvas);
        document.set_picture(canvas, Some((PictureKey::Id(picture), Some(size))));
    }
}

impl<T> CustomElement<T> for CanvasElement {
    fn observed_attributes(&self) -> Vec<String> {
        vec![WIDTH_ATTRIBUTE.to_owned(), HEIGHT_ATTRIBUTE.to_owned()]
    }

    fn constructed(&self, document: &mut Document<T>, element: NodeId) {
        self.0.0.borrow_mut().insert(
            element,
            Element {
                picture: PictureId::new(),
                mode: Mode::None,
            },
        );
        self.show_attributes(document, element);
    }

    fn attribute_changed_callback(
        &self,
        document: &mut Document<T>,
        element: NodeId,
        _name: &str,
        _old: Option<&str>,
        _new: Option<&str>,
    ) {
        self.show_attributes(document, element);
    }

    fn freed(&self, element: NodeId) {
        self.0.0.borrow_mut().remove(&element);
    }
}

#[cfg(test)]
mod tests;
