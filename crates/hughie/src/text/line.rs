//! One line of text shaped with one style: the shaper behind SVG `<text>`.
//!
//! The block path (`block/`) lays a whole Lynx paragraph out; this is the
//! other thing a host needs from parley, a single run of characters under
//! one resolved font, with no breaking, no alignment and no paragraph
//! state. [`shape_line`] builds a one-style [`parley::RangedBuilder`] over
//! the shared [`TextContext`], breaks it as one line, and reads the
//! positioned glyphs back per font run. The output names each run's font
//! handle, size and variation coordinates, which is exactly what a vello
//! `draw_glyphs` call takes, and the glyph positions are relative to the
//! line's start on its alphabetic baseline, so a caller places the line by
//! one translation.
//!
//! Lives in this crate because the context's parley handles are
//! `pub(super)`: a one-line shaper is the one other reader they have.

use std::borrow::Cow;

use parley::{
    FontFamily, FontFamilyName, FontStyle as ParleyFontStyle, FontWeight as ParleyFontWeight,
    FontWidth, PositionedLayoutItem, StyleProperty,
};

use super::TextContext;
use crate::style::TextBrush;

/// A font as a document resolves it before shaping: the CSS `font-*`
/// properties with every keyword already turned into its number.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedFont {
    /// The `font-family` list, in preference order. A name parley does not
    /// know as a generic family (`serif`, `monospace`, …) is a named family.
    /// An empty list shapes with the context's default family.
    pub families: Vec<String>,
    /// The font size in CSS px.
    pub size: f32,
    /// The `font-weight` number, 1 to 1000.
    pub weight: u16,
    pub style: ResolvedFontStyle,
    /// The `font-stretch` as a ratio: 1 is `normal`, 0.5 `ultra-condensed`,
    /// 2 `ultra-expanded`.
    pub stretch: f32,
}

/// The `font-style` keyword.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ResolvedFontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

/// One shaped line: its glyph runs and the metrics a caller places and
/// anchors it by.
#[derive(Clone, Debug, Default)]
pub struct ShapedLine {
    pub runs: Vec<ShapedRun>,
    /// The line's whole advance along the baseline, trailing whitespace
    /// included.
    pub advance: f32,
    /// How far the line's ink reaches above the baseline, as the fonts
    /// report it.
    pub ascent: f32,
    /// How far it reaches below.
    pub descent: f32,
}

/// A maximal sequence of glyphs from one font at one size.
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub font: parley::FontData,
    /// The size the glyphs are scaled to, in px per em.
    pub size: f32,
    /// The variation axis coordinates the font is instanced at.
    pub normalized_coords: Vec<i16>,
    pub glyphs: Vec<ShapedGlyph>,
}

/// One positioned glyph: its id in the run's font and its offset from the
/// line's start on the baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// Shapes `text` as one line under `font` with `letter_spacing` px between
/// characters, through the fonts `context` knows.
///
/// Empty text shapes to an empty line without touching parley (which would
/// shape a substitute space for an empty build).
#[must_use]
pub fn shape_line(
    context: &mut TextContext,
    text: &str,
    font: &ResolvedFont,
    letter_spacing: f32,
) -> ShapedLine {
    if text.is_empty() {
        return ShapedLine::default();
    }
    let (font_context, layout_context) = context.font_and_layout_contexts();
    let mut builder = layout_context.ranged_builder(font_context, text, 1.0, false);
    builder.push_default(StyleProperty::<TextBrush>::FontFamily(family_list(
        &font.families,
    )));
    builder.push_default(StyleProperty::FontSize(font.size));
    builder.push_default(StyleProperty::FontWeight(ParleyFontWeight::new(f32::from(
        font.weight,
    ))));
    builder.push_default(StyleProperty::FontStyle(match font.style {
        ResolvedFontStyle::Normal => ParleyFontStyle::Normal,
        ResolvedFontStyle::Italic => ParleyFontStyle::Italic,
        ResolvedFontStyle::Oblique => ParleyFontStyle::Oblique(None),
    }));
    builder.push_default(StyleProperty::FontWidth(FontWidth::from_ratio(
        font.stretch,
    )));
    builder.push_default(StyleProperty::LetterSpacing(letter_spacing));
    let mut layout = builder.build(text);
    layout.break_all_lines(None);

    let mut line = ShapedLine {
        runs: Vec::new(),
        advance: layout.full_width(),
        ascent: 0.0,
        descent: 0.0,
    };
    for laid in layout.lines() {
        let metrics = laid.metrics();
        line.ascent = line.ascent.max(metrics.ascent);
        line.descent = line.descent.max(metrics.descent);
        for item in laid.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let run = glyph_run.run();
            // `positioned_glyphs` would add the line's baseline to every
            // `y`; the caller's baseline is where it places the line.
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
            line.runs.push(ShapedRun {
                font: run.font().clone(),
                size: run.font_size(),
                normalized_coords: run.normalized_coords().to_vec(),
                glyphs,
            });
        }
    }
    line
}

/// The parley family list for `families`: each name parsed as CSS would
/// parse it, so a generic keyword is the generic family and anything else
/// a named one; an empty list is the default generic family.
fn family_list(families: &[String]) -> FontFamily<'_> {
    let mut names: Vec<FontFamilyName<'_>> = families
        .iter()
        .map(|name| {
            FontFamilyName::parse(name)
                .unwrap_or(FontFamilyName::Named(Cow::Borrowed(name.as_str())))
        })
        .collect();
    match names.len() {
        0 => FontFamily::Single(FontFamilyName::Generic(parley::GenericFamily::SansSerif)),
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
            families: vec!["Ahem".to_owned()],
            size,
            weight: 400,
            style: ResolvedFontStyle::Normal,
            stretch: 1.0,
        }
    }

    /// Ahem's glyphs are one em squares, so three characters at 10 px are
    /// three glyphs 10 px apart on a 30 px line, 8 px above the baseline
    /// and 2 px below.
    #[test]
    fn a_line_of_ahem_advances_one_em_per_glyph() {
        let mut context = ahem_context();
        let line = shape_line(&mut context, "abc", &ahem(10.0), 0.0);
        assert_eq!(line.runs.len(), 1, "one font, one run");
        let run = &line.runs[0];
        assert!((run.size - 10.0).abs() < f32::EPSILON);
        assert!(run.normalized_coords.is_empty(), "Ahem has no axes");
        let xs: Vec<f32> = run.glyphs.iter().map(|glyph| glyph.x).collect();
        assert_eq!(xs, [0.0, 10.0, 20.0]);
        assert!(run.glyphs.iter().all(|glyph| glyph.y == 0.0));
        assert!((line.advance - 30.0).abs() < 1e-3, "{}", line.advance);
        assert!((line.ascent - 8.0).abs() < 1e-3, "{}", line.ascent);
        assert!((line.descent - 2.0).abs() < 1e-3, "{}", line.descent);
    }

    #[test]
    fn letter_spacing_widens_the_line() {
        let mut context = ahem_context();
        let line = shape_line(&mut context, "abc", &ahem(10.0), 2.0);
        assert!((line.advance - 36.0).abs() < 1e-3, "{}", line.advance);
        let xs: Vec<f32> = line.runs[0].glyphs.iter().map(|glyph| glyph.x).collect();
        assert_eq!(xs, [0.0, 12.0, 24.0]);
    }

    #[test]
    fn empty_text_is_an_empty_line() {
        let mut context = ahem_context();
        let line = shape_line(&mut context, "", &ahem(10.0), 0.0);
        assert!(line.runs.is_empty());
        assert_eq!(line.advance, 0.0);
    }

    /// Generic keywords stay generic and anything else is a named family,
    /// with no family resolving to the default generic.
    #[test]
    fn family_names_parse_as_css_does() {
        assert!(matches!(
            family_list(&[]),
            FontFamily::Single(FontFamilyName::Generic(parley::GenericFamily::SansSerif))
        ));
        assert!(matches!(
            family_list(&["monospace".to_owned()]),
            FontFamily::Single(FontFamilyName::Generic(parley::GenericFamily::Monospace))
        ));
        let families = ["Ahem".to_owned(), "serif".to_owned()];
        let list = family_list(&families);
        let FontFamily::List(names) = list else {
            panic!("two families are a list");
        };
        assert!(matches!(&names[0], FontFamilyName::Named(name) if name == "Ahem"));
        assert!(matches!(
            names[1],
            FontFamilyName::Generic(parley::GenericFamily::Serif)
        ));
    }
}
