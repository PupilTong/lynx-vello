//! Run-level parameter vocabulary of a paragraph, and its parley translation.
//!
//! Plain resolved-value structs instead of the box-protocol wire format or a
//! trait over `ComputedValues`: the Lynx grammar is narrower than stylo's —
//! there is no `word-spacing`, and `white-space-collapse` has the three values
//! [`WhiteSpaceCollapse`] names — so small enums make the unsupported states
//! unrepresentable instead of mapped and ignored. Font values stay stylo
//! types: they are already seldon public API, cheap to own, and their parley
//! translation is shared with the measurement path.
//!
//! Paragraph-wide values belong to each consumer: the Lynx block's are
//! `crate::text::block::BlockStyle`. What every consumer hands the shaper is
//! a [`BreakStyle`], the line-breaking values parley keeps on each style.

use std::borrow::Cow;

use parley::setting::Tag;
use parley::{
    Alignment, FontFamily as ParleyFontFamily, FontFamilyName as ParleyFontFamilyName, FontFeature,
    FontFeatures, FontVariation, FontVariations, FontWeight as ParleyFontWeight,
    GenericFamily as ParleyGenericFamily, IndentOptions, LineHeight as ParleyLineHeight,
    OverflowWrap as ParleyOverflowWrap, TextStyle as ParleyTextStyle,
    TextWrapMode as ParleyTextWrapMode, WordBreak as ParleyWordBreak,
};
use stylo::values::computed::font::{GenericFontFamily, SingleFontFamily};
use stylo::values::computed::{
    FontFamily, FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight,
};

use crate::style::TextBrush;
use crate::text::line::generic_family;

#[derive(Debug, Clone, PartialEq)]
pub struct RunStyle {
    pub font_family: FontFamily,
    pub font_size: f32,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub font_features: FontFeatureSettings,
    pub font_variations: FontVariationSettings,
    pub letter_spacing: f32,
    pub line_height: LineHeight,
}

impl Default for RunStyle {
    fn default() -> Self {
        Self {
            font_family: FontFamily::generic(GenericFontFamily::SansSerif).clone(),
            font_size: 16.0,
            font_weight: FontWeight::NORMAL,
            font_style: FontStyle::NORMAL,
            font_features: FontFeatureSettings::normal(),
            font_variations: FontVariationSettings::normal(),
            letter_spacing: 0.0,
            line_height: LineHeight::Normal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LineHeight {
    #[default]
    Normal,
    Number(f32),
    Px(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Start,
    End,
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextWrap {
    #[default]
    Wrap,
    NoWrap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WhiteSpaceCollapse {
    #[default]
    Collapse,
    PreserveBreaks,
    Preserve,
    BreakSpaces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextSecurity {
    #[default]
    None,
    Circle,
    Disc,
    Square,
}

impl TextSecurity {
    pub(in crate::text) const fn mask(self) -> Option<char> {
        match self {
            Self::None => None,
            Self::Circle => Some('\u{25E6}'),
            Self::Disc => Some('\u{2022}'),
            Self::Square => Some('\u{25A0}'),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WordBreak {
    #[default]
    Normal,
    BreakAll,
    KeepAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverflowWrap {
    #[default]
    Normal,
    Anywhere,
    BreakWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BreakStyle {
    pub word_break: WordBreak,
    pub text_wrap: TextWrap,
    pub overflow_wrap: OverflowWrap,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextIndent {
    Px(f32),
    Percent(f32),
}

impl Default for TextIndent {
    fn default() -> Self {
        Self::Px(0.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndentKeywords {
    pub each_line: bool,
    pub hanging: bool,
}

impl IndentKeywords {
    pub(in crate::text) const fn options(self) -> IndentOptions {
        IndentOptions {
            each_line: self.each_line,
            hanging: self.hanging,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum VerticalAlign {
    #[default]
    Baseline,
    Sub,
    Super,
    Top,
    TextTop,
    Middle,
    Bottom,
    TextBottom,
    Length(f32),
    Percent(f32),
    Center,
}

pub(in crate::text) fn resolve_alignment(text_align: TextAlign, direction: Direction) -> Alignment {
    match (text_align, direction) {
        (TextAlign::Left, _)
        | (TextAlign::Start, Direction::Ltr)
        | (TextAlign::End, Direction::Rtl) => Alignment::Left,
        (TextAlign::Right, _)
        | (TextAlign::Start, Direction::Rtl)
        | (TextAlign::End, Direction::Ltr) => Alignment::Right,
        (TextAlign::Center, _) => Alignment::Center,
        (TextAlign::Justify, _) => Alignment::Justify,
    }
}

pub(in crate::text) fn parley_style(
    run: &RunStyle,
    breaking: BreakStyle,
) -> ParleyTextStyle<'_, 'static, TextBrush> {
    let features = run
        .font_features
        .0
        .iter()
        .map(|setting| {
            FontFeature::new(
                Tag::from_bytes(setting.tag.0.to_be_bytes()),
                u16::try_from(setting.value).unwrap_or(0),
            )
        })
        .collect::<Vec<_>>();
    let variations = run
        .font_variations
        .0
        .iter()
        .map(|setting| {
            FontVariation::new(Tag::from_bytes(setting.tag.0.to_be_bytes()), setting.value)
        })
        .collect::<Vec<_>>();

    ParleyTextStyle {
        font_family: translate_font_family_list(&run.font_family),
        font_size: run.font_size,
        font_style: crate::text::ResolvedFontStyle::from(run.font_style).into(),
        font_weight: ParleyFontWeight::new(run.font_weight.value()),
        font_variations: if variations.is_empty() {
            FontVariations::empty()
        } else {
            FontVariations::List(Cow::Owned(variations))
        },
        font_features: if features.is_empty() {
            FontFeatures::empty()
        } else {
            FontFeatures::List(Cow::Owned(features))
        },
        line_height: match run.line_height {
            LineHeight::Normal => ParleyLineHeight::MetricsRelative(1.0),
            LineHeight::Number(factor) => ParleyLineHeight::FontSizeRelative(factor),
            LineHeight::Px(px) => ParleyLineHeight::Absolute(px),
        },
        letter_spacing: run.letter_spacing,
        ..ParleyTextStyle::default()
    }
    .with_breaking(breaking)
}

pub(in crate::text) trait WithBreaking: Sized {
    #[must_use]
    fn with_breaking(self, breaking: BreakStyle) -> Self;
}

impl WithBreaking for ParleyTextStyle<'_, '_, TextBrush> {
    fn with_breaking(self, breaking: BreakStyle) -> Self {
        Self {
            word_break: match breaking.word_break {
                WordBreak::Normal => ParleyWordBreak::Normal,
                WordBreak::BreakAll => ParleyWordBreak::BreakAll,
                WordBreak::KeepAll => ParleyWordBreak::KeepAll,
            },
            overflow_wrap: match breaking.overflow_wrap {
                OverflowWrap::Normal => ParleyOverflowWrap::Normal,
                OverflowWrap::Anywhere => ParleyOverflowWrap::Anywhere,
                OverflowWrap::BreakWord => ParleyOverflowWrap::BreakWord,
            },
            text_wrap_mode: match breaking.text_wrap {
                TextWrap::Wrap => ParleyTextWrapMode::Wrap,
                TextWrap::NoWrap => ParleyTextWrapMode::NoWrap,
            },
            ..self
        }
    }
}

impl From<stylo::values::computed::TextAlign> for TextAlign {
    fn from(value: stylo::values::computed::TextAlign) -> Self {
        use stylo::values::computed::TextAlign as StyloAlign;

        match value {
            StyloAlign::Start => Self::Start,
            StyloAlign::End => Self::End,
            StyloAlign::Left => Self::Left,
            StyloAlign::Right => Self::Right,
            StyloAlign::Center => Self::Center,
        }
    }
}

impl From<stylo::computed_values::direction::T> for Direction {
    fn from(value: stylo::computed_values::direction::T) -> Self {
        use stylo::computed_values::direction;

        match value {
            direction::T::Rtl => Self::Rtl,
            direction::T::Ltr => Self::Ltr,
        }
    }
}

impl From<stylo::computed_values::text_wrap_mode::T> for TextWrap {
    fn from(value: stylo::computed_values::text_wrap_mode::T) -> Self {
        use stylo::computed_values::text_wrap_mode::T as WrapMode;

        match value {
            WrapMode::Wrap => Self::Wrap,
            WrapMode::Nowrap => Self::NoWrap,
        }
    }
}

impl From<stylo::values::computed::WordBreak> for WordBreak {
    fn from(value: stylo::values::computed::WordBreak) -> Self {
        match value {
            stylo::values::computed::WordBreak::BreakAll => Self::BreakAll,
            stylo::values::computed::WordBreak::KeepAll => Self::KeepAll,
            stylo::values::computed::WordBreak::Normal => Self::Normal,
        }
    }
}

impl RunStyle {
    #[must_use]
    pub fn from_run_style<S: crate::style::TextRunStyle>(style: &S) -> Self {
        use stylo::values::computed::LineHeight as StyloLineHeight;

        Self {
            font_family: style.font_family(),
            font_size: style.font_size(),
            font_weight: style.font_weight(),
            font_style: style.font_style(),
            font_features: style.font_feature_settings(),
            font_variations: style.font_variation_settings(),
            letter_spacing: style
                .letter_spacing()
                .0
                .resolve(stylo::values::computed::Length::new(0.0))
                .px(),
            line_height: match style.line_height() {
                StyloLineHeight::Normal => LineHeight::Normal,
                StyloLineHeight::Number(factor) => LineHeight::Number(factor.0),
                StyloLineHeight::Length(length) => LineHeight::Px(length.0.px()),
            },
        }
    }
}

pub(in crate::text) fn translate_font_family_list(family: &FontFamily) -> ParleyFontFamily<'_> {
    let families = &family.families.list;
    if families.is_empty() {
        return ParleyGenericFamily::SansSerif.into();
    }
    if families.len() == 1 {
        return translate_font_family_name(&families[0]).into();
    }
    ParleyFontFamily::List(Cow::Owned(
        families.iter().map(translate_font_family_name).collect(),
    ))
}

impl crate::text::ResolvedFont {
    #[must_use]
    pub fn from_computed(font: &stylo::properties::style_structs::Font) -> Self {
        use crate::text::{ResolvedFamily, ResolvedFontStyle};

        let families = font
            .font_family
            .families
            .list
            .iter()
            .map(ResolvedFamily::from)
            .collect();
        Self {
            families,
            size: font.font_size.computed_size().px(),
            weight: font.font_weight.value(),
            style: ResolvedFontStyle::from(font.font_style),
            stretch: font.font_width.to_percentage().0,
        }
    }
}

pub(in crate::text) fn translate_font_family_name(
    single: &SingleFontFamily,
) -> ParleyFontFamilyName<'_> {
    match single {
        SingleFontFamily::FamilyName(name) => {
            ParleyFontFamilyName::Named(Cow::Borrowed(name.name.as_ref()))
        }
        SingleFontFamily::Generic(generic) => {
            ParleyFontFamilyName::Generic(generic_family(*generic))
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use parley::FontStyle as ParleyFontStyle;
    use stylo::values::generics::font::{FeatureTagValue, FontSettings, FontTag, VariationValue};

    use super::*;

    #[test]
    fn alignment_resolves_against_the_declared_direction() {
        for (text_align, direction, expected) in [
            (TextAlign::Left, Direction::Ltr, Alignment::Left),
            (TextAlign::Left, Direction::Rtl, Alignment::Left),
            (TextAlign::Right, Direction::Ltr, Alignment::Right),
            (TextAlign::Right, Direction::Rtl, Alignment::Right),
            (TextAlign::Center, Direction::Ltr, Alignment::Center),
            (TextAlign::Center, Direction::Rtl, Alignment::Center),
            (TextAlign::Justify, Direction::Ltr, Alignment::Justify),
            (TextAlign::Justify, Direction::Rtl, Alignment::Justify),
            (TextAlign::Start, Direction::Ltr, Alignment::Left),
            (TextAlign::Start, Direction::Rtl, Alignment::Right),
            (TextAlign::End, Direction::Ltr, Alignment::Right),
            (TextAlign::End, Direction::Rtl, Alignment::Left),
        ] {
            assert_eq!(
                resolve_alignment(text_align, direction),
                expected,
                "for {text_align:?} under {direction:?}",
            );
        }
    }

    #[test]
    fn run_translation_carries_font_identity_features_and_variations() {
        let run = RunStyle {
            font_style: FontStyle::ITALIC,
            font_weight: FontWeight::from_float(700.0),
            font_features: FontSettings(
                vec![FeatureTagValue {
                    tag: FontTag(u32::from_be_bytes(*b"kern")),
                    value: 1,
                }]
                .into(),
            ),
            font_variations: FontSettings(
                vec![VariationValue {
                    tag: FontTag(u32::from_be_bytes(*b"wght")),
                    value: 650.0,
                }]
                .into(),
            ),
            ..RunStyle::default()
        };

        let style = parley_style(&run, BreakStyle::default());
        assert!(matches!(style.font_style, ParleyFontStyle::Italic));
        assert_eq!(style.font_weight, ParleyFontWeight::new(700.0));
        assert!(matches!(&style.font_features, FontFeatures::List(list) if list.len() == 1));
        assert!(matches!(&style.font_variations, FontVariations::List(list) if list.len() == 1));

        let oblique = RunStyle {
            font_style: FontStyle::oblique(30.0),
            ..RunStyle::default()
        };
        assert!(matches!(
            parley_style(&oblique, BreakStyle::default()).font_style,
            ParleyFontStyle::Oblique(Some(degrees)) if degrees == 30.0,
        ));
    }

    #[test]
    fn overflow_wrap_translates_every_value() {
        for (overflow_wrap, expected) in [
            (OverflowWrap::Normal, ParleyOverflowWrap::Normal),
            (OverflowWrap::Anywhere, ParleyOverflowWrap::Anywhere),
            (OverflowWrap::BreakWord, ParleyOverflowWrap::BreakWord),
        ] {
            let breaking = BreakStyle {
                overflow_wrap,
                ..BreakStyle::default()
            };
            assert_eq!(
                parley_style(&RunStyle::default(), breaking).overflow_wrap,
                expected,
            );
        }
    }
}
