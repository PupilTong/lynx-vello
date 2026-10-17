//! Paragraph-wide parameters of one Lynx text block.
//!
//! The run-level vocabulary is the paragraph core's
//! ([`crate::text::paragraph`]); what is added here is what only a Lynx
//! `<text>` has: the `text-maxline` and `text-maxlength` limits and the
//! verbatim white-space switch, which arrive through
//! [`LynxTextStyle`](crate::style::LynxTextStyle), the `text-overflow` value
//! the truncation marker is gated on, and the `overflow-wrap` the block always
//! breaks with. Custom inline-truncation is content, not a parameter.

use core::num::NonZeroU32;

use crate::text::paragraph::content::WhiteSpaceProcessing;
use crate::text::paragraph::{
    BreakStyle, Direction, IndentKeywords, OverflowWrap, RunStyle, TextAlign, TextWrap, WordBreak,
};

/// Container-level parameters of one Lynx text block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockStyle {
    pub text_align: TextAlign,
    /// Resolves `Start`/`End` only. Lynx forwards no direction to its
    /// paragraph engine — bidi comes from the text itself — so the declared
    /// direction decides alignment and nothing else.
    pub direction: Direction,
    /// `text-wrap-mode` — except that a verbatim block takes `white-space:
    /// pre` as native takes it, as wrapping ([`Self::from_container_style`]
    /// decides that, in `wrap_mode`). Whitespace collapsing is a per-run value
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
    /// Whether the paragraph keeps its text verbatim, as every native Lynx
    /// backend does for a `<text>`: no run's `white-space-collapse` is read,
    /// every space, tab and newline is kept, tabs stop every 20px, a newline
    /// that ends the text adds an empty last line, and under
    /// `text_wrap: NoWrap` nothing after the first newline is shown. Off, the
    /// runs are processed by css-text-3, which is what a `display: -lynx-text`
    /// paragraph that is no `<text>` — a `<markdown>` shadow paragraph —
    /// needs.
    pub verbatim: bool,
    /// The establishing element's own font and `line-height`: the root
    /// inline box's strut, which every line box starts with (CSS 2.2
    /// §10.8.1), as the browser starts web-core's `x-text` lines with the
    /// `x-text`'s own font and Android's `StaticLayout` with the text's base
    /// paint. `None` leaves the strut to the first content run, which is
    /// what a paragraph built without a host has.
    pub strut: Option<RunStyle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextOverflow {
    #[default]
    Clip,
    Ellipsis,
}

impl BlockStyle {
    /// How the block's white space is processed: verbatim, as native Lynx
    /// processes a `<text>`'s, or css-text-3's. Only the content flow takes
    /// native's `nowrap` first-line cut; the truncation content is laid in at
    /// a cut of its own.
    pub(in crate::text) fn processing(&self, content: bool) -> WhiteSpaceProcessing {
        if self.verbatim {
            WhiteSpaceProcessing::Verbatim {
                first_line_only: content && self.text_wrap == TextWrap::NoWrap,
            }
        } else {
            WhiteSpaceProcessing::CssKeepingLeadingSpace
        }
    }

    /// Which lines the block's `text-indent` goes on.
    ///
    /// A verbatim block indents the first line and every line that starts
    /// after a newline — css-text-3's `each-line` — as native Lynx does: it
    /// applies `text-indent` as an Android `LeadingMarginSpan.Standard(indent,
    /// 0)` over the whole text
    /// (`lynx/platform/android/lynx_android/src/main/java/com/lynx/tasm/behavior/shadow/text/
    /// TextShadowNode.java:295-298`), and Android's `Layout` gives that first-line margin to
    /// the first line of every paragraph, which is every line after a `\n`. Soft-wrapped lines
    /// are not indented. A CSS-processed block indents its first line only.
    pub(in crate::text) const fn indent_keywords(&self) -> IndentKeywords {
        IndentKeywords {
            each_line: self.verbatim,
            hanging: false,
        }
    }

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
/// establishing element's computed custom properties alongside paragraph CSS;
/// `own_font` is the same element's computed values seen as a run, which the
/// strut is built from.
/// `text_indent` is absent by design — it is a [`BlockConstraint`] input,
/// because a percentage resolves against the definite inline size.
///
/// [`BlockConstraint`]: super::BlockConstraint
impl BlockStyle {
    #[must_use]
    pub fn from_container_style<S: crate::style::LynxTextStyle, R: crate::style::TextRunStyle>(
        style: &S,
        own_font: &R,
    ) -> Self {
        use stylo::values::specified::text::TextOverflowSide;

        let verbatim = style.verbatim_white_space();
        Self {
            text_align: style.text_align().into(),
            direction: style.direction().into(),
            text_wrap: wrap_mode(
                verbatim,
                style.text_wrap_mode(),
                style.white_space_collapse(),
            ),
            word_break: style.word_break().into(),
            overflow: match style.text_overflow().second {
                TextOverflowSide::Clip => TextOverflow::Clip,
                TextOverflowSide::Ellipsis => TextOverflow::Ellipsis,
            },
            max_lines: style.text_maxline(),
            max_chars: style.text_maxlength(),
            verbatim,
            strut: Some(RunStyle::from_run_style(own_font)),
        }
    }
}

/// The block's wrap mode, from the container's two `white-space` longhands.
///
/// A CSS paragraph takes `text-wrap-mode` as it is. A verbatim paragraph —
/// a `<text>` — reads `white-space` the way native Lynx parses it, which
/// knows only `normal` and `nowrap` and drops every other keyword
/// (`lynx/core/renderer/css/parser/enum_handler.cc:76-89`): so only `nowrap`
/// keeps its line from wrapping and cuts it at the first newline. Of the
/// shorthand's keywords, `pre` is the other one that computes
/// `text-wrap-mode: nowrap`, and it is told apart by its
/// `white-space-collapse: preserve`: any `white-space-collapse` other than
/// `collapse` beside `nowrap` means the author did not write native's
/// `nowrap`, and the paragraph wraps. `pre-wrap`, `pre-line` and
/// `break-spaces` compute `text-wrap-mode: wrap` already, which is what
/// native's dropped keyword leaves too. The collapse half itself is never
/// read by a verbatim paragraph: its text keeps every space and newline
/// whatever the keyword says.
fn wrap_mode(
    verbatim: bool,
    mode: stylo::computed_values::text_wrap_mode::T,
    collapse: stylo::computed_values::white_space_collapse::T,
) -> TextWrap {
    use stylo::computed_values::white_space_collapse::T as Collapse;

    let wrap = TextWrap::from(mode);
    if verbatim && wrap == TextWrap::NoWrap && collapse != Collapse::Collapse {
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

    /// Every `white-space` keyword's two longhands, verbatim and not.
    #[test]
    fn a_verbatim_block_wraps_unless_white_space_is_nowrap() {
        use stylo::computed_values::text_wrap_mode::T as Mode;
        use stylo::computed_values::white_space_collapse::T as Collapse;

        for (keyword, mode, collapse, verbatim, css) in [
            (
                "normal",
                Mode::Wrap,
                Collapse::Collapse,
                TextWrap::Wrap,
                TextWrap::Wrap,
            ),
            (
                "nowrap",
                Mode::Nowrap,
                Collapse::Collapse,
                TextWrap::NoWrap,
                TextWrap::NoWrap,
            ),
            (
                "pre",
                Mode::Nowrap,
                Collapse::Preserve,
                TextWrap::Wrap,
                TextWrap::NoWrap,
            ),
            (
                "pre-wrap",
                Mode::Wrap,
                Collapse::Preserve,
                TextWrap::Wrap,
                TextWrap::Wrap,
            ),
            (
                "pre-line",
                Mode::Wrap,
                Collapse::PreserveBreaks,
                TextWrap::Wrap,
                TextWrap::Wrap,
            ),
            (
                "break-spaces",
                Mode::Wrap,
                Collapse::BreakSpaces,
                TextWrap::Wrap,
                TextWrap::Wrap,
            ),
        ] {
            assert_eq!(
                wrap_mode(true, mode, collapse),
                verbatim,
                "{keyword}, verbatim"
            );
            assert_eq!(wrap_mode(false, mode, collapse), css, "{keyword}, css");
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
        assert!(!style.verbatim);
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
