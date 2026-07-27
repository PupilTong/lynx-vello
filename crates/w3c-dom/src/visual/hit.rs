//! Hit testing: reverse paint order over the built [`PaintOrder`].

use euclid::default::{Point2D, Rect};

use super::{PaintItemKind, PaintOrder, geometry};
use crate::NodeId;
use crate::document::Document;

impl PaintOrder {
    /// The topmost element whose rounded border box contains `point`
    /// (viewport CSS px), honoring transforms, clip chains, `visibility`,
    /// and `pointer-events`. Text-run hits resolve to the parent element.
    ///
    /// Transformed candidates map the point through the inverse of their
    /// world matrix; a singular matrix means the element is not rendered
    /// (css-transforms-1) and never hit, and a point projecting behind the
    /// eye (w ≤ 0 under perspective) misses likewise.
    ///
    /// # Panics
    ///
    /// Panics when nodes were removed from `document` after this frame was
    /// built: freed ids may have been recycled, so the frame's geometry can
    /// no longer name nodes truthfully. Rebuild via
    /// [`Document::paint_order`].
    /// Asserts this frame still truthfully names `document`'s nodes: freed
    /// ids can be recycled by later creations, so any consumer resolving the
    /// frame's `NodeId`s against live document state (hit testing, painting)
    /// must check this after structural mutations.
    ///
    /// # Panics
    ///
    /// Panics when nodes were removed from `document` after this frame was
    /// built; rebuild via [`Document::paint_order`].
    pub fn assert_fresh<T>(&self, document: &Document<T>) {
        assert_eq!(
            self.epoch,
            document.node_removal_epoch(),
            "stale PaintOrder: nodes were removed after this frame was built; \
             rebuild it with Document::paint_order",
        );
    }

    #[must_use]
    pub fn hit_test<T>(&self, document: &Document<T>, point: Point2D<f32>) -> Option<NodeId> {
        self.assert_fresh(document);
        self.items.iter().rev().find_map(|item| {
            if !item.hit_testable {
                return None;
            }
            let local = item.transform.inverse()?.transform_point2d(point)?;
            // A box's hit region is half-open at its trailing edges (browser
            // event targeting: elementFromPoint at the far right/bottom edge
            // misses the box); leading edges and interior shared edges are
            // resolved by reverse paint order. Clip testing below stays
            // inclusive — clip regions are geometric, not targets.
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
        })
    }

    /// Whether `point` (viewport space) falls inside every clip on the
    /// chain. Each clip is tested in its own local space — its transform is
    /// anchored local → viewport, so the original point is mapped through
    /// each clip's own inverse.
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
