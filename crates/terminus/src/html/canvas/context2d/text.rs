//! Text: the default font, shaping one line through
//! [`shape_line`](seldon::text::shape_line), and the metrics `measureText`
//! reports (HTML "Drawing text to the bitmap").

use std::borrow::Cow;

use seldon::text::{
    FontMetrics, GenericFamily, LineOptions, LineWhiteSpace, ResolvedFamily, ResolvedFont,
    ResolvedFontStyle, ShapedLine, TextContext, font_metrics, shape_line,
};
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

use super::{Direction, TextAlign, TextBaseline};
use crate::kurbo::Rect;

/// The `font` a context starts with: `10px sans-serif`.
pub(crate) fn default_font() -> ResolvedFont {
    ResolvedFont {
        families: vec![ResolvedFamily::Generic(GenericFamily::SansSerif)],
        size: 10.0,
        weight: 400.0,
        style: ResolvedFontStyle::Normal,
        stretch: 1.0,
    }
}

/// `TextMetrics`, every distance in CSS px from the alignment point
/// `textAlign` and `textBaseline` select, with the standard's signs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextMetrics {
    pub width: f64,
    pub actual_bounding_box_left: f64,
    pub actual_bounding_box_right: f64,
    pub font_bounding_box_ascent: f64,
    pub font_bounding_box_descent: f64,
    pub actual_bounding_box_ascent: f64,
    pub actual_bounding_box_descent: f64,
    pub em_height_ascent: f64,
    pub em_height_descent: f64,
    pub hanging_baseline: f64,
    pub alphabetic_baseline: f64,
    pub ideographic_baseline: f64,
}

/// One shaped line: its glyph runs, x from the line's left edge and y down
/// from the alphabetic baseline, and the metrics it is anchored by.
#[derive(Debug)]
pub(crate) struct Line {
    pub(crate) shaped: ShapedLine,
    pub(crate) width: f64,
    /// The first available font's ascent and descent.
    ascent: f64,
    descent: f64,
}

impl Line {
    /// Shapes `text` as one unwrapped line, after the text preparation
    /// algorithm's whitespace step: every ASCII whitespace character becomes
    /// a space, and every space is kept.
    pub(crate) fn shape(
        context: &mut TextContext,
        text: &str,
        font: &ResolvedFont,
        direction: Direction,
    ) -> Self {
        const WHITESPACE: [char; 4] = ['\t', '\n', '\x0C', '\r'];
        let text: Cow<'_, str> = if text.contains(WHITESPACE) {
            Cow::Owned(text.replace(WHITESPACE, " "))
        } else {
            Cow::Borrowed(text)
        };
        let options = LineOptions {
            letter_spacing: 0.0,
            // `inherit` with no element to inherit from is `ltr`.
            direction: Some(if direction == Direction::Rtl {
                seldon::text::paragraph::Direction::Rtl
            } else {
                seldon::text::paragraph::Direction::Ltr
            }),
            white_space: LineWhiteSpace::Preserve,
        };
        let shaped = shape_line(context, &text, font, options);
        // Text with no glyph run (empty text) names no font: the first
        // available font's metrics are read off a shaped letter.
        let size = f64::from(font.size);
        let (ascent, descent) = shaped
            .first_font
            .or_else(|| font_metrics(context, font))
            .map_or((size * 0.8, size * 0.2), |metrics: FontMetrics| {
                (f64::from(metrics.ascent), f64::from(metrics.descent))
            });
        Self {
            width: f64::from(shaped.advance),
            shaped,
            ascent,
            descent,
        }
    }

    /// The y of `baseline` relative to the alphabetic baseline, down
    /// positive.
    ///
    /// The em square's top and bottom split the font size in the ratio of
    /// the font's ascent to its descent. The hanging baseline sits at 80% of
    /// the ascent and the ideographic-under baseline at the descent: the
    /// standard leaves these to the font's baseline table, which is not
    /// read.
    pub(crate) fn baseline_offset(&self, baseline: TextBaseline, size: f64) -> f64 {
        let total = self.ascent + self.descent;
        let (em_ascent, em_descent) = if total > 0.0 {
            (size * self.ascent / total, size * self.descent / total)
        } else {
            (size, 0.0)
        };
        match baseline {
            TextBaseline::Top => -em_ascent,
            TextBaseline::Hanging => -0.8 * self.ascent,
            TextBaseline::Middle => (em_descent - em_ascent) / 2.0,
            TextBaseline::Alphabetic => 0.0,
            TextBaseline::Ideographic => self.descent,
            TextBaseline::Bottom => em_descent,
        }
    }

    /// The union of the shaped glyphs' outline bounds, relative to the
    /// line's left edge on the alphabetic baseline, y down; `None` for no
    /// ink.
    pub(crate) fn ink_bounds(&self) -> Option<Rect> {
        let mut bounds: Option<Rect> = None;
        for run in &self.shaped.runs {
            let Ok(font) = FontRef::from_index(run.font.data.data(), run.font.index) else {
                continue;
            };
            let coords: Vec<NormalizedCoord> = run
                .normalized_coords
                .iter()
                .map(|coord| NormalizedCoord::from_bits(*coord))
                .collect();
            let outlines = font.outline_glyphs();
            for glyph in &run.glyphs {
                let Some(outline) = outlines.get(GlyphId::new(glyph.id)) else {
                    continue;
                };
                let mut pen = BoundsPen::default();
                let settings =
                    DrawSettings::unhinted(Size::new(run.size), LocationRef::new(&coords));
                if outline.draw(settings, &mut pen).is_err() {
                    continue;
                }
                let Some(glyph_bounds) = pen.bounds else {
                    continue;
                };
                // Outlines are y-up; the line is y-down.
                let placed = Rect::new(
                    f64::from(glyph.x) + glyph_bounds.x0,
                    f64::from(glyph.y) - glyph_bounds.y1,
                    f64::from(glyph.x) + glyph_bounds.x1,
                    f64::from(glyph.y) - glyph_bounds.y0,
                );
                bounds = Some(bounds.map_or(placed, |b| b.union(placed)));
            }
        }
        bounds
    }
}

/// The physical anchor `textAlign` picks, as a fraction of the line width
/// from its left edge.
pub(crate) fn align_fraction(align: TextAlign, direction: Direction) -> f64 {
    let rtl = direction == Direction::Rtl;
    match align {
        TextAlign::Center => 0.5,
        TextAlign::Right => 1.0,
        TextAlign::Start if rtl => 1.0,
        TextAlign::End if !rtl => 1.0,
        TextAlign::Left | TextAlign::Start | TextAlign::End => 0.0,
    }
}

/// The metrics `measureText` answers for a shaped line.
pub(crate) fn metrics(
    line: &Line,
    ink: Option<Rect>,
    size: f64,
    align: TextAlign,
    baseline: TextBaseline,
    direction: Direction,
) -> TextMetrics {
    let anchor_x = line.width * align_fraction(align, direction);
    let anchor_y = line.baseline_offset(baseline, size);
    let em_over = line.baseline_offset(TextBaseline::Top, size);
    let em_under = line.baseline_offset(TextBaseline::Bottom, size);
    let (left, right, ascent, descent) = ink.map_or((0.0, 0.0, 0.0, 0.0), |ink| {
        (
            anchor_x - ink.x0,
            ink.x1 - anchor_x,
            anchor_y - ink.y0,
            ink.y1 - anchor_y,
        )
    });
    TextMetrics {
        width: line.width,
        actual_bounding_box_left: left,
        actual_bounding_box_right: right,
        font_bounding_box_ascent: anchor_y + line.ascent,
        font_bounding_box_descent: line.descent - anchor_y,
        actual_bounding_box_ascent: ascent,
        actual_bounding_box_descent: descent,
        em_height_ascent: anchor_y - em_over,
        em_height_descent: em_under - anchor_y,
        hanging_baseline: anchor_y - line.baseline_offset(TextBaseline::Hanging, size),
        alphabetic_baseline: anchor_y,
        ideographic_baseline: anchor_y - line.baseline_offset(TextBaseline::Ideographic, size),
    }
}

/// An outline pen keeping the exact bounds of the segments drawn.
#[derive(Default)]
struct BoundsPen {
    current: (f64, f64),
    bounds: Option<Rect>,
}

impl BoundsPen {
    fn add(&mut self, rect: Rect) {
        self.bounds = Some(self.bounds.map_or(rect, |b| b.union(rect)));
    }
}

impl OutlinePen for BoundsPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.current = (f64::from(x), f64::from(y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        use crate::kurbo::{Line as Segment, ParamCurveExtrema};
        let end = (f64::from(x), f64::from(y));
        self.add(Segment::new(self.current, end).bounding_box());
        self.current = end;
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        use crate::kurbo::{ParamCurveExtrema, QuadBez};
        let end = (f64::from(x), f64::from(y));
        let quad = QuadBez::new(self.current, (f64::from(cx0), f64::from(cy0)), end);
        self.add(quad.bounding_box());
        self.current = end;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        use crate::kurbo::{CubicBez, ParamCurveExtrema};
        let end = (f64::from(x), f64::from(y));
        let cubic = CubicBez::new(
            self.current,
            (f64::from(cx0), f64::from(cy0)),
            (f64::from(cx1), f64::from(cy1)),
            end,
        );
        self.add(cubic.bounding_box());
        self.current = end;
    }

    fn close(&mut self) {}
}
