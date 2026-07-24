//! Text measurement style protocol.

use stylo::computed_values::{text_wrap_mode, white_space_collapse};
use stylo::properties::ComputedValues;
use stylo::values::computed::{
    FontFamily, FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight, LetterSpacing,
    LineHeight, TextAlign, TextIndent, WordBreak,
};

use crate::style::{CoreStyle, initial_values};

style_protocol! {
    pub trait TextContainerStyle: CoreStyle {
        defaults(style) {
            text_align -> TextAlign =
                style.inherited_values().get_inherited_text().clone_text_align(),
            text_wrap_mode -> text_wrap_mode::T =
                style.inherited_values().get_inherited_text().clone_text_wrap_mode(),
            white_space_collapse -> white_space_collapse::T =
                style.inherited_values().get_inherited_text().clone_white_space_collapse(),
            word_break -> WordBreak =
                style.inherited_values().get_inherited_text().clone_word_break(),
            text_indent -> TextIndent =
                style.inherited_values().get_inherited_text().clone_text_indent(),
        }
    }
}

style_protocol! {
    pub trait TextRunStyle: Sized {
        defaults(style) {
            computed_text_values -> Option<&ComputedValues> = None,
            font_family -> FontFamily = style
                .computed_text_values()
                .map_or_else(
                    || initial_values().get_font().clone_font_family(),
                    |values| values.get_font().clone_font_family(),
                ),
            font_family_ref -> Option<&FontFamily> = style
                .computed_text_values()
                .map(|values| &values.get_font().font_family),
            font_size -> f32 = style.computed_text_values().map_or(16.0, |values| {
                values.get_font().clone_font_size().computed_size().px()
            }),
            font_weight -> FontWeight = style
                .computed_text_values()
                .map_or(FontWeight::NORMAL, |values| values.get_font().clone_font_weight()),
            font_style -> FontStyle = style
                .computed_text_values()
                .map_or(FontStyle::NORMAL, |values| values.get_font().clone_font_style()),
            letter_spacing -> LetterSpacing = style.computed_text_values().map_or_else(
                LetterSpacing::normal,
                |values| values.get_inherited_text().clone_letter_spacing(),
            ),
            line_height -> LineHeight = style.computed_text_values().map_or_else(
                LineHeight::normal,
                |values| values.get_font().clone_line_height(),
            ),
            font_feature_settings -> FontFeatureSettings =
                style.computed_text_values().map_or_else(
                    FontFeatureSettings::normal,
                    |values| values.get_font().clone_font_feature_settings(),
                ),
            font_variation_settings -> FontVariationSettings =
                style.computed_text_values().map_or_else(
                    FontVariationSettings::normal,
                    |values| values.get_font().clone_font_variation_settings(),
                ),
        }
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
        assert!(TextRunStyle::font_family_ref(&view).is_none());
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
