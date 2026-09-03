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

use super::{AnimationSample, PaintOrder};
use crate::NodeId;
use crate::paint::compose::{self, ComposeOp};
use crate::scroll::ScrollAxes;
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
    /// The committed, already-clamped offset.
    pub offset: Vector2D<f32>,
    /// The largest offset the committed geometry admits, per axis.
    pub max_offset: Vector2D<f32>,
    /// The scrollport (padding box) size, which is also what the encode
    /// window is sized from.
    pub scrollport: Size2D<f32>,
    /// The scrollport's clip node in the frame's clip table — the shape the
    /// compose plan sizes this slot's retained layer from.
    pub(crate) clip: Option<u32>,
}

impl ScrollSlot {
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

/// One composite-animated element in the committed frame: the target of the
/// compose-time retargeting that lets its animation play without commits.
///
/// A slot with no exported curve still tags its subtree — the element's
/// animation was found ineligible after the slot was allocated — and samples
/// as the committed values, so composition draws exactly the committed frame
/// and the element rides main-thread ticks instead.
#[derive(Debug)]
pub struct AnimationSlot {
    /// The animated element.
    pub node: NodeId,
    /// The nearest enclosing animation slot, when animated elements nest.
    pub(crate) parent: Option<u32>,
    /// The exported curve, absent in the ineligible case described above.
    pub(crate) curve: Option<crate::visual::curves::CompositeCurve>,
}

impl AnimationSlot {
    /// This slot's compose values at `now`: `None` — or no exported curve —
    /// samples the committed values (identity delta, committed opacity).
    pub(crate) fn sample(&self, now: Option<f64>) -> AnimationSample {
        let committed = AnimationSample {
            parent: self.parent,
            delta: Affine::IDENTITY,
            alpha: None,
        };
        let (Some(curve), Some(now)) = (&self.curve, now) else {
            return committed;
        };
        let sample = curve.sample(now);
        AnimationSample {
            parent: self.parent,
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
/// The scene is carried *split*: per-chain fragments plus the compose
/// program over them, so scroll offsets apply at composition.
pub struct CommittedFrame {
    pub(crate) order: PaintOrder,
    pub(crate) presentation: Presentation,
    pub(crate) animations_active: bool,
    pub(crate) needs_main_ticks: bool,
    pub(crate) viewport: Size2D<f32>,
    pub(crate) device_pixel_ratio: f32,
}

/// The split scene: fragments, the program that assembles them, and the
/// layer plan when scroller content bakes into retained planes.
pub(crate) struct Presentation {
    pub(crate) fragments: Vec<Scene>,
    pub(crate) program: Vec<ComposeOp>,
    /// One entry per [`ComposeOp::Image`], in program order. Carries names
    /// and geometry; never pixels.
    pub(crate) image_draws: Vec<crate::paint::compose::ImageDraw>,
    pub(crate) plan: Option<crate::paint::plan::CompositePlan>,
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
            [ComposeOp::Fragment { index: 0, chain }]
                if *chain == crate::paint::compose::ComposeChain::default()
        )
        .then(|| &self.presentation.fragments[0])
    }

    /// The frame's layer decomposition, present when its scroller content
    /// bakes into retained planes a GPU target keeps between commits.
    #[must_use]
    pub fn composite_plan(&self) -> Option<&crate::paint::plan::CompositePlan> {
        self.presentation.plan.as_ref()
    }

    /// Encodes plane `index` of the frame's plan into `scene`, translated so
    /// the plane's rect starts at the origin — the scene a target renders
    /// into the plane's texture, over a transparent base.
    ///
    /// # Panics
    ///
    /// If the frame has no plan or `index` is out of range.
    pub fn bake_plane(&self, index: usize, scene: &mut Scene, images: &[Option<ImageData>]) {
        let plan = self
            .composite_plan()
            .expect("bake_plane reads the frame's plan");
        let spec = &plan.planes[index];
        let translate = Affine::translate((-spec.rect.x0, -spec.rect.y0));
        let ops = spec.ops.start as usize..spec.ops.end as usize;
        compose::bake_ops(
            scene,
            &self.presentation.fragments,
            &self.presentation.program[ops],
            &self.presentation.image_draws,
            images,
            spec.slot,
            translate,
        );
    }

    /// Composes the frame from its plan: raw steps replay at the offsets
    /// `offset_of` reports, planes draw as `plane_images[index]` — the
    /// textures a target baked via [`Self::bake_plane`] — translated by
    /// their slot chains. The per-frame scroll path for a layered frame:
    /// its cost is the raw (root and animated) content plus one image draw
    /// per plane, never the scroller content.
    ///
    /// # Panics
    ///
    /// If the frame has no plan or `plane_images` is shorter than the plan's
    /// planes.
    pub fn composite_into(
        &self,
        scene: &mut Scene,
        plane_images: &[crate::vello::peniko::ImageData],
        images: &[Option<ImageData>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        animation_now: Option<f64>,
    ) {
        use crate::vello::peniko::{Extend, ImageBrush, ImageQuality, ImageSampler};
        let plan = self
            .composite_plan()
            .expect("composite_into reads the frame's plan");
        let samples = self.order.sample_animations(animation_now);
        let device_transform = compose::device_chain_transform(
            self.order.slots(),
            &samples,
            self.device_pixel_ratio,
            offset_of,
        );
        for step in &plan.steps {
            match step {
                crate::paint::plan::CompositeStep::Ops(range) => {
                    let ops = range.start as usize..range.end as usize;
                    compose::replay_ops(
                        scene,
                        &self.presentation.fragments,
                        &self.presentation.program[ops],
                        &self.presentation.image_draws,
                        images,
                        &samples,
                        &device_transform,
                    );
                }
                crate::paint::plan::CompositeStep::Plane(index) => {
                    let spec = &plan.planes[*index as usize];
                    // The slot's clip chain re-applies around the draw, each
                    // clip at its own chain's offset — the bake skipped these
                    // shapes because they must not translate with the plane.
                    let scale = Affine::scale(f64::from(self.device_pixel_ratio));
                    let slots = self.order.slots();
                    let clips = self.order.clips();
                    let mut chain_clips: Vec<u32> = Vec::new();
                    let mut next = slots[spec.slot as usize].clip;
                    while let Some(clip) = next {
                        chain_clips.push(clip);
                        next = clips[clip as usize]
                            .parent
                            .map(|parent| u32::try_from(parent).expect("clip indices fit u32"));
                    }
                    for &clip in chain_clips.iter().rev() {
                        let node = &clips[clip as usize];
                        let outer = device_transform(crate::paint::walker::clip_chain(node));
                        crate::paint::walker::encode_clip(scene, node, outer, scale);
                    }
                    let chain = crate::paint::compose::ComposeChain {
                        scroll: Some(spec.slot),
                        animation: None,
                    };
                    // Integer device translations by construction — the rect
                    // is integer-valued and offsets are snapped — so nearest
                    // sampling reproduces the texture exactly.
                    let transform =
                        device_transform(chain) * Affine::translate((spec.rect.x0, spec.rect.y0));
                    let brush = ImageBrush {
                        image: &plane_images[*index as usize],
                        sampler: ImageSampler {
                            x_extend: Extend::Pad,
                            y_extend: Extend::Pad,
                            quality: ImageQuality::Low,
                            alpha: 1.0,
                        },
                    };
                    scene.draw_image(brush, transform);
                    for _ in &chain_clips {
                        scene.pop_layer();
                    }
                }
            }
        }
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
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        animation_now: Option<f64>,
    ) {
        let samples = self.order.sample_animations(animation_now);
        compose::replay(
            scene,
            &self.presentation.fragments,
            &self.presentation.program,
            &self.presentation.image_draws,
            images,
            self.order.slots(),
            &samples,
            self.device_pixel_ratio,
            offset_of,
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

    /// Whether the frame carries any exported curve — the compositor then
    /// recomposes each frame at its clock reading instead of reusing the
    /// drawn frame.
    #[must_use]
    pub fn has_live_curves(&self) -> bool {
        self.order
            .animations()
            .iter()
            .any(|slot| slot.curve.is_some())
    }

    /// Whether any exported curve has run past its domain at `now`: the cue
    /// to send one `BeginFrame` so the main thread runs the finish restyle
    /// and commits the animation's end state.
    #[must_use]
    pub fn animation_boundary_passed(&self, now: f64) -> bool {
        self.order.animations().iter().any(|slot| {
            slot.curve
                .as_ref()
                .is_some_and(|curve| curve.expired_at(now))
        })
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
        let samples = self.order.sample_animations(animation_now);
        self.order
            .raw_hits_at(point, offset_of, &samples, self.device_pixel_ratio)
            .next()
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
        offset_of: &'frame (dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>> + 'frame),
        samples: &'frame [AnimationSample],
        ratio: f32,
    ) -> impl Iterator<Item = HitTarget> + 'frame {
        self.items().iter().rev().filter_map(move |item| {
            let node = self.item_hit(item, point, offset_of, samples, ratio)?;
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
