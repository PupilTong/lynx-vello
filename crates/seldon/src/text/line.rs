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
    WhiteSpaceCollapse,
};

use super::TextContext;
use super::paragraph::Direction;
use crate::style::TextBrush;

/// A font as a document resolves it before shaping: the CSS `font-*`
/// properties with every keyword already turned into its number.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedFont {
    /// The `font-family` list, in preference order. An empty list shapes
    /// with the generic `sans-serif`.
    pub families: Vec<ResolvedFamily>,
    /// The font size in CSS px.
    pub size: f32,
    /// The `font-weight` number, 1 to 1000.
    pub weight: f32,
    pub style: ResolvedFontStyle,
    /// The `font-stretch` as a ratio: 1 is `normal`, 0.5 `ultra-condensed`,
    /// 2 `ultra-expanded`.
    pub stretch: f32,
}

/// One entry of a `font-family` list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedFamily {
    /// A family name, matched as written, whatever it spells: a family
    /// named `serif` is not the generic `serif`.
    Named(String),
    Generic(GenericFamily),
}

impl ResolvedFamily {
    /// One `font-family` entry in CSS syntax: an unquoted generic keyword
    /// is the generic family, a quoted name is the name without its quotes,
    /// and text that does not parse as one entry is a name as written. A
    /// list parses to its first entry.
    #[must_use]
    pub fn parse(css: &str) -> Self {
        match FontFamilyName::parse(css) {
            Some(FontFamilyName::Generic(generic)) => Self::Generic(generic),
            Some(FontFamilyName::Named(name)) => Self::Named(name.into_owned()),
            None => Self::Named(css.to_owned()),
        }
    }
}

/// The `font-style` value.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ResolvedFontStyle {
    #[default]
    Normal,
    Italic,
    /// `oblique`, with its angle in degrees when the value names one; with
    /// none, parley's default angle, 14 degrees.
    Oblique(Option<f32>),
}

/// How [`shape_line`] lays its text out, beside the font.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LineOptions {
    /// Px added after every character.
    pub letter_spacing: f32,
    /// The bidi paragraph level: `None` takes the direction of the text's
    /// first strong character, as parley does by default.
    pub direction: Option<Direction>,
    pub white_space: LineWhiteSpace,
}

/// Which parley `white-space-collapse` style [`shape_line`] shapes under.
///
/// Either way the text is shaped as given, every space and tab included:
/// parley's one-style builder collapses nothing, so a caller that collapses
/// white space does it before shaping (as SVG's parse does). What differs is
/// white space at the line's end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineWhiteSpace {
    /// White space at the line's end hangs past the line's end edge
    /// (css-text-4 §4.3.2), so on a right-to-left line it lies left of
    /// x = 0.
    #[default]
    Collapse,
    /// White space at the line's end stays inside the line, as css-text-4
    /// keeps preserved white space before a forced break, so a line's glyphs
    /// start at x = 0 in either direction. A newline still ends a line, so a
    /// caller that wants one line replaces newlines first.
    Preserve,
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
    /// The ascent and descent of the font that shaped the first glyph run,
    /// in visual order; `None` when no glyph run was shaped, as for empty
    /// text. [`font_metrics`] answers for a font without text.
    pub first_font: Option<FontMetrics>,
}

/// A font's ascent and descent at the size it is shaped at, in px, both
/// positive away from the baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
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
    /// What the font lacks of the requested weight and style, which a draw
    /// adds to the outlines.
    pub synthesis: Synthesis,
    pub glyphs: Vec<ShapedGlyph>,
}

/// The synthetic bold and oblique a run's font needs to match the requested
/// weight and style, as parley's font selection reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Synthesis {
    /// The requested weight is more than 200 above the font's own and the
    /// font has no weight axis: the outlines are to be widened.
    pub embolden: bool,
    /// The angle in degrees the outlines are to be slanted by, top to the
    /// right, for an italic or oblique request the font cannot meet with a
    /// face or an axis of its own.
    pub skew: Option<f32>,
}

/// One positioned glyph: its id in the run's font and its offset from the
/// line's start on the baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// Shapes `text` as one line under `font`, laid out as `options` says,
/// through the fonts `context` knows.
///
/// Empty text shapes to an empty line without touching parley.
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
    // Shaped in the context's scratch layout, whose buffers the next shaping
    // reuses: the line keeps its own glyph copies, not the layout.
    let locale = context.locale();
    let families = family_list(&font.families);
    let attributes = Attributes {
        width: FontWidth::from_ratio(font.stretch),
        weight: ParleyFontWeight::new(font.weight),
        style: match font.style {
            ResolvedFontStyle::Normal => ParleyFontStyle::Normal,
            ResolvedFontStyle::Italic => ParleyFontStyle::Italic,
            ResolvedFontStyle::Oblique(angle) => ParleyFontStyle::Oblique(angle),
        },
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
    builder.push_default(StyleProperty::WhiteSpaceCollapse(
        match options.white_space {
            LineWhiteSpace::Collapse => WhiteSpaceCollapse::Collapse,
            LineWhiteSpace::Preserve => WhiteSpaceCollapse::Preserve,
        },
    ));
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

/// The ascent and descent, at `font`'s size, of the font parley sets the
/// letter `x` in under `font`: the first family of the list that has the
/// letter. What a caller that needs the first available font's metrics
/// reads when its own text shaped no glyph run. `None` when no font in
/// `context` sets the letter.
#[must_use]
pub fn font_metrics(context: &mut TextContext, font: &ResolvedFont) -> Option<FontMetrics> {
    shape_line(context, "x", font, LineOptions::default()).first_font
}

/// The parley family list for `families`; an empty list is the default
/// generic family.
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

    /// Ahem's glyphs are one em squares, so three characters at 10 px are
    /// three glyphs 10 px apart on a 30 px line, 8 px above the baseline
    /// and 2 px below.
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

    /// A right-to-left line lays its runs out from the right: under an
    /// explicit `rtl` level the leading space of `" a"` ends the line on
    /// the left, where `ltr` puts it first. With no direction the text's
    /// first strong character decides, and Latin is left to right.
    #[test]
    fn the_base_direction_orders_the_line() {
        let mut context = ahem_context();
        let font = ahem(10.0);
        let preserved = |direction| LineOptions {
            direction,
            white_space: LineWhiteSpace::Preserve,
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

    /// Both styles shape every space; on a right-to-left line the trailing
    /// spaces of `"a  "` hang left of x = 0 under `Collapse` and stay inside
    /// the line under `Preserve`.
    #[test]
    fn white_space_is_shaped_as_given_and_hangs_or_stays() {
        let mut context = ahem_context();
        let font = ahem(10.0);
        let shape = |context: &mut TextContext, text, direction, white_space| {
            let options = LineOptions {
                direction,
                white_space,
                ..LineOptions::default()
            };
            shape_line(context, text, &font, options)
        };
        for white_space in [LineWhiteSpace::Collapse, LineWhiteSpace::Preserve] {
            let line = shape(&mut context, "a  b", None, white_space);
            assert_eq!(line.advance, 40.0, "{white_space:?}");
            assert_eq!(xs(&line), [0.0, 10.0, 20.0, 30.0], "{white_space:?}");
        }
        let rtl = Some(Direction::Rtl);
        let hanging = shape(&mut context, "a  ", rtl, LineWhiteSpace::Collapse);
        let kept = shape(&mut context, "a  ", rtl, LineWhiteSpace::Preserve);
        assert_eq!(xs(&hanging), [-20.0, -10.0, 0.0]);
        assert_eq!(xs(&kept), [0.0, 10.0, 20.0]);
        assert_eq!((hanging.advance, kept.advance), (30.0, 30.0));
    }

    /// The first run's font reports Ahem's ascent of 0.8 em and descent of
    /// 0.2 em, and [`font_metrics`] answers the same with no text.
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

    /// Ahem has one regular face and no axes, so a bold request is
    /// emboldened and an italic or oblique one slanted, by the angle an
    /// oblique names and by parley's 14 degrees otherwise.
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

    /// Generic keywords stay generic, a quoted keyword is a name, and
    /// anything else is a named family; no family resolves to the default
    /// generic.
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
