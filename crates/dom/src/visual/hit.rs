//! Hit testing: reverse paint order over the built [`PaintOrder`].

use euclid::default::{Point2D, Rect};

use super::{LocalHit, PaintItemKind, PaintOrder, geometry};
use crate::NodeId;
use crate::document::Document;

impl PaintOrder {
    /// Asserts this frame still truthfully names `document`'s nodes: freed
    /// ids can be recycled by later creations, so any consumer resolving the
    /// frame's `NodeId`s against live document state (hit testing, painting)
    /// must check this after structural mutations.
    ///
    /// # Panics
    ///
    /// Panics when nodes were removed from `document` after this frame was
    /// built; the DOM's next visual operation rebuilds it.
    pub(crate) fn assert_fresh<T>(&self, document: &Document<T>) {
        assert_eq!(
            self.epoch,
            document.node_removal_epoch(),
            "stale PaintOrder: nodes were removed after this frame was built; \
             rebuild it through the document visual pipeline",
        );
    }

    /// The stricter freshness painting needs: no visual-affecting mutation
    /// of any kind since the frame was built. Painting resolves the frame's
    /// geometry snapshot against **live** styles, layouts, and text — after
    /// a style/structure/layout mutation the mix is incoherent (a
    /// `display: none` element could paint at its former size), even though
    /// the self-contained geometry snapshot remains safe for hit testing.
    ///
    /// # Panics
    ///
    /// Panics when the document's private visual-mutation epoch (or the
    /// removal epoch) moved after this frame was built; the DOM's next visual
    /// operation rebuilds it.
    pub(crate) fn assert_visually_fresh<T>(&self, document: &Document<T>) {
        self.assert_fresh(document);
        assert_eq!(
            self.visual_epoch,
            document.visual_epoch(),
            "visually stale PaintOrder: the document mutated after this frame was built; \
             rebuild it through the document visual pipeline before painting",
        );
    }

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
    /// Panics per [`Self::assert_fresh`] (node removals only — the
    /// geometry snapshot is self-contained, so non-structural mutations
    /// keep the frame queryable).
    #[must_use]
    pub(crate) fn hit_test<T>(
        &self,
        document: &Document<T>,
        point: Point2D<f32>,
    ) -> Option<NodeId> {
        self.hit_test_local(document, point).map(|(node, _)| node)
    }

    /// [`Self::hit_test`], plus where the point landed inside the item that
    /// was hit — what an event's `offsetX`/`offsetY` is built from.
    ///
    /// The returned node is the resolved *target*; the returned point belongs
    /// to the item named by [`LocalHit::item`]. For a text-run hit those differ
    /// (the target is the styled parent element, the point is in the run's own
    /// box), which is why the item index comes along.
    ///
    /// # Panics
    ///
    /// Panics per [`Self::assert_fresh`].
    #[must_use]
    pub(crate) fn hit_test_local<T>(
        &self,
        document: &Document<T>,
        point: Point2D<f32>,
    ) -> Option<(NodeId, LocalHit)> {
        self.assert_fresh(document);
        self.items
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, item)| {
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
                if !geometry::rounded_rect_contains(Rect::from_size(item.size), &item.radii, local)
                {
                    return None;
                }
                if !self.point_passes_clips(item.clip, point) {
                    return None;
                }
                let node = match item.kind {
                    PaintItemKind::ElementBox => item.node,
                    PaintItemKind::TextRun { element } => element,
                };
                Some((
                    node,
                    LocalHit {
                        item: index,
                        position: local,
                    },
                ))
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
