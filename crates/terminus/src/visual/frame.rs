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
use crate::kurbo::Affine;
use crate::paint::compose::{self, ComposeOp, FilterGroup};
use crate::paint::display_list::{DisplayList, ImageRef, Texture};
use crate::render::picture::{PictureFrame, PictureKey};
use crate::scroll::{
    AutoScrollRequest, CaptureAxes, ChainLink, ScrollAxes, ScrollRequest, SnapAxis, SnapPoint,
    SnapStrictness, unwrap_axis, wrap_axis, wrap_period_axis,
};
use crate::tree::document::NodeId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollSlot {
    pub node: NodeId,
    pub parent: Option<u32>,
    pub user_scrollable: ScrollAxes,
    pub chains: ScrollAxes,
    pub bounce: ScrollAxes,
    pub circular: ScrollAxes,
    pub(crate) capture: CaptureAxes,
    pub snap: SnapSlot,
    pub offset: Vector2D<f32>,
    pub max_offset: Vector2D<f32>,
    pub scrollport: Size2D<f32>,
    pub viewport_axes: [Vector2D<f32>; 2],
    pub request: Option<ScrollRequest>,
    pub anchor_shift: Vector2D<f32>,
    pub reach: Vector2D<f32>,
}

#[derive(Clone, Copy)]
pub(crate) struct ScrollOffsets<'a> {
    pub(crate) slots: &'a [ScrollSlot],
    pub(crate) offset_of: &'a dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
}

impl ScrollOffsets<'_> {
    pub(crate) fn of(&self, slot: u32) -> Vector2D<f32> {
        let slot = &self.slots[slot as usize];
        (self.offset_of)(slot).unwrap_or(slot.offset)
    }
}

fn wrapped_offsets(
    offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
) -> impl Fn(&ScrollSlot) -> Option<Vector2D<f32>> + '_ {
    move |slot: &ScrollSlot| offset_of(slot).map(|offset| slot.wrap(offset))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapSlotAxis {
    pub strictness: SnapStrictness,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SnapSlot {
    pub x: Option<SnapSlotAxis>,
    pub y: Option<SnapSlotAxis>,
}

impl SnapSlotAxis {
    pub(super) fn axis<'frame>(
        &self,
        points: &'frame [SnapPoint],
        period: Option<f32>,
    ) -> SnapAxis<'frame> {
        SnapAxis {
            strictness: self.strictness,
            points: &points[self.start as usize..self.end as usize],
            period,
        }
    }
}

impl ScrollSlot {
    #[must_use]
    pub(crate) fn link(&self) -> ChainLink {
        ChainLink {
            user_scrollable: self.user_scrollable,
            chains: self.chains,
            capture: self.capture,
        }
    }

    pub(crate) fn viewport_translation(&self, offset: Vector2D<f32>) -> Vector2D<f32> {
        self.viewport_axes[0] * offset.x + self.viewport_axes[1] * offset.y
    }

    #[must_use]
    pub fn wrap_period(&self) -> (Option<f32>, Option<f32>) {
        (
            wrap_period_axis(self.circular.x, self.max_offset.x, self.scrollport.width),
            wrap_period_axis(self.circular.y, self.max_offset.y, self.scrollport.height),
        )
    }

    #[must_use]
    pub fn wrap(&self, offset: Vector2D<f32>) -> Vector2D<f32> {
        let axis = |value: f32, period: Option<f32>| match period {
            Some(period) => wrap_axis(value, period),
            None => value,
        };
        let (x, y) = self.wrap_period();
        Vector2D::new(axis(offset.x, x), axis(offset.y, y))
    }

    #[must_use]
    pub fn unwrap(&self, offset: Vector2D<f32>) -> Vector2D<f32> {
        let (x, y) = self.wrap_period();
        Vector2D::new(
            unwrap_axis(offset.x, self.max_offset.x, x),
            unwrap_axis(offset.y, self.max_offset.y, y),
        )
    }

    #[must_use]
    pub fn encode_window(&self) -> (Vector2D<f32>, Vector2D<f32>) {
        let (wrap_x, wrap_y) = self.wrap_period();
        let (low_x, high_x) = encode_window_axis(
            self.offset.x,
            self.max_offset.x,
            self.scrollport.width,
            wrap_x.is_some(),
        );
        let (low_y, high_y) = encode_window_axis(
            self.offset.y,
            self.max_offset.y,
            self.scrollport.height,
            wrap_y.is_some(),
        );
        (Vector2D::new(low_x, low_y), Vector2D::new(high_x, high_y))
    }

    #[must_use]
    pub fn relevance_window(&self) -> (Vector2D<f32>, Vector2D<f32>) {
        let (low, high) = self.encode_window();
        let (wrap_x, wrap_y) = self.wrap_period();
        let toward = self
            .request
            .map_or(self.reach, |request| request.target - self.offset);
        let (low_x, high_x) = relevance_window_axis(
            (low.x, high.x),
            toward.x,
            self.max_offset.x,
            self.scrollport.width,
            wrap_x.is_some(),
        );
        let (low_y, high_y) = relevance_window_axis(
            (low.y, high.y),
            toward.y,
            self.max_offset.y,
            self.scrollport.height,
            wrap_y.is_some(),
        );
        (Vector2D::new(low_x, low_y), Vector2D::new(high_x, high_y))
    }

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
            } else {
                pending > committed && high - pending < (high - committed) / 2.0
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

pub const ENCODE_WINDOW_SCROLLPORTS: f32 = 1.0;

pub const RELEVANCE_REACH_SCROLLPORTS: f32 = 3.0;

pub(crate) fn encode_window_axis(offset: f32, max: f32, extent: f32, wraps: bool) -> (f32, f32) {
    if wraps {
        return (0.0, max);
    }
    let slack = if max > 0.0 {
        extent * ENCODE_WINDOW_SCROLLPORTS
    } else {
        0.0
    };
    ((offset - slack).max(0.0), (offset + slack).min(max))
}

pub(crate) fn relevance_window_axis(
    window: (f32, f32),
    toward: f32,
    max: f32,
    extent: f32,
    wraps: bool,
) -> (f32, f32) {
    let (low, high) = window;
    if wraps || max <= 0.0 || !toward.is_finite() {
        return window;
    }
    let cap = (RELEVANCE_REACH_SCROLLPORTS * extent).max(0.0);
    let toward = toward.clamp(-cap, cap);
    if toward > 0.0 {
        (low, (high + toward).min(max).max(high))
    } else if toward < 0.0 {
        ((low + toward).max(0.0).min(low), high)
    } else {
        window
    }
}

pub(crate) const MAX_MOVING_EXTENT_VIEWPORTS: f32 = 3.0;

#[derive(Debug)]
pub struct AnimationSlot {
    pub node: NodeId,
    pub(crate) curve: crate::visual::curves::CompositeCurve,
}

impl AnimationSlot {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitTarget {
    pub node: NodeId,
    pub scroll: Option<u32>,
}

pub struct CommittedFrame {
    pub(crate) order: PaintOrder,
    pub(crate) presentation: Presentation,
    pub(crate) animations_active: bool,
    pub(crate) needs_main_ticks: bool,
    pub(crate) earliest_expiry: Option<f64>,
    pub(crate) viewport: Size2D<f32>,
    pub(crate) device_pixel_ratio: f32,
    pub(crate) paints_content: bool,
    pub(crate) container_rects: Vec<crate::visual::container_timing::ContainerRect>,
}

pub(crate) struct Presentation {
    pub(crate) fragments: Vec<DisplayList>,
    pub(crate) program: Vec<ComposeOp>,
    pub(crate) image_draws: Vec<crate::paint::compose::ImageDraw>,
    pub(crate) vector_draws: Vec<crate::paint::compose::VectorDraw>,
    pub(crate) filter_groups: Vec<FilterGroup>,
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
    #[must_use]
    pub fn scene(&self) -> Option<&DisplayList> {
        matches!(
            self.presentation.program.as_slice(),
            [ComposeOp::Fragment {
                index: 0,
                space: None
            }]
        )
        .then(|| &self.presentation.fragments[0])
    }

    pub fn compose_into(
        &self,
        scene: &mut DisplayList,
        images: &[Option<ImageRef>],
        filtered: &[Option<Texture>],
        vectors: &[Option<Texture>],
        offset_of: &dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
        animation_now: Option<f64>,
    ) {
        self.compose_target_into(
            scene,
            self.device_viewport(),
            images,
            filtered,
            vectors,
            offset_of,
            animation_now,
        );
    }

    #[must_use]
    pub fn device_viewport(&self) -> (u32, u32) {
        let length = |css: f32| {
            let device = f64::from(css) * f64::from(self.device_pixel_ratio);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "clamped to the u32 range immediately before the cast"
            )]
            if device.is_finite() {
                device.ceil().clamp(0.0, f64::from(u32::MAX)) as u32
            } else {
                u32::MAX
            }
        };
        (length(self.viewport.width), length(self.viewport.height))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one composition's full inputs: the target, its size, the three pixel tables, \
                  and the two compose readings"
    )]
    pub fn compose_target_into(
        &self,
        scene: &mut DisplayList,
        target: (u32, u32),
        images: &[Option<ImageRef>],
        filtered: &[Option<Texture>],
        vectors: &[Option<Texture>],
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
        let samples = self.order.space_samples(
            &animations,
            &stickies,
            &anchored,
            self.device_pixel_ratio,
            offset_of,
        );
        compose::replay(
            scene,
            crate::kurbo::Rect::new(0.0, 0.0, f64::from(target.0), f64::from(target.1)),
            &self.presentation.fragments,
            &self.presentation.program,
            &self.presentation.image_draws,
            images,
            &self.presentation.vector_draws,
            vectors,
            &self.presentation.filter_groups,
            filtered,
            &samples,
            &samples.seam_passes_with(self.order.animations(), animation_now),
        );
    }

    #[must_use]
    pub fn filter_groups(&self) -> &[FilterGroup] {
        &self.presentation.filter_groups
    }

    pub(crate) fn vector_draws(&self) -> &[compose::VectorDraw] {
        &self.presentation.vector_draws
    }

    #[must_use]
    pub fn draws_vectors(&self) -> bool {
        !self.presentation.vector_draws.is_empty()
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one bake's full inputs: the entry, its scene, the three pixel tables, and the \
                  two compose readings"
    )]
    pub fn bake_filter(
        &self,
        index: usize,
        scene: &mut DisplayList,
        images: &[Option<ImageRef>],
        filtered: &[Option<Texture>],
        vectors: &[Option<Texture>],
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
                vector_draws: &self.presentation.vector_draws,
                vectors,
                filter_groups: groups,
                filtered,
                spaces: self.order.spaces(),
                samples: &animations,
            },
            group.ops.start as usize..group.ops.end as usize,
            &samples,
            &samples.seam_passes_with(self.order.animations(), animation_now),
            compose::ReplayTarget::Bake {
                entry: group.space,
                origin: (group.rect.x0, group.rect.y0),
                size: group.size(),
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
            crate::kurbo::Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
            Affine::IDENTITY,
        );
    }

    pub fn resolve_images<P>(
        &self,
        pixels: &P,
        images: &mut Vec<Option<ImageRef>>,
        sources: &mut Vec<PictureKey>,
    ) -> usize
    where
        P: crate::render::image::FrameImages<PlatformFrame = std::convert::Infallible> + ?Sized,
    {
        images.clear();
        let mut hints = Vec::new();
        let mut indices = Vec::new();
        self.image_sources(sources, &mut hints, &mut indices);
        let mut wanted = 0;
        let distinct: Vec<Option<ImageRef>> = sources
            .iter()
            .zip(&hints)
            .map(|(source, hint)| {
                let described = pixels.describe(source, *hint)?;
                let image = match source {
                    PictureKey::Url(_) => described.pixels,
                    PictureKey::Id(id) => pixels.take_frame(*id).map(|taken| taken.frame),
                }
                .map(PictureFrame::into_pixels);
                let Some(image) = image else {
                    pixels.want(source, *hint);
                    wanted += 1;
                    return None;
                };
                Some(ImageRef::held(&image))
            })
            .collect();
        images.extend(indices.into_iter().map(|index| distinct[index].clone()));
        wanted
    }

    pub(crate) fn image_sources(
        &self,
        sources: &mut Vec<PictureKey>,
        hints: &mut Vec<crate::render::image::ImageSizeHint>,
        indices: &mut Vec<usize>,
    ) {
        sources.clear();
        hints.clear();
        indices.clear();
        indices.reserve(self.presentation.image_draws.len());
        for draw in &self.presentation.image_draws {
            let seen = sources.iter().position(|source| source.same(&draw.image));
            let hint = draw.size_hint();
            let index = seen.unwrap_or_else(|| {
                sources.push(draw.image.clone());
                hints.push(hint);
                sources.len() - 1
            });
            hints[index] = hints[index].union(hint);
            indices.push(index);
        }
    }

    #[must_use]
    pub fn commit_id(&self) -> u64 {
        self.order.commit_id()
    }

    #[must_use]
    pub const fn paints_content(&self) -> bool {
        self.paints_content
    }

    pub(crate) fn container_rects(&self) -> &[crate::visual::container_timing::ContainerRect] {
        &self.container_rects
    }

    pub(crate) fn paints_under_containers(&self) -> bool {
        !self.container_rects.is_empty()
    }

    #[must_use]
    pub const fn animations_active(&self) -> bool {
        self.animations_active
    }

    #[must_use]
    pub const fn needs_main_ticks(&self) -> bool {
        self.needs_main_ticks
    }

    #[must_use]
    pub fn has_live_curves(&self) -> bool {
        self.presentation.composed.clock
    }

    pub(crate) fn composes_scroll_curves(&self) -> bool {
        self.presentation.composed.scroll
    }

    #[must_use]
    pub fn has_exported_curves(&self) -> bool {
        !self.order.animations().is_empty()
    }

    #[must_use]
    pub fn animation_boundary_passed(&self, now: f64) -> bool {
        self.earliest_expiry.is_some_and(|expiry| now >= expiry)
    }

    #[must_use]
    pub const fn viewport(&self) -> Size2D<f32> {
        self.viewport
    }

    #[must_use]
    pub const fn device_pixel_ratio(&self) -> f32 {
        self.device_pixel_ratio
    }

    #[must_use]
    pub fn snap_points(&self) -> &[SnapPoint] {
        self.order.snap_points()
    }

    #[must_use]
    pub fn snap_axes(&self, slot: &ScrollSlot) -> (Option<SnapAxis<'_>>, Option<SnapAxis<'_>>) {
        self.order.snap_axes(slot)
    }

    #[must_use]
    pub fn scroll_slots(&self) -> &[ScrollSlot] {
        self.order.slots()
    }

    #[must_use]
    pub fn auto_scroll_requests(&self) -> &[(NodeId, AutoScrollRequest)] {
        self.order.auto_scroll_requests()
    }

    #[must_use]
    pub fn slot_of(&self, node: NodeId) -> Option<u32> {
        self.order.slot_of(node)
    }

    #[must_use]
    pub fn covers_scroll_offset(&self, node: NodeId, offset: Vector2D<f32>) -> bool {
        self.slot_of(node).is_some_and(|index| {
            let slot = &self.order.slots()[index as usize];
            let (low, high) = slot.encode_window();
            let (period_x, period_y) = slot.wrap_period();
            let axis = |value: f32, low: f32, high: f32, period: Option<f32>| match period {
                Some(_) => value.is_finite(),
                None => value >= low && value <= high,
            };
            axis(offset.x, low.x, high.x, period_x) && axis(offset.y, low.y, high.y, period_y)
        })
    }

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
        let passes = samples.seam_passes_with(self.order.animations(), animation_now);
        if passes.is_empty() {
            return self.order.raw_hits_at(point, &samples).next();
        }
        let offsets: SmallVec<[_; 2]> = passes
            .iter()
            .map(|pass| samples.shifted_offsets(pass.slot, pass.shift))
            .collect();
        let seams: SmallVec<[(u32, SpaceSamples<'_>); 2]> = passes
            .iter()
            .zip(&offsets)
            .map(|(pass, offsets)| {
                (
                    pass.slot,
                    SpaceSamples {
                        offset_of: offsets,
                        animations: &pass.animations,
                        ..samples
                    },
                )
            })
            .collect();
        let spaces = self.order.spaces();
        self.order
            .hit_testable_items()
            .iter()
            .rev()
            .find_map(|item| {
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

    #[must_use]
    pub fn animation_slots(&self) -> &[AnimationSlot] {
        self.order.animations()
    }

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
    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::Document;
    use crate::tree::document::tests::device;

    fn scrolling_page() -> (
        Document<()>,
        crate::tree::document::NodeId,
        crate::tree::document::NodeId,
    ) {
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
            anchor_shift: Vector2D::zero(),
            reach: Vector2D::zero(),
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
    fn unwrap_reads_past_the_seams_midpoint_as_the_start() {
        let slot = circular_y_slot();
        for (y, expected) in [
            (420.0, 420.0),
            (450.0, 450.0),
            (480.0, -20.0),
            (200.0, 200.0),
            (980.0, -20.0),
            (-20.0, -20.0),
            (-80.0, 420.0),
        ] {
            assert_eq!(
                slot.unwrap(Vector2D::new(470.0, y)),
                Vector2D::new(470.0, expected),
                "{y}"
            );
        }
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

    fn tall_slot(offset_y: f32, reach_y: f32) -> ScrollSlot {
        ScrollSlot {
            circular: ScrollAxes::NONE,
            offset: Vector2D::new(0.0, offset_y),
            max_offset: Vector2D::new(0.0, 2000.0),
            reach: Vector2D::new(0.0, reach_y),
            ..circular_y_slot()
        }
    }

    #[test]
    fn a_zero_reach_relevance_window_is_the_encode_window() {
        let slot = tall_slot(500.0, 0.0);
        assert_eq!(
            slot.encode_window(),
            (Vector2D::new(0.0, 400.0), Vector2D::new(0.0, 600.0))
        );
        assert_eq!(slot.relevance_window(), slot.encode_window());
    }

    #[test]
    fn a_forward_reach_stretches_the_relevance_window_ahead_only() {
        let slot = tall_slot(500.0, 150.0);
        assert_eq!(
            slot.relevance_window(),
            (Vector2D::new(0.0, 400.0), Vector2D::new(0.0, 750.0))
        );
        assert_eq!(
            slot.encode_window(),
            (Vector2D::new(0.0, 400.0), Vector2D::new(0.0, 600.0)),
            "the encode window is the offset's alone"
        );
    }

    #[test]
    fn a_backward_reach_stretches_the_relevance_window_behind_only() {
        let slot = tall_slot(500.0, -150.0);
        assert_eq!(
            slot.relevance_window(),
            (Vector2D::new(0.0, 250.0), Vector2D::new(0.0, 600.0))
        );
        assert_eq!(
            tall_slot(500.0, -1000.0).relevance_window().0,
            Vector2D::new(0.0, 100.0),
            "capped at three scrollports behind"
        );
        assert_eq!(
            tall_slot(150.0, -300.0).relevance_window().0,
            Vector2D::zero(),
            "and never before the start"
        );
    }

    #[test]
    fn the_relevance_reach_is_capped_at_three_scrollports() {
        assert_eq!(
            tall_slot(500.0, 1000.0).relevance_window(),
            (Vector2D::new(0.0, 400.0), Vector2D::new(0.0, 900.0))
        );
    }

    #[test]
    fn the_relevance_window_never_passes_the_scroll_range() {
        let slot = tall_slot(1700.0, 300.0);
        assert_eq!(slot.encode_window().1, Vector2D::new(0.0, 1800.0));
        assert_eq!(slot.relevance_window().1, Vector2D::new(0.0, 2000.0));
    }

    #[test]
    fn a_pending_request_target_wins_over_the_reach() {
        let slot = ScrollSlot {
            request: Some(crate::scroll::ScrollRequest {
                serial: std::num::NonZeroU64::MIN,
                target: Vector2D::new(0.0, 300.0),
                behavior: crate::scroll::ScrollBehavior::Instant,
                yields_to_pointer: false,
            }),
            ..tall_slot(500.0, 150.0)
        };
        assert_eq!(
            slot.relevance_window(),
            (Vector2D::new(0.0, 200.0), Vector2D::new(0.0, 600.0)),
            "the request goes back 200 px; the forward reach is not applied"
        );
    }

    #[test]
    fn a_circular_axis_relevance_window_ignores_the_reach() {
        let slot = ScrollSlot {
            reach: Vector2D::new(0.0, 150.0),
            ..circular_y_slot()
        };
        assert_eq!(slot.relevance_window(), slot.encode_window());
        let slot = ScrollSlot {
            reach: Vector2D::new(50.0, 150.0),
            ..circular_y_slot()
        };
        assert_eq!(
            slot.relevance_window(),
            (Vector2D::new(100.0, 0.0), Vector2D::new(350.0, 400.0)),
            "the bounded x axis stretches, the circular y does not"
        );
    }

    #[test]
    fn a_circular_axis_covers_every_offset_and_a_bounded_one_keeps_its_window() {
        let (mut document, root, scroller) = scrolling_page();
        document.set_inline_style(scroller, "overscroll-behavior-y: circular");
        let frame = document.commit();
        let covers = |x: f32, y: f32| frame.covers_scroll_offset(scroller, Vector2D::new(x, y));
        assert!(covers(0.0, 900.0), "across the seam");
        assert!(covers(0.0, 999.0));
        assert!(covers(0.0, -300.0), "off the circle, normalized at compose");
        assert!(!covers(0.0, f32::NAN));
        assert!(!covers(10.0, 900.0), "x has nothing to scroll");
        assert!(
            !frame.covers_scroll_offset(root, Vector2D::zero()),
            "no slot"
        );

        let before = frame.commit_id();
        assert_eq!(
            document.scroll_to(scroller, Vector2D::new(0.0, 900.0)),
            Vector2D::new(0.0, 900.0)
        );
        assert!(
            !document.needs_render(),
            "an offset across the seam composes from the frame"
        );
        assert_eq!(document.commit().commit_id(), before);

        let (mut document, _, scroller) = scrolling_page();
        let frame = document.commit();
        assert!(frame.covers_scroll_offset(scroller, Vector2D::new(0.0, 200.0)));
        assert!(
            !frame.covers_scroll_offset(scroller, Vector2D::new(0.0, 300.0)),
            "a bounded axis covers its window alone"
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
        document.scroll_to(scroller, euclid::default::Vector2D::new(0.0, 120.0));
        assert!(
            !document.needs_render(),
            "a scroll the retained frame can compose invalidates nothing"
        );
        assert_eq!(
            document.commit().commit_id(),
            before.commit_id(),
            "no new frame is built for it"
        );

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

        let hit = frame
            .hit(Point2D::new(15.0, 15.0), &|_| None, None)
            .expect("escapee hit");
        assert_eq!(hit.node, escapee);
        assert_eq!(hit.scroll, None, "the escapee left both scrollers");
    }
}
