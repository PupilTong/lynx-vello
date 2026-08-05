//! Hit testing: reverse paint order over the built [`PaintOrder`].
//!
//! Every query here is a pure read of the frame's self-contained geometry
//! snapshot — no style flush, no layout, no rebuild. Freshness policy lives
//! with the callers: painting demands [`PaintOrder::assert_visually_fresh`],
//! while the document-level hit queries gate on
//! [`PaintOrder::names_live_nodes`] and fail closed instead of panicking,
//! because "input arrived between a removal and its repaint" is a normal
//! transient state, not a bug.

use euclid::default::{Point2D, Rect};

use super::{PaintItem, PaintItemKind, PaintOrder, geometry};
use crate::NodeId;
use crate::tree::document::Document;

impl PaintOrder {
    /// Whether this frame still truthfully names `document`'s nodes: freed
    /// ids can be recycled by later creations, so any consumer resolving the
    /// frame's `NodeId`s against live document state must check this after
    /// structural mutations. The geometry snapshot itself is self-contained —
    /// a frame that fails this check is untrustworthy, not unsound: its
    /// answers may name a recycled stranger.
    #[must_use]
    pub(crate) fn names_live_nodes<T>(&self, document: &Document<T>) -> bool {
        self.epoch == document.node_removal_epoch()
    }

    /// Asserts [`Self::names_live_nodes`].
    ///
    /// # Panics
    ///
    /// Panics when nodes were removed from `document` after this frame was
    /// built; the DOM's next visual operation rebuilds it.
    pub(crate) fn assert_fresh<T>(&self, document: &Document<T>) {
        assert!(
            self.names_live_nodes(document),
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

    /// Every element whose rounded border box contains `point` (viewport CSS
    /// px), topmost first in paint order, each element once — the
    /// CSSOM-View `elementsFromPoint` list. An element hit through both its
    /// own box and its text runs collapses to its first (topmost) hit.
    #[must_use]
    pub(crate) fn elements_at(&self, point: Point2D<f32>) -> Vec<NodeId> {
        let mut elements = Vec::new();
        for node in self.hits_at(point) {
            if !elements.contains(&node) {
                elements.push(node);
            }
        }
        elements
    }

    /// The topmost element whose rounded border box contains `point` — input
    /// targeting's single answer, and the head of [`Self::elements_at`].
    #[must_use]
    pub(crate) fn first_element_at(&self, point: Point2D<f32>) -> Option<NodeId> {
        self.hits_at(point).next()
    }

    /// Raw hits at `point`, topmost first, undeduplicated: an element can
    /// repeat when both its box and its text runs are hit.
    fn hits_at(&self, point: Point2D<f32>) -> impl Iterator<Item = NodeId> + '_ {
        self.items
            .iter()
            .rev()
            .filter_map(move |item| self.item_hit(item, point))
    }

    /// The one hit predicate every query shares, honoring transforms, clip
    /// chains, `visibility`, and `pointer-events`. Text-run hits resolve to
    /// the styled parent element.
    ///
    /// Transformed candidates map the point through the inverse of their
    /// world matrix; a singular matrix means the element is not rendered
    /// (css-transforms-1) and never hit, and a point projecting behind the
    /// eye (w ≤ 0 under perspective) misses likewise.
    fn item_hit(&self, item: &PaintItem, point: Point2D<f32>) -> Option<NodeId> {
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
