//! The presenting painter: input routing, scrolling, composition, and one
//! draw target, over the frames a document commits.
//!
//! A [`Painter`] is built on the thread that will draw, and everything it
//! owns — a surface, a scene buffer, a gesture arena — is touched from that
//! thread alone. It is `!Send` by construction, which is what makes the
//! constructing thread the painting thread for its whole life.
//!
//! It knows nothing of how frames reach it or where what it decides goes.
//! Its owner adopts each committed frame into it with the pixels that frame
//! draws ([`Painter::adopt`]), hands it the host's input, and delivers the
//! events it decides ([`EmitEvent`]) and the scroll offsets it posts
//! ([`Painter::drain_changed`]) to whatever runs the document. What the
//! painter keeps between those calls is what the screen shows: the live
//! scroll offsets ([`ScrollIntents`]), the gesture in progress
//! ([`GestureRouter`]), the frame clock, and what its target was last
//! rendered from.
//!
//! The frame is baked unscrolled, as fragments plus a compose program, and
//! the painter composes it flat at its own offsets, so a scroll recomposes
//! and re-hit-tests without a commit. A frame carrying `filter: blur()`
//! groups or `backdrop-filter` elements bakes them offscreen first
//! ([`crate::render::blur`]).

mod gesture;
mod graphics;
mod images;
mod inertia;
mod motion;

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::rc::Rc;
use std::sync::Arc;

use euclid::default::Vector2D;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use vello::Scene;
use vello::peniko::{Color, ImageData};

pub use self::gesture::{
    EmitEvent, GesturePolicy, GestureRouter, InputDecision, InputDecisions, RouterHost,
};
pub use self::graphics::WindowTarget;
use self::graphics::{AcquiredFrame, FrameAcquisition, WindowGraphics};
use self::inertia::{AutoScroll, Axis, BounceBack, ChainOutcome, DragTrack, Fling, Glide};
use self::motion::{ElasticStep, Motion, resolve_elastic_step, stretch_of, unwind_stretch};
use crate::clock::ClockInstant;
use crate::input::{InputEvent, InputKind, PointerId};
use crate::render::gpu::Headless;
use crate::render::image::FrameImages;
use crate::scroll::{
    AutoScrollRequest, ChainLink, ScrollAxes, ScrollBehavior, ScrollRequest, drive_chain,
    settle_offset,
};
use crate::style::device::Device;
use crate::tree::document::NodeId;
use crate::visual::{CommittedFrame, HitTarget};

/// View metrics: the CSS size a document lays out at, and the device scale
/// it is drawn at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Viewport width in CSS pixels.
    pub width: f32,
    /// Viewport height in CSS pixels.
    pub height: f32,
    /// Physical pixels per CSS pixel.
    pub device_pixel_ratio: f32,
}

impl Viewport {
    /// Creates a viewport with a device-pixel ratio of 1.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio: 1.0,
        }
    }

    /// Returns this viewport with a new device-pixel ratio.
    #[must_use]
    pub const fn with_device_pixel_ratio(mut self, device_pixel_ratio: f32) -> Self {
        self.device_pixel_ratio = device_pixel_ratio;
        self
    }

    /// The style device a document at these metrics is built on.
    #[must_use]
    pub fn device(self) -> Device {
        Device::new(self.width, self.height, self.device_pixel_ratio)
    }
}

/// The physical pixel size of the render target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

impl FrameSize {
    /// The physical target a CSS viewport at this device scale needs.
    ///
    /// The same computation [`Painter::new`] and [`Painter::resize`] make,
    /// exposed because a host that owns the surface's backing store — a
    /// browser canvas — has to size it before it hands a painter a target.
    ///
    /// # Errors
    ///
    /// [`PainterError::Viewport`] if the metrics are not finite and positive,
    /// or if the physical target would exceed 16384 pixels on either axis.
    pub fn for_viewport(
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<Self, PainterError> {
        if !width.is_finite()
            || !height.is_finite()
            || !device_pixel_ratio.is_finite()
            || width <= 0.0
            || height <= 0.0
            || device_pixel_ratio <= 0.0
        {
            return Err(PainterError::Viewport(format!(
                "CSS size and device-pixel ratio must be finite and positive, got \
                 {width}\u{d7}{height} at {device_pixel_ratio}\u{d7}"
            )));
        }

        let physical_width = f64::from(width) * f64::from(device_pixel_ratio);
        let physical_height = f64::from(height) * f64::from(device_pixel_ratio);
        if physical_width > f64::from(MAX_RENDER_DIMENSION)
            || physical_height > f64::from(MAX_RENDER_DIMENSION)
        {
            return Err(PainterError::Viewport(format!(
                "the physical render target may not exceed \
                 {MAX_RENDER_DIMENSION}\u{d7}{MAX_RENDER_DIMENSION}, got \
                 {physical_width:.0}\u{d7}{physical_height:.0}"
            )));
        }

        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "finite positive values were bounded to 16384 immediately above"
        )]
        Ok(Self {
            width: physical_width.round().max(1.0) as u32,
            height: physical_height.round().max(1.0) as u32,
        })
    }
}

/// Where a painter's pixels go, named once and kept for its whole life.
///
/// There is no attaching a target later: [`Painter::new`] builds it, on the
/// thread that will draw into it, before the painter exists — and a painter
/// then shows whichever documents it is handed through that one target.
pub enum DrawTarget {
    /// A window's presentation stack, built from a `'static` surface target —
    /// a shared window handle or an owned canvas.
    Window(WindowTarget),
    /// A texture the painter owns and nothing displays.
    /// [`Painter::draw_offscreen`] renders into it and [`Painter::capture`]
    /// reads it back.
    ///
    /// Native only in practice: building one blocks the calling thread on a
    /// device request, and in a browser that thread is the one whose event
    /// loop would answer it — so a Wasm painter is refused this target at
    /// construction rather than hanging on it.
    Offscreen,
}

impl DrawTarget {
    /// A window target, from anything a surface can be built out of.
    #[must_use]
    pub fn window(target: impl Into<WindowTarget>) -> Self {
        Self::Window(target.into())
    }
}

// Hand-written: a `SurfaceTarget` carries a window handle or a canvas, and
// neither is `Debug`. Which target this is, is the whole of what a formatter
// can honestly say.
impl fmt::Debug for DrawTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Window(_) => "DrawTarget::Window",
            Self::Offscreen => "DrawTarget::Offscreen",
        })
    }
}

const MAX_RENDER_DIMENSION: u32 = 16_384;

/// One captured frame: tightly packed RGBA8 pixels at size.
#[derive(Clone)]
pub struct Screenshot {
    /// The physical size the pixels were rendered at.
    pub size: FrameSize,
    /// `size.width × size.height` RGBA8 pixels, row-major, unpadded.
    pub pixels: Vec<u8>,
}

impl fmt::Debug for Screenshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Screenshot")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// What a rendered target is identified by: the commit it came from, and
/// the scroll generation it was composed at.
///
/// Images need no term of their own. A load that changes what a frame draws
/// dirties the document, and every rebuild takes a new commit id.
type ComposeKey = (u64, u64);

/// What building, resizing or drawing through a painter fails with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PainterError {
    /// The metrics are not finite and positive, or the physical target would
    /// exceed 16384 pixels on either axis.
    Viewport(String),
    /// A GPU operation failed: a device request, a bake, a readback.
    Gpu(String),
    /// Rendering failed: the surface could not be built, acquired or
    /// rendered into, or there was nothing rendered to read back.
    Render(String),
}

impl fmt::Display for PainterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Viewport(message) => write!(formatter, "invalid viewport: {message}"),
            Self::Gpu(message) => write!(formatter, "GPU operation failed: {message}"),
            Self::Render(message) => write!(formatter, "rendering failed: {message}"),
        }
    }
}

impl std::error::Error for PainterError {}

/// One scroll container's state as the painter last posted it
/// ([`Painter::drain_changed`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollEntry {
    /// The offset, clamped to the committed range: a `contain-bounce`
    /// stretch posts its edge.
    pub offset: Vector2D<f32>,
    /// The painter clock of the pass that posted it, in seconds.
    pub at: f64,
    /// Where the container came to rest, if it did since the last take: a
    /// drag let go without a fling, a fling spent, a bounce back landed, a
    /// snap after a commit. Kept across later posts until taken.
    pub rest: Option<Vector2D<f32>>,
    /// The newest programmatic scroll request the painter had handled for
    /// the container when it posted ([`crate::visual::ScrollSlot::request`]). The
    /// document drops a post naming an older one than the container's
    /// pending request, which it was made before the painter saw, and
    /// acknowledges the request with one that names it.
    pub request: Option<NonZeroU64>,
}

/// The painter's monotonic animation timeline. Its epoch is the start of
/// whichever document's timeline it is rebased onto, and one reading is
/// shared by every operation in a frame.
#[derive(Debug)]
pub struct FrameClock {
    epoch: ClockInstant,
    #[cfg(any(test, feature = "painter-test-utils"))]
    pinned: Option<f64>,
}

impl FrameClock {
    /// A clock whose epoch is now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            epoch: ClockInstant::now(),
            #[cfg(any(test, feature = "painter-test-utils"))]
            pinned: None,
        }
    }

    /// Moves the epoch onto a document's own timeline.
    ///
    /// A painter shown a fresh document reproduces the old semantics
    /// exactly — the two instants are the same moment. A painter shown a
    /// document that has already been running keeps reading the time that
    /// document's animations were started against, rather than restarting
    /// them at whatever the painter's own age happens to be.
    pub fn rebase(&mut self, epoch: ClockInstant) {
        self.epoch = epoch;
    }

    /// Seconds since the epoch.
    #[must_use]
    pub fn now_seconds(&self) -> f64 {
        #[cfg(any(test, feature = "painter-test-utils"))]
        if let Some(seconds) = self.pinned {
            return seconds;
        }
        self.epoch.elapsed().as_secs_f64()
    }

    /// Holds every later reading at `seconds` (never below zero), for a test
    /// that steps the timeline by hand.
    #[cfg(any(test, feature = "painter-test-utils"))]
    #[doc(hidden)]
    pub fn pin(&mut self, seconds: f64) {
        self.pinned = Some(seconds.max(0.0));
    }
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod clock_tests {
    use super::FrameClock;

    #[test]
    fn the_clock_starts_near_zero_and_never_goes_back() {
        let clock = FrameClock::new();
        let first = clock.now_seconds();
        let second = clock.now_seconds();
        assert!((0.0..1.0).contains(&first), "the epoch is clock creation");
        assert!(second >= first, "and time only moves forward");
    }

    #[test]
    fn a_pinned_clock_holds_the_instant_a_test_named() {
        let mut clock = FrameClock::new();
        clock.pin(1.5);
        assert!((clock.now_seconds() - 1.5).abs() < 1e-9);
        clock.pin(1.75);
        assert!((clock.now_seconds() - 1.75).abs() < 1e-9);
    }

    #[test]
    fn a_pinned_clock_never_reports_negative_time() {
        let mut clock = FrameClock::new();
        clock.pin(-5.0);
        assert!((clock.now_seconds() - 0.0).abs() < f64::EPSILON);
    }
}

/// The canvas colour: what a painter's target holds before a frame composes
/// over it, and so what shows wherever the page painted nothing.
///
/// Named once because two renders read it. The composite render clears to
/// it, and the bake of a `backdrop-filter` element at the document's root
/// renders over it, since filter-effects-2's Backdrop Root Image of the root
/// holds the canvas (see [`crate::render::blur`]).
const CANVAS: Color = Color::WHITE;

/// Where a painter's pixels go: a window's presentation stack, or a texture
/// it owns and nothing displays. It is built with the painter and is the one
/// it has for its whole life, whatever documents come and go past it.
enum Output {
    /// A painter with nowhere to draw. Test-only, so a test that exercises
    /// routing alone pays for no GPU device; production has exactly the two
    /// targets an embedder can name.
    #[cfg(feature = "painter-test-utils")]
    None,
    #[cfg_attr(
        target_arch = "wasm32",
        allow(
            dead_code,
            reason = "a browser view is refused this target at construction"
        )
    )]
    Offscreen(Box<Headless>),
    Window(Box<WindowGraphics>),
}

impl Output {
    /// Builds the target an embedder named, on the thread that will draw into
    /// it — the only thread macOS lets a surface be created from.
    async fn build(target: DrawTarget, frame_size: FrameSize) -> Result<Self, PainterError> {
        match target {
            DrawTarget::Window(target) => Ok(Self::Window(Box::new(
                WindowGraphics::new(target, frame_size).await?,
            ))),
            DrawTarget::Offscreen => Self::offscreen(),
        }
    }

    /// Gives up the frame a target that is itself the only copy of one holds.
    ///
    /// What a painter does when it is pointed at a different document. A window
    /// keeps its surface, because what is on screen is the previous page's
    /// last frame and it stays there until the next one is presented. An
    /// offscreen target is given up instead and rebuilt on the next render:
    /// its texture is the only place a frame exists, so keeping it would hand
    /// a reader the previous document's pixels as this one's.
    fn forget(&mut self) {
        match self {
            #[cfg(feature = "painter-test-utils")]
            Self::None => {}
            Self::Offscreen(gpu) => gpu.forget(),
            // A window keeps its surface and its last presented frame, but
            // the filter bakes still have to forget which *frame* they belong
            // to: commit ids restart at one per document.
            Self::Window(graphics) => graphics.forget_filters(),
        }
    }

    /// Bakes the textures of `frame`'s vector image draws not already
    /// resident in this target's raster cache into `out`, index-parallel
    /// with the frame's vector draws.
    ///
    /// Copied out for the same reason [`Self::prepare_filters`]'s table is.
    ///
    /// # Errors
    ///
    /// [`PainterError::Gpu`] if a bake render fails.
    fn prepare_vectors(
        &mut self,
        frame: &CommittedFrame,
        out: &mut Vec<Option<ImageData>>,
    ) -> Result<(), PainterError> {
        out.clear();
        let baked = match self {
            #[cfg(feature = "painter-test-utils")]
            // A painter with nowhere to draw bakes nothing, so every vector
            // draw encodes nothing — which is what a routing test wants: no
            // device.
            Self::None => return Ok(()),
            Self::Offscreen(gpu) => gpu
                .prepare_vectors(frame)
                .map_err(|error| PainterError::Gpu(error.to_string()))?,
            Self::Window(graphics) => graphics.prepare_vectors(frame)?,
        };
        out.extend(baked.iter().cloned());
        Ok(())
    }

    /// Bakes `frame`'s `filter: blur()` groups and `backdrop-filter`
    /// elements into `out`, index-parallel with the frame's filter entries.
    /// `vectors` is the table [`Self::prepare_vectors`] filled for this
    /// frame, since an entry's range may draw one.
    ///
    /// The table is copied out rather than borrowed because the very next
    /// step needs the target mutably again to render; an `ImageData` is a
    /// blob handle plus four fields, and `out` keeps its capacity across
    /// frames, so the copy allocates nothing in the steady state.
    ///
    /// # Errors
    ///
    /// [`PainterError::Gpu`] if a bake render fails.
    #[expect(
        clippy::too_many_arguments,
        reason = "one bake pre-step's full inputs: the frame, its two pixel tables, the three \
                  compose readings, and the table it fills"
    )]
    fn prepare_filters(
        &mut self,
        frame: &CommittedFrame,
        images: &[Option<ImageData>],
        vectors: &[Option<ImageData>],
        offset_of: &dyn Fn(&crate::visual::ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
        animation_now: Option<f64>,
        out: &mut Vec<Option<ImageData>>,
    ) -> Result<(), PainterError> {
        out.clear();
        let baked = match self {
            #[cfg(feature = "painter-test-utils")]
            // A painter with nowhere to draw bakes nothing, so every entry
            // falls back to replaying raw — which is what a routing test
            // wants: no device.
            Self::None => return Ok(()),
            Self::Offscreen(gpu) => gpu
                .prepare_filters(
                    frame,
                    images,
                    vectors,
                    offset_of,
                    scroll_generation,
                    animation_now,
                    CANVAS,
                )
                .map_err(|error| PainterError::Gpu(error.to_string()))?,
            Self::Window(graphics) => graphics.prepare_filters(
                frame,
                images,
                vectors,
                offset_of,
                scroll_generation,
                animation_now,
            )?,
        };
        out.extend(baked.iter().cloned());
        Ok(())
    }

    /// Renders one composed scene, drawing `images`, into this target at
    /// `size`.
    ///
    /// A window renders into the texture it re-presents rather than into the
    /// swap-chain image, which is what lets re-exposure present the same frame
    /// again with a blit alone. Neither kind waits for the submitted work here:
    /// a window presents, and an offscreen target's readback polls the device
    /// itself — only [`Painter::draw_offscreen`], which renders offscreen and
    /// reads nothing back, waits.
    ///
    /// The scene's bitmaps are named because a target tracks what its
    /// renderer's image atlas still holds; see
    /// [`crate::render::gpu::AtlasResidency`].
    fn render(
        &mut self,
        scene: &Scene,
        images: &[Option<ImageData>],
        size: FrameSize,
    ) -> Result<(), PainterError> {
        match self {
            #[cfg(feature = "painter-test-utils")]
            Self::None => Ok(()),
            Self::Offscreen(gpu) => gpu
                .render_frame(scene, images, size.width, size.height, CANVAS)
                .map_err(|error| PainterError::Gpu(error.to_string())),
            Self::Window(graphics) => graphics.render_to_target(scene, images, size),
        }
    }

    /// A windowless GPU target.
    ///
    /// `Headless::new` blocks on a device request, and a browser Worker is
    /// the thread whose event loop would have answered it — so rather than
    /// hang, a Wasm view is told no.
    fn offscreen() -> Result<Self, PainterError> {
        #[cfg(target_arch = "wasm32")]
        return Err(PainterError::Gpu(
            "an offscreen target blocks the thread that builds it on a device \
             request; a browser Worker is the thread that would answer it"
                .to_owned(),
        ));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let gpu = Headless::new().map_err(|error| PainterError::Gpu(error.to_string()))?;
            Ok(Self::Offscreen(Box::new(gpu)))
        }
    }
}

/// When one committed frame was composed and presented by this painter: the
/// presentation half of a frame's timing, beside the commit half the document
/// records (`Document::last_commit_timing`), which it meets on `commit_id`.
///
/// `compose_start..compose_end` brackets the frame's composition through the
/// GPU encode and submit — the vector and `filter: blur()` bakes, the compose
/// program's replay at the live scroll offsets and animation reading, and the
/// render. `presented` is the instant the window's present call returned, or
/// the instant an offscreen draw's submitted work finished: after the frame
/// was handed on, not after a display showed it, which is what Lynx's
/// `paintEnd` measures too.
#[derive(Clone, Copy, Debug)]
pub struct PresentedFrame {
    /// The presented frame's commit id ([`CommittedFrame::commit_id`]).
    pub commit_id: u64,
    /// Composition started.
    pub compose_start: ClockInstant,
    /// The GPU work was encoded and submitted.
    pub compose_end: ClockInstant,
    /// The present call returned, or the offscreen draw finished.
    pub presented: ClockInstant,
}

/// What [`Painter::render_frame`] measured of a render that produced an
/// image: when composition started and when the render was submitted.
#[derive(Clone, Copy)]
struct ComposeSpan {
    start: ClockInstant,
    end: ClockInstant,
}

/// What [`Painter::render_frame`] answers of a render that produced an
/// image: its [`ComposeSpan`], or `None` when presentation timing was off
/// ([`Painter::set_presentation_timing`]) and nothing was measured.
#[derive(Clone, Copy)]
struct Rendered {
    span: Option<ComposeSpan>,
}

/// The presentation records of the view a painter shows: the last presented
/// frame and Paint Timing's two first marks, and whether draws record them.
#[derive(Clone, Copy, Debug)]
struct PaintTiming {
    /// [`Painter::set_presentation_timing`]. Painter-lifetime, like the
    /// target: forgetting the view keeps it.
    enabled: bool,
    last_presented: Option<PresentedFrame>,
    first_paint: Option<PresentedFrame>,
    first_contentful_paint: Option<PresentedFrame>,
}

impl Default for PaintTiming {
    fn default() -> Self {
        Self {
            enabled: true,
            last_presented: None,
            first_paint: None,
            first_contentful_paint: None,
        }
    }
}

impl PaintTiming {
    /// The records of a new view, with the switch as it was.
    fn forget(&mut self) {
        *self = Self {
            enabled: self.enabled,
            ..Self::default()
        };
    }

    /// Records the presentation of `frame`.
    fn note(&mut self, frame: &CommittedFrame, span: ComposeSpan, presented: ClockInstant) {
        let record = PresentedFrame {
            commit_id: frame.commit_id(),
            compose_start: span.start,
            compose_end: span.end,
            presented,
        };
        self.last_presented = Some(record);
        if self.first_paint.is_none() {
            self.first_paint = Some(record);
        }
        if self.first_contentful_paint.is_none() && frame.paints_content() {
            self.first_contentful_paint = Some(record);
        }
    }
}

/// One draw target and everything that turns a document's frames into pixels
/// in it: the gesture router, the scroll intents, the composition, the frame
/// clock.
///
/// Kept on one thread by construction — the `Rc` marker makes the whole
/// struct `!Send` — and that thread is the one that built it, which is
/// therefore the one that draws.
///
/// It is built before any frame exists and outlives every document it shows.
/// What it derives from one document — the adopted frame, the gesture arena,
/// the resolved pixels — is dropped by [`Self::forget_view`]; the draw target
/// and what it holds are not: what was last drawn stays on screen, and stays
/// capturable, until [`Self::forget_target`].
pub struct Painter {
    viewport: Viewport,
    frame_size: FrameSize,
    output: Output,
    /// A window nobody can see draws nothing; the frame it owes stays owed.
    occluded: bool,
    /// A draw target that failed once cannot be reached again: it is reported
    /// once, and nothing tries to paint it afterwards. Painter-lifetime state:
    /// neither forgetting the view nor the target clears it, because neither
    /// replaces the target that failed.
    render_failed: bool,
    /// The last frame adopted ([`Self::adopt`]). Everything routing and
    /// drawing read comes from here, so one pass sees one state — and it
    /// survives the document, so its last frame is still drawable and
    /// capturable.
    frame: Option<Arc<CommittedFrame>>,
    redraw_pending: Cell<bool>,
    gesture: GestureRouter,
    clock: FrameClock,
    scroll_intents: ScrollIntents,
    /// What this painter's draw target holds: the compose key it was rendered
    /// from, at the size it was rendered for. The one record of that — a
    /// target has no second one of its own — so it says whether a frame may be
    /// skipped and whether there is anything to capture.
    ///
    /// The target's, not the document's: what was drawn stays drawn until
    /// [`Self::forget_target`] clears it.
    composed: Option<(ComposeKey, FrameSize)>,
    composed_scene: Scene,
    /// The `filter: blur()` textures of the frame being composed, copied out
    /// of the target's bake cache. Kept here so its capacity outlives a
    /// frame.
    composed_filters: Vec<Option<ImageData>>,
    /// The vector image textures of the frame being composed, copied out of
    /// the target's raster cache, for the same reason.
    composed_vectors: Vec<Option<ImageData>>,
    /// The pixels the adopted commit draws, read out of the host's store.
    images: images::PainterImages,
    /// The shown view's presentation records ([`Self::last_presented`],
    /// [`Self::first_paint`], [`Self::first_contentful_paint`]). The view's,
    /// not the target's: [`Self::forget_view`] clears them.
    paint_timing: PaintTiming,
    thread_bound: PhantomData<Rc<()>>,
}

impl fmt::Debug for Painter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Painter")
            .field("viewport", &self.viewport)
            .field("frame_size", &self.frame_size)
            .finish_non_exhaustive()
    }
}

/// One window frame between the swap-chain image it is presented in and its
/// present: what [`Painter::begin_draw`] answers and [`Painter::finish_draw`]
/// presents. Dropping it discards the image, which is what an abandoned frame
/// does.
#[must_use = "a window frame is presented by `Painter::finish_draw`"]
pub struct WindowFrame {
    acquired: AcquiredFrame,
    frame: Arc<CommittedFrame>,
    now: f64,
}

impl WindowFrame {
    /// The clock reading the frame is produced at, taken once its swap-chain
    /// image was.
    #[must_use]
    pub const fn now(&self) -> f64 {
        self.now
    }
}

impl fmt::Debug for WindowFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowFrame")
            .field("commit", &self.frame.commit_id())
            .field("now", &self.now)
            .finish_non_exhaustive()
    }
}

/// Where the user has scrolled each container to, and what is still moving
/// it: the painter's live offsets over the committed frame, with the drags,
/// flings, bounce backs, glides and auto-scrolls that write them
/// ([`inertia`]).
///
/// Public only for the test surface, which reads it through
/// `Painter::scroll_intents` and calls it through [`ScrollIntentsTestUtils`].
#[doc(hidden)]
#[cfg_attr(
    not(feature = "painter-test-utils"),
    expect(
        unreachable_pub,
        reason = "the link names it only for the test surface; a field of `Painter` otherwise"
    )
)]
#[derive(Debug)]
pub struct ScrollIntents {
    /// Each container's live offset where it differs from the committed
    /// one. In range on every axis, except that a `contain-bounce` axis
    /// may stand up to one scrollport past either edge while it is
    /// stretched — a drag past the boundary, a fling's overshoot, or a
    /// bounce back on its way home — and that an
    /// `overscroll-behavior: circular` axis may stand anywhere: it has no
    /// boundary, and its scrolling area repeats every
    /// [`crate::visual::ScrollSlot::wrap_period`]. Such an offset is composed, hit
    /// tested and sampled modulo the period (this crate normalizes it at every
    /// entry point), posted modulo the period and unclamped, which the
    /// document admits as it stands ([`Self::drain_changed`]), and brought
    /// back into `[0, period)` by each rebase, which shifts the drag origins
    /// and glide targets that refer to it by the same amount.
    offsets: FxHashMap<NodeId, Vector2D<f32>>,
    rebased_commit: Option<u64>,
    /// The snap area each container stood on, in the frame the offsets were
    /// last rebased onto: read off that frame's positions as a newer commit
    /// replaces it ([`Self::leave`]), and spent by the rebase onto that
    /// commit, which moves each such container at rest onto its area's new
    /// position (css-scroll-snap-1 §4.1.3). Empty outside that interval.
    /// What a container is snapped to follows from its offset and the frame
    /// it stands on, so nothing tracks it while that frame is at hand; this
    /// holds it for the one step that no longer has the frame.
    snapped: FxHashMap<NodeId, Snapped>,
    generation: u64,
    /// Where each drag found each container it moved: the offset before the
    /// drag's first step on it, keyed by the pointer and the container. The
    /// drag's end settles every entry of its pointer onto a snap position
    /// from there ([`Self::settle`]), and while an entry exists the
    /// container is held — a commit does not re-snap it under the finger.
    /// A fling keeps its drag's entries until it ends.
    gesture_origins: FxHashMap<(PointerId, NodeId), Vector2D<f32>>,
    /// The flings in flight; see [`inertia`].
    flings: SmallVec<[Fling; 1]>,
    /// The stretched axes springing back; see [`inertia`].
    bounce_backs: SmallVec<[BounceBack; 2]>,
    /// The axes moving to a snap position or to a smooth scroll's target;
    /// see [`inertia`].
    glides: SmallVec<[Glide; 2]>,
    /// The axes a script set moving at a constant rate
    /// ([`AutoScrollRequest`]); see [`inertia`]. A `Vec`, not an inline buffer: almost
    /// no page has one, so the painter of a page without holds the three
    /// words and nothing else.
    auto_scrolls: Vec<AutoScroll>,
    /// The newest programmatic scroll request or auto-scroll change handled
    /// per container ([`crate::visual::ScrollSlot::request`],
    /// [`CommittedFrame::auto_scroll_requests`]), which every post for it
    /// names. An
    /// entry is written when a request is first seen and pruned by every
    /// rebase to the containers the adopted frame still has a slot for, so
    /// it is bounded by the live containers that ever received a request.
    handled: FxHashMap<NodeId, NonZeroU64>,
    /// The drags in progress, for their release velocity; see [`inertia`].
    drags: FxHashMap<PointerId, DragTrack>,
    /// The containers a wheel step moved, each with the clock reading of
    /// its latest step: held off rest until
    /// [`inertia::WHEEL_REST_SECONDS`] pass with no further step, so a wheel
    /// gesture ends once rather than at every tick.
    wheel_steps: SmallVec<[(NodeId, f64); 1]>,
    /// The containers whose live offset changed, or whose hold a drag let
    /// go, since the last drain; see [`Painter::drain_changed`]. Bounded by
    /// the containers one pass touches.
    changed: SmallVec<[NodeId; 2]>,
    /// The entries the last publish posted, kept for their capacity.
    posting: SmallVec<[(NodeId, ScrollEntry); 2]>,
}

/// The snap area a container stood on, per axis; `None` on an axis where it
/// stood on no position.
#[derive(Debug, Clone, Copy)]
struct Snapped {
    x: Option<NodeId>,
    y: Option<NodeId>,
}

impl ScrollIntents {
    /// No live offsets, and nothing moving any container.
    fn new() -> Self {
        Self {
            offsets: FxHashMap::default(),
            rebased_commit: None,
            snapped: FxHashMap::default(),
            generation: 0,
            gesture_origins: FxHashMap::default(),
            flings: SmallVec::new(),
            bounce_backs: SmallVec::new(),
            glides: SmallVec::new(),
            auto_scrolls: Vec::new(),
            handled: FxHashMap::default(),
            drags: FxHashMap::default(),
            wheel_steps: SmallVec::new(),
            changed: SmallVec::new(),
            posting: SmallVec::new(),
        }
    }

    /// Reads, as a newer commit replaces `frame`, the snap area each
    /// container stands on in it ([`crate::scroll::SnapAxis::snapped_area`],
    /// from the live offset), for the rebase onto that commit to follow.
    /// Every container is read, moving or not: whether one is at rest is
    /// decided by that rebase.
    ///
    /// Offsets that never stood on `frame` — it was adopted and replaced
    /// with no rebase between — are not read against its positions; what
    /// the frame they do stand on left behind still holds.
    fn leave(&mut self, frame: &CommittedFrame) {
        if self.rebased_commit != Some(frame.commit_id()) {
            return;
        }
        self.snapped.clear();
        for slot in frame.scroll_slots() {
            let (snap_x, snap_y) = frame.snap_axes(slot);
            if snap_x.is_none() && snap_y.is_none() {
                continue;
            }
            let offset = self.offsets.get(&slot.node).copied().unwrap_or(slot.offset);
            let x = snap_x.and_then(|snap| snap.snapped_area(offset.x));
            let y = snap_y.and_then(|snap| snap.snapped_area(offset.y));
            if x.is_some() || y.is_some() {
                self.snapped.insert(slot.node, Snapped { x, y });
            }
        }
    }

    /// Brings the intents onto a newly adopted `frame`, at `now`: re-clamps
    /// every live offset to the frame's ranges — or, on a circular axis,
    /// normalizes it into the period — drops what moves a container the
    /// frame no longer has, carries out the requests the frame brings for
    /// the first time, and settles the snapping containers at rest, each
    /// onto the area it stood on where it stood on one.
    ///
    /// A normalization moves the live offset by whole periods, so the
    /// drag origins and glide targets of that container move with it
    /// ([`Self::shift_motion`]): a release still settles from where the
    /// drag found the container, and a glide still ends on the position it
    /// was aimed at, in the same unwrapped frame as the offset.
    fn rebase(&mut self, frame: &CommittedFrame, now: f64) {
        if self.rebased_commit == Some(frame.commit_id()) {
            return;
        }
        self.rebased_commit = Some(frame.commit_id());
        let changed = &mut self.changed;
        let mut shifts: SmallVec<[(NodeId, Vector2D<f32>); 2]> = SmallVec::new();
        self.offsets.retain(|node, offset| {
            let Some(slot) = frame.slot_of(*node) else {
                return false;
            };
            let slot = &frame.scroll_slots()[slot as usize];
            let (period_x, period_y) = slot.wrap_period();
            let wrapped = slot.wrap(*offset);
            let axis = |value: f32,
                        wrapped: f32,
                        max: f32,
                        extent: f32,
                        bounces: bool,
                        period: Option<f32>| {
                match period {
                    Some(_) => wrapped,
                    None => clamp_intent_axis(value, max, extent, bounces),
                }
            };
            let brought = Vector2D::new(
                axis(
                    offset.x,
                    wrapped.x,
                    slot.max_offset.x,
                    slot.scrollport.width,
                    slot.bounce.x,
                    period_x,
                ),
                axis(
                    offset.y,
                    wrapped.y,
                    slot.max_offset.y,
                    slot.scrollport.height,
                    slot.bounce.y,
                    period_y,
                ),
            );
            if brought != *offset {
                // Whole periods on a circular axis, which what refers to
                // the offset moves by too; a non-finite offset was nowhere,
                // so nothing follows it.
                let shift = |value: f32, brought: f32, period: Option<f32>| {
                    if period.is_some() && value.is_finite() {
                        brought - value
                    } else {
                        0.0
                    }
                };
                let shift = Vector2D::new(
                    shift(offset.x, brought.x, period_x),
                    shift(offset.y, brought.y, period_y),
                );
                if shift != Vector2D::zero() {
                    shifts.push((*node, shift));
                }
                *offset = brought;
                note_changed(changed, *node);
            }
            *offset != slot.offset
        });
        self.gesture_origins
            .retain(|(_, node), _| frame.slot_of(*node).is_some());
        for (node, shift) in shifts {
            self.shift_motion(node, shift);
        }
        self.retain_motion(frame);
        self.apply_requests(frame, now);
        self.settle_at_rest(frame);
    }

    /// Carries out each programmatic scroll and each auto-scroll change
    /// `frame` brings that this painter has not handled yet: its serial is
    /// newer than the one recorded for the container.
    ///
    /// The two do not replace one another — a scroll request says where the
    /// container is, an auto-scroll change whether it moves on from there —
    /// so one container's pair in one frame is both carried out, the scroll
    /// first.
    fn apply_requests(&mut self, frame: &CommittedFrame, now: f64) {
        if !self.handled.is_empty() {
            self.handled
                .retain(|node, _| frame.slot_of(*node).is_some());
        }
        // Chosen against the serials as they stood before this frame: the
        // scroll request below may raise a container's past an auto-scroll
        // change made before it in the same turn.
        let auto_scrolls: SmallVec<[(NodeId, AutoScrollRequest); 1]> = frame
            .auto_scroll_requests()
            .iter()
            .filter(|(node, request)| {
                self.handled
                    .get(node)
                    .is_none_or(|handled| *handled < request.serial)
            })
            .copied()
            .collect();
        for slot in frame.scroll_slots() {
            let Some(request) = slot.request else {
                continue;
            };
            if self
                .handled
                .get(&slot.node)
                .is_some_and(|handled| *handled >= request.serial)
            {
                continue;
            }
            self.handled.insert(slot.node, request.serial);
            self.apply_request(frame, slot, request, now);
        }
        for (node, request) in auto_scrolls {
            let handled = self.handled.entry(node).or_insert(request.serial);
            *handled = (*handled).max(request.serial);
            self.apply_auto_scroll(node, request);
        }
    }

    /// One programmatic scroll of `slot`'s container. The fling or bounce
    /// back moving it stops ([`Self::stop_flings_and_bounces_on`]), while an
    /// auto-scroll goes on from where the request leaves the container and
    /// a glide is kept or ended below. One that yields to a pointer
    /// ([`crate::tree::document::Document::scroll_to_yielding`]) is dropped whole while a drag
    /// holds the container. An instant one
    /// puts the live offset on the target, where a drag holding the
    /// container continues from; a smooth one glides there, to the position
    /// the at-rest snap rule would move the target to, so a snapping
    /// container does not jump at the end — from rest, unless a glide to
    /// that position is already running, which goes on as it is
    /// ([`Self::glide_to_or_keep`]). On a
    /// circular axis the glide runs along the shorter arc of the circle
    /// ([`nearest_copy`]), across the seam when that is shorter. Either way
    /// the container is posted, naming the request.
    fn apply_request(
        &mut self,
        frame: &CommittedFrame,
        slot: &crate::visual::ScrollSlot,
        request: ScrollRequest,
        now: f64,
    ) {
        let node = slot.node;
        if request.yields_to_pointer && self.is_held(node) {
            // A turn no script asked for, under a finger — or under the
            // fling that finger let go, which keeps its holds: nothing
            // moves. Posted all the same, so the request reads as handled.
            note_changed(&mut self.changed, node);
            return;
        }
        self.stop_flings_and_bounces_on(frame, node, now);
        // The request decides where the container goes, not the area it
        // stood on before.
        self.snapped.remove(&node);
        let live = self.offsets.get(&node).copied().unwrap_or(slot.offset);
        match request.behavior {
            ScrollBehavior::Instant => {
                self.stop_glides(node);
                for ((_, held), origin) in &mut self.gesture_origins {
                    if *held == node {
                        *origin = request.target;
                    }
                }
                if request.target == slot.offset {
                    self.offsets.remove(&node);
                } else {
                    self.offsets.insert(node, request.target);
                }
            }
            ScrollBehavior::Smooth => {
                let (snap_x, snap_y) = frame.snap_axes(slot);
                let target = settle_offset(
                    request.target,
                    request.target,
                    slot.scrollport,
                    snap_x,
                    snap_y,
                );
                // `live` is in the period: `rebase` normalizes before `apply_requests`.
                let target = nearest_copy(slot, live, target);
                self.glide_to_or_keep(frame, node, live, target, now);
            }
        }
        note_changed(&mut self.changed, node);
        self.generation += 1;
    }

    /// css-scroll-snap-1 §6.1: a snapping container must rest on a snap
    /// position. Every commit publishes fresh positions — the first layout,
    /// a relayout that moved the areas, a programmatic scroll the document
    /// applied — so each snapping container no drag is holding, no fling is
    /// driving and no bounce back or glide is moving settles from where it
    /// stands, at once, as an intent like any other scroll.
    ///
    /// §4.1.3 comes first: a container that stood on an area's position in
    /// the frame this one replaces ([`Self::leave`]) moves onto that area's
    /// position here, and the at-rest rule then finds it on a position and
    /// leaves it. A container something is moving follows nothing — where
    /// it rests is decided when the drag, fling, bounce back or glide ends —
    /// so what was read for it is dropped with the rest. A container with
    /// an auto-scroll is not at rest either, even while that stands at its
    /// edge for a range that grows: snapped away from the edge it would
    /// move straight back.
    fn settle_at_rest(&mut self, frame: &CommittedFrame) {
        let mut snapped = std::mem::take(&mut self.snapped);
        for slot in frame.scroll_slots() {
            if self.is_held(slot.node)
                || self.is_bouncing(slot.node)
                || self.is_gliding(slot.node)
                || self.has_auto_scroll(slot.node)
            {
                continue;
            }
            if let Some(snapped) = snapped.get(&slot.node) {
                self.follow_snapped(frame, slot, *snapped);
            }
            self.settle_node_at_rest(frame, slot);
        }
        snapped.clear();
        self.snapped = snapped;
    }

    /// css-scroll-snap-1 §4.1.3 for one container at rest: on each axis it
    /// stood on an area's position, it moves to the position `frame` gives
    /// that area ([`crate::scroll::SnapAxis::position_of`]), however far that
    /// is and whatever the strictness. An axis whose area contributes no
    /// position any more keeps its offset, for the at-rest rule. On a
    /// circular axis the position is taken into the period, where the
    /// rebase has just brought the offset: the old offset says nothing
    /// about which copy is near under the new period, and nothing refers to
    /// the offset of a container at rest.
    fn follow_snapped(
        &mut self,
        frame: &CommittedFrame,
        slot: &crate::visual::ScrollSlot,
        snapped: Snapped,
    ) {
        let (snap_x, snap_y) = frame.snap_axes(slot);
        let offset = self.offsets.get(&slot.node).copied().unwrap_or(slot.offset);
        let axis =
            |area: Option<NodeId>, snap: Option<crate::scroll::SnapAxis<'_>>, offset: f32| {
                area.zip(snap)
                    .and_then(|(area, snap)| snap.position_of(area, offset))
                    .unwrap_or(offset)
            };
        let followed = slot.wrap(Vector2D::new(
            axis(snapped.x, snap_x, offset.x),
            axis(snapped.y, snap_y, offset.y),
        ));
        if followed != offset {
            self.write(slot.node, followed);
            self.generation += 1;
        }
    }

    /// The at-rest rule for one container: from where it stands to where it
    /// stands, so a `mandatory` axis moves to its nearest position and a
    /// `proximity` one only when within range. A stretched axis is left to
    /// its bounce back.
    fn settle_node_at_rest(&mut self, frame: &CommittedFrame, slot: &crate::visual::ScrollSlot) {
        let (snap_x, snap_y) = frame.snap_axes(slot);
        if snap_x.is_none() && snap_y.is_none() {
            return;
        }
        let offset = self.offsets.get(&slot.node).copied().unwrap_or(slot.offset);
        let settled = settle_stretch_aware(offset, offset, slot, snap_x, snap_y);
        if settled != offset {
            self.write(slot.node, settled);
            self.generation += 1;
        }
    }

    /// Settles every container `pointer`'s drag moved onto a snap position,
    /// from where the drag found it to where it left it, gliding there
    /// ([`Self::glide_to`]), and releases the hold on them. A stretched axis
    /// is not settled: it springs back first ([`Self::start_bounce_backs`])
    /// and is re-snapped when it lands. An axis already gliding keeps its
    /// glide.
    fn settle(&mut self, frame: &CommittedFrame, pointer: PointerId, now: f64) {
        self.rebase(frame, now);
        let held: SmallVec<[(NodeId, Vector2D<f32>); 2]> = self
            .gesture_origins
            .iter()
            .filter(|((held_by, _), _)| *held_by == pointer)
            .map(|((_, node), origin)| (*node, *origin))
            .collect();
        for (node, origin) in held {
            self.gesture_origins.remove(&(pointer, node));
            let Some(index) = frame.slot_of(node) else {
                continue;
            };
            // Let go whether or not it moves now, so its rest is posted.
            note_changed(&mut self.changed, node);
            let slot = &frame.scroll_slots()[index as usize];
            let (snap_x, snap_y) = frame.snap_axes(slot);
            let offset = self.offsets.get(&node).copied().unwrap_or(slot.offset);
            let settled = settle_stretch_aware(origin, offset, slot, snap_x, snap_y);
            self.glide_to(frame, node, offset, settled, now);
        }
    }

    /// Drives one scroll step from the slot `from` names, outward along the
    /// published chain, and reports per axis what was absorbed and where a
    /// container is now stretched. Order and reach are the document's
    /// [`drive_chain`] over the slots' published policy, and each step
    /// lands per [`resolve_elastic_step`] — [`crate::scroll::resolve_step`]'s
    /// rule, the same walk the document runs over live geometry, plus the
    /// `contain-bounce` stretch the document never takes. A drag or fling
    /// step (`pointer` set) records where it found each container it moved,
    /// and a drag step feeds its release velocity; a wheel step snaps as it
    /// lands, and ends the glides on each container it moves, so where the
    /// wheel put it stands rather than being overwritten by the glide's
    /// next frame. Before the walk, a container on the chain already stretched
    /// on an axis unwinds toward its edge first (the restore-first rule),
    /// whatever the walk's order.
    fn chain(
        &mut self,
        frame: &CommittedFrame,
        from: NodeId,
        delta: Vector2D<f32>,
        motion: Motion,
        pointer: Option<PointerId>,
        at: f64,
    ) -> ChainOutcome {
        self.rebase(frame, at);
        if motion == Motion::Drag
            && let Some(pointer) = pointer
        {
            self.track_drag(frame, pointer, from, delta, at);
        }
        let slots = frame.scroll_slots();
        let Some(start) = frame.slot_of(from) else {
            return ChainOutcome::default();
        };
        let mut indices: SmallVec<[u32; 4]> = SmallVec::new();
        let mut links: SmallVec<[ChainLink; 4]> = SmallVec::new();
        let mut current = Some(start);
        while let Some(index) = current {
            let slot = &slots[index as usize];
            indices.push(index);
            links.push(slot.link());
            current = slot.parent;
        }
        let mut stretched = ScrollAxes::NONE;
        let mut unwound = Vector2D::zero();
        let mut delta = delta;
        for axis in Axis::BOTH {
            if axis.of(delta) == 0.0 {
                continue;
            }
            for &index in &indices {
                let slot = &slots[index as usize];
                if !axis.flag(slot.bounce) {
                    continue;
                }
                let offset = self.offsets.get(&slot.node).copied().unwrap_or(slot.offset);
                let (applied, absorbed) = unwind_stretch(
                    motion,
                    axis.of(offset),
                    axis.of(delta),
                    axis.of(slot.max_offset),
                    axis.extent(slot.scrollport),
                );
                if absorbed == 0.0 {
                    continue;
                }
                let mut moved = offset;
                axis.set(&mut moved, applied);
                if let Some(pointer) = pointer {
                    self.gesture_origins
                        .entry((pointer, slot.node))
                        .or_insert(offset);
                }
                if motion == Motion::Wheel {
                    self.stop_glides(slot.node);
                    self.note_wheel_step(slot.node, at);
                } else {
                    self.end_wheel_steps(slot.node);
                }
                self.write(slot.node, moved);
                if stretch_of(applied, axis.of(slot.max_offset)) != 0.0 {
                    axis.raise(&mut stretched);
                }
                axis.set(&mut unwound, absorbed);
                let remaining = axis.of(delta) - absorbed;
                axis.set(&mut delta, remaining);
                break;
            }
        }
        let consumed = drive_chain(&links, delta, |link, admitted| {
            let slot = &slots[indices[link] as usize];
            let offset = self.offsets.get(&slot.node).copied().unwrap_or(slot.offset);
            let step = slot_step(frame, slot, motion, offset, admitted);
            stretched.x |= step.stretched.x;
            stretched.y |= step.stretched.y;
            if step.applied != offset {
                if let Some(pointer) = pointer {
                    self.gesture_origins
                        .entry((pointer, slot.node))
                        .or_insert(offset);
                }
                if motion == Motion::Wheel {
                    self.stop_glides(slot.node);
                    self.note_wheel_step(slot.node, at);
                } else {
                    self.end_wheel_steps(slot.node);
                }
                self.write(slot.node, step.applied);
            }
            step.absorbed
        });
        let outcome = ChainOutcome {
            absorbed: unwound + consumed.map_or_else(Vector2D::zero, |(_, absorbed)| absorbed),
            stretched,
        };
        if outcome.consumed() {
            self.generation += 1;
            // A step that landed a container back in range ends the
            // bounce back that was bringing it there.
            self.retain_motion(frame);
        }
        outcome
    }

    /// The live offset of `node`'s container, where it differs from the
    /// committed one.
    #[must_use]
    fn offset_for(&self, node: NodeId) -> Option<Vector2D<f32>> {
        self.offsets.get(&node).copied()
    }

    /// The one write of a live offset, which records the container for the
    /// next publish.
    fn write(&mut self, node: NodeId, offset: Vector2D<f32>) {
        self.offsets.insert(node, offset);
        note_changed(&mut self.changed, node);
    }

    /// Answers the changed containers as the document adopts them — clamped
    /// to the committed range, and `rest` where nothing holds, flings,
    /// bounces, glides or auto-scrolls them — and forgets them. A container
    /// `frame` no longer carries is dropped. Both buffers keep their
    /// capacity.
    ///
    /// An axis with a [`crate::visual::ScrollSlot::wrap_period`] is normalized into
    /// its period and posted unclamped: the document admits the whole
    /// period on such an axis, so while the scrollport straddles the seam —
    /// the normalized offset past `max_offset` — the document holds the
    /// offset the screen shows, and its readbacks, its `scrollIntoView` and
    /// its scroll timeline samples agree with the painter. Every other axis
    /// is clamped, so the document sees the edge a `contain-bounce` stretch
    /// stands past.
    fn drain_changed(
        &mut self,
        frame: Option<&CommittedFrame>,
        at: f64,
    ) -> smallvec::Drain<'_, [(NodeId, ScrollEntry); 2]> {
        let mut posting = std::mem::take(&mut self.posting);
        if let Some(frame) = frame {
            for &node in &self.changed {
                let Some(index) = frame.slot_of(node) else {
                    continue;
                };
                let slot = &frame.scroll_slots()[index as usize];
                let live = slot.wrap(self.offsets.get(&node).copied().unwrap_or(slot.offset));
                let wrap = wrap_axes(slot);
                let axis = |value: f32, max: f32, wraps: bool| {
                    if wraps {
                        value
                    } else {
                        clamp_scroll_axis(value, max)
                    }
                };
                let offset = Vector2D::new(
                    axis(live.x, slot.max_offset.x, wrap.x),
                    axis(live.y, slot.max_offset.y, wrap.y),
                );
                let moving = self.is_held(node)
                    || self.is_flinging(frame, node)
                    || self.is_bouncing(node)
                    || self.is_gliding(node)
                    || self.is_auto_scrolling_node(frame, node)
                    || self.is_wheeling(node);
                posting.push((
                    node,
                    ScrollEntry {
                        offset,
                        at,
                        rest: (!moving).then_some(offset),
                        request: self.handled.get(&node).copied(),
                    },
                ));
            }
        }
        self.changed.clear();
        self.posting = posting;
        self.posting.drain(..)
    }
}

/// Records `node` once among the containers a pass changed.
fn note_changed(changed: &mut SmallVec<[NodeId; 2]>, node: NodeId) {
    if !changed.contains(&node) {
        changed.push(node);
    }
}

fn clamp_scroll_axis(value: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, max)
    } else {
        0.0
    }
}

/// [`resolve_elastic_step`] for one chain step on `slot`'s container,
/// standing at `offset`, over the slot's published geometry, policy and
/// snap data.
fn slot_step(
    frame: &CommittedFrame,
    slot: &crate::visual::ScrollSlot,
    motion: Motion,
    offset: Vector2D<f32>,
    admitted: Vector2D<f32>,
) -> ElasticStep {
    let (snap_x, snap_y) = frame.snap_axes(slot);
    resolve_elastic_step(
        motion,
        offset,
        admitted,
        slot.max_offset,
        slot.scrollport,
        slot.bounce,
        wrap_axes(slot),
        snap_x,
        snap_y,
    )
}

/// `target` on each axis `slot` wraps, moved to its copy nearest `live`: a
/// whole period down when it lies more than half a period ahead of `live`,
/// a whole period up when more than half a period behind, and unchanged
/// otherwise (a tie keeps it). A glide from `live` to the result runs along
/// the shorter arc of the circle, across the seam when that is shorter; a
/// glide target outside `[0, period)` is brought back with the offset by
/// the next rebase ([`ScrollIntents::shift_motion`]). A bounded axis keeps
/// its target.
fn nearest_copy(
    slot: &crate::visual::ScrollSlot,
    live: Vector2D<f32>,
    target: Vector2D<f32>,
) -> Vector2D<f32> {
    let (period_x, period_y) = slot.wrap_period();
    let axis = |live: f32, target: f32, period: Option<f32>| match period {
        Some(period) if target - live > period / 2.0 => target - period,
        Some(period) if target - live < -period / 2.0 => target + period,
        _ => target,
    };
    Vector2D::new(
        axis(live.x, target.x, period_x),
        axis(live.y, target.y, period_y),
    )
}

/// The axes on which `slot` wraps: circular, with content to scroll
/// ([`crate::visual::ScrollSlot::wrap_period`]). Everything the intents do treats
/// only these as boundless; a circular axis without overflow clamps like
/// any other.
fn wrap_axes(slot: &crate::visual::ScrollSlot) -> ScrollAxes {
    let (x, y) = slot.wrap_period();
    ScrollAxes {
        x: x.is_some(),
        y: y.is_some(),
    }
}

/// Where an intent may stand on one bounded axis: in range, or — on a
/// `contain-bounce` axis — up to one scrollport past either edge.
fn clamp_intent_axis(value: f32, max: f32, extent: f32, bounces: bool) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    if bounces {
        value.clamp(-extent, max + extent)
    } else {
        value.clamp(0.0, max)
    }
}

/// [`settle_offset`] over an intent that may be stretched: a stretched
/// axis keeps its stretch (its bounce back settles it when it lands), and
/// the rest settle from an in-range `start`.
///
/// A wrapped axis is never stretched and its `start` is not clamped: a
/// drag origin there may stand outside `0..=max` in the same unwrapped
/// frame as `offset`, and the axis's snap positions repeat every period,
/// so the settle finds the copy nearest where the gesture ended.
fn settle_stretch_aware(
    start: Vector2D<f32>,
    offset: Vector2D<f32>,
    slot: &crate::visual::ScrollSlot,
    snap_x: Option<crate::scroll::SnapAxis<'_>>,
    snap_y: Option<crate::scroll::SnapAxis<'_>>,
) -> Vector2D<f32> {
    let wrap = wrap_axes(slot);
    let start_axis = |value: f32, max: f32, wraps: bool| {
        if wraps && value.is_finite() {
            value
        } else {
            clamp_scroll_axis(value, max)
        }
    };
    let start = Vector2D::new(
        start_axis(start.x, slot.max_offset.x, wrap.x),
        start_axis(start.y, slot.max_offset.y, wrap.y),
    );
    let stretched_x = !wrap.x && stretch_of(offset.x, slot.max_offset.x) != 0.0;
    let stretched_y = !wrap.y && stretch_of(offset.y, slot.max_offset.y) != 0.0;
    let settled = settle_offset(
        start,
        Vector2D::new(
            if stretched_x { start.x } else { offset.x },
            if stretched_y { start.y } else { offset.y },
        ),
        slot.scrollport,
        snap_x,
        snap_y,
    );
    Vector2D::new(
        if stretched_x { offset.x } else { settled.x },
        if stretched_y { offset.y } else { settled.y },
    )
}

/// Composes one frame and renders it into `output`, whatever kind of target
/// that is.
///
/// The one render path: a frame is composed flat, with each scroll slot at
/// the offset the intents carry for it. A frame whose whole program is one
/// unscrolled fragment, asked for at no offset and no animation instant, is
/// rendered straight out of the commit — composing it would copy it.
#[expect(
    clippy::too_many_arguments,
    reason = "one frame's whole path to pixels: the target, the three compose buffers, the \
              frame, and the pixel table it draws from"
)]
fn compose_and_render(
    output: &mut Output,
    buffer: &mut Scene,
    filtered: &mut Vec<Option<ImageData>>,
    vectors: &mut Vec<Option<ImageData>>,
    intents: &ScrollIntents,
    frame: &CommittedFrame,
    images: &[Option<ImageData>],
    size: FrameSize,
    animation_now: Option<f64>,
) -> Result<(), PainterError> {
    // The two optional pre-steps, each skipped on one test. A frame drawing
    // no SVG document bakes no vector texture; a frame with no
    // `filter: blur()` group and no `backdrop-filter` element — the
    // overwhelming majority — bakes no filter and touches no offscreen
    // texture at all. The vectors come first: a filter's range may draw one.
    if frame.draws_vectors() {
        output.prepare_vectors(frame, vectors)?;
    } else {
        vectors.clear();
    }
    if frame.filter_groups().is_empty() {
        filtered.clear();
    } else {
        output.prepare_filters(
            frame,
            images,
            vectors,
            &|slot| intents.offset_for(slot.node),
            intents.generation,
            animation_now,
            filtered,
        )?;
    }
    let scene = if intents.offsets.is_empty()
        && animation_now.is_none()
        && let Some(scene) = frame.scene()
    {
        // A filtered frame is never one unscrolled fragment — it carries at
        // least one filter bracket or backdrop op — so this fast path never
        // skips a bake.
        scene
    } else {
        buffer.reset();
        frame.compose_into(
            buffer,
            images,
            filtered,
            vectors,
            &|slot| intents.offset_for(slot.node),
            animation_now,
        );
        &*buffer
    };
    output.render(scene, images, size)
}

fn route_published(
    frame: Option<&CommittedFrame>,
    intents: &ScrollIntents,
    event: &InputEvent,
    animation_now: Option<f64>,
) -> Option<HitTarget> {
    // A key event is not routed by position: the router targets it at the
    // focus target itself.
    if matches!(event.kind, InputKind::Key { .. }) {
        return None;
    }
    let finite = event.position.x.is_finite()
        && event.position.y.is_finite()
        && match event.kind {
            InputKind::Wheel { delta } => delta.x.is_finite() && delta.y.is_finite(),
            _ => true,
        };
    if !finite {
        debug_assert!(false, "host input events must be finite, got {event:?}");
        return None;
    }
    frame?.hit(
        event.position,
        &|slot| intents.offset_for(slot.node),
        animation_now,
    )
}

impl Painter {
    /// Builds a painter over one draw target, on the thread that will draw
    /// into it — the only thread macOS lets a surface be created from — that
    /// synthesizes gestures on `gestures`.
    ///
    /// It shows nothing yet: [`Self::adopt`] hands it a frame.
    ///
    /// # Errors
    ///
    /// [`PainterError::Viewport`] if the metrics are not finite and positive
    /// or the physical target would exceed 16384 pixels on either axis, and
    /// [`PainterError::Gpu`] or [`PainterError::Render`] if the target itself
    /// cannot be built.
    pub async fn new(
        target: DrawTarget,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        gestures: GesturePolicy,
    ) -> Result<Self, PainterError> {
        let frame_size = FrameSize::for_viewport(width, height, device_pixel_ratio)?;
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        let output = Output::build(target, frame_size).await?;
        Ok(Self::with_output(viewport, frame_size, output, gestures))
    }

    fn with_output(
        viewport: Viewport,
        frame_size: FrameSize,
        output: Output,
        gestures: GesturePolicy,
    ) -> Self {
        Self {
            viewport,
            frame_size,
            output,
            occluded: false,
            render_failed: false,
            frame: None,
            redraw_pending: Cell::new(false),
            gesture: GestureRouter::new(gestures),
            clock: FrameClock::new(),
            scroll_intents: ScrollIntents::new(),
            composed: None,
            composed_scene: Scene::new(),
            composed_filters: Vec::new(),
            composed_vectors: Vec::new(),
            images: images::PainterImages::default(),
            paint_timing: PaintTiming::default(),
            thread_bound: PhantomData,
        }
    }

    /// The CSS metrics this painter draws at.
    #[must_use]
    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// The physical pixel size of this painter's draw target.
    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    /// Whether this painter presents into a window.
    #[must_use]
    pub const fn presents_to_window(&self) -> bool {
        matches!(self.output, Output::Window(_))
    }

    /// Whether this painter renders into a texture nothing displays.
    #[must_use]
    pub const fn is_offscreen(&self) -> bool {
        matches!(self.output, Output::Offscreen(_))
    }

    /// Whether this painter's draw target has failed. A failed painter draws
    /// nothing further.
    #[must_use]
    pub const fn has_failed(&self) -> bool {
        self.render_failed
    }

    /// Whether the window is reported invisible ([`Self::set_occluded`]).
    #[must_use]
    pub const fn is_occluded(&self) -> bool {
        self.occluded
    }

    /// Moves the frame clock's epoch onto a document's own timeline
    /// ([`FrameClock::rebase`]).
    pub fn rebase_clock(&mut self, epoch: ClockInstant) {
        self.clock.rebase(epoch);
    }

    /// The frame clock's reading, in seconds on the timeline it was last
    /// rebased onto.
    #[must_use]
    pub fn now_seconds(&self) -> f64 {
        self.clock.now_seconds()
    }

    /// The newest committed frame this painter has adopted.
    #[must_use]
    pub fn frame(&self) -> Option<&Arc<CommittedFrame>> {
        self.frame.as_ref()
    }

    /// Adopts `frame`, the newest a document has published, together with
    /// the pixels it draws, read out of `store`; answers whether it did.
    ///
    /// A frame of the commit already adopted replaces it as it stands. A new
    /// commit's pixels are read unless this painter already holds them: the
    /// two are one fact — a frame indexes its store's bitmaps by draw order,
    /// so a frame from one commit over another commit's table draws the wrong
    /// images. So a new commit whose pixels are not held is refused when
    /// there is no `store` to read them from — the document has gone, and its
    /// store with it — and the painter keeps the last commit it did read,
    /// whole. An adopted new commit asks for a redraw, and the snap area each
    /// container stood on in the outgoing frame is read while that frame is
    /// still at hand, for the rebase onto the new one (css-scroll-snap-1
    /// §4.1.3).
    ///
    /// **May block.** A store is allowed — required, in fact — to restore a
    /// bitmap it evicted, and adopting a commit is the read that asks it to.
    /// Call it before the drawing path acquires a swap-chain image, so a
    /// restore cannot stall the chain under vsync.
    pub fn adopt(
        &mut self,
        frame: Option<Arc<CommittedFrame>>,
        store: Option<&dyn FrameImages>,
    ) -> bool {
        let commit = frame.as_ref().map(|frame| frame.commit_id());
        if commit != self.frame.as_ref().map(|frame| frame.commit_id()) {
            if let Some(incoming) = frame.as_deref()
                && !self.images.holds(incoming.commit_id())
            {
                let Some(store) = store else {
                    return false;
                };
                self.images.resolve(incoming, store);
            }
            // While the outgoing frame's snap positions can still be read.
            if let Some(outgoing) = self.frame.as_deref() {
                self.scroll_intents.leave(outgoing);
            }
            self.redraw_pending.set(true);
        }
        self.frame = frame;
        true
    }

    /// Drops everything derived from the document this painter showed: the
    /// adopted frame, where it was scrolled to, the gesture in progress, and
    /// the pixels resolved out of its store.
    ///
    /// Not what the target holds — that is [`Self::forget_target`], because
    /// the pixels on screen survive the document they came from.
    ///
    /// What the intents had not drained yet is dropped with them, and so are
    /// the presentation records ([`Self::last_presented`],
    /// [`Self::first_paint`], [`Self::first_contentful_paint`]): they are on
    /// the shown view's timeline, so a painter attached to another view
    /// starts that view's first paint afresh.
    pub fn forget_view(&mut self) {
        self.frame = None;
        self.scroll_intents = ScrollIntents::new();
        self.gesture = GestureRouter::new(self.gesture.policy());
        self.images.forget();
        self.paint_timing.forget();
    }

    /// Turns the presentation records ([`Self::last_presented`],
    /// [`Self::first_paint`], [`Self::first_contentful_paint`]) on or off for
    /// the draws that follow. On by default.
    ///
    /// Off, a draw reads no clock for them and records nothing: the records
    /// stay as they were, and a frame drawn while it was off is never
    /// recorded, not even as a first mark once it is turned on again — the
    /// next frame drawn while it is on is. An embedder that reports
    /// presentation times turns it on while something waits for one.
    pub fn set_presentation_timing(&mut self, enabled: bool) {
        self.paint_timing.enabled = enabled;
    }

    /// Whether draws record their presentation
    /// ([`Self::set_presentation_timing`]).
    #[must_use]
    pub const fn presentation_timing(&self) -> bool {
        self.paint_timing.enabled
    }

    /// The last frame this painter presented ([`PresentedFrame`]): a window
    /// draw that rendered, or an offscreen draw that did. A window draw that
    /// re-presents the frame its target holds, a capture, a draw that
    /// failed and a draw while presentation timing was off
    /// ([`Self::set_presentation_timing`]) record nothing. `None` until the
    /// shown view's first such draw.
    #[must_use]
    pub const fn last_presented(&self) -> Option<PresentedFrame> {
        self.paint_timing.last_presented
    }

    /// Paint Timing's `first-paint`: the first frame this painter presented
    /// for the shown view, whatever it drew, among the frames drawn while
    /// presentation timing was on ([`Self::set_presentation_timing`]). Set
    /// once per view ([`Self::forget_view`] clears it).
    #[must_use]
    pub const fn first_paint(&self) -> Option<PresentedFrame> {
        self.paint_timing.first_paint
    }

    /// Paint Timing's `first-contentful-paint`: the first frame this painter
    /// presented for the shown view, while presentation timing was on
    /// ([`Self::set_presentation_timing`]), whose commit
    /// [`paints_content`](CommittedFrame::paints_content). A commit built
    /// while its document's content detection was off
    /// (`Document::set_content_detection`) answers `false`, so this stays
    /// `None` until a frame built with detection on is presented. Set once
    /// per view ([`Self::forget_view`] clears it).
    #[must_use]
    pub const fn first_contentful_paint(&self) -> Option<PresentedFrame> {
        self.paint_timing.first_contentful_paint
    }

    /// Drops what the draw target holds: the key it was rendered from, and —
    /// for an offscreen target, whose texture is the only copy of that frame —
    /// the frame itself.
    ///
    /// What a painter does before it is pointed at a different document.
    /// Commit ids restart at one per document, so a key kept across the
    /// change would answer the new page's first frame with the old page's
    /// work — which is also why `Output::forget` gives up the target's
    /// `filter: blur()` bake cache, keyed the same way.
    pub fn forget_target(&mut self) {
        self.composed = None;
        self.composed_filters.clear();
        self.composed_vectors.clear();
        self.output.forget();
    }

    /// Whether this painter owes the timeline another frame on its own
    /// account: the adopted frame animates, a gesture deadline is armed, the
    /// scroll intents are moving, or an auto-scroll can move. `frame_demand`
    /// is the owner's own term, asked after the frame's animations and before
    /// the rest, and only when the frame's animations do not already answer.
    #[must_use]
    pub fn is_animating(&self, frame_demand: impl FnOnce() -> bool) -> bool {
        self.frame
            .as_ref()
            .is_some_and(|frame| frame.animations_active())
            || frame_demand()
            || self.gesture.needs_frame()
            || self.scroll_intents.is_animating()
            || self
                .frame
                .as_ref()
                .is_some_and(|frame| self.scroll_intents.is_auto_scrolling(frame))
    }

    /// Whether this painter has a frame to put on its window: it animates
    /// ([`Self::is_animating`], with `frame_demand`), or something asked for a
    /// frame no turn has produced yet.
    ///
    /// Always false for a window nobody can see, a target that failed, and a
    /// painter that presents to no window at all.
    #[must_use]
    pub fn owes_frame(&self, frame_demand: impl FnOnce() -> bool) -> bool {
        if self.render_failed || self.occluded || !self.presents_to_window() {
            return false;
        }
        self.is_animating(frame_demand) || self.redraw_pending.get()
    }

    /// Routes one normalized OS input event against the adopted frame, at the
    /// clock's reading now, and answers that reading.
    ///
    /// A pointer or wheel event is hit-tested at the live offsets, rebased
    /// onto the adopted frame first; the router decides on it against `host`,
    /// and its decisions are applied ([`Self::apply_decisions`]): the scrolls
    /// move the intents, and every event the router decided is appended to
    /// `emits`, in order, for the caller to deliver. A pass that moved an
    /// offset or armed a gesture deadline asks for a frame. A key event is not
    /// routed by position and moves no scroll: it is named and appended, and
    /// nothing of the pointer pipeline runs for it.
    pub fn route_input(
        &mut self,
        event: &InputEvent,
        host: &impl RouterHost,
        emits: &mut Vec<EmitEvent>,
    ) -> f64 {
        let at = self.clock.now_seconds();
        let mut decisions = InputDecisions::new();
        if matches!(event.kind, InputKind::Key { .. }) {
            self.gesture.on_input(event, None, at, host, &mut decisions);
            self.apply_decisions(&mut decisions, None, at, emits);
            return at;
        }
        let adopted = self.frame.clone();
        if let Some(frame) = &adopted {
            self.scroll_intents.rebase(frame, at);
        }
        let generation = self.scroll_intents.generation;
        let frame = adopted.as_deref();
        let animation_now = frame.and_then(|frame| frame.has_exported_curves().then_some(at));
        let target = route_published(frame, &self.scroll_intents, event, animation_now);
        self.gesture
            .on_input(event, target, at, host, &mut decisions);
        self.apply_decisions(&mut decisions, frame, at, emits);
        if self.gesture.needs_frame() || self.scroll_intents.generation != generation {
            self.refresh();
        }
        at
    }

    /// Applies one pass's decisions in order, at `at_seconds`, the clock
    /// reading of the pass that produced them — an input event's arrival, or
    /// the tick's own `now`.
    ///
    /// A scroll decision drives the chain over `frame` (and drives nothing
    /// without one), reporting a drag's consumed scroll back to the router; a
    /// scroll end settles what the drag moved; an emit is appended to `emits`
    /// for the caller to deliver after the pass. It is one reading for the
    /// whole pass, so a due hold gesture flushed ahead of the event that found
    /// it and the press gesture synthesized after it belong to the same
    /// moment.
    pub fn apply_decisions(
        &mut self,
        decisions: &mut InputDecisions,
        frame: Option<&CommittedFrame>,
        at_seconds: f64,
        emits: &mut Vec<EmitEvent>,
    ) {
        let Self {
            gesture,
            scroll_intents,
            ..
        } = self;
        for decision in decisions.drain(..) {
            match decision {
                InputDecision::Scroll {
                    pointer,
                    from,
                    delta,
                } => {
                    let motion = match pointer {
                        Some(_) => Motion::Drag,
                        None => Motion::Wheel,
                    };
                    let consumed = frame.is_some_and(|frame| {
                        scroll_intents
                            .chain(frame, from, delta, motion, pointer, at_seconds)
                            .consumed()
                    });
                    if consumed && let Some(pointer) = pointer {
                        gesture.note_scroll_consumed(pointer);
                    }
                }
                InputDecision::ScrollEnd { pointer } => {
                    if let Some(frame) = frame {
                        scroll_intents.end_drag(frame, pointer, at_seconds);
                    }
                }
                InputDecision::Emit(event) => emits.push(event),
            }
        }
    }

    /// Runs the gesture clock at `now` over the adopted frame: a due hold
    /// deadline fires against `host`, and its decisions are applied, its
    /// events appended to `emits`. Polls for nothing, so the drawing path may
    /// call it while it holds a swap-chain image.
    ///
    /// The scroll motion advances separately ([`Self::tick_scroll`]), after
    /// the caller has delivered those events.
    pub fn tick_gestures(&mut self, now: f64, host: &impl RouterHost, emits: &mut Vec<EmitEvent>) {
        let adopted = self.frame.clone();
        let mut decisions = InputDecisions::new();
        self.gesture.on_tick(now, host, &mut decisions);
        self.apply_decisions(&mut decisions, adopted.as_deref(), now, emits);
    }

    /// Advances the flings, bounce backs, glides and auto-scrolls over the
    /// adopted frame to `now`.
    pub fn tick_scroll(&mut self, now: f64) {
        if let Some(frame) = &self.frame {
            self.scroll_intents.tick(frame, now);
        }
    }

    /// Brings the scroll intents onto the adopted frame at `now`
    /// (`ScrollIntents::rebase`): re-clamped to its ranges, its first-seen
    /// requests carried out, its snapping containers at rest settled. Once
    /// per commit; a no-op for a frame already rebased onto.
    pub fn rebase_scroll(&mut self, now: f64) {
        if let Some(frame) = &self.frame {
            self.scroll_intents.rebase(frame, now);
        }
    }

    /// Whether a container's live offset changed, or a drag let go of one,
    /// since the last drain.
    #[must_use]
    pub fn scroll_changed(&self) -> bool {
        !self.scroll_intents.changed.is_empty()
    }

    /// Forgets the containers that changed since the last drain, posting
    /// nothing: what a painter with nowhere to post them does.
    pub fn discard_changed(&mut self) {
        self.scroll_intents.changed.clear();
    }

    /// Hands out each container that changed since the last drain, as the
    /// document adopts it at `at`, and forgets them: clamped to the adopted
    /// frame's range, and `rest` where nothing holds, flings, bounces, glides
    /// or auto-scrolls it. A container the adopted frame no longer carries is
    /// dropped.
    ///
    /// An axis with a [`crate::visual::ScrollSlot::wrap_period`] is normalized into
    /// its period and posted unclamped: the document admits the whole period
    /// on such an axis, so while the scrollport straddles the seam it holds
    /// the offset the screen shows. Every other axis is clamped, so the
    /// document sees the edge a `contain-bounce` stretch stands past.
    pub fn drain_changed(&mut self, at: f64) -> impl Iterator<Item = (NodeId, ScrollEntry)> + '_ {
        self.scroll_intents.drain_changed(self.frame.as_deref(), at)
    }

    /// Asks for a frame nothing else would have asked for.
    ///
    /// It wakes nobody: every caller is on this thread, inside a call the
    /// host is already making, so the turn that host is in is the turn that
    /// answers it.
    pub fn refresh(&self) {
        self.redraw_pending.set(true);
    }

    /// Takes the frame asked for, answering whether one was.
    pub fn take_redraw(&self) -> bool {
        self.redraw_pending.replace(false)
    }

    /// Reports whether the window is visible. A window nobody can see draws
    /// nothing, and un-occluding asks again for the frame that was held back.
    pub fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        if !occluded {
            self.refresh();
        }
    }

    /// Applies new device metrics, if they moved at all, and answers whether
    /// they did; metrics that moved ask for a frame.
    ///
    /// The size is validated first, so a target the painter could not render
    /// is refused before anything else has seen it.
    ///
    /// # Errors
    ///
    /// [`PainterError::Viewport`] if the metrics are not finite and positive,
    /// or if the physical target would exceed 16384 pixels on either axis.
    pub fn resize(
        &mut self,
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
    ) -> Result<bool, PainterError> {
        let next_size = FrameSize::for_viewport(width, height, device_pixel_ratio)?;
        let moved = self.viewport.width.to_bits() != width.to_bits()
            || self.viewport.height.to_bits() != height.to_bits()
            || self.viewport.device_pixel_ratio.to_bits() != device_pixel_ratio.to_bits();
        if !moved {
            return Ok(false);
        }
        self.viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        self.frame_size = next_size;
        self.refresh();
        Ok(true)
    }

    /// Composes `frame` at `key` and renders it into the draw target,
    /// answering when it composed if it rendered anything.
    ///
    /// The one path from a committed frame to pixels, whatever the target and
    /// whoever asked: a window's draw, an offscreen draw, and a capture of
    /// either. The caller passes the frame it derived `key` and
    /// `animation_now` from, so what the record names and what the target
    /// holds cannot disagree.
    ///
    /// # Errors
    ///
    /// [`PainterError::Render`] or [`PainterError::Gpu`] if the target refuses
    /// the scene, in which case it holds nothing: a failed render is allowed to
    /// have dropped what was there.
    fn render_frame(
        &mut self,
        frame: &CommittedFrame,
        key: ComposeKey,
        animation_now: Option<f64>,
        force: bool,
    ) -> Result<Option<Rendered>, PainterError> {
        let size = self.frame_size;
        if self.composed == Some((key, size)) && !force && animation_now.is_none() {
            return Ok(None);
        }
        let start = self.paint_timing.enabled.then(ClockInstant::now);
        let Self {
            output,
            composed_scene,
            composed_filters,
            composed_vectors,
            scroll_intents,
            images,
            ..
        } = &mut *self;
        let rendered = compose_and_render(
            output,
            composed_scene,
            composed_filters,
            composed_vectors,
            scroll_intents,
            frame,
            images.resolved(),
            size,
            animation_now,
        );
        match rendered {
            Ok(()) => {
                let span = start.map(|start| ComposeSpan {
                    start,
                    end: ClockInstant::now(),
                });
                self.composed = Some((key, size));
                Ok(Some(Rendered { span }))
            }
            Err(error) => {
                self.composed = None;
                Err(error)
            }
        }
    }

    /// Starts one window frame: takes the swap-chain image the adopted frame
    /// will be presented in, and reads the clock the frame is produced at.
    ///
    /// `None` when there is nothing to put on the window: nothing has been
    /// adopted yet (acquiring an image for it would wait on vsync natively
    /// and, in a browser, present an untouched one — blanking whatever the
    /// previous page left on the canvas), the swap chain has no image to give
    /// this frame (the frame stays owed — [`Self::refresh`] — and the host
    /// takes it at its next display frame, so an empty swap chain does not
    /// spin), the window is occluded, or this painter presents to no window.
    ///
    /// Between this and [`Self::finish_draw`] the caller runs the frame's
    /// gesture clock and posts its scroll state; nothing it does there may
    /// adopt a newer frame.
    ///
    /// # Errors
    ///
    /// [`PainterError::Render`] if the surface was lost; the target has then
    /// failed for good ([`Self::has_failed`]).
    pub fn begin_draw(&mut self) -> Result<Option<WindowFrame>, PainterError> {
        if self.occluded {
            return Ok(None);
        }
        let Some(frame) = self.frame.clone() else {
            return Ok(None);
        };
        let size = self.frame_size;
        let Output::Window(graphics) = &mut self.output else {
            return Ok(None);
        };
        let acquired = match graphics.acquire(size) {
            Ok(FrameAcquisition::Ready(acquired)) => acquired,
            Ok(FrameAcquisition::Retry) => {
                self.refresh();
                return Ok(None);
            }
            Err(error) => {
                self.render_failed = true;
                return Err(error);
            }
        };
        let now = self.clock.now_seconds();
        Ok(Some(WindowFrame {
            acquired,
            frame,
            now,
        }))
    }

    /// Composes and renders the frame [`Self::begin_draw`] started, at its
    /// clock reading and the intents' offsets now, and presents it. Presented
    /// whether or not this rendered: a surface that was invalidated or
    /// re-exposed puts the retained frame up again with a blit alone.
    ///
    /// # Errors
    ///
    /// [`PainterError::Render`] or [`PainterError::Gpu`] if the render fails;
    /// the target has then failed for good ([`Self::has_failed`]).
    pub fn finish_draw(&mut self, window_frame: WindowFrame) -> Result<(), PainterError> {
        let WindowFrame {
            acquired,
            frame,
            now,
        } = window_frame;
        let key: ComposeKey = (frame.commit_id(), self.scroll_intents.generation);
        let animation_now = frame.has_live_curves().then_some(now);
        let rendered = match self.render_frame(&frame, key, animation_now, false) {
            Ok(rendered) => rendered,
            Err(error) => {
                self.render_failed = true;
                return Err(error);
            }
        };
        let Output::Window(graphics) = &mut self.output else {
            unreachable!("a window frame is only begun on a window");
        };
        graphics.present(acquired);
        if let Some(span) = rendered.and_then(|rendered| rendered.span) {
            self.paint_timing.note(&frame, span, ClockInstant::now());
        }
        Ok(())
    }

    /// Renders the adopted frame into an offscreen target at `now`, waiting
    /// for the submitted work, and answers whether it rendered: a frame the
    /// target already holds is rendered again only when `force` is set or
    /// it animates. Nothing adopted, or a painter that presents into a
    /// window, renders nothing here and answers `false`.
    ///
    /// The submitted work is waited for here rather than in the render
    /// itself: a capture's readback polls the device on its own, and this is
    /// the caller that asks for pixels nothing reads back.
    ///
    /// # Errors
    ///
    /// [`PainterError::Gpu`] or [`PainterError::Render`] if the render fails.
    pub fn draw_offscreen(&mut self, now: f64, force: bool) -> Result<bool, PainterError> {
        if !self.is_offscreen() {
            return Ok(false);
        }
        let Some(frame) = self.frame.clone() else {
            return Ok(false);
        };
        let key: ComposeKey = (frame.commit_id(), self.scroll_intents.generation);
        let animation_now = frame.has_live_curves().then_some(now);
        let Some(rendered) = self.render_frame(&frame, key, animation_now, force)? else {
            return Ok(false);
        };
        let Output::Offscreen(gpu) = &self.output else {
            unreachable!("the offscreen output was just checked");
        };
        gpu.wait_idle()
            .map_err(|error| PainterError::Gpu(error.to_string()))?;
        if let Some(span) = rendered.span {
            self.paint_timing.note(&frame, span, ClockInstant::now());
        }
        Ok(true)
    }

    /// Reads back what this painter's target holds, after bringing it up to
    /// the adopted frame at `now`, so a capture is of what this painter would
    /// draw rather than of whatever it last drew.
    ///
    /// # Errors
    ///
    /// [`PainterError::Render`] if nothing has been rendered at the current
    /// size to read back, and [`PainterError::Gpu`] or
    /// [`PainterError::Render`] if bringing the target up to the adopted
    /// frame, or the read itself, fails.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self, now: f64) -> Result<Screenshot, PainterError> {
        // Refused before anything is composed, so a painter with no target
        // says so rather than reporting the empty target a compose would
        // leave.
        #[cfg(feature = "painter-test-utils")]
        if matches!(self.output, Output::None) {
            return Err(PainterError::Render(
                "this painter has no draw target to capture".to_owned(),
            ));
        }
        let size = self.frame_size;
        if let Some(frame) = self.frame.clone() {
            let key: ComposeKey = (frame.commit_id(), self.scroll_intents.generation);
            let animation_now = frame.has_live_curves().then_some(now);
            self.render_frame(&frame, key, animation_now, false)?;
        }
        // Read back only what the record names at the size being read: a
        // record at another size is not this capture's pixels.
        if !self.composed.is_some_and(|(_, rendered)| rendered == size) {
            return Err(PainterError::Render(
                "no frame has been rendered to capture".to_owned(),
            ));
        }
        let pixels = match &mut self.output {
            #[cfg(feature = "painter-test-utils")]
            Output::None => unreachable!("a painter with no target was refused above"),
            Output::Offscreen(gpu) => gpu
                .read_pixels()
                .map_err(|error| PainterError::Gpu(error.to_string()))?,
            Output::Window(graphics) => graphics.capture_frame(size)?,
        };
        Ok(Screenshot { size, pixels })
    }
}

/// The test surface: what an owner's own tests read and pin of a painter.
/// Not a contract — hidden, and only in builds with `painter-test-utils`.
#[cfg(feature = "painter-test-utils")]
impl Painter {
    /// A painter with nowhere to draw, for tests about routing, events and
    /// timers: they pay for no GPU device, and need no executor to build one.
    ///
    /// # Panics
    ///
    /// If the metrics are not a valid target size.
    #[doc(hidden)]
    #[must_use]
    pub fn without_output(
        width: f32,
        height: f32,
        device_pixel_ratio: f32,
        gestures: GesturePolicy,
    ) -> Self {
        let frame_size = FrameSize::for_viewport(width, height, device_pixel_ratio)
            .expect("a test painter is built at a valid size");
        let viewport = Viewport::new(width, height).with_device_pixel_ratio(device_pixel_ratio);
        Self::with_output(viewport, frame_size, Output::None, gestures)
    }

    /// Whether this painter has a draw target at all.
    #[doc(hidden)]
    #[must_use]
    pub const fn has_target(&self) -> bool {
        !matches!(self.output, Output::None)
    }

    /// The frame clock, to pin.
    #[doc(hidden)]
    pub fn clock_mut(&mut self) -> &mut FrameClock {
        &mut self.clock
    }

    /// The gesture router.
    #[doc(hidden)]
    #[must_use]
    pub const fn gesture(&self) -> &GestureRouter {
        &self.gesture
    }

    /// The scroll intents.
    #[doc(hidden)]
    #[must_use]
    pub const fn scroll_intents(&self) -> &ScrollIntents {
        &self.scroll_intents
    }

    /// The scroll intents, to drive by hand.
    #[doc(hidden)]
    pub fn scroll_intents_mut(&mut self) -> &mut ScrollIntents {
        &mut self.scroll_intents
    }

    /// Overrides the target size the next render is made at, without
    /// validating it — so a test can make the GPU refuse one.
    #[doc(hidden)]
    pub fn set_frame_size(&mut self, size: FrameSize) {
        self.frame_size = size;
    }

    /// Whether a frame is asked for, without taking it.
    #[doc(hidden)]
    #[must_use]
    pub fn redraw_pending(&self) -> bool {
        self.redraw_pending.get()
    }
}

/// The test surface of [`ScrollIntents`]: forwarders to the methods the
/// painter itself calls, for an owner's tests that read the intents or drive
/// them by hand. Not a contract — hidden, and only in builds with
/// `painter-test-utils`.
///
/// A trait rather than an inherent block because each forwarder keeps the
/// name of the method it forwards to, which an inherent block cannot hold
/// twice. Outside this module those methods are private, so a call through
/// the trait in scope resolves here.
#[cfg(feature = "painter-test-utils")]
#[doc(hidden)]
pub trait ScrollIntentsTestUtils {
    /// [`ScrollIntents::rebase`].
    fn rebase(&mut self, frame: &CommittedFrame, now: f64);
    /// [`ScrollIntents::tick`].
    fn tick(&mut self, frame: &CommittedFrame, now: f64);
    /// [`ScrollIntents::write`].
    fn write(&mut self, node: NodeId, offset: Vector2D<f32>);
    /// [`ScrollIntents::start_bounce_backs`].
    fn start_bounce_backs(&mut self, frame: &CommittedFrame, now: f64);
    /// [`ScrollIntents::is_animating`].
    #[must_use]
    fn is_animating(&self) -> bool;
    /// [`ScrollIntents::is_wheeling`].
    #[must_use]
    fn is_wheeling(&self, node: NodeId) -> bool;
    /// [`ScrollIntents::is_auto_scrolling`].
    #[must_use]
    fn is_auto_scrolling(&self, frame: &CommittedFrame) -> bool;
    /// [`ScrollIntents::has_auto_scroll`].
    #[must_use]
    fn has_auto_scroll(&self, node: NodeId) -> bool;
    /// [`ScrollIntents::is_held`].
    #[must_use]
    fn is_held(&self, node: NodeId) -> bool;
    /// [`ScrollIntents::is_flinging`].
    #[must_use]
    fn is_flinging(&self, frame: &CommittedFrame, node: NodeId) -> bool;
    /// [`ScrollIntents::is_bouncing`].
    #[must_use]
    fn is_bouncing(&self, node: NodeId) -> bool;
    /// [`ScrollIntents::is_gliding`].
    #[must_use]
    fn is_gliding(&self, node: NodeId) -> bool;
    /// [`ScrollIntents::offset_for`].
    #[must_use]
    fn offset_for(&self, node: NodeId) -> Option<Vector2D<f32>>;
    /// Whether any container's live offset differs from its committed one.
    #[must_use]
    fn has_live_offsets(&self) -> bool;
    /// Where each drag holding `node`'s container found it: the offset
    /// before that drag's first step on it.
    fn drag_origins(&self, node: NodeId) -> impl Iterator<Item = Vector2D<f32>> + '_;
}

#[cfg(feature = "painter-test-utils")]
impl ScrollIntentsTestUtils for ScrollIntents {
    fn rebase(&mut self, frame: &CommittedFrame, now: f64) {
        Self::rebase(self, frame, now);
    }

    fn tick(&mut self, frame: &CommittedFrame, now: f64) {
        Self::tick(self, frame, now);
    }

    fn write(&mut self, node: NodeId, offset: Vector2D<f32>) {
        Self::write(self, node, offset);
    }

    fn start_bounce_backs(&mut self, frame: &CommittedFrame, now: f64) {
        Self::start_bounce_backs(self, frame, now);
    }

    fn is_animating(&self) -> bool {
        Self::is_animating(self)
    }

    fn is_wheeling(&self, node: NodeId) -> bool {
        Self::is_wheeling(self, node)
    }

    fn is_auto_scrolling(&self, frame: &CommittedFrame) -> bool {
        Self::is_auto_scrolling(self, frame)
    }

    fn has_auto_scroll(&self, node: NodeId) -> bool {
        Self::has_auto_scroll(self, node)
    }

    fn is_held(&self, node: NodeId) -> bool {
        Self::is_held(self, node)
    }

    fn is_flinging(&self, frame: &CommittedFrame, node: NodeId) -> bool {
        Self::is_flinging(self, frame, node)
    }

    fn is_bouncing(&self, node: NodeId) -> bool {
        Self::is_bouncing(self, node)
    }

    fn is_gliding(&self, node: NodeId) -> bool {
        Self::is_gliding(self, node)
    }

    fn offset_for(&self, node: NodeId) -> Option<Vector2D<f32>> {
        Self::offset_for(self, node)
    }

    fn has_live_offsets(&self) -> bool {
        !self.offsets.is_empty()
    }

    fn drag_origins(&self, node: NodeId) -> impl Iterator<Item = Vector2D<f32>> + '_ {
        self.gesture_origins
            .iter()
            .filter(move |((_, held), _)| *held == node)
            .map(|(_, origin)| *origin)
    }
}

/// [`inertia`]'s quiet interval that ends one wheel gesture, for tests that
/// wait it out.
#[cfg(feature = "painter-test-utils")]
#[doc(hidden)]
pub const WHEEL_REST_SECONDS: f64 = inertia::WHEEL_REST_SECONDS;

/// [`inertia`]'s least interval between two writes of one auto-scroll, for
/// tests that step past it.
#[cfg(feature = "painter-test-utils")]
#[doc(hidden)]
pub const AUTO_SCROLL_MIN_INTERVAL_SECONDS: f64 = inertia::AUTO_SCROLL_MIN_INTERVAL_SECONDS;

#[cfg(all(test, feature = "painter-test-utils"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod painter_tests {
    use super::{GesturePolicy, Painter};
    use crate::render::image::NoImages;
    use crate::tree::document::Document;
    use crate::tree::document::tests::device;

    const POLICY: GesturePolicy = GesturePolicy {
        tap_slop: 50.0,
        long_press_move_slop: 8.0,
        drag_slop: 8.0,
        long_press_seconds: 0.5,
        tap_event: "press",
        long_press_event: "hold",
    };

    fn painter() -> Painter {
        Painter::without_output(800.0, 600.0, 1.0, POLICY)
    }

    #[test]
    fn un_occluding_asks_for_the_frame_held_back() {
        let mut painter = painter();
        painter.set_occluded(true);
        assert!(painter.is_occluded());
        assert!(!painter.redraw_pending(), "occluding asks for nothing");
        painter.set_occluded(false);
        assert!(!painter.is_occluded());
        assert!(painter.take_redraw(), "un-occluding asks for a frame");
    }

    #[test]
    fn an_occluded_painter_owes_no_frame() {
        let mut painter = painter();
        painter.set_occluded(true);
        painter.refresh();
        assert!(painter.redraw_pending());
        assert!(
            !painter.owes_frame(|| true),
            "neither a pending redraw nor the document's demand"
        );
    }

    #[test]
    fn a_painter_without_output_begins_no_draw() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let frame = document.commit();
        let mut painter = painter();
        assert!(painter.adopt(Some(frame), Some(&NoImages)));
        assert!(painter.frame().is_some(), "a frame is adopted");
        assert!(matches!(painter.begin_draw(), Ok(None)));
    }
}
