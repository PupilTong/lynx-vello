//! Text measurement style protocol.
//!
//! This module is deliberately independent of Parley. It is the stable seam
//! between host-owned computed styles and an optional text measurement
//! adapter: the host lends paragraph and run style views, and the adapter
//! translates their small values only when it shapes text. Paint-only values
//! (brushes, color, decoration, shadow, and stroke) are outside this
//! measurement protocol.
//!
//! Sequence-valued run styles use GAT iterators, following the Grid track-list
//! protocol. A host can therefore lend font families and OpenType settings
//! directly from its computed-style storage without materializing an
//! engine-owned style struct.

use crate::style::CoreStyle;
use crate::style::value::LengthPercentage;

/// `text-align`: inline-axis alignment of a paragraph's line boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextAlign {
    /// Align to the physical left edge.
    Left,
    /// Center each line.
    Center,
    /// Align to the physical right edge.
    Right,
    /// Align to the inline-start edge (left in LTR, right in RTL).
    #[default]
    Start,
    /// Align to the inline-end edge (right in LTR, left in RTL).
    End,
    /// Expand inter-word opportunities to fill the line.
    Justify,
}

/// Lynx's supported subset of `white-space`.
///
/// Newline preservation for `<raw-text>` is represented separately by
/// [`TextRun::preserve_newlines`], not by widening this value with unsupported
/// `pre*` modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WhiteSpace {
    /// Collapse whitespace and wrap lines normally.
    #[default]
    Normal,
    /// Collapse whitespace but do not wrap lines.
    NoWrap,
}

/// Lynx's supported subset of CSS `word-break`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WordBreak {
    /// Use the language's ordinary soft wrap opportunities.
    #[default]
    Normal,
    /// Permit breaks between any two characters (except where CSS forbids
    /// them for CJK text).
    BreakAll,
    /// Suppress breaks between CJK characters.
    KeepAll,
}

/// CSS generic font families.
///
/// The default matches the user-agent default used by the text adapter. A
/// host should still expose its fully computed `font-family` list rather than
/// relying on this value's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GenericFontFamily {
    /// A proportional serif face.
    Serif,
    /// A proportional sans-serif face.
    #[default]
    SansSerif,
    /// A fixed-pitch face.
    Monospace,
    /// A cursive or handwritten face.
    Cursive,
    /// A decorative face.
    Fantasy,
    /// The platform user-interface face.
    SystemUi,
    /// The platform user-interface serif face.
    UiSerif,
    /// The platform user-interface sans-serif face.
    UiSansSerif,
    /// The platform user-interface monospace face.
    UiMonospace,
    /// The platform user-interface rounded face.
    UiRounded,
    /// A face designed for emoji.
    Emoji,
    /// A face designed for mathematical notation.
    Math,
    /// The Chinese Fang Song style.
    FangSong,
}

/// One entry in a computed `font-family` fallback list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFamily<'a> {
    /// A host-resolved family name.
    Named(&'a str),
    /// A CSS generic family.
    Generic(GenericFontFamily),
}

impl Default for FontFamily<'_> {
    fn default() -> Self {
        Self::Generic(GenericFontFamily::default())
    }
}

/// `font-weight` values supported by Lynx.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FontWeight {
    /// `normal` (equivalent to `400`).
    #[default]
    Normal,
    /// `bold` (equivalent to `700`).
    Bold,
    /// Numeric weight `100`.
    W100,
    /// Numeric weight `200`.
    W200,
    /// Numeric weight `300`.
    W300,
    /// Numeric weight `400`.
    W400,
    /// Numeric weight `500`.
    W500,
    /// Numeric weight `600`.
    W600,
    /// Numeric weight `700`.
    W700,
    /// Numeric weight `800`.
    W800,
    /// Numeric weight `900`.
    W900,
}

impl FontWeight {
    /// Returns the numeric OpenType weight represented by this CSS value.
    #[must_use]
    pub const fn value(self) -> u16 {
        match self {
            Self::Normal | Self::W400 => 400,
            Self::Bold | Self::W700 => 700,
            Self::W100 => 100,
            Self::W200 => 200,
            Self::W300 => 300,
            Self::W500 => 500,
            Self::W600 => 600,
            Self::W800 => 800,
            Self::W900 => 900,
        }
    }
}

/// Lynx's supported subset of CSS `font-style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FontStyle {
    /// Upright glyphs.
    #[default]
    Normal,
    /// A designed italic face, with synthesis as a fallback.
    Italic,
    /// A slanted face, with synthesis as a fallback.
    Oblique,
}

/// Computed `line-height`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LineHeight {
    /// The font's normal line metrics.
    #[default]
    Normal,
    /// A unitless factor multiplied by the run's font size.
    Factor(f32),
    /// An absolute line height in CSS pixels.
    Length(f32),
}

/// A four-byte OpenType feature or variation axis tag.
pub type OpenTypeTag = [u8; 4];

/// One `font-feature-settings` entry: `(OpenType tag, feature value)`.
pub type FontFeatureSetting = (OpenTypeTag, u16);

/// One `font-variation-settings` entry: `(OpenType axis tag, coordinate)`.
pub type FontVariationSetting = (OpenTypeTag, f32);

/// Paragraph-level style of a `<text>` leaf box.
///
/// Inline direction comes from [`CoreStyle::direction`]; it is not duplicated
/// here. Defaults are the CSS initial values.
pub trait TextContainerStyle: CoreStyle {
    /// `text-align`.
    fn text_align(&self) -> TextAlign {
        TextAlign::Start
    }

    /// `white-space` (the two values Lynx exposes).
    fn white_space(&self) -> WhiteSpace {
        WhiteSpace::Normal
    }

    /// `word-break`.
    fn word_break(&self) -> WordBreak {
        WordBreak::Normal
    }

    /// `text-indent`.
    ///
    /// Percentage basis: the containing block's inline size.
    fn text_indent(&self) -> LengthPercentage {
        LengthPercentage::ZERO
    }
}

/// Shaping style for one host-assembled text run.
///
/// Runs are inline content rather than boxes, so this intentionally does not
/// inherit [`CoreStyle`]. The three sequence accessors have no defaults: an
/// associated iterator type cannot be conjured by a default method. Empty
/// iterators mean no explicit family/settings entries; the adapter supplies
/// its user-agent fallback family in that case.
///
/// This trait is intentionally not object-safe. Its GAT iterators preserve
/// borrowed host storage and keep text measurement statically dispatched.
///
/// ```compile_fail
/// use neutron_star::style::TextRunStyle;
/// fn erased(_: &dyn TextRunStyle) {}
/// ```
pub trait TextRunStyle: Sized {
    /// Borrowed iterator over the computed `font-family` fallback list.
    type FontFamilies<'a>: Iterator<Item = FontFamily<'a>>
    where
        Self: 'a;

    /// Borrowed iterator over computed `font-feature-settings` entries.
    type FontFeatureSettings<'a>: Iterator<Item = FontFeatureSetting>
    where
        Self: 'a;

    /// Borrowed iterator over computed `font-variation-settings` entries.
    type FontVariationSettings<'a>: Iterator<Item = FontVariationSetting>
    where
        Self: 'a;

    /// The computed `font-family` fallback list.
    fn font_families(&self) -> Self::FontFamilies<'_>;

    /// Computed font size in CSS pixels.
    fn font_size(&self) -> f32 {
        16.0
    }

    /// Computed `font-weight`.
    fn font_weight(&self) -> FontWeight {
        FontWeight::Normal
    }

    /// Computed `font-style`.
    fn font_style(&self) -> FontStyle {
        FontStyle::Normal
    }

    /// Computed `letter-spacing` in CSS pixels (`normal` is `0`).
    fn letter_spacing(&self) -> f32 {
        0.0
    }

    /// Computed `line-height`.
    fn line_height(&self) -> LineHeight {
        LineHeight::Normal
    }

    /// Computed `font-feature-settings` entries.
    fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_>;

    /// Computed `font-variation-settings` entries.
    fn font_variation_settings(&self) -> Self::FontVariationSettings<'_>;
}

/// One borrowed text/style run assembled by the host.
///
/// A paragraph is a sequence of these values, allowing mixed styles without
/// flattening them into a single materialized style. `preserve_newlines`
/// models `<raw-text>`'s hard-coded `white-space-collapse: preserve-breaks`
/// behavior independently of the container's [`WhiteSpace`] value.
#[derive(Debug)]
pub struct TextRun<'a, R: TextRunStyle> {
    /// UTF-8 text covered by this run.
    pub text: &'a str,
    /// Borrowed shaping style for this run.
    pub style: &'a R,
    /// Whether newline characters create forced line breaks in this run.
    pub preserve_newlines: bool,
}

impl<R: TextRunStyle> Clone for TextRun<'_, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R: TextRunStyle> Copy for TextRun<'_, R> {}

/// Measurement-only Parley brush.
///
/// Painting can widen this alias additively when render styling joins the
/// protocol; measurement currently carries no paint data.
pub type TextBrush = ();

impl<S: TextContainerStyle> TextContainerStyle for &S {
    fn text_align(&self) -> TextAlign {
        (**self).text_align()
    }

    fn white_space(&self) -> WhiteSpace {
        (**self).white_space()
    }

    fn word_break(&self) -> WordBreak {
        (**self).word_break()
    }

    fn text_indent(&self) -> LengthPercentage {
        (**self).text_indent()
    }
}

impl<S: TextRunStyle> TextRunStyle for &S {
    type FontFamilies<'a>
        = S::FontFamilies<'a>
    where
        Self: 'a;
    type FontFeatureSettings<'a>
        = S::FontFeatureSettings<'a>
    where
        Self: 'a;
    type FontVariationSettings<'a>
        = S::FontVariationSettings<'a>
    where
        Self: 'a;

    fn font_families(&self) -> Self::FontFamilies<'_> {
        S::font_families(&**self)
    }

    fn font_size(&self) -> f32 {
        (**self).font_size()
    }

    fn font_weight(&self) -> FontWeight {
        (**self).font_weight()
    }

    fn font_style(&self) -> FontStyle {
        (**self).font_style()
    }

    fn letter_spacing(&self) -> f32 {
        (**self).letter_spacing()
    }

    fn line_height(&self) -> LineHeight {
        (**self).line_height()
    }

    fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
        S::font_feature_settings(&**self)
    }

    fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
        S::font_variation_settings(&**self)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {}
    impl TextContainerStyle for Defaults {}

    impl TextRunStyle for Defaults {
        type FontFamilies<'a> = core::iter::Empty<FontFamily<'a>>;
        type FontFeatureSettings<'a> = core::iter::Empty<FontFeatureSetting>;
        type FontVariationSettings<'a> = core::iter::Empty<FontVariationSetting>;

        fn font_families(&self) -> Self::FontFamilies<'_> {
            core::iter::empty()
        }

        fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
            core::iter::empty()
        }

        fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
            core::iter::empty()
        }
    }

    #[test]
    fn text_defaults_match_css_initial_values() {
        let style = Defaults;

        assert_eq!(style.text_align(), TextAlign::Start);
        assert_eq!(style.white_space(), WhiteSpace::Normal);
        assert_eq!(style.word_break(), WordBreak::Normal);
        assert_eq!(style.text_indent(), LengthPercentage::ZERO);
        assert_eq!(style.font_families().count(), 0);
        assert_eq!(style.font_size(), 16.0);
        assert_eq!(style.font_weight(), FontWeight::Normal);
        assert_eq!(style.font_style(), FontStyle::Normal);
        assert_eq!(style.letter_spacing(), 0.0);
        assert_eq!(style.line_height(), LineHeight::Normal);
        assert_eq!(style.font_feature_settings().count(), 0);
        assert_eq!(style.font_variation_settings().count(), 0);
        assert_eq!(
            FontFamily::default(),
            FontFamily::Generic(GenericFontFamily::SansSerif)
        );
    }

    #[derive(Debug)]
    struct BorrowedStyle {
        named_family: String,
        features: Vec<FontFeatureSetting>,
        variations: Vec<FontVariationSetting>,
    }

    impl TextRunStyle for BorrowedStyle {
        type FontFamilies<'a> = core::array::IntoIter<FontFamily<'a>, 2>;
        type FontFeatureSettings<'a> =
            core::iter::Copied<core::slice::Iter<'a, FontFeatureSetting>>;
        type FontVariationSettings<'a> =
            core::iter::Copied<core::slice::Iter<'a, FontVariationSetting>>;

        fn font_families(&self) -> Self::FontFamilies<'_> {
            [
                FontFamily::Named(&self.named_family),
                FontFamily::Generic(GenericFontFamily::SansSerif),
            ]
            .into_iter()
        }

        fn font_size(&self) -> f32 {
            20.0
        }

        fn font_weight(&self) -> FontWeight {
            FontWeight::Bold
        }

        fn font_style(&self) -> FontStyle {
            FontStyle::Italic
        }

        fn letter_spacing(&self) -> f32 {
            1.25
        }

        fn line_height(&self) -> LineHeight {
            LineHeight::Factor(1.5)
        }

        fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
            self.features.iter().copied()
        }

        fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
            self.variations.iter().copied()
        }
    }

    #[test]
    fn borrowed_run_style_and_reference_forwarding_preserve_sequences() {
        fn assert_copy<T: Copy>() {}

        let style = BorrowedStyle {
            named_family: "Ahem".to_owned(),
            features: vec![(*b"liga", 0), (*b"kern", 1)],
            variations: vec![(*b"wght", 625.0)],
        };
        let view = &style;

        assert_eq!(
            TextRunStyle::font_families(&view).collect::<Vec<_>>(),
            vec![
                FontFamily::Named("Ahem"),
                FontFamily::Generic(GenericFontFamily::SansSerif),
            ]
        );
        assert_eq!(TextRunStyle::font_size(&view), 20.0);
        assert_eq!(TextRunStyle::font_weight(&view), FontWeight::Bold);
        assert_eq!(TextRunStyle::font_style(&view), FontStyle::Italic);
        assert_eq!(TextRunStyle::letter_spacing(&view), 1.25);
        assert_eq!(TextRunStyle::line_height(&view), LineHeight::Factor(1.5));
        assert_eq!(
            TextRunStyle::font_feature_settings(&view).collect::<Vec<_>>(),
            style.features
        );
        assert_eq!(
            TextRunStyle::font_variation_settings(&view).collect::<Vec<_>>(),
            style.variations
        );

        let run = TextRun {
            text: "hello",
            style: view,
            preserve_newlines: true,
        };
        assert_eq!(run.text, "hello");
        assert!(run.preserve_newlines);
        assert_copy::<TextRun<'_, BorrowedStyle>>();
    }

    #[test]
    fn font_weight_aliases_have_their_css_numeric_values() {
        assert_eq!(FontWeight::Normal.value(), 400);
        assert_eq!(FontWeight::W400.value(), 400);
        assert_eq!(FontWeight::Bold.value(), 700);
        assert_eq!(FontWeight::W700.value(), 700);
        assert_eq!(FontWeight::W100.value(), 100);
        assert_eq!(FontWeight::W200.value(), 200);
        assert_eq!(FontWeight::W300.value(), 300);
        assert_eq!(FontWeight::W500.value(), 500);
        assert_eq!(FontWeight::W600.value(), 600);
        assert_eq!(FontWeight::W800.value(), 800);
        assert_eq!(FontWeight::W900.value(), 900);
    }
}
