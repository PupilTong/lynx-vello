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

pub use self::gesture::{
    EmitEvent, GesturePolicy, GestureRouter, InputDecision, InputDecisions, RouterHost,
};
pub use self::graphics::WindowTarget;
use self::graphics::{AcquiredFrame, FrameAcquisition, WindowGraphics};
use self::inertia::{AutoScroll, Axis, BounceBack, ChainOutcome, DragTrack, Fling, Glide};
use self::motion::{ElasticStep, Motion, resolve_elastic_step, stretch_of, unwind_stretch};
use crate::clock::ClockInstant;
use crate::input::{InputEvent, InputKind, PointerId};
use crate::paint::display_list::{DisplayList, ImageRef, Texture};
use crate::peniko::Color;
use crate::render::gpu::{Headless, SharedGpu};
use crate::render::image::FrameImages;
use crate::scroll::{
    AutoScrollRequest, ChainLink, ScrollAxes, ScrollBehavior, ScrollRequest, drive_chain,
    settle_offset,
};
use crate::style::device::Device;
use crate::tree::document::NodeId;
use crate::visual::container_timing::{ContainerPaint, ContainerTimer};
use crate::visual::{CommittedFrame, HitTarget};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    pub device_pixel_ratio: f32,
}

impl Viewport {
    #[must_use]
    pub const fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            device_pixel_ratio: 1.0,
        }
    }

    #[must_use]
    pub const fn with_device_pixel_ratio(mut self, device_pixel_ratio: f32) -> Self {
        self.device_pixel_ratio = device_pixel_ratio;
        self
    }

    #[must_use]
    pub fn device(self) -> Device {
        Device::new(self.width, self.height, self.device_pixel_ratio)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

impl FrameSize {
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
            reason = "finite positive values were bounded to the device texture limit immediately above"
        )]
        Ok(Self {
            width: physical_width.round().max(1.0) as u32,
            height: physical_height.round().max(1.0) as u32,
        })
    }
}

pub enum DrawTarget {
    Window(WindowTarget),
    Offscreen,
}

impl DrawTarget {
    #[must_use]
    pub fn window(target: impl Into<WindowTarget>) -> Self {
        Self::Window(target.into())
    }
}

impl fmt::Debug for DrawTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Window(_) => "DrawTarget::Window",
            Self::Offscreen => "DrawTarget::Offscreen",
        })
    }
}

const MAX_RENDER_DIMENSION: u32 = SharedGpu::LIMITS.max_texture_dimension_2d;

#[derive(Clone)]
pub struct Screenshot {
    pub size: FrameSize,
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

type ComposeKey = (u64, u64, u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PainterError {
    Viewport(String),
    Gpu(String),
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollEntry {
    pub offset: Vector2D<f32>,
    pub at: f64,
    pub rest: Option<Vector2D<f32>>,
    pub request: Option<NonZeroU64>,
    pub anchor_shift: Vector2D<f32>,
    pub reach: Vector2D<f32>,
}

#[derive(Debug)]
pub struct FrameClock {
    epoch: ClockInstant,
    #[cfg(any(test, feature = "painter-test-utils"))]
    pinned: Option<f64>,
}

impl FrameClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            epoch: ClockInstant::now(),
            #[cfg(any(test, feature = "painter-test-utils"))]
            pinned: None,
        }
    }

    pub fn rebase(&mut self, epoch: ClockInstant) {
        self.epoch = epoch;
    }

    #[must_use]
    pub fn now_seconds(&self) -> f64 {
        #[cfg(any(test, feature = "painter-test-utils"))]
        if let Some(seconds) = self.pinned {
            return seconds;
        }
        self.epoch.elapsed().as_secs_f64()
    }

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

#[cfg(test)]
mod frame_size_tests {
    use super::{FrameSize, MAX_RENDER_DIMENSION};

    #[test]
    fn the_render_target_is_bounded_by_the_device_texture_limit() {
        assert_eq!(MAX_RENDER_DIMENSION, 8192);
        let edge = FrameSize::for_viewport(4096.0, 100.0, 2.0).expect("at the limit");
        assert_eq!((edge.width, edge.height), (8192, 200));
        let tall = FrameSize::for_viewport(100.0, 8192.0, 1.0).expect("at the limit");
        assert_eq!(tall.height, 8192);
        for (width, height, ratio) in [(8193.0, 100.0, 1.0), (100.0, 4097.0, 2.0)] {
            let error = FrameSize::for_viewport(width, height, ratio).unwrap_err();
            assert!(error.to_string().contains("8192"), "{error}");
        }
    }
}

const CANVAS: Color = Color::WHITE;

enum Output {
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
    async fn build(target: DrawTarget, frame_size: FrameSize) -> Result<Self, PainterError> {
        match target {
            DrawTarget::Window(target) => Ok(Self::Window(Box::new(
                WindowGraphics::new(target, frame_size).await?,
            ))),
            DrawTarget::Offscreen => Self::offscreen(),
        }
    }

    fn forget(&mut self) {
        match self {
            #[cfg(feature = "painter-test-utils")]
            Self::None => {}
            Self::Offscreen(gpu) => gpu.forget(),
            Self::Window(graphics) => graphics.forget_filters(),
        }
    }

    fn forget_filters(&mut self) {
        match self {
            #[cfg(feature = "painter-test-utils")]
            Self::None => {}
            Self::Offscreen(gpu) => gpu.forget_filters(),
            Self::Window(graphics) => graphics.forget_filters(),
        }
    }

    #[cfg_attr(
        not(feature = "painter-test-utils"),
        expect(
            clippy::unnecessary_wraps,
            reason = "only the test-only target has no stack"
        )
    )]
    fn gpu(&self) -> Option<&Arc<SharedGpu>> {
        match self {
            #[cfg(feature = "painter-test-utils")]
            Self::None => None,
            Self::Offscreen(gpu) => Some(gpu.gpu()),
            Self::Window(graphics) => Some(graphics.gpu()),
        }
    }

    fn lost_pixels(&self) -> u64 {
        match self {
            #[cfg(feature = "painter-test-utils")]
            Self::None => 0,
            Self::Offscreen(gpu) => gpu.lost_pixels(),
            Self::Window(graphics) => graphics.lost_pixels(),
        }
    }

    fn prepare_vectors(
        &mut self,
        frame: &CommittedFrame,
        out: &mut Vec<Option<Texture>>,
    ) -> Result<(), PainterError> {
        out.clear();
        let baked = match self {
            #[cfg(feature = "painter-test-utils")]
            Self::None => return Ok(()),
            Self::Offscreen(gpu) => gpu
                .prepare_vectors(frame)
                .map_err(|error| PainterError::Gpu(error.to_string()))?,
            Self::Window(graphics) => graphics.prepare_vectors(frame)?,
        };
        out.extend(baked.iter().copied());
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one bake pre-step's full inputs: the frame, its two pixel tables, the three \
                  compose readings, and the table it fills"
    )]
    fn prepare_filters(
        &mut self,
        frame: &CommittedFrame,
        images: &[Option<ImageRef>],
        vectors: &[Option<Texture>],
        offset_of: &dyn Fn(&crate::visual::ScrollSlot) -> Option<Vector2D<f32>>,
        scroll_generation: u64,
        animation_now: Option<f64>,
        out: &mut Vec<Option<Texture>>,
    ) -> Result<(), PainterError> {
        out.clear();
        let baked = match self {
            #[cfg(feature = "painter-test-utils")]
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
        out.extend(baked.iter().copied());
        Ok(())
    }

    fn render(&mut self, scene: &DisplayList, size: FrameSize) -> Result<(), PainterError> {
        match self {
            #[cfg(feature = "painter-test-utils")]
            Self::None => Ok(()),
            Self::Offscreen(gpu) => gpu
                .render_frame(scene, size.width, size.height, CANVAS)
                .map_err(|error| PainterError::Gpu(error.to_string())),
            Self::Window(graphics) => graphics.render_to_target(scene, size),
        }
    }

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

#[derive(Clone, Copy, Debug)]
pub struct PresentedFrame {
    pub commit_id: u64,
    pub compose_start: ClockInstant,
    pub compose_end: ClockInstant,
    pub presented: ClockInstant,
}

#[derive(Clone, Copy)]
struct ComposeSpan {
    start: ClockInstant,
    end: ClockInstant,
}

#[derive(Clone, Copy)]
struct Rendered {
    span: Option<ComposeSpan>,
}

#[derive(Clone, Copy, Debug)]
struct PaintTiming {
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
    fn forget(&mut self) {
        *self = Self {
            enabled: self.enabled,
            ..Self::default()
        };
    }

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

pub struct Painter {
    viewport: Viewport,
    frame_size: FrameSize,
    output: Output,
    occluded: bool,
    render_failed: bool,
    frame: Option<Arc<CommittedFrame>>,
    redraw_pending: Cell<bool>,
    gesture: GestureRouter,
    clock: FrameClock,
    scroll_intents: ScrollIntents,
    composed: Option<(ComposeKey, FrameSize)>,
    composed_scene: DisplayList,
    composed_filters: Vec<Option<Texture>>,
    composed_vectors: Vec<Option<Texture>>,
    images: images::PainterImages,
    lost_pixels: u64,
    paint_timing: PaintTiming,
    container_timer: ContainerTimer,
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

#[must_use = "a window frame is presented by `Painter::finish_draw`"]
pub struct WindowFrame {
    acquired: AcquiredFrame,
    frame: Arc<CommittedFrame>,
    now: f64,
}

impl WindowFrame {
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
    offsets: FxHashMap<NodeId, Vector2D<f32>>,
    rebased_commit: Option<u64>,
    snapped: FxHashMap<NodeId, Snapped>,
    generation: u64,
    gesture_origins: FxHashMap<(PointerId, NodeId), Vector2D<f32>>,
    flings: SmallVec<[Fling; 1]>,
    bounce_backs: SmallVec<[BounceBack; 2]>,
    glides: SmallVec<[Glide; 2]>,
    auto_scrolls: Vec<AutoScroll>,
    handled: FxHashMap<NodeId, NonZeroU64>,
    anchor_shifts: FxHashMap<NodeId, Vector2D<f32>>,
    drags: FxHashMap<PointerId, DragTrack>,
    wheel_steps: SmallVec<[(NodeId, f64); 1]>,
    changed: SmallVec<[NodeId; 2]>,
    posting: SmallVec<[(NodeId, ScrollEntry); 2]>,
}

#[derive(Debug, Clone, Copy)]
struct Snapped {
    x: Option<NodeId>,
    y: Option<NodeId>,
}

impl ScrollIntents {
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
            anchor_shifts: FxHashMap::default(),
            drags: FxHashMap::default(),
            wheel_steps: SmallVec::new(),
            changed: SmallVec::new(),
            posting: SmallVec::new(),
        }
    }

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

    fn rebase(&mut self, frame: &CommittedFrame, now: f64) {
        if self.rebased_commit == Some(frame.commit_id()) {
            return;
        }
        self.rebased_commit = Some(frame.commit_id());
        let mut shifts: SmallVec<[(NodeId, Vector2D<f32>); 2]> = SmallVec::new();
        for slot in frame.scroll_slots() {
            let applied = self
                .anchor_shifts
                .entry(slot.node)
                .or_insert(slot.anchor_shift);
            let shift = slot.anchor_shift - *applied;
            *applied = slot.anchor_shift;
            if shift != Vector2D::zero()
                && let Some(offset) = self.offsets.get_mut(&slot.node)
            {
                *offset += shift;
                shifts.push((slot.node, shift));
            }
        }
        self.anchor_shifts
            .retain(|node, _| frame.slot_of(*node).is_some());
        let changed = &mut self.changed;
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

    fn apply_requests(&mut self, frame: &CommittedFrame, now: f64) {
        if !self.handled.is_empty() {
            self.handled
                .retain(|node, _| frame.slot_of(*node).is_some());
        }
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

    fn apply_request(
        &mut self,
        frame: &CommittedFrame,
        slot: &crate::visual::ScrollSlot,
        request: ScrollRequest,
        now: f64,
    ) {
        let node = slot.node;
        if request.yields_to_pointer && self.is_held(node) {
            note_changed(&mut self.changed, node);
            return;
        }
        self.stop_flings_and_bounces_on(frame, node, now);
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
                let target = nearest_copy(slot, live, target);
                self.glide_to_or_keep(frame, node, live, target, now);
            }
        }
        note_changed(&mut self.changed, node);
        self.generation += 1;
    }

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
            note_changed(&mut self.changed, node);
            let slot = &frame.scroll_slots()[index as usize];
            let (snap_x, snap_y) = frame.snap_axes(slot);
            let offset = self.offsets.get(&node).copied().unwrap_or(slot.offset);
            let settled = settle_stretch_aware(origin, offset, slot, snap_x, snap_y);
            self.glide_to(frame, node, offset, settled, now);
        }
    }

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
            self.retain_motion(frame);
        }
        outcome
    }

    #[must_use]
    fn offset_for(&self, node: NodeId) -> Option<Vector2D<f32>> {
        self.offsets.get(&node).copied()
    }

    fn write(&mut self, node: NodeId, offset: Vector2D<f32>) {
        self.offsets.insert(node, offset);
        note_changed(&mut self.changed, node);
    }

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
                        anchor_shift: self
                            .anchor_shifts
                            .get(&node)
                            .copied()
                            .unwrap_or(slot.anchor_shift),
                        reach: self.reach_of(frame, slot, offset),
                    },
                ));
            }
        }
        self.changed.clear();
        self.posting = posting;
        self.posting.drain(..)
    }
}

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

fn wrap_axes(slot: &crate::visual::ScrollSlot) -> ScrollAxes {
    let (x, y) = slot.wrap_period();
    ScrollAxes {
        x: x.is_some(),
        y: y.is_some(),
    }
}

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

#[expect(
    clippy::too_many_arguments,
    reason = "one frame's whole path to pixels: the target, the three compose buffers, the \
              frame, and the pixel table it draws from"
)]
fn compose_and_render(
    output: &mut Output,
    buffer: &mut DisplayList,
    filtered: &mut Vec<Option<Texture>>,
    vectors: &mut Vec<Option<Texture>>,
    intents: &ScrollIntents,
    frame: &CommittedFrame,
    images: &[Option<ImageRef>],
    size: FrameSize,
    animation_now: Option<f64>,
) -> Result<(), PainterError> {
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
        scene
    } else {
        buffer.reset();
        frame.compose_target_into(
            buffer,
            (size.width, size.height),
            images,
            filtered,
            vectors,
            &|slot| intents.offset_for(slot.node),
            animation_now,
        );
        &*buffer
    };
    output.render(scene, size)
}

fn route_published(
    frame: Option<&CommittedFrame>,
    intents: &ScrollIntents,
    event: &InputEvent,
    animation_now: Option<f64>,
) -> Option<HitTarget> {
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
            composed_scene: DisplayList::new(),
            composed_filters: Vec::new(),
            composed_vectors: Vec::new(),
            images: images::PainterImages::default(),
            lost_pixels: 0,
            paint_timing: PaintTiming::default(),
            container_timer: ContainerTimer::default(),
            thread_bound: PhantomData,
        }
    }

    #[must_use]
    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    #[must_use]
    pub const fn frame_size(&self) -> FrameSize {
        self.frame_size
    }

    #[must_use]
    pub const fn presents_to_window(&self) -> bool {
        matches!(self.output, Output::Window(_))
    }

    #[must_use]
    pub const fn is_offscreen(&self) -> bool {
        matches!(self.output, Output::Offscreen(_))
    }

    #[must_use]
    pub const fn has_failed(&self) -> bool {
        self.render_failed
    }

    #[must_use]
    pub const fn is_occluded(&self) -> bool {
        self.occluded
    }

    pub fn rebase_clock(&mut self, epoch: ClockInstant) {
        self.clock.rebase(epoch);
    }

    #[must_use]
    pub fn now_seconds(&self) -> f64 {
        self.clock.now_seconds()
    }

    #[must_use]
    pub fn frame(&self) -> Option<&Arc<CommittedFrame>> {
        self.frame.as_ref()
    }

    pub fn adopt<S>(&mut self, frame: Option<Arc<CommittedFrame>>, store: Option<&S>) -> bool
    where
        S: FrameImages + ?Sized,
    {
        self.images.bind(self.output.gpu());
        let commit = frame.as_ref().map(|frame| frame.commit_id());
        let mut described = false;
        if commit != self.frame.as_ref().map(|frame| frame.commit_id()) {
            if let Some(incoming) = frame.as_deref()
                && !self.images.holds(incoming.commit_id())
            {
                let Some(store) = store else {
                    self.images.orphan();
                    self.images.release_held(None::<&S>);
                    return false;
                };
                self.images.resolve(incoming, store);
                described = true;
            }
            if let Some(outgoing) = self.frame.as_deref() {
                self.scroll_intents.leave(outgoing);
            }
            self.redraw_pending.set(true);
        } else if self.images.is_stale()
            && let Some(store) = store
            && let Some(current) = frame.as_deref()
        {
            self.images.resolve(current, store);
            described = true;
            self.output.forget_filters();
            self.redraw_pending.set(true);
        }
        self.frame = frame;
        match store {
            Some(store) if !described && self.images.refresh(store) => {
                self.redraw_pending.set(true);
            }
            Some(_) => {}
            None => self.images.orphan(),
        }
        self.images.release_held(store);
        true
    }

    #[must_use]
    pub fn awaits_pictures(&self) -> bool {
        self.images.awaits()
    }

    pub fn forget_view(&mut self) {
        self.frame = None;
        self.scroll_intents = ScrollIntents::new();
        self.gesture = GestureRouter::new(self.gesture.policy());
        self.images.forget();
        self.paint_timing.forget();
        self.container_timer.forget();
    }

    pub fn set_presentation_timing(&mut self, enabled: bool) {
        self.paint_timing.enabled = enabled;
    }

    #[must_use]
    pub const fn presentation_timing(&self) -> bool {
        self.paint_timing.enabled
    }

    #[must_use]
    pub const fn last_presented(&self) -> Option<PresentedFrame> {
        self.paint_timing.last_presented
    }

    #[must_use]
    pub const fn first_paint(&self) -> Option<PresentedFrame> {
        self.paint_timing.first_paint
    }

    #[must_use]
    pub const fn first_contentful_paint(&self) -> Option<PresentedFrame> {
        self.paint_timing.first_contentful_paint
    }

    pub fn forget_target(&mut self) {
        self.composed = None;
        self.composed_filters.clear();
        self.composed_vectors.clear();
        self.output.forget();
    }

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

    #[must_use]
    pub fn owes_frame(&self, frame_demand: impl FnOnce() -> bool) -> bool {
        if self.render_failed || self.occluded || !self.presents_to_window() {
            return false;
        }
        self.is_animating(frame_demand) || self.redraw_pending.get()
    }

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

    pub fn tick_gestures(&mut self, now: f64, host: &impl RouterHost, emits: &mut Vec<EmitEvent>) {
        let adopted = self.frame.clone();
        let mut decisions = InputDecisions::new();
        self.gesture.on_tick(now, host, &mut decisions);
        self.apply_decisions(&mut decisions, adopted.as_deref(), now, emits);
    }

    pub fn tick_scroll(&mut self, now: f64) {
        if let Some(frame) = &self.frame {
            self.scroll_intents.tick(frame, now);
        }
    }

    pub fn rebase_scroll(&mut self, now: f64) {
        if let Some(frame) = &self.frame {
            self.scroll_intents.rebase(frame, now);
        }
    }

    #[must_use]
    pub fn scroll_changed(&self) -> bool {
        !self.scroll_intents.changed.is_empty()
    }

    pub fn discard_changed(&mut self) {
        self.scroll_intents.changed.clear();
    }

    pub fn drain_changed(&mut self, at: f64) -> impl Iterator<Item = (NodeId, ScrollEntry)> + '_ {
        self.scroll_intents.drain_changed(self.frame.as_deref(), at)
    }

    pub fn refresh(&self) {
        self.redraw_pending.set(true);
    }

    pub fn take_redraw(&self) -> bool {
        self.redraw_pending.replace(false)
    }

    pub fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        if !occluded {
            self.refresh();
        }
    }

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
        let lost = self.output.lost_pixels();
        if lost != self.lost_pixels {
            self.lost_pixels = lost;
            if !self.images.is_stale() {
                self.images.mark_stale();
                self.redraw_pending.set(true);
            }
        }
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

    pub fn finish_draw(&mut self, window_frame: WindowFrame) -> Result<(), PainterError> {
        let WindowFrame {
            acquired,
            frame,
            now,
        } = window_frame;
        let key: ComposeKey = (
            frame.commit_id(),
            self.scroll_intents.generation,
            self.images.revision(),
        );
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
        if let Some(rendered) = rendered {
            self.note_presented(&frame, rendered);
        }
        Ok(())
    }

    pub fn draw_offscreen(&mut self, now: f64, force: bool) -> Result<bool, PainterError> {
        if !self.is_offscreen() {
            return Ok(false);
        }
        let Some(frame) = self.frame.clone() else {
            return Ok(false);
        };
        let key: ComposeKey = (
            frame.commit_id(),
            self.scroll_intents.generation,
            self.images.revision(),
        );
        let animation_now = frame.has_live_curves().then_some(now);
        let Some(rendered) = self.render_frame(&frame, key, animation_now, force)? else {
            return Ok(false);
        };
        let Output::Offscreen(gpu) = &self.output else {
            unreachable!("the offscreen output was just checked");
        };
        gpu.wait_idle()
            .map_err(|error| PainterError::Gpu(error.to_string()))?;
        self.note_presented(&frame, rendered);
        Ok(true)
    }

    fn note_presented(&mut self, frame: &CommittedFrame, rendered: Rendered) {
        let containers = frame.paints_under_containers();
        if rendered.span.is_none() && !containers {
            return;
        }
        let presented = ClockInstant::now();
        if let Some(span) = rendered.span {
            self.paint_timing.note(frame, span, presented);
        }
        if containers {
            self.container_timer.note(frame, presented);
        }
    }

    pub fn take_container_paints(&mut self) -> Vec<ContainerPaint> {
        self.container_timer.take()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture(&mut self, now: f64) -> Result<Screenshot, PainterError> {
        #[cfg(feature = "painter-test-utils")]
        if matches!(self.output, Output::None) {
            return Err(PainterError::Render(
                "this painter has no draw target to capture".to_owned(),
            ));
        }
        let size = self.frame_size;
        let awaited = || {
            PainterError::Render(
                "a picture this frame draws has not arrived yet; take the host's turns until \
                 `awaits_pictures` is false"
                    .to_owned(),
            )
        };
        if self.images.awaits() {
            return Err(awaited());
        }
        if let Some(frame) = self.frame.clone() {
            let key: ComposeKey = (
                frame.commit_id(),
                self.scroll_intents.generation,
                self.images.revision(),
            );
            let animation_now = frame.has_live_curves().then_some(now);
            self.render_frame(&frame, key, animation_now, false)?;
        }
        if self.images.awaits() {
            return Err(awaited());
        }
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

#[cfg(feature = "painter-test-utils")]
impl Painter {
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

    #[doc(hidden)]
    #[must_use]
    pub const fn has_target(&self) -> bool {
        !matches!(self.output, Output::None)
    }

    #[doc(hidden)]
    pub fn clock_mut(&mut self) -> &mut FrameClock {
        &mut self.clock
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn gesture(&self) -> &GestureRouter {
        &self.gesture
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn scroll_intents(&self) -> &ScrollIntents {
        &self.scroll_intents
    }

    #[doc(hidden)]
    pub fn scroll_intents_mut(&mut self) -> &mut ScrollIntents {
        &mut self.scroll_intents
    }

    #[doc(hidden)]
    pub fn set_frame_size(&mut self, size: FrameSize) {
        self.frame_size = size;
    }

    #[doc(hidden)]
    #[must_use]
    pub fn redraw_pending(&self) -> bool {
        self.redraw_pending.get()
    }
}

#[cfg(feature = "painter-test-utils")]
#[doc(hidden)]
pub trait ScrollIntentsTestUtils {
    fn rebase(&mut self, frame: &CommittedFrame, now: f64);
    fn tick(&mut self, frame: &CommittedFrame, now: f64);
    fn write(&mut self, node: NodeId, offset: Vector2D<f32>);
    fn start_bounce_backs(&mut self, frame: &CommittedFrame, now: f64);
    #[must_use]
    fn is_animating(&self) -> bool;
    #[must_use]
    fn is_wheeling(&self, node: NodeId) -> bool;
    #[must_use]
    fn is_auto_scrolling(&self, frame: &CommittedFrame) -> bool;
    #[must_use]
    fn has_auto_scroll(&self, node: NodeId) -> bool;
    #[must_use]
    fn is_held(&self, node: NodeId) -> bool;
    #[must_use]
    fn is_flinging(&self, frame: &CommittedFrame, node: NodeId) -> bool;
    #[must_use]
    fn is_bouncing(&self, node: NodeId) -> bool;
    #[must_use]
    fn is_gliding(&self, node: NodeId) -> bool;
    #[must_use]
    fn offset_for(&self, node: NodeId) -> Option<Vector2D<f32>>;
    #[must_use]
    fn has_live_offsets(&self) -> bool;
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

#[cfg(feature = "painter-test-utils")]
#[doc(hidden)]
pub const WHEEL_REST_SECONDS: f64 = inertia::WHEEL_REST_SECONDS;

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
