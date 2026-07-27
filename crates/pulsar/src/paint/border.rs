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
use vello::Scene;
use vello::kurbo::{BezPath, Cap, Rect, Stroke};
use vello::peniko::{Color, Fill};
use w3c_dom::layout::Edges;
use w3c_dom::visual::{CornerRadii, Size2D};

use crate::convert::resolve_color;
use crate::paint::{BoxFragment, PathScratch};
use crate::shape::{BoxShape, inner_radii, ring_path_into, with_shape};

/// A border side.
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

/// Paints the four border sides.
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
    // Fast path: every painting side is `solid` in one color (the
    // ubiquitous case) — one even-odd ring fill, no clip layers.
    // Zero-width sides contribute no ring area, so leaving them out of the
    // uniformity check is sound — but the whole ring is filled, so every
    // positive-width side must actually be in the painting set: a
    // positive-width side that dropped out (fully transparent color) owns
    // ring area that must stay unpainted, which only the per-side path
    // honors.
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

/// The sides that paint, in top/right/bottom/left order. Geometry (used
/// widths) comes from layout — a `none`/`hidden` side already has zero
/// width there — style/color come from the computed style.
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

/// `Some(color)` when every painting side is `solid` in that one color.
fn uniform_solid_color(sides: &[SidePaint]) -> Option<Color> {
    let first = sides.first()?;
    sides
        .iter()
        .all(|side| side.line == BorderStyle::Solid && same_color(side.color, first.color))
        .then_some(first.color)
}

/// Paints one side clipped to its miter quad.
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
            // css-backgrounds-3 §3.2: two lines, each a third of the border
            // width, with the middle third open. Insetting by ⅓ and ⅔ of
            // every side's width keeps the sub-ring boundary radii
            // interpolating consistently across adjacent `double` sides.
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
            // Stroke the border centerline (halfway ring, radii averaged)
            // at the side's width; the quad clip keeps this side only.
            let centerline = inset_shape(fragment, 0.5);
            let stroke = dash_stroke(side.line, side.width);
            with_shape!(&centerline, |shape| scene
                .stroke(&stroke, transform, side.color, None, shape));
        }
        // Filtered out in `paintable_sides`.
        BorderStyle::None | BorderStyle::Hidden => {}
    }
    scene.pop_layer();
}

/// The CSS2 §8.5.4 miter quad for one side, rebuilt into the caller's
/// reusable buffer: outer box corners joined to the matching inner
/// (padding-box) corners. With rounded corners the diagonal splits the
/// corner region between adjacent sides — behaviorally fine.
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

/// Corner radii of the concentric ring boundary `fraction` of the way from
/// the border-box edge (0) to the padding-box edge (1): the outer radii
/// shrunk by the fraction-scaled side widths, clamped at zero. Linear in
/// `fraction` until a radius clamps — `double`'s sub-ring boundaries and
/// the dashed/dotted centerline (fraction ½, the outer/inner average) rely
/// on this interpolation.
fn ring_boundary_radii(radii: &CornerRadii, widths: &Edges<f32>, fraction: f32) -> CornerRadii {
    inner_radii(radii, &widths.map(|width| width * fraction))
}

/// The ring boundary `fraction` of the way through the border as a shape:
/// the border box inset by the fraction-scaled side widths, with
/// [`ring_boundary_radii`], degenerate-clamped like the padding box.
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

/// The dash geometry for `dashed`/`dotted`, scaled by the line width `w`
/// (css-backgrounds-3 §3.2 leaves exact patterns to the UA):
/// - `dashed`: 2w dashes with 1w gaps — a 3w period, inside the browsers' typical 2w–3w dash /
///   1w–3w gap envelope.
/// - `dotted`: near-zero-length dashes with round caps on a 2w period — circular dots of diameter
///   w, 2w apart center to center, as browsers draw them. The dash length must not be exactly zero:
///   vello 0.9 GPU-strokes every path (`scene.rs` `GPU_STROKES = true`), and a zero-length dash
///   reaches `vello_encoding`'s `PathEncoder` as a coincident `MoveTo`/`LineTo` pair whose segment
///   `line_to` rejects (no start tangent between coincident points) and whose dangling `MoveTo`
///   `finish` truncates — so the whole dotted border encodes zero segments and paints nothing. A
///   width-proportional stub (5% of w) keeps the dot visible as a round-cap circle and cannot
///   collapse under f32 rounding at large scene coordinates the way an absolute epsilon could; the
///   gap shrinks by the stub so the period stays exactly 2w.
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

/// The single shade `inset`/`outset` give a side (CSS2 §8.5.3: `inset`
/// sinks the box — top/left dark, bottom/right light; `outset` raises it —
/// the reverse). Other styles keep the side's own color.
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

/// The (outer-half, inner-half) shades for `groove`/`ridge` (CSS2 §8.5.3:
/// `groove` looks carved in — outer half of top/left dark, inner half
/// light, mirrored on bottom/right; `ridge` is the inverse).
fn split_shades(line: BorderStyle, side: Side, color: Color) -> (Color, Color) {
    let dark_outer = matches!(side, Side::Top | Side::Left) == (line == BorderStyle::Groove);
    if dark_outer {
        (darken(color), lighten(color))
    } else {
        (lighten(color), darken(color))
    }
}

/// CSS2 §8.5.3 leaves the 3D "darker" shade to the UA; browsers scale RGB
/// toward black by about a third — dark = ⅔·c, alpha preserved.
fn darken(color: Color) -> Color {
    shade(color, |channel| channel * (2.0 / 3.0))
}

/// The matching light shade: blend a third of the way toward white —
/// light = c + (1 − c)/3, alpha preserved.
fn lighten(color: Color) -> Color {
    shade(color, |channel| channel + (1.0 - channel) / 3.0)
}

fn shade(color: Color, tone: impl Fn(f32) -> f32) -> Color {
    let [r, g, b, a] = color.components;
    Color::new([tone(r), tone(g), tone(b), a])
}

/// Bit-exact color equality (identical resolution paths produce identical
/// bits; avoids float comparison).
fn same_color(a: Color, b: Color) -> bool {
    a.components.map(f32::to_bits) == b.components.map(f32::to_bits)
}

/// Paints the outline ring outside the border box.
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
        // Centerline stroke, mirroring the border sides — one whole-ring
        // stroke (outlines have no per-side styles to miter between).
        let half = width / 2.0;
        let centerline = BoxShape::new(
            fragment.border_box.inflate(half, half),
            &grow_radii(&fragment.radii, half as f32),
        );
        let stroke = dash_stroke(line, width);
        with_shape!(&centerline, |shape| scene
            .stroke(&stroke, transform, color, None, shape));
    } else {
        // `auto` and every other paintable style draw as a solid ring —
        // the double/3D sub-ring split is not worth machinery no bundle
        // can author yet (recorded approximation). `none`/`hidden` never
        // reach here (`resolved_outline` gates on a painting style).
        let outer = BoxShape::new(
            fragment.border_box.inflate(width, width),
            &grow_radii(&fragment.radii, width as f32),
        );
        let inner = BoxShape::new(fragment.border_box, &fragment.radii);
        ring_path_into(&mut paths.ring, &outer, &inner);
        scene.fill(Fill::EvenOdd, transform, color, None, &paths.ring);
    }
}

/// How far outside the border box the outline extends (its used width), for
/// layer-bounds accounting. Zero when no outline paints.
pub(crate) fn outline_extent(style: &ComputedValues) -> f64 {
    resolved_outline(style).map_or(0.0, |(width, _, _)| width)
}

/// The used outline `(width, style, color)` when one would paint: style not
/// `none`/`hidden` (`auto` paints as solid), nonzero used width,
/// non-transparent color. The fork's lynx grammar seeds
/// `outline`/`outline-color`/`outline-style`/`outline-width` and
/// deliberately omits `outline-offset` (Lynx outlines are flush rings), so
/// the geometry above never needs an offset.
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

/// Outline ring radii: the border-box radii grown by `by` so the ring stays
/// concentric with the box. Sharp corners stay sharp — a corner with either
/// radius component at zero is square (css-backgrounds-3 §5) and keeps a
/// square outline corner, the same corner treatment §7.4.1 gives
/// spread-inflated shadows.
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
    use vello::kurbo::{Point, Shape};

    use super::*;

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
        // Outer third boundary of a `double` border: outer radius minus a
        // third of the adjacent side widths.
        let third = ring_boundary_radii(&radii, &widths, 1.0 / 3.0);
        assert_close(third.top_left.width, 12.0 - 3.0);
        assert_close(third.top_left.height, 9.0 - 2.0);
        // Inner third boundary: two thirds in.
        let two_thirds = ring_boundary_radii(&radii, &widths, 2.0 / 3.0);
        assert_close(two_thirds.top_left.width, 12.0 - 6.0);
        assert_close(two_thirds.top_left.height, 9.0 - 4.0);
        // Fraction 1 is the padding-box radius.
        let inner = ring_boundary_radii(&radii, &widths, 1.0);
        assert_close(inner.top_left.width, 3.0);
        assert_close(inner.top_left.height, 3.0);
        // Small radii clamp at zero instead of going negative.
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
        // Stub dash (5% of w, nonzero so vello's encoder keeps it) plus its
        // gap keep the exact 2w period.
        assert!((stroke.dash_pattern[0] - 0.15).abs() < 1e-12);
        assert!(stroke.dash_pattern[0] > 0.0);
        assert!((stroke.dash_pattern[1] - 5.85).abs() < 1e-12);
        assert!((stroke.dash_pattern[0] + stroke.dash_pattern[1] - 6.0).abs() < 1e-12);
        assert_eq!(stroke.start_cap, Cap::Round);
        assert_eq!(stroke.end_cap, Cap::Round);
    }

    #[test]
    fn dash_strokes_encode_nonempty_gpu_stroke_segments() {
        // Regression guard for the vello 0.9 GPU-stroke encoder dropping
        // zero-length dashes (coincident MoveTo/LineTo pairs have no start
        // tangent, so `PathEncoder::line_to` rejects them and `finish`
        // truncates the dangling MoveTo): a dotted stroke around a rect must
        // encode actual path segments, not silently vanish.
        let rect = Rect::new(0.0, 0.0, 100.0, 40.0);
        for line in [BorderStyle::Dotted, BorderStyle::Dashed] {
            let stroke = dash_stroke(line, 3.0);
            let mut scene = Scene::new();
            scene.stroke(
                &stroke,
                vello::kurbo::Affine::IDENTITY,
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
        // left 4, top 6, right 4, bottom 8.
        let outer = Rect::new(0.0, 0.0, 40.0, 30.0);
        let inner = Rect::new(4.0, 6.0, 36.0, 22.0);
        let side_quad = |side| {
            let mut path = BezPath::new();
            side_quad_into(&mut path, side, outer, inner);
            path
        };
        let top = side_quad(Side::Top);
        let left = side_quad(Side::Left);
        // Middle of the top band belongs to the top quad only.
        assert_ne!(top.winding(Point::new(20.0, 3.0)), 0);
        assert_eq!(left.winding(Point::new(20.0, 3.0)), 0);
        // The top-left corner diagonal runs (0,0) → (4,6), i.e. y = 1.5x:
        // (1, 5) sits below it (left side), (3, 2) above it (top side).
        assert_eq!(top.winding(Point::new(1.0, 5.0)), 0);
        assert_ne!(left.winding(Point::new(1.0, 5.0)), 0);
        assert_ne!(top.winding(Point::new(3.0, 2.0)), 0);
        assert_eq!(left.winding(Point::new(3.0, 2.0)), 0);
        // Nothing from the bottom band leaks into the top quad.
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
        // Border box 60×40, widths (l 9, r 6, t 6, b 3): the halfway
        // centerline sits at the average of the two boxes and fraction 1
        // reproduces the padding box.
        let widths = Edges {
            left: 9.0_f32,
            right: 6.0,
            top: 6.0,
            bottom: 3.0,
        };
        let padding_box = Rect::new(9.0, 6.0, 54.0, 37.0);
        let fragment = BoxFragment {
            node: 0,
            transform: vello::kurbo::Affine::IDENTITY,
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
