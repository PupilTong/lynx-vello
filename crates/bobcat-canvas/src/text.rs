//! Text: the font description, shaping one line with parley, and the
//! metrics `measureText` reports (HTML "Drawing text to the bitmap").

use std::borrow::Cow;
use std::sync::Arc;

use parley::{
    BaseDirection, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight, FontWidth,
    GenericFamily, Layout, LayoutContext, PositionedLayoutItem, StyleProperty, WhiteSpaceCollapse,
};
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use terminus::link::kurbo::Rect;

use crate::{Direction, TextAlign, TextBaseline};

/// The resolved `font`: what the CSS `font` shorthand names, every keyword
/// already a number. Cloning shares the family list.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    /// The `font-family` list in preference order, generic families
    /// included.
    pub families: Arc<[FontFamilyName<'static>]>,
    /// The font size in CSS px.
    pub size: f32,
    pub weight: FontWeight,
    pub style: FontStyle,
    /// `font-stretch`.
    pub width: FontWidth,
}

impl Default for Font {
    /// `10px sans-serif`.
    fn default() -> Self {
        Self {
            families: Arc::new([FontFamilyName::Generic(GenericFamily::SansSerif)]),
            size: 10.0,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
            width: FontWidth::NORMAL,
        }
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

/// The parley state text is shaped in, kept between calls for its buffers.
pub(crate) struct Shaper {
    context: LayoutContext<()>,
    pub(crate) layout: Layout<()>,
}

impl std::fmt::Debug for Shaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shaper").finish_non_exhaustive()
    }
}

/// One shaped line, in layout space: x from the line's left edge, y down
/// from the alphabetic baseline.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Line {
    pub(crate) width: f64,
    /// The alphabetic baseline's y in the layout.
    pub(crate) baseline: f32,
    /// The first available font's ascent and descent.
    ascent: f64,
    descent: f64,
}

impl Line {
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

impl Shaper {
    pub(crate) fn new() -> Self {
        Self {
            context: LayoutContext::new(),
            layout: Layout::new(),
        }
    }

    /// Shapes `text` as one unwrapped line, after the text preparation
    /// algorithm's whitespace step: every ASCII whitespace character becomes
    /// a space.
    pub(crate) fn shape(
        &mut self,
        fonts: &mut FontContext,
        text: &str,
        font: &Font,
        direction: Direction,
    ) -> Line {
        const WHITESPACE: [char; 4] = ['\t', '\n', '\x0C', '\r'];
        let text: Cow<'_, str> = if text.contains(WHITESPACE) {
            Cow::Owned(text.replace(WHITESPACE, " "))
        } else {
            Cow::Borrowed(text)
        };
        self.build(fonts, &text, font, direction);
        let (ascent, descent) = self.first_run_metrics().unwrap_or_else(|| {
            // No glyph run names the first available font: read its metrics
            // off a shaped letter, then shape the text again.
            self.build(fonts, "x", font, direction);
            let metrics = self.first_run_metrics();
            self.build(fonts, &text, font, direction);
            let size = f64::from(font.size);
            metrics.unwrap_or((size * 0.8, size * 0.2))
        });
        let baseline = self
            .layout
            .lines()
            .next()
            .map_or(0.0, |line| line.metrics().baseline);
        Line {
            width: f64::from(self.layout.full_width()),
            baseline,
            ascent,
            descent,
        }
    }

    fn build(&mut self, fonts: &mut FontContext, text: &str, font: &Font, direction: Direction) {
        let mut builder = self.context.ranged_builder(fonts, text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(FontFamily::List(Cow::Borrowed(
            &font.families,
        ))));
        builder.push_default(StyleProperty::FontSize(font.size));
        builder.push_default(StyleProperty::FontWeight(font.weight));
        builder.push_default(StyleProperty::FontStyle(font.style));
        builder.push_default(StyleProperty::FontWidth(font.width));
        builder.push_default(StyleProperty::WhiteSpaceCollapse(
            WhiteSpaceCollapse::Preserve,
        ));
        builder.set_base_direction(if direction == Direction::Rtl {
            BaseDirection::Rtl
        } else {
            BaseDirection::Ltr
        });
        builder.build_into(&mut self.layout, text);
        self.layout.break_all_lines(None);
    }

    fn first_run_metrics(&self) -> Option<(f64, f64)> {
        let line = self.layout.lines().next()?;
        line.items().find_map(|item| match item {
            PositionedLayoutItem::GlyphRun(run) => {
                let metrics = run.run().font_metrics();
                Some((f64::from(metrics.ascent), f64::from(metrics.descent)))
            }
            PositionedLayoutItem::InlineBox(_) => None,
        })
    }

    /// The union of the shaped glyphs' outline bounds, in layout space
    /// relative to the alphabetic baseline; `None` for no ink.
    pub(crate) fn ink_bounds(&self, line: &Line) -> Option<Rect> {
        let mut bounds: Option<Rect> = None;
        for laid in self.layout.lines() {
            for item in laid.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font_data = run.font();
                let Ok(font) = FontRef::from_index(font_data.data.data(), font_data.index) else {
                    continue;
                };
                let coords: Vec<NormalizedCoord> = run
                    .normalized_coords()
                    .iter()
                    .map(|coord| NormalizedCoord::from_bits(coord.to_bits()))
                    .collect();
                let outlines = font.outline_glyphs();
                for glyph in glyph_run.positioned_glyphs() {
                    let Some(outline) = outlines.get(GlyphId::new(glyph.id)) else {
                        continue;
                    };
                    let mut pen = BoundsPen::default();
                    let settings = DrawSettings::unhinted(
                        Size::new(run.font_size()),
                        LocationRef::new(&coords),
                    );
                    if outline.draw(settings, &mut pen).is_err() {
                        continue;
                    }
                    let Some(glyph_bounds) = pen.bounds else {
                        continue;
                    };
                    // Outlines are y-up; the layout is y-down.
                    let placed = Rect::new(
                        f64::from(glyph.x) + glyph_bounds.x0,
                        f64::from(glyph.y - line.baseline) - glyph_bounds.y1,
                        f64::from(glyph.x) + glyph_bounds.x1,
                        f64::from(glyph.y - line.baseline) - glyph_bounds.y0,
                    );
                    bounds = Some(bounds.map_or(placed, |b| b.union(placed)));
                }
            }
        }
        bounds
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
        use terminus::link::kurbo::{Line as Segment, ParamCurveExtrema};
        let end = (f64::from(x), f64::from(y));
        self.add(Segment::new(self.current, end).bounding_box());
        self.current = end;
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        use terminus::link::kurbo::{ParamCurveExtrema, QuadBez};
        let end = (f64::from(x), f64::from(y));
        let quad = QuadBez::new(self.current, (f64::from(cx0), f64::from(cy0)), end);
        self.add(quad.bounding_box());
        self.current = end;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        use terminus::link::kurbo::{CubicBez, ParamCurveExtrema};
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
