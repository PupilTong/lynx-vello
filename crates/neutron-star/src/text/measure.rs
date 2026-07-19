//! Parley-backed implementation of the generic leaf measurement protocol.

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

use super::content::normalize_runs;
use super::{ArtifactSlots, TextContext, TextLayout, TextLayoutView};
use crate::compute::{LeafMeasureInput, LeafMeasurer};
use crate::style::{
    CalcHandle, Direction, FontFamily, FontStyle, GenericFontFamily, LengthPercentage, LineHeight,
    TextAlign, TextBrush, TextContainerStyle, TextRun, TextRunStyle, WhiteSpace, WordBreak,
};
use crate::tree::{AvailableSpace, LayoutGoal};

/// Node-scoped Parley adapter for a host-owned paragraph.
///
/// The immutable container style and cloneable run iterator normally borrow
/// from the node's epoch-immutable text/style data. The mutable
/// [`TextContext`] and [`ArtifactSlots`] borrow separately from host-owned
/// interior-mutable slots; both borrows are node-scoped and end with the
/// measurer. `resolve_calc` is the same host callback used by box layout and
/// is needed only when `text-indent` contains a `calc()` value.
pub struct TextMeasurer<'session, 'source, Container, RunStyle, Runs, ResolveCalc>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Runs: Iterator<Item = TextRun<'source, RunStyle>> + Clone,
    ResolveCalc: Fn(CalcHandle, f32) -> f32,
{
    context: &'session mut TextContext,
    artifacts: &'session mut ArtifactSlots,
    container_style: &'source Container,
    runs: Runs,
    resolve_calc: ResolveCalc,
}

impl<'session, 'source, Container, RunStyle, Runs, ResolveCalc>
    TextMeasurer<'session, 'source, Container, RunStyle, Runs, ResolveCalc>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Runs: Iterator<Item = TextRun<'source, RunStyle>> + Clone,
    ResolveCalc: Fn(CalcHandle, f32) -> f32,
{
    /// Borrows one node's text inputs and host-owned measurement state.
    pub fn new(
        context: &'session mut TextContext,
        artifacts: &'session mut ArtifactSlots,
        container_style: &'source Container,
        runs: Runs,
        resolve_calc: ResolveCalc,
    ) -> Self {
        Self {
            context,
            artifacts,
            container_style,
            runs,
            resolve_calc,
        }
    }

    fn shape(&mut self) -> TextLayout {
        let content = normalize_runs(self.runs.clone());
        #[cfg(test)]
        self.context.record_shape();
        let (font_context, layout_context) = self.context.parts();
        // Keep layout in fractional CSS pixels. Device-pixel quantization belongs
        // to the engine's later DPR-aware rounding and rendering passes.
        let mut builder =
            layout_context.style_run_builder(font_context, content.text.as_str(), 1.0, false);
        let word_break = self.container_style.word_break();
        // Chromium's ordinary ASCII pair table deliberately suppresses
        // breaks between AL-class characters. Applying it to `break-all`
        // would erase opportunities created by that CSS value, so let ICU's
        // selected BreakAll mode own those boundaries.
        if word_break != WordBreak::BreakAll {
            builder.set_line_break_override(Some(CHROMIUM_LINE_BREAK_OVERRIDE));
        }
        builder.reserve(content.ranges.len(), content.ranges.len());

        for range in &content.ranges {
            let style =
                translate_run_style(range.style, word_break, self.container_style.white_space());
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

impl<'source, Container, RunStyle, Runs, ResolveCalc> fmt::Debug
    for TextMeasurer<'_, 'source, Container, RunStyle, Runs, ResolveCalc>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Runs: Iterator<Item = TextRun<'source, RunStyle>> + Clone,
    ResolveCalc: Fn(CalcHandle, f32) -> f32,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextMeasurer")
            .finish_non_exhaustive()
    }
}

impl<'source, Container, RunStyle, Runs, ResolveCalc> LeafMeasurer
    for TextMeasurer<'_, 'source, Container, RunStyle, Runs, ResolveCalc>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Runs: Iterator<Item = TextRun<'source, RunStyle>> + Clone,
    ResolveCalc: Fn(CalcHandle, f32) -> f32,
{
    type Measurement<'a>
        = TextLayoutView<'a>
    where
        Self: 'a;

    fn measure(&mut self, input: LeafMeasureInput) -> Self::Measurement<'_> {
        let inline_basis = definite_inline_size(input).unwrap_or(0.0).max(0.0);
        let indent = resolve_length_percentage(
            self.container_style.text_indent(),
            inline_basis,
            &self.resolve_calc,
        );
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
                // An available-space limit constrains wrapping, but it does not
                // make an auto-sized text box fill that limit. Rebreak at the
                // measured width so alignment positions glyphs inside the box.
                artifact.rebreak(Some(measured_width), indent);
            }
            artifact.align(alignment);
        }
        TextLayoutView::new(artifact)
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

fn resolve_length_percentage(
    value: LengthPercentage,
    basis: f32,
    resolve_calc: &impl Fn(CalcHandle, f32) -> f32,
) -> f32 {
    match value {
        LengthPercentage::Length(length) => length,
        LengthPercentage::Percent(fraction) => fraction * basis,
        LengthPercentage::Calc(handle) => resolve_calc(handle, basis),
    }
}

fn alignment(value: TextAlign, direction: Direction) -> Alignment {
    match (value, direction) {
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

fn translate_run_style(
    style: &impl TextRunStyle,
    word_break: WordBreak,
    white_space: WhiteSpace,
) -> ParleyTextStyle<'static, 'static, TextBrush> {
    let mut families: Vec<_> = style.font_families().map(translate_font_family).collect();
    if families.is_empty() {
        families.push(ParleyFontFamilyName::Generic(
            ParleyGenericFamily::SansSerif,
        ));
    }
    let features = style
        .font_feature_settings()
        .map(|(tag, value)| FontFeature::new(Tag::from_bytes(tag), value))
        .collect::<Vec<_>>();
    let variations = style
        .font_variation_settings()
        .map(|(tag, value)| FontVariation::new(Tag::from_bytes(tag), value))
        .collect::<Vec<_>>();

    ParleyTextStyle {
        font_family: ParleyFontFamily::List(Cow::Owned(families)),
        font_size: style.font_size(),
        font_style: match style.font_style() {
            FontStyle::Normal => ParleyFontStyle::Normal,
            FontStyle::Italic => ParleyFontStyle::Italic,
            FontStyle::Oblique => ParleyFontStyle::Oblique(None),
        },
        font_weight: ParleyFontWeight::new(f32::from(style.font_weight().value())),
        font_variations: FontVariations::List(Cow::Owned(variations)),
        font_features: FontFeatures::List(Cow::Owned(features)),
        line_height: match style.line_height() {
            LineHeight::Normal => ParleyLineHeight::MetricsRelative(1.0),
            LineHeight::Factor(factor) => ParleyLineHeight::FontSizeRelative(factor),
            LineHeight::Length(length) => ParleyLineHeight::Absolute(length),
        },
        letter_spacing: style.letter_spacing(),
        word_break: match word_break {
            WordBreak::Normal => ParleyWordBreak::Normal,
            WordBreak::BreakAll => ParleyWordBreak::BreakAll,
            WordBreak::KeepAll => ParleyWordBreak::KeepAll,
        },
        overflow_wrap: ParleyOverflowWrap::BreakWord,
        text_wrap_mode: match white_space {
            WhiteSpace::Normal => ParleyTextWrapMode::Wrap,
            WhiteSpace::NoWrap => ParleyTextWrapMode::NoWrap,
        },
        ..ParleyTextStyle::default()
    }
}

fn translate_font_family(value: FontFamily<'_>) -> ParleyFontFamilyName<'static> {
    match value {
        FontFamily::Named(name) => ParleyFontFamilyName::Named(Cow::Owned(name.to_owned())),
        FontFamily::Generic(generic) => {
            ParleyFontFamilyName::Generic(translate_generic_family(generic))
        }
    }
}

const fn translate_generic_family(value: GenericFontFamily) -> ParleyGenericFamily {
    match value {
        GenericFontFamily::Serif => ParleyGenericFamily::Serif,
        GenericFontFamily::SansSerif => ParleyGenericFamily::SansSerif,
        GenericFontFamily::Monospace => ParleyGenericFamily::Monospace,
        GenericFontFamily::Cursive => ParleyGenericFamily::Cursive,
        GenericFontFamily::Fantasy => ParleyGenericFamily::Fantasy,
        GenericFontFamily::SystemUi => ParleyGenericFamily::SystemUi,
        GenericFontFamily::UiSerif => ParleyGenericFamily::UiSerif,
        GenericFontFamily::UiSansSerif => ParleyGenericFamily::UiSansSerif,
        GenericFontFamily::UiMonospace => ParleyGenericFamily::UiMonospace,
        GenericFontFamily::UiRounded => ParleyGenericFamily::UiRounded,
        GenericFontFamily::Emoji => ParleyGenericFamily::Emoji,
        GenericFontFamily::Math => ParleyGenericFamily::Math,
        GenericFontFamily::FangSong => ParleyGenericFamily::FangSong,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::compute::LeafMeasurement;
    use crate::geometry::Size;
    use crate::style::{CoreStyle, FontFeatureSetting, FontVariationSetting, FontWeight, TextRun};
    use crate::tree::RequestedAxis;

    const AHEM: &[u8] = include_bytes!("../../tests/fixtures/Ahem.ttf");

    #[derive(Debug, Default)]
    struct ContainerStyle {
        align: TextAlign,
        direction: Direction,
    }

    impl CoreStyle for ContainerStyle {
        fn direction(&self) -> Direction {
            self.direction
        }
    }

    impl TextContainerStyle for ContainerStyle {
        fn text_align(&self) -> TextAlign {
            self.align
        }
    }

    #[derive(Debug)]
    struct RunStyle {
        family: FontFamily<'static>,
    }

    impl TextRunStyle for RunStyle {
        type FontFamilies<'a> = core::iter::Once<FontFamily<'a>>;
        type FontFeatureSettings<'a> = core::iter::Once<FontFeatureSetting>;
        type FontVariationSettings<'a> = core::iter::Once<FontVariationSetting>;

        fn font_families(&self) -> Self::FontFamilies<'_> {
            core::iter::once(self.family)
        }

        fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
            core::iter::once((*b"kern", 1))
        }

        fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
            core::iter::once((*b"wght", 400.0))
        }
    }

    #[test]
    fn one_shaped_layout_is_rebroken_for_probe_and_commit_constraints() {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(AHEM), 1);
        let style = RunStyle {
            family: FontFamily::Named("Ahem"),
        };
        let container = ContainerStyle::default();
        let runs = [TextRun {
            text: "abcdefghij",
            style: &style,
            preserve_newlines: false,
        }];
        let mut artifacts = ArtifactSlots::default();
        let mut measurer = TextMeasurer::new(
            &mut context,
            &mut artifacts,
            &container,
            runs.into_iter(),
            |_, _| unreachable!("the test has no calc values"),
        );
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
        assert_eq!(measurer.measure(commit).artifact().line_count(), 2);
        assert_eq!(measurer.context.shape_count(), 1);
        assert!(measurer.artifacts.probe().is_none());
        assert!(measurer.artifacts.committed().is_some());

        assert_eq!(measurer.measure(narrower).size(), Size::new(48.0, 64.0));
        assert_eq!(measurer.context.shape_count(), 1);
        assert!(measurer.artifacts.probe().is_some());
        assert!(measurer.artifacts.committed().is_some());
    }

    #[test]
    fn constraint_alignment_and_length_mappings_cover_protocol_values() {
        let input = LeafMeasureInput::new(
            Size::NONE,
            Size::new(AvailableSpace::MinContent, AvailableSpace::MaxContent),
            LayoutGoal::Commit,
        );
        let empty = TextLayout::shaped(parley::Layout::default(), false);
        assert_eq!(line_break_width(input, &empty), Some(0.0));
        assert_eq!(
            alignment(TextAlign::Start, Direction::Rtl),
            Alignment::Right
        );
        assert_eq!(alignment(TextAlign::End, Direction::Rtl), Alignment::Left);
        assert_eq!(
            alignment(TextAlign::Center, Direction::Ltr),
            Alignment::Center
        );
        assert_eq!(
            alignment(TextAlign::Justify, Direction::Ltr),
            Alignment::Justify
        );

        let handle = CalcHandle::from_raw(7);
        let length = resolve_length_percentage(LengthPercentage::Length(9.0), 40.0, &|_, _| 0.0);
        assert!((length - 9.0).abs() <= f32::EPSILON);
        let percent = resolve_length_percentage(LengthPercentage::Percent(0.25), 40.0, &|_, _| 0.0);
        assert!((percent - 10.0).abs() <= f32::EPSILON);
        let calc =
            resolve_length_percentage(LengthPercentage::Calc(handle), 40.0, &|seen, basis| {
                assert_eq!(seen, handle);
                basis + 2.0
            });
        assert!((calc - 42.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn translation_covers_font_and_paragraph_value_enums() {
        let all_generics = [
            GenericFontFamily::Serif,
            GenericFontFamily::SansSerif,
            GenericFontFamily::Monospace,
            GenericFontFamily::Cursive,
            GenericFontFamily::Fantasy,
            GenericFontFamily::SystemUi,
            GenericFontFamily::UiSerif,
            GenericFontFamily::UiSansSerif,
            GenericFontFamily::UiMonospace,
            GenericFontFamily::UiRounded,
            GenericFontFamily::Emoji,
            GenericFontFamily::Math,
            GenericFontFamily::FangSong,
        ];
        for generic in all_generics {
            assert!(matches!(
                translate_font_family(FontFamily::Generic(generic)),
                ParleyFontFamilyName::Generic(_)
            ));
        }

        let empty = EmptyRunStyle {
            font_style: FontStyle::Oblique,
            line_height: LineHeight::Length(24.0),
            weight: FontWeight::W900,
        };
        let translated = translate_run_style(&empty, WordBreak::BreakAll, WhiteSpace::NoWrap);
        assert_eq!(translated.font_style, ParleyFontStyle::Oblique(None));
        assert_eq!(translated.line_height, ParleyLineHeight::Absolute(24.0));
        assert_eq!(translated.word_break, ParleyWordBreak::BreakAll);
        assert_eq!(translated.text_wrap_mode, ParleyTextWrapMode::NoWrap);
        assert_eq!(translated.overflow_wrap, ParleyOverflowWrap::BreakWord);
    }

    struct EmptyRunStyle {
        font_style: FontStyle,
        line_height: LineHeight,
        weight: FontWeight,
    }

    impl TextRunStyle for EmptyRunStyle {
        type FontFamilies<'a> = core::iter::Empty<FontFamily<'a>>;
        type FontFeatureSettings<'a> = core::iter::Empty<FontFeatureSetting>;
        type FontVariationSettings<'a> = core::iter::Empty<FontVariationSetting>;

        fn font_families(&self) -> Self::FontFamilies<'_> {
            core::iter::empty()
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

        fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
            core::iter::empty()
        }

        fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
            core::iter::empty()
        }
    }
}
