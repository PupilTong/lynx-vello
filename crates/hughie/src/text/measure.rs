//! Parley-backed implementation of hughie's fixed text-content path.

use core::fmt;
use std::borrow::Cow;

use parley::setting::Tag;
use parley::{
    Alignment, CHROMIUM_LINE_BREAK_OVERRIDE, FontFamily as ParleyFontFamily,
    FontFamilyName as ParleyFontFamilyName, FontFeature, FontFeatures,
    FontStyle as ParleyFontStyle, FontVariation, FontVariations, FontWeight as ParleyFontWeight,
    GenericFamily as ParleyGenericFamily, InlineBox as ParleyInlineBox, InlineBoxKind,
    LineHeight as ParleyLineHeight, OverflowWrap as ParleyOverflowWrap,
    TextStyle as ParleyTextStyle, TextWrapMode as ParleyTextWrapMode, WordBreak as ParleyWordBreak,
};
use stylo::Zero;
use stylo::computed_values::{direction, text_wrap_mode};
use stylo::values::computed::font::{FontFamily, GenericFontFamily, SingleFontFamily};
use stylo::values::computed::{FontStyle, Length, LineHeight, TextAlign, WordBreak};

use super::content::{
    AtomicInlineBox, InlineItem, ShapingContent, normalize_items, normalize_runs,
};
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
        compute_leaf_layout_with_measurement(input, container_style, None, true, |measure_input| {
            self.measure(measure_input).metrics()
        })
    }

    fn shape(&mut self) -> TextLayout {
        let content = normalize_runs(
            self.runs.clone(),
            self.container_style.white_space_collapse(),
        );
        shape_content(self.context, self.container_style, content)
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
        let artifact = reusable.unwrap_or_else(|| Box::new(self.shape()));
        match goal {
            LayoutGoal::Measure(_) => self.artifacts.probe = Some(artifact),
            LayoutGoal::Commit => self.artifacts.committed = Some(artifact),
        }
    }
}

/// Parley adapter for a source-ordered mixture of text runs and measured
/// atomic inline boxes.
pub struct InlineMeasurer<'session, 'source, Container, RunStyle, Items>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Items: Iterator<Item = InlineItem<'source, RunStyle>> + Clone,
{
    context: &'session mut TextContext,
    artifacts: &'session mut TextLayoutStore,
    container_style: &'source Container,
    items: Items,
}

impl<'session, 'source, Container, RunStyle, Items>
    InlineMeasurer<'session, 'source, Container, RunStyle, Items>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Items: Iterator<Item = InlineItem<'source, RunStyle>> + Clone,
{
    pub fn new(
        context: &'session mut TextContext,
        artifacts: &'session mut TextLayoutStore,
        container_style: &'source Container,
        items: Items,
    ) -> Self {
        Self {
            context,
            artifacts,
            container_style,
            items,
        }
    }

    pub fn compute_layout(&mut self, input: LayoutInput) -> LayoutOutput {
        let container_style = self.container_style;
        compute_leaf_layout_with_measurement(input, container_style, None, true, |measure_input| {
            self.measure(measure_input).metrics()
        })
    }

    pub fn measure(&mut self, input: LeafMeasureInput) -> TextMeasurement<'_> {
        self.install_artifact_if_needed(input.goal);
        measure_installed_artifact(self.artifacts, self.container_style, input)
    }

    fn shape(&mut self) -> TextLayout {
        let content = normalize_items(
            self.items.clone(),
            self.container_style.white_space_collapse(),
        );
        shape_content(self.context, self.container_style, content)
    }

    fn install_artifact_if_needed(&mut self, goal: LayoutGoal) {
        let target_matches = match goal {
            LayoutGoal::Measure(_) => self
                .artifacts
                .probe
                .as_deref()
                .is_some_and(|artifact| inline_boxes_match(artifact, self.items.clone())),
            LayoutGoal::Commit => self
                .artifacts
                .committed
                .as_deref()
                .is_some_and(|artifact| inline_boxes_match(artifact, self.items.clone())),
        };
        if target_matches {
            return;
        }

        let reusable = match goal {
            LayoutGoal::Measure(_) => self
                .artifacts
                .committed
                .as_deref()
                .filter(|artifact| inline_boxes_match(artifact, self.items.clone()))
                .map(|artifact| Box::new(artifact.clone())),
            LayoutGoal::Commit => {
                if self
                    .artifacts
                    .probe
                    .as_deref()
                    .is_some_and(|artifact| inline_boxes_match(artifact, self.items.clone()))
                {
                    self.artifacts.probe.take()
                } else {
                    self.artifacts.probe = None;
                    None
                }
            }
        };
        let artifact = reusable.unwrap_or_else(|| Box::new(self.shape()));
        match goal {
            LayoutGoal::Measure(_) => self.artifacts.probe = Some(artifact),
            LayoutGoal::Commit => self.artifacts.committed = Some(artifact),
        }
    }
}

impl<'source, Container, RunStyle, Items> fmt::Debug
    for InlineMeasurer<'_, 'source, Container, RunStyle, Items>
where
    Container: TextContainerStyle,
    RunStyle: TextRunStyle + 'source,
    Items: Iterator<Item = InlineItem<'source, RunStyle>> + Clone,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InlineMeasurer")
            .finish_non_exhaustive()
    }
}

fn shape_content<R: TextRunStyle>(
    context: &mut TextContext,
    container_style: &impl TextContainerStyle,
    shaping: ShapingContent<'_, R>,
) -> TextLayout {
    #[cfg(test)]
    context.record_shape();
    let (font_context, layout_context) = context.font_and_layout_contexts();
    let mut builder =
        layout_context.style_run_builder(font_context, shaping.text.as_str(), 1.0, false);
    let word_break = container_style.word_break();
    if word_break != WordBreak::BreakAll {
        builder.set_line_break_override(Some(CHROMIUM_LINE_BREAK_OVERRIDE));
    }
    builder.reserve(shaping.ranges.len(), shaping.ranges.len());

    for range in &shaping.ranges {
        let owned_family;
        let family = if let Some(family) = range.style.font_family_ref() {
            family
        } else {
            owned_family = range.style.font_family();
            &owned_family
        };
        let style = translate_run_style(
            range.style,
            family,
            word_break,
            container_style.text_wrap_mode(),
        );
        let style_index = builder.push_style(style);
        builder.push_style_run(style_index, range.bytes.clone());
    }

    let has_content = !shaping.text.is_empty() || !shaping.boxes.is_empty();
    let mut atomic_boxes = Vec::with_capacity(shaping.boxes.len());
    for inline_box in shaping.boxes {
        let atomic = sanitize_atomic_inline_box(inline_box.inline_box);
        builder.push_inline_box(ParleyInlineBox {
            id: atomic.id,
            kind: InlineBoxKind::InFlow,
            index: inline_box.index,
            width: atomic.width,
            height: atomic.height,
        });
        atomic_boxes.push(atomic);
    }
    let layout = builder.build(shaping.text.as_str());
    TextLayout::shaped_with_inline_boxes(layout, has_content, atomic_boxes)
}

fn inline_boxes_match<'source, RunStyle, Items>(artifact: &TextLayout, items: Items) -> bool
where
    RunStyle: TextRunStyle + 'source,
    Items: Iterator<Item = InlineItem<'source, RunStyle>>,
{
    artifact.inline_boxes_match(items.filter_map(|item| match item {
        InlineItem::Text(_) => None,
        InlineItem::Atomic(inline_box) => Some(sanitize_atomic_inline_box(inline_box)),
    }))
}

fn sanitize_atomic_inline_box(inline_box: AtomicInlineBox) -> AtomicInlineBox {
    let width = finite_non_negative(inline_box.width);
    let height = finite_non_negative(inline_box.height);
    let baseline = if inline_box.baseline.is_finite() {
        inline_box.baseline.clamp(0.0, height)
    } else {
        height
    };
    AtomicInlineBox {
        id: inline_box.id,
        width,
        height,
        baseline,
    }
}

fn finite_non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
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
        self.install_artifact_if_needed(input.goal);
        measure_installed_artifact(self.artifacts, self.container_style, input)
    }
}

fn measure_installed_artifact<'a>(
    artifacts: &'a mut TextLayoutStore,
    container_style: &impl TextContainerStyle,
    input: LeafMeasureInput,
) -> TextMeasurement<'a> {
    let inline_basis = definite_inline_size(input).unwrap_or(0.0).max(0.0);
    let indent = container_style
        .text_indent()
        .length
        .resolve(Length::new(inline_basis))
        .px();
    let alignment = alignment(container_style.text_align(), container_style.direction());

    let artifact = match input.goal {
        LayoutGoal::Measure(_) => artifacts
            .probe
            .as_deref_mut()
            .expect("a probe artifact was installed"),
        LayoutGoal::Commit => artifacts
            .committed
            .as_deref_mut()
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

fn translate_run_style<'family>(
    style: &impl TextRunStyle,
    family: &'family FontFamily,
    word_break: WordBreak,
    wrap_mode: text_wrap_mode::T,
) -> ParleyTextStyle<'family, 'static, crate::style::TextBrush> {
    let feature_settings = style.font_feature_settings();
    let features = feature_settings
        .0
        .iter()
        .map(|setting| {
            FontFeature::new(
                Tag::from_bytes(setting.tag.0.to_be_bytes()),
                u16::try_from(setting.value).unwrap_or(0),
            )
        })
        .collect::<Vec<_>>();
    let variation_settings = style.font_variation_settings();
    let variations = variation_settings
        .0
        .iter()
        .map(|setting| {
            FontVariation::new(Tag::from_bytes(setting.tag.0.to_be_bytes()), setting.value)
        })
        .collect::<Vec<_>>();

    let font_style = style.font_style();
    ParleyTextStyle {
        font_family: translate_font_family_list(family),
        font_size: style.font_size(),
        font_style: if font_style == FontStyle::NORMAL {
            ParleyFontStyle::Normal
        } else if font_style == FontStyle::ITALIC {
            ParleyFontStyle::Italic
        } else {
            ParleyFontStyle::Oblique(Some(font_style.oblique_degrees()))
        },
        font_weight: ParleyFontWeight::new(style.font_weight().value()),
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

fn translate_font_family_list(family: &FontFamily) -> ParleyFontFamily<'_> {
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

fn translate_font_family_name(single: &SingleFontFamily) -> ParleyFontFamilyName<'_> {
    match single {
        SingleFontFamily::FamilyName(name) => {
            ParleyFontFamilyName::Named(Cow::Borrowed(name.name.as_ref()))
        }
        SingleFontFamily::Generic(generic) => {
            ParleyFontFamilyName::Generic(translate_generic_family(*generic))
        }
    }
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

    fn measure_input(width: AvailableSpace, goal: LayoutGoal) -> LeafMeasureInput {
        LeafMeasureInput::new(
            Size::NONE,
            Size::new(width, AvailableSpace::MaxContent),
            goal,
        )
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1.0e-5, "{actual} != {expected}");
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
        let probe = measure_input(
            AvailableSpace::Definite(80.0),
            LayoutGoal::Measure(RequestedAxis::Both),
        );

        assert_eq!(measurer.measure(probe).size(), Size::new(80.0, 32.0));
        assert_eq!(measurer.context.shape_count(), 1);
        let narrower = measure_input(
            AvailableSpace::Definite(48.0),
            LayoutGoal::Measure(RequestedAxis::Both),
        );
        assert_eq!(measurer.measure(narrower).size(), Size::new(48.0, 64.0));
        assert_eq!(measurer.context.shape_count(), 1);
        let probe_address = core::ptr::from_ref(
            measurer
                .artifacts
                .probe()
                .expect("the measured artifact remains a probe"),
        );

        let commit = measure_input(AvailableSpace::Definite(80.0), LayoutGoal::Commit);
        assert_eq!(measurer.measure(commit).layout().line_count(), 2);
        assert_eq!(measurer.context.shape_count(), 1);
        assert!(measurer.artifacts.probe().is_none());
        let committed = measurer
            .artifacts
            .committed()
            .expect("the probe was promoted to committed");
        assert!(core::ptr::eq(probe_address, core::ptr::from_ref(committed)));

        assert_eq!(measurer.measure(narrower).size(), Size::new(48.0, 64.0));
        assert_eq!(measurer.context.shape_count(), 1);
        assert!(measurer.artifacts.probe().is_some());
        assert!(measurer.artifacts.committed().is_some());
    }

    #[test]
    fn pure_atomic_paragraphs_wrap_and_expose_positions_and_baselines() {
        let mut context = TextContext::without_system_fonts();
        let container = ContainerStyle::default();
        let items = [
            InlineItem::<RunStyle>::Atomic(AtomicInlineBox::new(1, 40.0, 16.0).with_baseline(6.0)),
            InlineItem::Atomic(AtomicInlineBox::new(2, 30.0, 20.0).with_baseline(12.0)),
        ];
        let mut artifacts = TextLayoutStore::default();
        let mut measurer =
            InlineMeasurer::new(&mut context, &mut artifacts, &container, items.into_iter());

        let max_content = measurer.measure(measure_input(
            AvailableSpace::MaxContent,
            LayoutGoal::Measure(RequestedAxis::Both),
        ));
        assert_close(max_content.size().width, 70.0);
        assert_eq!(max_content.layout().line_count(), 1);

        let committed = measurer.measure(measure_input(
            AvailableSpace::Definite(50.0),
            LayoutGoal::Commit,
        ));
        assert_eq!(committed.layout().line_count(), 2);
        assert!(
            committed
                .layout()
                .first_baseline()
                .is_some_and(|value| value > 0.0)
        );
        let first = committed
            .layout()
            .positioned_inline_box(1)
            .expect("the first atomic box is positioned");
        let second = committed
            .layout()
            .positioned_inline_box(2)
            .expect("the second atomic box is positioned");
        assert_eq!(first.size, Size::new(40.0, 16.0));
        assert_close(first.baseline, 6.0);
        assert_eq!(second.size, Size::new(30.0, 20.0));
        assert_close(second.baseline, 12.0);
        assert!(second.origin.y >= first.origin.y + first.size.height);
        assert!(committed.size().height >= second.origin.y + second.size.height);
    }

    #[test]
    fn changed_atomic_metrics_reshape_retained_intrinsic_widths() {
        let mut context = TextContext::without_system_fonts();
        let container = ContainerStyle::default();
        let mut artifacts = TextLayoutStore::default();

        {
            let items = [InlineItem::<RunStyle>::Atomic(AtomicInlineBox::new(
                1, 20.0, 10.0,
            ))];
            let mut measurer =
                InlineMeasurer::new(&mut context, &mut artifacts, &container, items.into_iter());
            assert_close(
                measurer
                    .measure(measure_input(
                        AvailableSpace::MaxContent,
                        LayoutGoal::Measure(RequestedAxis::Both),
                    ))
                    .size()
                    .width,
                20.0,
            );
        }
        assert_eq!(context.shape_count(), 1);

        {
            let items = [InlineItem::<RunStyle>::Atomic(AtomicInlineBox::new(
                1, 45.0, 14.0,
            ))];
            let mut measurer =
                InlineMeasurer::new(&mut context, &mut artifacts, &container, items.into_iter());
            assert_close(
                measurer
                    .measure(measure_input(
                        AvailableSpace::MaxContent,
                        LayoutGoal::Measure(RequestedAxis::Both),
                    ))
                    .size()
                    .width,
                45.0,
            );
            assert_close(
                measurer
                    .measure(measure_input(
                        AvailableSpace::MaxContent,
                        LayoutGoal::Commit,
                    ))
                    .size()
                    .width,
                45.0,
            );
        }
        assert_eq!(context.shape_count(), 2);
    }

    #[test]
    fn constraint_and_alignment_mappings_cover_protocol_values() {
        let input = measure_input(AvailableSpace::MinContent, LayoutGoal::Commit);
        let empty =
            TextLayout::shaped_with_inline_boxes(parley::Layout::default(), false, Vec::new());
        assert_eq!(line_break_width(input, &empty), Some(0.0));
        for (value, direction, expected) in [
            (TextAlign::Start, direction::T::Rtl, Alignment::Right),
            (TextAlign::End, direction::T::Rtl, Alignment::Left),
            (TextAlign::Center, direction::T::Ltr, Alignment::Center),
            (TextAlign::Left, direction::T::Rtl, Alignment::Left),
            (TextAlign::Right, direction::T::Ltr, Alignment::Right),
        ] {
            assert_eq!(alignment(value, direction), expected);
        }
    }

    struct EmptyRunStyle {
        font_style: FontStyle,
        line_height: LineHeight,
        weight: FontWeight,
        generic: Option<GenericFontFamily>,
    }

    impl TextRunStyle for EmptyRunStyle {
        fn font_family(&self) -> FontFamily {
            FontFamily {
                families: FontFamilyList {
                    list: stylo::ArcSlice::from_iter(
                        self.generic.map(SingleFontFamily::Generic).into_iter(),
                    ),
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
            generic: None,
        };
        let family = empty.font_family();
        let translated = translate_run_style(
            &empty,
            &family,
            WordBreak::BreakAll,
            text_wrap_mode::T::Nowrap,
        );
        assert_eq!(translated.font_style, ParleyFontStyle::Italic);
        assert_eq!(translated.line_height, ParleyLineHeight::Absolute(24.0));
        assert_eq!(translated.word_break, ParleyWordBreak::BreakAll);
        assert_eq!(translated.text_wrap_mode, ParleyTextWrapMode::NoWrap);
        assert_eq!(translated.overflow_wrap, ParleyOverflowWrap::BreakWord);
        assert!(matches!(
            translated.font_family,
            ParleyFontFamily::Single(ParleyFontFamilyName::Generic(
                ParleyGenericFamily::SansSerif
            ))
        ));

        let spaced = EmptyRunStyle {
            font_style: FontStyle::NORMAL,
            line_height: LineHeight::Number(NonNegative(1.5)),
            weight: FontWeight::NORMAL,
            generic: None,
        };
        let family = spaced.font_family();
        let translated =
            translate_run_style(&spaced, &family, WordBreak::Normal, text_wrap_mode::T::Wrap);
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
            generic: None,
        };
        let family = oblique.font_family();
        let translated = translate_run_style(
            &oblique,
            &family,
            WordBreak::KeepAll,
            text_wrap_mode::T::Wrap,
        );
        assert_eq!(translated.font_style, ParleyFontStyle::Oblique(Some(20.0)));
        assert_eq!(translated.word_break, ParleyWordBreak::KeepAll);
    }

    #[test]
    fn generic_families_translate_without_the_empty_list_fallback() {
        let style = EmptyRunStyle {
            font_style: FontStyle::NORMAL,
            line_height: LineHeight::Normal,
            weight: FontWeight::NORMAL,
            generic: Some(GenericFontFamily::Monospace),
        };
        let family = style.font_family();
        let translated =
            translate_run_style(&style, &family, WordBreak::Normal, text_wrap_mode::T::Wrap);
        assert!(matches!(
            translated.font_family,
            ParleyFontFamily::Single(ParleyFontFamilyName::Generic(
                ParleyGenericFamily::Monospace
            ))
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
