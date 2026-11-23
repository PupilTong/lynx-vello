//! The CSS strings a 2D context's attributes take, parsed by stylo: a
//! `<color>` (`fillStyle`, `strokeStyle`, `shadowColor`, `addColorStop`)
//! and the `font` shorthand, with the serializations their getters answer.
//!
//! Both parse with an author-origin context over `about:blank`
//! ([`author_parser_context`]) and compute with no `Device` and no element,
//! so they read nothing of a document or a device and run on any thread: a
//! relative font size resolves against the default canvas font's 10px, and
//! what needs a viewport, a container or a system setting is refused.

use std::sync::LazyLock;

use cssparser::Parser;
use seldon::text::{ResolvedFamily, ResolvedFont, ResolvedFontStyle};
use stylo::color::AbsoluteColor;
use stylo::context::QuirksMode;
use stylo::properties::shorthands::font;
use stylo::stylesheets::{CssRuleType, UrlExtraData};
use stylo::values::computed::Length;
use stylo::values::computed::font::FontWeight as ComputedFontWeight;
use stylo::values::specified::font::{
    FontFamily, FontSize, FontStyle as SpecifiedFontStyle, FontWeight as SpecifiedFontWeight,
    FontWidth as SpecifiedFontWidth, LineHeight,
};
use stylo::values::specified::length::{LengthPercentage, NoCalcLength};
use stylo::values::specified::{self, Color as SpecifiedColor};
use stylo_traits::{CssWriter, ToCss};

use crate::paint::convert;
use crate::peniko::Color;
use crate::style::engine::author_parser_context;
use crate::tree::document::about_blank_url_data;

/// The base a relative font size resolves against: the default canvas
/// font's 10px, HTML's base for an `OffscreenCanvas`, whose font style
/// source object is not an element.
const BASE_FONT_SIZE: f32 = 10.0;
/// `medium`, the size the absolute-size keywords are scaled from.
const MEDIUM_FONT_SIZE: f32 = 16.0;
/// CSS Fonts' ratio between adjacent sizes, for `larger` and `smaller`.
const RELATIVE_SIZE_RATIO: f32 = 1.2;

/// The base URL every canvas string parses against, built once.
static URL_DATA: LazyLock<UrlExtraData> = LazyLock::new(about_blank_url_data);

/// A CSS `<color>` as sRGB, each channel clamped to 0..=1 (NaN as 0),
/// `currentcolor` as opaque black; `None` when `text` is not one, or names
/// a system color, which needs a device.
///
/// The parser is the fork's `lynx` grammar, which leaves out `currentcolor`
/// (and `color-mix()`, `light-dark()`, `contrast-color()`): the keyword is
/// matched here, first.
pub(super) fn parse_color(text: &str) -> Option<Color> {
    let current =
        Parser::new(text).parse_entirely(|parser| -> Result<(), cssparser::ParseError<()>> {
            Ok(parser.expect_ident_matching("currentcolor")?)
        });
    if current.is_ok() {
        return Some(Color::BLACK);
    }
    let context = author_parser_context(&URL_DATA, CssRuleType::Style);
    let mut parser = Parser::new(text);
    let computed = SpecifiedColor::parse_and_compute(&context, &mut parser, None).ok()?;
    let srgb = convert::color(computed.resolve_to_absolute(&AbsoluteColor::BLACK));
    let channel = |value: f32| {
        if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, 1.0)
        }
    };
    Some(Color::new(srgb.components.map(channel)))
}

/// HTML's serialization of a canvas color: `#rrggbb` when it is opaque,
/// `rgba(r, g, b, a)` otherwise, `a` in the fewest decimals that keep its
/// 8-bit value.
pub(super) fn serialize_color(color: Color) -> String {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    if a == 255 {
        return format!("#{r:02x}{g:02x}{b:02x}");
    }
    let alpha = f64::from(a) / 255.0;
    let two = (alpha * 100.0).round() / 100.0;
    #[expect(clippy::float_cmp, reason = "both are whole numbers")]
    let shortest = if (two * 255.0).round() == f64::from(a) {
        two
    } else {
        (alpha * 1000.0).round() / 1000.0
    };
    format!("rgba({r}, {g}, {b}, {shortest})")
}

/// A `font` value: what the canvas draws with, and the serialization the
/// `font` getter answers.
#[derive(Debug)]
pub(super) struct ParsedFont {
    pub(super) font: ResolvedFont,
    pub(super) css: String,
}

/// The CSS `font` shorthand, as HTML's `font` setter takes it: `None` for
/// text that does not parse as the shorthand — a CSS-wide keyword included —
/// and for a size this engine cannot resolve without a document (viewport
/// and container units, `rpx`, `calc()` other than of absolute lengths).
/// `line-height` is parsed and dropped. The serialization is the shorthand's
/// own, with the size in px and no line height.
pub(super) fn parse_font(text: &str) -> Option<ParsedFont> {
    let context = author_parser_context(&URL_DATA, CssRuleType::Style);
    let mut parser = Parser::new(text);
    let longhands = parser
        .parse_entirely(|parser| font::parse_value(&context, parser))
        .ok()?;
    let size = font_size_px(&longhands.font_size)?;
    if !size.is_finite() || size < 0.0 {
        return None;
    }
    let families = families(&longhands.font_family)?;
    let font = ResolvedFont {
        families,
        size,
        weight: weight(&longhands.font_weight)?,
        style: style(&longhands.font_style)?,
        stretch: width(&longhands.font_width)?,
    };
    let size = FontSize::Length(LengthPercentage::Length(NoCalcLength::from_px(size)));
    let line_height = LineHeight::normal();
    let serialize = font::LonghandsToSerialize {
        font_style: &longhands.font_style,
        font_variant_alternates: &longhands.font_variant_alternates,
        font_variant_caps: &longhands.font_variant_caps,
        font_variant_east_asian: &longhands.font_variant_east_asian,
        font_variant_ligatures: &longhands.font_variant_ligatures,
        font_variant_numeric: &longhands.font_variant_numeric,
        font_variant_position: &longhands.font_variant_position,
        font_weight: &longhands.font_weight,
        font_width: &longhands.font_width,
        font_size: &size,
        line_height: &line_height,
        font_family: &longhands.font_family,
        font_optical_sizing: Some(&longhands.font_optical_sizing),
        font_variation_settings: Some(&longhands.font_variation_settings),
        font_kerning: &longhands.font_kerning,
        font_feature_settings: &longhands.font_feature_settings,
        font_language_override: &longhands.font_language_override,
    };
    let mut css = String::new();
    serialize.to_css(&mut CssWriter::new(&mut css)).ok()?;
    Some(ParsedFont { font, css })
}

/// A specified `font-size` in px, relative sizes against
/// [`BASE_FONT_SIZE`].
fn font_size_px(size: &FontSize) -> Option<f32> {
    match size {
        FontSize::Length(LengthPercentage::Length(length)) => length_px(*length),
        FontSize::Length(LengthPercentage::Percentage(percentage)) => {
            Some(BASE_FONT_SIZE * percentage.get())
        }
        FontSize::Length(LengthPercentage::Calc(calc)) => {
            calc.to_computed_pixel_length_without_context().ok()
        }
        FontSize::Keyword(info) => {
            let keyword = info
                .kw
                .to_length_without_context(QuirksMode::NoQuirks, Length::new(MEDIUM_FONT_SIZE));
            Some(keyword.0.px() * info.factor + info.offset.px())
        }
        FontSize::Larger => Some(BASE_FONT_SIZE * RELATIVE_SIZE_RATIO),
        FontSize::Smaller => Some(BASE_FONT_SIZE / RELATIVE_SIZE_RATIO),
        FontSize::System(_) => None,
    }
}

/// An absolute or font-relative length in px. The font-relative units take
/// stylo's ratios for a font with no metrics.
fn length_px(length: NoCalcLength) -> Option<f32> {
    if let Some(px) = length.to_px_if_absolute() {
        return Some(px);
    }
    let em = match length.unit() {
        "em" | "rem" | "cap" | "rcap" | "ic" | "ric" => 1.0,
        "ex" | "rex" | "ch" | "rch" => 0.5,
        "lh" | "rlh" => 1.2,
        _ => return None,
    };
    Some(length.unitless_value() * em * BASE_FONT_SIZE)
}

/// The family list; `None` for a system font.
fn families(family: &FontFamily) -> Option<Vec<ResolvedFamily>> {
    let FontFamily::Values(list) = family else {
        return None;
    };
    Some(list.iter().map(ResolvedFamily::from).collect())
}

/// The weight; `bolder` and `lighter` relative to the default font's
/// `normal`.
fn weight(weight: &SpecifiedFontWeight) -> Option<f32> {
    let value = match weight {
        SpecifiedFontWeight::Absolute(absolute) => absolute.compute()?,
        SpecifiedFontWeight::Bolder => ComputedFontWeight::normal().bolder(),
        SpecifiedFontWeight::Lighter => ComputedFontWeight::normal().lighter(),
        SpecifiedFontWeight::System(_) => return None,
    };
    Some(value.value())
}

fn style(style: &SpecifiedFontStyle) -> Option<ResolvedFontStyle> {
    match style {
        SpecifiedFontStyle::Specified(specified::font::SpecifiedFontStyle::Italic) => {
            Some(ResolvedFontStyle::Italic)
        }
        SpecifiedFontStyle::Specified(specified::font::SpecifiedFontStyle::Oblique(angle)) => {
            // A `calc()` angle needs a computed-value context.
            let degrees = angle.degrees()?;
            Some(if degrees == 0.0 {
                ResolvedFontStyle::Normal
            } else {
                ResolvedFontStyle::Oblique(Some(degrees))
            })
        }
        SpecifiedFontStyle::System(_) => None,
    }
}

/// `font-stretch`'s keyword as a width ratio (CSS Fonts 4, "font-width").
fn width(width: &SpecifiedFontWidth) -> Option<f32> {
    let SpecifiedFontWidth::Keyword(keyword) = width else {
        return None;
    };
    Some(keyword.compute().to_percentage().0)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use seldon::text::{GenericFamily, ResolvedFamily, ResolvedFontStyle};

    use super::{parse_color, parse_font, serialize_color};
    use crate::html::canvas::{CanvasError, Gradient, OffscreenCanvas, Style};
    use crate::peniko::Color;

    fn rgba8(color: Color) -> [u8; 4] {
        color.to_rgba8().to_u8_array()
    }

    fn fill_rgba8(canvas: &mut OffscreenCanvas) -> [u8; 4] {
        match canvas.context_2d().fill_style() {
            Style::Color(color) => rgba8(*color),
            other => panic!("a color fill style, not {other:?}"),
        }
    }

    #[test]
    fn colors_parse_with_stylo_and_serialize_as_the_standard_says() {
        let rgba = |text: &str| parse_color(text).map(rgba8);
        assert_eq!(rgba("red"), Some([255, 0, 0, 255]));
        assert_eq!(rgba("#0f08"), Some([0, 255, 0, 136]));
        assert_eq!(rgba("hsl(240 100% 50%)"), Some([0, 0, 255, 255]));
        assert_eq!(rgba("rgb(0 0 0 / 30%)"), Some([0, 0, 0, 77]));
        // No element to inherit from: `currentcolor` is opaque black.
        assert_eq!(rgba("currentcolor"), Some([0, 0, 0, 255]));
        assert_eq!(rgba(" CurrentColor "), Some([0, 0, 0, 255]));
        assert_eq!(rgba("transparent"), Some([0, 0, 0, 0]));
        assert_eq!(rgba("not a color"), None);
        assert_eq!(rgba("red blue"), None);

        assert_eq!(
            serialize_color(Color::from_rgba8(255, 0, 16, 255)),
            "#ff0010"
        );
        assert_eq!(
            serialize_color(Color::from_rgba8(0, 0, 255, 128)),
            "rgba(0, 0, 255, 0.5)"
        );
        assert_eq!(
            serialize_color(Color::from_rgba8(0, 0, 0, 77)),
            "rgba(0, 0, 0, 0.3)"
        );
        assert_eq!(
            serialize_color(Color::from_rgba8(0, 0, 0, 0)),
            "rgba(0, 0, 0, 0)"
        );
    }

    #[test]
    fn the_font_shorthand_parses_with_stylo() {
        // Stylo serializes a family name of plain identifiers without quotes.
        let parsed = parse_font("italic bold 20px \"Times New Roman\", serif").unwrap();
        assert_eq!(parsed.css, "italic bold 20px Times New Roman, serif");
        assert!((parsed.font.size - 20.0).abs() < f32::EPSILON);
        assert!((parsed.font.weight - 700.0).abs() < f32::EPSILON);
        assert_eq!(parsed.font.style, ResolvedFontStyle::Italic);
        assert_eq!(
            parsed.font.families,
            [
                ResolvedFamily::Named("Times New Roman".to_owned()),
                ResolvedFamily::Generic(GenericFamily::Serif)
            ]
        );
        // Relative sizes resolve against 10px; the line height is dropped.
        let size = |text: &str| parse_font(text).map(|parsed| (parsed.font.size, parsed.css));
        assert_eq!(size("2em serif"), Some((20.0, "20px serif".to_owned())));
        assert_eq!(size("150% serif"), Some((15.0, "15px serif".to_owned())));
        assert_eq!(size("larger serif"), Some((12.0, "12px serif".to_owned())));
        assert_eq!(
            size("12px/30px sans-serif"),
            Some((12.0, "12px sans-serif".to_owned()))
        );
        assert_eq!(size("medium serif").map(|(px, _)| px), Some(16.0));
        // Not a shorthand, a CSS-wide keyword, or a size that needs a viewport.
        for refused in ["inherit", "initial", "20px", "bold", "10vw serif", ""] {
            assert!(parse_font(refused).is_none(), "{refused}");
        }
    }

    /// `bolder`/`lighter` against the default font's weight, and the
    /// `font-stretch` keywords as ratios.
    #[test]
    fn relative_weights_and_width_keywords_resolve() {
        let font = |text: &str| parse_font(text).unwrap().font;
        assert!((font("bolder 10px serif").weight - 700.0).abs() < f32::EPSILON);
        assert!((font("lighter 10px serif").weight - 100.0).abs() < f32::EPSILON);
        for (keyword, ratio) in [
            ("ultra-condensed", 0.5),
            ("extra-condensed", 0.625),
            ("condensed", 0.75),
            ("semi-condensed", 0.875),
            ("semi-expanded", 1.125),
            ("expanded", 1.25),
            ("extra-expanded", 1.5),
            ("ultra-expanded", 2.0),
        ] {
            let stretch = font(&format!("{keyword} 10px serif")).stretch;
            assert!(
                (stretch - ratio).abs() < f32::EPSILON,
                "{keyword}: {stretch}"
            );
        }
        assert!((font("10px serif").stretch - 1.0).abs() < f32::EPSILON);
        assert_eq!(
            font("10px monospace, cursive, fantasy, system-ui, sans-serif").families,
            [
                GenericFamily::Monospace,
                GenericFamily::Cursive,
                GenericFamily::Fantasy,
                GenericFamily::SystemUi,
                GenericFamily::SansSerif,
            ]
            .map(ResolvedFamily::Generic)
        );
    }

    /// Neither parser reads a document or anything of the thread it runs on.
    #[test]
    fn colors_and_fonts_parse_on_a_thread_with_no_document() {
        std::thread::spawn(|| {
            assert!(parse_color("rebeccapurple").is_some());
            assert_eq!(parse_font("bold 1em serif").unwrap().css, "bold 10px serif");
        })
        .join()
        .expect("the parsing thread finishes");
    }

    #[test]
    fn a_color_string_sets_the_style_and_answers_its_serialization() {
        let mut canvas = OffscreenCanvas::new(4, 4);
        let mut context = canvas.context_2d();
        assert_eq!(
            context.set_fill_style_css("red").as_deref(),
            Some("#ff0000")
        );
        assert_eq!(
            context
                .set_stroke_style_css("rgb(0 0 255 / 50%)")
                .as_deref(),
            Some("rgba(0, 0, 255, 0.5)")
        );
        assert_eq!(
            context.set_shadow_color_css("currentcolor").as_deref(),
            Some("#000000")
        );
        match context.stroke_style() {
            Style::Color(color) => assert_eq!(rgba8(*color), [0, 0, 255, 128]),
            other => panic!("a color stroke style, not {other:?}"),
        }
        assert_eq!(rgba8(context.shadow_color()), [0, 0, 0, 255]);
        assert_eq!(fill_rgba8(&mut canvas), [255, 0, 0, 255]);
    }

    #[test]
    fn a_string_that_is_not_a_color_leaves_the_attribute_unchanged() {
        let mut canvas = OffscreenCanvas::new(4, 4);
        let mut context = canvas.context_2d();
        context.set_fill_style_css("lime");
        context.set_stroke_style_css("lime");
        context.set_shadow_color_css("lime");
        for refused in ["not a color", "red blue", "", "inherit"] {
            assert_eq!(context.set_fill_style_css(refused), None, "{refused}");
            assert_eq!(context.set_stroke_style_css(refused), None, "{refused}");
            assert_eq!(context.set_shadow_color_css(refused), None, "{refused}");
        }
        match context.stroke_style() {
            Style::Color(color) => assert_eq!(rgba8(*color), [0, 255, 0, 255]),
            other => panic!("a color stroke style, not {other:?}"),
        }
        assert_eq!(rgba8(context.shadow_color()), [0, 255, 0, 255]);
        assert_eq!(fill_rgba8(&mut canvas), [0, 255, 0, 255]);
    }

    #[test]
    fn a_font_string_sets_the_font_or_leaves_it_unchanged() {
        let mut canvas = OffscreenCanvas::new(4, 4);
        let mut context = canvas.context_2d();
        assert_eq!(
            context.set_font_css("bold 2em serif").as_deref(),
            Some("bold 20px serif")
        );
        let set = context.font().clone();
        assert!((set.size - 20.0).abs() < f32::EPSILON);
        for refused in ["inherit", "20px", "10vw serif", "", "caption"] {
            assert_eq!(context.set_font_css(refused), None, "{refused}");
            assert_eq!(context.font(), &set, "{refused}");
        }
    }

    /// HTML checks the offset before the color.
    #[test]
    fn add_color_stop_checks_the_offset_then_the_color() {
        let gradient = Gradient::linear(0.0, 0.0, 1.0, 0.0).unwrap();
        assert_eq!(gradient.add_color_stop_css(0.5, "red"), Ok(()));
        assert_eq!(gradient.add_color_stop_css(0.0, "currentcolor"), Ok(()));
        for offset in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                gradient.add_color_stop_css(offset, "not a color"),
                Err(CanvasError::Type)
            );
            assert_eq!(
                gradient.add_color_stop_css(offset, "red"),
                Err(CanvasError::Type)
            );
        }
        for offset in [-0.1, 1.5] {
            assert_eq!(
                gradient.add_color_stop_css(offset, "not a color"),
                Err(CanvasError::IndexSize)
            );
            assert_eq!(
                gradient.add_color_stop_css(offset, "red"),
                Err(CanvasError::IndexSize)
            );
        }
        assert_eq!(
            gradient.add_color_stop_css(1.0, "not a color"),
            Err(CanvasError::Syntax)
        );
    }
}
