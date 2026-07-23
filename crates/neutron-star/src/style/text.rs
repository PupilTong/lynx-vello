//! Text measurement style protocol.

use stylo::computed_values::{text_wrap_mode, white_space_collapse};
use stylo::values::computed::{
    FontFamily, FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight, LetterSpacing,
    LineHeight, TextAlign, TextIndent, WordBreak,
};

use crate::style::CoreStyle;

pub trait TextContainerStyle: CoreStyle {
    fn text_align(&self) -> TextAlign {
        TextAlign::Start
    }

    fn text_wrap_mode(&self) -> text_wrap_mode::T {
        text_wrap_mode::T::Wrap
    }

    fn white_space_collapse(&self) -> white_space_collapse::T {
        white_space_collapse::T::Collapse
    }

    fn word_break(&self) -> WordBreak {
        WordBreak::Normal
    }

    fn text_indent(&self) -> TextIndent {
        TextIndent::zero()
    }
}

pub trait TextRunStyle: Sized {
    fn font_family(&self) -> FontFamily;

    fn font_size(&self) -> f32 {
        16.0
    }

    fn font_weight(&self) -> FontWeight {
        FontWeight::NORMAL
    }

    fn font_style(&self) -> FontStyle {
        FontStyle::NORMAL
    }

    fn letter_spacing(&self) -> LetterSpacing {
        LetterSpacing::normal()
    }

    fn line_height(&self) -> LineHeight {
        LineHeight::normal()
    }

    fn font_feature_settings(&self) -> FontFeatureSettings {
        FontFeatureSettings::normal()
    }

    fn font_variation_settings(&self) -> FontVariationSettings {
        FontVariationSettings::normal()
    }
}

/// One borrowed text/style run assembled by the host.
#[derive(Debug)]
pub struct TextRun<'a, R: TextRunStyle> {
    pub text: &'a str,
    pub style: &'a R,
    pub preserve_newlines: bool,
}

impl<R: TextRunStyle> Clone for TextRun<'_, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R: TextRunStyle> Copy for TextRun<'_, R> {}

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
        assert_eq!(TextContainerStyle::text_align(&view), TextAlign::Start);
        assert_eq!(
            TextContainerStyle::white_space_collapse(&view),
            white_space_collapse::T::Collapse
        );
        assert_eq!(TextContainerStyle::word_break(&view), WordBreak::Normal);
        assert!(TextContainerStyle::text_indent(&view).length.is_zero());
        assert_eq!(TextRunStyle::font_weight(&view), FontWeight::NORMAL);
        assert_eq!(TextRunStyle::font_style(&view), FontStyle::NORMAL);
        assert_eq!(TextRunStyle::letter_spacing(&view), LetterSpacing::normal());
        assert_eq!(TextRunStyle::line_height(&view), LineHeight::normal());
        assert_eq!(
            TextRunStyle::font_feature_settings(&view),
            FontFeatureSettings::normal()
        );
        assert_eq!(
            TextRunStyle::font_variation_settings(&view),
            FontVariationSettings::normal()
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
