#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    reason = "CSS/style geometry is f32 while Vello/Kurbo geometry is f64"
)]

//! Per-fragment CSS painters. The walker owns traversal, clipping, and
//! group compositing; each submodule paints exactly one fragment family
//! into the scene in the item's local coordinate space.

pub(crate) mod background;
pub(crate) mod border;
pub(crate) mod filters;
pub(crate) mod mask;
pub(crate) mod shadow;
pub(crate) mod text;

use crate::NodeId;
use crate::layout::{Edges, Layout, TextLayout};
use crate::shape::ReferenceBoxes;
use crate::vello::kurbo::{Affine, BezPath, Rect, Vec2};
use crate::visual::CornerRadii;

/// Glyph-silhouette source for `background-clip: text`: the element's
/// descendant text layouts with their offsets from the element's border-box
/// origin, collected by the walker (which owns the document borrow).
/// Empty means the clip region is empty — the background paints nothing.
#[derive(Debug, Default)]
pub(crate) struct TextClip<'doc> {
    pub runs: Vec<(Vec2, &'doc TextLayout)>,
}

impl TextClip<'_> {
    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

/// Reusable path buffers for the border/shadow painters, owned by the
/// walker's per-frame scratch. Every path built into a buffer is fully
/// encoded into the scene by the fill/clip call that immediately follows
/// its construction, so one buffer per role serves every item in the frame
/// without per-item allocation.
#[derive(Debug, Default)]
pub(crate) struct PathScratch {
    /// Ring (outer minus inner) fill/clip paths.
    pub ring: BezPath,
    /// Border side miter quads.
    pub quad: BezPath,
}

/// One element box's paint-ready geometry, in item-local space: the border
/// box has origin `(0, 0)`; `transform` maps item-local CSS px all the way
/// to device px (root scale included).
#[derive(Debug, Clone)]
pub(crate) struct BoxFragment {
    pub node: NodeId,
    pub transform: Affine,
    pub border_box: Rect,
    pub padding_box: Rect,
    pub content_box: Rect,
    pub radii: CornerRadii,
    pub border_widths: Edges<f32>,
    pub padding_widths: Edges<f32>,
}

impl BoxFragment {
    pub(crate) fn new(
        node: NodeId,
        transform: Affine,
        size: crate::Size2D<f32>,
        radii: CornerRadii,
        layout: &Layout,
    ) -> Self {
        let border_box = Rect::new(0.0, 0.0, size.width as f64, size.height as f64);
        let border = layout.border;
        let padding = layout.padding;
        let padding_box = Rect::new(
            border.left as f64,
            border.top as f64,
            (size.width - border.right).max(border.left) as f64,
            (size.height - border.bottom).max(border.top) as f64,
        );
        let content_box = Rect::new(
            padding_box.x0 + padding.left as f64,
            padding_box.y0 + padding.top as f64,
            (padding_box.x1 - padding.right as f64).max(padding_box.x0 + padding.left as f64),
            (padding_box.y1 - padding.bottom as f64).max(padding_box.y0 + padding.top as f64),
        );
        Self {
            node,
            transform,
            border_box,
            padding_box,
            content_box,
            radii,
            border_widths: border,
            padding_widths: padding,
        }
    }

    pub(crate) fn reference_boxes(&self) -> ReferenceBoxes<'_> {
        ReferenceBoxes {
            border: self.border_box,
            padding: self.padding_box,
            content: self.content_box,
            radii: &self.radii,
            border_widths: &self.border_widths,
            padding_widths: &self.padding_widths,
        }
    }
}
