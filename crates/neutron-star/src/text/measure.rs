//! Parley-backed implementation of neutron-star's fixed text-content path.

use core::fmt;
use std::borrow::Cow;

use parley::setting::Tag;
use parley::{
    Alignment, CHROMIUM_LINE_BREAK_OVERRIDE, FontFamily as ParleyFontFamily,
    FontFamilyName as ParleyFontFamilyName, FontFeature, FontFeatures,
    FontStyle as ParleyFontStyle, FontVariation, FontVariations, FontWeight as ParleyFontWeight,
    GenericFamily as ParleyGenericFamily, LineHeight as ParleyLineHeight,
    OverflowWrap as ParleyOverflowWrap, TextStyle as ParleyTextStyle,
    TextWrapMode as ParleyTextWrapMode, WordBreak as ParleyWordBreak,
};
use stylo::Zero;
use stylo::computed_values::{direction, text_wrap_mode};
use stylo::values::computed::font::{GenericFontFamily, SingleFontFamily};
use stylo::values::computed::{FontStyle, Length, LineHeight, TextAlign, WordBreak};

use super::content::normalize_runs;
use super::{TextContext, TextLayout, TextLayoutStore, TextMeasurement};
use crate::compute::{LeafMeasureInput, compute_leaf_layout_with_measurement};
use crate::style::{TextContainerStyle, TextRun, TextRunStyle};
use crate::tree::{AvailableSpace, LayoutGoal, LayoutInput, LayoutOutput};

/// Node-scoped Parley adapter for a host-owned paragraph.
pub struct TextMeasurer<'session, 'source, Container, RunStyle, Runs>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Runs: Iterator<Item = TextRun<'source, RunStyle>> + Clone,
{
    context: &'session mut TextContext,
    artifacts: &'session mut TextLayoutStore,
    container_style: &'source Container,
    runs: Runs,
}

impl<'session, 'source, Container, RunStyle, Runs>
    TextMeasurer<'session, 'source, Container, RunStyle, Runs>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Runs: Iterator<Item = TextRun<'source, RunStyle>> + Clone,
{
    pub fn new(
        context: &'session mut TextContext,
        artifacts: &'session mut TextLayoutStore,
        container_style: &'source Container,
        runs: Runs,
    ) -> Self {
        Self {
            context,
            artifacts,
            container_style,
            runs,
        }
    }

    pub fn compute_layout(&mut self, input: LayoutInput) -> LayoutOutput {
        let container_style = self.container_style;
        compute_leaf_layout_with_measurement(input, container_style, None, |measure_input| {
            self.measure(measure_input).metrics()
        })
    }

    fn shape(&mut self) -> TextLayout {
        let content = normalize_runs(
            self.runs.clone(),
            self.container_style.white_space_collapse(),
        );
        #[cfg(test)]
        self.context.record_shape();
        let (font_context, layout_context) = self.context.font_and_layout_contexts();
        let mut builder =
            layout_context.style_run_builder(font_context, content.text.as_str(), 1.0, false);
        let word_break = self.container_style.word_break();
        if word_break != WordBreak::BreakAll {
            builder.set_line_break_override(Some(CHROMIUM_LINE_BREAK_OVERRIDE));
        }
        builder.reserve(content.ranges.len(), content.ranges.len());

        for range in &content.ranges {
            let style = translate_run_style(
                range.style,
                word_break,
                self.container_style.text_wrap_mode(),
            );
            let style_index = builder.push_style(style);
            builder.push_style_run(style_index, range.bytes.clone());
        }

        let has_text = !content.text.is_empty();
        let layout = builder.build(content.text.as_str());
        TextLayout::shaped(layout, has_text)
    }

    fn install_artifact_if_needed(&mut self, goal: LayoutGoal) {
        let missing = match goal {
            LayoutGoal::Measure(_) => self.artifacts.probe.is_none(),
            LayoutGoal::Commit => self.artifacts.committed.is_none(),
        };
        if !missing {
            return;
        }

        let reusable = match goal {
            LayoutGoal::Measure(_) => self.artifacts.committed.clone(),
            LayoutGoal::Commit => self.artifacts.probe.take(),
        };
        let artifact = reusable.unwrap_or_else(|| self.shape());
        match goal {
            LayoutGoal::Measure(_) => self.artifacts.probe = Some(artifact),
            LayoutGoal::Commit => self.artifacts.committed = Some(artifact),
        }
    }
}

impl<'source, Container, RunStyle, Runs> fmt::Debug
    for TextMeasurer<'_, 'source, Container, RunStyle, Runs>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Runs: Iterator<Item = TextRun<'source, RunStyle>> + Clone,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextMeasurer")
            .finish_non_exhaustive()
    }
}

impl<'source, Container, RunStyle, Runs> TextMeasurer<'_, 'source, Container, RunStyle, Runs>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Runs: Iterator<Item = TextRun<'source, RunStyle>> + Clone,
{
    pub fn measure(&mut self, input: LeafMeasureInput) -> TextMeasurement<'_> {
        let inline_basis = definite_inline_size(input).unwrap_or(0.0).max(0.0);
        let indent = self
            .container_style
            .text_indent()
            .length
            .resolve(Length::new(inline_basis))
            .px();
        let alignment = alignment(
            self.container_style.text_align(),
            self.container_style.direction(),
        );

        self.install_artifact_if_needed(input.goal);
        let artifact = match input.goal {
            LayoutGoal::Measure(_) => self
                .artifacts
                .probe
                .as_mut()
                .expect("a probe artifact was installed"),
            LayoutGoal::Commit => self
                .artifacts
                .committed
                .as_mut()
                .expect("a committed artifact was installed"),
        };
        let max_advance = line_break_width(input, artifact);
        artifact.rebreak(max_advance, indent);
        if matches!(input.goal, LayoutGoal::Commit) {
            let measured_width = artifact.size().width;
            if input.known_dimensions.width.is_none()
                && max_advance.is_some_and(|limit| limit > measured_width)
            {
                artifact.rebreak(Some(measured_width), indent);
            }
            artifact.align(alignment);
        }
        TextMeasurement::new(artifact)
    }
}

fn definite_inline_size(input: LeafMeasureInput) -> Option<f32> {
    input
        .known_dimensions
        .width
        .or(match input.available_space.width {
            AvailableSpace::Definite(width) => Some(width),
            AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
        })
}

fn line_break_width(input: LeafMeasureInput, artifact: &TextLayout) -> Option<f32> {
    input.known_dimensions.width.map_or_else(
        || match input.available_space.width {
            AvailableSpace::Definite(width) => Some(width.max(0.0)),
            AvailableSpace::MinContent => Some(artifact.min_content_width().max(0.0)),
            AvailableSpace::MaxContent => None,
        },
        |width| Some(width.max(0.0)),
    )
}

fn alignment(value: TextAlign, direction: direction::T) -> Alignment {
    match (value, direction) {
        (TextAlign::Left, _)
        | (TextAlign::Start, direction::T::Ltr)
        | (TextAlign::End, direction::T::Rtl) => Alignment::Left,
        (TextAlign::Right, _)
        | (TextAlign::Start, direction::T::Rtl)
        | (TextAlign::End, direction::T::Ltr) => Alignment::Right,
        (TextAlign::Center, _) => Alignment::Center,
    }
}

fn translate_run_style(
    style: &impl TextRunStyle,
    word_break: WordBreak,
    wrap_mode: text_wrap_mode::T,
) -> ParleyTextStyle<'static, 'static, crate::style::TextBrush> {
    let families = translate_font_family_list(style);
    let features = style
        .font_feature_settings()
        .0
        .iter()
        .map(|setting| {
            FontFeature::new(
                Tag::from_bytes(setting.tag.0.to_be_bytes()),
                u16::try_from(setting.value).unwrap_or(0),
            )
        })
        .collect::<Vec<_>>();
    let variations = style
        .font_variation_settings()
        .0
        .iter()
        .map(|setting| {
            FontVariation::new(Tag::from_bytes(setting.tag.0.to_be_bytes()), setting.value)
        })
        .collect::<Vec<_>>();

    let font_style = style.font_style();
    ParleyTextStyle {
        font_family: ParleyFontFamily::List(Cow::Owned(families)),
        font_size: style.font_size(),
        font_style: if font_style == FontStyle::NORMAL {
            ParleyFontStyle::Normal
        } else if font_style == FontStyle::ITALIC {
            ParleyFontStyle::Italic
        } else {
            ParleyFontStyle::Oblique(Some(font_style.oblique_degrees()))
        },
        font_weight: ParleyFontWeight::new(style.font_weight().value()),
        font_variations: FontVariations::List(Cow::Owned(variations)),
        font_features: FontFeatures::List(Cow::Owned(features)),
        line_height: match style.line_height() {
            LineHeight::Normal => ParleyLineHeight::MetricsRelative(1.0),
            LineHeight::Number(factor) => ParleyLineHeight::FontSizeRelative(factor.0),
            LineHeight::Length(length) => ParleyLineHeight::Absolute(length.0.px()),
        },
        letter_spacing: style.letter_spacing().0.resolve(Length::zero()).px(),
        word_break: match word_break {
            WordBreak::Normal => ParleyWordBreak::Normal,
            WordBreak::BreakAll => ParleyWordBreak::BreakAll,
            WordBreak::KeepAll => ParleyWordBreak::KeepAll,
        },
        overflow_wrap: ParleyOverflowWrap::BreakWord,
        text_wrap_mode: match wrap_mode {
            text_wrap_mode::T::Wrap => ParleyTextWrapMode::Wrap,
            text_wrap_mode::T::Nowrap => ParleyTextWrapMode::NoWrap,
        },
        ..ParleyTextStyle::default()
    }
}

fn translate_font_family_list(style: &impl TextRunStyle) -> Vec<ParleyFontFamilyName<'static>> {
    let family = style.font_family();
    let mut families = family
        .families
        .list
        .iter()
        .map(|single| match single {
            SingleFontFamily::FamilyName(name) => {
                ParleyFontFamilyName::Named(Cow::Owned(name.name.to_string()))
            }
            SingleFontFamily::Generic(generic) => {
                ParleyFontFamilyName::Generic(translate_generic_family(*generic))
            }
        })
        .collect::<Vec<_>>();
    if families.is_empty() {
        families.push(ParleyFontFamilyName::Generic(
            ParleyGenericFamily::SansSerif,
        ));
    }
    families
}

const fn translate_generic_family(value: GenericFontFamily) -> ParleyGenericFamily {
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
    use stylo::Atom;
    use stylo::values::computed::font::{
        FamilyName, FontFamily, FontFamilyList, FontFamilyNameSyntax,
    };
    use stylo::values::computed::{
        Display, FontFeatureSettings, FontVariationSettings, FontWeight, LetterSpacing,
        NonNegativeLength,
    };
    use stylo::values::generics::NonNegative;
    use stylo::values::generics::font::{FeatureTagValue, FontSettings, FontTag, VariationValue};

    use super::*;
    use crate::geometry::Size;
    use crate::style::CoreStyle;
    use crate::tree::RequestedAxis;

    const AHEM: &[u8] = include_bytes!("../../tests/fixtures/Ahem.ttf");

    #[derive(Debug)]
    struct ContainerStyle {
        align: TextAlign,
        direction: direction::T,
    }

    impl Default for ContainerStyle {
        fn default() -> Self {
            Self {
                align: TextAlign::Start,
                direction: direction::T::Ltr,
            }
        }
    }

    impl CoreStyle for ContainerStyle {
        fn display(&self) -> Display {
            Display::Flex
        }

        fn direction(&self) -> direction::T {
            self.direction
        }
    }

    impl TextContainerStyle for ContainerStyle {
        fn text_align(&self) -> TextAlign {
            self.align
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

    #[derive(Debug)]
    struct RunStyle {
        family: FontFamily,
    }

    impl TextRunStyle for RunStyle {
        fn font_family(&self) -> FontFamily {
            self.family.clone()
        }

        fn font_feature_settings(&self) -> FontFeatureSettings {
            FontSettings(
                vec![FeatureTagValue {
                    tag: FontTag(u32::from_be_bytes(*b"kern")),
                    value: 1,
                }]
                .into(),
            )
        }

        fn font_variation_settings(&self) -> FontVariationSettings {
            FontSettings(
                vec![VariationValue {
                    tag: FontTag(u32::from_be_bytes(*b"wght")),
                    value: 400.0,
                }]
                .into(),
            )
        }
    }

    #[test]
    fn one_shaped_layout_is_rebroken_for_probe_and_commit_constraints() {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(AHEM), 1);
        let style = RunStyle {
            family: named_family("Ahem"),
        };
        let container = ContainerStyle::default();
        let runs = [TextRun {
            text: "abcdefghij",
            style: &style,
            preserve_newlines: false,
        }];
        let mut artifacts = TextLayoutStore::default();
        let mut measurer =
            TextMeasurer::new(&mut context, &mut artifacts, &container, runs.into_iter());
        let probe = LeafMeasureInput::new(
            Size::NONE,
            Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
            LayoutGoal::Measure(RequestedAxis::Both),
        );

        assert_eq!(measurer.measure(probe).size(), Size::new(80.0, 32.0));
        assert_eq!(measurer.context.shape_count(), 1);
        let narrower = LeafMeasureInput::new(
            Size::NONE,
            Size::new(AvailableSpace::Definite(48.0), AvailableSpace::MaxContent),
            LayoutGoal::Measure(RequestedAxis::Both),
        );
        assert_eq!(measurer.measure(narrower).size(), Size::new(48.0, 64.0));
        assert_eq!(measurer.context.shape_count(), 1);

        let commit = LeafMeasureInput::new(
            Size::NONE,
            Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
            LayoutGoal::Commit,
        );
        assert_eq!(measurer.measure(commit).layout().line_count(), 2);
        assert_eq!(measurer.context.shape_count(), 1);
        assert!(measurer.artifacts.probe().is_none());
        assert!(measurer.artifacts.committed().is_some());

        assert_eq!(measurer.measure(narrower).size(), Size::new(48.0, 64.0));
        assert_eq!(measurer.context.shape_count(), 1);
        assert!(measurer.artifacts.probe().is_some());
        assert!(measurer.artifacts.committed().is_some());
    }

    #[test]
    fn constraint_and_alignment_mappings_cover_protocol_values() {
        let input = LeafMeasureInput::new(
            Size::NONE,
            Size::new(AvailableSpace::MinContent, AvailableSpace::MaxContent),
            LayoutGoal::Commit,
        );
        let empty = TextLayout::shaped(parley::Layout::default(), false);
        assert_eq!(line_break_width(input, &empty), Some(0.0));
        assert_eq!(
            alignment(TextAlign::Start, direction::T::Rtl),
            Alignment::Right
        );
        assert_eq!(
            alignment(TextAlign::End, direction::T::Rtl),
            Alignment::Left
        );
        assert_eq!(
            alignment(TextAlign::Center, direction::T::Ltr),
            Alignment::Center
        );
        assert_eq!(
            alignment(TextAlign::Left, direction::T::Rtl),
            Alignment::Left
        );
        assert_eq!(
            alignment(TextAlign::Right, direction::T::Ltr),
            Alignment::Right
        );
    }

    struct EmptyRunStyle {
        font_style: FontStyle,
        line_height: LineHeight,
        weight: FontWeight,
    }

    impl TextRunStyle for EmptyRunStyle {
        fn font_family(&self) -> FontFamily {
            FontFamily {
                families: FontFamilyList {
                    list: stylo::ArcSlice::from_iter(std::iter::empty()),
                },
                is_system_font: false,
                is_initial: false,
            }
        }

        fn font_style(&self) -> FontStyle {
            self.font_style
        }

        fn font_weight(&self) -> FontWeight {
            self.weight
        }

        fn line_height(&self) -> LineHeight {
            self.line_height
        }
    }

    #[test]
    fn translation_covers_font_and_paragraph_value_enums() {
        for generic in [
            GenericFontFamily::None,
            GenericFontFamily::Serif,
            GenericFontFamily::SansSerif,
            GenericFontFamily::Monospace,
            GenericFontFamily::Cursive,
            GenericFontFamily::Fantasy,
        ] {
            let _ = translate_generic_family(generic);
        }
        assert!(matches!(
            translate_generic_family(GenericFontFamily::SystemUi),
            ParleyGenericFamily::SystemUi
        ));

        let empty = EmptyRunStyle {
            font_style: FontStyle::ITALIC,
            line_height: LineHeight::Length(NonNegativeLength::new(24.0)),
            weight: FontWeight::from_float(900.0),
        };
        let translated =
            translate_run_style(&empty, WordBreak::BreakAll, text_wrap_mode::T::Nowrap);
        assert_eq!(translated.font_style, ParleyFontStyle::Italic);
        assert_eq!(translated.line_height, ParleyLineHeight::Absolute(24.0));
        assert_eq!(translated.word_break, ParleyWordBreak::BreakAll);
        assert_eq!(translated.text_wrap_mode, ParleyTextWrapMode::NoWrap);
        assert_eq!(translated.overflow_wrap, ParleyOverflowWrap::BreakWord);
        assert!(matches!(
            translated.font_family,
            ParleyFontFamily::List(ref list) if !list.is_empty()
        ));

        let spaced = EmptyRunStyle {
            font_style: FontStyle::NORMAL,
            line_height: LineHeight::Number(NonNegative(1.5)),
            weight: FontWeight::NORMAL,
        };
        let translated = translate_run_style(&spaced, WordBreak::Normal, text_wrap_mode::T::Wrap);
        assert_eq!(translated.font_style, ParleyFontStyle::Normal);
        assert_eq!(
            translated.line_height,
            ParleyLineHeight::FontSizeRelative(1.5)
        );
        assert_eq!(translated.word_break, ParleyWordBreak::Normal);
        assert_eq!(translated.text_wrap_mode, ParleyTextWrapMode::Wrap);

        let _ = LetterSpacing::normal();

        let oblique = EmptyRunStyle {
            font_style: FontStyle::oblique(20.0),
            line_height: LineHeight::Normal,
            weight: FontWeight::NORMAL,
        };
        let translated = translate_run_style(&oblique, WordBreak::KeepAll, text_wrap_mode::T::Wrap);
        assert_eq!(translated.font_style, ParleyFontStyle::Oblique(Some(20.0)));
        assert_eq!(translated.word_break, ParleyWordBreak::KeepAll);
    }

    struct GenericRunStyle;

    impl TextRunStyle for GenericRunStyle {
        fn font_family(&self) -> FontFamily {
            FontFamily {
                families: FontFamilyList {
                    list: stylo::ArcSlice::from_iter(std::iter::once(SingleFontFamily::Generic(
                        GenericFontFamily::Monospace,
                    ))),
                },
                is_system_font: false,
                is_initial: false,
            }
        }
    }

    #[test]
    fn generic_families_translate_without_the_empty_list_fallback() {
        let translated =
            translate_run_style(&GenericRunStyle, WordBreak::Normal, text_wrap_mode::T::Wrap);
        let ParleyFontFamily::List(list) = translated.font_family else {
            panic!("family translation always produces a list");
        };
        assert_eq!(list.len(), 1);
        assert!(matches!(
            list[0],
            ParleyFontFamilyName::Generic(ParleyGenericFamily::Monospace)
        ));
    }

    #[test]
    fn measurer_debug_is_non_exhaustive_and_stable() {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(AHEM), 1);
        let style = RunStyle {
            family: named_family("Ahem"),
        };
        let container = ContainerStyle::default();
        let runs = [TextRun {
            text: "a",
            style: &style,
            preserve_newlines: false,
        }];
        let mut artifacts = TextLayoutStore::default();
        let measurer =
            TextMeasurer::new(&mut context, &mut artifacts, &container, runs.into_iter());
        assert_eq!(format!("{measurer:?}"), "TextMeasurer { .. }");
    }
}
