//! Paragraph-wide parameters of one Lynx text block.
//!
//! The run-level vocabulary is the paragraph core's
//! ([`crate::text::paragraph`]); what is added here is what only a Lynx
//! `<text>` has: the `text-maxline` and `text-maxlength` limits, which arrive
//! through [`LynxTextStyle`](crate::style::LynxTextStyle), the `text-overflow` value
//! the truncation marker is gated on, and the `overflow-wrap` the block always
//! breaks with. Custom inline-truncation is content, not a parameter.

use core::num::NonZeroU32;

use crate::text::paragraph::content::WhiteSpaceProcessing;
use crate::text::paragraph::{
    BreakStyle, Direction, IndentKeywords, OverflowWrap, RunStyle, TextAlign, TextWrap, WordBreak,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockStyle {
    pub text_align: TextAlign,
    pub direction: Direction,
    pub text_wrap: TextWrap,
    pub word_break: WordBreak,
    pub overflow: TextOverflow,
    pub max_lines: Option<NonZeroU32>,
    pub max_chars: Option<u32>,
    pub strut: Option<RunStyle>,
    pub strut_line_when_empty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextOverflow {
    #[default]
    Clip,
    Ellipsis,
}

impl BlockStyle {
    pub(in crate::text) fn processing(&self, content: bool) -> WhiteSpaceProcessing {
        WhiteSpaceProcessing::Verbatim {
            first_line_only: content && self.text_wrap == TextWrap::NoWrap,
        }
    }

    pub(in crate::text) const INDENT_KEYWORDS: IndentKeywords = IndentKeywords {
        each_line: true,
        hanging: false,
    };

    pub(in crate::text) const fn breaking(&self) -> BreakStyle {
        BreakStyle {
            word_break: self.word_break,
            text_wrap: self.text_wrap,
            overflow_wrap: OverflowWrap::BreakWord,
        }
    }

    pub(in crate::text) fn shapes_as(&self, built: &Self, built_strut: Option<&RunStyle>) -> bool {
        self.text_wrap == built.text_wrap
            && self.word_break == built.word_break
            && self.strut_line_when_empty == built.strut_line_when_empty
            && match &self.strut {
                Some(strut) => built_strut == Some(strut),
                None if self.strut_line_when_empty => {
                    built_strut.is_some_and(|strut| *strut == RunStyle::default())
                }
                None => built_strut.is_none(),
            }
    }
}

impl BlockStyle {
    #[must_use]
    pub fn from_container_style<S: crate::style::LynxTextStyle, R: crate::style::TextRunStyle>(
        style: &S,
        own_font: &R,
    ) -> Self {
        use stylo::values::specified::text::TextOverflowSide;

        Self {
            text_align: style.text_align().into(),
            direction: style.direction().into(),
            text_wrap: wrap_mode(style.text_wrap_mode(), style.white_space_collapse()),
            word_break: style.word_break().into(),
            overflow: match style.text_overflow().second {
                TextOverflowSide::Clip => TextOverflow::Clip,
                TextOverflowSide::Ellipsis => TextOverflow::Ellipsis,
            },
            max_lines: style.text_maxline(),
            max_chars: style.text_maxlength(),
            strut: Some(RunStyle::from_run_style(own_font)),
            strut_line_when_empty: false,
        }
    }
}

fn wrap_mode(
    mode: stylo::computed_values::text_wrap_mode::T,
    collapse: stylo::computed_values::white_space_collapse::T,
) -> TextWrap {
    use stylo::computed_values::white_space_collapse::T as Collapse;

    let wrap = TextWrap::from(mode);
    if wrap == TextWrap::NoWrap && collapse != Collapse::Collapse {
        TextWrap::Wrap
    } else {
        wrap
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use parley::{
        FontStyle as ParleyFontStyle, LineHeight as ParleyLineHeight,
        OverflowWrap as ParleyOverflowWrap, TextWrapMode as ParleyTextWrapMode,
        WordBreak as ParleyWordBreak,
    };
    use stylo::Atom;
    use stylo::values::computed::font::{
        FamilyName, FontFamilyList, FontFamilyNameSyntax, GenericFontFamily, SingleFontFamily,
    };
    use stylo::values::computed::{FontFamily, FontStyle, FontWeight};

    use super::*;
    use crate::text::paragraph::style::parley_style;
    use crate::text::paragraph::{LineHeight, RunStyle, VerticalAlign};

    #[test]
    fn a_block_wraps_unless_white_space_is_nowrap() {
        use stylo::computed_values::text_wrap_mode::T as Mode;
        use stylo::computed_values::white_space_collapse::T as Collapse;

        for (keyword, mode, collapse, expected) in [
            ("normal", Mode::Wrap, Collapse::Collapse, TextWrap::Wrap),
            ("nowrap", Mode::Nowrap, Collapse::Collapse, TextWrap::NoWrap),
            ("pre", Mode::Nowrap, Collapse::Preserve, TextWrap::Wrap),
            ("pre-wrap", Mode::Wrap, Collapse::Preserve, TextWrap::Wrap),
            (
                "pre-line",
                Mode::Wrap,
                Collapse::PreserveBreaks,
                TextWrap::Wrap,
            ),
            (
                "break-spaces",
                Mode::Wrap,
                Collapse::BreakSpaces,
                TextWrap::Wrap,
            ),
        ] {
            assert_eq!(wrap_mode(mode, collapse), expected, "{keyword}");
        }
    }

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
        assert!(!style.strut_line_when_empty);
        assert_eq!(
            style.breaking(),
            BreakStyle {
                word_break: WordBreak::Normal,
                text_wrap: TextWrap::Wrap,
                overflow_wrap: OverflowWrap::BreakWord,
            },
        );

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

        let style = parley_style(&run, block.breaking());
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
            parley_style(&number, BlockStyle::default().breaking()).line_height,
            ParleyLineHeight::FontSizeRelative(1.5),
        );
        assert_eq!(
            parley_style(&RunStyle::default(), BlockStyle::default().breaking()).line_height,
            ParleyLineHeight::MetricsRelative(1.0),
        );
        assert_eq!(
            parley_style(
                &RunStyle::default(),
                BlockStyle {
                    word_break: WordBreak::BreakAll,
                    ..BlockStyle::default()
                }
                .breaking()
            )
            .word_break,
            ParleyWordBreak::BreakAll,
        );
    }
}
