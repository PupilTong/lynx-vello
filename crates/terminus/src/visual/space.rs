//! Compose spaces: the scroll, sticky, anchored and animation nodes a frame
//! composes through, as one tree in containing-block order.
//!
//! The frame is baked unscrolled, unstuck and at committed transforms. Each
//! node applies one CSS-px affine `X` built from committed geometry outside
//! it: a scroll node `Tr(−A·snap(o))` with `A` the scrollport's
//! [`ScrollSlot::viewport_axes`], a sticky node its own mapped shift, an
//! anchored node its box's default scroll shift `Tr(L·s)` — or the zero map
//! while `position-visibility` hides the box ([`super::anchored`]) — a
//! visibility node the identity or that same zero map, an
//! animation node its sampled delta `pre·L(t)·Lc⁻¹·pre⁻¹`. A record's live map
//! is the product of the `X`s on its path, root first — so a node's order on
//! the path is the order its movement applies in.
//!
//! Every record that moves at compose time names its innermost space: program
//! ops, image draws, filter entries, paint items and clips. An element's own
//! box, clip and effect layer take its *box space* — after its own sticky and
//! animation nodes, before its own scroll node — and its content takes its
//! *content space*, after its scroll node. An anchor-positioned box's
//! anchored node is the outermost of its own: it encloses its box space. Its
//! descendants that escape its containing-block chain (a `position: fixed`
//! box whose containing block is outside it) compose through its visibility
//! node instead, which hides them with it and does not shift them. [`SpaceSamples::css`] is
//! the one place the product is formed: compose, filter bakes and hit testing (which
//! inverts it) all read it.
//!
//! The slot tables keep their own parent links for their own relations:
//! [`ScrollSlot::parent`] is the recognition and chaining chain, and a sticky
//! slot's parent feeds its constraint solve.

use euclid::default::Vector2D;
use smallvec::SmallVec;
use stylo::properties::animated_properties::AnimationValueMap;

use super::anchored::{AnchoredSamples, AnchoredSlot};
use super::reach::Reach;
use super::{AnimationSamples, AnimationSlot, ClipNode, ScrollSlot, StickySamples};
use crate::kurbo::Affine;
use crate::paint::compose::snap_offset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Space {
    pub(crate) parent: Option<u32>,
    pub(crate) kind: SpaceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpaceKind {
    Scroll(u32),
    Sticky(u32),
    Animation(u32),
    Anchored(u32),
    AnchoredVisibility(u32),
}

pub(crate) fn path(spaces: &[Space], space: Option<u32>) -> impl Iterator<Item = SpaceKind> + '_ {
    std::iter::successors(space.map(|index| spaces[index as usize]), |node| {
        node.parent.map(|index| spaces[index as usize])
    })
    .map(|node| node.kind)
}

pub(crate) fn common_ancestor(
    spaces: &[Space],
    mut a: Option<u32>,
    mut b: Option<u32>,
) -> Option<u32> {
    while a != b {
        if a > b {
            a = a.and_then(|index| spaces[index as usize].parent);
        } else {
            b = b.and_then(|index| spaces[index as usize].parent);
        }
    }
    a
}

pub(crate) fn path_below(
    spaces: &[Space],
    space: Option<u32>,
    outer: Option<u32>,
) -> impl Iterator<Item = Space> + '_ {
    std::iter::successors(space, |&index| spaces[index as usize].parent)
        .take_while(move |&index| Some(index) != outer)
        .map(|index| spaces[index as usize])
}

pub(crate) fn curves_within<'a>(
    spaces: &'a [Space],
    animations: &'a [AnimationSlot],
    content: Option<u32>,
    group: Option<u32>,
) -> impl Iterator<Item = &'a Reach> + 'a {
    let common = common_ancestor(spaces, content, group);
    path_below(spaces, content, common).filter_map(move |node| match node.kind {
        SpaceKind::Animation(slot) => animations[slot as usize]
            .curve
            .transform
            .as_ref()
            .map(|track| &track.reach),
        SpaceKind::Scroll(_)
        | SpaceKind::Sticky(_)
        | SpaceKind::Anchored(_)
        | SpaceKind::AnchoredVisibility(_) => None,
    })
}

pub(crate) fn sampled_against(
    spaces: &[Space],
    animations: &[AnimationSlot],
    anchored: &[AnchoredSlot],
    record: Option<u32>,
    bake: Option<u32>,
) -> bool {
    let common = common_ancestor(spaces, record, bake);
    path_below(spaces, record, common).any(|node| match node.kind {
        SpaceKind::Animation(_) => true,
        SpaceKind::Anchored(slot) | SpaceKind::AnchoredVisibility(slot) => {
            anchored[slot as usize].reads_timeline()
        }
        SpaceKind::Scroll(_) | SpaceKind::Sticky(_) => false,
    }) || curves_within(spaces, animations, bake, record)
        .next()
        .is_some()
}

pub(crate) fn still_clip(
    spaces: &[Space],
    clips: &[ClipNode],
    animations: &[AnimationSlot],
    clip: Option<usize>,
    group: Option<u32>,
) -> Option<usize> {
    std::iter::successors(clip, |&index| clips[index].parent).find(|&index| {
        curves_within(spaces, animations, clips[index].space, group)
            .next()
            .is_none()
    })
}

pub(crate) fn movers_bounded(
    spaces: &[Space],
    clips: &[ClipNode],
    animations: &[AnimationSlot],
    clip: Option<usize>,
    group: Option<u32>,
) -> bool {
    let outer = still_clip(spaces, clips, animations, clip, group)
        .and_then(|index| common_ancestor(spaces, clips[index].space, group));
    path_below(spaces, group, outer).all(|node| match node.kind {
        SpaceKind::Animation(slot) => animations[slot as usize]
            .curve
            .transform
            .as_ref()
            .is_none_or(|track| track.reach.inverse_norm().is_some()),
        SpaceKind::Scroll(_)
        | SpaceKind::Sticky(_)
        | SpaceKind::Anchored(_)
        | SpaceKind::AnchoredVisibility(_) => true,
    })
}

pub(crate) fn linear_range(
    m: [Vector2D<f32>; 2],
    low: Vector2D<f32>,
    high: Vector2D<f32>,
) -> (Vector2D<f32>, Vector2D<f32>) {
    let (x0, x1) = (m[0] * low.x, m[0] * high.x);
    let (y0, y1) = (m[1] * low.y, m[1] * high.y);
    (
        Vector2D::new(
            x0.x.min(x1.x) + y0.x.min(y1.x),
            x0.y.min(x1.y) + y0.y.min(y1.y),
        ),
        Vector2D::new(
            x0.x.max(x1.x) + y0.x.max(y1.x),
            x0.y.max(x1.y) + y0.y.max(y1.y),
        ),
    )
}

pub(crate) fn rides(spaces: &[Space], space: Option<u32>, slot: u32) -> bool {
    path(spaces, space).any(|kind| kind == SpaceKind::Scroll(slot))
}

pub(crate) fn nearest_scroll(spaces: &[Space], space: Option<u32>) -> Option<u32> {
    path(spaces, space).find_map(|kind| match kind {
        SpaceKind::Scroll(slot) => Some(slot),
        _ => None,
    })
}

pub(crate) fn nearest_sticky(spaces: &[Space], space: Option<u32>) -> Option<u32> {
    path(spaces, space).find_map(|kind| match kind {
        SpaceKind::Sticky(slot) => Some(slot),
        _ => None,
    })
}

pub(crate) fn nearest_anchored(spaces: &[Space], space: Option<u32>) -> Option<u32> {
    std::iter::successors(space, |&index| spaces[index as usize].parent).find(|&index| {
        matches!(
            spaces[index as usize].kind,
            SpaceKind::Anchored(_) | SpaceKind::AnchoredVisibility(_)
        )
    })
}

pub(crate) fn nearest_animation(spaces: &[Space], space: Option<u32>) -> Option<u32> {
    path(spaces, space).find_map(|kind| match kind {
        SpaceKind::Animation(slot) => Some(slot),
        _ => None,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct SeamPass {
    pub(crate) slot: u32,
    pub(crate) shift: Vector2D<f32>,
    pub(crate) animations: AnimationSamples,
}

#[derive(Clone, Copy)]
pub(crate) struct SpaceSamples<'a> {
    pub(crate) spaces: &'a [Space],
    pub(crate) slots: &'a [ScrollSlot],
    pub(crate) animations: &'a AnimationSamples,
    pub(crate) stickies: &'a StickySamples,
    pub(crate) anchored: &'a AnchoredSamples,
    pub(crate) ratio: f32,
    pub(crate) offset_of: &'a dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
}

impl<'a> SpaceSamples<'a> {
    pub(crate) fn seam_passes(&self) -> SmallVec<[(u32, Vector2D<f32>); 2]> {
        let mut passes = SmallVec::new();
        for (slot, index) in self.slots.iter().zip(0_u32..) {
            let (period_x, period_y) = slot.wrap_period();
            if period_x.is_none() && period_y.is_none() {
                continue;
            }
            let offset = (self.offset_of)(slot).unwrap_or(slot.offset);
            let x = period_x.filter(|_| offset.x > slot.max_offset.x);
            let y = period_y.filter(|_| offset.y > slot.max_offset.y);
            if let Some(x) = x {
                passes.push((index, Vector2D::new(-x, 0.0)));
            }
            if let Some(y) = y {
                passes.push((index, Vector2D::new(0.0, -y)));
            }
            if let (Some(x), Some(y)) = (x, y) {
                passes.push((index, Vector2D::new(-x, -y)));
            }
        }
        passes
    }

    pub(crate) fn seam_passes_with(
        &self,
        curves: &[AnimationSlot],
        now: Option<f64>,
    ) -> SmallVec<[SeamPass; 2]> {
        let mut values = AnimationValueMap::default();
        self.seam_passes()
            .into_iter()
            .map(|(slot, shift)| {
                let mut animations = self.animations.clone();
                let shifted = self.shifted_offsets(slot, shift);
                let offsets = super::ScrollOffsets {
                    slots: self.slots,
                    offset_of: &shifted,
                };
                for (index, sample) in animations.entries_mut() {
                    let curve = &curves[index as usize];
                    if curve.curve.views(slot) {
                        *sample = curve.sample(now, &offsets, &mut values);
                    }
                }
                SeamPass {
                    slot,
                    shift,
                    animations,
                }
            })
            .collect()
    }

    pub(crate) fn shifted_offsets(
        &self,
        slot: u32,
        shift: Vector2D<f32>,
    ) -> impl Fn(&ScrollSlot) -> Option<Vector2D<f32>> + 'a {
        let offset_of = self.offset_of;
        let node = self.slots[slot as usize].node;
        move |each: &ScrollSlot| {
            let offset = offset_of(each).unwrap_or(each.offset);
            Some(if each.node == node {
                offset + shift
            } else {
                offset
            })
        }
    }

    pub(crate) fn css(&self, space: Option<u32>) -> Affine {
        let mut product = Affine::IDENTITY;
        let mut current = space;
        while let Some(index) = current {
            let node = self.spaces[index as usize];
            product = self.node(node.kind) * product;
            current = node.parent;
        }
        product
    }

    pub(crate) fn device(&self, space: Option<u32>) -> Affine {
        let css = self.css(space);
        let scale = f64::from(self.ratio);
        if scale.is_finite() && scale > 0.0 {
            Affine::scale(scale) * css * Affine::scale(1.0 / scale)
        } else {
            css
        }
    }

    pub(crate) fn bake_map(
        &self,
        entry: Option<u32>,
        origin: (f64, f64),
    ) -> impl Fn(Option<u32>) -> Affine + '_ {
        let out = Affine::translate((-origin.0, -origin.1)) * self.device(entry).inverse();
        let device = self.device_cached();
        move |space| out * device(space)
    }

    pub(crate) fn device_cached(&self) -> impl Fn(Option<u32>) -> Affine + '_ {
        let last = std::cell::Cell::new(None);
        move |space| match last.get() {
            Some((cached, affine)) if cached == space => affine,
            _ => {
                let affine = self.device(space);
                last.set(Some((space, affine)));
                affine
            }
        }
    }

    fn node(&self, kind: SpaceKind) -> Affine {
        match kind {
            SpaceKind::Scroll(slot) => {
                let slot = &self.slots[slot as usize];
                let offset = (self.offset_of)(slot).unwrap_or(slot.offset);
                let shift = slot.viewport_translation(snap_offset(offset, self.ratio));
                Affine::translate((-f64::from(shift.x), -f64::from(shift.y)))
            }
            SpaceKind::Sticky(slot) => {
                let shift = self.stickies.get(slot).mapped;
                Affine::translate((f64::from(shift.x), f64::from(shift.y)))
            }
            SpaceKind::Animation(slot) => self.animations.get(slot).delta,
            SpaceKind::Anchored(slot) => self.anchored[slot as usize].affine(),
            SpaceKind::AnchoredVisibility(slot) => self.anchored[slot as usize].visibility_affine(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use euclid::default::{Point2D, Rect, Size2D, Vector2D};

    use super::{Space, SpaceKind, SpaceSamples};
    use crate::kurbo::{Affine, Point};
    use crate::scroll::{CaptureAxes, ScrollAxes};
    use crate::tree::document::DOCUMENT_ELEMENT_NODE_ID;
    use crate::visual::{
        AnimationSample, AnimationSamples, ClipNode, CornerRadii, ItemTransform, PaintItem,
        PaintItemKind, PaintOrder, ScrollSlot, SlotSamples, SnapSlot, StickySample, StickySamples,
    };

    const TREE: [Space; 3] = [
        Space {
            parent: None,
            kind: SpaceKind::Scroll(0),
        },
        Space {
            parent: Some(0),
            kind: SpaceKind::Animation(0),
        },
        Space {
            parent: Some(1),
            kind: SpaceKind::Scroll(1),
        },
    ];

    fn slot(parent: Option<u32>, offset_y: f32) -> ScrollSlot {
        ScrollSlot {
            node: DOCUMENT_ELEMENT_NODE_ID,
            parent,
            user_scrollable: ScrollAxes::default(),
            chains: ScrollAxes::default(),
            bounce: ScrollAxes::default(),
            circular: ScrollAxes::default(),
            capture: CaptureAxes::default(),
            snap: SnapSlot::default(),
            offset: Vector2D::new(0.0, offset_y),
            max_offset: Vector2D::new(0.0, 1000.0),
            scrollport: Size2D::new(100.0, 100.0),
            viewport_axes: [Vector2D::new(1.0, 0.0), Vector2D::new(0.0, 1.0)],
            request: None,
            anchor_shift: Vector2D::zero(),
            reach: Vector2D::zero(),
        }
    }

    fn quarter_turn() -> Affine {
        Affine::translate((50.0, 50.0))
            * Affine::rotate(std::f64::consts::FRAC_PI_2)
            * Affine::translate((-50.0, -50.0))
    }

    fn close(a: Affine, b: Affine) -> bool {
        a.as_coeffs()
            .iter()
            .zip(b.as_coeffs())
            .all(|(a, b)| (a - b).abs() < 1e-9)
    }

    fn table<S>(samples: impl IntoIterator<Item = S>) -> SlotSamples<S> {
        (0..).zip(samples).collect()
    }

    fn samples<'a>(
        slots: &'a [ScrollSlot],
        animations: &'a AnimationSamples,
        stickies: &'a StickySamples,
    ) -> SpaceSamples<'a> {
        SpaceSamples {
            spaces: &TREE,
            slots,
            animations,
            stickies,
            anchored: &NO_ANCHORED,
            ratio: 1.0,
            offset_of: &|_| None,
        }
    }

    static NO_ANCHORED: crate::visual::anchored::AnchoredSamples = smallvec::SmallVec::new_const();

    #[test]
    fn two_spaces_meet_at_their_deepest_shared_node() {
        let spaces = [
            Space {
                parent: None,
                kind: SpaceKind::Scroll(0),
            },
            Space {
                parent: Some(0),
                kind: SpaceKind::Animation(0),
            },
            Space {
                parent: Some(0),
                kind: SpaceKind::Sticky(0),
            },
            Space {
                parent: Some(1),
                kind: SpaceKind::Scroll(1),
            },
        ];
        let common = |a, b| super::common_ancestor(&spaces, a, b);
        assert_eq!(common(Some(3), Some(2)), Some(0));
        assert_eq!(common(Some(2), Some(3)), Some(0));
        assert_eq!(common(Some(3), Some(1)), Some(1));
        assert_eq!(common(Some(3), None), None);
        assert_eq!(common(Some(3), Some(3)), Some(3));
        let below: Vec<_> = super::path_below(&spaces, Some(3), Some(0))
            .map(|node| node.kind)
            .collect();
        assert_eq!(below, [SpaceKind::Scroll(1), SpaceKind::Animation(0)]);
        assert_eq!(super::path_below(&spaces, Some(3), None).count(), 3);
    }

    #[test]
    fn a_space_map_is_its_path_root_first() {
        let slots = [slot(None, 30.0), slot(Some(0), 20.0)];
        let animations = table([AnimationSample {
            delta: quarter_turn(),
            alpha: None,
        }]);
        let stickies = StickySamples::default();
        let samples = samples(&slots, &animations, &stickies);
        let outer = Affine::translate((0.0, -30.0));
        let inner = Affine::translate((0.0, -20.0));

        assert!(close(samples.css(None), Affine::IDENTITY));
        assert!(close(samples.css(Some(0)), outer));
        assert!(close(samples.css(Some(1)), outer * quarter_turn()));
        assert!(
            close(samples.css(Some(2)), outer * quarter_turn() * inner),
            "the inner scroll applies inside the delta: X_R · D · X_S",
        );
        assert!(
            !close(samples.css(Some(2)), outer * inner * quarter_turn()),
            "which is not the translations-outside order",
        );
    }

    #[test]
    fn a_bake_divides_the_entrys_own_map_out_on_the_left() {
        let slots = [slot(None, 30.0), slot(Some(0), 20.0)];
        let animations = table([AnimationSample {
            delta: quarter_turn(),
            alpha: None,
        }]);
        let stickies = StickySamples::default();
        let samples = samples(&slots, &animations, &stickies);
        let origin = (7.0, 11.0);
        let entry = Some(0);
        let bake = samples.bake_map(entry, origin);
        let placement = samples.device(entry) * Affine::translate(origin);

        for space in [None, entry, Some(1), Some(2)] {
            assert!(
                close(placement * bake(space), samples.device(space)),
                "a baked op must compose where the unbaked frame put it",
            );
        }
        let inverse_on_the_right = Affine::translate((-origin.0, -origin.1))
            * samples.device(Some(1))
            * samples.device(entry).inverse();
        assert!(
            !close(bake(Some(1)), inverse_on_the_right),
            "the other side is a different map once a node rotates",
        );
    }

    #[test]
    fn a_sticky_node_applies_only_its_own_shift() {
        let spaces = [
            Space {
                parent: None,
                kind: SpaceKind::Sticky(0),
            },
            Space {
                parent: Some(0),
                kind: SpaceKind::Sticky(1),
            },
        ];
        let mut stickies = [StickySample::default(); 2];
        stickies[0].mapped = Vector2D::new(0.0, 5.0);
        stickies[1].mapped = Vector2D::new(3.0, 7.0);
        let stickies = table(stickies);
        let animations = AnimationSamples::default();
        let samples = SpaceSamples {
            spaces: &spaces,
            ..samples(&[], &animations, &stickies)
        };
        assert!(close(samples.css(Some(1)), Affine::translate((3.0, 12.0))));
    }

    fn frame(items: &[(f32, Option<usize>)]) -> PaintOrder {
        let mut order = PaintOrder::empty();
        order.spaces.extend_from_slice(&TREE);
        order.slots.extend([slot(None, 30.0), slot(Some(0), 20.0)]);
        order.clips.push(ClipNode {
            parent: None,
            node: DOCUMENT_ELEMENT_NODE_ID,
            transform: ItemTransform::Translate(Vector2D::zero()),
            rect: Rect::new(Point2D::new(0.0, 0.0), Size2D::new(100.0, 100.0)),
            radii: CornerRadii::ZERO,
            space: Some(1),
        });
        order.items.extend(items.iter().map(|&(y, clip)| PaintItem {
            node: DOCUMENT_ELEMENT_NODE_ID,
            kind: PaintItemKind::ElementBox,
            transform: ItemTransform::Translate(Vector2D::new(0.0, y)),
            clip,
            size: Size2D::new(10.0, 10.0),
            radii: CornerRadii::ZERO,
            hit_testable: true,
            slot: Some(1),
            space: Some(2),
        }));
        order
    }

    fn on_screen(samples: &SpaceSamples<'_>, x: f64, y: f64) -> Point2D<f32> {
        let point = samples.css(Some(2)) * Point::new(x, y);
        #[allow(clippy::cast_possible_truncation, reason = "CSS px fit f32")]
        Point2D::new(point.x as f32, point.y as f32)
    }

    #[test]
    fn hit_testing_inverts_the_space_map_for_items_and_clips() {
        let order = frame(&[(50.0, Some(0)), (0.0, Some(0)), (0.0, None)]);
        let animations = table([AnimationSample {
            delta: quarter_turn(),
            alpha: None,
        }]);
        let stickies = StickySamples::default();
        let samples = order.space_samples(&animations, &stickies, &NO_ANCHORED, 1.0, &|_| None);
        let [inside, clipped, unclipped] = [0, 1, 2].map(|index| &order.items[index]);

        let point = on_screen(&samples, 5.0, 55.0);
        assert_eq!(
            order.item_hit(inside, point, &samples),
            Some(DOCUMENT_ELEMENT_NODE_ID)
        );
        let translations_outside = Affine::translate((0.0, -50.0)) * quarter_turn();
        let wrong = translations_outside * Point::new(5.0, 55.0);
        #[allow(clippy::cast_possible_truncation, reason = "CSS px fit f32")]
        let wrong = Point2D::new(wrong.x as f32, wrong.y as f32);
        assert_eq!(
            order.item_hit(inside, wrong, &samples),
            None,
            "the translations-outside order is not where the item is",
        );

        let point = on_screen(&samples, 5.0, 5.0);
        assert_eq!(order.item_hit(clipped, point, &samples), None);
        assert_eq!(
            order.item_hit(unclipped, point, &samples),
            Some(DOCUMENT_ELEMENT_NODE_ID),
            "only the clip rejects it",
        );

        let collapsed = table([AnimationSample {
            delta: Affine::scale(0.0),
            alpha: None,
        }]);
        let samples = order.space_samples(&collapsed, &stickies, &NO_ANCHORED, 1.0, &|_| None);
        assert_eq!(
            order.item_hit(unclipped, Point2D::new(0.0, 0.0), &samples),
            None,
            "a degenerate space hits nothing",
        );
    }
}
