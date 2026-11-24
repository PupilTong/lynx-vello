//! The layers an element's group effects open around its content, and the
//! item-list helpers the recorder (`super::record`) builds them with.

use std::sync::Arc;

use smallvec::SmallVec;

use super::recording::{Item, LayerClip};
use crate::kurbo::{Affine, BezPath, Cap, Join, Rect, Stroke};
use crate::peniko::Fill;
use crate::tree::document::NodeId;

/// Elements whose content is drawn where they stand.
pub(super) fn is_rendered(name: &str) -> bool {
    matches!(
        name,
        "svg"
            | "g"
            | "a"
            | "use"
            | "switch"
            | "path"
            | "rect"
            | "circle"
            | "ellipse"
            | "line"
            | "polyline"
            | "polygon"
            | "text"
    )
}

pub(super) fn is_shape(name: &str) -> bool {
    matches!(
        name,
        "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon"
    )
}

/// A `clipPath` resolved to one shape.
pub(super) struct ClipDef {
    shape: Arc<BezPath>,
    rule: Fill,
    /// The `clipPath`'s own `transform`, after the units mapping.
    transform: Affine,
    object_units: bool,
    /// Whether the shape is one child's, so it can double as a
    /// compositing layer's shape.
    single: bool,
    /// The `clipPath`'s own `clip-path`.
    outer: Option<Box<ClipDef>>,
}

impl ClipDef {
    /// A `clipPath` from its children's shapes, each already under its own
    /// transform, with their `clip-rule`s: one shape clips under its own
    /// rule and can double as a layer's shape; several clip with their
    /// concatenation under `nonzero`. `transform` is the `clipPath`'s own,
    /// `object_units` its `clipPathUnits`, `outer` its own `clip-path`.
    pub(super) fn new(
        mut parts: SmallVec<[(BezPath, Fill); 2]>,
        transform: Affine,
        object_units: bool,
        outer: Option<Box<Self>>,
    ) -> Self {
        let single = parts.len() == 1;
        let (shape, rule) = if single {
            parts.pop().expect("one part")
        } else {
            let mut shape = BezPath::new();
            for (part, _) in parts {
                shape.extend(part);
            }
            (shape, Fill::NonZero)
        };
        Self {
            shape: Arc::new(shape),
            rule,
            transform,
            object_units,
            single,
            outer,
        }
    }
}

/// Which layer items an element opened, and how to fix them up once its
/// content is known.
pub(super) struct OpenLayers {
    /// How many `Pop`s the element owes.
    count: usize,
    /// The compositing layer whose bounds the content decides.
    bounds_layer: Option<usize>,
    /// Clip items whose transform awaits the element's bounding box
    /// (`clipPathUnits="objectBoundingBox"`), with their pre-bbox transform.
    object_clips: SmallVec<[(usize, Affine, Affine); 1]>,
}

impl OpenLayers {
    /// Maps the transforms the opened layers recorded by `by`, applied
    /// after them: what a recorder does to the items it already pushed when
    /// it learns the element's own transform only after its content
    /// (`transform-box: fill-box`).
    pub(super) fn map_groups(&mut self, by: Affine) {
        for (_, group, _) in &mut self.object_clips {
            *group = by * *group;
        }
    }
}

/// Opens the layers the group effects of element `node` need around its
/// content, at the element's transform `transform`: a compositing layer for
/// `opacity` below one, shaped by `clip` when that is one shape, else by
/// the content's bounds, patched in by [`close_layers`]; then the clip
/// chain, outermost first. Every layer is `Normal`.
pub(super) fn open_layers(
    items: &mut Vec<Item>,
    node: NodeId,
    opacity: f32,
    clip: Option<ClipDef>,
    transform: Affine,
) -> OpenLayers {
    let mut open = OpenLayers {
        count: 0,
        bounds_layer: None,
        object_clips: SmallVec::new(),
    };
    let composite = opacity < 1.0;
    let mut own_clip_pushed = false;
    if composite {
        // A one-shape clip doubles as the layer's shape (the clips
        // clipping it are pushed inside, as clip layers).
        match &clip {
            Some(clip) if clip.single => {
                let index = items.len();
                if clip.object_units {
                    open.object_clips.push((index, transform, clip.transform));
                }
                items.push(Item::PushLayer {
                    node,
                    alpha: opacity,
                    clip: LayerClip::Path(clip.shape.clone(), clip.rule),
                    transform: transform * clip.transform,
                });
                own_clip_pushed = true;
            }
            _ => {
                open.bounds_layer = Some(items.len());
                items.push(Item::PushLayer {
                    node,
                    alpha: opacity,
                    clip: LayerClip::Bounds(Rect::ZERO),
                    transform,
                });
            }
        }
        open.count += 1;
    }
    if let Some(clip) = clip {
        if let Some(outer) = &clip.outer {
            push_clip_chain(items, node, outer, transform, &mut open);
        }
        if !own_clip_pushed {
            push_clip(items, node, &clip, transform, &mut open);
        }
    }
    open
}

/// Pushes `clip` and every `clipPath` clipping it, outermost first.
fn push_clip_chain(
    items: &mut Vec<Item>,
    node: NodeId,
    clip: &ClipDef,
    group: Affine,
    open: &mut OpenLayers,
) {
    if let Some(outer) = &clip.outer {
        push_clip_chain(items, node, outer, group, open);
    }
    push_clip(items, node, clip, group, open);
}

/// Pushes one clip layer for `clip`, placed by the transform `group` of
/// the clipped element `node`.
fn push_clip(
    items: &mut Vec<Item>,
    node: NodeId,
    clip: &ClipDef,
    group: Affine,
    open: &mut OpenLayers,
) {
    let index = items.len();
    if clip.object_units {
        open.object_clips.push((index, group, clip.transform));
    }
    items.push(Item::PushClip {
        node,
        shape: clip.shape.clone(),
        rule: clip.rule,
        transform: group * clip.transform,
    });
    open.count += 1;
}

/// Closes an element's layers and patches what its content, the items
/// from `content_start` on, decided: the compositing layer's bounds and
/// bounding-box clip transforms, both in the element's own coordinates,
/// which `transform` maps to the document's.
pub(super) fn close_layers(
    items: &mut Vec<Item>,
    open: OpenLayers,
    content_start: usize,
    transform: Affine,
) {
    if open.count == 0 {
        return;
    }
    if open.bounds_layer.is_some() || !open.object_clips.is_empty() {
        let content = &items[content_start..];
        let local = |include_stroke: bool| {
            if transform.determinant().abs() < f64::EPSILON {
                None
            } else {
                items_bounds(content, transform.inverse(), include_stroke)
            }
        };
        let layer_bounds = open.bounds_layer.and_then(|_| local(true));
        let bbox = (!open.object_clips.is_empty())
            .then(|| local(false))
            .flatten();
        if let Some(index) = open.bounds_layer
            && let Some(bounds) = layer_bounds
            && let Item::PushLayer { clip, .. } = &mut items[index]
        {
            *clip = LayerClip::Bounds(bounds);
        }
        if !open.object_clips.is_empty() {
            for (index, group, clip_transform) in open.object_clips {
                let units = bbox.map_or(Affine::scale(0.0), |bbox| {
                    Affine::translate((bbox.x0, bbox.y0))
                        * Affine::scale_non_uniform(bbox.width(), bbox.height())
                });
                if let Item::PushClip { transform, .. } | Item::PushLayer { transform, .. } =
                    &mut items[index]
                {
                    *transform = group * units * clip_transform;
                }
            }
        }
    }
    for _ in 0..open.count {
        items.push(Item::Pop);
    }
}

/// A kurbo stroke from SVG's stroke properties, `dashes` already doubled
/// to an even count (`None` or empty for a solid stroke).
pub(super) fn stroke(
    width: f64,
    cap: Cap,
    join: Join,
    miter_limit: f64,
    dashes: Option<&[f64]>,
    dash_offset: f64,
) -> Stroke {
    let stroke = Stroke::new(width)
        .with_caps(cap)
        .with_join(join)
        .with_miter_limit(miter_limit);
    match dashes {
        Some(dashes) => stroke.with_dashes(dash_offset, dashes.iter().copied()),
        None => stroke,
    }
}

/// The bounding box of every path in `items`, mapped by `into` (the
/// inverse of the target space's transform), strokes included when asked.
/// Text is not counted: its extent is known only once shaped.
pub(super) fn items_bounds(items: &[Item], into: Affine, include_stroke: bool) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;
    for item in items {
        let Item::Path {
            bounds: local,
            transform,
            stroke,
            ..
        } = item
        else {
            continue;
        };
        let mut local = *local;
        if include_stroke && let Some(stroke) = stroke {
            // Half the width on each side, plus the reach of a miter join.
            let reach = stroke_reach(&stroke.style, 1.0);
            local = local.inflate(reach, reach);
        }
        let mapped = (into * *transform).transform_rect_bbox(local);
        bounds = Some(bounds.map_or(mapped, |bounds| bounds.union(mapped)));
    }
    bounds
}

/// How far a stroke of `style` reaches past its path's bounding box: half
/// its width times its join's factor (a miter join's limit, a bevel's √2),
/// and at least `least` times half its width. A layer's bounds pass 1, the
/// reach of a round join and a butt or round cap; an item's ink extent
/// passes √2, the reach of a square cap.
fn stroke_reach(style: &Stroke, least: f64) -> f64 {
    style.width / 2.0
        * match style.join {
            Join::Miter => style.miter_limit.max(least),
            Join::Round => least,
            Join::Bevel => std::f64::consts::SQRT_2,
        }
}

/// The rectangle one recorded path or text item can put ink in, in the
/// coordinates its own transform maps from, or `None` for a layer push, a
/// pop and a text item that was not shaped: a path's bounding box grown by
/// the farthest its stroke reaches past it (a miter join to its limit, a
/// square cap or a bevel by √2 of half the width), a text item's shaped
/// chunk boxes, which glyph outlines can still leave (side bearings,
/// synthetic oblique).
///
/// What the paint walk sizes an `svg` element's paint item by
/// (`visual::svg`), so the cull test and the group bounds hold every pixel
/// a path draws. A layer's bounds ([`items_bounds`]) use the join alone, as
/// the recording always has ([`stroke_reach`]).
pub(crate) fn ink_extent(item: &Item) -> Option<Rect> {
    match item {
        Item::Path { bounds, stroke, .. } => Some(stroke.as_ref().map_or(*bounds, |stroke| {
            let reach = stroke_reach(&stroke.style, std::f64::consts::SQRT_2);
            bounds.inflate(reach, reach)
        })),
        Item::Text(text) => text
            .shaped
            .iter()
            .map(|chunk| chunk.bbox)
            .reduce(|a, b| a.union(b)),
        Item::PushLayer { .. } | Item::PushClip { .. } | Item::Pop => None,
    }
}
