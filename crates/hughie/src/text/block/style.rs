//! Parameter vocabulary of one Lynx text block.
//!
//! Plain resolved-value structs instead of the box-protocol wire format or a
//! trait over `ComputedValues`: the block's defining capabilities
//! (`text-maxline`, `text-maxlength`, inline-truncation content) are element
//! attributes with no home in a computed-style hook, and the Lynx grammar is
//! narrower than stylo's — `white-space` is only `normal | nowrap`, there is
//! no `word-spacing`, no `pre*` — so two-value enums make the unsupported
//! states unrepresentable instead of mapped and ignored. Font values stay
//! stylo types: they are already hughie public API, cheap to own, and their
//! parley translation is shared with the measurement path.

use core::num::NonZeroU32;
use std::borrow::Cow;

use parley::setting::Tag;
use parley::{
    Alignment, FontFamily as ParleyFontFamily, FontFamilyName as ParleyFontFamilyName, FontFeature,
    FontFeatures, FontStyle as ParleyFontStyle, FontVariation, FontVariations,
    FontWeight as ParleyFontWeight, GenericFamily as ParleyGenericFamily,
    LineHeight as ParleyLineHeight, OverflowWrap as ParleyOverflowWrap,
    TextStyle as ParleyTextStyle, TextWrapMode as ParleyTextWrapMode, WordBreak as ParleyWordBreak,
};
use stylo::values::computed::font::{GenericFontFamily, SingleFontFamily};
use stylo::values::computed::{
    FontFamily, FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight,
};

use crate::style::TextBrush;

/// Container-level parameters of one Lynx text block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockStyle {
    pub text_align: TextAlign,
    /// Resolves `Start`/`End` only. Lynx forwards no direction to its
    /// paragraph engine — bidi comes from the text itself — so the declared
    /// direction decides alignment and nothing else.
    pub direction: Direction,
    /// `white-space: normal | nowrap`. Collapsing is always on (Lynx has no
    /// `pre*` values); newline preservation is per-run for raw-text.
    pub text_wrap: TextWrap,
    pub word_break: WordBreak,
    pub overflow: TextOverflow,
    /// The `text-maxline` attribute. Lynx stores `optional<int>` and treats
    /// non-positive values as absent, which `NonZeroU32` makes unrepresentable.
    pub max_lines: Option<NonZeroU32>,
    /// The `text-maxlength` attribute, in UTF-16 source units where an atomic
    /// box counts exactly one. Zero is meaningful: everything is cut.
    pub max_chars: Option<u32>,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WordBreak {
    #[default]
    Normal,
    BreakAll,
    KeepAll,
}

/// The `text-indent` value, as a caller resolves it.
///
/// Deliberately not a [`BlockStyle`] field: a percentage resolves against the
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextOverflow {
    #[default]
    Clip,
    Ellipsis,
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
pub(in crate::text::block) fn resolve_alignment(
    text_align: TextAlign,
    direction: Direction,
) -> Alignment {
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

/// Translates one run into the parley style the shaper consumes.
///
/// `word_break` and `text_wrap` are container values pushed onto every run
/// because that is where parley keeps them; `overflow_wrap` is hardcoded
/// `BreakWord` because the web target hardcodes `overflow-wrap: break-word`
/// on `x-text` rather than exposing it as a style.
pub(in crate::text::block) fn parley_style<'style>(
    run: &'style RunStyle,
    block: &BlockStyle,
) -> ParleyTextStyle<'style, 'static, TextBrush> {
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
        word_break: match block.word_break {
            WordBreak::Normal => ParleyWordBreak::Normal,
            WordBreak::BreakAll => ParleyWordBreak::BreakAll,
            WordBreak::KeepAll => ParleyWordBreak::KeepAll,
        },
        overflow_wrap: ParleyOverflowWrap::BreakWord,
        text_wrap_mode: match block.text_wrap {
            TextWrap::Wrap => ParleyTextWrapMode::Wrap,
            TextWrap::NoWrap => ParleyTextWrapMode::NoWrap,
        },
        ..ParleyTextStyle::default()
    }
}

/// Builds the paragraph parameters from a container's computed style.
///
/// `max_lines` / `max_chars` stay `None`: they are element attributes with no
/// computed-style home, so a host that has them supplies them itself.
/// `text_indent` is absent by design — it is a [`BlockConstraint`] input,
/// because a percentage resolves against the definite inline size.
///
/// [`BlockConstraint`]: super::BlockConstraint
impl BlockStyle {
    #[must_use]
    pub fn from_container_style<S: crate::style::TextContainerStyle>(style: &S) -> Self {
        use stylo::computed_values::direction;
        use stylo::computed_values::text_wrap_mode::T as WrapMode;
        use stylo::values::computed::TextAlign as StyloAlign;
        use stylo::values::specified::text::TextOverflowSide;

        Self {
            text_align: match style.text_align() {
                StyloAlign::Start => TextAlign::Start,
                StyloAlign::End => TextAlign::End,
                StyloAlign::Left => TextAlign::Left,
                StyloAlign::Right => TextAlign::Right,
                StyloAlign::Center => TextAlign::Center,
            },
            direction: match style.direction() {
                direction::T::Rtl => Direction::Rtl,
                direction::T::Ltr => Direction::Ltr,
            },
            text_wrap: match style.text_wrap_mode() {
                WrapMode::Wrap => TextWrap::Wrap,
                WrapMode::Nowrap => TextWrap::NoWrap,
            },
            word_break: match style.word_break() {
                stylo::values::computed::WordBreak::BreakAll => WordBreak::BreakAll,
                stylo::values::computed::WordBreak::KeepAll => WordBreak::KeepAll,
                stylo::values::computed::WordBreak::Normal => WordBreak::Normal,
            },
            overflow: match style.text_overflow().second {
                TextOverflowSide::Clip => TextOverflow::Clip,
                TextOverflowSide::Ellipsis => TextOverflow::Ellipsis,
            },
            max_lines: None,
            max_chars: None,
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
/// Lives with the block rather than with the measurement path because the
/// block is the paragraph implementation that survives; the measurement path
/// borrows it back until it goes.
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

    use stylo::Atom;
    use stylo::values::computed::font::{
        FamilyName, FontFamilyList, FontFamilyNameSyntax, SingleFontFamily,
    };
    use stylo::values::generics::font::{FeatureTagValue, FontSettings, FontTag, VariationValue};

    use super::*;

    #[test]
    fn block_defaults_are_the_lynx_initial_values() {
        let style = BlockStyle::default();
        assert_eq!(style.text_align, TextAlign::Start);
        assert_eq!(style.direction, Direction::Ltr);
        assert_eq!(style.text_wrap, TextWrap::Wrap);
        assert_eq!(style.word_break, WordBreak::Normal);
        assert_eq!(style.overflow, TextOverflow::Clip);
        assert_eq!(style.max_lines, None);
        assert_eq!(style.max_chars, None);

        let run = RunStyle::default();
        assert_eq!(run.font_size, 16.0);
        assert_eq!(run.font_weight, FontWeight::NORMAL);
        assert_eq!(run.font_style, FontStyle::NORMAL);
        assert_eq!(run.letter_spacing, 0.0);
        assert_eq!(run.line_height, LineHeight::Normal);
        assert!(matches!(
            run.font_family.families.list.first(),
            Some(SingleFontFamily::Generic(GenericFontFamily::SansSerif))
        ));
        assert_eq!(VerticalAlign::default(), VerticalAlign::Baseline);
    }

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

    fn named_family(name: &str) -> FontFamily {
        FontFamily {
            families: FontFamilyList {
                list: stylo::ArcSlice::from_iter(std::iter::once(SingleFontFamily::FamilyName(
                    FamilyName {
                        name: Atom::from(name),
                        syntax: FontFamilyNameSyntax::Identifiers,
                    },
                ))),
            },
            is_system_font: false,
            is_initial: false,
        }
    }

    #[test]
    fn run_translation_covers_every_line_height_and_wrap_arm() {
        let block = BlockStyle {
            word_break: WordBreak::KeepAll,
            text_wrap: TextWrap::NoWrap,
            ..BlockStyle::default()
        };
        let run = RunStyle {
            font_family: named_family("Ahem"),
            font_size: 10.0,
            letter_spacing: 2.0,
            line_height: LineHeight::Px(14.0),
            ..RunStyle::default()
        };

        let style = parley_style(&run, &block);
        assert_eq!(style.font_size, 10.0);
        assert_eq!(style.letter_spacing, 2.0);
        assert_eq!(style.line_height, ParleyLineHeight::Absolute(14.0));
        assert_eq!(style.word_break, ParleyWordBreak::KeepAll);
        assert_eq!(style.text_wrap_mode, ParleyTextWrapMode::NoWrap);
        assert_eq!(style.overflow_wrap, ParleyOverflowWrap::BreakWord);
        assert!(matches!(style.font_style, ParleyFontStyle::Normal));

        let number = RunStyle {
            line_height: LineHeight::Number(1.5),
            ..RunStyle::default()
        };
        assert_eq!(
            parley_style(&number, &BlockStyle::default()).line_height,
            ParleyLineHeight::FontSizeRelative(1.5),
        );
        assert_eq!(
            parley_style(&RunStyle::default(), &BlockStyle::default()).line_height,
            ParleyLineHeight::MetricsRelative(1.0),
        );
        assert_eq!(
            parley_style(
                &RunStyle::default(),
                &BlockStyle {
                    word_break: WordBreak::BreakAll,
                    ..BlockStyle::default()
                }
            )
            .word_break,
            ParleyWordBreak::BreakAll,
        );
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

        let style = parley_style(&run, &BlockStyle::default());
        assert!(matches!(style.font_style, ParleyFontStyle::Italic));
        assert_eq!(style.font_weight, ParleyFontWeight::new(700.0));
        assert!(matches!(&style.font_features, FontFeatures::List(list) if list.len() == 1));
        assert!(matches!(&style.font_variations, FontVariations::List(list) if list.len() == 1));

        let oblique = RunStyle {
            font_style: FontStyle::oblique(30.0),
            ..RunStyle::default()
        };
        assert!(matches!(
            parley_style(&oblique, &BlockStyle::default()).font_style,
            ParleyFontStyle::Oblique(Some(degrees)) if degrees == 30.0,
        ));
    }
}
