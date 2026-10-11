//! The walk: one pass over the XML tree producing the flat item list.
//!
//! Every element goes through [`Converter::convert_element`], which
//! resolves its style, opens the layers its group effects need, converts
//! its content (children, shape, text, referenced element) and closes the
//! layers. Layer bounds and bounding-box clip transforms depend on the
//! content, so they are patched into the already-pushed items once the
//! content is in.

use std::str::FromStr;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use super::paint_server::{self, GradientSpec};
use super::style::{
    Axis, Fallback, Paint, Style, Viewport, declarations, parse_first_length, parse_length,
};
use super::text::{Space, TextChunk, TextItem, TextPaint, TextSpan, normalize_whitespace};
use super::{FillPaint, Item, LayerClip, StrokePaint, SvgError, VectorDocument, nesting, shapes};
use crate::render::image::{AspectAlign, AspectRatio};
use crate::vello::kurbo::{Affine, BezPath, Rect, Shape, Stroke};
use crate::vello::peniko::{Brush, Fill};

/// The default object size of CSS Images 3, in CSS px.
const DEFAULT_OBJECT_SIZE: (f32, f32) = (300.0, 150.0);

/// How deep the walk goes: element nesting and `use` expansion together.
/// An element (with its subtree) or a `use` target past it is skipped, and
/// markup nested deeper never reaches `roxmltree` ([`nesting::bound`]).
///
/// Both recurse once per level, and natively the host parses on a decode
/// pool thread with a 2 MiB stack, where an overflow aborts the process.
pub(super) const MAX_NESTING: u32 = 256;

/// Parses `bytes` as an SVG document.
pub(crate) fn parse(bytes: &[u8]) -> Result<VectorDocument, SvgError> {
    let text = std::str::from_utf8(bytes).map_err(|_| SvgError::NotUtf8)?;
    let text = nesting::bound(text, MAX_NESTING as usize).ok_or(SvgError::TooDeep)?;
    let xml = roxmltree::Document::parse_with_options(
        &text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(SvgError::Xml)?;
    let root = xml.root_element();
    if root.tag_name().name() != "svg" {
        return Err(SvgError::NotSvg);
    }
    let width = root.attribute("width").and_then(absolute_length);
    let height = root.attribute("height").and_then(absolute_length);
    let view_box = root.attribute("viewBox").and_then(view_box);
    let (natural, viewport) = vector_sizes(width, height, view_box.map(|v| (v.w, v.h)));
    let aspect = root
        .attribute("preserveAspectRatio")
        .map_or_else(AspectRatio::default, aspect_ratio);
    // Every `url(#id)` and `href` resolves through one table built in one
    // pass; the first element with an id wins, as `getElementById` says.
    let mut ids: FxHashMap<&str, roxmltree::Node<'_, '_>> = FxHashMap::default();
    for node in xml.descendants().filter(roxmltree::Node::is_element) {
        if let Some(id) = node.attribute("id") {
            ids.entry(id).or_insert(node);
        }
    }
    let mut converter = Converter {
        ids: &ids,
        items: Vec::new(),
        has_text: false,
        gradients: FxHashMap::default(),
        references: Vec::new(),
    };
    let root_transform = view_box.map_or(Affine::IDENTITY, |v| Affine::translate((-v.x, -v.y)));
    let context = Context {
        transform: root_transform,
        viewport: Viewport {
            width: f64::from(viewport.0),
            height: f64::from(viewport.1),
        },
        style: &Style::initial(),
        depth: 0,
    };
    converter.convert_root(root, context);
    Ok(VectorDocument {
        natural,
        viewport,
        aspect,
        items: converter.items,
        has_text: converter.has_text,
    })
}

/// A root `width` or `height` that is an absolute length, in px: a bare
/// number, or a number in one of the CSS absolute units (`px`; `in` = 96 px,
/// `cm` = 96/2.54 px, `mm` = 96/25.4 px, `pt` = 4/3 px, `pc` = 16 px),
/// finite and positive. Anything else, `em`, `ex` and `%` included, counts
/// as absent.
fn absolute_length(value: &str) -> Option<f32> {
    const UNITS: [(&str, f32); 6] = [
        ("px", 1.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("pt", 4.0 / 3.0),
        ("pc", 16.0),
    ];
    let value = value.trim();
    let (number, scale) = UNITS
        .iter()
        .find_map(|&(unit, scale)| value.strip_suffix(unit).map(|number| (number, scale)))
        .unwrap_or((value, 1.0));
    number
        .parse::<f32>()
        .ok()
        .map(|length| length * scale)
        .filter(|length| length.is_finite() && *length > 0.0)
}

/// A `viewBox` with a positive, finite width and height.
fn view_box(value: &str) -> Option<svgtypes::ViewBox> {
    svgtypes::ViewBox::from_str(value)
        .ok()
        .filter(|v| v.w.is_finite() && v.h.is_finite() && v.w > 0.0 && v.h > 0.0)
}

/// The natural size and viewport of a document from its root's absolute
/// `width` and `height` and its `viewBox` size.
///
/// The natural size is CSS Images 3 default sizing: both absolute → that
/// size; one absolute plus a `viewBox` → the other axis from the `viewBox`
/// ratio; a `viewBox` only → the largest size with its ratio that fits
/// 300×150; neither → 300×150; one absolute and no `viewBox` → that axis
/// with 300 or 150 for the other. Rounded to whole px, at least 1.
///
/// The viewport, the rectangle the items are drawn in, is the `viewBox`
/// size when there is one; otherwise the authored size when both axes are
/// authored, else the rounded natural size.
fn vector_sizes(
    width: Option<f32>,
    height: Option<f32>,
    view_box: Option<(f64, f64)>,
) -> ((u32, u32), (f32, f32)) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a viewBox size fits an f32"
    )]
    let view_box = view_box.map(|(w, h)| (w as f32, h as f32));
    let natural = match (width, height, view_box) {
        (Some(width), Some(height), _) => (width, height),
        (Some(width), None, Some((box_width, box_height))) => {
            (width, width * box_height / box_width)
        }
        (None, Some(height), Some((box_width, box_height))) => {
            (height * box_width / box_height, height)
        }
        (None, None, Some((box_width, box_height))) => {
            let scale = (DEFAULT_OBJECT_SIZE.0 / box_width).min(DEFAULT_OBJECT_SIZE.1 / box_height);
            (box_width * scale, box_height * scale)
        }
        (width, height, None) => (
            width.unwrap_or(DEFAULT_OBJECT_SIZE.0),
            height.unwrap_or(DEFAULT_OBJECT_SIZE.1),
        ),
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded and clamped to at least one before the cast, which saturates"
    )]
    let whole = |length: f32| length.round().max(1.0) as u32;
    let rounded = (whole(natural.0), whole(natural.1));
    let viewport = match (view_box, width, height) {
        (Some(view_box), _, _) => view_box,
        (None, Some(width), Some(height)) => (width, height),
        (None, _, _) => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a natural size with no viewBox is an authored px length or the default"
            )]
            let viewport = (rounded.0 as f32, rounded.1 as f32);
            viewport
        }
    };
    (rounded, viewport)
}

/// A `preserveAspectRatio` value; an unparsable one is the default.
fn aspect_ratio(value: &str) -> AspectRatio {
    use svgtypes::Align;
    let Ok(parsed) = svgtypes::AspectRatio::from_str(value) else {
        return AspectRatio::default();
    };
    let align = match parsed.align {
        Align::None => None,
        Align::XMinYMin => Some((AspectAlign::Min, AspectAlign::Min)),
        Align::XMidYMin => Some((AspectAlign::Mid, AspectAlign::Min)),
        Align::XMaxYMin => Some((AspectAlign::Max, AspectAlign::Min)),
        Align::XMinYMid => Some((AspectAlign::Min, AspectAlign::Mid)),
        Align::XMidYMid => Some((AspectAlign::Mid, AspectAlign::Mid)),
        Align::XMaxYMid => Some((AspectAlign::Max, AspectAlign::Mid)),
        Align::XMinYMax => Some((AspectAlign::Min, AspectAlign::Max)),
        Align::XMidYMax => Some((AspectAlign::Mid, AspectAlign::Max)),
        Align::XMaxYMax => Some((AspectAlign::Max, AspectAlign::Max)),
    };
    AspectRatio {
        align,
        slice: parsed.slice,
    }
}

/// The transform that maps `view_box` onto a `width`×`height` viewport
/// under `aspect` (SVG 2 §8.6).
fn view_box_transform(
    view_box: svgtypes::ViewBox,
    aspect: AspectRatio,
    width: f64,
    height: f64,
) -> Affine {
    let sx = width / view_box.w;
    let sy = height / view_box.h;
    let origin = Affine::translate((-view_box.x, -view_box.y));
    let Some((align_x, align_y)) = aspect.align else {
        return Affine::scale_non_uniform(sx, sy) * origin;
    };
    let scale = if aspect.slice { sx.max(sy) } else { sx.min(sy) };
    let factor = |align: AspectAlign| match align {
        AspectAlign::Min => 0.0,
        AspectAlign::Mid => 0.5,
        AspectAlign::Max => 1.0,
    };
    let tx = (width - view_box.w * scale) * factor(align_x);
    let ty = (height - view_box.h * scale) * factor(align_y);
    Affine::translate((tx, ty)) * Affine::scale(scale) * origin
}

/// Where an element is converted: its absolute transform, the viewport
/// percentages resolve against, the parent's computed style, and how many
/// levels of the walk enclose it ([`MAX_NESTING`]).
#[derive(Clone, Copy)]
struct Context<'s> {
    transform: Affine,
    viewport: Viewport,
    style: &'s Style,
    depth: u32,
}

/// A `clipPath` resolved to one shape.
struct ClipDef {
    shape: BezPath,
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

/// Which layer items an element opened, and how to fix them up once its
/// content is known.
struct OpenLayers {
    /// How many `Pop`s the element owes.
    count: usize,
    /// The compositing layer whose bounds the content decides.
    bounds_layer: Option<usize>,
    /// Clip items whose transform awaits the element's bounding box
    /// (`clipPathUnits="objectBoundingBox"`), with their pre-bbox transform.
    object_clips: SmallVec<[(usize, Affine, Affine); 1]>,
}

struct Converter<'a> {
    /// Every element with an `id`, by id.
    ids: &'a Ids<'a>,
    items: Vec<Item>,
    has_text: bool,
    /// Gradient specifications by element, built on first use.
    gradients: FxHashMap<roxmltree::NodeId, Option<Arc<GradientSpec>>>,
    /// The `use` targets being converted, against reference cycles.
    references: Vec<roxmltree::NodeId>,
}

/// Elements whose content is drawn where they stand.
fn is_rendered(name: &str) -> bool {
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

fn is_shape(name: &str) -> bool {
    matches!(
        name,
        "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon"
    )
}

impl<'a> Converter<'a> {
    /// Converts the root `svg` as a group: its own properties and group
    /// effects apply, but its viewport and `viewBox` are the document's,
    /// already in `parent` and the sizes, not a nested viewport to clip.
    fn convert_root(&mut self, root: roxmltree::Node<'a, 'a>, parent: Context<'_>) {
        let declared = declarations(root);
        let style = parent.style.resolve(&declared, parent.viewport);
        if !style.display || style.masked {
            return;
        }
        let transform = parent.transform * style.transform;
        let context = Context {
            transform,
            viewport: parent.viewport,
            style: &style,
            depth: parent.depth + 1,
        };
        let layers = self.open_layers(&style, context);
        let content_start = self.items.len();
        self.convert_children(root, context);
        self.close_layers(layers, content_start, transform);
    }

    /// Converts one element and everything it draws; nothing past
    /// [`MAX_NESTING`].
    fn convert_element(&mut self, node: roxmltree::Node<'a, 'a>, parent: Context<'_>) {
        let name = node.tag_name().name();
        if !is_rendered(name) || parent.depth >= MAX_NESTING {
            return;
        }
        let declared = declarations(node);
        let style = parent.style.resolve(&declared, parent.viewport);
        if !style.display || style.masked {
            return;
        }
        let transform = parent.transform * style.transform;
        let context = Context {
            transform,
            viewport: parent.viewport,
            style: &style,
            depth: parent.depth + 1,
        };
        let layers = self.open_layers(&style, context);
        let content_start = self.items.len();
        match name {
            "g" | "a" => self.convert_children(node, context),
            "svg" => self.convert_svg(node, context, None),
            "use" => self.convert_use(node, context),
            "switch" => {
                if let Some(child) =
                    node.children()
                        .filter(roxmltree::Node::is_element)
                        .find(|child| {
                            child.attribute("systemLanguage").is_none()
                                && child.attribute("requiredFeatures").is_none()
                                && child.attribute("requiredExtensions").is_none()
                        })
                {
                    self.convert_element(child, context);
                }
            }
            "text" => self.convert_text(node, context),
            _ => self.convert_shape(node, name, context),
        }
        self.close_layers(layers, content_start, transform);
    }

    fn convert_children(&mut self, node: roxmltree::Node<'a, 'a>, context: Context<'_>) {
        for child in node.children().filter(roxmltree::Node::is_element) {
            self.convert_element(child, context);
        }
    }

    /// Opens the layers `style`'s group effects need around the element's
    /// content: a compositing layer for opacity (shaped by a one-shape clip
    /// when there is one, else by the content's bounds), then the clip
    /// chain, outermost first. Every layer is `Normal`: `mix-blend-mode`
    /// has no presentation attribute and CSS is not read.
    fn open_layers(&mut self, style: &Style, context: Context<'_>) -> OpenLayers {
        let mut open = OpenLayers {
            count: 0,
            bounds_layer: None,
            object_clips: SmallVec::new(),
        };
        let clip = style
            .clip_path
            .as_deref()
            .and_then(|id| self.clip_path(id, context, 0));
        let composite = style.opacity < 1.0;
        let mut own_clip_pushed = false;
        if composite {
            // A one-shape clip doubles as the layer's shape (the clips
            // clipping it are pushed inside, as clip layers).
            match &clip {
                Some(clip) if clip.single => {
                    let index = self.items.len();
                    if clip.object_units {
                        open.object_clips
                            .push((index, context.transform, clip.transform));
                    }
                    self.items.push(Item::PushLayer {
                        alpha: style.opacity,
                        clip: LayerClip::Path(clip.shape.clone(), clip.rule),
                        transform: context.transform * clip.transform,
                    });
                    own_clip_pushed = true;
                }
                _ => {
                    open.bounds_layer = Some(self.items.len());
                    self.items.push(Item::PushLayer {
                        alpha: style.opacity,
                        clip: LayerClip::Bounds(Rect::ZERO),
                        transform: context.transform,
                    });
                }
            }
            open.count += 1;
        }
        if let Some(clip) = clip {
            if let Some(outer) = &clip.outer {
                self.push_clip_chain(outer, context.transform, &mut open);
            }
            if !own_clip_pushed {
                self.push_clip(&clip, context.transform, &mut open);
            }
        }
        open
    }

    /// Pushes `clip` and every `clipPath` clipping it, outermost first.
    fn push_clip_chain(&mut self, clip: &ClipDef, group: Affine, open: &mut OpenLayers) {
        if let Some(outer) = &clip.outer {
            self.push_clip_chain(outer, group, open);
        }
        self.push_clip(clip, group, open);
    }

    /// Pushes one clip layer for `clip`, placed by the clipped element's
    /// transform `group`.
    fn push_clip(&mut self, clip: &ClipDef, group: Affine, open: &mut OpenLayers) {
        let index = self.items.len();
        if clip.object_units {
            open.object_clips.push((index, group, clip.transform));
        }
        self.items.push(Item::PushClip {
            shape: clip.shape.clone(),
            rule: clip.rule,
            transform: group * clip.transform,
        });
        open.count += 1;
    }

    /// Closes the element's layers and patches what its content decided:
    /// the compositing layer's bounds and bounding-box clip transforms.
    fn close_layers(&mut self, open: OpenLayers, content_start: usize, transform: Affine) {
        if open.count == 0 {
            return;
        }
        if open.bounds_layer.is_some() || !open.object_clips.is_empty() {
            let content = &self.items[content_start..];
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
                && let Item::PushLayer { clip, .. } = &mut self.items[index]
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
                        &mut self.items[index]
                    {
                        *transform = group * units * clip_transform;
                    }
                }
            }
        }
        for _ in 0..open.count {
            self.items.push(Item::Pop);
        }
    }

    /// A nested `svg` (or a `symbol` through `use`): its viewport is
    /// clipped and its `viewBox` mapped by its `preserveAspectRatio`.
    /// `size` overrides the element's own `width`/`height` for a `use`.
    fn convert_svg(
        &mut self,
        node: roxmltree::Node<'a, 'a>,
        context: Context<'_>,
        size: Option<(Option<f64>, Option<f64>)>,
    ) {
        let font_size = f64::from(context.style.font.size);
        let length = |name: &str, axis: Axis| {
            node.attribute(name)
                .and_then(|value| parse_length(value, font_size, context.viewport, axis))
        };
        let x = length("x", Axis::Horizontal).unwrap_or(0.0);
        let y = length("y", Axis::Vertical).unwrap_or(0.0);
        let (use_width, use_height) = size.unwrap_or((None, None));
        let width = use_width
            .or_else(|| length("width", Axis::Horizontal))
            .unwrap_or(context.viewport.width);
        let height = use_height
            .or_else(|| length("height", Axis::Vertical))
            .unwrap_or(context.viewport.height);
        if !(width > 0.0 && height > 0.0) {
            return;
        }
        let view_box = node.attribute("viewBox").and_then(view_box);
        let aspect = node
            .attribute("preserveAspectRatio")
            .map_or_else(AspectRatio::default, aspect_ratio);
        let inner = Affine::translate((x, y))
            * view_box.map_or(Affine::IDENTITY, |v| {
                view_box_transform(v, aspect, width, height)
            });
        let viewport = view_box.map_or(Viewport { width, height }, |v| Viewport {
            width: v.w,
            height: v.h,
        });
        self.items.push(Item::PushClip {
            shape: Rect::new(x, y, x + width, y + height).to_path(0.1),
            rule: Fill::NonZero,
            transform: context.transform,
        });
        self.convert_children(
            node,
            Context {
                transform: context.transform * inner,
                viewport,
                style: context.style,
                depth: context.depth,
            },
        );
        self.items.push(Item::Pop);
    }

    /// A `use`: its target drawn at `x`/`y`, inheriting from the `use`.
    /// The expansion is a level of its own, so a chain of `use`s is bounded
    /// by [`MAX_NESTING`] like nested elements.
    fn convert_use(&mut self, node: roxmltree::Node<'a, 'a>, context: Context<'_>) {
        if context.depth >= MAX_NESTING {
            return;
        }
        let Some(target) = paint_server::href(node, self.ids) else {
            return;
        };
        if self.references.contains(&target.id())
            || node
                .ancestors()
                .any(|ancestor| ancestor.id() == target.id())
        {
            return;
        }
        let font_size = f64::from(context.style.font.size);
        let length = |name: &str, axis: Axis| {
            node.attribute(name)
                .and_then(|value| parse_length(value, font_size, context.viewport, axis))
        };
        let x = length("x", Axis::Horizontal).unwrap_or(0.0);
        let y = length("y", Axis::Vertical).unwrap_or(0.0);
        let placed = Context {
            transform: context.transform * Affine::translate((x, y)),
            depth: context.depth + 1,
            ..context
        };
        self.references.push(target.id());
        match target.tag_name().name() {
            // Both are viewports the `use` sizes; a `symbol` is only ever
            // drawn this way.
            "symbol" | "svg" => {
                let declared = declarations(target);
                let style = context.style.resolve(&declared, context.viewport);
                if style.display && !style.masked {
                    let inner = Context {
                        transform: placed.transform * style.transform,
                        viewport: placed.viewport,
                        style: &style,
                        depth: placed.depth,
                    };
                    let layers = self.open_layers(&style, inner);
                    let content_start = self.items.len();
                    self.convert_svg(
                        target,
                        inner,
                        Some((
                            length("width", Axis::Horizontal),
                            length("height", Axis::Vertical),
                        )),
                    );
                    self.close_layers(layers, content_start, inner.transform);
                }
            }
            _ => self.convert_element(target, placed),
        }
        self.references.pop();
    }

    /// A basic shape or `path` as one [`Item::Path`].
    fn convert_shape(&mut self, node: roxmltree::Node<'a, 'a>, name: &str, context: Context<'_>) {
        let style = context.style;
        if !style.visible {
            return;
        }
        let Some(shape) = Self::shape_geometry(node, name, context) else {
            return;
        };
        if shape.is_empty() {
            return;
        }
        let bbox = shape.bounding_box();
        let fill = self
            .resolve_paint(&style.fill, style.fill_opacity, style, Some(bbox), context)
            .map(|(brush, brush_transform)| FillPaint {
                brush,
                brush_transform,
                rule: style.fill_rule,
            });
        let stroke = (style.stroke_width > 0.0)
            .then(|| {
                self.resolve_paint(
                    &style.stroke,
                    style.stroke_opacity,
                    style,
                    Some(bbox),
                    context,
                )
            })
            .flatten()
            .map(|(brush, brush_transform)| StrokePaint {
                style: stroke_style(style),
                brush,
                brush_transform,
            });
        if fill.is_none() && stroke.is_none() {
            return;
        }
        self.items.push(Item::Path {
            shape,
            bounds: bbox,
            transform: context.transform,
            fill,
            stroke,
            fill_first: !style.stroke_first,
        });
    }

    /// The geometry of a shape element in its own user space.
    fn shape_geometry(
        node: roxmltree::Node<'a, 'a>,
        name: &str,
        context: Context<'_>,
    ) -> Option<BezPath> {
        let font_size = f64::from(context.style.font.size);
        let length = |name: &str, axis: Axis| {
            node.attribute(name)
                .and_then(|value| parse_length(value, font_size, context.viewport, axis))
        };
        let or_zero = |name: &str, axis: Axis| length(name, axis).unwrap_or(0.0);
        match name {
            "path" => node.attribute("d").map(shapes::path_data),
            "rect" => shapes::rect(
                or_zero("x", Axis::Horizontal),
                or_zero("y", Axis::Vertical),
                or_zero("width", Axis::Horizontal),
                or_zero("height", Axis::Vertical),
                length("rx", Axis::Horizontal),
                length("ry", Axis::Vertical),
            ),
            "circle" => shapes::circle(
                or_zero("cx", Axis::Horizontal),
                or_zero("cy", Axis::Vertical),
                or_zero("r", Axis::Diagonal),
            ),
            "ellipse" => shapes::ellipse(
                or_zero("cx", Axis::Horizontal),
                or_zero("cy", Axis::Vertical),
                length("rx", Axis::Horizontal),
                length("ry", Axis::Vertical),
            ),
            "line" => Some(shapes::line(
                or_zero("x1", Axis::Horizontal),
                or_zero("y1", Axis::Vertical),
                or_zero("x2", Axis::Horizontal),
                or_zero("y2", Axis::Vertical),
            )),
            "polyline" => node
                .attribute("points")
                .map(|points| shapes::polyline(points, false)),
            "polygon" => node
                .attribute("points")
                .map(|points| shapes::polyline(points, true)),
            _ => None,
        }
    }

    /// The brush for `paint` at `opacity` on an element of `style`, whose
    /// bounding box is `bbox`. `None` paints nothing.
    fn resolve_paint(
        &mut self,
        paint: &Paint,
        opacity: f32,
        style: &Style,
        bbox: Option<Rect>,
        context: Context<'_>,
    ) -> Option<(Brush, Option<Affine>)> {
        let solid = |color: svgtypes::Color| {
            Some((
                Brush::Solid(paint_server::with_opacity(color, opacity)),
                None,
            ))
        };
        match paint {
            Paint::None => None,
            Paint::Color(color) => solid(*color),
            Paint::CurrentColor => solid(style.color),
            Paint::Server { id, fallback } => match self.paint_server(id, context) {
                PaintServer::Gradient(spec) => paint_server::brush(&spec, bbox, opacity),
                PaintServer::Pattern => None,
                PaintServer::Missing => match fallback {
                    Some(Fallback::Color(color)) => solid(*color),
                    Some(Fallback::CurrentColor) => solid(style.color),
                    Some(Fallback::None) | None => None,
                },
            },
        }
    }

    /// The text paint for `paint`, resolved as far as the parse can.
    fn resolve_text_paint(
        &mut self,
        paint: &Paint,
        opacity: f32,
        style: &Style,
        context: Context<'_>,
    ) -> Option<TextPaint> {
        let solid = |color: svgtypes::Color| {
            Some(TextPaint::Solid(paint_server::with_opacity(color, opacity)))
        };
        match paint {
            Paint::None => None,
            Paint::Color(color) => solid(*color),
            Paint::CurrentColor => solid(style.color),
            Paint::Server { id, fallback } => match self.paint_server(id, context) {
                PaintServer::Gradient(spec) => Some(TextPaint::Gradient(spec, opacity)),
                PaintServer::Pattern => None,
                PaintServer::Missing => match fallback {
                    Some(Fallback::Color(color)) => solid(*color),
                    Some(Fallback::CurrentColor) => solid(style.color),
                    Some(Fallback::None) | None => None,
                },
            },
        }
    }

    /// What `url(#id)` names.
    fn paint_server(&mut self, id: &str, context: Context<'_>) -> PaintServer {
        let Some(node) = self.ids.get(id).copied() else {
            return PaintServer::Missing;
        };
        match node.tag_name().name() {
            "linearGradient" | "radialGradient" => {
                let spec = self.gradients.entry(node.id()).or_insert_with(|| {
                    paint_server::gradient(
                        node,
                        self.ids,
                        context.viewport,
                        f64::from(context.style.font.size),
                    )
                });
                spec.clone()
                    .map_or(PaintServer::Missing, PaintServer::Gradient)
            }
            "pattern" => PaintServer::Pattern,
            _ => PaintServer::Missing,
        }
    }

    /// The `clipPath` with `id` resolved to one shape, with its own clip
    /// chain. `depth` guards a `clip-path` cycle.
    fn clip_path(&mut self, id: &str, context: Context<'_>, depth: usize) -> Option<ClipDef> {
        if depth > 8 {
            return None;
        }
        let node = self
            .ids
            .get(id)
            .copied()
            .filter(|node| node.has_tag_name("clipPath"))?;
        let declared = declarations(node);
        let style = context.style.resolve(&declared, context.viewport);
        let object_units = node.attribute("clipPathUnits") == Some("objectBoundingBox");
        let outer = style
            .clip_path
            .as_deref()
            .and_then(|outer| self.clip_path(outer, context, depth + 1))
            .map(Box::new);
        // Children, each under its own transform; `use` one level deep.
        let mut parts: SmallVec<[(BezPath, Fill); 2]> = SmallVec::new();
        let child_context = Context {
            transform: Affine::IDENTITY,
            viewport: context.viewport,
            style: &style,
            depth: context.depth,
        };
        for child in node.children().filter(roxmltree::Node::is_element) {
            let (shape_node, placement) = if child.has_tag_name("use") {
                let Some(target) = paint_server::href(child, self.ids) else {
                    continue;
                };
                let use_declared = declarations(child);
                let use_style = style.resolve(&use_declared, context.viewport);
                let font_size = f64::from(use_style.font.size);
                let at = |name: &str, axis: Axis| {
                    child
                        .attribute(name)
                        .and_then(|value| parse_length(value, font_size, context.viewport, axis))
                        .unwrap_or(0.0)
                };
                (
                    target,
                    use_style.transform
                        * Affine::translate((at("x", Axis::Horizontal), at("y", Axis::Vertical))),
                )
            } else {
                (child, Affine::IDENTITY)
            };
            let name = shape_node.tag_name().name();
            if !is_shape(name) {
                continue;
            }
            let child_declared = declarations(shape_node);
            let child_style = style.resolve(&child_declared, context.viewport);
            if !child_style.display || !child_style.visible {
                continue;
            }
            let shape_context = Context {
                style: &child_style,
                ..child_context
            };
            let Some(mut shape) = Self::shape_geometry(shape_node, name, shape_context) else {
                continue;
            };
            let child_transform = placement * child_style.transform;
            if child_transform != Affine::IDENTITY {
                shape.apply_affine(child_transform);
            }
            parts.push((shape, child_style.clip_rule));
        }
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
        Some(ClipDef {
            shape,
            rule,
            transform: style.transform,
            object_units,
            single,
            outer,
        })
    }

    /// A `text` element as one [`Item::Text`].
    fn convert_text(&mut self, node: roxmltree::Node<'a, 'a>, context: Context<'_>) {
        let style = context.style;
        let font_size = f64::from(style.font.size);
        let first = |node: roxmltree::Node<'a, 'a>, name: &str, size: f64, axis: Axis| {
            node.attribute(name)
                .and_then(|value| parse_first_length(value, size, context.viewport, axis))
        };
        #[expect(clippy::cast_possible_truncation, reason = "text positions fit an f32")]
        let as_f32 = |value: f64| value as f32;
        let mut chunks = vec![TextChunk {
            x: as_f32(first(node, "x", font_size, Axis::Horizontal).unwrap_or(0.0)),
            y: as_f32(first(node, "y", font_size, Axis::Vertical).unwrap_or(0.0)),
            anchor: style.text_anchor,
            spans: Vec::new(),
        }];
        let mut pending = (
            as_f32(first(node, "dx", font_size, Axis::Horizontal).unwrap_or(0.0)),
            as_f32(first(node, "dy", font_size, Axis::Vertical).unwrap_or(0.0)),
        );
        self.collect_spans(node, context, &mut chunks, &mut pending);
        let space = node
            .ancestors()
            .find_map(|ancestor| {
                ancestor.attribute(("http://www.w3.org/XML/1998/namespace", "space"))
            })
            .map_or(Space::Default, |value| {
                if value == "preserve" {
                    Space::Preserve
                } else {
                    Space::Default
                }
            });
        normalize_whitespace(&mut chunks, space);
        if chunks.is_empty() {
            return;
        }
        self.has_text = true;
        self.items.push(Item::Text(TextItem {
            chunks,
            transform: context.transform,
        }));
    }

    /// Appends the text and `tspan`s under `node` to `chunks`, in order.
    /// `pending` is the `dx`/`dy` the next span takes.
    fn collect_spans(
        &mut self,
        node: roxmltree::Node<'a, 'a>,
        context: Context<'_>,
        chunks: &mut Vec<TextChunk>,
        pending: &mut (f32, f32),
    ) {
        let style = context.style;
        for child in node.children() {
            // `Node::text` answers an element's first text child too, so
            // the node type decides, not the text.
            if child.is_text() {
                let text = child.text().unwrap_or_default();
                if !style.visible {
                    continue;
                }
                let fill = self.resolve_text_paint(&style.fill, style.fill_opacity, style, context);
                let stroke = (style.stroke_width > 0.0)
                    .then(|| {
                        self.resolve_text_paint(&style.stroke, style.stroke_opacity, style, context)
                    })
                    .flatten()
                    .map(|paint| (stroke_style(style), paint));
                let (dx, dy) = std::mem::take(pending);
                chunks
                    .last_mut()
                    .expect("a text element starts with one chunk")
                    .spans
                    .push(TextSpan {
                        text: text.to_owned(),
                        dx,
                        dy,
                        font: style.font.clone(),
                        letter_spacing: style.letter_spacing,
                        fill,
                        stroke,
                        fill_first: !style.stroke_first,
                    });
            } else if child.is_element()
                && (child.has_tag_name("tspan") || child.has_tag_name("a"))
                && context.depth < MAX_NESTING
            {
                let declared = declarations(child);
                let child_style = style.resolve(&declared, context.viewport);
                if !child_style.display {
                    continue;
                }
                let font_size = f64::from(child_style.font.size);
                let first = |name: &str, axis: Axis| {
                    child.attribute(name).and_then(|value| {
                        parse_first_length(value, font_size, context.viewport, axis)
                    })
                };
                #[expect(clippy::cast_possible_truncation, reason = "text positions fit an f32")]
                let as_f32 = |value: f64| value as f32;
                let (x, y) = (first("x", Axis::Horizontal), first("y", Axis::Vertical));
                if x.is_some() || y.is_some() {
                    let previous = chunks.last().expect("a text element starts with one chunk");
                    chunks.push(TextChunk {
                        x: x.map_or(previous.x, as_f32),
                        y: y.map_or(previous.y, as_f32),
                        anchor: child_style.text_anchor,
                        spans: Vec::new(),
                    });
                    *pending = (0.0, 0.0);
                }
                pending.0 += as_f32(first("dx", Axis::Horizontal).unwrap_or(0.0));
                pending.1 += as_f32(first("dy", Axis::Vertical).unwrap_or(0.0));
                let child_context = Context {
                    style: &child_style,
                    depth: context.depth + 1,
                    ..context
                };
                self.collect_spans(child, child_context, chunks, pending);
            }
        }
    }
}

/// What a `url(#id)` paint resolved to.
enum PaintServer {
    Gradient(Arc<GradientSpec>),
    Pattern,
    Missing,
}

/// The document's elements by `id`.
pub(super) type Ids<'a> = FxHashMap<&'a str, roxmltree::Node<'a, 'a>>;

/// The kurbo stroke style of `style`.
fn stroke_style(style: &Style) -> Stroke {
    let stroke = Stroke::new(style.stroke_width)
        .with_caps(style.line_cap)
        .with_join(style.line_join)
        .with_miter_limit(style.miter_limit);
    match &style.dash_array {
        Some(dashes) => stroke.with_dashes(style.dash_offset, dashes.iter().copied()),
        None => stroke,
    }
}

/// The bounding box of every path in `items`, mapped by `into` (the
/// inverse of the target space's transform), strokes included when asked.
/// Text is not counted: its extent is known only once shaped.
fn items_bounds(items: &[Item], into: Affine, include_stroke: bool) -> Option<Rect> {
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
            let reach = stroke.style.width / 2.0
                * match stroke.style.join {
                    crate::vello::kurbo::Join::Miter => stroke.style.miter_limit.max(1.0),
                    crate::vello::kurbo::Join::Round => 1.0,
                    crate::vello::kurbo::Join::Bevel => std::f64::consts::SQRT_2,
                };
            local = local.inflate(reach, reach);
        }
        let mapped = (into * *transform).transform_rect_bbox(local);
        bounds = Some(bounds.map_or(mapped, |bounds| bounds.union(mapped)));
    }
    bounds
}
