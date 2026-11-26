//! One line of text shaped with one style: the shaper behind SVG `<text>`
//! and the Canvas 2D context's `fillText`, `strokeText` and `measureText`.
//!
//! The block path (`block/`) lays a whole Lynx paragraph out; this is the
//! other thing a host needs from parley, a single run of characters under
//! one resolved font, with no breaking, no alignment and no paragraph
//! state. [`shape_line`] builds a one-style [`parley::RangedBuilder`] over
//! the shared [`TextContext`], breaks it as one line, and reads the
//! positioned glyphs back per font run. The output names each run's font
//! handle, size, variation coordinates and synthesis, which is exactly what
//! a glyph run draw takes, and the glyph positions are relative to the
//! line's start on its alphabetic baseline, so a caller places the line by
//! one translation.
//!
//! Lives in this crate because the context's parley handles are
//! `pub(super)`: a one-line shaper is the one other reader they have.

use std::borrow::Cow;

use parley::fontique::Attributes;
use parley::{
    BaseDirection, FontFamily, FontFamilyName, FontStyle as ParleyFontStyle,
    FontWeight as ParleyFontWeight, FontWidth, GenericFamily, PositionedLayoutItem, StyleProperty,
};
use stylo::values::computed::font::{FontStyle, GenericFontFamily, SingleFontFamily};

use super::TextContext;
use super::paragraph::Direction;
use crate::style::TextBrush;

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedFont {
    pub families: Vec<ResolvedFamily>,
    pub size: f32,
    pub weight: f32,
    pub style: ResolvedFontStyle,
    pub stretch: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedFamily {
    Named(String),
    Generic(GenericFamily),
}

impl ResolvedFamily {
    #[must_use]
    pub fn parse(css: &str) -> Self {
        match FontFamilyName::parse(css) {
            Some(FontFamilyName::Generic(generic)) => Self::Generic(generic),
            Some(FontFamilyName::Named(name)) => Self::Named(name.into_owned()),
            None => Self::Named(css.to_owned()),
        }
    }
}

impl From<&SingleFontFamily> for ResolvedFamily {
    fn from(single: &SingleFontFamily) -> Self {
        match single {
            SingleFontFamily::FamilyName(name) => Self::Named(name.name.to_string()),
            SingleFontFamily::Generic(generic) => Self::Generic(generic_family(*generic)),
        }
    }
}

pub(in crate::text) const fn generic_family(value: GenericFontFamily) -> GenericFamily {
    match value {
        GenericFontFamily::None | GenericFontFamily::SansSerif => GenericFamily::SansSerif,
        GenericFontFamily::Serif => GenericFamily::Serif,
        GenericFontFamily::Monospace => GenericFamily::Monospace,
        GenericFontFamily::Cursive => GenericFamily::Cursive,
        GenericFontFamily::Fantasy => GenericFamily::Fantasy,
        GenericFontFamily::SystemUi => GenericFamily::SystemUi,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ResolvedFontStyle {
    #[default]
    Normal,
    Italic,
    Oblique(Option<f32>),
}

impl From<FontStyle> for ResolvedFontStyle {
    fn from(style: FontStyle) -> Self {
        if style == FontStyle::NORMAL {
            Self::Normal
        } else if style == FontStyle::ITALIC {
            Self::Italic
        } else {
            Self::Oblique(Some(style.oblique_degrees()))
        }
    }
}

impl From<ResolvedFontStyle> for ParleyFontStyle {
    fn from(style: ResolvedFontStyle) -> Self {
        match style {
            ResolvedFontStyle::Normal => Self::Normal,
            ResolvedFontStyle::Italic => Self::Italic,
            ResolvedFontStyle::Oblique(angle) => Self::Oblique(angle),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LineOptions {
    pub letter_spacing: f32,
    pub direction: Option<Direction>,
}

#[derive(Clone, Debug, Default)]
pub struct ShapedLine {
    pub runs: Vec<ShapedRun>,
    pub advance: f32,
    pub ascent: f32,
    pub descent: f32,
    pub first_font: Option<FontMetrics>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
}

#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub font: parley::FontData,
    pub size: f32,
    pub normalized_coords: Vec<i16>,
    pub synthesis: Synthesis,
    pub glyphs: Vec<ShapedGlyph>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Synthesis {
    pub embolden: bool,
    pub skew: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

#[must_use]
pub fn shape_line(
    context: &mut TextContext,
    text: &str,
    font: &ResolvedFont,
    options: LineOptions,
) -> ShapedLine {
    if text.is_empty() {
        return ShapedLine::default();
    }
    let locale = context.locale();
    let families = family_list(&font.families);
    let attributes = Attributes {
        width: FontWidth::from_ratio(font.stretch),
        weight: ParleyFontWeight::new(font.weight),
        style: font.style.into(),
    };
    context.cover(text, [(0..text.len(), &families, attributes)]);
    let (font_context, layout_context, layout) = context.shaping_parts();
    let mut builder = layout_context.ranged_builder(font_context, text, 1.0, false);
    builder.push_default(StyleProperty::<TextBrush>::Locale(locale));
    builder.push_default(StyleProperty::<TextBrush>::FontFamily(families.clone()));
    builder.push_default(StyleProperty::FontSize(font.size));
    builder.push_default(StyleProperty::FontWeight(attributes.weight));
    builder.push_default(StyleProperty::FontStyle(attributes.style));
    builder.push_default(StyleProperty::FontWidth(attributes.width));
    builder.push_default(StyleProperty::LetterSpacing(options.letter_spacing));
    builder.set_base_direction(match options.direction {
        None => BaseDirection::Auto,
        Some(Direction::Ltr) => BaseDirection::Ltr,
        Some(Direction::Rtl) => BaseDirection::Rtl,
    });
    builder.build_into(layout, text);
    layout.break_all_lines(None);

    let mut line = ShapedLine {
        runs: Vec::new(),
        advance: layout.full_width(),
        ascent: 0.0,
        descent: 0.0,
        first_font: None,
    };
    for laid in layout.lines() {
        let metrics = laid.metrics();
        line.ascent = line
            .ascent
            .max(metrics.baseline - metrics.content_block_min_coord);
        line.descent = line
            .descent
            .max(metrics.content_block_max_coord - metrics.baseline);
        for item in laid.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let run = glyph_run.run();
            if line.first_font.is_none() {
                let metrics = run.font_metrics();
                line.first_font = Some(FontMetrics {
                    ascent: metrics.ascent,
                    descent: metrics.descent,
                });
            }
            let mut x = glyph_run.offset();
            let glyphs = glyph_run
                .glyphs()
                .map(|glyph| {
                    let placed = ShapedGlyph {
                        id: glyph.id,
                        x: x + glyph.x,
                        y: glyph.y,
                    };
                    x += glyph.advance;
                    placed
                })
                .collect::<Vec<_>>();
            if glyphs.is_empty() {
                continue;
            }
            let synthesis = run.synthesis();
            line.runs.push(ShapedRun {
                font: run.font().clone(),
                size: run.font_size(),
                normalized_coords: run
                    .normalized_coords()
                    .iter()
                    .map(|coord| coord.to_bits())
                    .collect(),
                synthesis: Synthesis {
                    embolden: synthesis.embolden(),
                    skew: synthesis.skew(),
                },
                glyphs,
            });
        }
    }
    context.trim_scratch(text.len());
    line
}

#[must_use]
pub fn font_metrics(context: &mut TextContext, font: &ResolvedFont) -> Option<FontMetrics> {
    shape_line(context, "x", font, LineOptions::default()).first_font
}

fn family_list(families: &[ResolvedFamily]) -> FontFamily<'_> {
    let mut names: Vec<FontFamilyName<'_>> = families
        .iter()
        .map(|family| match family {
            ResolvedFamily::Named(name) => FontFamilyName::Named(Cow::Borrowed(name.as_str())),
            ResolvedFamily::Generic(generic) => FontFamilyName::Generic(*generic),
        })
        .collect();
    match names.len() {
        0 => FontFamily::Single(FontFamilyName::Generic(GenericFamily::SansSerif)),
        1 => FontFamily::Single(names.remove(0)),
        _ => FontFamily::List(Cow::Owned(names)),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;
    use crate::text::FontBlob;

    const AHEM: &[u8] = include_bytes!("../../tests/fixtures/Ahem.ttf");

    fn ahem_context() -> TextContext {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        context
    }

    fn ahem(size: f32) -> ResolvedFont {
        ResolvedFont {
            families: vec![ResolvedFamily::Named("Ahem".to_owned())],
            size,
            weight: 400.0,
            style: ResolvedFontStyle::Normal,
            stretch: 1.0,
        }
    }

    fn xs(line: &ShapedLine) -> Vec<f32> {
        line.runs
            .iter()
            .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.x))
            .collect()
    }

    #[test]
    fn a_line_of_ahem_advances_one_em_per_glyph() {
        let mut context = ahem_context();
        let line = shape_line(&mut context, "abc", &ahem(10.0), LineOptions::default());
        assert_eq!(line.runs.len(), 1, "one font, one run");
        let run = &line.runs[0];
        assert!((run.size - 10.0).abs() < f32::EPSILON);
        assert!(run.normalized_coords.is_empty(), "Ahem has no axes");
        assert_eq!(run.synthesis, Synthesis::default(), "Ahem is regular");
        assert_eq!(xs(&line), [0.0, 10.0, 20.0]);
        assert!(run.glyphs.iter().all(|glyph| glyph.y == 0.0));
        assert!((line.advance - 30.0).abs() < 1e-3, "{}", line.advance);
        assert!((line.ascent - 8.0).abs() < 1e-3, "{}", line.ascent);
        assert!((line.descent - 2.0).abs() < 1e-3, "{}", line.descent);
    }

    #[test]
    fn letter_spacing_widens_the_line() {
        let mut context = ahem_context();
        let options = LineOptions {
            letter_spacing: 2.0,
            ..LineOptions::default()
        };
        let line = shape_line(&mut context, "abc", &ahem(10.0), options);
        assert!((line.advance - 36.0).abs() < 1e-3, "{}", line.advance);
        assert_eq!(xs(&line), [0.0, 12.0, 24.0]);
    }

    #[test]
    fn empty_text_is_an_empty_line() {
        let mut context = ahem_context();
        let line = shape_line(&mut context, "", &ahem(10.0), LineOptions::default());
        assert!(line.runs.is_empty());
        assert_eq!(line.advance, 0.0);
        assert_eq!(line.first_font, None);
    }

    #[test]
    fn the_base_direction_orders_the_line() {
        let mut context = ahem_context();
        let font = ahem(10.0);
        let preserved = |direction| LineOptions {
            direction,
            ..LineOptions::default()
        };
        let ltr = shape_line(&mut context, "a ", &font, preserved(Some(Direction::Ltr)));
        let rtl = shape_line(&mut context, "a ", &font, preserved(Some(Direction::Rtl)));
        let auto = shape_line(&mut context, "a ", &font, preserved(None));
        assert_eq!(xs(&ltr), [0.0, 10.0], "`a` then the space");
        assert_eq!(xs(&rtl), [0.0, 10.0]);
        assert_eq!(xs(&auto), xs(&ltr));
        let a = |line: &ShapedLine| {
            line.runs
                .iter()
                .flat_map(|run| &run.glyphs)
                .find(|glyph| glyph.id == ltr.runs[0].glyphs[0].id)
                .map(|glyph| glyph.x)
        };
        assert_eq!(a(&ltr), Some(0.0), "`a` starts an ltr line");
        assert_eq!(a(&rtl), Some(10.0), "the space precedes `a` on the left");
        assert_eq!(rtl.advance, 20.0);
    }

    #[test]
    fn white_space_is_shaped_as_given_and_stays_inside_the_line() {
        let mut context = ahem_context();
        let font = ahem(10.0);
        let shape = |context: &mut TextContext, text, direction| {
            let options = LineOptions {
                direction,
                ..LineOptions::default()
            };
            shape_line(context, text, &font, options)
        };
        let line = shape(&mut context, "a  b", None);
        assert_eq!(line.advance, 40.0);
        assert_eq!(xs(&line), [0.0, 10.0, 20.0, 30.0]);
        for direction in [Direction::Ltr, Direction::Rtl] {
            let line = shape(&mut context, "a  ", Some(direction));
            assert_eq!(xs(&line), [0.0, 10.0, 20.0], "{direction:?}");
            assert_eq!(line.advance, 30.0, "{direction:?}");
        }
    }

    #[test]
    fn the_first_fonts_metrics_are_reported_with_and_without_text() {
        let mut context = ahem_context();
        let font = ahem(10.0);
        let expected = Some(FontMetrics {
            ascent: 8.0,
            descent: 2.0,
        });
        let line = shape_line(&mut context, "abc", &font, LineOptions::default());
        assert_eq!(line.first_font, expected);
        assert_eq!(font_metrics(&mut context, &font), expected);
        assert_eq!(
            shape_line(&mut context, "", &font, LineOptions::default()).first_font,
            None,
            "empty text shapes nothing"
        );
        assert_eq!(
            font_metrics(&mut TextContext::without_system_fonts(), &font),
            None,
            "a context with no font answers none"
        );
    }

    #[test]
    fn synthesis_and_the_oblique_angle_are_carried() {
        let mut context = ahem_context();
        let synthesis = |context: &mut TextContext, weight: f32, style| {
            let font = ResolvedFont {
                weight,
                style,
                ..ahem(10.0)
            };
            shape_line(context, "a", &font, LineOptions::default()).runs[0].synthesis
        };
        assert_eq!(
            synthesis(&mut context, 700.0, ResolvedFontStyle::Normal),
            Synthesis {
                embolden: true,
                skew: None,
            }
        );
        assert_eq!(
            synthesis(&mut context, 400.0, ResolvedFontStyle::Italic).skew,
            Some(14.0)
        );
        assert_eq!(
            synthesis(&mut context, 400.0, ResolvedFontStyle::Oblique(None)).skew,
            Some(14.0)
        );
        assert_eq!(
            synthesis(&mut context, 400.0, ResolvedFontStyle::Oblique(Some(20.0))).skew,
            Some(20.0)
        );
    }

    #[test]
    fn family_names_parse_as_css_does() {
        assert_eq!(
            ResolvedFamily::parse("monospace"),
            ResolvedFamily::Generic(GenericFamily::Monospace)
        );
        assert_eq!(
            ResolvedFamily::parse("'monospace'"),
            ResolvedFamily::Named("monospace".to_owned())
        );
        assert_eq!(
            ResolvedFamily::parse("Ahem"),
            ResolvedFamily::Named("Ahem".to_owned())
        );
        assert!(matches!(
            family_list(&[]),
            FontFamily::Single(FontFamilyName::Generic(GenericFamily::SansSerif))
        ));
        let families = [
            ResolvedFamily::Named("serif".to_owned()),
            ResolvedFamily::Generic(GenericFamily::Serif),
        ];
        let FontFamily::List(names) = family_list(&families) else {
            panic!("two families are a list");
        };
        assert!(matches!(&names[0], FontFamilyName::Named(name) if name == "serif"));
        assert!(matches!(
            names[1],
            FontFamilyName::Generic(GenericFamily::Serif)
        ));
    }
}
