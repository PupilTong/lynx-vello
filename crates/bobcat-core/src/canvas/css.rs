//! The CSS strings a canvas attribute takes, parsed by stylo: a `<color>`
//! (`fillStyle`, `strokeStyle`, `shadowColor`, `addColorStop`) and the `font`
//! shorthand.
//!
//! Both parse with an author-origin `ParserContext` over `about:blank` and
//! read nothing of a document or a device, so they run on either engine
//! thread.

use std::borrow::Cow;
use std::sync::{Arc, LazyLock};

use bobcat_canvas::Font;
use bobcat_canvas::parley::{FontFamilyName, FontStyle, FontWeight, FontWidth, GenericFamily};
use bobcat_canvas::peniko::Color;
use cssparser::Parser;
use stylo::color::{AbsoluteColor, ColorSpace};
use stylo::context::QuirksMode;
use stylo::custom_properties::AttrTaint;
use stylo::parser::ParserContext;
use stylo::properties::shorthands::font;
use stylo::stylesheets::{CssRuleType, Origin, UrlExtraData};
use stylo::values::computed::Length;
use stylo::values::computed::font::{GenericFontFamily, SingleFontFamily};
use stylo::values::specified::font::{
    AbsoluteFontWeight, FontFamily, FontSize, FontStyle as SpecifiedFontStyle,
    FontWeight as SpecifiedFontWeight, FontWidth as SpecifiedFontWidth, FontWidthKeyword,
    LineHeight,
};
use stylo::values::specified::length::{LengthPercentage, NoCalcLength};
use stylo::values::specified::{self, Color as SpecifiedColor};
use stylo_traits::{CssWriter, ParsingMode, ToCss};
use terminus::link::stylo;

/// The base a relative font size resolves against: the default canvas
/// font's 10px, HTML's base for an `OffscreenCanvas`, whose font style
/// source object is not an element (`crates/bobcat-core/docs/canvas.md`).
const BASE_FONT_SIZE: f32 = 10.0;
/// `medium`, the size the absolute-size keywords are scaled from.
const MEDIUM_FONT_SIZE: f32 = 16.0;
/// CSS Fonts' ratio between adjacent sizes, for `larger` and `smaller`.
const RELATIVE_SIZE_RATIO: f32 = 1.2;

static URL_DATA: LazyLock<UrlExtraData> = LazyLock::new(|| {
    UrlExtraData::from(url::Url::parse("about:blank").expect("about:blank is a valid URL"))
});

fn parser_context() -> ParserContext<'static> {
    ParserContext::new(
        Origin::Author,
        &URL_DATA,
        Some(CssRuleType::Style),
        ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        Cow::default(),
        None,
        None,
        AttrTaint::default(),
    )
}

/// A CSS `<color>` as sRGB, `currentcolor` as opaque black; `None` when
/// `text` is not one, or names a system color, which needs a device.
///
/// The parser is the fork's `lynx` grammar, which leaves out `currentcolor`
/// (and `color-mix()`, `light-dark()`, `contrast-color()`): the keyword is
/// matched here, first.
pub(crate) fn parse_color(text: &str) -> Option<Color> {
    let current =
        Parser::new(text).parse_entirely(|parser| -> Result<(), cssparser::ParseError<()>> {
            Ok(parser.expect_ident_matching("currentcolor")?)
        });
    if current.is_ok() {
        return Some(Color::BLACK);
    }
    let context = parser_context();
    let mut parser = Parser::new(text);
    let computed = SpecifiedColor::parse_and_compute(&context, &mut parser, None).ok()?;
    let srgb = computed
        .resolve_to_absolute(&AbsoluteColor::BLACK)
        .to_color_space(ColorSpace::Srgb);
    let channel = |value: f32| {
        if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, 1.0)
        }
    };
    Some(Color::new([
        channel(srgb.components.0),
        channel(srgb.components.1),
        channel(srgb.components.2),
        channel(srgb.alpha),
    ]))
}

/// HTML's serialization of a canvas color: `#rrggbb` when it is opaque,
/// `rgba(r, g, b, a)` otherwise, `a` in the fewest decimals that keep its
/// 8-bit value.
pub(crate) fn serialize_color(color: Color) -> String {
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
pub(crate) struct ParsedFont {
    pub(crate) font: Font,
    pub(crate) css: String,
}

/// The CSS `font` shorthand, as HTML's `font` setter takes it: `None` for
/// text that does not parse as the shorthand — a CSS-wide keyword included —
/// and for a size this engine cannot resolve without a document (viewport
/// and container units, `rpx`, `calc()` other than of absolute lengths).
/// `line-height` is parsed and dropped. The serialization is the shorthand's
/// own, with the size in px and no line height.
pub(crate) fn parse_font(text: &str) -> Option<ParsedFont> {
    let context = parser_context();
    let mut parser = Parser::new(text);
    let longhands = parser
        .parse_entirely(|parser| font::parse_value(&context, parser))
        .ok()?;
    let size = font_size_px(&longhands.font_size)?;
    if !size.is_finite() || size < 0.0 {
        return None;
    }
    let families = families(&longhands.font_family)?;
    let font = Font {
        families,
        size,
        weight: weight(&longhands.font_weight)?,
        style: style(&longhands.font_style)?,
        width: width(&longhands.font_width)?,
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

fn families(family: &FontFamily) -> Option<Arc<[FontFamilyName<'static>]>> {
    let FontFamily::Values(list) = family else {
        return None;
    };
    Some(
        list.iter()
            .map(|single| match single {
                SingleFontFamily::FamilyName(name) => {
                    FontFamilyName::Named(Cow::Owned(name.name.to_string()))
                }
                SingleFontFamily::Generic(generic) => FontFamilyName::Generic(match generic {
                    GenericFontFamily::Serif => GenericFamily::Serif,
                    GenericFontFamily::Monospace => GenericFamily::Monospace,
                    GenericFontFamily::Cursive => GenericFamily::Cursive,
                    GenericFontFamily::Fantasy => GenericFamily::Fantasy,
                    GenericFontFamily::SystemUi => GenericFamily::SystemUi,
                    _ => GenericFamily::SansSerif,
                }),
            })
            .collect(),
    )
}

/// The weight; `bolder` and `lighter` relative to the default font's 400.
fn weight(weight: &SpecifiedFontWeight) -> Option<FontWeight> {
    let value = match weight {
        SpecifiedFontWeight::Absolute(absolute) => absolute.compute()?.value(),
        SpecifiedFontWeight::Bolder => AbsoluteFontWeight::Bold.compute()?.value(),
        SpecifiedFontWeight::Lighter => 100.0,
        SpecifiedFontWeight::System(_) => return None,
    };
    Some(FontWeight::new(value))
}

fn style(style: &SpecifiedFontStyle) -> Option<FontStyle> {
    match style {
        SpecifiedFontStyle::Specified(specified::font::SpecifiedFontStyle::Italic) => {
            Some(FontStyle::Italic)
        }
        SpecifiedFontStyle::Specified(specified::font::SpecifiedFontStyle::Oblique(angle)) => {
            // A `calc()` angle needs a computed-value context.
            let degrees = angle.degrees()?;
            Some(if degrees == 0.0 {
                FontStyle::Normal
            } else {
                FontStyle::Oblique(Some(degrees))
            })
        }
        SpecifiedFontStyle::System(_) => None,
    }
}

/// `font-stretch`'s keyword as a width ratio (CSS Fonts 4, "font-width").
fn width(width: &SpecifiedFontWidth) -> Option<FontWidth> {
    let SpecifiedFontWidth::Keyword(keyword) = width else {
        return None;
    };
    let percent = match keyword {
        FontWidthKeyword::UltraCondensed => 50.0,
        FontWidthKeyword::ExtraCondensed => 62.5,
        FontWidthKeyword::Condensed => 75.0,
        FontWidthKeyword::SemiCondensed => 87.5,
        FontWidthKeyword::Normal => 100.0,
        FontWidthKeyword::SemiExpanded => 112.5,
        FontWidthKeyword::Expanded => 125.0,
        FontWidthKeyword::ExtraExpanded => 150.0,
        FontWidthKeyword::UltraExpanded => 200.0,
    };
    Some(FontWidth::from_ratio(percent / 100.0))
}
