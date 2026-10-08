//! SVG documents as vector images: a `usvg` tree encoded into a vello scene.
//!
//! The engine parses an SVG document into a [`usvg::Tree`] inside a
//! [`VectorImage`](crate::render::image::VectorImage) (`render/image.rs`).
//! The document has one of two producers: the bytes a host reported for an
//! `<image src>` or CSS `url()` source, or the markup an inline `<svg>` root's
//! subtree serialises to (`tree/inline_svg.rs`). Both reach this module the
//! same way. It turns the tree into scene commands once, in tree units, and
//! every draw of the image appends the cached scene under its own clip and transform
//! (`background.rs`). Rules (`docs/svg-vector-images-design.md`):
//!
//! - A node is placed at `node.abs_transform()` as usvg resolved it. Nothing multiplies parent
//!   transforms here; the only extra factor is the placement of a nested SVG image's tree.
//! - A group opens a layer only when `usvg::Group::should_isolate` says so. Opacity below one or a
//!   non-`Normal` blend pushes one compositing layer: its shape is the clip path when the group's
//!   `clipPath` has exactly one path child, otherwise the group's `layer_bounding_box` (object
//!   units, so the group's own transform places it). A clip with opacity one and `Normal` blend
//!   pushes a clip layer and no compositing layer. `isolate` alone pushes nothing.
//! - A `clipPath` with one path child clips with that path under its `clip-rule`. A `clipPath` with
//!   several children clips with the concatenation of every child path under `Fill::NonZero`. That
//!   is an approximation: the exact clip is the union, which differs where two children overlap
//!   with opposite winding. A `clipPath` that is itself clipped pushes that clip as well. A
//!   `clipPath` is a usvg subroot with its own transform, so a clip child is placed at
//!   `group.abs_transform() * clip.transform() * child.abs_transform()`, the product resvg uses.
//! - A group with a `mask` is skipped with everything in it: drawing it unmasked would show content
//!   the author hid. A group with `filter`s is drawn without them.
//! - A path fills with its fill rule and strokes with its width, caps, join, miter limit and dash
//!   pattern, in its `paint-order`. An invisible path draws nothing.
//! - Solid paint is RGBA8 with the paint opacity folded in. A linear gradient maps `spreadMethod`
//!   to `Extend`. A radial gradient maps SVG's focal circle to peniko's start circle and its outer
//!   circle to the end circle. A gradient's `transform()` is the brush transform, and every stop
//!   folds in the paint opacity. A pattern paints nothing.
//! - A nested SVG image renders its tree; a nested raster image draws nothing.
//! - Text never reaches here: usvg is built without its `text` feature, so `<text>` is dropped at
//!   parse.
//!
//! One rule comes from this crate rather than from the design: a blend
//! layer never opens directly inside a clip layer (vello
//! [#1198](https://github.com/linebender/vello/issues/1198); see the
//! `walker.rs` module docs). Where a group's clip layer would hold a
//! non-`Normal` blend group with no layer pushed between them, the innermost
//! of its clip layers, the one that encloses the children, is a full
//! `Normal` layer instead ([`opens_blend`]); the clip layers outside it stay
//! clip layers. The same test decides the layer a draw of the whole image
//! opens (`background.rs`).
//!
//! The segment, stroke and brush conversions follow `vello_svg` 0.11.0
//! (`src/util.rs`, Copyright 2023 the Vello Authors, Apache-2.0 OR MIT),
//! with the focal-circle mapping, `spreadMethod` and the subpath rule
//! changed as listed above.

use usvg::tiny_skia_path::{self, PathSegment};

use crate::vello::Scene;
use crate::vello::kurbo::{Affine, BezPath, Cap, Join, Point, Rect, Stroke};
use crate::vello::peniko::color::DynamicColor;
use crate::vello::peniko::{
    BlendMode, Brush, Color, ColorStop, Compose, Extend, Fill, Gradient, Mix,
};

/// Encodes `tree` into `scene`, in tree units.
pub(crate) fn encode(tree: &usvg::Tree, scene: &mut Scene) {
    encode_children(scene, tree.root(), Affine::IDENTITY);
}

/// Whether drawing `group`'s children opens a blend layer with no isolating
/// layer of `group`'s own in between: a child group with a non-`Normal`
/// blend, reached directly or through groups that push no layer.
///
/// A group pushes no layer when its opacity is one, its blend is `Normal`
/// and it has no clip, whether or not it isolates: `isolation: isolate` or a
/// `filter` alone pushes nothing ([`encode_group`]). A child with its own
/// clip stops the search, because that child runs this test for its own
/// clip layers. A masked group draws nothing, so its blend does not count.
pub(crate) fn opens_blend(group: &usvg::Group) -> bool {
    group.children().iter().any(|node| match node {
        usvg::Node::Group(child) if child.mask().is_none() => {
            child.blend_mode() != usvg::BlendMode::Normal
                || (child.opacity() == usvg::Opacity::ONE
                    && child.clip_path().is_none()
                    && opens_blend(child))
        }
        usvg::Node::Group(_) | usvg::Node::Path(_) | usvg::Node::Image(_) | usvg::Node::Text(_) => {
            false
        }
    })
}

/// Encodes every child of `group`. `base` places the tree `group` belongs
/// to: identity for the document, the image's transform for a nested SVG
/// image.
fn encode_children(scene: &mut Scene, group: &usvg::Group, base: Affine) {
    for node in group.children() {
        match node {
            usvg::Node::Group(child) => encode_group(scene, child, base),
            usvg::Node::Path(path) => encode_path(scene, path, base),
            usvg::Node::Image(image) => {
                if !image.is_visible() {
                    continue;
                }
                match image.kind() {
                    usvg::ImageKind::SVG(tree) => {
                        encode_children(scene, tree.root(), base * affine(image.abs_transform()));
                    }
                    // Raster images nested in an SVG document are a recorded
                    // gap: their bytes would need the host's decoder.
                    usvg::ImageKind::JPEG(_)
                    | usvg::ImageKind::PNG(_)
                    | usvg::ImageKind::GIF(_)
                    | usvg::ImageKind::WEBP(_) => {}
                }
            }
            // Unreachable: usvg is built without `text`, which drops every
            // text element at parse.
            usvg::Node::Text(_) => {}
        }
    }
}

fn encode_group(scene: &mut Scene, group: &usvg::Group, base: Affine) {
    if group.mask().is_some() {
        return;
    }
    if !group.should_isolate() {
        encode_children(scene, group, base);
        return;
    }
    let transform = base * affine(group.abs_transform());
    let clip = group.clip_path();
    let composite =
        group.opacity() != usvg::Opacity::ONE || group.blend_mode() != usvg::BlendMode::Normal;
    let isolate_clips = opens_blend(group);
    let mut layers = 0_usize;
    let mut own_clip_pushed = false;
    if composite {
        let blend = blend_mode(group.blend_mode());
        let alpha = group.opacity().get();
        if let Some((clip, path)) = clip.and_then(|clip| single_path(clip).map(|path| (clip, path)))
        {
            scene.push_layer(
                clip_rule(path),
                blend,
                alpha,
                transform * affine(clip.transform()) * affine(path.abs_transform()),
                &bez_path(path.data()),
            );
            own_clip_pushed = true;
        } else {
            let bounds = group.layer_bounding_box();
            scene.push_layer(
                Fill::NonZero,
                blend,
                alpha,
                transform,
                &Rect::new(
                    f64::from(bounds.left()),
                    f64::from(bounds.top()),
                    f64::from(bounds.right()),
                    f64::from(bounds.bottom()),
                ),
            );
        }
        layers += 1;
    }
    // Only the innermost layer encloses the children directly, so only that
    // one is promoted when they open a blend layer.
    if let Some(clip) = clip {
        if let Some(outer) = clip.clip_path() {
            layers += push_clip_chain(scene, outer, transform, own_clip_pushed && isolate_clips);
        }
        if !own_clip_pushed {
            push_clip(scene, clip, transform, isolate_clips);
            layers += 1;
        }
    }
    encode_children(scene, group, base);
    for _ in 0..layers {
        scene.pop_layer();
    }
}

/// Pushes `clip` and every `clipPath` clipping it, outermost first, and
/// answers how many layers it pushed. `isolate` applies to `clip`'s own
/// layer, the innermost one; the layers around it stay clip layers.
fn push_clip_chain(
    scene: &mut Scene,
    clip: &usvg::ClipPath,
    group: Affine,
    isolate: bool,
) -> usize {
    let mut layers = 0;
    if let Some(outer) = clip.clip_path() {
        layers += push_clip_chain(scene, outer, group, false);
    }
    push_clip(scene, clip, group, isolate);
    layers + 1
}

/// Pushes one layer clipped to `clip`'s children, placed by the clipped
/// group's transform `group`. A full `Normal` layer rather than a clip layer
/// when `isolate`, so a blend layer inside never opens directly in a clip
/// layer.
fn push_clip(scene: &mut Scene, clip: &usvg::ClipPath, group: Affine, isolate: bool) {
    let placement = group * affine(clip.transform());
    let (shape, transform, rule) = if let Some(path) = single_path(clip) {
        (
            bez_path(path.data()),
            placement * affine(path.abs_transform()),
            clip_rule(path),
        )
    } else {
        let mut shape = BezPath::new();
        concatenate_paths(&mut shape, clip.root());
        (shape, placement, Fill::NonZero)
    };
    if isolate {
        scene.push_layer(
            rule,
            BlendMode::new(Mix::Normal, Compose::SrcOver),
            1.0,
            transform,
            &shape,
        );
    } else {
        scene.push_clip_layer(rule, transform, &shape);
    }
}

/// A clip child's `clip-rule`, which usvg stores as the path's fill rule.
fn clip_rule(path: &usvg::Path) -> Fill {
    path.fill()
        .map_or(Fill::NonZero, |fill| fill_rule(fill.rule()))
}

fn fill_rule(rule: usvg::FillRule) -> Fill {
    match rule {
        usvg::FillRule::NonZero => Fill::NonZero,
        usvg::FillRule::EvenOdd => Fill::EvenOdd,
    }
}

/// The clip's only child, when that child is a path.
fn single_path(clip: &usvg::ClipPath) -> Option<&usvg::Path> {
    match clip.root().children() {
        [usvg::Node::Path(path)] => Some(path),
        _ => None,
    }
}

/// Appends every path under `group` to `shape`, each mapped by its own
/// transform within the clip.
fn concatenate_paths(shape: &mut BezPath, group: &usvg::Group) {
    for node in group.children() {
        match node {
            usvg::Node::Path(path) => {
                let mut part = bez_path(path.data());
                part.apply_affine(affine(path.abs_transform()));
                shape.extend(part);
            }
            usvg::Node::Group(child) => concatenate_paths(shape, child),
            usvg::Node::Image(_) | usvg::Node::Text(_) => {}
        }
    }
}

fn encode_path(scene: &mut Scene, path: &usvg::Path, base: Affine) {
    if !path.is_visible() {
        return;
    }
    let transform = base * affine(path.abs_transform());
    let shape = bez_path(path.data());
    let fill = |scene: &mut Scene| {
        let Some(fill) = path.fill() else {
            return;
        };
        let Some((brush, brush_transform)) = brush(fill.paint(), fill.opacity()) else {
            return;
        };
        scene.fill(
            fill_rule(fill.rule()),
            transform,
            &brush,
            brush_transform,
            &shape,
        );
    };
    let stroke = |scene: &mut Scene| {
        let Some(stroke) = path.stroke() else {
            return;
        };
        let Some((brush, brush_transform)) = brush(stroke.paint(), stroke.opacity()) else {
            return;
        };
        scene.stroke(
            &stroke_style(stroke),
            transform,
            &brush,
            brush_transform,
            &shape,
        );
    };
    match path.paint_order() {
        usvg::PaintOrder::FillAndStroke => {
            fill(scene);
            stroke(scene);
        }
        usvg::PaintOrder::StrokeAndFill => {
            stroke(scene);
            fill(scene);
        }
    }
}

/// The kurbo map of a usvg transform.
pub(crate) fn affine(transform: usvg::Transform) -> Affine {
    let usvg::Transform {
        sx,
        kx,
        ky,
        sy,
        tx,
        ty,
    } = transform;
    Affine::new([sx, ky, kx, sy, tx, ty].map(f64::from))
}

/// A usvg path as a kurbo path.
///
/// tiny-skia lets a segment follow `Close` without a `MoveTo`, and such a
/// segment starts at the closed subpath's first point. `BezPath` has no
/// such rule, so the conversion re-moves to that point first.
pub(crate) fn bez_path(data: &tiny_skia_path::Path) -> BezPath {
    let point = |p: tiny_skia_path::Point| Point::new(f64::from(p.x), f64::from(p.y));
    let mut shape = BezPath::new();
    let mut start = Point::ZERO;
    let mut closed = false;
    for segment in data.segments() {
        if std::mem::take(&mut closed) && !matches!(segment, PathSegment::MoveTo(_)) {
            shape.move_to(start);
        }
        match segment {
            PathSegment::MoveTo(p) => {
                start = point(p);
                shape.move_to(start);
            }
            PathSegment::LineTo(p) => shape.line_to(point(p)),
            PathSegment::QuadTo(p1, p2) => shape.quad_to(point(p1), point(p2)),
            PathSegment::CubicTo(p1, p2, p3) => shape.curve_to(point(p1), point(p2), point(p3)),
            PathSegment::Close => {
                shape.close_path();
                closed = true;
            }
        }
    }
    shape
}

fn stroke_style(stroke: &usvg::Stroke) -> Stroke {
    let style = Stroke::new(f64::from(stroke.width().get()))
        .with_caps(match stroke.linecap() {
            usvg::LineCap::Butt => Cap::Butt,
            usvg::LineCap::Round => Cap::Round,
            usvg::LineCap::Square => Cap::Square,
        })
        .with_join(match stroke.linejoin() {
            // kurbo has no clipped miter; a plain miter is the nearest join.
            usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => Join::Miter,
            usvg::LineJoin::Round => Join::Round,
            usvg::LineJoin::Bevel => Join::Bevel,
        })
        .with_miter_limit(f64::from(stroke.miterlimit().get()));
    match stroke.dasharray() {
        Some(dashes) => style.with_dashes(
            f64::from(stroke.dashoffset()),
            dashes.iter().map(|dash| f64::from(*dash)),
        ),
        None => style,
    }
}

/// The brush and brush transform for `paint` at `opacity`, or `None` for a
/// paint this walker does not draw (a pattern).
fn brush(paint: &usvg::Paint, opacity: usvg::Opacity) -> Option<(Brush, Option<Affine>)> {
    match paint {
        usvg::Paint::Color(color) => Some((
            Brush::Solid(Color::from_rgba8(
                color.red,
                color.green,
                color.blue,
                opacity.to_u8(),
            )),
            None,
        )),
        usvg::Paint::LinearGradient(linear) => {
            let gradient = Gradient::new_linear(
                (f64::from(linear.x1()), f64::from(linear.y1())),
                (f64::from(linear.x2()), f64::from(linear.y2())),
            );
            Some((
                Brush::Gradient(with_base(gradient, linear, opacity)),
                Some(affine(linear.transform())),
            ))
        }
        usvg::Paint::RadialGradient(radial) => {
            let gradient = Gradient::new_two_point_radial(
                (f64::from(radial.fx()), f64::from(radial.fy())),
                radial.fr().get(),
                (f64::from(radial.cx()), f64::from(radial.cy())),
                radial.r().get(),
            );
            Some((
                Brush::Gradient(with_base(gradient, radial, opacity)),
                Some(affine(radial.transform())),
            ))
        }
        // A recorded gap: a pattern would need its tile rendered and
        // repeated as an image brush.
        usvg::Paint::Pattern(_) => None,
    }
}

/// `gradient` with the stops and spread method of `base`, each stop's
/// opacity multiplied by the paint's.
fn with_base(gradient: Gradient, base: &usvg::BaseGradient, opacity: usvg::Opacity) -> Gradient {
    let stops: Vec<ColorStop> = base
        .stops()
        .iter()
        .map(|stop| ColorStop {
            offset: stop.offset().get(),
            color: DynamicColor::from_alpha_color(Color::from_rgba8(
                stop.color().red,
                stop.color().green,
                stop.color().blue,
                (stop.opacity() * opacity).to_u8(),
            )),
        })
        .collect();
    gradient
        .with_extend(match base.spread_method() {
            usvg::SpreadMethod::Pad => Extend::Pad,
            usvg::SpreadMethod::Reflect => Extend::Reflect,
            usvg::SpreadMethod::Repeat => Extend::Repeat,
        })
        .with_stops(stops.as_slice())
}

fn blend_mode(mode: usvg::BlendMode) -> BlendMode {
    let mix = match mode {
        usvg::BlendMode::Normal => Mix::Normal,
        usvg::BlendMode::Multiply => Mix::Multiply,
        usvg::BlendMode::Screen => Mix::Screen,
        usvg::BlendMode::Overlay => Mix::Overlay,
        usvg::BlendMode::Darken => Mix::Darken,
        usvg::BlendMode::Lighten => Mix::Lighten,
        usvg::BlendMode::ColorDodge => Mix::ColorDodge,
        usvg::BlendMode::ColorBurn => Mix::ColorBurn,
        usvg::BlendMode::HardLight => Mix::HardLight,
        usvg::BlendMode::SoftLight => Mix::SoftLight,
        usvg::BlendMode::Difference => Mix::Difference,
        usvg::BlendMode::Exclusion => Mix::Exclusion,
        usvg::BlendMode::Hue => Mix::Hue,
        usvg::BlendMode::Saturation => Mix::Saturation,
        usvg::BlendMode::Color => Mix::Color,
        usvg::BlendMode::Luminosity => Mix::Luminosity,
    };
    BlendMode::new(mix, Compose::SrcOver)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    //! Each test parses a hand-written document and compares the walker's
    //! scene with one built from the vello calls the rule names, through
    //! `assert_scenes_identical`, which compares every encoding stream.

    use super::{affine, bez_path, encode, opens_blend};
    use crate::paint::equivalence::assert_scenes_identical;
    use crate::vello::Scene;
    use crate::vello::kurbo::{Affine, BezPath, Cap, Join, PathEl, Point, Rect, Stroke};
    use crate::vello::peniko::color::DynamicColor;
    use crate::vello::peniko::{
        BlendMode, Brush, Color, ColorStop, Compose, Extend, Fill, Gradient, Mix,
    };

    const RED: Color = Color::from_rgba8(255, 0, 0, 255);
    const BLUE: Color = Color::from_rgba8(0, 0, 255, 255);

    fn parse(body: &str) -> usvg::Tree {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">{body}</svg>"#
        );
        usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("a valid test document")
    }

    fn encoded(tree: &usvg::Tree) -> Scene {
        let mut scene = Scene::new();
        encode(tree, &mut scene);
        scene
    }

    /// Every path in the tree, in document order, through groups.
    fn paths(group: &usvg::Group) -> Vec<&usvg::Path> {
        let mut found = Vec::new();
        for node in group.children() {
            match node {
                usvg::Node::Path(path) => found.push(path.as_ref()),
                usvg::Node::Group(child) => found.extend(paths(child)),
                usvg::Node::Image(_) | usvg::Node::Text(_) => {}
            }
        }
        found
    }

    /// The root's only child, which these documents make a group.
    fn only_group(tree: &usvg::Tree) -> &usvg::Group {
        match tree.root().children() {
            [usvg::Node::Group(group)] => group,
            other => panic!("expected one group, found {other:?}"),
        }
    }

    fn rect_of(bounds: usvg::NonZeroRect) -> Rect {
        Rect::new(
            f64::from(bounds.left()),
            f64::from(bounds.top()),
            f64::from(bounds.right()),
            f64::from(bounds.bottom()),
        )
    }

    fn solid_fill(
        scene: &mut Scene,
        rule: Fill,
        transform: Affine,
        color: Color,
        path: &usvg::Path,
    ) {
        scene.fill(
            rule,
            transform,
            &Brush::Solid(color),
            None,
            &bez_path(path.data()),
        );
    }

    fn normal() -> BlendMode {
        BlendMode::new(Mix::Normal, Compose::SrcOver)
    }

    #[test]
    fn a_path_fills_with_its_own_fill_rule() {
        for (rule, fill) in [("evenodd", Fill::EvenOdd), ("nonzero", Fill::NonZero)] {
            let tree = parse(&format!(
                r##"<path d="M0 0 H50 V50 H0 Z M10 10 H40 V40 H10 Z" fill-rule="{rule}" fill="#ff0000"/>"##
            ));
            let mut expected = Scene::new();
            solid_fill(
                &mut expected,
                fill,
                Affine::IDENTITY,
                RED,
                paths(tree.root())[0],
            );
            assert_scenes_identical(&encoded(&tree), &expected);
        }
    }

    #[test]
    fn solid_paint_folds_the_paint_opacity_into_rgba8() {
        let tree = parse(r##"<rect width="10" height="10" fill="#ff0000" fill-opacity="0.5"/>"##);
        let mut expected = Scene::new();
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgba8(255, 0, 0, 128),
            paths(tree.root())[0],
        );
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    #[test]
    fn a_stroke_carries_width_caps_join_miter_and_dashes() {
        let tree = parse(
            r##"<path d="M0 10 L90 10 L90 60" fill="none" stroke="#0000ff" stroke-width="4"
                stroke-linecap="round" stroke-linejoin="bevel" stroke-miterlimit="7"
                stroke-dasharray="5 3" stroke-dashoffset="2"/>"##,
        );
        let path = paths(tree.root())[0];
        let mut expected = Scene::new();
        expected.stroke(
            &Stroke::new(4.0)
                .with_caps(Cap::Round)
                .with_join(Join::Bevel)
                .with_miter_limit(7.0)
                .with_dashes(2.0, [5.0, 3.0]),
            Affine::IDENTITY,
            &Brush::Solid(BLUE),
            None,
            &bez_path(path.data()),
        );
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    #[test]
    fn paint_order_stroke_draws_the_stroke_first() {
        let tree = parse(
            r##"<rect width="10" height="10" fill="#ff0000" stroke="#0000ff" stroke-width="2"
                paint-order="stroke"/>"##,
        );
        let path = paths(tree.root())[0];
        let shape = bez_path(path.data());
        let mut expected = Scene::new();
        expected.stroke(
            // SVG's initial cap, join and miter limit, none of which is
            // kurbo's default.
            &Stroke::new(2.0)
                .with_caps(Cap::Butt)
                .with_join(Join::Miter)
                .with_miter_limit(4.0),
            Affine::IDENTITY,
            &Brush::Solid(BLUE),
            None,
            &shape,
        );
        expected.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &Brush::Solid(RED),
            None,
            &shape,
        );
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    #[test]
    fn an_invisible_path_and_a_pattern_draw_nothing() {
        let hidden =
            parse(r##"<rect width="10" height="10" fill="#ff0000" visibility="hidden"/>"##);
        assert!(encoded(&hidden).encoding().is_empty());
        let patterned = parse(
            r##"<defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse">
                  <rect width="2" height="2" fill="#ff0000"/></pattern></defs>
                <rect width="10" height="10" fill="url(#p)"/>"##,
        );
        assert!(encoded(&patterned).encoding().is_empty());
    }

    fn stops(colors: &[(f32, Color)]) -> Vec<ColorStop> {
        colors
            .iter()
            .map(|(offset, color)| ColorStop {
                offset: *offset,
                color: DynamicColor::from_alpha_color(*color),
            })
            .collect()
    }

    #[test]
    fn a_linear_gradient_maps_each_spread_method_and_folds_stop_opacity() {
        for (method, extend) in [
            ("pad", Extend::Pad),
            ("reflect", Extend::Reflect),
            ("repeat", Extend::Repeat),
        ] {
            let tree = parse(&format!(
                r##"<defs><linearGradient id="g" gradientUnits="userSpaceOnUse"
                      x1="10" y1="0" x2="40" y2="0" spreadMethod="{method}">
                      <stop offset="0" stop-color="#ff0000"/>
                      <stop offset="1" stop-color="#0000ff" stop-opacity="0.5"/>
                    </linearGradient></defs>
                    <rect width="100" height="20" fill="url(#g)" fill-opacity="0.5"/>"##
            ));
            let gradient = Gradient::new_linear((10.0, 0.0), (40.0, 0.0))
                .with_extend(extend)
                .with_stops(
                    stops(&[
                        (0.0, Color::from_rgba8(255, 0, 0, 128)),
                        (1.0, Color::from_rgba8(0, 0, 255, 64)),
                    ])
                    .as_slice(),
                );
            let mut expected = Scene::new();
            expected.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                &Brush::Gradient(gradient),
                Some(Affine::IDENTITY),
                &bez_path(paths(tree.root())[0].data()),
            );
            assert_scenes_identical(&encoded(&tree), &expected);
        }
    }

    /// SVG's focal circle is where the gradient starts, so it is peniko's
    /// start circle; the outer circle is the end circle.
    #[test]
    fn a_radial_gradient_starts_at_the_focal_circle() {
        let tree = parse(
            r##"<defs><radialGradient id="g" gradientUnits="userSpaceOnUse"
                  cx="50" cy="50" r="40" fx="40" fy="45" fr="5" spreadMethod="repeat"
                  gradientTransform="translate(3 4)">
                  <stop offset="0" stop-color="#ff0000"/>
                  <stop offset="1" stop-color="#0000ff"/>
                </radialGradient></defs>
                <rect width="100" height="100" fill="url(#g)"/>"##,
        );
        let gradient = Gradient::new_two_point_radial((40.0, 45.0), 5.0, (50.0, 50.0), 40.0)
            .with_extend(Extend::Repeat)
            .with_stops(stops(&[(0.0, RED), (1.0, BLUE)]).as_slice());
        let mut expected = Scene::new();
        expected.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &Brush::Gradient(gradient),
            Some(Affine::translate((3.0, 4.0))),
            &bez_path(paths(tree.root())[0].data()),
        );
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    #[test]
    fn group_opacity_is_one_layer_over_the_layer_bounds() {
        let tree = parse(
            r##"<g opacity="0.5" transform="translate(5 6)">
                  <rect x="10" y="10" width="20" height="20" fill="#ff0000"/>
                </g>"##,
        );
        let group = only_group(&tree);
        let mut expected = Scene::new();
        expected.push_layer(
            Fill::NonZero,
            normal(),
            0.5,
            Affine::translate((5.0, 6.0)),
            &rect_of(group.layer_bounding_box()),
        );
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::translate((5.0, 6.0)),
            RED,
            paths(tree.root())[0],
        );
        expected.pop_layer();
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    #[test]
    fn a_blend_mode_is_the_group_layer_blend() {
        let tree = parse(
            r##"<g style="mix-blend-mode:multiply">
                  <rect width="20" height="20" fill="#ff0000"/>
                </g>"##,
        );
        let group = only_group(&tree);
        let mut expected = Scene::new();
        expected.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Multiply, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &rect_of(group.layer_bounding_box()),
        );
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            paths(tree.root())[0],
        );
        expected.pop_layer();
        assert_scenes_identical(&encoded(&tree), &expected);
        assert!(
            opens_blend(tree.root()),
            "a draw of this image must isolate it"
        );
    }

    #[test]
    fn isolation_alone_and_a_filter_push_nothing() {
        for attribute in [r#"style="isolation:isolate""#, r#"filter="url(#f)""#] {
            let tree = parse(&format!(
                r##"<defs><filter id="f"><feGaussianBlur stdDeviation="2"/></filter></defs>
                    <g {attribute}><rect width="20" height="20" fill="#ff0000"/></g>"##
            ));
            assert!(only_group(&tree).should_isolate(), "{attribute} isolates");
            let mut expected = Scene::new();
            solid_fill(
                &mut expected,
                Fill::NonZero,
                Affine::IDENTITY,
                RED,
                paths(tree.root())[0],
            );
            assert_scenes_identical(&encoded(&tree), &expected);
        }
    }

    #[test]
    fn a_masked_group_is_skipped_and_its_siblings_still_draw() {
        let tree = parse(
            r##"<defs><mask id="m"><rect width="10" height="10" fill="#ffffff"/></mask></defs>
                <g mask="url(#m)"><rect width="20" height="20" fill="#ff0000"/></g>
                <rect y="30" width="20" height="20" fill="#0000ff"/>"##,
        );
        let blue = *paths(tree.root()).last().expect("the sibling");
        let mut expected = Scene::new();
        solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, BLUE, blue);
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    /// A one-path clip on a group with opacity 1 is a clip layer at
    /// `group.abs_transform() * clip.transform() * child.abs_transform()`,
    /// under the child's `clip-rule`.
    #[test]
    fn a_single_path_clip_is_a_clip_layer_under_the_full_transform_product() {
        let tree = parse(
            r##"<defs><clipPath id="c" transform="translate(5 5)">
                  <rect x="1" y="2" width="30" height="30" clip-rule="evenodd"/>
                </clipPath></defs>
                <g clip-path="url(#c)" transform="scale(2)">
                  <rect width="40" height="40" fill="#ff0000"/>
                </g>"##,
        );
        let group = only_group(&tree);
        let clip = group.clip_path().expect("the clip");
        let clip_child = paths(clip.root())[0];
        let mut expected = Scene::new();
        expected.push_clip_layer(
            Fill::EvenOdd,
            Affine::scale(2.0) * Affine::translate((5.0, 5.0)) * affine(clip_child.abs_transform()),
            &bez_path(clip_child.data()),
        );
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::scale(2.0),
            RED,
            paths(tree.root())[0],
        );
        expected.pop_layer();
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    /// With opacity, a one-path clip is the compositing layer's own shape.
    #[test]
    fn a_single_path_clip_with_opacity_is_the_layer_shape() {
        let tree = parse(
            r##"<defs><clipPath id="c"><circle cx="20" cy="20" r="10"/></clipPath></defs>
                <g clip-path="url(#c)" opacity="0.25">
                  <rect width="40" height="40" fill="#ff0000"/>
                </g>"##,
        );
        let clip = only_group(&tree).clip_path().expect("the clip");
        let clip_child = paths(clip.root())[0];
        let mut expected = Scene::new();
        expected.push_layer(
            Fill::NonZero,
            normal(),
            0.25,
            affine(clip.transform()) * affine(clip_child.abs_transform()),
            &bez_path(clip_child.data()),
        );
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            paths(tree.root())[0],
        );
        expected.pop_layer();
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    fn concatenated(clip: &usvg::ClipPath) -> BezPath {
        let mut shape = BezPath::new();
        for path in paths(clip.root()) {
            let mut part = bez_path(path.data());
            part.apply_affine(affine(path.abs_transform()));
            shape.extend(part);
        }
        shape
    }

    /// Several children clip with their concatenation; with opacity the
    /// layer takes the bounds and the clip sits inside it.
    #[test]
    fn a_multi_child_clip_concatenates_its_children() {
        for opacity in ["1", "0.5"] {
            let tree = parse(&format!(
                r##"<defs><clipPath id="c">
                      <rect width="10" height="10"/>
                      <rect x="20" y="20" width="10" height="10" transform="rotate(10)"/>
                    </clipPath></defs>
                    <g clip-path="url(#c)" opacity="{opacity}">
                      <rect width="40" height="40" fill="#ff0000"/>
                    </g>"##
            ));
            let group = only_group(&tree);
            let clip = group.clip_path().expect("the clip");
            assert_eq!(clip.root().children().len(), 2);
            let mut expected = Scene::new();
            if opacity == "0.5" {
                expected.push_layer(
                    Fill::NonZero,
                    normal(),
                    0.5,
                    Affine::IDENTITY,
                    &rect_of(group.layer_bounding_box()),
                );
            }
            expected.push_clip_layer(Fill::NonZero, affine(clip.transform()), &concatenated(clip));
            solid_fill(
                &mut expected,
                Fill::NonZero,
                Affine::IDENTITY,
                RED,
                paths(tree.root())[0],
            );
            expected.pop_layer();
            if opacity == "0.5" {
                expected.pop_layer();
            }
            assert_scenes_identical(&encoded(&tree), &expected);
        }
    }

    /// A `clipPath` that is itself clipped pushes that clip first, under the
    /// same group transform.
    #[test]
    fn a_clipped_clip_path_pushes_its_own_clip_first() {
        let tree = parse(
            r##"<defs>
                  <clipPath id="outer"><rect width="15" height="40"/></clipPath>
                  <clipPath id="inner" clip-path="url(#outer)"><rect width="40" height="15"/></clipPath>
                </defs>
                <g clip-path="url(#inner)"><rect width="40" height="40" fill="#ff0000"/></g>"##,
        );
        let inner = only_group(&tree).clip_path().expect("the clip");
        let outer = inner.clip_path().expect("the clip's clip");
        let mut expected = Scene::new();
        for clip in [outer, inner] {
            let child = paths(clip.root())[0];
            expected.push_clip_layer(
                Fill::NonZero,
                affine(clip.transform()) * affine(child.abs_transform()),
                &bez_path(child.data()),
            );
        }
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            paths(tree.root())[0],
        );
        expected.pop_layer();
        expected.pop_layer();
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    /// A blend group directly inside a clip-only group would open a blend
    /// layer inside a clip layer (vello #1198), so the clip becomes a full
    /// `Normal` layer.
    #[test]
    fn a_clip_around_a_blend_group_is_a_full_layer() {
        let tree = parse(
            r##"<defs><clipPath id="c"><rect width="30" height="30"/></clipPath></defs>
                <g clip-path="url(#c)">
                  <g style="mix-blend-mode:screen"><rect width="20" height="20" fill="#ff0000"/></g>
                </g>"##,
        );
        let group = only_group(&tree);
        let clip = group.clip_path().expect("the clip");
        let clip_child = paths(clip.root())[0];
        let blend = match group.children() {
            [usvg::Node::Group(blend)] => blend,
            other => panic!("expected the blend group, found {other:?}"),
        };
        let mut expected = Scene::new();
        expected.push_layer(
            Fill::NonZero,
            normal(),
            1.0,
            affine(clip.transform()) * affine(clip_child.abs_transform()),
            &bez_path(clip_child.data()),
        );
        expected.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Screen, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &rect_of(blend.layer_bounding_box()),
        );
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            paths(tree.root())[0],
        );
        expected.pop_layer();
        expected.pop_layer();
        assert_scenes_identical(&encoded(&tree), &expected);
        assert!(
            !opens_blend(tree.root()),
            "the blend sits inside the clip's own layer, not at the image's top level"
        );
    }

    /// The only child group of `group`.
    fn only_child_group(group: &usvg::Group) -> &usvg::Group {
        match group.children() {
            [usvg::Node::Group(child)] => child,
            other => panic!("expected one child group, found {other:?}"),
        }
    }

    /// An `isolation: isolate` group pushes no layer, so a blend group
    /// inside it still opens directly in the clip layer around both; the
    /// clip becomes a full `Normal` layer exactly as with no group between.
    #[test]
    fn a_clip_around_an_isolated_group_around_a_blend_group_is_a_full_layer() {
        let tree = parse(
            r##"<defs><clipPath id="c"><rect width="30" height="30"/></clipPath></defs>
                <g clip-path="url(#c)">
                  <g style="isolation:isolate">
                    <g style="mix-blend-mode:screen"><rect width="20" height="20" fill="#ff0000"/></g>
                  </g>
                </g>"##,
        );
        let group = only_group(&tree);
        let clip = group.clip_path().expect("the clip");
        let clip_child = paths(clip.root())[0];
        let isolated = only_child_group(group);
        assert!(isolated.should_isolate(), "the middle group is kept");
        let blend = only_child_group(isolated);
        let mut expected = Scene::new();
        expected.push_layer(
            Fill::NonZero,
            normal(),
            1.0,
            affine(clip.transform()) * affine(clip_child.abs_transform()),
            &bez_path(clip_child.data()),
        );
        expected.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Screen, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &rect_of(blend.layer_bounding_box()),
        );
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            paths(tree.root())[0],
        );
        expected.pop_layer();
        expected.pop_layer();
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    /// At the image root, a blend group under an `isolation: isolate` group
    /// opens in whatever layer a draw of the image pushes, so the image
    /// counts as opening a blend (`background.rs` promotes its tile layers).
    #[test]
    fn a_blend_under_an_isolated_group_at_the_root_opens_a_blend() {
        let tree = parse(
            r##"<g style="isolation:isolate">
                  <g style="mix-blend-mode:screen"><rect width="20" height="20" fill="#ff0000"/></g>
                </g>"##,
        );
        let blend = only_child_group(only_group(&tree));
        let mut expected = Scene::new();
        expected.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Screen, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &rect_of(blend.layer_bounding_box()),
        );
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            paths(tree.root())[0],
        );
        expected.pop_layer();
        assert_scenes_identical(&encoded(&tree), &expected);
        assert!(
            opens_blend(tree.root()),
            "the isolated group pushes no layer between the draw and the blend"
        );
    }

    /// A clipped `clipPath` around a blend group promotes only the innermost
    /// layer, the one that encloses the children: the inner clip when it is
    /// a clip layer of its own, the outer clip when the inner one is the
    /// group's compositing layer.
    #[test]
    fn only_the_innermost_clip_layer_around_a_blend_is_full() {
        for opacity in ["1", "0.5"] {
            let tree = parse(&format!(
                r##"<defs>
                      <clipPath id="outer"><rect width="15" height="40"/></clipPath>
                      <clipPath id="inner" clip-path="url(#outer)"><rect width="40" height="15"/></clipPath>
                    </defs>
                    <g clip-path="url(#inner)" opacity="{opacity}">
                      <g style="mix-blend-mode:screen"><rect width="20" height="20" fill="#ff0000"/></g>
                    </g>"##
            ));
            let group = only_group(&tree);
            let inner = group.clip_path().expect("the clip");
            let outer = inner.clip_path().expect("the clip's clip");
            let blend = only_child_group(group);
            let place = |clip: &usvg::ClipPath| {
                let child = paths(clip.root())[0];
                (
                    affine(clip.transform()) * affine(child.abs_transform()),
                    bez_path(child.data()),
                )
            };
            let (outer_transform, outer_shape) = place(outer);
            let (inner_transform, inner_shape) = place(inner);
            let mut expected = Scene::new();
            if opacity == "1" {
                expected.push_clip_layer(Fill::NonZero, outer_transform, &outer_shape);
                expected.push_layer(Fill::NonZero, normal(), 1.0, inner_transform, &inner_shape);
            } else {
                expected.push_layer(Fill::NonZero, normal(), 0.5, inner_transform, &inner_shape);
                expected.push_layer(Fill::NonZero, normal(), 1.0, outer_transform, &outer_shape);
            }
            expected.push_layer(
                Fill::NonZero,
                BlendMode::new(Mix::Screen, Compose::SrcOver),
                1.0,
                Affine::IDENTITY,
                &rect_of(blend.layer_bounding_box()),
            );
            solid_fill(
                &mut expected,
                Fill::NonZero,
                Affine::IDENTITY,
                RED,
                paths(tree.root())[0],
            );
            expected.pop_layer();
            expected.pop_layer();
            expected.pop_layer();
            assert_scenes_identical(&encoded(&tree), &expected);
        }
    }

    fn collect_images<'a>(group: &'a usvg::Group, found: &mut Vec<&'a usvg::Image>) {
        for node in group.children() {
            match node {
                usvg::Node::Image(image) => found.push(image),
                usvg::Node::Group(child) => collect_images(child, found),
                usvg::Node::Path(_) | usvg::Node::Text(_) => {}
            }
        }
    }

    /// A nested SVG image renders its own tree, placed by the image's
    /// transform; a nested raster image draws nothing.
    #[test]
    fn a_nested_svg_image_renders_and_a_nested_raster_image_does_not() {
        let tree = parse(
            r#"<image x="10" y="20" width="40" height="40"
                  href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='10'%3E%3Crect width='10' height='10' fill='%23ff0000'/%3E%3C/svg%3E"/>
                <image x="0" y="0" width="10" height="10"
                  href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="/>"#,
        );
        let mut images = Vec::new();
        collect_images(tree.root(), &mut images);
        assert_eq!(images.len(), 2, "both images parse");
        let usvg::ImageKind::SVG(nested) = images[0].kind() else {
            panic!("the first image is an SVG document");
        };
        assert_eq!(
            affine(images[0].abs_transform()),
            Affine::new([4.0, 0.0, 0.0, 4.0, 10.0, 20.0]),
            "the 10x10 document fills its 40x40 box"
        );
        let mut expected = Scene::new();
        solid_fill(
            &mut expected,
            Fill::NonZero,
            affine(images[0].abs_transform()),
            RED,
            paths(nested.root())[0],
        );
        assert_scenes_identical(&encoded(&tree), &expected);
    }

    #[test]
    fn an_empty_document_encodes_nothing() {
        assert!(encoded(&parse("")).encoding().is_empty());
        assert!(
            encoded(&parse(r#"<text x="0" y="10">dropped at parse</text>"#))
                .encoding()
                .is_empty(),
            "text is dropped with usvg's `text` feature off"
        );
    }

    /// A segment after `Close` starts at the closed subpath's first point,
    /// so the kurbo path moves there before it.
    #[test]
    fn a_segment_after_close_starts_at_the_subpath_start() {
        let tree = parse(
            r##"<path d="M5 5 L10 5 L10 10 Z L20 20 M30 30 L40 30 Z M50 50 L60 60" stroke="#000000"/>"##,
        );
        let elements = bez_path(paths(tree.root())[0].data()).elements().to_vec();
        assert_eq!(
            elements,
            [
                PathEl::MoveTo(Point::new(5.0, 5.0)),
                PathEl::LineTo(Point::new(10.0, 5.0)),
                PathEl::LineTo(Point::new(10.0, 10.0)),
                PathEl::ClosePath,
                PathEl::MoveTo(Point::new(5.0, 5.0)),
                PathEl::LineTo(Point::new(20.0, 20.0)),
                PathEl::MoveTo(Point::new(30.0, 30.0)),
                PathEl::LineTo(Point::new(40.0, 30.0)),
                PathEl::ClosePath,
                PathEl::MoveTo(Point::new(50.0, 50.0)),
                PathEl::LineTo(Point::new(60.0, 60.0)),
            ]
        );
    }

    #[test]
    fn a_vector_image_builds_its_scene_once() {
        let image = crate::VectorImage::new(
            std::sync::Arc::new(parse(r##"<rect width="10" height="10" fill="#ff0000"/>"##)),
            (100, 100),
            (100.0, 100.0),
        );
        let first = std::sync::Arc::clone(image.scene());
        assert!(!first.encoding().is_empty());
        assert!(std::sync::Arc::ptr_eq(&first, image.scene()));
    }
}
