//! Text runs: retained Parley layouts as vello glyph runs, plus
//! decorations, `text-shadow` (offset + color, no blur — recorded v1
//! limit), and `text-stroke`.
//!
//! Spec sketch:
//! - Walk `layout.parley_layout().lines()` → `line.items()` → `PositionedLayoutItem::GlyphRun`; for
//!   each run: `scene.draw_glyphs(run.font())` with `font_size`, the run's `normalized_coords`
//!   (parley coord i16s convert to `vello::NormalizedCoord`), `hint(false)` (transforms are
//!   arbitrary), `brush` = the element's used `color`, glyphs mapped from
//!   `glyph_run.positioned_glyphs()`.
//! - Synthesis (fake bold/oblique) from `run.synthesis()`: embolden via
//!   `DrawGlyphs::font_embolden`, oblique via `glyph_transform` skew.
//! - `text-shadow`: repeat the glyph pass per shadow (last-specified first, under the main pass),
//!   offset by the shadow offset, shadow color.
//! - `text-stroke` (`-webkit-text-stroke` semantics): a second glyph pass drawn with
//!   `StyleRef::Stroke` of the stroke width and color, over the fill (`WebKit` and Lynx native
//!   convention: fill first, stroke on top).
//! - Decorations from the element style (`text-decoration-line`/`style`/ thickness defaults): per
//!   line, per run — underline/line-through rects from `run.metrics()` (offsets are
//!   baseline-relative); `wavy` builds one sine-period path tiled across the run advance; `double`
//!   draws two lines a thickness apart. The stylo fork compiles `overline` out of
//!   `text-decoration-line` under the `lynx` feature (Lynx's decoration bitflags have no overline),
//!   so only underline and line-through can reach the painter.
//! - The whole painter works in the text item's local space (origin at the text box's top-left,
//!   which is also the Parley layout origin); `transform` already includes the device scale.

use dom::layout::TextLayout;
use parley::{GlyphRun, Layout, PositionedLayoutItem};
use smallvec::SmallVec;
use stylo::computed_values::text_decoration_style::T as TextDecorationStyle;
use stylo::properties::ComputedValues;
use stylo::values::computed::TextDecorationLine;
use vello::kurbo::{Affine, BezPath, Diagonal2, Line, Rect, Stroke};
use vello::peniko::{Color, Fill, StyleRef};
use vello::{FontEmbolden, Scene};

use crate::convert;

/// Paints one text item's committed layout with the styled parent element's
/// paint properties.
pub(crate) fn paint(
    scene: &mut Scene,
    style: &ComputedValues,
    layout: &TextLayout,
    transform: Affine,
    decorations: &[Decorations],
) {
    let layout = layout.parley_layout();

    // text-shadow passes go under everything else, last-specified first so
    // the first-specified shadow paints on top (css-text-decor-3 §4). Only
    // the offset and color paint; blur is not painted — recorded v1 limit.
    // The shadow silhouette includes the decorations (the spec shadows "the
    // text and all its decorations") but not the text-stroke pass.
    for shadow in style.get_inherited_text().text_shadow.0.iter().rev() {
        let color = convert::resolve_color(style, &shadow.color);
        let offset = Affine::translate((
            f64::from(shadow.horizontal.px()),
            f64::from(shadow.vertical.px()),
        ));
        let shadowed: SmallVec<[Decorations; 2]> = decorations
            .iter()
            .map(|deco| Decorations { color, ..*deco })
            .collect();
        paint_pass(scene, layout, transform * offset, color, None, &shadowed);
    }

    // The element's used `color`. Lynx's gradient-valued `color` (text-
    // gradient sugar) is not painted as a gradient: `clone_color()` collapses
    // a gradient to the fork's parallel solid color, so the text paints that
    // currentcolor fallback instead — recorded v1 limit.
    let fill = convert::current_color(style);
    paint_pass(
        scene,
        layout,
        transform,
        fill,
        text_stroke(style),
        decorations,
    );
}

/// The decorations that apply to text under `element` — css-text-decor-3
/// §2: `text-decoration-line` is **not inherited**; each ancestor box with
/// a decoration is a *decorating box* whose lines propagate to all in-flow
/// descendant text, drawn with the originating box's own style and color.
/// Collected nearest-first by walking the DOM ancestors; propagation from
/// ancestors stops at an out-of-flow (absolutely positioned) box, which per
/// spec does not receive them — that box's own decorations still apply.
/// Boxless (`display: contents`) ancestors are treated as decorating boxes,
/// matching browser rendering of decorated `display: contents` spans.
pub(crate) fn propagated_decorations<T>(
    document: &dom::Document<T>,
    element: dom::NodeId,
) -> SmallVec<[Decorations; 2]> {
    use stylo::computed_values::position::T as Position;
    let mut out = SmallVec::new();
    let mut current = Some(element);
    while let Some(id) = current {
        let Some(node) = document.get(id) else { break };
        if !node.is_element() {
            break;
        }
        let Some(style) = document.paint_style(id) else {
            break;
        };
        if let Some(deco) = decorations(style) {
            out.push(deco);
        }
        if matches!(
            style.get_box().position,
            Position::Absolute | Position::Fixed
        ) {
            break;
        }
        current = node.parent_id();
    }
    out
}

/// How far outside the text box this style's text can paint: the largest
/// `text-shadow` offset reach (`max(|h|, |v|)` across shadows; blur is
/// excluded because it is not painted — recorded v1 limit) plus half a
/// `text-stroke` width. Glyph overshoot itself is padded by the caller.
pub(crate) fn extent(style: &ComputedValues) -> f64 {
    let shadow_reach = style
        .get_inherited_text()
        .text_shadow
        .0
        .iter()
        .map(|shadow| f64::from(shadow.horizontal.px().abs().max(shadow.vertical.px().abs())))
        .fold(0.0, f64::max);
    let stroke_reach = text_stroke(style).map_or(0.0, |(width, _)| width / 2.0);
    shadow_reach + stroke_reach
}

/// One decorating box's paint: which lines it draws, in its style/color.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Decorations {
    underline: bool,
    line_through: bool,
    style: TextDecorationStyle,
    color: Color,
}

fn decorations(style: &ComputedValues) -> Option<Decorations> {
    let text = style.get_text();
    let line = text.text_decoration_line;
    // The `lynx` fork compiles `text-decoration-line` without an `OVERLINE`
    // bit (Lynx's TextDecorationType has none), so only these two exist.
    let underline = line.contains(TextDecorationLine::UNDERLINE);
    let line_through = line.contains(TextDecorationLine::LINE_THROUGH);
    if !(underline || line_through)
        || matches!(text.text_decoration_style, TextDecorationStyle::MozNone)
    {
        return None;
    }
    Some(Decorations {
        underline,
        line_through,
        style: text.text_decoration_style,
        // The fork keeps the real `text-decoration-color` longhand (initial
        // value currentcolor), which Lynx also exposes; resolving it against
        // the element covers both the authored and the currentcolor case.
        color: convert::resolve_color(style, &text.text_decoration_color),
    })
}

/// The `text-stroke` paint (`-webkit-text-stroke` semantics): the fork
/// compiles the `-webkit-text-stroke-width`/`-color` longhands (with Lynx's
/// unprefixed `text-stroke-*` aliases) under the `lynx` feature.
fn text_stroke(style: &ComputedValues) -> Option<(f64, Color)> {
    let inherited = style.get_inherited_text();
    let width = inherited._webkit_text_stroke_width.to_f64_px();
    (width > 0.0).then(|| {
        (
            width,
            convert::resolve_color(style, &inherited._webkit_text_stroke_color),
        )
    })
}

/// One full silhouette pass over the layout: per glyph run, decorations
/// under the text (underline), the glyphs (optional stroke pass over the
/// fill — `WebKit` and Lynx native convention: fill first, stroke on top),
/// then decorations over the text (line-through) per css-text-decor-3's
/// painting order.
/// Draws only the glyph ink — no decorations, shadows, or stroke — in an
/// opaque color: the alpha source for `background-clip: text` sandwiches
/// (css-backgrounds-4 clips to the text; decorations not contributing to
/// the clip is a recorded v1 limit shared with the module doc).
pub(crate) fn paint_silhouette(scene: &mut Scene, layout: &TextLayout, transform: Affine) {
    paint_pass(
        scene,
        layout.parley_layout(),
        transform,
        Color::BLACK,
        None,
        &[],
    );
}

fn paint_pass(
    scene: &mut Scene,
    layout: &Layout<()>,
    transform: Affine,
    fill: Color,
    stroke: Option<(f64, Color)>,
    decorations: &[Decorations],
) {
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let metrics = *glyph_run.run().metrics();
            let baseline = f64::from(glyph_run.baseline());
            let x = f64::from(glyph_run.offset());
            let width = f64::from(glyph_run.advance());

            for deco in decorations.iter().rev().filter(|deco| deco.underline) {
                let band = band(
                    x,
                    width,
                    baseline,
                    f64::from(metrics.underline_offset),
                    f64::from(metrics.underline_size),
                );
                paint_band(
                    scene,
                    transform,
                    deco.color,
                    deco.style,
                    DecorationKind::Underline,
                    &band,
                );
            }

            draw_glyph_run(scene, &glyph_run, transform, Fill::NonZero.into(), fill);
            if let Some((stroke_width, stroke_color)) = stroke {
                let stroke_style = Stroke::new(stroke_width);
                draw_glyph_run(
                    scene,
                    &glyph_run,
                    transform,
                    (&stroke_style).into(),
                    stroke_color,
                );
            }

            for deco in decorations.iter().rev().filter(|deco| deco.line_through) {
                let band = band(
                    x,
                    width,
                    baseline,
                    f64::from(metrics.strikethrough_offset),
                    f64::from(metrics.strikethrough_size),
                );
                paint_band(
                    scene,
                    transform,
                    deco.color,
                    deco.style,
                    DecorationKind::LineThrough,
                    &band,
                );
            }
        }
    }
}

/// Encodes one positioned glyph run with its synthesis (fake bold/oblique).
fn draw_glyph_run(
    scene: &mut Scene,
    glyph_run: &GlyphRun<'_, ()>,
    transform: Affine,
    style: StyleRef<'_>,
    color: Color,
) {
    let run = glyph_run.run();
    let synthesis = run.synthesis();
    let embolden = if synthesis.embolden() {
        // FreeType's synthetic-bold convention: total strength = size / 24,
        // half of it applied to each side of the outline.
        let amount = f64::from(run.font_size()) / 48.0;
        FontEmbolden::new(Diagonal2::new(amount, amount))
    } else {
        FontEmbolden::default()
    };
    // The glyph transform applies in y-up outline space; a positive skew
    // factor leans the glyph top rightward (faux oblique).
    let glyph_transform = synthesis
        .skew()
        .map(|degrees| Affine::skew(f64::from(degrees).to_radians().tan(), 0.0));
    scene
        .draw_glyphs(run.font())
        .font_size(run.font_size())
        .transform(transform)
        .glyph_transform(glyph_transform)
        .font_embolden(embolden)
        // parley and vello both use bare `i16` F2DOT14 coords
        // (`vello::NormalizedCoord` is an `i16` alias), so the run's slice
        // passes straight through.
        .normalized_coords(run.normalized_coords())
        .hint(false)
        .brush(color)
        .draw(
            style,
            glyph_run.positioned_glyphs().map(|glyph| vello::Glyph {
                id: glyph.id,
                x: glyph.x,
                y: glyph.y,
            }),
        );
}

/// One decoration line's geometry in item-local space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DecorationBand {
    /// Left edge (the glyph run's offset).
    x: f64,
    /// Width (the glyph run's advance).
    width: f64,
    /// Top edge of the single/solid band.
    top: f64,
    /// Band thickness from the run's font metrics (the fork compiles
    /// `text-decoration-thickness` gecko-only, so metrics always decide).
    thickness: f64,
}

impl DecorationBand {
    fn rect(&self, top: f64) -> Rect {
        Rect::new(self.x, top, self.x + self.width, top + self.thickness)
    }

    fn centerline(&self) -> f64 {
        self.top + self.thickness / 2.0
    }
}

/// Parley's `RunMetrics` decoration fields are y-up distances from the
/// baseline to the **top** of the decoration (negative = below the
/// baseline); layout space is y-down with the baseline at `baseline`, so
/// the band top is `baseline - offset`.
fn band(x: f64, width: f64, baseline: f64, offset: f64, thickness: f64) -> DecorationBand {
    DecorationBand {
        x,
        width,
        top: baseline - offset,
        thickness,
    }
}

#[derive(Clone, Copy, Debug)]
enum DecorationKind {
    Underline,
    LineThrough,
}

/// Both band tops for `text-decoration-style: double`: underlines grow away
/// from the text (downward) while line-throughs stay centered on the strike
/// position; the gap equals one thickness. Exact double-line geometry is UA
/// discretion (css-text-decor-3 §2.2).
fn double_tops(kind: DecorationKind, top: f64, thickness: f64) -> [f64; 2] {
    match kind {
        DecorationKind::Underline => [top, top + 2.0 * thickness],
        DecorationKind::LineThrough => [top - thickness, top + thickness],
    }
}

fn paint_band(
    scene: &mut Scene,
    transform: Affine,
    color: Color,
    line_style: TextDecorationStyle,
    kind: DecorationKind,
    band: &DecorationBand,
) {
    if band.width <= 0.0 || band.thickness <= 0.0 {
        return;
    }
    match line_style {
        TextDecorationStyle::Solid => {
            scene.fill(Fill::NonZero, transform, color, None, &band.rect(band.top));
        }
        TextDecorationStyle::Double => {
            for top in double_tops(kind, band.top, band.thickness) {
                scene.fill(Fill::NonZero, transform, color, None, &band.rect(top));
            }
        }
        TextDecorationStyle::Dotted | TextDecorationStyle::Dashed => {
            // Dash geometry is UA discretion (css-text-decor-3 §2.2):
            // butt-capped thickness-sized dots / 3×-thickness dashes, gaps
            // equal to the painted segment. Lynx's `-x-text-decoration-width`
            // and `-x-text-decoration-gap` knobs are not in the fork's
            // grammar, so they cannot override this.
            let segment = if matches!(line_style, TextDecorationStyle::Dotted) {
                band.thickness
            } else {
                3.0 * band.thickness
            };
            let stroke = Stroke::new(band.thickness).with_dashes(0.0, [segment, segment]);
            let centerline = Line::new(
                (band.x, band.centerline()),
                (band.x + band.width, band.centerline()),
            );
            scene.stroke(&stroke, transform, color, None, &centerline);
        }
        TextDecorationStyle::Wavy => {
            scene.stroke(
                &Stroke::new(band.thickness),
                transform,
                color,
                None,
                &wavy_path(band),
            );
        }
        // Gecko-legacy alias of `none`; filtered out before painting.
        TextDecorationStyle::MozNone => {}
    }
}

/// A sine-like wave along the band's centerline, stroked at the band
/// thickness. Wave geometry is UA discretion (css-text-decor-3 §2.2); the
/// recorded choices are amplitude = thickness and wavelength = 6 ×
/// thickness. Each half-period is one symmetric cubic arch: a cubic with
/// both interior control points lifted by `k` peaks at `3k/4` (at `t =
/// 0.5`), so lifting by 4/3 × amplitude peaks exactly at the amplitude.
/// The final partial arch is compressed to end exactly at the band's right
/// edge, with its amplitude scaled by the same ratio so the wave flattens
/// out instead of steepening.
fn wavy_path(band: &DecorationBand) -> BezPath {
    let amplitude = band.thickness;
    let half_period = 3.0 * band.thickness;
    let centerline = band.centerline();
    let end = band.x + band.width;

    let mut path = BezPath::new();
    path.move_to((band.x, centerline));
    let mut start = band.x;
    // The first arch rises above the centerline (negative y is up in the
    // item's y-down space).
    let mut sign = -1.0;
    while end - start > 1e-6 {
        let arch_width = half_period.min(end - start);
        let lift = sign * amplitude * (4.0 / 3.0) * (arch_width / half_period);
        path.curve_to(
            (start + arch_width / 3.0, centerline + lift),
            (start + 2.0 * arch_width / 3.0, centerline + lift),
            (start + arch_width, centerline),
        );
        start += arch_width;
        sign = -sign;
    }
    path
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use vello::kurbo::{CubicBez, ParamCurve, PathEl, Point};

    use super::*;

    fn assert_near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    /// The cubic segments of a path built as one move plus curves.
    fn cubics(path: &BezPath) -> Vec<CubicBez> {
        let mut segments = Vec::new();
        let mut current = Point::ZERO;
        for element in path.elements() {
            match *element {
                PathEl::MoveTo(point) => current = point,
                PathEl::CurveTo(p1, p2, p3) => {
                    segments.push(CubicBez::new(current, p1, p2, p3));
                    current = p3;
                }
                _ => panic!("wavy paths contain only moves and cubics"),
            }
        }
        segments
    }

    #[test]
    fn wavy_waves_tile_whole_arches_that_peak_at_the_amplitude() {
        // thickness 2 → amplitude 2, half-period 6; width 12 = two arches.
        let band = DecorationBand {
            x: 10.0,
            width: 12.0,
            top: 100.0,
            thickness: 2.0,
        };
        let path = wavy_path(&band);
        let segments = cubics(&path);
        assert_eq!(segments.len(), 2);

        let centerline = 101.0;
        assert_eq!(segments[0].p0, Point::new(10.0, centerline));
        assert_eq!(segments[0].p3, Point::new(16.0, centerline));
        assert_eq!(segments[1].p3, Point::new(22.0, centerline));
        // A symmetric arch peaks at t = 0.5; the first rises, the second dips.
        assert_near(segments[0].eval(0.5).y, centerline - 2.0);
        assert_near(segments[1].eval(0.5).y, centerline + 2.0);
    }

    #[test]
    fn wavy_final_partial_arches_compress_to_the_band_edge() {
        // Width 9 with half-period 6: one full arch plus a half-width tail.
        let band = DecorationBand {
            x: 0.0,
            width: 9.0,
            top: 0.0,
            thickness: 2.0,
        };
        let path = wavy_path(&band);
        let segments = cubics(&path);
        assert_eq!(segments.len(), 2);
        let centerline = 1.0;
        assert_eq!(segments[1].p3, Point::new(9.0, centerline));
        // The 3-wide tail scales its amplitude by 3/6 = 0.5 → peaks at 1.
        assert_near(segments[1].eval(0.5).y, centerline + 1.0);
    }

    #[test]
    fn double_underlines_grow_down_and_double_strikes_straddle() {
        let [first, second] = double_tops(DecorationKind::Underline, 10.0, 2.0);
        assert_near(first, 10.0);
        assert_near(second, 14.0);
        let [above, below] = double_tops(DecorationKind::LineThrough, 10.0, 2.0);
        assert_near(above, 8.0);
        assert_near(below, 12.0);
    }

    #[test]
    fn bands_convert_parley_y_up_offsets_to_y_down_tops() {
        // Underline metrics sit below the baseline (negative y-up offset).
        let underline = band(5.0, 40.0, 20.0, -3.0, 1.5);
        assert_near(underline.top, 23.0);
        assert_eq!(
            underline.rect(underline.top),
            Rect::new(5.0, 23.0, 45.0, 24.5)
        );
        // Strikethrough metrics sit above the baseline (positive offset).
        let strike = band(5.0, 40.0, 20.0, 8.0, 1.0);
        assert_near(strike.top, 12.0);
        assert_near(strike.centerline(), 12.5);
    }
}
