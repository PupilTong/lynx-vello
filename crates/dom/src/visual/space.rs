//! Compose spaces: the scroll, sticky and animation nodes a frame composes
//! through, as one tree in containing-block order.
//!
//! The frame is baked unscrolled, unstuck and at committed transforms. Each
//! node applies one CSS-px affine `X` built from committed geometry outside
//! it: a scroll node `Tr(−A·snap(o))` with `A` the scrollport's
//! [`ScrollSlot::viewport_axes`], a sticky node its own mapped shift, an
//! animation node its sampled delta `pre·L(t)·Lc⁻¹·pre⁻¹`. A record's live map
//! is the product of the `X`s on its path, root first — so a node's order on
//! the path is the order its movement applies in.
//!
//! Every record that moves at compose time names its innermost space: program
//! ops, image draws, filter entries, paint items and clips. An element's own
//! box, clip and effect layer take its *box space* — after its own sticky and
//! animation nodes, before its own scroll node — and its content takes its
//! *content space*, after its scroll node. [`SpaceSamples::css`] is the one
//! place the product is formed: compose, filter bakes and hit testing (which
//! inverts it) all read it.
//!
//! The slot tables keep their own parent links for their own relations:
//! [`ScrollSlot::parent`] is the recognition and chaining chain, and a sticky
//! slot's parent feeds its constraint solve.

use euclid::default::Vector2D;

use super::reach::Reach;
use super::{AnimationSamples, AnimationSlot, ClipNode, ScrollSlot, StickySamples};
use crate::paint::compose::snap_offset;
use crate::vello::kurbo::Affine;

/// One node of a frame's space tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Space {
    /// The next node outward, in the same table.
    pub(crate) parent: Option<u32>,
    pub(crate) kind: SpaceKind,
}

/// What a space node applies, by index into its slot table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpaceKind {
    /// A scroll container's content, by scroll slot.
    Scroll(u32),
    /// A sticky box, by sticky slot.
    Sticky(u32),
    /// An exported animation, by animation slot.
    Animation(u32),
}

/// The kinds on `space`'s path, innermost first.
pub(crate) fn path(spaces: &[Space], space: Option<u32>) -> impl Iterator<Item = SpaceKind> + '_ {
    std::iter::successors(space.map(|index| spaces[index as usize]), |node| {
        node.parent.map(|index| spaces[index as usize])
    })
    .map(|node| node.kind)
}

/// The deepest space on both `a`'s and `b`'s paths; `None` is the root.
///
/// A node's parent is always an earlier entry, so of two distinct spaces the
/// later is never the other's ancestor and steps outward.
pub(crate) fn common_ancestor(
    spaces: &[Space],
    mut a: Option<u32>,
    mut b: Option<u32>,
) -> Option<u32> {
    while a != b {
        // `None` orders first, so the stepping side is always a node.
        if a > b {
            a = a.and_then(|index| spaces[index as usize].parent);
        } else {
            b = b.and_then(|index| spaces[index as usize].parent);
        }
    }
    a
}

/// The nodes from `space` outward up to, not including, `outer`, an
/// ancestor of `space` or the root.
pub(crate) fn path_below(
    spaces: &[Space],
    space: Option<u32>,
    outer: Option<u32>,
) -> impl Iterator<Item = Space> + '_ {
    std::iter::successors(space, |&index| spaces[index as usize].parent)
        .take_while(move |&index| Some(index) != outer)
        .map(|index| spaces[index as usize])
}

/// The reach of every transform curve that moves `content` relative to
/// `group`: those on `content`'s path below the two spaces' common ancestor,
/// innermost first. An opacity-only curve moves nothing.
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
        SpaceKind::Scroll(_) | SpaceKind::Sticky(_) => None,
    })
}

/// Whether what a record in `record` draws into a bake in `bake` changes
/// with the timeline reading: a curve on `record`'s path below the two
/// spaces' common ancestor moves or fades it, and a transform curve on
/// `bake`'s moves the bake across it. An opacity-only curve on `bake`'s
/// side changes neither, since its alpha applies where the bake is drawn.
pub(crate) fn sampled_against(
    spaces: &[Space],
    animations: &[AnimationSlot],
    record: Option<u32>,
    bake: Option<u32>,
) -> bool {
    let common = common_ancestor(spaces, record, bake);
    path_below(spaces, record, common).any(|node| matches!(node.kind, SpaceKind::Animation(_)))
        || curves_within(spaces, animations, bake, record)
            .next()
            .is_some()
}

/// The innermost clip on `clip`'s chain that no transform curve moves
/// relative to `group`. A curve carrying content around inside a group
/// cannot carry it out of this clip.
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

/// Whether a group in `group` bounds content a transform curve moves inside
/// it under clip chain `clip`: the region of the chain's [`still_clip`] —
/// or, with none, the viewport — pulls back into `group` through no curve
/// whose scale range reaches 0.
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
        SpaceKind::Scroll(_) | SpaceKind::Sticky(_) => true,
    })
}

/// The innermost scroll slot on `space`'s path.
pub(crate) fn nearest_scroll(spaces: &[Space], space: Option<u32>) -> Option<u32> {
    path(spaces, space).find_map(|kind| match kind {
        SpaceKind::Scroll(slot) => Some(slot),
        _ => None,
    })
}

/// The innermost sticky slot on `space`'s path.
pub(crate) fn nearest_sticky(spaces: &[Space], space: Option<u32>) -> Option<u32> {
    path(spaces, space).find_map(|kind| match kind {
        SpaceKind::Sticky(slot) => Some(slot),
        _ => None,
    })
}

/// The innermost animation slot on `space`'s path.
pub(crate) fn nearest_animation(spaces: &[Space], space: Option<u32>) -> Option<u32> {
    path(spaces, space).find_map(|kind| match kind {
        SpaceKind::Animation(slot) => Some(slot),
        _ => None,
    })
}

/// One instant's node inputs with the tables they index: the scroll offsets
/// `offset_of` reports (falling back to the committed ones), the sampled
/// sticky shifts and animation deltas.
#[derive(Clone, Copy)]
pub(crate) struct SpaceSamples<'a> {
    pub(crate) spaces: &'a [Space],
    pub(crate) slots: &'a [ScrollSlot],
    pub(crate) animations: &'a AnimationSamples,
    pub(crate) stickies: &'a StickySamples,
    pub(crate) ratio: f32,
    pub(crate) offset_of: &'a dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>>,
}

impl SpaceSamples<'_> {
    /// `space`'s live map in viewport CSS px: its path's node transforms,
    /// root first.
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

    /// [`Self::css`] conjugated into device px: encoded content carries the
    /// device scale as its outermost factor, so the map applies inside one
    /// scale and outside the other.
    pub(crate) fn device(&self, space: Option<u32>) -> Affine {
        let css = self.css(space);
        let scale = f64::from(self.ratio);
        if scale.is_finite() && scale > 0.0 {
            Affine::scale(scale) * css * Affine::scale(1.0 / scale)
        } else {
            css
        }
    }

    /// Maps a space into a filter entry's bake target: device px with the
    /// entry's `rect` origin at `(0, 0)` and the entry's own map divided out
    /// on the *left*, because the composed frame draws the baked texture at
    /// `device(entry) · Tr(origin)`. So
    /// `device(entry) · Tr(origin) · bake_map(entry, origin)(space)` is
    /// `device(space)` for every space — whatever moves between the two, and
    /// not only when the two differ by a translation.
    pub(crate) fn bake_map(
        &self,
        entry: Option<u32>,
        origin: (f64, f64),
    ) -> impl Fn(Option<u32>) -> Affine + '_ {
        let out = Affine::translate((-origin.0, -origin.1)) * self.device(entry).inverse();
        let device = self.device_cached();
        move |space| out * device(space)
    }

    /// [`Self::device`] behind a one-entry cache: consecutive ops mostly
    /// share a space.
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
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use euclid::default::{Point2D, Rect, Size2D, Transform3D, Vector2D};

    use super::{Space, SpaceKind, SpaceSamples};
    use crate::scroll::{ScrollAxes, ScrollCapture};
    use crate::tree::document::DOCUMENT_ELEMENT_NODE_ID;
    use crate::vello::kurbo::{Affine, Point};
    use crate::visual::{
        AnimationSample, AnimationSamples, ClipNode, CornerRadii, PaintItem, PaintItemKind,
        PaintOrder, ScrollSlot, SlotSamples, SnapSlot, StickySample, StickySamples,
    };

    /// Scroller `R` at the root, an animated element `A` inside it, and a
    /// scroller `S` inside `A`: spaces 0, 1 and 2.
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
            capture: ScrollCapture::default(),
            snap: SnapSlot::default(),
            offset: Vector2D::new(0.0, offset_y),
            max_offset: Vector2D::new(0.0, 1000.0),
            scrollport: Size2D::new(100.0, 100.0),
            viewport_axes: [Vector2D::new(1.0, 0.0), Vector2D::new(0.0, 1.0)],
        }
    }

    /// A quarter turn about `(50, 50)`: a delta that commutes with no
    /// translation.
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

    /// A whole-frame sample table: slot `i` is `samples[i]`.
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
            ratio: 1.0,
            offset_of: &|_| None,
        }
    }

    /// Two branches under a scroller: an animated element holding a
    /// scroller (spaces 1 and 3), and a sticky box (space 2).
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

    /// A filter entry in the outer scroller's content, ops in the animated
    /// element's space: the rotation and the scroll translation do not
    /// commute, so the side the entry's own map is divided out on is
    /// observable.
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
        // What compose draws the baked texture at.
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

    /// A frame over [`TREE`] with one clip in `A`'s box space and one item
    /// per case in `S`'s content space.
    fn frame(items: &[(f32, Option<usize>)]) -> PaintOrder {
        let mut order = PaintOrder::empty();
        order.spaces.extend_from_slice(&TREE);
        order.slots.extend([slot(None, 30.0), slot(Some(0), 20.0)]);
        order.clips.push(ClipNode {
            parent: None,
            node: DOCUMENT_ELEMENT_NODE_ID,
            transform: Transform3D::identity(),
            rect: Rect::new(Point2D::new(0.0, 0.0), Size2D::new(100.0, 100.0)),
            radii: CornerRadii::ZERO,
            space: Some(1),
        });
        order.items.extend(items.iter().map(|&(y, clip)| PaintItem {
            node: DOCUMENT_ELEMENT_NODE_ID,
            kind: PaintItemKind::ElementBox,
            transform: Transform3D::translation(0.0, y, 0.0),
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
        let samples = order.space_samples(&animations, &stickies, 1.0, &|_| None);
        let [inside, clipped, unclipped] = [0, 1, 2].map(|index| &order.items[index]);

        // Baked at (5, 55); `S` scrolls it to (5, 35) in `A`'s box, inside
        // the clip.
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

        // Baked at (5, 5); `S` scrolls it to (5, −15) in `A`'s box, above
        // the clip, which moves with `A` and not with `S`.
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
        let samples = order.space_samples(&collapsed, &stickies, 1.0, &|_| None);
        assert_eq!(
            order.item_hit(unclipped, Point2D::new(0.0, 0.0), &samples),
            None,
            "a degenerate space hits nothing",
        );
    }
}
