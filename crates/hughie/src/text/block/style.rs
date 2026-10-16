//! Paragraph-wide parameters of one Lynx text block.
//!
//! The run-level vocabulary is the paragraph core's
//! ([`crate::text::paragraph`]); what is added here is what only a Lynx
//! `<text>` has: the `text-maxline` and `text-maxlength` limits, which arrive
//! through [`LynxTextStyle`](crate::style::LynxTextStyle), the
//! `text-overflow` value the truncation marker is gated on, and the
//! `overflow-wrap` the block always breaks with. Custom inline-truncation is
//! content, not a parameter.

use core::num::NonZeroU32;

use crate::text::paragraph::{BreakStyle, Direction, OverflowWrap, TextAlign, TextWrap, WordBreak};

/// Container-level parameters of one Lynx text block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockStyle {
    pub text_align: TextAlign,
    /// Resolves `Start`/`End` only. Lynx forwards no direction to its
    /// paragraph engine — bidi comes from the text itself — so the declared
    /// direction decides alignment and nothing else.
    pub direction: Direction,
    /// `text-wrap-mode`. Whitespace collapsing is a per-run value
    /// ([`WhiteSpaceCollapse`](super::WhiteSpaceCollapse) on each
    /// [`TextRunItem`](super::TextRunItem)),
    /// because `white-space-collapse` inherits into nested text.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextOverflow {
    #[default]
    Clip,
    Ellipsis,
}

impl BlockStyle {
    /// The line-breaking values every run of the block is shaped with.
    ///
    /// `word_break` and `text_wrap` are container values pushed onto every run
    /// because that is where parley keeps them; `overflow_wrap` is hardcoded
    /// `BreakWord` because the web target hardcodes `overflow-wrap: break-word`
    /// on `x-text` rather than exposing it as a style.
    pub(in crate::text) const fn breaking(&self) -> BreakStyle {
        BreakStyle {
            word_break: self.word_break,
            text_wrap: self.text_wrap,
            overflow_wrap: OverflowWrap::BreakWord,
        }
    }
}

/// Builds the paragraph parameters from a container's computed style.
///
/// Paragraph limits come from `LynxTextStyle`, whose host view reads the
/// establishing element's computed custom properties alongside paragraph CSS.
/// `text_indent` is absent by design — it is a [`BlockConstraint`] input,
/// because a percentage resolves against the definite inline size.
///
/// [`BlockConstraint`]: super::BlockConstraint
impl BlockStyle {
    #[must_use]
    pub fn from_container_style<S: crate::style::LynxTextStyle>(style: &S) -> Self {
        use stylo::values::specified::text::TextOverflowSide;

        Self {
            text_align: style.text_align().into(),
            direction: style.direction().into(),
            text_wrap: style.text_wrap_mode().into(),
            word_break: style.word_break().into(),
            overflow: match style.text_overflow().second {
                TextOverflowSide::Clip => TextOverflow::Clip,
                TextOverflowSide::Ellipsis => TextOverflow::Ellipsis,
            },
            max_lines: style.text_maxline(),
            max_chars: style.text_maxlength(),
        }
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
    fn block_defaults_are_the_lynx_initial_values() {
        let style = BlockStyle::default();
        assert_eq!(style.text_align, TextAlign::Start);
        assert_eq!(style.direction, Direction::Ltr);
        assert_eq!(style.text_wrap, TextWrap::Wrap);
        assert_eq!(style.word_break, WordBreak::Normal);
        assert_eq!(style.overflow, TextOverflow::Clip);
        assert_eq!(style.max_lines, None);
        assert_eq!(style.max_chars, None);
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
