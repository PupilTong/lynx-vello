//! What an SVG subtree records into: a [`Recording`], the flat list of
//! [`Item`]s the recorder (`super::record`) walks a cascaded subtree into,
//! in paint order and viewport units, every transform absolute and every
//! paint a final peniko brush, with text collected and shaped later
//! (`super::text::shape`). The visual plan (`visual::svg`) and the paint
//! walk (`paint::svg`) draw it, for an `svg` element in a document and for
//! an SVG image alike.

use std::sync::Arc;

use super::text::TextItem;
use crate::kurbo::{Affine, BezPath, Rect, Stroke};
use crate::peniko::{Brush, Fill};
use crate::render::image::AspectRatio;
use crate::tree::document::NodeId;

/// An SVG document recorded as a flat command list in viewport units, with
/// its sizes: what the visual plan and the paint walk draw once
/// [`shape`](super::text::shape) shaped its text.
#[derive(Clone, Debug)]
pub(crate) struct Recording {
    /// The rectangle the items are drawn in: the `viewBox` size when the
    /// root has one, else the natural size.
    pub(crate) viewport: (f32, f32),
    /// The root's `preserveAspectRatio`.
    pub(crate) aspect: AspectRatio,
    pub(crate) items: Vec<Item>,
    /// Whether any item is text, so [`shape`](super::text::shape) knows whether it will shape.
    pub(crate) has_text: bool,
    /// The elements whose `transform` or `opacity` animates, in document
    /// order, each with the items it draws: what the visual plan of an `svg`
    /// element in a document gives an animation slot (`visual::svg`). Empty,
    /// and unallocated, for a recording with no such element and for every
    /// SVG image's.
    pub(crate) animated: Vec<Animated>,
}

/// One element of a [`Recording`] whose `transform` or `opacity` animates:
/// where its drawing is in the item list, and what the track of a curve
/// moving it is folded from.
#[derive(Clone, Debug)]
pub(crate) struct Animated {
    pub(crate) node: NodeId,
    /// The items of the element and its subtree: its own layer pushes, its
    /// content and its pops.
    pub(crate) items: std::ops::Range<usize>,
    /// The absolute transform of the element's parent, in viewport units.
    pub(crate) parent: Affine,
    /// The reference box `transform-box` names, in the element's user
    /// space, when a curve may move the element; `None` when what the
    /// recording drew around it depends on where its content is (an
    /// enclosing element's bounds layer, bounding-box clip or bounding-box
    /// transform), which a curve moving the content would leave stale.
    pub(crate) reference: Option<Rect>,
}

/// One drawing command. Transforms are absolute (viewport space); shapes
/// are in the coordinates the transform maps from. Every command but a pop
/// names the element it was recorded from (`node`): the painted shape or
/// text, or the element whose group effects open the layer.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "a path carries its brushes inline: a document is a few hundred items"
)]
pub(crate) enum Item {
    Path {
        node: NodeId,
        shape: BezPath,
        /// The shape's bounding box in its own coordinates, computed once:
        /// what bounding-box paint units and enclosing layers read.
        bounds: Rect,
        transform: Affine,
        fill: Option<FillPaint>,
        stroke: Option<StrokePaint>,
        /// `paint-order`: the fill before the stroke.
        fill_first: bool,
    },
    /// A `Normal` compositing layer, popped by a matching [`Item::Pop`].
    PushLayer {
        node: NodeId,
        alpha: f32,
        clip: LayerClip,
        transform: Affine,
    },
    /// A clip layer, popped by a matching [`Item::Pop`].
    PushClip {
        node: NodeId,
        shape: Arc<BezPath>,
        rule: Fill,
        transform: Affine,
    },
    Pop,
    Text(TextItem),
}

impl Item {
    /// The element the item was recorded from; `None` for a pop.
    pub(crate) fn node(&self) -> Option<NodeId> {
        match self {
            Self::Path { node, .. }
            | Self::PushLayer { node, .. }
            | Self::PushClip { node, .. } => Some(*node),
            Self::Text(text) => Some(text.node),
            Self::Pop => None,
        }
    }

    /// The item's absolute transform; the identity for a pop.
    pub(crate) fn transform(&self) -> Affine {
        match self {
            Self::Path { transform, .. }
            | Self::PushLayer { transform, .. }
            | Self::PushClip { transform, .. } => *transform,
            Self::Text(text) => text.transform,
            Self::Pop => Affine::IDENTITY,
        }
    }
}

/// A fill: its brush, the brush transform (`None` for a solid colour) and
/// the fill rule.
#[derive(Clone, Debug)]
pub(crate) struct FillPaint {
    pub(crate) brush: Brush,
    pub(crate) brush_transform: Option<Affine>,
    pub(crate) rule: Fill,
}

/// A stroke: its style and brush.
#[derive(Clone, Debug)]
pub(crate) struct StrokePaint {
    pub(crate) style: Stroke,
    pub(crate) brush: Brush,
    pub(crate) brush_transform: Option<Affine>,
}

/// What a compositing layer is clipped to.
#[derive(Clone, Debug)]
pub(crate) enum LayerClip {
    /// The bounding box of the layer's own drawing, in the layer's
    /// coordinates. Text inside the layer extends it when [`shape`](super::text::shape) shapes
    /// it.
    Bounds(Rect),
    /// The element's single-shape clip path, which doubles as the layer's
    /// shape.
    Path(Arc<BezPath>, Fill),
}
