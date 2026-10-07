//! The committed frame: one commit's output, owned outright.
//!
//! A [`CommittedFrame`] carries everything the thread that does not hold the
//! document needs from one commit — the paint-order tables, the encoded
//! scene, and the scroll-slot table — with no borrow of the document that
//! built it. `NodeId`s inside it stay safe however stale the frame gets: an
//! id is retired on free and never reissued, so it resolves to the node it
//! was built for or to nothing, never to a stranger. Liveness is therefore
//! not this type's concern; a consumer that must act on a node re-validates
//! against the document at the point of action.
//!
//! The scroll-slot table exists so input recognition can run without the
//! document: the nearest-scrollable walk, scroll chaining, and clamping all
//! read published geometry here instead of live styles. Offsets and bounds
//! are as of the commit; between commits they are a snapshot, which is the
//! same screen-semantics staleness hit testing already accepts.

use euclid::default::{Point2D, Size2D, Vector2D};
use smallvec::SmallVec;
use stylo::properties::animated_properties::AnimationValueMap;

use super::{AnimationSample, PaintOrder, SpaceSamples};
use crate::NodeId;
use crate::paint::compose::{self, ComposeOp, FilterGroup};
use crate::scroll::{
    CaptureAxes, ChainLink, ScrollAxes, ScrollRequest, SnapAxis, SnapPoint, SnapStrictness,
};
use crate::vello::Scene;
use crate::vello::kurbo::Affine;
use crate::vello::peniko::ImageData;

/// One scroll container in the committed frame, linked to the nearest scroll
/// container on its containing-block chain.
///
/// The chain is containing-block-based, not DOM-based, because that is what
/// scrolling follows: a box is only carried by, and only chains into,
/// scroll containers it is laid out inside of (CSS2 §11.1.1). The builder
/// assigns entries with the same escape rules the paint order itself uses,
/// so an absolutely-positioned box whose containing block is outside its
/// DOM-side scroller correctly links past it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollSlot {
    /// The scroll container element.
    pub node: NodeId,
    /// The nearest enclosing scroll container on the containing-block chain.
    pub parent: Option<u32>,
    /// The axes the user may scroll directly (`overflow: scroll`); a
    /// `hidden` container is in the table — it scrolls programmatically and
    /// carries chain structure — with both flags off.
    pub user_scrollable: ScrollAxes,
    /// The axes a boundary chains past: `overscroll-behavior: auto`.
    pub chains: ScrollAxes,
    /// The axes whose boundary stretches and springs back:
    /// `overscroll-behavior: contain-bounce`. Policy only: the painter's
    /// intents may stand outside `0..=max_offset` on such an axis, while the
    /// committed `offset` never does.
    pub bounce: ScrollAxes,
    /// The axes with no boundary: `overscroll-behavior: circular`. Policy
    /// only: the painter's intents may stand anywhere on such an axis and
    /// compose modulo [`Self::wrap_period`], while the committed `offset`
    /// never leaves `0..=max_offset`.
    pub circular: ScrollAxes,
    /// Whether the container above goes first, per axis:
    /// `scroll-capture-x` / `scroll-capture-y`.
    pub capture: CaptureAxes,
    /// The axes this container snaps on, each naming its points in the
    /// frame's [`snap_points`](CommittedFrame::snap_points).
    pub snap: SnapSlot,
    /// The committed, already-clamped offset.
    pub offset: Vector2D<f32>,
    /// The largest offset the committed geometry admits, per axis.
    pub max_offset: Vector2D<f32>,
    /// The scrollport (padding box) size, which is also what the encode
    /// window is sized from.
    pub scrollport: Size2D<f32>,
    /// Local horizontal and vertical unit vectors in viewport CSS pixels.
    /// Scroll offsets stay local; composition maps their translations through
    /// these axes so transformed scroll containers move their content correctly.
    pub viewport_axes: [Vector2D<f32>; 2],
    /// The container's pending programmatic scroll, target clamped to
    /// `max_offset`: carried by every frame until the painter acknowledges
    /// it, and handled by the painter once per serial
    /// ([`Document::scroll_to_with`](crate::Document::scroll_to_with)).
    pub request: Option<ScrollRequest>,
}

/// Where compose puts a frame's scroll slots at one instant: `offset_of`'s
/// override for a slot, else its committed offset.
#[derive(Clone, Copy)]
pub(crate) struct ScrollOffsets<'a> {
    pub(crate) slots: &'a [ScrollSlot],
    pub(crate) offset_of: &'a dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
}

impl ScrollOffsets<'_> {
    /// Slot `slot`'s offset.
    pub(crate) fn of(&self, slot: u32) -> Vector2D<f32> {
        let slot = &self.slots[slot as usize];
        (self.offset_of)(slot).unwrap_or(slot.offset)
    }
}

/// `offset_of` with every offset normalized into its slot's period
/// ([`ScrollSlot::wrap`]): what the frame's entry points hand everything
/// they sample, so scroll timelines, stickies, anchored boxes and the space
/// maps all read an offset inside the scrolling area's one period.
fn wrapped_offsets(
    offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) -> impl Fn(&ScrollSlot) -> Option<Vector2D<f32>> + '_ {
    move |slot: &ScrollSlot| offset_of(slot).map(|offset| slot.wrap(offset))
}

/// One axis of a slot's snapping: its strictness and the `start..end`
/// range of its points in the frame's snap-point table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapSlotAxis {
    pub strictness: SnapStrictness,
    pub start: u32,
    pub end: u32,
}

/// A slot's snapping, per axis; `None` on an axis it does not snap on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SnapSlot {
    pub x: Option<SnapSlotAxis>,
    pub y: Option<SnapSlotAxis>,
}

impl SnapSlotAxis {
    /// This axis over the frame's points, repeating every `period` on a
    /// circular axis.
    fn axis<'frame>(&self, points: &'frame [SnapPoint], period: Option<f32>) -> SnapAxis<'frame> {
        SnapAxis {
            strictness: self.strictness,
            points: &points[self.start as usize..self.end as usize],
            period,
        }
    }
}

impl ScrollSlot {
    /// This container's part in a chain walk, for
    /// [`drive_chain`](crate::scroll::drive_chain).
    #[must_use]
    pub fn link(&self) -> ChainLink {
        ChainLink {
            user_scrollable: self.user_scrollable,
            chains: self.chains,
            capture: self.capture,
        }
    }

    pub(crate) fn viewport_translation(&self, offset: Vector2D<f32>) -> Vector2D<f32> {
        self.viewport_axes[0] * offset.x + self.viewport_axes[1] * offset.y
    }

    /// The period each axis wraps with: the extent of the whole scrolling
    /// area (`max_offset` plus the scrollport) on a [`Self::circular`] axis
    /// whose content overflows, `None` otherwise. An axis with nothing to
    /// scroll has nothing to wrap either, so it keeps the ordinary clamp.
    #[must_use]
    pub fn wrap_period(&self) -> (Option<f32>, Option<f32>) {
        let axis =
            |circular: bool, max: f32, extent: f32| (circular && max > 0.0).then_some(max + extent);
        (
            axis(self.circular.x, self.max_offset.x, self.scrollport.width),
            axis(self.circular.y, self.max_offset.y, self.scrollport.height),
        )
    }

    /// `offset` normalized into `[0, period)` on each axis that has a
    /// [`Self::wrap_period`], and unchanged on the others — this clamps
    /// nothing. A non-finite offset on a wrapped axis normalizes to `0`.
    ///
    /// The painter's offset on a circular axis stands anywhere on an
    /// unbounded line; this is where it is consumed. A normalized offset
    /// above `max_offset` is one whose scrollport straddles the seam: it
    /// shows the end of the scrolling area followed by its start.
    #[must_use]
    pub fn wrap(&self, offset: Vector2D<f32>) -> Vector2D<f32> {
        let axis = |value: f32, period: Option<f32>| match period {
            Some(period) => {
                let wrapped = value.rem_euclid(period);
                // `rem_euclid` of a tiny negative value rounds up to the
                // period itself, which is the start again.
                if wrapped.is_finite() && wrapped < period {
                    wrapped
                } else {
                    0.0
                }
            }
            None => value,
        };
        let (x, y) = self.wrap_period();
        Vector2D::new(axis(offset.x, x), axis(offset.y, y))
    }

    /// The offset range the committed encode covers on each axis — the
    /// window compose may move through without a recommit. Sized in
    /// scrollports around the committed offset, clamped to what the geometry
    /// admits. A scroll outside it recommits at once (`Document::scroll_to`);
    /// one that has used half the headroom toward an edge recommits early
    /// ([`Self::recenter_due`]), so the painter is never left at the edge.
    ///
    /// On an axis with a [`Self::wrap_period`] the window is the whole
    /// scrolling area, `0..=max_offset`, whatever the committed offset: the
    /// painter's offset moves around the circle without telling the document
    /// until it settles, and across the seam it composes both ends at once,
    /// so no window narrower than the whole area stays valid. That also
    /// makes every `content-visibility: auto` box on that axis relevant.
    #[must_use]
    pub fn encode_window(&self) -> (Vector2D<f32>, Vector2D<f32>) {
        let slack = Vector2D::new(
            if self.max_offset.x > 0.0 {
                self.scrollport.width * ENCODE_WINDOW_SCROLLPORTS
            } else {
                0.0
            },
            if self.max_offset.y > 0.0 {
                self.scrollport.height * ENCODE_WINDOW_SCROLLPORTS
            } else {
                0.0
            },
        );
        let (wrap_x, wrap_y) = self.wrap_period();
        let low = Vector2D::new(
            if wrap_x.is_some() {
                0.0
            } else {
                (self.offset.x - slack.x).max(0.0)
            },
            if wrap_y.is_some() {
                0.0
            } else {
                (self.offset.y - slack.y).max(0.0)
            },
        );
        let high = Vector2D::new(
            if wrap_x.is_some() {
                self.max_offset.x
            } else {
                (self.offset.x + slack.x).min(self.max_offset.x)
            },
            if wrap_y.is_some() {
                self.max_offset.y
            } else {
                (self.offset.y + slack.y).min(self.max_offset.y)
            },
        );
        (low, high)
    }

    /// Whether a scroll to `offset` has used more than half the headroom
    /// [`Self::encode_window`] leaves toward the edge it moves to, on either
    /// axis, so the commit should re-center the window on it. `offset` is
    /// clamped to the committed range first: a `contain-bounce` stretch is
    /// composed from the edge's own content and asks for nothing. An axis
    /// with a [`Self::wrap_period`] never asks: its window is already the
    /// whole scrolling area.
    #[must_use]
    pub fn recenter_due(&self, offset: Vector2D<f32>) -> bool {
        let clamp = |value: f32, max: f32| {
            if value.is_finite() {
                value.clamp(0.0, max)
            } else {
                0.0
            }
        };
        let axis = |pending: f32, committed: f32, low: f32, high: f32| {
            if pending < committed {
                pending - low < (committed - low) / 2.0
            } else if pending > committed {
                high - pending < (high - committed) / 2.0
            } else {
                false
            }
        };
        let (low, high) = self.encode_window();
        let (wrap_x, wrap_y) = self.wrap_period();
        (wrap_x.is_none()
            && axis(
                clamp(offset.x, self.max_offset.x),
                self.offset.x,
                low.x,
                high.x,
            ))
            || (wrap_y.is_none()
                && axis(
                    clamp(offset.y, self.max_offset.y),
                    self.offset.y,
                    low.y,
                    high.y,
                ))
    }
}

/// How far past the committed offset, in scrollports per scrollable axis,
/// the encode covers — the compose headroom before a recentering commit is
/// due.
pub const ENCODE_WINDOW_SCROLLPORTS: f32 = 1.0;

/// The largest area, in viewports, of an element's `max(size, content_size)`
/// that still exports a transform curve. Culling bounds a moving subtree by
/// the viewport pulled back through its curve's reach, so this cap is what
/// bounds the encode only where no reach does — a scale range reaching 0, or
/// a list holding an op the reach does not model (matrix, skew, 3D rotation)
/// or a mismatched remainder stylo decomposes, either of which admits the
/// whole subtree. Firefox caps composited transform animations the same way
/// (`nsDisplayList.cpp`: 1.125 viewports, 4096² px).
pub(crate) const MAX_MOVING_EXTENT_VIEWPORTS: f32 = 3.0;

/// One composite-animated element in the committed frame: the target of the
/// compose-time retargeting that lets its animation play without commits.
///
/// A slot exists only where a curve was exported — every structural and
/// value-level refusal happens before it is allocated — so sampling one is
/// always sampling a live curve. The curve is a clone of the element's stylo
/// animation state, `Send + Sync` by construction.
#[derive(Debug)]
pub struct AnimationSlot {
    /// The animated element.
    pub node: NodeId,
    /// The exported curve.
    pub(crate) curve: crate::visual::curves::CompositeCurve,
}

impl AnimationSlot {
    /// This slot's compose values at `now` — the committed instant when
    /// `None` — with each scroll slot at `offsets`, and `values` as scratch.
    pub(crate) fn sample(
        &self,
        now: Option<f64>,
        offsets: &ScrollOffsets<'_>,
        values: &mut AnimationValueMap,
    ) -> AnimationSample {
        let sample = self.curve.sample(now, offsets, values);
        AnimationSample {
            delta: sample.delta,
            alpha: sample.alpha,
        }
    }
}

/// What a frame hit test reports: the element to act on, plus the scroll
/// slot recognition starts its chain walk from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitTarget {
    /// The topmost hit-testable element at the point. May be freed by the
    /// time a consumer acts on it; the id then resolves to nothing.
    pub node: NodeId,
    /// The nearest ancestor-or-self scroll container of the hit item, as an
    /// index into [`CommittedFrame::scroll_slots`].
    pub scroll: Option<u32>,
}

/// One commit's published output. Immutable, self-contained, and shared by
/// `Arc`: the committer retains one for its own queries, the compositor holds
/// one to draw and route input from.
///
/// The scene is carried *split*: per-space fragments plus the compose
/// program over them, so scroll offsets apply at composition.
pub struct CommittedFrame {
    pub(crate) order: PaintOrder,
    pub(crate) presentation: Presentation,
    pub(crate) animations_active: bool,
    pub(crate) needs_main_ticks: bool,
    /// The earliest instant an exported curve leaves its domain at, over
    /// every slot; `None` when nothing exported ever ends.
    pub(crate) earliest_expiry: Option<f64>,
    pub(crate) viewport: Size2D<f32>,
    pub(crate) device_pixel_ratio: f32,
}

/// The split scene: fragments plus the program that assembles them.
pub(crate) struct Presentation {
    pub(crate) fragments: Vec<Scene>,
    pub(crate) program: Vec<ComposeOp>,
    /// One entry per [`ComposeOp::Image`], in program order. Carries names
    /// and geometry; never pixels.
    pub(crate) image_draws: Vec<crate::paint::compose::ImageDraw>,
    /// One entry per [`ComposeOp::PushFilter`] and [`ComposeOp::PushBackdrop`],
    /// in program order. Carries device geometry and σ; never a GPU resource.
    pub(crate) filter_groups: Vec<FilterGroup>,
    /// The slots `program` reads, derived from it at commit.
    pub(crate) composed: crate::visual::ComposedSlots,
}

impl std::fmt::Debug for CommittedFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommittedFrame")
            .field("commit_id", &self.commit_id())
            .field("viewport", &self.viewport)
            .finish_non_exhaustive()
    }
}

impl CommittedFrame {
    /// The frame's one fragment when that is the whole program — the common
    /// shape with no scroll containers or group effects, borrowable at no
    /// cost. `None` otherwise: no whole-frame composition is materialized
    /// beside the fragments (it would double the content-proportional
    /// memory); compose one with [`Self::compose_into`] where a flat scene
    /// is genuinely needed.
    #[must_use]
    pub fn scene(&self) -> Option<&Scene> {
        matches!(
            self.presentation.program.as_slice(),
            [ComposeOp::Fragment {
                index: 0,
                space: None
            }]
        )
        .then(|| &self.presentation.fragments[0])
    }

    /// Composes the frame into `scene` with each scroll slot at the offset
    /// `offset_of` reports for it, falling back to the committed one.
    ///
    /// This is the compositor's per-frame path: scrolling recomposes instead
    /// of recommitting, for as long as every overridden offset stays inside
    /// its slot's [`ScrollSlot::encode_window`].
    ///
    /// An offset on a circular axis may stand anywhere: it is normalized into
    /// the slot's period ([`ScrollSlot::wrap`]) before anything reads it, and
    /// a slot whose scrollport straddles the seam draws its content a second
    /// time, one period back.
    pub fn compose_into(
        &self,
        scene: &mut Scene,
        images: &[Option<ImageData>],
        filtered: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        animation_now: Option<f64>,
    ) {
        let wrapped = wrapped_offsets(offset_of);
        let offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>> = &wrapped;
        let composed = &self.presentation.composed;
        let animations =
            self.order
                .sample_composed_animations(&composed.animations, animation_now, offset_of);
        let stickies = self.order.sample_composed_stickies(
            &composed.stickies,
            self.device_pixel_ratio,
            offset_of,
        );
        let anchored =
            self.order
                .sample_anchored(&animations, &stickies, self.device_pixel_ratio, offset_of);
        compose::replay(
            scene,
            &self.presentation.fragments,
            &self.presentation.program,
            &self.presentation.image_draws,
            images,
            &self.presentation.filter_groups,
            filtered,
            &self.order.space_samples(
                &animations,
                &stickies,
                &anchored,
                self.device_pixel_ratio,
                offset_of,
            ),
        );
    }

    /// The frame's offscreen-baked entries — `filter: blur()` groups and
    /// `backdrop-filter` elements — in program order. Empty for the
    /// overwhelmingly common frame, which is why the whole bake pre-step is
    /// skipped on one test of this slice.
    #[must_use]
    pub fn filter_groups(&self) -> &[FilterGroup] {
        &self.presentation.filter_groups
    }

    /// Replays filter entry `index`'s own ops into `scene`, in the bake
    /// target's coordinates: device px with the entry's `rect` origin at
    /// `(0, 0)` and the entry's own space divided out, because that space is
    /// applied when the baked texture is drawn rather than baked into it.
    /// `SpaceSamples::bake_map` fixes that division's side — left, the only
    /// side that survives a rotating or scaling node between the entry's
    /// space and an op's.
    ///
    /// The replay samples the curves at `animation_now` and the live offsets
    /// exactly when the entry [`FilterGroup::samples_animations`], and takes
    /// their committed values otherwise, where no relative map in its range
    /// depends on them.
    /// A `backdrop-filter` entry's range is a prefix of the frame, so it can
    /// hold other elements' exported curves; a `filter: blur()` group's holds
    /// the curves of its own content, and no clip of its ancestors', which
    /// the walker pushes outside the group. After the replay a
    /// backdrop pops the layers the range left open and draws its pre-blur
    /// passes over the whole bake rect, so neither lands inside a clip.
    ///
    /// Offsets on a circular axis are normalized as [`Self::compose_into`]
    /// normalizes them, and the seam copy of a straddling container the
    /// entry does not ride is baked into the texture with the rest.
    ///
    /// `filtered` must already hold the textures of every entry this one's
    /// range draws — bake in order of increasing `ops.end`.
    pub fn bake_filter(
        &self,
        index: usize,
        scene: &mut Scene,
        images: &[Option<ImageData>],
        filtered: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        animation_now: Option<f64>,
    ) {
        let groups = &self.presentation.filter_groups;
        let Some(group) = groups.get(index) else {
            return;
        };
        let wrapped = wrapped_offsets(offset_of);
        let offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>> = &wrapped;
        let composed = &self.presentation.composed;
        let animations = if group.samples_animations() {
            self.order
                .sample_composed_animations(&composed.animations, animation_now, offset_of)
        } else {
            PaintOrder::committed_animations(&composed.animations)
        };
        let stickies = self.order.sample_composed_stickies(
            &composed.stickies,
            self.device_pixel_ratio,
            offset_of,
        );
        let anchored =
            self.order
                .sample_anchored(&animations, &stickies, self.device_pixel_ratio, offset_of);
        let samples = self.order.space_samples(
            &animations,
            &stickies,
            &anchored,
            self.device_pixel_ratio,
            offset_of,
        );
        // An entry inside a box `position-visibility` hides composes through
        // the zero map: it draws nothing, so it bakes nothing either — its
        // own map has no inverse to bake through.
        if samples.device(group.space).determinant().abs() < f64::EPSILON {
            return;
        }
        compose::replay_seams(
            scene,
            compose::Tables {
                fragments: &self.presentation.fragments,
                program: &self.presentation.program,
                image_draws: &self.presentation.image_draws,
                images,
                filter_groups: groups,
                filtered,
                spaces: self.order.spaces(),
                samples: &animations,
            },
            group.ops.start as usize..group.ops.end as usize,
            &samples,
            compose::ReplayTarget::Bake {
                entry: group.space,
                origin: (group.rect.x0, group.rect.y0),
            },
        );
        let Some(backdrop) = group.backdrop.as_ref() else {
            return;
        };
        for _ in 0..backdrop.open_pushes {
            scene.pop_layer();
        }
        let (width, height) = group.size();
        crate::paint::filters::draw_passes_rect(
            scene,
            &backdrop.before,
            crate::vello::kurbo::Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
            Affine::IDENTITY,
        );
    }

    /// Reads this frame's pixels once, into a table the compose path indexes
    /// by draw rather than by name.
    ///
    /// Composition replays the program on every frame that scrolls or
    /// animates, so resolving by source string there would hash a URL and
    /// clone an `ImageData` per draw per frame to re-learn an answer that
    /// only changes when the commit does. This runs once per commit instead,
    /// and encoding a draw becomes a slice index.
    ///
    /// `images` ends up with one entry per image draw, in draw order.
    /// `sources` receives each distinct image once, in first-draw order —
    /// the frame's working set, and the residency hint a host is given.
    ///
    /// Each source is read once, with the size hint covering every draw of
    /// it in this frame — which is why the draws are walked before the first
    /// read: a source drawn small first and large later must be decoded for
    /// the large draw.
    pub fn resolve_images<P: crate::FrameImages + ?Sized>(
        &self,
        pixels: &P,
        images: &mut Vec<Option<ImageData>>,
        sources: &mut Vec<std::sync::Arc<str>>,
    ) {
        images.clear();
        sources.clear();
        // One entry per distinct source, parallel to `sources`. A frame draws
        // few distinct images however many draws it has, so this scan is over
        // a handful of pointers.
        let mut hints: Vec<crate::ImageSizeHint> = Vec::new();
        let mut indices: Vec<usize> = Vec::with_capacity(self.presentation.image_draws.len());
        for draw in &self.presentation.image_draws {
            // Every draw of one source shares one allocation — the registry
            // hands back its own key — so pointer identity is the whole
            // dedup, and no URL is ever compared.
            let seen = sources
                .iter()
                .position(|source| std::sync::Arc::ptr_eq(source, &draw.image));
            let hint = draw.size_hint();
            let index = seen.unwrap_or_else(|| {
                sources.push(std::sync::Arc::clone(&draw.image));
                hints.push(hint);
                sources.len() - 1
            });
            hints[index] = hints[index].union(hint);
            indices.push(index);
        }
        let distinct: Vec<Option<ImageData>> = sources
            .iter()
            .zip(&hints)
            .map(|(source, hint)| pixels.read(source, *hint))
            .collect();
        images.extend(indices.into_iter().map(|index| distinct[index].clone()));
    }

    /// This frame's commit id. Monotonic across a document's life, so it
    /// orders commits; it says nothing about what changed between two of
    /// them.
    #[must_use]
    pub fn commit_id(&self) -> u64 {
        self.order.commit_id()
    }

    /// Whether the document had a running animation at commit time — the
    /// compositor's cue to keep producing frames.
    #[must_use]
    pub const fn animations_active(&self) -> bool {
        self.animations_active
    }

    /// Whether something animating still needs per-frame main-thread ticks:
    /// an animation or transition this frame could not export as a curve.
    /// The painter posts one frame request per frame while this holds.
    #[must_use]
    pub const fn needs_main_ticks(&self) -> bool {
        self.needs_main_ticks
    }

    /// Whether the compose program references an exported curve that reads
    /// the document timeline — the compositor then recomposes each frame at
    /// its clock reading instead of reusing the drawn frame.
    ///
    /// A curve the program leaves out — culled, or on content that draws
    /// nothing — changes no pixel at any instant of its domain, so a frame
    /// whose curves are all left out composes once. A curve on a scroll
    /// timeline alone moves only when a scroll offset does, and a scroll
    /// recomposes by itself. Hit tests read [`Self::has_exported_curves`]
    /// instead, and [`Self::animation_boundary_passed`] counts every curve.
    #[must_use]
    pub fn has_live_curves(&self) -> bool {
        self.presentation.composed.clock
    }

    /// Whether the compose program references a curve that reads a scroll
    /// slot's offset: a filter entry sampling animations then re-bakes on a
    /// scroll, whatever the paths its range rides.
    pub(crate) fn composes_scroll_curves(&self) -> bool {
        self.presentation.composed.scroll
    }

    /// Whether the frame exports any curve — hit tests then sample at the
    /// input's clock reading: a curve moves its element's hit area even where
    /// the program draws nothing of it. A scroll timeline's curve samples its
    /// offsets whatever the reading.
    #[must_use]
    pub fn has_exported_curves(&self) -> bool {
        !self.order.animations().is_empty()
    }

    /// Whether any exported curve has run past its domain at `now`: the cue
    /// to post a frame request — each frame, until a commit without the passed
    /// curve is adopted — so the main thread runs the finish restyle and
    /// commits the animation's end state. Until then the compositor draws
    /// the curve's last instant inside its domain: past it an animation's
    /// contribution can be replaced by the base value, which only that
    /// commit knows.
    ///
    /// Read off the commit's own minimum expiry, because the compositor asks
    /// this on every input, draw, capture and tick.
    #[must_use]
    pub fn animation_boundary_passed(&self, now: f64) -> bool {
        self.earliest_expiry.is_some_and(|expiry| now >= expiry)
    }

    /// The CSS-px viewport this frame was committed for.
    #[must_use]
    pub const fn viewport(&self) -> Size2D<f32> {
        self.viewport
    }

    #[must_use]
    pub const fn device_pixel_ratio(&self) -> f32 {
        self.device_pixel_ratio
    }

    /// Every snap position in the frame, sliced per slot and axis by
    /// [`ScrollSlot::snap`].
    #[must_use]
    pub fn snap_points(&self) -> &[SnapPoint] {
        self.order.snap_points()
    }

    /// A slot's snapping on each axis, over the frame's points. On a
    /// circular axis the positions repeat every [`ScrollSlot::wrap_period`],
    /// so the painter snaps across the seam.
    #[must_use]
    pub fn snap_axes(&self, slot: &ScrollSlot) -> (Option<SnapAxis<'_>>, Option<SnapAxis<'_>>) {
        let points = self.snap_points();
        let (period_x, period_y) = slot.wrap_period();
        (
            slot.snap.x.as_ref().map(|axis| axis.axis(points, period_x)),
            slot.snap.y.as_ref().map(|axis| axis.axis(points, period_y)),
        )
    }

    /// The frame's scroll containers, chain-linked; see [`ScrollSlot`].
    #[must_use]
    pub fn scroll_slots(&self) -> &[ScrollSlot] {
        self.order.slots()
    }

    /// The slot a scroll container node has in this frame, if it is one:
    /// one lookup in the index the commit built.
    #[must_use]
    pub fn slot_of(&self, node: NodeId) -> Option<u32> {
        self.order.slot_of(node)
    }

    /// Whether this frame can still be composed at `offset` for the scroll
    /// container `node`: it carries the container as a slot, and `offset` is
    /// inside that slot's [`ScrollSlot::encode_window`].
    ///
    /// `false` for a container this frame does not know, which is the case
    /// the caller has to rebuild for anyway.
    #[must_use]
    pub fn covers_scroll_offset(&self, node: NodeId, offset: Vector2D<f32>) -> bool {
        self.slot_of(node).is_some_and(|index| {
            let (low, high) = self.order.slots()[index as usize].encode_window();
            offset.x >= low.x && offset.x <= high.x && offset.y >= low.y && offset.y <= high.y
        })
    }

    /// The topmost hit-testable element at `point`, with its scroll slot.
    ///
    /// The frame is baked unscrolled; `offset_of` supplies each slot's
    /// current offset (the compositor's between-commit values), falling back
    /// to the committed ones. No liveness filter — see the module doc.
    ///
    /// An offset on a circular axis is normalized into its period first, as
    /// composition does. Where a slot's scrollport straddles the seam, an
    /// item that rides the slot is tested at its original place and then at
    /// its copy one period back, in its own place in the front-to-back
    /// order: the two copies tile the scrollport and both are cut by its
    /// clip, so at most one of them is under the point.
    #[must_use]
    pub fn hit(
        &self,
        point: Point2D<f32>,
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        animation_now: Option<f64>,
    ) -> Option<HitTarget> {
        let wrapped = wrapped_offsets(offset_of);
        let offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>> = &wrapped;
        let animations = self.order.sample_animations(animation_now, offset_of);
        let stickies = self
            .order
            .sample_stickies(self.device_pixel_ratio, offset_of);
        let anchored =
            self.order
                .sample_anchored(&animations, &stickies, self.device_pixel_ratio, offset_of);
        let samples = self.order.space_samples(
            &animations,
            &stickies,
            &anchored,
            self.device_pixel_ratio,
            offset_of,
        );
        let passes = samples.seam_passes();
        if passes.is_empty() {
            return self.order.raw_hits_at(point, &samples).next();
        }
        let offsets: SmallVec<[_; 2]> = passes
            .iter()
            .map(|&(slot, shift)| samples.shifted_offsets(slot, shift))
            .collect();
        let seams: SmallVec<[(u32, SpaceSamples<'_>); 2]> = passes
            .iter()
            .zip(&offsets)
            .map(|(&(slot, _), offsets)| {
                (
                    slot,
                    SpaceSamples {
                        offset_of: offsets,
                        ..samples
                    },
                )
            })
            .collect();
        let spaces = self.order.spaces();
        self.order.items().iter().rev().find_map(|item| {
            let node = self.order.item_hit(item, point, &samples).or_else(|| {
                seams
                    .iter()
                    .filter(|(slot, _)| super::space::rides(spaces, item.space, *slot))
                    .find_map(|(_, seam)| self.order.item_hit(item, point, seam))
            })?;
            Some(HitTarget {
                node,
                scroll: item.slot,
            })
        })
    }

    /// The frame's composite-animated elements; see [`AnimationSlot`].
    #[must_use]
    pub fn animation_slots(&self) -> &[AnimationSlot] {
        self.order.animations()
    }

    /// The first slot on the chain from `from` (inclusive) the user may
    /// scroll on any of `axes` — the published-data equivalent of the
    /// document's `nearest_user_scrollable`.
    #[must_use]
    pub fn nearest_user_scrollable(&self, from: Option<u32>, axes: ScrollAxes) -> Option<u32> {
        let slots = self.order.slots();
        let mut current = from;
        while let Some(index) = current {
            let slot = slots[index as usize];
            if (slot.user_scrollable.x && axes.x) || (slot.user_scrollable.y && axes.y) {
                return Some(index);
            }
            current = slot.parent;
        }
        None
    }
}

#[cfg(test)]
impl CommittedFrame {
    /// The frame's scroll slots at their committed offsets.
    pub(crate) fn committed_offsets(&self) -> ScrollOffsets<'_> {
        fn committed(_: &ScrollSlot) -> Option<Vector2D<f32>> {
            None
        }
        ScrollOffsets {
            slots: self.scroll_slots(),
            offset_of: &committed,
        }
    }
}

impl PaintOrder {
    /// Front-to-back hits with their scroll slots, no liveness filter.
    pub(crate) fn raw_hits_at<'frame>(
        &'frame self,
        point: Point2D<f32>,
        samples: &'frame SpaceSamples<'frame>,
    ) -> impl Iterator<Item = HitTarget> + 'frame {
        self.hit_testable_items()
            .iter()
            .rev()
            .filter_map(move |item| {
                let node = self.item_hit(item, point, samples)?;
                Some(HitTarget {
                    node,
                    scroll: item.slot,
                })
            })
    }
}

/// The whole point of the type: it crosses threads.
#[allow(dead_code, reason = "compile-time thread-safety assertion")]
const fn assert_frame_is_shareable()
where
    CommittedFrame: Send + Sync,
{
}

#[cfg(test)]
#[allow(clippy::float_cmp, reason = "the expectations are exact offsets")]
mod tests {
    use euclid::default::{Point2D, Vector2D};

    use super::ScrollSlot;
    use crate::scroll::ScrollAxes;
    use crate::tree::document::tests::device;
    use crate::{Document, StylesheetOrigin};

    fn scrolling_page() -> (Document<()>, crate::NodeId, crate::NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .scroller { display: flex; overflow: scroll; width: 200px; height: 200px; }
             .content { flex-shrink: 0; width: 200px; height: 1000px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let scroller = document.create_element("view", ());
        document.add_class(scroller, "scroller");
        document.append_child(root, scroller);
        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(scroller, content);
        (document, root, scroller)
    }

    /// A 100×100 scrollport over 500 px of content on both axes (max 400,
    /// period 500), circular on `y` alone, standing at `(200, 200)`.
    fn circular_y_slot() -> ScrollSlot {
        ScrollSlot {
            node: crate::tree::document::DOCUMENT_ELEMENT_NODE_ID,
            parent: None,
            user_scrollable: ScrollAxes { x: true, y: true },
            chains: ScrollAxes::NONE,
            bounce: ScrollAxes::NONE,
            circular: ScrollAxes { x: false, y: true },
            capture: crate::scroll::CaptureAxes::default(),
            snap: super::SnapSlot::default(),
            offset: Vector2D::new(200.0, 200.0),
            max_offset: Vector2D::new(400.0, 400.0),
            scrollport: euclid::default::Size2D::new(100.0, 100.0),
            viewport_axes: [Vector2D::new(1.0, 0.0), Vector2D::new(0.0, 1.0)],
            request: None,
        }
    }

    #[test]
    fn a_circular_axis_wraps_by_its_whole_scrolling_area() {
        let slot = circular_y_slot();
        assert_eq!(slot.wrap_period(), (None, Some(500.0)));
        assert_eq!(
            slot.wrap(Vector2D::new(-30.0, -30.0)),
            Vector2D::new(-30.0, 470.0),
            "the bounded axis is left alone, unclamped",
        );
        assert_eq!(
            slot.wrap(Vector2D::new(0.0, 1260.0)),
            Vector2D::new(0.0, 260.0)
        );
        assert_eq!(
            slot.wrap(Vector2D::new(0.0, f32::NAN)),
            Vector2D::new(0.0, 0.0)
        );
        assert_eq!(
            slot.wrap(Vector2D::new(0.0, -1.0e-12)),
            Vector2D::new(0.0, 0.0),
            "a hair below the start is the start, never the period itself",
        );

        let flat = ScrollSlot {
            max_offset: Vector2D::new(400.0, 0.0),
            ..slot
        };
        assert_eq!(
            flat.wrap_period(),
            (None, None),
            "nothing overflows, nothing wraps"
        );
        assert_eq!(
            flat.wrap(Vector2D::new(0.0, 30.0)),
            Vector2D::new(0.0, 30.0)
        );
    }

    #[test]
    fn a_circular_axis_encodes_its_whole_scrolling_area_and_never_recenters() {
        let slot = circular_y_slot();
        let (low, high) = slot.encode_window();
        assert_eq!(low, Vector2D::new(100.0, 0.0), "x keeps its ±1 scrollport");
        assert_eq!(high, Vector2D::new(300.0, 400.0), "y spans 0..=max");
        assert!(!slot.recenter_due(Vector2D::new(200.0, 0.0)));
        assert!(!slot.recenter_due(Vector2D::new(200.0, 400.0)));
        assert!(
            slot.recenter_due(Vector2D::new(290.0, 200.0)),
            "the bounded axis still recenters"
        );
    }

    #[test]
    fn a_commit_publishes_the_scroll_table_and_slotted_hits() {
        let (mut document, root, scroller) = scrolling_page();
        let frame = document.commit();

        let slots = frame.scroll_slots();
        assert_eq!(slots.len(), 1, "one scroll container, one slot");
        assert_eq!(slots[0].node, scroller);
        assert_eq!(slots[0].parent, None);
        assert!(slots[0].user_scrollable.y);
        assert!((slots[0].max_offset.y - 800.0).abs() < 0.5);

        let inside = frame
            .hit(Point2D::new(50.0, 50.0), &|_| None, None)
            .expect("content hit");
        assert_eq!(inside.scroll, Some(0), "content carries its scroller");
        let outside = frame
            .hit(Point2D::new(500.0, 500.0), &|_| None, None)
            .expect("page hit");
        assert_eq!(outside.node, root);
        assert_eq!(outside.scroll, None, "the page is no scroll container");
    }

    #[test]
    fn a_composable_scroll_recommits_nothing_and_the_next_commit_publishes_it() {
        let (mut document, root, scroller) = scrolling_page();
        let before = document.commit();
        document.scroll_to(scroller, crate::Vector2D::new(0.0, 120.0));
        assert!(
            !document.needs_render(),
            "a scroll the retained frame can compose invalidates nothing"
        );
        assert_eq!(
            document.commit().commit_id(),
            before.commit_id(),
            "no new frame is built for it"
        );

        // Any real commit picks the live offset up into the slot table.
        document.set_inline_style(root, "background-color: teal");
        let frame = document.commit();
        assert!((frame.scroll_slots()[0].offset.y - 120.0).abs() < f32::EPSILON);
    }

    #[test]
    fn an_absolute_escapee_links_past_its_dom_side_scroller() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.add_stylesheet(
            "page { display: flex; width: 800px; height: 600px; }
             .outer { display: flex; overflow: scroll; width: 400px; height: 400px; }
             .inner { display: flex; overflow: scroll; width: 200px; height: 200px; }
             .content { flex-shrink: 0; width: 200px; height: 1000px; }
             .escapee { position: absolute; left: 10px; top: 10px;
                        width: 20px; height: 20px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let outer = document.create_element("view", ());
        document.add_class(outer, "outer");
        document.append_child(root, outer);
        let inner = document.create_element("view", ());
        document.add_class(inner, "inner");
        document.append_child(outer, inner);
        let content = document.create_element("view", ());
        document.add_class(content, "content");
        document.append_child(inner, content);
        let escapee = document.create_element("view", ());
        document.add_class(escapee, "escapee");
        document.append_child(inner, escapee);

        let frame = document.commit();
        let slots = frame.scroll_slots();
        let inner_slot = frame.slot_of(inner).expect("the inner scroller has a slot");
        assert_eq!(
            slots[inner_slot as usize].parent,
            frame.slot_of(outer),
            "nested scrollers chain outward"
        );

        // The escapee's containing block is the viewport (no positioned
        // ancestor), so its item must carry no slot at all — it neither
        // scrolls with the inner scroller nor chains into it.
        let hit = frame
            .hit(Point2D::new(15.0, 15.0), &|_| None, None)
            .expect("escapee hit");
        assert_eq!(hit.node, escapee);
        assert_eq!(hit.scroll, None, "the escapee left both scrollers");
    }
}
