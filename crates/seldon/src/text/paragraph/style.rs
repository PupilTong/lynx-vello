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
    FontFeatures, FontStyle as ParleyFontStyle, FontVariation, FontVariations,
    FontWeight as ParleyFontWeight, GenericFamily as ParleyGenericFamily, IndentOptions,
    LineHeight as ParleyLineHeight, OverflowWrap as ParleyOverflowWrap,
    TextStyle as ParleyTextStyle, TextWrapMode as ParleyTextWrapMode, WordBreak as ParleyWordBreak,
};
use stylo::values::computed::font::{GenericFontFamily, SingleFontFamily};
use stylo::values::computed::{
    FontFamily, FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight,
};

use crate::style::TextBrush;

/// The fully resolved effective style of one text run.
///
/// The Lynx scoped style overlay — a nested text applies only its own declared
/// properties over the inherited run state — is the host cascade's job: on the
/// web compat target nested `x-text` is `display: inline` and plain CSS
/// inheritance produces exactly that overlay, so each run arrives here already
/// carrying the effective values of its innermost enclosing text element.
#[derive(Debug, Clone, PartialEq)]
pub struct RunStyle {
    pub font_family: FontFamily,
    /// CSS px; the block lays out at scale 1.0 like the measurement path.
    pub font_size: f32,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub font_features: FontFeatureSettings,
    pub font_variations: FontVariationSettings,
    /// Resolved px.
    pub letter_spacing: f32,
    /// Per-run, following the web compat target (native textra applies
    /// line-height paragraph-wide; a host wanting that writes one value into
    /// every run, which inheritance does on its own).
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
    /// The CSS unitless number: a factor of the run's font size.
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

/// css-text-4 `white-space-collapse`, per run.
///
/// The four values the fork parses; `discard` and `preserve-spaces` do not
/// parse there at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WhiteSpaceCollapse {
    /// Spaces, tabs and segment breaks collapse (css-text-3 §4.1.1).
    #[default]
    Collapse,
    /// Segment breaks are preserved as forced line breaks; spaces and tabs
    /// still collapse, and the ones next to a preserved break are removed.
    /// What Lynx raw-text carries.
    PreserveBreaks,
    /// Spaces, tabs and segment breaks are all preserved. A preserved segment
    /// break is a forced line break, a preserved space is one cluster per
    /// source space, and a preserved tab advances to the next tab stop
    /// (`super::tab`).
    Preserve,
    /// Preserved like [`Self::Preserve`], except that a wrap opportunity
    /// follows every preserved space and tab and none of them hangs at a
    /// line end: trailing spaces take width and wrap like content
    /// (css-text-3 §4.1.3, §5.1).
    BreakSpaces,
}

/// `-webkit-text-security`, per run: whether the run's characters are
/// replaced by a mask character before white-space processing.
///
/// Under any value but [`Self::None`] every character of the run is masked,
/// white space and newlines included, as Blink's `LayoutText::SecureText`
/// fills the whole text node with the mask before white-space processing: a
/// masked run never collapses, never wraps at a space it had and never breaks
/// at a newline it had. One mask stands for one Unicode scalar value of the
/// source, where Blink fills per UTF-16 code unit, so a caret never lands
/// between the halves of a surrogate pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextSecurity {
    #[default]
    None,
    /// U+25E6 WHITE BULLET, Blink's `kWhiteBulletCharacter`.
    Circle,
    /// U+2022 BULLET, Blink's `kBulletCharacter`.
    Disc,
    /// U+25A0 BLACK SQUARE, Blink's `kBlackSquareCharacter`.
    Square,
}

impl TextSecurity {
    /// The character every source character of a run is shown as, or `None`
    /// for an unmasked run. Each is three bytes in UTF-8
    /// (`super::content::MASK_BYTES`).
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

/// css-text-3 `overflow-wrap`: whether a word with no other break point may
/// be broken at an arbitrary point when it does not fit its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverflowWrap {
    #[default]
    Normal,
    /// The arbitrary break points also count toward the min-content width.
    Anywhere,
    /// The arbitrary break points do not count toward the min-content width.
    BreakWord,
}

/// The line-breaking values parley keeps on every style rather than on the
/// paragraph.
///
/// `parley_style` writes them into each style it builds, and `word_break`
/// also decides whether the paragraph takes the line-break override
/// (`super::shape`). A consumer whose values are paragraph-wide hands the same
/// one for every span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BreakStyle {
    pub word_break: WordBreak,
    pub text_wrap: TextWrap,
    pub overflow_wrap: OverflowWrap,
}

/// The `text-indent` value, as a caller resolves it.
///
/// Deliberately not a paragraph style field: a percentage resolves against the
/// definite inline size, which is the caller's to know, while the block only
/// ever sees a break width — resolving against *that* would make the indent
/// depend on the constraint it helps decide. The resolved px goes into
/// [`BlockConstraint`](super::BlockConstraint).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextIndent {
    Px(f32),
    /// Resolved against a definite layout width; zero when unconstrained.
    Percent(f32),
}

impl Default for TextIndent {
    fn default() -> Self {
        Self::Px(0.0)
    }
}

/// The `text-indent` keywords (css-text-3 §8.1): which lines the indent
/// applies to. Both off is the first line only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndentKeywords {
    /// `each-line`: also every line after a forced line break.
    pub each_line: bool,
    /// `hanging`: the lines the indent would apply to are the ones it skips,
    /// and the other way round.
    pub hanging: bool,
}

impl IndentKeywords {
    /// The same choice in parley's vocabulary.
    pub(in crate::text) const fn options(self) -> IndentOptions {
        IndentOptions {
            each_line: self.each_line,
            hanging: self.hanging,
        }
    }
}

/// Vertical alignment of an atomic inline box on its line.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum VerticalAlign {
    /// Covers Lynx `kDefault` and `kBaseline`.
    #[default]
    Baseline,
    Sub,
    Super,
    Top,
    TextTop,
    Middle,
    Bottom,
    TextBottom,
    /// CSS sign: positive raises the box above its baseline position.
    Length(f32),
    /// A fraction of the line's resolved line height; positive raises.
    Percent(f32),
    /// Lynx-only value; behaves as [`Self::Middle`] per the recorded mapping
    /// in `docs/tracking/css-text.md`.
    Center,
}

/// Maps the declared alignment to parley's physical vocabulary.
///
/// Resolution happens against the declared direction, matching Lynx's
/// `ResolveTextAlign` (physical before the engine); parley's own `Start`/`End`
/// — resolved against the detected content direction — are deliberately
/// unused.
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

/// Translates one run into the parley style the shaper consumes, with the
/// line-breaking values of `breaking` written in.
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
        font_style: if run.font_style == FontStyle::NORMAL {
            ParleyFontStyle::Normal
        } else if run.font_style == FontStyle::ITALIC {
            ParleyFontStyle::Italic
        } else {
            ParleyFontStyle::Oblique(Some(run.font_style.oblique_degrees()))
        },
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
        ..ParleyTextStyle::default()
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

/// Builds one run's parameters from the style of the innermost element its
/// characters sit in.
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

/// Translates a stylo family list into parley's.
///
/// Lives with the paragraph core rather than with the measurement path
/// because the core is the paragraph implementation that survives; the
/// measurement path borrows it back until it goes.
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

pub(in crate::text) fn translate_font_family_name(
    single: &SingleFontFamily,
) -> ParleyFontFamilyName<'_> {
    match single {
        SingleFontFamily::FamilyName(name) => {
            ParleyFontFamilyName::Named(Cow::Borrowed(name.name.as_ref()))
        }
        SingleFontFamily::Generic(generic) => {
            ParleyFontFamilyName::Generic(translate_generic_family(*generic))
        }
    }
}

pub(in crate::text) const fn translate_generic_family(
    value: GenericFontFamily,
) -> ParleyGenericFamily {
    match value {
        GenericFontFamily::None | GenericFontFamily::SansSerif => ParleyGenericFamily::SansSerif,
        GenericFontFamily::Serif => ParleyGenericFamily::Serif,
        GenericFontFamily::Monospace => ParleyGenericFamily::Monospace,
        GenericFontFamily::Cursive => ParleyGenericFamily::Cursive,
        GenericFontFamily::Fantasy => ParleyGenericFamily::Fantasy,
        GenericFontFamily::SystemUi => ParleyGenericFamily::SystemUi,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

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
