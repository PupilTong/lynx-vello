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

use super::{AnimationSample, PaintOrder, SpaceSamples};
use crate::NodeId;
use crate::paint::compose::{self, ComposeOp, FilterGroup};
use crate::scroll::{ChainLink, ScrollAxes, ScrollCapture, SnapAxis, SnapPoint, SnapStrictness};
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
    /// Whether the container above goes first: `scroll-capture`.
    pub capture: ScrollCapture,
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
    fn axis<'frame>(&self, points: &'frame [SnapPoint]) -> SnapAxis<'frame> {
        SnapAxis {
            strictness: self.strictness,
            points: &points[self.start as usize..self.end as usize],
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

    /// The offset range the committed encode covers on each axis — the
    /// window compose may move through without a recommit. Sized in
    /// scrollports around the committed offset, clamped to what the geometry
    /// admits.
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
        let low = Vector2D::new(
            (self.offset.x - slack.x).max(0.0),
            (self.offset.y - slack.y).max(0.0),
        );
        let high = Vector2D::new(
            (self.offset.x + slack.x).min(self.max_offset.x),
            (self.offset.y + slack.y).min(self.max_offset.y),
        );
        (low, high)
    }
}

/// How far past the committed offset, in scrollports per scrollable axis,
/// the encode covers — the compose headroom before a refill commit is due.
pub const ENCODE_WINDOW_SCROLLPORTS: f32 = 1.0;

/// The largest area, in viewports, of an element's `max(size, content_size)`
/// that still exports a transform curve. Culling bounds a moving subtree by
/// the viewport pulled back through its curve's reach, so this cap is what
/// bounds the encode only where no reach does — a scale range reaching 0,
/// which admits the whole subtree. Firefox caps composited transform
/// animations the same way (`nsDisplayList.cpp`: 1.125 viewports, 4096² px).
pub(crate) const MAX_MOVING_EXTENT_VIEWPORTS: f32 = 3.0;

/// One composite-animated element in the committed frame: the target of the
/// compose-time retargeting that lets its animation play without commits.
///
/// A slot exists only where a curve was exported — every structural and
/// value-level refusal happens before it is allocated — so sampling one is
/// always sampling a live curve.
#[derive(Debug)]
pub struct AnimationSlot {
    /// The animated element.
    pub node: NodeId,
    /// The exported curve.
    pub(crate) curve: crate::visual::curves::CompositeCurve,
}

impl AnimationSlot {
    /// This slot's compose values at `now`; `None` is the committed values
    /// (identity delta, committed opacity).
    pub(crate) fn sample(&self, now: Option<f64>) -> AnimationSample {
        let Some(now) = now else {
            return AnimationSample {
                delta: Affine::IDENTITY,
                alpha: None,
            };
        };
        let sample = self.curve.sample(now);
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
    pub fn compose_into(
        &self,
        scene: &mut Scene,
        images: &[Option<ImageData>],
        filtered: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        animation_now: Option<f64>,
    ) {
        let composed = &self.presentation.composed;
        let animations = self
            .order
            .sample_composed_animations(&composed.animations, animation_now);
        let stickies = self.order.sample_composed_stickies(
            &composed.stickies,
            self.device_pixel_ratio,
            offset_of,
        );
        compose::replay(
            scene,
            &self.presentation.fragments,
            &self.presentation.program,
            &self.presentation.image_draws,
            images,
            &self.presentation.filter_groups,
            filtered,
            &self
                .order
                .space_samples(&animations, &stickies, self.device_pixel_ratio, offset_of),
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
    /// The replay samples at `animation_now` exactly when the entry
    /// [`FilterGroup::samples_animations`], and at the committed instant
    /// otherwise, where every relative map in its range is time-independent.
    /// A `backdrop-filter` entry's range is a prefix of the frame, so it can
    /// hold other elements' exported curves; a `filter: blur()` group's holds
    /// its ancestors' clips, which its own element's curve moves it across.
    /// Export eligibility refuses an animated element inside a composited
    /// group, so no curve moves a group's own content. After the replay a
    /// backdrop pops the layers the range left open and draws its pre-blur
    /// passes over the whole bake rect, so neither lands inside a clip.
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
        let composed = &self.presentation.composed;
        let animations = self.order.sample_composed_animations(
            &composed.animations,
            animation_now.filter(|_| group.samples_animations()),
        );
        let stickies = self.order.sample_composed_stickies(
            &composed.stickies,
            self.device_pixel_ratio,
            offset_of,
        );
        let samples =
            self.order
                .space_samples(&animations, &stickies, self.device_pixel_ratio, offset_of);
        let transform = samples.bake_map(group.space, (group.rect.x0, group.rect.y0));
        compose::replay_ops(
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
            &transform,
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
    /// The compositor sends one `BeginFrame` per frame while this holds.
    #[must_use]
    pub const fn needs_main_ticks(&self) -> bool {
        self.needs_main_ticks
    }

    /// Whether the compose program references an exported curve — the
    /// compositor then recomposes each frame at its clock reading instead of
    /// reusing the drawn frame.
    ///
    /// A curve the program leaves out — culled, or on content that draws
    /// nothing — changes no pixel at any instant of its domain, so a frame
    /// whose curves are all left out composes once. Hit tests read
    /// [`Self::has_exported_curves`] instead, and
    /// [`Self::animation_boundary_passed`] counts every curve.
    #[must_use]
    pub fn has_live_curves(&self) -> bool {
        !self.presentation.composed.animations.is_empty()
    }

    /// Whether the frame exports any curve — hit tests then sample at the
    /// input's clock reading: a curve moves its element's hit area even where
    /// the program draws nothing of it.
    #[must_use]
    pub fn has_exported_curves(&self) -> bool {
        !self.order.animations().is_empty()
    }

    /// Whether any exported curve has run past its domain at `now`: the cue
    /// to send one `BeginFrame` so the main thread runs the finish restyle
    /// and commits the animation's end state.
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

    /// A slot's snapping on each axis, over the frame's points.
    #[must_use]
    pub fn snap_axes(&self, slot: &ScrollSlot) -> (Option<SnapAxis<'_>>, Option<SnapAxis<'_>>) {
        let points = self.snap_points();
        (
            slot.snap.x.as_ref().map(|axis| axis.axis(points)),
            slot.snap.y.as_ref().map(|axis| axis.axis(points)),
        )
    }

    /// The frame's scroll containers, chain-linked; see [`ScrollSlot`].
    #[must_use]
    pub fn scroll_slots(&self) -> &[ScrollSlot] {
        self.order.slots()
    }

    /// The slot a scroll container node has in this frame, if it is one.
    #[must_use]
    pub fn slot_of(&self, node: NodeId) -> Option<u32> {
        self.order
            .slots()
            .iter()
            .position(|slot| slot.node == node)
            .map(|index| u32::try_from(index).expect("a frame cannot hold 2^32 scroll containers"))
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
    #[must_use]
    pub fn hit(
        &self,
        point: Point2D<f32>,
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        animation_now: Option<f64>,
    ) -> Option<HitTarget> {
        let animations = self.order.sample_animations(animation_now);
        let stickies = self
            .order
            .sample_stickies(self.device_pixel_ratio, offset_of);
        let samples =
            self.order
                .space_samples(&animations, &stickies, self.device_pixel_ratio, offset_of);
        self.order.raw_hits_at(point, &samples).next()
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

impl PaintOrder {
    /// Front-to-back hits with their scroll slots, no liveness filter.
    pub(crate) fn raw_hits_at<'frame>(
        &'frame self,
        point: Point2D<f32>,
        samples: &'frame SpaceSamples<'frame>,
    ) -> impl Iterator<Item = HitTarget> + 'frame {
        self.items().iter().rev().filter_map(move |item| {
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
mod tests {
    use euclid::default::Point2D;

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
