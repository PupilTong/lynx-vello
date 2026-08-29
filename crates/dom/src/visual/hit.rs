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

use euclid::default::{Point2D, Rect};

use super::{PaintItem, PaintItemKind, PaintOrder, geometry};
use crate::NodeId;
use crate::tree::document::Document;

impl PaintOrder {
    pub(crate) fn assert_visually_fresh<T>(&self, document: &Document<T>) {
        assert_eq!(
            self.visual_epoch,
            document.visual_epoch(),
            "visually stale PaintOrder: the document mutated after this frame was built; \
             rebuild it through the document visual pipeline before painting",
        );
    }

    #[must_use]
    pub(crate) fn elements_at<T>(
        &self,
        document: &Document<T>,
        point: Point2D<f32>,
    ) -> Vec<NodeId> {
        let mut elements = Vec::new();
        for node in self.hits_at(document, point) {
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
    ) -> Option<NodeId> {
        self.hits_at(document, point).next()
    }

    /// Items whose node died since the frame was built are skipped, not
    /// answered as their id: the frame's geometry for them is gone, and the
    /// id names nothing a caller could do anything with.
    fn hits_at<'frame, T>(
        &'frame self,
        document: &'frame Document<T>,
        point: Point2D<f32>,
    ) -> impl Iterator<Item = NodeId> + 'frame {
        self.items
            .iter()
            .rev()
            .filter_map(move |item| self.item_hit(item, point))
            .filter(move |&node| document.contains_node(node))
    }

    pub(super) fn item_hit(&self, item: &PaintItem, point: Point2D<f32>) -> Option<NodeId> {
        if !item.hit_testable {
            return None;
        }
        let local = item.transform.inverse()?.transform_point2d(point)?;
        if local.x >= item.size.width || local.y >= item.size.height {
            return None;
        }
        if !geometry::rounded_rect_contains(Rect::from_size(item.size), &item.radii, local) {
            return None;
        }
        if !self.point_passes_clips(item.clip, point) {
            return None;
        }
        Some(match item.kind {
            PaintItemKind::ElementBox => item.node,
            PaintItemKind::TextRun { element } => element,
        })
    }

    fn point_passes_clips(&self, mut clip: Option<usize>, point: Point2D<f32>) -> bool {
        while let Some(index) = clip {
            let node = &self.clips[index];
            let Some(local) = node
                .transform
                .inverse()
                .and_then(|inverse| inverse.transform_point2d(point))
            else {
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
