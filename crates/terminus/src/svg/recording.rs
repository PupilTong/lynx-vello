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

#[derive(Clone, Debug)]
pub(crate) struct Recording {
    pub(crate) viewport: (f32, f32),
    pub(crate) aspect: AspectRatio,
    pub(crate) items: Vec<Item>,
    pub(crate) has_text: bool,
    pub(crate) animated: Vec<Animated>,
}

#[derive(Clone, Debug)]
pub(crate) struct Animated {
    pub(crate) node: NodeId,
    pub(crate) items: std::ops::Range<usize>,
    pub(crate) parent: Affine,
    pub(crate) reference: Option<Rect>,
}

#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "a path carries its brushes inline: a document is a few hundred items"
)]
pub(crate) enum Item {
    Path {
        node: NodeId,
        shape: BezPath,
        bounds: Rect,
        transform: Affine,
        fill: Option<FillPaint>,
        stroke: Option<StrokePaint>,
        fill_first: bool,
    },
    PushLayer {
        node: NodeId,
        alpha: f32,
        clip: LayerClip,
        transform: Affine,
    },
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
    pub(crate) fn node(&self) -> Option<NodeId> {
        match self {
            Self::Path { node, .. }
            | Self::PushLayer { node, .. }
            | Self::PushClip { node, .. } => Some(*node),
            Self::Text(text) => Some(text.node),
            Self::Pop => None,
        }
    }

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

#[derive(Clone, Debug)]
pub(crate) struct FillPaint {
    pub(crate) brush: Brush,
    pub(crate) brush_transform: Option<Affine>,
    pub(crate) rule: Fill,
}

#[derive(Clone, Debug)]
pub(crate) struct StrokePaint {
    pub(crate) style: Stroke,
    pub(crate) brush: Brush,
    pub(crate) brush_transform: Option<Affine>,
}

#[derive(Clone, Debug)]
pub(crate) enum LayerClip {
    Bounds(Rect),
    Path(Arc<BezPath>, Fill),
}
