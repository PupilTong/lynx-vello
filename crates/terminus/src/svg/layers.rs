//! The layers an element's group effects open around its content, and the
//! item-list helpers the recorder (`super::record`) builds them with.

use std::sync::Arc;

use smallvec::SmallVec;

use super::recording::{Item, LayerClip};
use crate::kurbo::{Affine, BezPath, Cap, Join, Rect, Stroke};
use crate::peniko::Fill;
use crate::tree::document::NodeId;

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

pub(super) struct ClipDef {
    shape: Arc<BezPath>,
    rule: Fill,
    transform: Affine,
    object_units: bool,
    single: bool,
    outer: Option<Box<ClipDef>>,
}

impl ClipDef {
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

pub(super) struct OpenLayers {
    count: usize,
    bounds_layer: Option<usize>,
    object_clips: SmallVec<[(usize, Affine, Affine); 1]>,
}

impl OpenLayers {
    pub(super) fn map_groups(&mut self, by: Affine) {
        for (_, group, _) in &mut self.object_clips {
            *group = by * *group;
        }
    }

    pub(super) fn binds_content(&self) -> bool {
        self.bounds_layer.is_some() || !self.object_clips.is_empty()
    }
}

pub(super) fn open_layers(
    items: &mut Vec<Item>,
    node: NodeId,
    (opacity, fades): (f32, bool),
    clip: Option<ClipDef>,
    transform: Affine,
) -> OpenLayers {
    let mut open = OpenLayers {
        count: 0,
        bounds_layer: None,
        object_clips: SmallVec::new(),
    };
    let composite = opacity < 1.0 || fades;
    let mut own_clip_pushed = false;
    if composite {
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
            let reach = stroke_reach(&stroke.style, 1.0);
            local = local.inflate(reach, reach);
        }
        let mapped = (into * *transform).transform_rect_bbox(local);
        bounds = Some(bounds.map_or(mapped, |bounds| bounds.union(mapped)));
    }
    bounds
}

fn stroke_reach(style: &Stroke, least: f64) -> f64 {
    style.width / 2.0
        * match style.join {
            Join::Miter => style.miter_limit.max(least),
            Join::Round => least,
            Join::Bevel => std::f64::consts::SQRT_2,
        }
}

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
