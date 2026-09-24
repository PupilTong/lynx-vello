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

use super::{AnimationSample, ScrollSlot, StickySample};
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
    pub(crate) animations: &'a [AnimationSample],
    pub(crate) stickies: &'a [StickySample],
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
                let shift = self.stickies[slot as usize].mapped;
                Affine::translate((f64::from(shift.x), f64::from(shift.y)))
            }
            SpaceKind::Animation(slot) => self.animations[slot as usize].delta,
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
        AnimationSample, ClipNode, CornerRadii, PaintItem, PaintItemKind, PaintOrder, ScrollSlot,
        SnapSlot, StickySample,
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

    fn samples<'a>(
        slots: &'a [ScrollSlot],
        animations: &'a [AnimationSample],
        stickies: &'a [StickySample],
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

    #[test]
    fn a_space_map_is_its_path_root_first() {
        let slots = [slot(None, 30.0), slot(Some(0), 20.0)];
        let animations = [AnimationSample {
            delta: quarter_turn(),
            alpha: None,
        }];
        let samples = samples(&slots, &animations, &[]);
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
        let animations = [AnimationSample {
            delta: quarter_turn(),
            alpha: None,
        }];
        let samples = samples(&slots, &animations, &[]);
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
        let samples = SpaceSamples {
            spaces: &spaces,
            ..samples(&[], &[], &stickies)
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
        let animations = [AnimationSample {
            delta: quarter_turn(),
            alpha: None,
        }];
        let samples = order.space_samples(&animations, &[], 1.0, &|_| None);
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

        let collapsed = [AnimationSample {
            delta: Affine::scale(0.0),
            alpha: None,
        }];
        let samples = order.space_samples(&collapsed, &[], 1.0, &|_| None);
        assert_eq!(
            order.item_hit(unclipped, Point2D::new(0.0, 0.0), &samples),
            None,
            "a degenerate space hits nothing",
        );
    }
}
