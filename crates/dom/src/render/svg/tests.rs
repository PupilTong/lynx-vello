//! Each test parses a hand-written document and compares the converter's
//! scene with one built from the vello calls the rule names, through
//! `assert_scenes_identical`, which compares every encoding stream, or
//! reads the parsed items directly.

#![allow(clippy::float_cmp)]

use std::sync::Arc;

use super::shapes::{self, ARC_TOLERANCE};
use super::{Item, LayerClip, SvgError, VectorDocument, encode, opens_blend, parse};
use crate::paint::equivalence::assert_scenes_identical;
use crate::render::image::{AspectAlign, AspectRatio};
use crate::vello::Scene;
use crate::vello::kurbo::{
    Affine, BezPath, Cap, Circle, Ellipse, Join, PathEl, Point, Rect, Shape, Stroke,
};
use crate::vello::peniko::color::DynamicColor;
use crate::vello::peniko::{
    BlendMode, Brush, Color, ColorStop, Compose, Extend, Fill, Gradient, Mix,
};

const RED: Color = Color::from_rgba8(255, 0, 0, 255);
const BLUE: Color = Color::from_rgba8(0, 0, 255, 255);

fn document(body: &str) -> VectorDocument {
    let svg =
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">{body}</svg>"#);
    parse(svg.as_bytes()).expect("a valid test document")
}

fn encoded(parsed: &VectorDocument) -> Scene {
    encode(parsed)
}

/// Every path item, in order, with its transform.
fn paths(parsed: &VectorDocument) -> Vec<(&BezPath, Affine)> {
    parsed
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Path {
                shape, transform, ..
            } => Some((shape, *transform)),
            _ => None,
        })
        .collect()
}

fn rect_path(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    Rect::new(x0, y0, x1, y1).to_path(ARC_TOLERANCE)
}

fn solid_fill(scene: &mut Scene, rule: Fill, transform: Affine, color: Color, shape: &BezPath) {
    scene.fill(rule, transform, &Brush::Solid(color), None, shape);
}

fn normal() -> BlendMode {
    BlendMode::new(Mix::Normal, Compose::SrcOver)
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

// --- Paths and shapes -------------------------------------------------

#[test]
fn a_path_fills_with_its_own_fill_rule() {
    for (rule, fill) in [("evenodd", Fill::EvenOdd), ("nonzero", Fill::NonZero)] {
        let parsed = document(&format!(
            r##"<path d="M0 0 H50 V50 H0 Z M10 10 H40 V40 H10 Z" fill-rule="{rule}" fill="#ff0000"/>"##
        ));
        let (shape, transform) = paths(&parsed)[0];
        assert_eq!(transform, Affine::IDENTITY);
        assert_eq!(
            shape.elements(),
            [
                PathEl::MoveTo(Point::new(0.0, 0.0)),
                PathEl::LineTo(Point::new(50.0, 0.0)),
                PathEl::LineTo(Point::new(50.0, 50.0)),
                PathEl::LineTo(Point::new(0.0, 50.0)),
                PathEl::ClosePath,
                PathEl::MoveTo(Point::new(10.0, 10.0)),
                PathEl::LineTo(Point::new(40.0, 10.0)),
                PathEl::LineTo(Point::new(40.0, 40.0)),
                PathEl::LineTo(Point::new(10.0, 40.0)),
                PathEl::ClosePath,
            ]
        );
        let mut expected = Scene::new();
        solid_fill(&mut expected, fill, Affine::IDENTITY, RED, shape);
        assert_scenes_identical(&encoded(&parsed), &expected);
    }
}

#[test]
fn solid_paint_folds_the_paint_opacity_into_rgba8() {
    let parsed = document(r##"<rect width="10" height="10" fill="#ff0000" fill-opacity="0.5"/>"##);
    let mut expected = Scene::new();
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgba8(255, 0, 0, 128),
        &rect_path(0.0, 0.0, 10.0, 10.0),
    );
    assert_scenes_identical(&encoded(&parsed), &expected);
}

#[test]
fn a_stroke_carries_width_caps_join_miter_and_dashes() {
    let parsed = document(
        r##"<path d="M0 10 L90 10 L90 60" fill="none" stroke="#0000ff" stroke-width="4"
            stroke-linecap="round" stroke-linejoin="bevel" stroke-miterlimit="7"
            stroke-dasharray="5 3" stroke-dashoffset="2"/>"##,
    );
    let (shape, _) = paths(&parsed)[0];
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
        shape,
    );
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// An odd dash list is repeated to an even one, as SVG asks, and a zero
/// `stroke-width` strokes nothing.
#[test]
fn odd_dash_lists_double_and_a_zero_width_strokes_nothing() {
    let parsed = document(
        r##"<rect width="10" height="10" fill="none" stroke="#0000ff" stroke-dasharray="1 2 3"/>"##,
    );
    let Item::Path {
        stroke: Some(stroke),
        ..
    } = &parsed.items[0]
    else {
        panic!("a stroked path");
    };
    assert_eq!(
        stroke.style.dash_pattern.as_slice(),
        [1.0, 2.0, 3.0, 1.0, 2.0, 3.0]
    );
    let unstroked = document(
        r##"<rect width="10" height="10" fill="none" stroke="#0000ff" stroke-width="0"/>"##,
    );
    assert!(
        unstroked.items.is_empty(),
        "no fill and no stroke draws nothing"
    );
}

#[test]
fn paint_order_stroke_draws_the_stroke_first() {
    let parsed = document(
        r##"<rect width="10" height="10" fill="#ff0000" stroke="#0000ff" stroke-width="2"
            paint-order="stroke"/>"##,
    );
    let shape = rect_path(0.0, 0.0, 10.0, 10.0);
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
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, RED, &shape);
    assert_scenes_identical(&encoded(&parsed), &expected);
}

#[test]
fn an_invisible_path_and_a_pattern_draw_nothing() {
    let hidden = document(r##"<rect width="10" height="10" fill="#ff0000" visibility="hidden"/>"##);
    assert!(hidden.items.is_empty());
    let patterned = document(
        r##"<defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse">
              <rect width="2" height="2" fill="#ff0000"/></pattern></defs>
            <rect width="10" height="10" fill="url(#p)"/>"##,
    );
    assert!(patterned.items.is_empty());
    assert!(encoded(&patterned).encoding().is_empty());
}

/// `visibility` inherits but a child may turn it back on; `display: none`
/// takes the subtree with it.
#[test]
fn visibility_and_display_follow_their_inheritance() {
    let parsed = document(
        r##"<g visibility="hidden">
              <rect width="10" height="10" fill="#ff0000"/>
              <rect width="10" height="10" fill="#0000ff" visibility="visible"/>
            </g>
            <g display="none"><rect width="10" height="10" fill="#ff0000"/></g>"##,
    );
    let mut expected = Scene::new();
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        BLUE,
        &rect_path(0.0, 0.0, 10.0, 10.0),
    );
    assert_scenes_identical(&encoded(&parsed), &expected);
}

#[test]
fn basic_shapes_become_their_paths() {
    let parsed = document(
        r#"<rect x="1" y="2" width="10" height="20" rx="3"/>
            <circle cx="5" cy="6" r="4"/>
            <ellipse cx="5" cy="6" rx="4"/>
            <line x1="0" y1="1" x2="2" y2="3"/>
            <polyline points="0,0 10,0 10,10"/>
            <polygon points="0 0 10 0 10 10"/>"#,
    );
    let shapes = paths(&parsed);
    assert_eq!(shapes.len(), 6);
    let rounded = shapes::rect(1.0, 2.0, 10.0, 20.0, Some(3.0), None).expect("a rect");
    assert_eq!(shapes[0].0, &rounded);
    assert_eq!(
        rounded.bounding_box(),
        Rect::new(1.0, 2.0, 11.0, 22.0),
        "the corner arcs stay inside the rect"
    );
    assert_eq!(
        shapes[1].0,
        &Circle::new((5.0, 6.0), 4.0).to_path(ARC_TOLERANCE)
    );
    assert_eq!(
        shapes[2].0,
        &Ellipse::new((5.0, 6.0), (4.0, 4.0), 0.0).to_path(ARC_TOLERANCE),
        "a missing ry takes rx"
    );
    assert_eq!(
        shapes[3].0.elements(),
        [
            PathEl::MoveTo(Point::new(0.0, 1.0)),
            PathEl::LineTo(Point::new(2.0, 3.0))
        ]
    );
    assert_eq!(
        shapes[4].0.elements(),
        [
            PathEl::MoveTo(Point::new(0.0, 0.0)),
            PathEl::LineTo(Point::new(10.0, 0.0)),
            PathEl::LineTo(Point::new(10.0, 10.0)),
        ]
    );
    assert_eq!(shapes[5].0.elements().last(), Some(&PathEl::ClosePath));
}

/// A segment after `Close` starts at the closed subpath's first point,
/// so the kurbo path moves there before it.
#[test]
fn a_segment_after_close_starts_at_the_subpath_start() {
    let elements = shapes::path_data("M5 5 L10 5 L10 10 Z L20 20 M30 30 L40 30 Z M50 50 L60 60")
        .elements()
        .to_vec();
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

/// Relative commands, smooth curves and arcs: a malformed tail renders up
/// to the error.
#[test]
fn path_data_resolves_relative_smooth_and_arc_segments() {
    let path =
        shapes::path_data("m10 10 l5 0 c0 0 5 5 5 5 s5 5 5 5 q0 5 5 5 t5 5 a5 5 0 0 1 10 0 z");
    assert_eq!(path.elements()[0], PathEl::MoveTo(Point::new(10.0, 10.0)));
    assert_eq!(path.elements()[1], PathEl::LineTo(Point::new(15.0, 10.0)));
    assert_eq!(
        path.elements()[2],
        PathEl::CurveTo(
            Point::new(15.0, 10.0),
            Point::new(20.0, 15.0),
            Point::new(20.0, 15.0)
        )
    );
    // `s` reflects the previous cubic's second control point through the
    // current point.
    assert_eq!(
        path.elements()[3],
        PathEl::CurveTo(
            Point::new(20.0, 15.0),
            Point::new(25.0, 20.0),
            Point::new(25.0, 20.0)
        )
    );
    assert_eq!(
        path.elements()[4],
        PathEl::QuadTo(Point::new(25.0, 25.0), Point::new(30.0, 25.0))
    );
    assert_eq!(
        path.elements()[5],
        PathEl::QuadTo(Point::new(35.0, 25.0), Point::new(35.0, 30.0))
    );
    assert!(
        path.elements()[6..]
            .iter()
            .any(|element| matches!(element, PathEl::CurveTo(..))),
        "the arc is cubics"
    );
    assert_eq!(path.elements().last(), Some(&PathEl::ClosePath));
    let truncated = shapes::path_data("M0 0 L10 10 L x");
    assert_eq!(truncated.elements().len(), 2, "up to the error");
}

/// Lengths: percentages of the viewport axes, `em` of the element's font
/// size, absolute units at 96 px per inch.
#[test]
fn lengths_resolve_against_the_viewport_and_the_font_size() {
    let parsed = document(
        r##"<rect x="10%" y="1in" width="50%" height="1em" font-size="12" stroke="#000000" stroke-width="2em"/>"##,
    );
    let (shape, _) = paths(&parsed)[0];
    assert_eq!(shape.bounding_box(), Rect::new(10.0, 96.0, 60.0, 108.0));
    let Item::Path {
        stroke: Some(stroke),
        ..
    } = &parsed.items[0]
    else {
        panic!("a stroked path");
    };
    assert_eq!(stroke.style.width, 24.0);
}

// --- Paint servers --------------------------------------------------------

#[test]
fn a_linear_gradient_maps_each_spread_method_and_folds_stop_opacity() {
    for (method, extend) in [
        ("pad", Extend::Pad),
        ("reflect", Extend::Reflect),
        ("repeat", Extend::Repeat),
    ] {
        let parsed = document(&format!(
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
            &rect_path(0.0, 0.0, 100.0, 20.0),
        );
        assert_scenes_identical(&encoded(&parsed), &expected);
    }
}

/// SVG's focal circle is where the gradient starts, so it is peniko's
/// start circle; the outer circle is the end circle.
#[test]
fn a_radial_gradient_starts_at_the_focal_circle() {
    let parsed = document(
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
        &rect_path(0.0, 0.0, 100.0, 100.0),
    );
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// `objectBoundingBox` units (the default) are fractions of the shape's
/// bounding box, so the brush transform maps the unit square onto it.
#[test]
fn an_object_bounding_box_gradient_maps_the_unit_square_onto_the_shape() {
    let parsed = document(
        r##"<defs><linearGradient id="g" x1="0" y1="0" x2="100%" y2="1">
              <stop offset="0" stop-color="#ff0000"/>
              <stop offset="1" stop-color="#0000ff"/>
            </linearGradient></defs>
            <rect x="10" y="20" width="40" height="20" fill="url(#g)"/>"##,
    );
    let gradient = Gradient::new_linear((0.0, 0.0), (1.0, 1.0))
        .with_stops(stops(&[(0.0, RED), (1.0, BLUE)]).as_slice());
    let mut expected = Scene::new();
    expected.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        &Brush::Gradient(gradient),
        Some(Affine::translate((10.0, 20.0)) * Affine::scale_non_uniform(40.0, 20.0)),
        &rect_path(10.0, 20.0, 50.0, 40.0),
    );
    assert_scenes_identical(&encoded(&parsed), &expected);
    // A shape with no area disables such a paint.
    let flat = document(
        r##"<defs><linearGradient id="g"><stop offset="0" stop-color="#ff0000"/>
            <stop offset="1" stop-color="#0000ff"/></linearGradient></defs>
            <line x1="0" y1="5" x2="50" y2="5" fill="none" stroke="url(#g)"/>"##,
    );
    assert!(flat.items.is_empty());
}

/// A gradient inherits what it does not declare, stops included, along
/// its `href` chain; its own attributes win.
#[test]
fn a_gradient_href_chain_supplies_stops_and_attributes() {
    let parsed = document(
        r##"<defs>
              <linearGradient id="stops" spreadMethod="reflect">
                <stop offset="0" stop-color="#ff0000"/>
                <stop offset="1" stop-color="#0000ff"/>
              </linearGradient>
              <linearGradient id="mid" href="#stops" gradientUnits="userSpaceOnUse" x2="30"/>
              <linearGradient id="g" xlink:href="#mid" xmlns:xlink="http://www.w3.org/1999/xlink" y2="10"/>
            </defs>
            <rect width="100" height="20" fill="url(#g)"/>"##,
    );
    let gradient = Gradient::new_linear((0.0, 0.0), (30.0, 10.0))
        .with_extend(Extend::Reflect)
        .with_stops(stops(&[(0.0, RED), (1.0, BLUE)]).as_slice());
    let mut expected = Scene::new();
    expected.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        &Brush::Gradient(gradient),
        Some(Affine::IDENTITY),
        &rect_path(0.0, 0.0, 100.0, 20.0),
    );
    assert_scenes_identical(&encoded(&parsed), &expected);
    let cyclic = document(
        r##"<defs><linearGradient id="a" href="#b"/><linearGradient id="b" href="#a"/></defs>
            <rect width="10" height="10" fill="url(#a)"/>"##,
    );
    assert!(cyclic.items.is_empty(), "a cycle ends with no stops");
}

/// One stop is a solid colour; a missing reference paints its fallback.
#[test]
fn a_one_stop_gradient_and_a_fallback_are_solid() {
    let parsed = document(
        r##"<defs><linearGradient id="g"><stop offset="0" stop-color="#ff0000" stop-opacity="0.5"/></linearGradient></defs>
            <rect width="10" height="10" fill="url(#g)"/>
            <rect width="10" height="10" fill="url(#missing) #0000ff"/>
            <rect width="10" height="10" fill="url(#missing)"/>"##,
    );
    let shape = rect_path(0.0, 0.0, 10.0, 10.0);
    let mut expected = Scene::new();
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgba8(255, 0, 0, 128),
        &shape,
    );
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, BLUE, &shape);
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// A stop's `currentColor` is the stop's own `color`, inherited through the
/// gradient and its ancestors, never the painted shape's: two shapes of
/// different `color` sharing one gradient get the same stop.
#[test]
fn a_stops_current_color_inherits_through_the_gradient_not_the_shape() {
    let parsed = document(
        r##"<defs color="#0000ff">
              <linearGradient id="defs"><stop offset="0" stop-color="currentColor"/></linearGradient>
              <linearGradient id="own" color="#ff0000"><stop offset="0" stop-color="currentColor"/></linearGradient>
            </defs>
            <rect width="10" height="10" color="#00ff00" fill="url(#defs)"/>
            <rect width="10" height="10" color="#ff00ff" fill="url(#defs)"/>
            <rect width="10" height="10" color="#0000ff" fill="url(#own)"/>"##,
    );
    let fills: Vec<&Brush> = parsed
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Path {
                fill: Some(fill), ..
            } => Some(&fill.brush),
            _ => None,
        })
        .collect();
    assert_eq!(
        fills,
        [&Brush::Solid(BLUE), &Brush::Solid(BLUE), &Brush::Solid(RED)]
    );
}

// --- Groups, layers and clips -------------------------------------------

#[test]
fn group_opacity_is_one_layer_over_the_layer_bounds() {
    let parsed = document(
        r##"<g opacity="0.5" transform="translate(5 6)">
              <rect x="10" y="10" width="20" height="20" fill="#ff0000"/>
            </g>"##,
    );
    let mut expected = Scene::new();
    expected.push_layer(
        Fill::NonZero,
        normal(),
        0.5,
        Affine::translate((5.0, 6.0)),
        &Rect::new(10.0, 10.0, 30.0, 30.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::translate((5.0, 6.0)),
        RED,
        &rect_path(10.0, 10.0, 30.0, 30.0),
    );
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// The layer bounds include a stroke's reach, in the group's own
/// coordinates.
#[test]
fn layer_bounds_include_strokes() {
    let parsed = document(
        r##"<g opacity="0.5">
              <rect x="10" y="10" width="20" height="20" fill="none" stroke="#ff0000" stroke-width="4" stroke-linejoin="round"/>
            </g>"##,
    );
    let Item::PushLayer {
        clip: LayerClip::Bounds(bounds),
        ..
    } = &parsed.items[0]
    else {
        panic!("a bounds layer");
    };
    assert_eq!(*bounds, Rect::new(8.0, 8.0, 32.0, 32.0));
}

#[test]
fn a_blend_mode_is_the_group_layer_blend() {
    let parsed = document(
        r##"<g mix-blend-mode="multiply">
              <rect width="20" height="20" fill="#ff0000"/>
            </g>"##,
    );
    let mut expected = Scene::new();
    expected.push_layer(
        Fill::NonZero,
        BlendMode::new(Mix::Multiply, Compose::SrcOver),
        1.0,
        Affine::IDENTITY,
        &Rect::new(0.0, 0.0, 20.0, 20.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        RED,
        &rect_path(0.0, 0.0, 20.0, 20.0),
    );
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
    assert!(opens_blend(&parsed), "a draw of this image must isolate it");
}

/// Opacity on a shape itself wraps that one shape in a layer, as a group
/// would.
#[test]
fn opacity_on_a_shape_is_a_layer_around_it() {
    let parsed = document(r##"<rect width="20" height="20" fill="#ff0000" opacity="0.5"/>"##);
    let mut expected = Scene::new();
    expected.push_layer(
        Fill::NonZero,
        normal(),
        0.5,
        Affine::IDENTITY,
        &Rect::new(0.0, 0.0, 20.0, 20.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        RED,
        &rect_path(0.0, 0.0, 20.0, 20.0),
    );
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
}

#[test]
fn isolation_alone_and_a_filter_push_nothing() {
    for attribute in [r#"isolation="isolate""#, r#"filter="url(#f)""#] {
        let parsed = document(&format!(
            r##"<defs><filter id="f"><feGaussianBlur stdDeviation="2"/></filter></defs>
                <g {attribute}><rect width="20" height="20" fill="#ff0000"/></g>"##
        ));
        let mut expected = Scene::new();
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            &rect_path(0.0, 0.0, 20.0, 20.0),
        );
        assert_scenes_identical(&encoded(&parsed), &expected);
    }
}

#[test]
fn a_masked_group_is_skipped_and_its_siblings_still_draw() {
    let parsed = document(
        r##"<defs><mask id="m"><rect width="10" height="10" fill="#ffffff"/></mask></defs>
            <g mask="url(#m)"><rect width="20" height="20" fill="#ff0000"/></g>
            <rect y="30" width="20" height="20" fill="#0000ff"/>"##,
    );
    let mut expected = Scene::new();
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        BLUE,
        &rect_path(0.0, 30.0, 20.0, 50.0),
    );
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// A one-shape clip on a group with opacity 1 is a clip layer at
/// `group transform * clipPath transform`, under the child's `clip-rule`.
#[test]
fn a_single_path_clip_is_a_clip_layer_under_the_full_transform_product() {
    let parsed = document(
        r##"<defs><clipPath id="c" transform="translate(5 5)">
              <rect x="1" y="2" width="30" height="30" clip-rule="evenodd"/>
            </clipPath></defs>
            <g clip-path="url(#c)" transform="scale(2)">
              <rect width="40" height="40" fill="#ff0000"/>
            </g>"##,
    );
    let mut expected = Scene::new();
    expected.push_clip_layer(
        Fill::EvenOdd,
        Affine::scale(2.0) * Affine::translate((5.0, 5.0)),
        &rect_path(1.0, 2.0, 31.0, 32.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::scale(2.0),
        RED,
        &rect_path(0.0, 0.0, 40.0, 40.0),
    );
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// With opacity, a one-shape clip is the compositing layer's own shape.
#[test]
fn a_single_path_clip_with_opacity_is_the_layer_shape() {
    let parsed = document(
        r##"<defs><clipPath id="c"><circle cx="20" cy="20" r="10"/></clipPath></defs>
            <g clip-path="url(#c)" opacity="0.25">
              <rect width="40" height="40" fill="#ff0000"/>
            </g>"##,
    );
    let mut expected = Scene::new();
    expected.push_layer(
        Fill::NonZero,
        normal(),
        0.25,
        Affine::IDENTITY,
        &Circle::new((20.0, 20.0), 10.0).to_path(ARC_TOLERANCE),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        RED,
        &rect_path(0.0, 0.0, 40.0, 40.0),
    );
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// Several children clip with their concatenation, each under its own
/// transform; with opacity the layer takes the bounds and the clip sits
/// inside it.
#[test]
fn a_multi_child_clip_concatenates_its_children() {
    for opacity in ["1", "0.5"] {
        let parsed = document(&format!(
            r##"<defs><clipPath id="c">
                  <rect width="10" height="10"/>
                  <rect x="20" y="20" width="10" height="10" transform="rotate(10)"/>
                </clipPath></defs>
                <g clip-path="url(#c)" opacity="{opacity}">
                  <rect width="40" height="40" fill="#ff0000"/>
                </g>"##
        ));
        let mut concatenated = rect_path(0.0, 0.0, 10.0, 10.0);
        let mut rotated = rect_path(20.0, 20.0, 30.0, 30.0);
        rotated.apply_affine(Affine::rotate(10_f64.to_radians()));
        concatenated.extend(rotated);
        let mut expected = Scene::new();
        if opacity == "0.5" {
            expected.push_layer(
                Fill::NonZero,
                normal(),
                0.5,
                Affine::IDENTITY,
                &Rect::new(0.0, 0.0, 40.0, 40.0),
            );
        }
        expected.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &concatenated);
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            &rect_path(0.0, 0.0, 40.0, 40.0),
        );
        expected.pop_layer();
        if opacity == "0.5" {
            expected.pop_layer();
        }
        assert_scenes_identical(&encoded(&parsed), &expected);
    }
}

/// A `clipPath` that is itself clipped pushes that clip first, under the
/// same group transform.
#[test]
fn a_clipped_clip_path_pushes_its_own_clip_first() {
    let parsed = document(
        r##"<defs>
              <clipPath id="outer"><rect width="15" height="40"/></clipPath>
              <clipPath id="inner" clip-path="url(#outer)"><rect width="40" height="15"/></clipPath>
            </defs>
            <g clip-path="url(#inner)"><rect width="40" height="40" fill="#ff0000"/></g>"##,
    );
    let mut expected = Scene::new();
    expected.push_clip_layer(
        Fill::NonZero,
        Affine::IDENTITY,
        &rect_path(0.0, 0.0, 15.0, 40.0),
    );
    expected.push_clip_layer(
        Fill::NonZero,
        Affine::IDENTITY,
        &rect_path(0.0, 0.0, 40.0, 15.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        RED,
        &rect_path(0.0, 0.0, 40.0, 40.0),
    );
    expected.pop_layer();
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// `clipPathUnits="objectBoundingBox"` scales the clip by the clipped
/// element's bounding box.
#[test]
fn an_object_bounding_box_clip_scales_to_the_element() {
    let parsed = document(
        r##"<defs><clipPath id="c" clipPathUnits="objectBoundingBox"><rect width="0.5" height="1"/></clipPath></defs>
            <g clip-path="url(#c)"><rect x="10" y="10" width="40" height="20" fill="#ff0000"/></g>"##,
    );
    let mut expected = Scene::new();
    expected.push_clip_layer(
        Fill::NonZero,
        Affine::translate((10.0, 10.0)) * Affine::scale_non_uniform(40.0, 20.0),
        &rect_path(0.0, 0.0, 0.5, 1.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        RED,
        &rect_path(10.0, 10.0, 50.0, 30.0),
    );
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// A blend group directly inside a clip-only group would open a blend
/// layer inside a clip layer (vello #1198), so the clip becomes a full
/// `Normal` layer.
#[test]
fn a_clip_around_a_blend_group_is_a_full_layer() {
    let parsed = document(
        r##"<defs><clipPath id="c"><rect width="30" height="30"/></clipPath></defs>
            <g clip-path="url(#c)">
              <g mix-blend-mode="screen"><rect width="20" height="20" fill="#ff0000"/></g>
            </g>"##,
    );
    let mut expected = Scene::new();
    expected.push_layer(
        Fill::NonZero,
        normal(),
        1.0,
        Affine::IDENTITY,
        &rect_path(0.0, 0.0, 30.0, 30.0),
    );
    expected.push_layer(
        Fill::NonZero,
        BlendMode::new(Mix::Screen, Compose::SrcOver),
        1.0,
        Affine::IDENTITY,
        &Rect::new(0.0, 0.0, 20.0, 20.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        RED,
        &rect_path(0.0, 0.0, 20.0, 20.0),
    );
    expected.pop_layer();
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
    assert!(
        !opens_blend(&parsed),
        "the blend sits inside the clip's own layer, not at the image's top level"
    );
}

/// An `isolation: isolate` group pushes no layer, so a blend group inside
/// it still opens directly in the clip layer around both; the clip becomes
/// a full `Normal` layer exactly as with no group between.
#[test]
fn a_clip_around_an_isolated_group_around_a_blend_group_is_a_full_layer() {
    let parsed = document(
        r##"<defs><clipPath id="c"><rect width="30" height="30"/></clipPath></defs>
            <g clip-path="url(#c)">
              <g isolation="isolate">
                <g mix-blend-mode="screen"><rect width="20" height="20" fill="#ff0000"/></g>
              </g>
            </g>"##,
    );
    let mut expected = Scene::new();
    expected.push_layer(
        Fill::NonZero,
        normal(),
        1.0,
        Affine::IDENTITY,
        &rect_path(0.0, 0.0, 30.0, 30.0),
    );
    expected.push_layer(
        Fill::NonZero,
        BlendMode::new(Mix::Screen, Compose::SrcOver),
        1.0,
        Affine::IDENTITY,
        &Rect::new(0.0, 0.0, 20.0, 20.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        RED,
        &rect_path(0.0, 0.0, 20.0, 20.0),
    );
    expected.pop_layer();
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// At the image root, a blend group under an `isolation: isolate` group
/// opens in whatever layer a draw of the image pushes, so the image
/// counts as opening a blend (`background.rs` promotes its tile layers).
#[test]
fn a_blend_under_an_isolated_group_at_the_root_opens_a_blend() {
    let parsed = document(
        r##"<g isolation="isolate">
              <g mix-blend-mode="screen"><rect width="20" height="20" fill="#ff0000"/></g>
            </g>"##,
    );
    let mut expected = Scene::new();
    expected.push_layer(
        Fill::NonZero,
        BlendMode::new(Mix::Screen, Compose::SrcOver),
        1.0,
        Affine::IDENTITY,
        &Rect::new(0.0, 0.0, 20.0, 20.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        RED,
        &rect_path(0.0, 0.0, 20.0, 20.0),
    );
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
    assert!(
        opens_blend(&parsed),
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
        let parsed = document(&format!(
            r##"<defs>
                  <clipPath id="outer"><rect width="15" height="40"/></clipPath>
                  <clipPath id="inner" clip-path="url(#outer)"><rect width="40" height="15"/></clipPath>
                </defs>
                <g clip-path="url(#inner)" opacity="{opacity}">
                  <g mix-blend-mode="screen"><rect width="20" height="20" fill="#ff0000"/></g>
                </g>"##
        ));
        let outer_shape = rect_path(0.0, 0.0, 15.0, 40.0);
        let inner_shape = rect_path(0.0, 0.0, 40.0, 15.0);
        let mut expected = Scene::new();
        if opacity == "1" {
            expected.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &outer_shape);
            expected.push_layer(Fill::NonZero, normal(), 1.0, Affine::IDENTITY, &inner_shape);
        } else {
            expected.push_layer(Fill::NonZero, normal(), 0.5, Affine::IDENTITY, &inner_shape);
            expected.push_layer(Fill::NonZero, normal(), 1.0, Affine::IDENTITY, &outer_shape);
        }
        expected.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Screen, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 20.0, 20.0),
        );
        solid_fill(
            &mut expected,
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            &rect_path(0.0, 0.0, 20.0, 20.0),
        );
        expected.pop_layer();
        expected.pop_layer();
        expected.pop_layer();
        assert_scenes_identical(&encoded(&parsed), &expected);
    }
}

// --- Structure: use, symbol, nested svg, switch, style -------------------

/// A `use` draws its target at `x`/`y`, inheriting from the `use`; a
/// `symbol` target is a nested viewport sized by the `use`.
#[test]
fn use_draws_its_target_and_a_symbol_is_a_sized_viewport() {
    let parsed = document(
        r##"<defs>
              <rect id="r" width="10" height="10"/>
              <symbol id="s" viewBox="0 0 10 10"><rect width="10" height="10" fill="#ff0000"/></symbol>
            </defs>
            <use href="#r" x="5" y="6" fill="#0000ff"/>
            <use href="#s" x="20" y="20" width="20" height="40"/>"##,
    );
    let mut expected = Scene::new();
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::translate((5.0, 6.0)),
        BLUE,
        &rect_path(0.0, 0.0, 10.0, 10.0),
    );
    // The symbol's 10x10 viewBox meets the 20x40 viewport at scale 2,
    // centred vertically, inside the viewport's clip.
    expected.push_clip_layer(
        Fill::NonZero,
        Affine::translate((20.0, 20.0)),
        &rect_path(0.0, 0.0, 20.0, 40.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::translate((20.0, 20.0)) * Affine::translate((0.0, 10.0)) * Affine::scale(2.0),
        RED,
        &rect_path(0.0, 0.0, 10.0, 10.0),
    );
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// A `use` that reaches itself, directly or through an ancestor, draws
/// nothing rather than looping.
#[test]
fn a_recursive_use_draws_nothing() {
    let parsed = document(
        r##"<g id="g"><use href="#g"/><rect width="10" height="10" fill="#ff0000"/></g>
            <use id="u" href="#u"/>"##,
    );
    assert_eq!(paths(&parsed).len(), 1, "only the rect, once");
}

/// Parses `body` inside a root `svg` on a thread with the stack the native
/// blocking pool gives the parse (tokio's 2 MiB), so a walk too deep for
/// it fails the test the way it would abort the process.
fn document_on_pool_stack(body: String) -> VectorDocument {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || document(&body))
        .expect("a parse thread")
        .join()
        .expect("the parse returns")
}

/// Element nesting past [`MAX_NESTING`](super::parse::MAX_NESTING) is
/// skipped with its subtree instead of recursing until the stack runs
/// out, in `roxmltree` or in the walk; the levels above the bound still
/// convert.
#[test]
fn nesting_past_the_bound_is_skipped() {
    let bound = super::parse::MAX_NESTING as usize;
    let levels = 3_000;
    let parsed = document_on_pool_stack(format!(
        r#"{}<rect width="10" height="10"/>{}"#,
        r#"<g opacity="0.5">"#.repeat(levels),
        "</g>".repeat(levels),
    ));
    assert!(paths(&parsed).is_empty(), "the rect lies below the bound");
    let layers = parsed
        .items
        .iter()
        .filter(|item| matches!(item, Item::PushLayer { .. }))
        .count();
    assert!(
        (bound - 1..=bound).contains(&layers),
        "one layer per group above the bound, found {layers}",
    );
    assert_eq!(parsed.items.len(), 2 * layers, "each layer and its pop");
}

/// A chain of `use`s, each expanding the next, is bounded the same way,
/// though its markup is flat: 3,000 of them would overflow the stack.
#[test]
fn a_use_chain_past_the_bound_is_skipped() {
    use std::fmt::Write as _;

    let chain = |length: usize| {
        let mut body = String::from("<defs>");
        for index in 0..length {
            write!(body, r##"<use id="u{index}" href="#u{}"/>"##, index + 1).expect("a String");
        }
        write!(
            body,
            r##"<rect id="u{length}" width="10" height="10"/></defs><use href="#u0"/>"##
        )
        .expect("a String");
        document_on_pool_stack(body)
    };
    let long = chain(3_000);
    assert!(long.items.is_empty(), "the rect lies past the bound");
    assert_eq!(
        paths(&chain(100)).len(),
        1,
        "a chain within the bound draws"
    );
}

/// The pass ahead of `roxmltree` cuts each element nested past the bound
/// with its content and leaves every other byte; comments, `CDATA`,
/// processing instructions, declarations and quoted attribute values nest
/// nothing, as in the parser.
#[test]
fn markup_past_the_bound_is_cut_before_the_parse() {
    use std::borrow::Cow;

    use super::nesting::bound;

    let untouched = |text: &str, limit| {
        assert!(
            matches!(bound(text, limit), Some(Cow::Borrowed(kept)) if kept == text),
            "{text} is left as it is",
        );
    };
    untouched("<a><b>x</b><b/></a>", 2);
    untouched(
        "<a><!--<b><c>--><![CDATA[<b><c>]]><?p <b><c>?><b t='>'/></a>",
        2,
    );
    untouched(r#"<!DOCTYPE a [<!ENTITY e "]>"><!--"-->]><a>&e;</a>"#, 1);
    assert_eq!(
        bound("<a><b><c>x<d/></c>y<e/></b>z</a>", 2).as_deref(),
        Some("<a><b>y</b>z</a>"),
    );
    assert_eq!(
        bound(r#"<a><b t="/>"><c/></b></a>"#, 2).as_deref(),
        Some(r#"<a><b t="/>"></b></a>"#),
        "a `/>` inside a value neither empties the tag nor ends it",
    );
    assert_eq!(
        bound("<a><b><c>", 2).as_deref(),
        Some("<a><b>"),
        "an unclosed cut runs to the end"
    );

    // An entity's markup nests where it is referenced, up to ten
    // references deep, inside the parser: the bound refuses what it cannot
    // cut, and lets through what cannot reach it.
    let entity = r#"<!DOCTYPE a [<!ENTITY e "<b><c/></b>">]><a>&e;</a>"#;
    assert!(bound(entity, 2).is_none());
    untouched(entity, 256);
    let deep = format!(
        r#"<!DOCTYPE svg [<!ENTITY e "{}">]><svg xmlns="http://www.w3.org/2000/svg">&e;</svg>"#,
        "<g>".repeat(300) + &"</g>".repeat(300),
    );
    assert!(matches!(parse(deep.as_bytes()), Err(SvgError::TooDeep)));
}

/// A document nested well inside the bound draws its leaf.
#[test]
fn nesting_inside_the_bound_draws_the_leaf() {
    let levels = 200;
    let parsed = document_on_pool_stack(format!(
        r#"{}<rect width="10" height="10"/>{}"#,
        "<g>".repeat(levels),
        "</g>".repeat(levels),
    ));
    assert_eq!(paths(&parsed).len(), 1);
}

/// A nested `svg` clips to its viewport and maps its `viewBox` by its
/// `preserveAspectRatio`.
#[test]
fn a_nested_svg_clips_and_maps_its_view_box() {
    let parsed = document(
        r##"<svg x="10" y="10" width="40" height="20" viewBox="0 0 10 10" preserveAspectRatio="xMaxYMid meet">
              <rect width="10" height="10" fill="#ff0000"/>
            </svg>"##,
    );
    let mut expected = Scene::new();
    expected.push_clip_layer(
        Fill::NonZero,
        Affine::IDENTITY,
        &rect_path(10.0, 10.0, 50.0, 30.0),
    );
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::translate((10.0, 10.0)) * Affine::translate((20.0, 0.0)) * Affine::scale(2.0),
        RED,
        &rect_path(0.0, 0.0, 10.0, 10.0),
    );
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
    let stretched = document(
        r##"<svg width="40" height="20" viewBox="0 0 10 10" preserveAspectRatio="none">
              <rect width="10" height="10" fill="#ff0000"/>
            </svg>"##,
    );
    assert_eq!(
        paths(&stretched)[0].1,
        Affine::scale_non_uniform(4.0, 2.0),
        "`none` stretches"
    );
}

/// A `switch` draws its first child with no conditional attribute.
#[test]
fn a_switch_draws_its_first_unconditional_child() {
    let parsed = document(
        r##"<switch>
              <rect systemLanguage="xx" width="10" height="10" fill="#ff0000"/>
              <rect requiredFeatures="f" width="10" height="10" fill="#ff0000"/>
              <rect width="10" height="10" fill="#0000ff"/>
              <rect width="10" height="10" fill="#ff0000"/>
            </switch>"##,
    );
    let mut expected = Scene::new();
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        BLUE,
        &rect_path(0.0, 0.0, 10.0, 10.0),
    );
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// CSS inside the document is not read: a `<style>` rule, `!important`
/// included, changes nothing, so each fill stays its presentation
/// attribute's or the initial black, and a rule's stroke and `display`
/// never apply.
#[test]
fn a_style_element_changes_nothing() {
    let parsed = document(
        r##"<style>
              rect { fill: #0000ff; }
              .a { fill: #00ff00 !important; stroke: #ff0000; stroke-width: 2; }
              #b { fill: #00ff00 !important; display: none; }
            </style>
            <rect width="10" height="10"/>
            <rect class="a" width="10" height="10" fill="#ff0000"/>
            <rect id="b" width="10" height="10" fill="#0000ff"/>
            <g><rect class="a" width="10" height="10" fill="none"/></g>"##,
    );
    let shape = rect_path(0.0, 0.0, 10.0, 10.0);
    let mut expected = Scene::new();
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgba8(0, 0, 0, 255),
        &shape,
    );
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, RED, &shape);
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, BLUE, &shape);
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// The `style` attribute is not read either: its declarations, `!important`
/// included, change neither a fill nor a group's opacity or blend.
#[test]
fn a_style_attribute_changes_nothing() {
    let parsed = document(
        r##"<rect width="10" height="10" style="fill: #0000ff"/>
            <rect width="10" height="10" fill="#ff0000"
                  style="fill: #0000ff !important; stroke: #0000ff; display: none"/>
            <g style="opacity: 0.5; mix-blend-mode: multiply">
              <rect width="10" height="10" fill="#0000ff"/>
            </g>"##,
    );
    let shape = rect_path(0.0, 0.0, 10.0, 10.0);
    let mut expected = Scene::new();
    solid_fill(
        &mut expected,
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgba8(0, 0, 0, 255),
        &shape,
    );
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, RED, &shape);
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, BLUE, &shape);
    assert_scenes_identical(&encoded(&parsed), &expected);
    assert!(
        !opens_blend(&parsed),
        "a blend in a style attribute opens nothing"
    );
}

/// `inherit` takes the parent's value, and `currentColor` is each
/// element's own `color`.
#[test]
fn inherit_and_current_color_resolve_per_element() {
    let parsed = document(
        r##"<g fill="#ff0000" color="#0000ff" stroke="currentColor" stroke-width="0">
              <rect width="10" height="10" fill="inherit"/>
              <rect width="10" height="10" fill="currentColor"/>
              <rect width="10" height="10" fill="currentColor" color="#ff0000"/>
              <g opacity="0.5"><rect width="10" height="10" opacity="inherit"/></g>
            </g>"##,
    );
    let shape = rect_path(0.0, 0.0, 10.0, 10.0);
    let mut expected = Scene::new();
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, RED, &shape);
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, BLUE, &shape);
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, RED, &shape);
    for _ in 0..2 {
        expected.push_layer(
            Fill::NonZero,
            normal(),
            0.5,
            Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 10.0, 10.0),
        );
    }
    solid_fill(&mut expected, Fill::NonZero, Affine::IDENTITY, RED, &shape);
    expected.pop_layer();
    expected.pop_layer();
    assert_scenes_identical(&encoded(&parsed), &expected);
}

/// `image` elements draw nothing, whatever they name.
#[test]
fn an_image_element_draws_nothing() {
    let parsed = document(
        r#"<image x="10" y="20" width="40" height="40"
              href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='10'%3E%3Crect width='10' height='10' fill='%23ff0000'/%3E%3C/svg%3E"/>
            <image x="0" y="0" width="10" height="10" href="/etc/hosts"/>"#,
    );
    assert!(parsed.items.is_empty());
}

#[test]
fn an_empty_document_encodes_nothing() {
    let parsed = document("");
    assert!(parsed.items.is_empty());
    assert!(encoded(&parsed).encoding().is_empty());
    assert!(!opens_blend(&parsed));
}

// --- Sizes and errors ------------------------------------------------------

/// The natural size follows CSS Images 3 default sizing, and the viewport
/// is the `viewBox` size when there is one.
#[test]
fn natural_size_and_viewport_follow_the_root_attributes() {
    let cases = [
        // (a) Both absolute, with or without a viewBox.
        (r#"width="40" height="30""#, (40, 30), (40.0, 30.0)),
        (r#"width="40" height="30px""#, (40, 30), (40.0, 30.0)),
        (
            r#"width="40px" height="30" viewBox="0 0 4 3""#,
            (40, 30),
            (4.0, 3.0),
        ),
        // (b) One absolute plus a viewBox.
        (r#"width="40" viewBox="0 0 20 10""#, (40, 20), (20.0, 10.0)),
        (r#"height="30" viewBox="0 0 20 10""#, (60, 30), (20.0, 10.0)),
        // (c) A viewBox only, fitted into 300x150.
        (r#"viewBox="0 0 10 10""#, (150, 150), (10.0, 10.0)),
        (r#"viewBox="0 0 40 10""#, (300, 75), (40.0, 10.0)),
        (r#"viewBox="0,0,40,10""#, (300, 75), (40.0, 10.0)),
        // (d) Neither: the default object size, which is also the viewport.
        ("", (300, 150), (300.0, 150.0)),
        (r#"width="50%" height="2em""#, (300, 150), (300.0, 150.0)),
        // (e) One absolute and no viewBox.
        (r#"width="50%" height="20""#, (300, 20), (300.0, 20.0)),
        (r#"width="50%" height="1in""#, (300, 96), (300.0, 96.0)),
        (r#"width="3pc" height="1ex""#, (48, 150), (48.0, 150.0)),
        (r#"width="40""#, (40, 150), (40.0, 150.0)),
        // Whole px, at least one; the authored size stays the viewport.
        (r#"width="10.4" height="0.2""#, (10, 1), (10.4, 0.2)),
    ];
    for (attributes, natural, viewport) in cases {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" {attributes}><rect x="2" y="3" width="5" height="7"/></svg>"#
        );
        let parsed =
            parse(svg.as_bytes()).unwrap_or_else(|error| panic!("<svg {attributes}>: {error}"));
        assert_eq!(parsed.natural, natural, "<svg {attributes}> natural");
        assert_eq!(parsed.viewport, viewport, "<svg {attributes}> viewport");
    }
}

/// Every CSS absolute unit converts at 96 px per inch; a relative unit
/// counts as absent.
#[test]
fn absolute_units_convert_to_px() {
    for (length, px) in [
        ("1in", 96.0),
        ("2.54cm", 96.0),
        ("25.4mm", 96.0),
        ("72pt", 96.0),
        ("6pc", 96.0),
        ("96px", 96.0),
        ("96", 96.0),
    ] {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{length}" height="{length}"/>"#
        );
        let parsed = parse(svg.as_bytes()).expect(length);
        assert_eq!(parsed.natural, (96, 96), "{length}");
        assert!(
            (parsed.viewport.0 - px).abs() < 1e-3,
            "{length} is {} px",
            parsed.viewport.0
        );
    }
    for absent in ["1em", "1ex", "50%", "1rem", "1vw", "0in", "-2pt"] {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{absent}" height="{absent}"/>"#
        );
        let parsed = parse(svg.as_bytes()).expect(absent);
        assert_eq!(parsed.natural, (300, 150), "{absent}");
    }
}

/// A `viewBox` with an origin moves the content so the viewport starts at
/// the origin.
#[test]
fn a_view_box_origin_translates_the_content() {
    let parsed = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="10 20 100 50"><rect width="5" height="5"/></svg>"#,
    )
    .expect("parses");
    assert_eq!(parsed.viewport, (100.0, 50.0));
    assert_eq!(paths(&parsed)[0].1, Affine::translate((-10.0, -20.0)));
}

#[test]
fn preserve_aspect_ratio_parses_into_the_image_aspect() {
    let aspect_of = |value: &str| {
        let svg =
            format!(r#"<svg xmlns="http://www.w3.org/2000/svg" preserveAspectRatio="{value}"/>"#);
        parse(svg.as_bytes()).expect("parses").aspect
    };
    assert_eq!(aspect_of("xMidYMid meet"), AspectRatio::default());
    assert_eq!(
        aspect_of("xMinYMax slice"),
        AspectRatio {
            align: Some((AspectAlign::Min, AspectAlign::Max)),
            slice: true,
        }
    );
    assert_eq!(
        aspect_of("none"),
        AspectRatio {
            align: None,
            slice: false,
        }
    );
    assert_eq!(
        aspect_of("xMaxYMin"),
        AspectRatio {
            align: Some((AspectAlign::Max, AspectAlign::Min)),
            slice: false,
        }
    );
    assert_eq!(aspect_of("garbage"), AspectRatio::default());
    let absent = parse(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#).expect("parses");
    assert_eq!(absent.aspect, AspectRatio::default());
}

#[test]
fn unreadable_documents_fail_with_their_own_error() {
    assert!(matches!(
        parse(b"<svg xmlns='http://www.w3.org/2000/svg'><rect></svg>"),
        Err(SvgError::Xml(_))
    ));
    assert!(matches!(parse(b"<svg \xff/>"), Err(SvgError::NotUtf8)));
    assert!(matches!(
        parse(&[0x1f, 0x8b, 0x08, 0x00]),
        Err(SvgError::NotUtf8)
    ));
    assert!(matches!(
        parse(b"<html xmlns='http://www.w3.org/1999/xhtml'/>"),
        Err(SvgError::NotSvg)
    ));
    let error = parse(b"<svg xmlns='http://www.w3.org/2000/svg'><rect></svg>").unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("the document is not well-formed XML")
    );
}

/// Every item type crosses threads, and a document clones.
#[test]
fn a_document_is_sendable_and_clonable() {
    let parsed = document(r#"<g opacity="0.5"><rect width="1" height="1"/></g>"#);
    let shared = Arc::new(parsed.clone());
    std::thread::spawn(move || shared.items.len())
        .join()
        .expect("the thread ran");
    assert_eq!(parsed.items.len(), 3);
    assert!(format!("{parsed:?}").starts_with("VectorDocument"));
}
