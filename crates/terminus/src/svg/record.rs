//! A cascaded SVG subtree as the vector item list.
//!
//! [`record`] walks the elements under an SVG `svg` element whose styles are
//! flushed and produces a [`Recording`]: [`Item`]s in paint order, in
//! viewport units, every transform absolute and every paint a final brush,
//! which the visual plan and the paint walk draw (`visual::svg`,
//! `paint::svg`).
//!
//! # Elements
//!
//! `svg` (root and nested: `x`, `y`, `width`, `height`, `viewBox`,
//! `preserveAspectRatio`), `g`, `a` (as `g`), `defs`, `symbol` (through
//! `use`), `use`, `path`, `rect`, `circle`, `ellipse`, `line`, `polyline`,
//! `polygon`, `text`, `tspan`, `clipPath`, `linearGradient`,
//! `radialGradient`, `stop`, `switch` (its first child with no
//! `systemLanguage`, `requiredFeatures` or `requiredExtensions`). `image`,
//! `mask`, `filter`, `pattern`, `marker`, `style`, `title`, `desc`,
//! `metadata` and unknown elements draw nothing.
//!
//! # Values
//!
//! - **Properties** are each element's computed values: `fill`, `stroke` and their opacities,
//!   `fill-rule`, `clip-rule`, the stroke's width, caps, join, miter limit (at least one), dashes
//!   and offset, `paint-order`, `text-anchor` (`inherited_svg`); `opacity`; `visibility`;
//!   `display`; `clip-path: url()` and `mask-image` (`svg`); `transform`, `transform-origin` and
//!   `transform-box` (`box`); the font and `letter-spacing` (a percentage of the font size). A
//!   `currentcolor` paint resolves against the painted element's own computed `color`. Author
//!   rules, `style` elements, the `style` attribute and presentation attributes all reach the walk
//!   through the cascade.
//! - **Geometry** that is a CSS property is computed: `cx`, `cy`, `r`, `rx`, `ry`, `x`, `y`,
//!   `width`, `height` and `d`. A percentage is of the viewport's width for a horizontal length,
//!   its height for a vertical one, and its normalised diagonal for `r`, the stroke width, dashes
//!   and the dash offset. Geometry that is an attribute only (`points`, `x1`…`y2`, `viewBox`,
//!   `preserveAspectRatio`, the units attributes, a stop's `offset`, a `use`'s `width`/`height`, a
//!   `symbol`'s viewport, text positions, `xml:space`) is read from the attribute
//!   ([`super::attributes`]).
//! - **`transform`** is the computed list placed by `transform-origin` against the reference box
//!   `transform-box` names (css-transforms-1 §6.1), folded by the one matrix builder the CSS boxes
//!   use ([`ContextMatrix`]): `view-box` is the nearest viewport (its `viewBox` size at the user
//!   space origin); `fill-box` the element's own bounding box; `border-box` and `stroke-box` the
//!   bounding box with the stroke's reach, as the specification's used value for an element with no
//!   CSS box (`border-box` computes as `stroke-box`). A container's bounding box is known only
//!   after its content: such a container records its content first and maps the items it pushed
//!   once the box is known. Text does not count towards a bounding box before shaping, so a text
//!   element with no other content takes the `view-box` reference.
//! - **References** (`url(#id)`, a gradient's `href`) resolve through one id map over the root's
//!   light subtree, the first element with an id winning. A `url()` names an element of the subtree
//!   when it is a fragment of the document's own URL. A `use` draws its instance, the clone in its
//!   shadow root (`super::use_element`), at its computed `x`/`y`; a `symbol` or `svg` instance is a
//!   viewport the `use`'s `width`/`height` attributes size.
//!
//! # Groups, layers and clips
//!
//! - An element with `opacity` below one draws inside one `Normal` compositing layer. Its shape is
//!   the clip path when the element's `clip-path` has exactly one shape child, otherwise the
//!   bounding box of what it draws (strokes included), in the element's own coordinates. A clip
//!   with opacity one is a clip layer alone. `filter` and `mix-blend-mode` are not recorded.
//! - A `clipPath` with one shape child clips with that shape under its `clip-rule`; with several,
//!   with the concatenation of every child under `nonzero`, an approximation of the union that
//!   differs where children overlap with opposite winding. A `clipPath` that is itself clipped
//!   pushes that clip first; a chain deeper than eight references (a cycle included) is cut.
//!   `clipPathUnits="objectBoundingBox"` scales the clip by the clipped element's bounding box. A
//!   `text` child of a `clipPath` contributes nothing (glyph outlines are not available before
//!   shaping). A `clip-path` basic shape on an SVG element is not recorded.
//! - An element with a mask is skipped with its whole subtree: drawing it unmasked would show what
//!   the author hid.
//! - In a recording of an `svg` element in a document ([`RootGroup::Box`]), an element whose
//!   `opacity` or `transform` animates is marked in [`Recording::animated`] with its item range,
//!   its parent's absolute transform and the reference box its transform was folded against, for
//!   the visual plan to hand to the painter. An animated `opacity` opens the compositing layer at
//!   any value, 1 included. The reference box is left out, so no curve moves the element, when an
//!   enclosing element drew something from the extent of its content: a layer shaped by its bounds,
//!   a clip in `objectBoundingBox` units, a transform against its bounding box. An element with no
//!   animation records as it always has, and an SVG image's recording marks nothing.
//! - A nested `svg` and a `symbol` clip to their viewport and map their `viewBox` by their
//!   `preserveAspectRatio`. The root's `preserveAspectRatio` is carried as [`Recording::aspect`]
//!   for the painter, which maps the viewport onto the destination box.
//!
//! # Paint servers
//!
//! Solid colours fold the paint opacity into RGBA8. A gradient is resolved
//! for each reference ([`super::gradient`]): its attributes and stops come
//! along its `href` chain (at most 16 hops), `spreadMethod` maps to
//! [`Extend`](crate::peniko::Extend), `gradientTransform` is the brush
//! transform, `objectBoundingBox` units resolve against the shape's bounding
//! box (a text chunk's advance box after shaping) and `userSpaceOnUse`
//! percentages against the referencing element's viewport, `em` against the
//! gradient's own font size. A stop's colour is its own computed
//! `stop-color` (`currentcolor` its own `color`) with its `stop-opacity`. A
//! radial gradient's focal circle is peniko's start circle. A gradient with
//! one stop is that colour, with none paints nothing; a `pattern` paints
//! nothing, and an unresolvable `url()` paints its fallback or nothing.
//!
//! # Text
//!
//! `text` and `tspan`, as chunks: the `text` element starts one at its
//! `x`/`y`, and a `tspan` with an absolute `x` or `y` starts another; only
//! the first value of each list is read. `dx`/`dy` move the pen. A span is a
//! run of characters under one style, so a `tspan` that only changes the
//! font continues its chunk. Whitespace follows `xml:space`
//! ([`super::text`]). `textPath`, per-character `x`/`y` lists, `rotate`,
//! `textLength`, `dominant-baseline` and bidi reordering within a chunk are
//! not recorded.
//!
//! An element with no computed style (one inside a `display: none` subtree,
//! which the cascade leaves unstyled) draws nothing. The walk goes at most
//! [`MAX_NESTING`] levels deep, counting element nesting, `tspan` nesting
//! and `use` expansion.

use std::sync::Arc;

use euclid::default::{Point2D, Size2D};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use stylo::properties::ComputedValues;
use stylo::url::ComputedUrl;
use stylo::values::computed::basic_shape::ClipPath;
use stylo::values::computed::svg::{SVGOpacity, SVGPaint, SVGPaintKind};
use stylo::values::computed::{Length, LengthPercentage, Size};
use stylo::values::generics::length::LengthPercentageOrAuto;
use stylo::values::generics::svg::{SVGLength, SVGPaintFallback, SVGStrokeDashArray};
use stylo::values::specified::svg::PaintOrder;

use super::attributes::{
    Axis, Viewport, absolute_length, aspect_ratio, parse_first_length, parse_length, vector_sizes,
    view_box, view_box_transform,
};
use super::gradient::{gradient, is_gradient, plain};
use super::layers::{
    ClipDef, OpenLayers, close_layers, is_rendered, is_shape, items_bounds, open_layers, stroke,
};
use super::paint_server::{GradientSpec, brush, fold_opacity};
use super::recording::{Animated, FillPaint, Item, Recording, StrokePaint};
use super::shapes::ARC_TOLERANCE;
use super::text::{
    Space, TextAnchor, TextChunk, TextItem, TextPaint, TextSpan, normalize_whitespace,
};
use crate::kurbo::{Affine, BezPath, Cap, Join, Rect, Shape, Stroke};
use crate::paint::convert::resolve_color;
use crate::peniko::{Brush, Fill};
use crate::render::image::AspectRatio;
use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;
use crate::tree::xml::MAX_NESTING;
use crate::visual::transform::{ContextMatrix, planar};

/// Where the root `svg` element's own group effects (`opacity`,
/// `clip-path`, `mask`, `transform`) are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RootGroup {
    /// In the item list: an SVG document used as an image has no box to
    /// carry them.
    Recorded,
    /// By the root's own CSS box, which the paint walk composites like any
    /// box's: an `svg` element in a document. The list holds its content
    /// only.
    Box,
}

/// Records the subtree of the SVG `svg` element `root`, whose styles are
/// flushed. The natural size, viewport and `preserveAspectRatio` come from
/// the root's attributes, by CSS Images 3 default sizing
/// ([`vector_sizes`]).
pub(crate) fn record<T>(document: &Document<T>, root: NodeId, group: RootGroup) -> Recording {
    let node = document.get(root).expect("the root is live");
    let width = plain(node, "width").and_then(absolute_length);
    let height = plain(node, "height").and_then(absolute_length);
    let root_view_box = plain(node, "viewBox").and_then(view_box);
    let (_natural, viewport) = vector_sizes(width, height, root_view_box.map(|v| (v.w, v.h)));
    let aspect = plain(node, "preserveAspectRatio").map_or_else(AspectRatio::default, aspect_ratio);
    let mut recorder = Recorder {
        document,
        ids: ids(document, root),
        base: &document.root_node().document_url_data().0,
        items: Vec::new(),
        has_text: false,
        animated: Vec::new(),
        marks: group == RootGroup::Box,
        content_bound: 0,
    };
    let context = Context {
        transform: root_view_box.map_or(Affine::IDENTITY, |v| Affine::translate((-v.x, -v.y))),
        viewport: Viewport {
            width: f64::from(viewport.0),
            height: f64::from(viewport.1),
        },
        depth: 0,
    };
    recorder.record_root(root, context, group);
    Recording {
        viewport,
        aspect,
        items: recorder.items,
        has_text: recorder.has_text,
        animated: recorder.animated,
    }
}

/// Every element of `root`'s light subtree that has an `id`, by id; the
/// first in tree order wins, as `getElementById` says. A `use` instance is
/// in a shadow tree and is not in it.
fn ids<T>(document: &Document<T>, root: NodeId) -> FxHashMap<&str, NodeId> {
    let mut ids = FxHashMap::default();
    let mut stack = vec![root];
    while let Some(current) = stack.pop() {
        let Some(node) = document.get(current) else {
            continue;
        };
        if let Some(id) = node.id_attribute() {
            ids.entry(id).or_insert(current);
        }
        stack.extend(node.child_ids().iter().rev().copied());
    }
    ids
}

/// Where an element is recorded: its absolute transform, the viewport
/// percentages resolve against, and how many levels of the walk enclose it.
#[derive(Clone, Copy)]
struct Context {
    transform: Affine,
    viewport: Viewport,
    depth: u32,
}

/// What a `url()` paint names.
enum Server {
    Gradient(GradientSpec),
    Pattern,
    Missing,
}

/// An element's own transform, or what is needed to know it.
enum OwnTransform {
    Known(Affine),
    /// The reference box is the element's bounding box, which only its
    /// content decides.
    AfterContent,
}

struct Recorder<'d, T> {
    document: &'d Document<T>,
    ids: FxHashMap<&'d str, NodeId>,
    /// The document's URL, which a `url()` naming an element of the subtree
    /// is a fragment of.
    base: &'d url::Url,
    items: Vec<Item>,
    has_text: bool,
    animated: Vec<Animated>,
    /// Whether elements whose `transform` or `opacity` animates are marked
    /// in [`Recording::animated`]: for an `svg` element in a document, whose
    /// plan can hand them to the painter, and not for an SVG image.
    marks: bool,
    /// How many elements enclosing the one being recorded drew something
    /// from the extent of their content: a bounds layer, a bounding-box
    /// clip, a transform against their bounding box.
    content_bound: u32,
}

impl<'d, T> Recorder<'d, T> {
    /// `id` as an SVG element, with its local name.
    fn element(&self, id: NodeId) -> Option<(&'d Node<T>, &'d str)> {
        let node = self.document.get(id)?;
        if !node.is_element() || !node.in_svg_namespace() {
            return None;
        }
        Some((node, node.tag_name()?))
    }

    fn style(&self, id: NodeId) -> Option<&'d ComputedValues> {
        self.document.get(id)?.layout_computed_style()
    }

    /// The root: its content, under its own group effects when they are
    /// the list's to draw.
    fn record_root(&mut self, root: NodeId, context: Context, group: RootGroup) {
        let Some(style) = self.style(root) else {
            return;
        };
        match group {
            RootGroup::Box => self.children(
                root,
                Context {
                    depth: context.depth + 1,
                    ..context
                },
            ),
            RootGroup::Recorded => {
                if hidden(style) {
                    return;
                }
                self.with_effects(root, style, context, |recorder, inner| {
                    recorder.children(root, inner);
                });
            }
        }
    }

    fn children(&mut self, id: NodeId, context: Context) {
        let Some(node) = self.document.get(id) else {
            return;
        };
        for &child in node.child_ids() {
            self.record_element(child, context);
        }
    }

    /// One element and everything it draws; nothing past [`MAX_NESTING`].
    fn record_element(&mut self, id: NodeId, context: Context) {
        let Some((node, name)) = self.element(id) else {
            return;
        };
        if !is_rendered(name) || context.depth >= MAX_NESTING {
            return;
        }
        let Some(style) = self.style(id) else {
            return;
        };
        if hidden(style) {
            return;
        }
        self.with_effects(id, style, context, |recorder, inner| match name {
            "g" | "a" => recorder.children(id, inner),
            "svg" => recorder.nested_viewport(id, name, style, inner, None),
            "use" => recorder.record_use(id, inner),
            "switch" => {
                let chosen = node.child_ids().iter().copied().find(|&child| {
                    recorder.element(child).is_some_and(|(child, _)| {
                        ["systemLanguage", "requiredFeatures", "requiredExtensions"]
                            .iter()
                            .all(|condition| child.attribute(condition).is_none())
                    })
                });
                if let Some(child) = chosen {
                    recorder.record_element(child, inner);
                }
            }
            "text" => recorder.record_text(id, style, inner),
            _ => recorder.record_shape(id, name, style, inner),
        });
    }

    /// Records `draw` under the own transform of element `id`, of `style`,
    /// and inside the layers its `opacity` and `clip-path` open.
    fn with_effects(
        &mut self,
        id: NodeId,
        style: &'d ComputedValues,
        context: Context,
        draw: impl FnOnce(&mut Self, Context),
    ) {
        let first = self.items.len();
        let (fades, moves) = self.animates(id);
        // Marked before its content, so the list is in document order.
        let entry = (fades || moves).then(|| {
            self.animated.push(Animated {
                node: id,
                items: first..first,
                parent: context.transform,
                reference: None,
            });
            self.animated.len() - 1
        });
        // A curve moving the element would leave what an enclosing element
        // drew from the extent of its content stale.
        let moves = moves && self.content_bound == 0;
        let reference = (moves || has_transform(style)).then(|| reference(style, context.viewport));
        let reference = match reference {
            None | Some(Reference::Before(_)) => {
                let rect = match reference {
                    Some(Reference::Before(rect)) => Some(rect),
                    _ => None,
                };
                let own = rect.map_or(Affine::IDENTITY, |rect| transform_against(style, rect));
                let transform = context.transform * own;
                let inner = Context {
                    transform,
                    viewport: context.viewport,
                    depth: context.depth + 1,
                };
                let layers = self.open_layers(id, style, inner, fades);
                let bound = u32::from(layers.binds_content());
                let content_start = self.items.len();
                self.content_bound += bound;
                draw(self, inner);
                self.content_bound -= bound;
                close_layers(&mut self.items, layers, content_start, transform);
                rect.filter(|_| moves)
            }
            Some(Reference::After { include_stroke }) => {
                // Recorded as if the element had no transform of its own,
                // then mapped by it once the content gives the box.
                let provisional = Context {
                    transform: context.transform,
                    viewport: context.viewport,
                    depth: context.depth + 1,
                };
                let nested = self.animated.len();
                let mut layers = self.open_layers(id, style, provisional, fades);
                let content_start = self.items.len();
                self.content_bound += 1;
                draw(self, provisional);
                self.content_bound -= 1;
                let invertible = context.transform.determinant().abs() >= f64::EPSILON;
                let bounds = invertible
                    .then(|| {
                        items_bounds(
                            &self.items[content_start..],
                            context.transform.inverse(),
                            include_stroke,
                        )
                    })
                    .flatten()
                    .unwrap_or_else(|| view_rect(context.viewport));
                let own = transform_against(style, bounds);
                if invertible && has_transform(style) {
                    let map = context.transform * own * context.transform.inverse();
                    for item in &mut self.items[first..] {
                        map_item(item, map);
                    }
                    layers.map_groups(map);
                    for marked in &mut self.animated[nested..] {
                        marked.parent = map * marked.parent;
                    }
                }
                close_layers(
                    &mut self.items,
                    layers,
                    content_start,
                    context.transform * own,
                );
                Some(bounds).filter(|_| moves && invertible)
            }
        };
        if let Some(entry) = entry {
            let marked = &mut self.animated[entry];
            marked.items.end = self.items.len();
            marked.reference = reference;
        }
    }

    /// Whether element `id`'s `opacity` and its `transform` animate, as
    /// the stacking contexts of the CSS boxes read it
    /// (`visual::stacking`), when this recording marks such elements.
    fn animates(&self, id: NodeId) -> (bool, bool) {
        if !self.marks {
            return (false, false);
        }
        self.document.get(id).map_or((false, false), |node| {
            (node.animates_opacity(), node.animates_transform())
        })
    }

    /// [`open_layers`] for element `id` of `style`, its clip path resolved
    /// first; `fades` opens its compositing layer whatever its opacity.
    fn open_layers(
        &mut self,
        id: NodeId,
        style: &'d ComputedValues,
        context: Context,
        fades: bool,
    ) -> OpenLayers {
        let clip = self
            .clip_url(style)
            .and_then(|id| self.clip_path(id, context.viewport, 0));
        open_layers(
            &mut self.items,
            id,
            (style.get_effects().opacity, fades),
            clip,
            context.transform,
        )
    }

    /// The id `style`'s `clip-path: url()` names in this subtree.
    fn clip_url(&self, style: &'d ComputedValues) -> Option<&'d str> {
        match &style.get_svg().clip_path {
            ClipPath::Url(url) => self.fragment(url),
            _ => None,
        }
    }

    /// The fragment `url` names when it is one of the document's own URL.
    fn fragment<'u>(&self, url: &'u ComputedUrl) -> Option<&'u str> {
        match url {
            ComputedUrl::Valid(url) => {
                let fragment = url.fragment()?;
                let same =
                    url[..url::Position::AfterQuery] == self.base[..url::Position::AfterQuery];
                same.then_some(fragment)
            }
            ComputedUrl::Invalid(text) => text.trim().strip_prefix('#'),
        }
    }

    /// A nested `svg`, or a `symbol` or `svg` a `use` instantiates: its
    /// viewport is clipped and its `viewBox` mapped by its
    /// `preserveAspectRatio`. `size` is the `use`'s `width` and `height`,
    /// which override the element's own.
    fn nested_viewport(
        &mut self,
        id: NodeId,
        name: &str,
        style: &'d ComputedValues,
        context: Context,
        size: Option<(Option<f64>, Option<f64>)>,
    ) {
        let Some((node, _)) = self.element(id) else {
            return;
        };
        let viewport = context.viewport;
        let (x, y, own_width, own_height) = if name == "svg" {
            let svg = style.get_svg();
            let position = style.get_position();
            (
                resolve(&svg.x, viewport, Axis::Horizontal),
                resolve(&svg.y, viewport, Axis::Vertical),
                size_of(&position.width, viewport, Axis::Horizontal),
                size_of(&position.height, viewport, Axis::Vertical),
            )
        } else {
            let font_size = font_size(style);
            let length = |name: &str, axis: Axis| {
                plain(node, name).and_then(|value| parse_length(value, font_size, viewport, axis))
            };
            (
                length("x", Axis::Horizontal).unwrap_or(0.0),
                length("y", Axis::Vertical).unwrap_or(0.0),
                length("width", Axis::Horizontal),
                length("height", Axis::Vertical),
            )
        };
        let (use_width, use_height) = size.unwrap_or((None, None));
        let width = use_width.or(own_width).unwrap_or(viewport.width);
        let height = use_height.or(own_height).unwrap_or(viewport.height);
        if !(width > 0.0 && height > 0.0) {
            return;
        }
        let nested_box = plain(node, "viewBox").and_then(view_box);
        let aspect =
            plain(node, "preserveAspectRatio").map_or_else(AspectRatio::default, aspect_ratio);
        let inner = Affine::translate((x, y))
            * nested_box.map_or(Affine::IDENTITY, |v| {
                view_box_transform(v, aspect, width, height)
            });
        let inner_viewport = nested_box.map_or(Viewport { width, height }, |v| Viewport {
            width: v.w,
            height: v.h,
        });
        self.items.push(Item::PushClip {
            node: id,
            shape: Arc::new(Rect::new(x, y, x + width, y + height).to_path(0.1)),
            rule: Fill::NonZero,
            transform: context.transform,
        });
        self.children(
            id,
            Context {
                transform: context.transform * inner,
                viewport: inner_viewport,
                depth: context.depth,
            },
        );
        self.items.push(Item::Pop);
    }

    /// A `use`: its instance drawn at its computed `x`/`y`. The expansion
    /// is a level of its own.
    fn record_use(&mut self, id: NodeId, context: Context) {
        if context.depth >= MAX_NESTING {
            return;
        }
        let Some((node, _)) = self.element(id) else {
            return;
        };
        let Some(style) = self.style(id) else {
            return;
        };
        let Some(instance) = self.instance(node) else {
            return;
        };
        let viewport = context.viewport;
        let svg = style.get_svg();
        let x = resolve(&svg.x, viewport, Axis::Horizontal);
        let y = resolve(&svg.y, viewport, Axis::Vertical);
        let placed = Context {
            transform: context.transform * Affine::translate((x, y)),
            viewport,
            depth: context.depth + 1,
        };
        let Some((_, name)) = self.element(instance) else {
            return;
        };
        if matches!(name, "symbol" | "svg") {
            let Some(target_style) = self.style(instance) else {
                return;
            };
            if hidden(target_style) {
                return;
            }
            let font_size = font_size(style);
            let length = |name: &str, axis: Axis| {
                plain(node, name).and_then(|value| parse_length(value, font_size, viewport, axis))
            };
            let size = (
                length("width", Axis::Horizontal),
                length("height", Axis::Vertical),
            );
            self.with_effects(instance, target_style, placed, |recorder, inner| {
                recorder.nested_viewport(instance, name, target_style, inner, Some(size));
            });
        } else {
            self.record_element(instance, placed);
        }
    }

    /// The element a `use` instantiates: the first element of its shadow
    /// root.
    fn instance(&self, node: &Node<T>) -> Option<NodeId> {
        let shadow = self.document.get(node.shadow_root_id()?)?;
        shadow
            .child_ids()
            .iter()
            .copied()
            .find(|&child| self.element(child).is_some())
    }

    /// A basic shape or `path` as one [`Item::Path`].
    fn record_shape(
        &mut self,
        id: NodeId,
        name: &str,
        style: &'d ComputedValues,
        context: Context,
    ) {
        if !visible(style) {
            return;
        }
        let Some(shape) = self.geometry(id, name, style, context.viewport) else {
            return;
        };
        if shape.is_empty() {
            return;
        }
        let bbox = shape.bounding_box();
        let svg = style.get_inherited_svg();
        let fill = self
            .resolve_paint(
                &svg.fill,
                opacity(svg.fill_opacity),
                style,
                Some(bbox),
                context.viewport,
            )
            .map(|(brush, brush_transform)| FillPaint {
                brush,
                brush_transform,
                rule: fill_rule(svg.fill_rule),
            });
        let stroke = (stroke_width(style, context.viewport) > 0.0)
            .then(|| {
                self.resolve_paint(
                    &svg.stroke,
                    opacity(svg.stroke_opacity),
                    style,
                    Some(bbox),
                    context.viewport,
                )
            })
            .flatten()
            .map(|(brush, brush_transform)| StrokePaint {
                style: stroke_style(style, context.viewport),
                brush,
                brush_transform,
            });
        if fill.is_none() && stroke.is_none() {
            return;
        }
        self.items.push(Item::Path {
            node: id,
            shape,
            bounds: bbox,
            transform: context.transform,
            fill,
            stroke,
            fill_first: !stroke_first(style),
        });
    }

    /// The geometry of a shape element in its own user space.
    fn geometry(
        &self,
        id: NodeId,
        name: &str,
        style: &ComputedValues,
        viewport: Viewport,
    ) -> Option<BezPath> {
        let (node, _) = self.element(id)?;
        let svg = style.get_svg();
        let position = style.get_position();
        let font_size = font_size(style);
        let attribute = |name: &str, axis: Axis| {
            plain(node, name)
                .and_then(|value| parse_length(value, font_size, viewport, axis))
                .unwrap_or(0.0)
        };
        let auto = |length: &LengthPercentageOrAuto<_>, axis: Axis| match length {
            LengthPercentageOrAuto::LengthPercentage(length) => {
                let length: &stylo::values::computed::NonNegativeLengthPercentage = length;
                Some(resolve(&length.0, viewport, axis))
            }
            LengthPercentageOrAuto::Auto => None,
        };
        match name {
            "path" => match &svg.d {
                stylo::values::specified::DProperty::Path(data) => {
                    Some(crate::paint::shape::svg_path(data, ARC_TOLERANCE))
                }
                stylo::values::specified::DProperty::None => None,
            },
            "rect" => super::shapes::rect(
                resolve(&svg.x, viewport, Axis::Horizontal),
                resolve(&svg.y, viewport, Axis::Vertical),
                size_of(&position.width, viewport, Axis::Horizontal).unwrap_or(0.0),
                size_of(&position.height, viewport, Axis::Vertical).unwrap_or(0.0),
                auto(&svg.rx, Axis::Horizontal),
                auto(&svg.ry, Axis::Vertical),
            ),
            "circle" => super::shapes::circle(
                resolve(&svg.cx, viewport, Axis::Horizontal),
                resolve(&svg.cy, viewport, Axis::Vertical),
                resolve(&svg.r.0, viewport, Axis::Diagonal),
            ),
            "ellipse" => super::shapes::ellipse(
                resolve(&svg.cx, viewport, Axis::Horizontal),
                resolve(&svg.cy, viewport, Axis::Vertical),
                auto(&svg.rx, Axis::Horizontal),
                auto(&svg.ry, Axis::Vertical),
            ),
            "line" => Some(super::shapes::line(
                attribute("x1", Axis::Horizontal),
                attribute("y1", Axis::Vertical),
                attribute("x2", Axis::Horizontal),
                attribute("y2", Axis::Vertical),
            )),
            "polyline" => {
                plain(node, "points").map(|points| super::shapes::polyline(points, false))
            }
            "polygon" => plain(node, "points").map(|points| super::shapes::polyline(points, true)),
            _ => None,
        }
    }

    /// The brush for `paint` at `opacity` on an element of `style` whose
    /// bounding box is `bbox`. `None` paints nothing.
    fn resolve_paint(
        &self,
        paint: &SVGPaint,
        opacity: f32,
        style: &ComputedValues,
        bbox: Option<Rect>,
        viewport: Viewport,
    ) -> Option<(Brush, Option<Affine>)> {
        let solid = |color: &stylo::values::computed::Color| {
            Some((
                Brush::Solid(fold_opacity(resolve_color(style, color), opacity)),
                None,
            ))
        };
        match &paint.kind {
            SVGPaintKind::None | SVGPaintKind::ContextFill | SVGPaintKind::ContextStroke => None,
            SVGPaintKind::Color(color) => solid(color),
            SVGPaintKind::PaintServer(url) => match self.paint_server(url, viewport) {
                Server::Gradient(spec) => brush(&spec, bbox, opacity),
                Server::Pattern => None,
                Server::Missing => match &paint.fallback {
                    SVGPaintFallback::Color(color) => solid(color),
                    SVGPaintFallback::None | SVGPaintFallback::Unset => None,
                },
            },
        }
    }

    /// The text paint for `paint`, resolved as far as recording can: a
    /// gradient waits for the shaped chunk's box.
    fn resolve_text_paint(
        &self,
        paint: &SVGPaint,
        opacity: f32,
        style: &ComputedValues,
        viewport: Viewport,
    ) -> Option<TextPaint> {
        let solid = |color: &stylo::values::computed::Color| {
            Some(TextPaint::Solid(fold_opacity(
                resolve_color(style, color),
                opacity,
            )))
        };
        match &paint.kind {
            SVGPaintKind::None | SVGPaintKind::ContextFill | SVGPaintKind::ContextStroke => None,
            SVGPaintKind::Color(color) => solid(color),
            SVGPaintKind::PaintServer(url) => match self.paint_server(url, viewport) {
                Server::Gradient(spec) => Some(TextPaint::Gradient(Arc::new(spec), opacity)),
                Server::Pattern => None,
                Server::Missing => match &paint.fallback {
                    SVGPaintFallback::Color(color) => solid(color),
                    SVGPaintFallback::None | SVGPaintFallback::Unset => None,
                },
            },
        }
    }

    /// What `url` names.
    fn paint_server(&self, url: &ComputedUrl, viewport: Viewport) -> Server {
        let Some(&target) = self.fragment(url).and_then(|id| self.ids.get(id)) else {
            return Server::Missing;
        };
        match self.element(target) {
            Some((_, name)) if is_gradient(name) => {
                gradient(self.document, &self.ids, target, viewport)
                    .map_or(Server::Missing, Server::Gradient)
            }
            Some((_, "pattern")) => Server::Pattern,
            _ => Server::Missing,
        }
    }

    /// The `clipPath` with `id` resolved to one shape, with its own clip
    /// chain. `depth` cuts a `clip-path` chain past eight references, a
    /// cycle included.
    fn clip_path(&self, id: &str, viewport: Viewport, depth: usize) -> Option<ClipDef> {
        if depth > 8 {
            return None;
        }
        let target = *self.ids.get(id)?;
        let (node, name) = self.element(target)?;
        if name != "clipPath" {
            return None;
        }
        let style = self.style(target)?;
        let object_units = plain(node, "clipPathUnits") == Some("objectBoundingBox");
        let outer = self
            .clip_url(style)
            .and_then(|outer| self.clip_path(outer, viewport, depth + 1))
            .map(Box::new);
        // Children, each under its own transform; `use` one level deep.
        let mut parts: SmallVec<[(BezPath, Fill); 2]> = SmallVec::new();
        for &child in node.child_ids() {
            let Some((child_node, child_name)) = self.element(child) else {
                continue;
            };
            let (shape_id, placement) = if child_name == "use" {
                let Some(instance) = self.instance(child_node) else {
                    continue;
                };
                let Some(use_style) = self.style(child) else {
                    continue;
                };
                let svg = use_style.get_svg();
                let at = Affine::translate((
                    resolve(&svg.x, viewport, Axis::Horizontal),
                    resolve(&svg.y, viewport, Axis::Vertical),
                ));
                (instance, known_transform(use_style, viewport, None) * at)
            } else {
                (child, Affine::IDENTITY)
            };
            let Some((_, shape_name)) = self.element(shape_id) else {
                continue;
            };
            if !is_shape(shape_name) {
                continue;
            }
            let Some(child_style) = self.style(shape_id) else {
                continue;
            };
            if child_style.get_box().display.is_none() || !visible(child_style) {
                continue;
            }
            let Some(mut shape) = self.geometry(shape_id, shape_name, child_style, viewport) else {
                continue;
            };
            let child_transform =
                placement * known_transform(child_style, viewport, Some(shape.bounding_box()));
            if child_transform != Affine::IDENTITY {
                shape.apply_affine(child_transform);
            }
            parts.push((shape, fill_rule(child_style.get_inherited_svg().clip_rule)));
        }
        Some(ClipDef::new(
            parts,
            known_transform(style, viewport, None),
            object_units,
            outer,
        ))
    }

    /// A `text` element as one [`Item::Text`].
    fn record_text(&mut self, id: NodeId, style: &'d ComputedValues, context: Context) {
        let Some((node, _)) = self.element(id) else {
            return;
        };
        let size = font_size(style);
        let viewport = context.viewport;
        let first = |name: &str, axis: Axis| {
            plain(node, name).and_then(|value| parse_first_length(value, size, viewport, axis))
        };
        let mut chunks = vec![TextChunk {
            x: as_f32(first("x", Axis::Horizontal).unwrap_or(0.0)),
            y: as_f32(first("y", Axis::Vertical).unwrap_or(0.0)),
            anchor: text_anchor(style),
            spans: Vec::new(),
        }];
        let mut pending = (
            as_f32(first("dx", Axis::Horizontal).unwrap_or(0.0)),
            as_f32(first("dy", Axis::Vertical).unwrap_or(0.0)),
        );
        self.collect_spans(id, style, context, &mut chunks, &mut pending);
        normalize_whitespace(&mut chunks, self.space(id));
        if chunks.is_empty() {
            return;
        }
        self.has_text = true;
        self.items.push(Item::Text(TextItem {
            node: id,
            chunks,
            transform: context.transform,
            shaped: Vec::new(),
        }));
    }

    /// The `xml:space` of the nearest of `id` and its ancestors that has
    /// one, along the flat tree.
    fn space(&self, id: NodeId) -> Space {
        let mut current = self.document.get(id);
        while let Some(node) = current {
            if !node.in_svg_namespace() {
                break;
            }
            if let Some(value) = plain(node, "xml:space") {
                return if value == "preserve" {
                    Space::Preserve
                } else {
                    Space::Default
                };
            }
            current = node.flat_parent();
        }
        Space::Default
    }

    /// Appends the text and `tspan`s under `id` to `chunks`, in order.
    /// `pending` is the `dx`/`dy` the next span takes.
    fn collect_spans(
        &self,
        id: NodeId,
        style: &'d ComputedValues,
        context: Context,
        chunks: &mut Vec<TextChunk>,
        pending: &mut (f32, f32),
    ) {
        let Some(node) = self.document.get(id) else {
            return;
        };
        let viewport = context.viewport;
        for &child in node.child_ids() {
            let Some(child_node) = self.document.get(child) else {
                continue;
            };
            if child_node.is_text_node() {
                let text = child_node.text().unwrap_or_default();
                if !visible(style) {
                    continue;
                }
                let svg = style.get_inherited_svg();
                let fill =
                    self.resolve_text_paint(&svg.fill, opacity(svg.fill_opacity), style, viewport);
                let stroke = (stroke_width(style, viewport) > 0.0)
                    .then(|| {
                        self.resolve_text_paint(
                            &svg.stroke,
                            opacity(svg.stroke_opacity),
                            style,
                            viewport,
                        )
                    })
                    .flatten()
                    .map(|paint| (stroke_style(style, viewport), paint));
                let (dx, dy) = std::mem::take(pending);
                let size = font_size(style);
                chunks
                    .last_mut()
                    .expect("a text element starts with one chunk")
                    .spans
                    .push(TextSpan {
                        text: text.to_owned(),
                        dx,
                        dy,
                        font: seldon::text::ResolvedFont::from_computed(style.get_font()),
                        letter_spacing: as_f32(
                            style
                                .get_inherited_text()
                                .letter_spacing
                                .0
                                .resolve(Length::new(as_f32(size)))
                                .px()
                                .into(),
                        ),
                        fill,
                        stroke,
                        fill_first: !stroke_first(style),
                    });
            } else if let Some((span_node, name)) = self.element(child)
                && matches!(name, "tspan" | "a")
                && context.depth < MAX_NESTING
            {
                let Some(span_style) = self.style(child) else {
                    continue;
                };
                if span_style.get_box().display.is_none() {
                    continue;
                }
                let size = font_size(span_style);
                let first = |name: &str, axis: Axis| {
                    plain(span_node, name)
                        .and_then(|value| parse_first_length(value, size, viewport, axis))
                };
                let (x, y) = (first("x", Axis::Horizontal), first("y", Axis::Vertical));
                if x.is_some() || y.is_some() {
                    let previous = chunks.last().expect("a text element starts with one chunk");
                    chunks.push(TextChunk {
                        x: x.map_or(previous.x, as_f32),
                        y: y.map_or(previous.y, as_f32),
                        anchor: text_anchor(span_style),
                        spans: Vec::new(),
                    });
                    *pending = (0.0, 0.0);
                }
                pending.0 += as_f32(first("dx", Axis::Horizontal).unwrap_or(0.0));
                pending.1 += as_f32(first("dy", Axis::Vertical).unwrap_or(0.0));
                self.collect_spans(
                    child,
                    span_style,
                    Context {
                        depth: context.depth + 1,
                        ..context
                    },
                    chunks,
                    pending,
                );
            }
        }
    }
}

/// Whether an element and its subtree draw nothing: `display: none`, or a
/// mask, which drawing unmasked would show what the author hid.
fn hidden(style: &ComputedValues) -> bool {
    style.get_box().display.is_none() || crate::paint::mask::has_mask(style)
}

fn visible(style: &ComputedValues) -> bool {
    use stylo::computed_values::visibility::T as Visibility;
    style.get_inherited_box().visibility == Visibility::Visible
}

/// The viewport as a `view-box` reference box: its size at the user space
/// origin (css-transforms-1 §6.1).
fn view_rect(viewport: Viewport) -> Rect {
    Rect::new(0.0, 0.0, viewport.width, viewport.height)
}

/// Whether `style` gives the element a transform of its own: a transform
/// list or an individual `rotate`, `scale` or `translate`.
fn has_transform(style: &ComputedValues) -> bool {
    use stylo::values::computed::transform::{Rotate, Scale, Translate};

    let box_style = style.get_box();
    !(box_style.transform.0.is_empty()
        && matches!(box_style.rotate, Rotate::None)
        && matches!(box_style.scale, Scale::None)
        && matches!(box_style.translate, Translate::None))
}

/// The reference box `transform-box` names (css-transforms-1 §6.1): known
/// before the content (`view-box`), or the element's bounding box, strokes
/// included or not, known after it.
enum Reference {
    Before(Rect),
    After { include_stroke: bool },
}

fn reference(style: &ComputedValues, viewport: Viewport) -> Reference {
    use stylo::values::specified::transform::TransformBox;

    match style.get_box().transform_box {
        TransformBox::ViewBox => Reference::Before(view_rect(viewport)),
        TransformBox::FillBox | TransformBox::ContentBox => Reference::After {
            include_stroke: false,
        },
        TransformBox::BorderBox | TransformBox::StrokeBox => Reference::After {
            include_stroke: true,
        },
    }
}

/// What an element's own transform needs: none when it has no transform at
/// all, its reference box otherwise.
fn own_transform(style: &ComputedValues, viewport: Viewport) -> OwnTransform {
    if !has_transform(style) {
        return OwnTransform::Known(Affine::IDENTITY);
    }
    match reference(style, viewport) {
        Reference::Before(rect) => OwnTransform::Known(box_transform(style, rect)),
        Reference::After { .. } => OwnTransform::AfterContent,
    }
}

/// The own transform an element of `style` records against the reference
/// box `reference`: the identity when it has no transform.
fn transform_against(style: &ComputedValues, reference: Rect) -> Affine {
    if has_transform(style) {
        box_transform(style, reference)
    } else {
        Affine::IDENTITY
    }
}

/// An element's own transform when its bounding box, if it needs one, is
/// `bounds` (a shape's geometry) or unknown (the `view-box` reference
/// stands in).
fn known_transform(style: &ComputedValues, viewport: Viewport, bounds: Option<Rect>) -> Affine {
    match own_transform(style, viewport) {
        OwnTransform::Known(own) => own,
        OwnTransform::AfterContent => {
            box_transform(style, bounds.unwrap_or_else(|| view_rect(viewport)))
        }
    }
}

/// `style`'s transform with its origin resolved against `reference`, through
/// the matrix builder of the CSS boxes: the origin and a percentage in the
/// list resolve against the box's size, and the box's origin moves both.
fn box_transform(style: &ComputedValues, reference: Rect) -> Affine {
    let matrix = context_matrix(style, reference).with_list(&style.get_box().transform);
    planar(&matrix) * Affine::translate((-reference.x0, -reference.y0))
}

/// The matrix builder of the CSS boxes around an SVG element's transform
/// list, its reference box `reference` in the element's user space standing
/// for the border box at its offset: the factors [`box_transform`] folds a
/// list with, and an animation slot's track folds a sampled list with
/// (`visual::svg`).
pub(crate) fn context_matrix(style: &ComputedValues, reference: Rect) -> ContextMatrix {
    #[expect(clippy::cast_possible_truncation, reason = "CSS geometry is f32")]
    let (x, y, width, height) = (
        reference.x0 as f32,
        reference.y0 as f32,
        reference.width() as f32,
        reference.height() as f32,
    );
    ContextMatrix::of(style, Size2D::new(width, height), Point2D::new(x, y), None)
}

/// Maps an already recorded item's transform by `map`, applied after it.
fn map_item(item: &mut Item, map: Affine) {
    match item {
        Item::Path { transform, .. }
        | Item::PushLayer { transform, .. }
        | Item::PushClip { transform, .. } => *transform = map * *transform,
        Item::Text(text) => text.transform = map * text.transform,
        Item::Pop => {}
    }
}

/// The length a percentage of `axis` resolves against in `viewport`.
fn basis(viewport: Viewport, axis: Axis) -> f64 {
    match axis {
        Axis::Horizontal => viewport.width,
        Axis::Vertical => viewport.height,
        Axis::Diagonal => viewport.diagonal(),
    }
}

/// A computed length or percentage in user units.
fn resolve(length: &LengthPercentage, viewport: Viewport, axis: Axis) -> f64 {
    f64::from(
        length
            .resolve(Length::new(as_f32(basis(viewport, axis))))
            .px(),
    )
}

/// A computed `width` or `height`: a length or percentage, or `None` for
/// `auto` and the intrinsic keywords.
fn size_of(size: &Size, viewport: Viewport, axis: Axis) -> Option<f64> {
    match size {
        Size::LengthPercentage(length) => Some(resolve(&length.0, viewport, axis)),
        _ => None,
    }
}

fn font_size(style: &ComputedValues) -> f64 {
    f64::from(style.get_font().font_size.computed_size().px())
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "text positions and CSS lengths fit an f32"
)]
fn as_f32(value: f64) -> f32 {
    value as f32
}

fn opacity(value: SVGOpacity) -> f32 {
    match value {
        SVGOpacity::Opacity(opacity) => opacity.clamp(0.0, 1.0),
        _ => 1.0,
    }
}

fn fill_rule(rule: stylo::values::generics::basic_shape::FillRule) -> Fill {
    match rule {
        stylo::values::generics::basic_shape::FillRule::Nonzero => Fill::NonZero,
        stylo::values::generics::basic_shape::FillRule::Evenodd => Fill::EvenOdd,
    }
}

fn text_anchor(style: &ComputedValues) -> TextAnchor {
    use stylo::values::computed::svg::TextAnchor as Computed;
    match style.get_inherited_svg().text_anchor {
        Computed::Start => TextAnchor::Start,
        Computed::Middle => TextAnchor::Middle,
        Computed::End => TextAnchor::End,
    }
}

/// `paint-order` names the stroke before the fill.
fn stroke_first(style: &ComputedValues) -> bool {
    let order = &style.get_inherited_svg().paint_order;
    order.order_at(0) == PaintOrder::Stroke
        || (order.order_at(0) == PaintOrder::Markers && order.order_at(1) == PaintOrder::Stroke)
}

fn stroke_width(style: &ComputedValues, viewport: Viewport) -> f64 {
    match &style.get_inherited_svg().stroke_width {
        SVGLength::LengthPercentage(width) => resolve(&width.0, viewport, Axis::Diagonal),
        SVGLength::ContextValue => 1.0,
    }
}

/// The kurbo stroke of `style`: an odd dash list repeated to an even one,
/// a list of zeros no dashes.
fn stroke_style(style: &ComputedValues, viewport: Viewport) -> Stroke {
    use stylo::values::computed::svg::{StrokeLinecap, StrokeLinejoin};

    let svg = style.get_inherited_svg();
    let mut dashes: Vec<f64> = match &svg.stroke_dasharray {
        SVGStrokeDashArray::Values(values) => values
            .iter()
            .map(|dash| resolve(&dash.0, viewport, Axis::Diagonal))
            .collect(),
        SVGStrokeDashArray::ContextValue => Vec::new(),
    };
    if dashes.iter().all(|dash| *dash == 0.0) {
        dashes.clear();
    }
    if dashes.len() % 2 == 1 {
        dashes.extend_from_within(..);
    }
    let offset = match &svg.stroke_dashoffset {
        SVGLength::LengthPercentage(offset) => resolve(offset, viewport, Axis::Diagonal),
        SVGLength::ContextValue => 0.0,
    };
    stroke(
        stroke_width(style, viewport),
        match svg.stroke_linecap {
            StrokeLinecap::Butt => Cap::Butt,
            StrokeLinecap::Round => Cap::Round,
            StrokeLinecap::Square => Cap::Square,
        },
        match svg.stroke_linejoin {
            StrokeLinejoin::Miter => Join::Miter,
            StrokeLinejoin::Round => Join::Round,
            StrokeLinejoin::Bevel => Join::Bevel,
        },
        // SVG 2 makes a limit below one an error; the cascade keeps it.
        f64::from(svg.stroke_miterlimit.0).max(1.0),
        (!dashes.is_empty()).then_some(dashes.as_slice()),
        offset,
    )
}
