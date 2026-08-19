//! Borders and outlines (css-backgrounds-3 §4, css-ui-4 §4).
//!
//! Spec sketch:
//! - Fast path: every side that paints is `solid` in one color → fill the ring between the outer
//!   border-box shape and the inner padding-box shape (even-odd) in one draw.
//! - General path: per side, clip to the trapezoidal side region between the outer and inner shapes
//!   (miter lines join outer corners to inner corners per CSS2 §8.5.4; for rounded corners the
//!   split runs along the corner region's diagonal) and paint the side's style: `solid` fills the
//!   ring; `double` fills two sub-rings (outer third, inner third); `dashed`/`dotted` stroke the
//!   side's centerline (2w dashes with 1w gaps, near-0-length round dots at 2w spacing — see
//!   `dash_stroke` for why the dot length can't be exactly zero); `inset`/`outset`/`groove`/
//!   `ridge` use the CSS2 §8.5.3 darker/lighter color split per side (`groove`/`ridge` split the
//!   ring in half). `hidden`/`none` paint nothing (layout already zeroes their widths).
//! - `outline`: a ring **outside** the border box, flush against it (the fork's lynx grammar
//!   deliberately omits `outline-offset` — Lynx outlines are flush rings); `auto` paints as solid,
//!   dashed/dotted stroke the ring centerline, other paintable styles fall back to solid. Radii
//!   grow by the outline width (sharp corners stay sharp).
//! - Zero-size boxes, zero-width sides, and fully transparent colors skip cleanly.

use smallvec::SmallVec;
use stylo::properties::ComputedValues;
use stylo::values::computed::{BorderStyle, OutlineStyle};

use crate::Size2D;
use crate::layout::Edges;
use crate::paint::convert::resolve_color;
use crate::paint::shape::{BoxShape, inner_radii, ring_path_into, with_shape};
use crate::paint::{BoxFragment, PathScratch};
use crate::vello::Scene;
use crate::vello::kurbo::{BezPath, Cap, Rect, Stroke};
use crate::vello::peniko::{Color, Fill};
use crate::visual::CornerRadii;

#[derive(Debug, Clone, Copy)]
enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

/// One side that actually paints: nonzero used width, paintable style,
/// non-transparent resolved color.
#[derive(Debug, Clone, Copy)]
struct SidePaint {
    side: Side,
    width: f64,
    line: BorderStyle,
    color: Color,
}

pub(crate) fn paint(
    scene: &mut Scene,
    paths: &mut PathScratch,
    style: &ComputedValues,
    fragment: &BoxFragment,
) {
    if fragment.border_box.width() <= 0.0 || fragment.border_box.height() <= 0.0 {
        return;
    }
    let sides = paintable_sides(style, &fragment.border_widths);
    if sides.is_empty() {
        return;
    }
    let outer = BoxShape::new(fragment.border_box, &fragment.radii);
    let inner = BoxShape::new(
        fragment.padding_box,
        &inner_radii(&fragment.radii, &fragment.border_widths),
    );
    let positive_width_sides = [
        fragment.border_widths.top,
        fragment.border_widths.right,
        fragment.border_widths.bottom,
        fragment.border_widths.left,
    ]
    .iter()
    .filter(|&&width| width > 0.0)
    .count();
    if sides.len() == positive_width_sides
        && let Some(color) = uniform_solid_color(&sides)
    {
        ring_path_into(&mut paths.ring, &outer, &inner);
        scene.fill(Fill::EvenOdd, fragment.transform, color, None, &paths.ring);
        return;
    }
    for side in &sides {
        paint_side(scene, paths, fragment, &outer, &inner, side);
    }
}

fn paintable_sides(style: &ComputedValues, widths: &Edges<f32>) -> SmallVec<[SidePaint; 4]> {
    let border = style.get_border();
    let sides = [
        (
            Side::Top,
            widths.top,
            border.border_top_style,
            &border.border_top_color,
        ),
        (
            Side::Right,
            widths.right,
            border.border_right_style,
            &border.border_right_color,
        ),
        (
            Side::Bottom,
            widths.bottom,
            border.border_bottom_style,
            &border.border_bottom_color,
        ),
        (
            Side::Left,
            widths.left,
            border.border_left_style,
            &border.border_left_color,
        ),
    ];
    sides
        .into_iter()
        .filter(|(_, width, line, _)| *width > 0.0 && !line.none_or_hidden())
        .filter_map(|(side, width, line, color)| {
            let color = resolve_color(style, color);
            (color.components[3] > 0.0).then_some(SidePaint {
                side,
                width: f64::from(width),
                line,
                color,
            })
        })
        .collect()
}

fn uniform_solid_color(sides: &[SidePaint]) -> Option<Color> {
    let first = sides.first()?;
    sides
        .iter()
        .all(|side| side.line == BorderStyle::Solid && same_color(side.color, first.color))
        .then_some(first.color)
}

fn paint_side(
    scene: &mut Scene,
    paths: &mut PathScratch,
    fragment: &BoxFragment,
    outer: &BoxShape,
    inner: &BoxShape,
    side: &SidePaint,
) {
    let transform = fragment.transform;
    side_quad_into(
        &mut paths.quad,
        side.side,
        fragment.border_box,
        fragment.padding_box,
    );
    scene.push_clip_layer(Fill::NonZero, transform, &paths.quad);
    match side.line {
        BorderStyle::Solid | BorderStyle::Inset | BorderStyle::Outset => {
            let color = flat_shade(side.line, side.side, side.color);
            ring_path_into(&mut paths.ring, outer, inner);
            scene.fill(Fill::EvenOdd, transform, color, None, &paths.ring);
        }
        BorderStyle::Double => {
            let boundary_a = inset_shape(fragment, 1.0 / 3.0);
            let boundary_b = inset_shape(fragment, 2.0 / 3.0);
            let color = side.color;
            ring_path_into(&mut paths.ring, outer, &boundary_a);
            scene.fill(Fill::EvenOdd, transform, color, None, &paths.ring);
            ring_path_into(&mut paths.ring, &boundary_b, inner);
            scene.fill(Fill::EvenOdd, transform, color, None, &paths.ring);
        }
        BorderStyle::Groove | BorderStyle::Ridge => {
            let mid = inset_shape(fragment, 0.5);
            let (outer_shade, inner_shade) = split_shades(side.line, side.side, side.color);
            ring_path_into(&mut paths.ring, outer, &mid);
            scene.fill(Fill::EvenOdd, transform, outer_shade, None, &paths.ring);
            ring_path_into(&mut paths.ring, &mid, inner);
            scene.fill(Fill::EvenOdd, transform, inner_shade, None, &paths.ring);
        }
        BorderStyle::Dashed | BorderStyle::Dotted => {
            let centerline = inset_shape(fragment, 0.5);
            let stroke = dash_stroke(side.line, side.width);
            with_shape!(&centerline, |shape| scene
                .stroke(&stroke, transform, side.color, None, shape));
        }
        BorderStyle::None | BorderStyle::Hidden => {}
    }
    scene.pop_layer();
}

fn side_quad_into(path: &mut BezPath, side: Side, outer: Rect, inner: Rect) {
    let (a, b, c, d) = match side {
        Side::Top => (
            (outer.x0, outer.y0),
            (outer.x1, outer.y0),
            (inner.x1, inner.y0),
            (inner.x0, inner.y0),
        ),
        Side::Right => (
            (outer.x1, outer.y0),
            (outer.x1, outer.y1),
            (inner.x1, inner.y1),
            (inner.x1, inner.y0),
        ),
        Side::Bottom => (
            (outer.x1, outer.y1),
            (outer.x0, outer.y1),
            (inner.x0, inner.y1),
            (inner.x1, inner.y1),
        ),
        Side::Left => (
            (outer.x0, outer.y1),
            (outer.x0, outer.y0),
            (inner.x0, inner.y0),
            (inner.x0, inner.y1),
        ),
    };
    path.truncate(0);
    path.move_to(a);
    path.line_to(b);
    path.line_to(c);
    path.line_to(d);
    path.close_path();
}

fn ring_boundary_radii(radii: &CornerRadii, widths: &Edges<f32>, fraction: f32) -> CornerRadii {
    inner_radii(radii, &widths.map(|width| width * fraction))
}

fn inset_shape(fragment: &BoxFragment, fraction: f32) -> BoxShape {
    let border_box = fragment.border_box;
    let scaled = fragment.border_widths.map(|width| width * fraction);
    let x0 = border_box.x0 + f64::from(scaled.left);
    let y0 = border_box.y0 + f64::from(scaled.top);
    let rect = Rect::new(
        x0,
        y0,
        (border_box.x1 - f64::from(scaled.right)).max(x0),
        (border_box.y1 - f64::from(scaled.bottom)).max(y0),
    );
    BoxShape::new(
        rect,
        &ring_boundary_radii(&fragment.radii, &fragment.border_widths, fraction),
    )
}

fn dash_stroke(line: BorderStyle, width: f64) -> Stroke {
    match line {
        BorderStyle::Dotted => {
            let stub = 0.05 * width;
            Stroke::new(width)
                .with_caps(Cap::Round)
                .with_dashes(0.0, [stub, 2.0 * width - stub])
        }
        _ => Stroke::new(width)
            .with_caps(Cap::Butt)
            .with_dashes(0.0, [2.0 * width, width]),
    }
}

fn flat_shade(line: BorderStyle, side: Side, color: Color) -> Color {
    let top_left = matches!(side, Side::Top | Side::Left);
    match line {
        BorderStyle::Inset => {
            if top_left {
                darken(color)
            } else {
                lighten(color)
            }
        }
        BorderStyle::Outset => {
            if top_left {
                lighten(color)
            } else {
                darken(color)
            }
        }
        _ => color,
    }
}

fn split_shades(line: BorderStyle, side: Side, color: Color) -> (Color, Color) {
    let dark_outer = matches!(side, Side::Top | Side::Left) == (line == BorderStyle::Groove);
    if dark_outer {
        (darken(color), lighten(color))
    } else {
        (lighten(color), darken(color))
    }
}

fn darken(color: Color) -> Color {
    shade(color, |channel| channel * (2.0 / 3.0))
}

fn lighten(color: Color) -> Color {
    shade(color, |channel| channel + (1.0 - channel) / 3.0)
}

fn shade(color: Color, tone: impl Fn(f32) -> f32) -> Color {
    let [r, g, b, a] = color.components;
    Color::new([tone(r), tone(g), tone(b), a])
}

fn same_color(a: Color, b: Color) -> bool {
    a.components.map(f32::to_bits) == b.components.map(f32::to_bits)
}

pub(crate) fn paint_outline(
    scene: &mut Scene,
    paths: &mut PathScratch,
    style: &ComputedValues,
    fragment: &BoxFragment,
) {
    let Some((width, line, color)) = resolved_outline(style) else {
        return;
    };
    if fragment.border_box.width() <= 0.0 || fragment.border_box.height() <= 0.0 {
        return;
    }
    let transform = fragment.transform;
    if let OutlineStyle::BorderStyle(line @ (BorderStyle::Dashed | BorderStyle::Dotted)) = line {
        let half = width / 2.0;
        let centerline = BoxShape::new(
            fragment.border_box.inflate(half, half),
            &grow_radii(&fragment.radii, half as f32),
        );
        let stroke = dash_stroke(line, width);
        with_shape!(&centerline, |shape| scene
            .stroke(&stroke, transform, color, None, shape));
    } else {
        let outer = BoxShape::new(
            fragment.border_box.inflate(width, width),
            &grow_radii(&fragment.radii, width as f32),
        );
        let inner = BoxShape::new(fragment.border_box, &fragment.radii);
        ring_path_into(&mut paths.ring, &outer, &inner);
        scene.fill(Fill::EvenOdd, transform, color, None, &paths.ring);
    }
}

pub(crate) fn outline_extent(style: &ComputedValues) -> f64 {
    resolved_outline(style).map_or(0.0, |(width, _, _)| width)
}

fn resolved_outline(style: &ComputedValues) -> Option<(f64, OutlineStyle, Color)> {
    let outline = style.get_outline();
    let line = outline.outline_style;
    if matches!(
        line,
        OutlineStyle::BorderStyle(BorderStyle::None | BorderStyle::Hidden)
    ) {
        return None;
    }
    let width = outline.outline_width.0.to_f64_px();
    if width <= 0.0 {
        return None;
    }
    let color = resolve_color(style, &outline.outline_color);
    if color.components[3] <= 0.0 {
        return None;
    }
    Some((width, line, color))
}

fn grow_radii(radii: &CornerRadii, by: f32) -> CornerRadii {
    let corner = |corner: Size2D<f32>| {
        if corner.width > 0.0 && corner.height > 0.0 {
            Size2D::new(corner.width + by, corner.height + by)
        } else {
            corner
        }
    };
    CornerRadii {
        top_left: corner(radii.top_left),
        top_right: corner(radii.top_right),
        bottom_right: corner(radii.bottom_right),
        bottom_left: corner(radii.bottom_left),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::vello::kurbo::{Point, Shape};

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
    }

    fn uniform_radii(radius: f32) -> CornerRadii {
        let corner = Size2D::new(radius, radius);
        CornerRadii {
            top_left: corner,
            top_right: corner,
            bottom_right: corner,
            bottom_left: corner,
        }
    }

    #[test]
    fn sub_ring_boundary_radii_interpolate_linearly() {
        let radii = CornerRadii {
            top_left: Size2D::new(12.0, 9.0),
            top_right: Size2D::new(6.0, 6.0),
            bottom_right: Size2D::new(0.0, 0.0),
            bottom_left: Size2D::new(2.0, 2.0),
        };
        let widths = Edges {
            left: 9.0,
            right: 6.0,
            top: 6.0,
            bottom: 3.0,
        };
        let third = ring_boundary_radii(&radii, &widths, 1.0 / 3.0);
        assert_close(third.top_left.width, 12.0 - 3.0);
        assert_close(third.top_left.height, 9.0 - 2.0);
        let two_thirds = ring_boundary_radii(&radii, &widths, 2.0 / 3.0);
        assert_close(two_thirds.top_left.width, 12.0 - 6.0);
        assert_close(two_thirds.top_left.height, 9.0 - 4.0);
        let inner = ring_boundary_radii(&radii, &widths, 1.0);
        assert_close(inner.top_left.width, 3.0);
        assert_close(inner.top_left.height, 3.0);
        assert_close(two_thirds.bottom_left.width, 0.0);
        assert_close(two_thirds.bottom_left.height, 0.0);
    }

    #[test]
    fn dashed_pattern_has_a_three_width_period() {
        let stroke = dash_stroke(BorderStyle::Dashed, 4.0);
        assert_eq!(stroke.dash_pattern.len(), 2);
        assert!((stroke.dash_pattern[0] - 8.0).abs() < 1e-12); // 2w dash
        assert!((stroke.dash_pattern[1] - 4.0).abs() < 1e-12); // 1w gap
        assert!((stroke.width - 4.0).abs() < 1e-12);
        assert_eq!(stroke.start_cap, Cap::Butt);
    }

    #[test]
    fn dotted_pattern_is_round_dots_every_two_widths() {
        let stroke = dash_stroke(BorderStyle::Dotted, 3.0);
        assert_eq!(stroke.dash_pattern.len(), 2);
        assert!((stroke.dash_pattern[0] - 0.15).abs() < 1e-12);
        assert!(stroke.dash_pattern[0] > 0.0);
        assert!((stroke.dash_pattern[1] - 5.85).abs() < 1e-12);
        assert!((stroke.dash_pattern[0] + stroke.dash_pattern[1] - 6.0).abs() < 1e-12);
        assert_eq!(stroke.start_cap, Cap::Round);
        assert_eq!(stroke.end_cap, Cap::Round);
    }

    #[test]
    fn dash_strokes_encode_nonempty_gpu_stroke_segments() {
        let rect = Rect::new(0.0, 0.0, 100.0, 40.0);
        for line in [BorderStyle::Dotted, BorderStyle::Dashed] {
            let stroke = dash_stroke(line, 3.0);
            let mut scene = Scene::new();
            scene.stroke(
                &stroke,
                crate::vello::kurbo::Affine::IDENTITY,
                Color::new([0.0, 0.0, 0.0, 1.0]),
                None,
                &rect,
            );
            assert!(
                scene.encoding().n_path_segments > 0,
                "{line:?} stroke encoded no path segments"
            );
        }
    }

    #[test]
    fn three_d_shades_move_a_third_toward_black_and_white() {
        let base = Color::new([0.9, 0.6, 0.3, 0.5]);
        let dark = darken(base);
        assert_close(dark.components[0], 0.6);
        assert_close(dark.components[1], 0.4);
        assert_close(dark.components[2], 0.2);
        assert_close(dark.components[3], 0.5); // alpha preserved
        let light = lighten(base);
        assert_close(light.components[0], 0.9 + 0.1 / 3.0);
        assert_close(light.components[1], 0.6 + 0.4 / 3.0);
        assert_close(light.components[2], 0.3 + 0.7 / 3.0);
        assert_close(light.components[3], 0.5);
    }

    #[test]
    fn inset_outset_shade_top_left_against_bottom_right() {
        let base = Color::new([0.6, 0.6, 0.6, 1.0]);
        assert!(same_color(
            flat_shade(BorderStyle::Inset, Side::Top, base),
            darken(base)
        ));
        assert!(same_color(
            flat_shade(BorderStyle::Inset, Side::Bottom, base),
            lighten(base)
        ));
        assert!(same_color(
            flat_shade(BorderStyle::Outset, Side::Left, base),
            lighten(base)
        ));
        assert!(same_color(
            flat_shade(BorderStyle::Outset, Side::Right, base),
            darken(base)
        ));
        assert!(same_color(
            flat_shade(BorderStyle::Solid, Side::Top, base),
            base
        ));
    }

    #[test]
    fn groove_and_ridge_split_the_shades_oppositely() {
        let base = Color::new([0.6, 0.6, 0.6, 1.0]);
        let (outer, inner) = split_shades(BorderStyle::Groove, Side::Top, base);
        assert!(same_color(outer, darken(base)));
        assert!(same_color(inner, lighten(base)));
        let (outer, inner) = split_shades(BorderStyle::Groove, Side::Bottom, base);
        assert!(same_color(outer, lighten(base)));
        assert!(same_color(inner, darken(base)));
        let (outer, inner) = split_shades(BorderStyle::Ridge, Side::Left, base);
        assert!(same_color(outer, lighten(base)));
        assert!(same_color(inner, darken(base)));
        let (outer, inner) = split_shades(BorderStyle::Ridge, Side::Right, base);
        assert!(same_color(outer, darken(base)));
        assert!(same_color(inner, lighten(base)));
    }

    #[test]
    fn side_quads_split_corners_along_the_miter_diagonal() {
        let outer = Rect::new(0.0, 0.0, 40.0, 30.0);
        let inner = Rect::new(4.0, 6.0, 36.0, 22.0);
        let side_quad = |side| {
            let mut path = BezPath::new();
            side_quad_into(&mut path, side, outer, inner);
            path
        };
        let top = side_quad(Side::Top);
        let left = side_quad(Side::Left);
        assert_ne!(top.winding(Point::new(20.0, 3.0)), 0);
        assert_eq!(left.winding(Point::new(20.0, 3.0)), 0);
        assert_eq!(top.winding(Point::new(1.0, 5.0)), 0);
        assert_ne!(left.winding(Point::new(1.0, 5.0)), 0);
        assert_ne!(top.winding(Point::new(3.0, 2.0)), 0);
        assert_eq!(left.winding(Point::new(3.0, 2.0)), 0);
        assert_eq!(top.winding(Point::new(20.0, 26.0)), 0);
    }

    #[test]
    fn fast_path_requires_uniform_solid_sides() {
        let red = Color::new([1.0, 0.0, 0.0, 1.0]);
        let blue = Color::new([0.0, 0.0, 1.0, 1.0]);
        let side = |side, line, color| SidePaint {
            side,
            width: 2.0,
            line,
            color,
        };
        let uniform = [
            side(Side::Top, BorderStyle::Solid, red),
            side(Side::Left, BorderStyle::Solid, red),
        ];
        assert!(uniform_solid_color(&uniform).is_some_and(|color| same_color(color, red)));
        let mixed_style = [
            side(Side::Top, BorderStyle::Solid, red),
            side(Side::Left, BorderStyle::Dashed, red),
        ];
        assert!(uniform_solid_color(&mixed_style).is_none());
        let mixed_color = [
            side(Side::Top, BorderStyle::Solid, red),
            side(Side::Left, BorderStyle::Solid, blue),
        ];
        assert!(uniform_solid_color(&mixed_color).is_none());
        assert!(uniform_solid_color(&[]).is_none());
    }

    #[test]
    fn outline_radii_grow_but_sharp_corners_stay_sharp() {
        let mut radii = uniform_radii(4.0);
        radii.bottom_right = Size2D::new(0.0, 0.0);
        radii.bottom_left = Size2D::new(0.0, 5.0); // square per §5: one zero
        let grown = grow_radii(&radii, 3.0);
        assert_close(grown.top_left.width, 7.0);
        assert_close(grown.top_left.height, 7.0);
        assert_close(grown.bottom_right.width, 0.0);
        assert_close(grown.bottom_left.width, 0.0);
        assert_close(grown.bottom_left.height, 5.0);
    }

    #[test]
    fn inset_shapes_land_between_border_and_padding_boxes() {
        let widths = Edges {
            left: 9.0_f32,
            right: 6.0,
            top: 6.0,
            bottom: 3.0,
        };
        let padding_box = Rect::new(9.0, 6.0, 54.0, 37.0);
        let fragment = BoxFragment {
            node: crate::tree::document::DOCUMENT_NODE_ID,
            transform: crate::vello::kurbo::Affine::IDENTITY,
            border_box: Rect::new(0.0, 0.0, 60.0, 40.0),
            padding_box,
            content_box: padding_box,
            radii: uniform_radii(8.0),
            border_widths: widths,
            padding_widths: Edges::default(),
        };
        let centerline = inset_shape(&fragment, 0.5).bounding_box();
        assert!((centerline.x0 - 4.5).abs() < 1e-9);
        assert!((centerline.y0 - 3.0).abs() < 1e-9);
        assert!((centerline.x1 - 57.0).abs() < 1e-9);
        assert!((centerline.y1 - 38.5).abs() < 1e-9);
        let inner = inset_shape(&fragment, 1.0).bounding_box();
        assert!((inner.x0 - padding_box.x0).abs() < 1e-9);
        assert!((inner.y0 - padding_box.y0).abs() < 1e-9);
        assert!((inner.x1 - padding_box.x1).abs() < 1e-9);
        assert!((inner.y1 - padding_box.y1).abs() < 1e-9);
    }
}
