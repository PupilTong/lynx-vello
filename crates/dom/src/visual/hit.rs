//! Hit testing: reverse paint order over the built [`PaintOrder`].
//!
//! Every query here is a pure read of the frame's self-contained geometry
//! snapshot — no style flush, no layout, no rebuild. Freshness policy lives
//! with the callers: painting demands [`PaintOrder::assert_visually_fresh`],
//! while the document-level hit queries answer from whatever frame is
//! retained and simply skip items whose node is no longer live, because
//! "input arrived between a removal and its repaint" is a normal transient
//! state, not a bug.
//!
//! Skipping per item is enough because a [`NodeId`] is never reissued: an id
//! the frame still names either resolves to the node it was drawn for, or to
//! nothing at all. It can never resolve to a stranger, so a removal anywhere
//! in the tree no longer has to blank every query in it.

use euclid::default::{Point2D, Rect, Vector2D};

use super::{PaintItem, PaintItemKind, PaintOrder, ScrollSlot, SpaceSamples, geometry};
use crate::NodeId;
use crate::tree::document::Document;
use crate::vello::kurbo::{Affine, Point};

/// Where a hit query's scroll offsets come from: `None` falls back to the
/// slot's committed offset. The frame is baked unscrolled, so a query
/// carries the point *into* each item's space before inverting its
/// transform — the inverse of the map composition applies, snapped the same
/// way.
pub(crate) type OffsetSource<'a> = dyn Fn(&ScrollSlot) -> Option<Vector2D<f32>> + 'a;

impl PaintOrder {
    pub(crate) fn assert_visually_fresh<T>(&self, document: &Document<T>) {
        assert!(
            self.commit_id == document.commit_id() && !document.visual_dirty(),
            "visually stale PaintOrder: the document mutated after this frame was built; \
             rebuild it through the document visual pipeline before painting",
        );
    }

    #[must_use]
    pub(crate) fn elements_at<T>(
        &self,
        document: &Document<T>,
        point: Point2D<f32>,
        offsets: &OffsetSource<'_>,
        ratio: f32,
    ) -> Vec<NodeId> {
        let animations = self.sample_animations(None, offsets);
        let stickies = self.sample_stickies(ratio, offsets);
        let samples = self.space_samples(&animations, &stickies, ratio, offsets);
        let mut elements = Vec::new();
        for node in self.hits_at(document, point, &samples) {
            if !elements.contains(&node) {
                elements.push(node);
            }
        }
        elements
    }

    #[must_use]
    pub(crate) fn first_element_at<T>(
        &self,
        document: &Document<T>,
        point: Point2D<f32>,
        offsets: &OffsetSource<'_>,
        ratio: f32,
    ) -> Option<NodeId> {
        let animations = self.sample_animations(None, offsets);
        let stickies = self.sample_stickies(ratio, offsets);
        let samples = self.space_samples(&animations, &stickies, ratio, offsets);
        self.hits_at(document, point, &samples).next()
    }

    /// Items whose node died since the frame was built are skipped, not
    /// answered as their id: the frame's geometry for them is gone, and the
    /// id names nothing a caller could do anything with.
    fn hits_at<'frame, T>(
        &'frame self,
        document: &'frame Document<T>,
        point: Point2D<f32>,
        samples: &'frame SpaceSamples<'frame>,
    ) -> impl Iterator<Item = NodeId> + 'frame {
        self.items
            .iter()
            .rev()
            .filter_map(move |item| self.item_hit(item, point, samples))
            .filter(move |&node| document.contains_node(node))
    }

    pub(super) fn item_hit(
        &self,
        item: &PaintItem,
        point: Point2D<f32>,
        samples: &SpaceSamples<'_>,
    ) -> Option<NodeId> {
        if !item.hit_testable {
            return None;
        }
        // The frame is baked unscrolled: carry the screen point back through
        // the item's space before inverting its own transform. A degenerate
        // space paints the item collapsed; nothing to hit.
        let unmoved = unmap(samples.css(item.space), point)?;
        let local = item.transform.inverse()?.transform_point2d(unmoved)?;
        if local.x >= item.size.width || local.y >= item.size.height {
            return None;
        }
        if !geometry::rounded_rect_contains(Rect::from_size(item.size), &item.radii, local) {
            return None;
        }
        if !self.point_passes_clips(item.clip, point, samples) {
            return None;
        }
        Some(match item.kind {
            PaintItemKind::ElementBox => item.node,
            PaintItemKind::TextRun { element } => element,
        })
    }

    fn point_passes_clips(
        &self,
        mut clip: Option<usize>,
        point: Point2D<f32>,
        samples: &SpaceSamples<'_>,
    ) -> bool {
        while let Some(index) = clip {
            let node = &self.clips[index];
            let Some(local) = unmap(samples.css(node.space), point).and_then(|unmoved| {
                node.transform
                    .inverse()
                    .and_then(|inverse| inverse.transform_point2d(unmoved))
            }) else {
                return false;
            };
            if !geometry::rounded_rect_contains(node.rect, &node.radii, local) {
                return false;
            }
            clip = node.parent;
        }
        true
    }
}

/// `point` carried back through a space's live map, or `None` when the map
/// is degenerate.
fn unmap(map: Affine, point: Point2D<f32>) -> Option<Point2D<f32>> {
    if map.determinant().abs() < f64::EPSILON {
        return None;
    }
    let unmoved = map.inverse() * Point::new(f64::from(point.x), f64::from(point.y));
    #[allow(clippy::cast_possible_truncation, reason = "CSS px fit f32")]
    Some(Point2D::new(unmoved.x as f32, unmoved.y as f32))
}
