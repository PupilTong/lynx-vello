//! Hit testing: reverse paint order over the built [`PaintOrder`].

use euclid::default::{Point2D, Rect};

use super::{PaintItemKind, PaintOrder, geometry};
use crate::NodeId;

impl PaintOrder {
    /// The topmost element whose rounded border box contains `point`
    /// (viewport CSS px), honoring transforms, clip chains, `visibility`,
    /// and `pointer-events`. Text-run hits resolve to the parent element.
    ///
    /// Transformed candidates map the point through the inverse of their
    /// world matrix; a singular matrix means the element is not rendered
    /// (css-transforms-1) and never hit, and a point projecting behind the
    /// eye (w ≤ 0 under perspective) misses likewise.
    #[must_use]
    pub fn hit_test(&self, point: Point2D<f32>) -> Option<NodeId> {
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
