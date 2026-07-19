//! Text measurement style protocol.
//!
//! This module is deliberately independent of Parley. It is the stable seam
//! between host-owned computed styles and an optional text measurement
//! adapter: the host lends paragraph and run style views in stylo's computed
//! font/text vocabulary, and the adapter translates their values only when it
//! shapes text. Paint-only values (brushes, color, decoration, shadow, and
//! stroke) are outside this measurement protocol.
//!
//! The legacy single `white-space` value is replaced by the fork's property
//! pair: [`TextContainerStyle::text_wrap_mode`] (wrap vs. nowrap) and
//! [`TextContainerStyle::white_space_collapse`] (collapsing). `text-align:
//! justify` is not parseable under the lynx grammar and has no handling.

use stylo::computed_values::{text_wrap_mode, white_space_collapse};
use stylo::values::computed::{
    FontFamily, FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight, LetterSpacing,
    LineHeight, TextAlign, TextIndent, WordBreak,
};

use crate::style::CoreStyle;

/// Paragraph-level style of a `<text>` leaf box.
///
/// Inline direction comes from [`CoreStyle::direction`]; it is not duplicated
/// here. Defaults are the fork's initial values.
pub trait TextContainerStyle: CoreStyle {
    /// `text-align` (`justify` is not parseable under the lynx grammar).
    fn text_align(&self) -> TextAlign {
        TextAlign::Start
    }

    /// `text-wrap-mode` — whether lines may wrap.
    fn text_wrap_mode(&self) -> text_wrap_mode::T {
        text_wrap_mode::T::Wrap
    }

    /// `white-space-collapse` — how document whitespace collapses before
    /// shaping.
    fn white_space_collapse(&self) -> white_space_collapse::T {
        white_space_collapse::T::Collapse
    }

    /// `word-break`.
    fn word_break(&self) -> WordBreak {
        WordBreak::Normal
    }

    /// `text-indent`. Only the length component participates in
    /// measurement; the `hanging`/`each-line` flags are ignored (documented
    /// vocabulary-swap delta).
    ///
    /// Percentage basis: the containing block's inline size.
    fn text_indent(&self) -> TextIndent {
        TextIndent::zero()
    }
}

/// Shaping style for one host-assembled text run.
///
/// Runs are inline content rather than boxes, so this intentionally does not
/// inherit [`CoreStyle`]. [`font_family`](Self::font_family) is required (the
/// initial family list is host policy); the scalar accessors default to the
/// CSS initial values. Sequence values (`font-family`,
/// `font-feature-settings`, `font-variation-settings`) are stylo's
/// refcounted computed values, cloned per accessor call.
///
/// This trait is intentionally not object-safe; text measurement stays
/// statically dispatched.
///
/// ```compile_fail
/// use neutron_star::style::TextRunStyle;
/// fn erased(_: &dyn TextRunStyle) {}
/// ```
pub trait TextRunStyle: Sized {
    /// The computed `font-family` fallback list.
    fn font_family(&self) -> FontFamily;

    /// Computed font size in CSS pixels.
    fn font_size(&self) -> f32 {
        16.0
    }

    /// Computed `font-weight`.
    fn font_weight(&self) -> FontWeight {
        FontWeight::NORMAL
    }

    /// Computed `font-style`.
    fn font_style(&self) -> FontStyle {
        FontStyle::NORMAL
    }

    /// Computed `letter-spacing` (`normal` behaves as `0`; the lynx grammar
    /// is length-only).
    fn letter_spacing(&self) -> LetterSpacing {
        LetterSpacing::normal()
    }

    /// Computed `line-height` (a computed `Length` is already absolute CSS
    /// pixels).
    fn line_height(&self) -> LineHeight {
        LineHeight::normal()
    }

    /// Computed `font-feature-settings` entries.
    fn font_feature_settings(&self) -> FontFeatureSettings {
        FontFeatureSettings::normal()
    }

    /// Computed `font-variation-settings` entries.
    fn font_variation_settings(&self) -> FontVariationSettings {
        FontVariationSettings::normal()
    }
}

/// One borrowed text/style run assembled by the host.
///
/// A paragraph is a sequence of these values, allowing mixed styles without
/// flattening them into a single materialized style. `preserve_newlines`
/// models `<raw-text>`'s hard-coded `white-space-collapse: preserve-breaks`
/// behavior independently of the container's
/// [`white_space_collapse`](TextContainerStyle::white_space_collapse) value.
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

    fn text_wrap_mode(&self) -> text_wrap_mode::T {
        (**self).text_wrap_mode()
    }

    fn white_space_collapse(&self) -> white_space_collapse::T {
        (**self).white_space_collapse()
    }

    fn word_break(&self) -> WordBreak {
        (**self).word_break()
    }

    fn text_indent(&self) -> TextIndent {
        (**self).text_indent()
    }
}

impl<S: TextRunStyle> TextRunStyle for &S {
    fn font_family(&self) -> FontFamily {
        (**self).font_family()
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

    fn letter_spacing(&self) -> LetterSpacing {
        (**self).letter_spacing()
    }

    fn line_height(&self) -> LineHeight {
        (**self).line_height()
    }

    fn font_feature_settings(&self) -> FontFeatureSettings {
        (**self).font_feature_settings()
    }

    fn font_variation_settings(&self) -> FontVariationSettings {
        (**self).font_variation_settings()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use stylo::Zero;
    use stylo::values::computed::Display;
    use stylo::values::computed::font::{GenericFontFamily, SingleFontFamily};

    use super::*;

    #[derive(Debug)]
    struct Defaults;

    impl CoreStyle for Defaults {
        fn display(&self) -> Display {
            Display::Flex
        }
    }
    impl TextContainerStyle for Defaults {}

    impl TextRunStyle for Defaults {
        fn font_family(&self) -> FontFamily {
            FontFamily::generic(GenericFontFamily::SansSerif).clone()
        }
    }

    #[test]
    fn text_defaults_match_fork_initial_values() {
        let style = Defaults;

        assert_eq!(style.text_align(), TextAlign::Start);
        assert_eq!(style.text_wrap_mode(), text_wrap_mode::T::Wrap);
        assert_eq!(
            style.white_space_collapse(),
            white_space_collapse::T::Collapse
        );
        assert_eq!(style.word_break(), WordBreak::Normal);
        assert!(style.text_indent().length.is_zero());
        assert_eq!(style.font_size(), 16.0);
        assert_eq!(style.font_weight(), FontWeight::NORMAL);
        assert_eq!(style.font_style(), FontStyle::NORMAL);
        assert_eq!(style.letter_spacing(), LetterSpacing::normal());
        assert_eq!(style.line_height(), LineHeight::normal());
        assert_eq!(style.font_feature_settings(), FontFeatureSettings::normal());
        assert_eq!(
            style.font_variation_settings(),
            FontVariationSettings::normal()
        );
    }

    #[test]
    fn reference_forwarding_preserves_run_values_and_copy_runs() {
        fn assert_copy<T: Copy>() {}

        let style = Defaults;
        let view = &style;
        assert_eq!(TextRunStyle::font_size(&view), 16.0);
        assert!(matches!(
            TextRunStyle::font_family(&view).families.list.first(),
            Some(SingleFontFamily::Generic(GenericFontFamily::SansSerif))
        ));
        assert_eq!(
            TextContainerStyle::text_wrap_mode(&view),
            text_wrap_mode::T::Wrap
        );

        let run = TextRun {
            text: "hello",
            style: view,
            preserve_newlines: true,
        };
        assert_eq!(run.text, "hello");
        assert!(run.preserve_newlines);
        assert_copy::<TextRun<'_, Defaults>>();
    }
}
