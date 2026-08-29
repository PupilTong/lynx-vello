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
use super::layout::BreakConstraint;
#[cfg(debug_assertions)]
use super::layout::ShapeFingerprint;
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
        shape_content(
            self.context,
            self.container_style,
            content,
            #[cfg(debug_assertions)]
            self.shape_fingerprint(),
        )
    }

    /// Returns the node's retained layout, shaping it on first use.
    ///
    /// There is one layout per node for every goal. A probe re-breaks the same
    /// shaped data the commit uses instead of deep-cloning Parley's ten
    /// vectors into a second slot, and hands it back afterwards through
    /// [`TextLayoutStore::restore_committed`].
    fn retained(&mut self) -> &mut TextLayout {
        #[cfg(debug_assertions)]
        let fingerprint = self.shape_fingerprint();
        if self.artifacts.artifact.is_none() {
            let shaped = self.shape();
            self.artifacts.artifact = Some(Box::new(shaped));
        }
        let artifact = self
            .artifacts
            .artifact
            .as_deref_mut()
            .expect("the retained layout was just installed");
        #[cfg(debug_assertions)]
        artifact.assert_shaped_from(fingerprint);
        artifact
    }

    /// Hashes exactly the content and style values that reach Parley's shaper,
    /// so a retained layout that outlived one of them is caught here instead
    /// of painting stale glyphs.
    #[cfg(debug_assertions)]
    fn shape_fingerprint(&self) -> ShapeFingerprint {
        paragraph_shape_fingerprint(
            self.container_style,
            self.runs.clone().map(InlineItem::Text),
        )
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

    pub fn measure(&mut self, input: LeafMeasureInput) -> TextMeasurement {
        let container_style = self.container_style;
        let artifact = self.retained();
        measure_retained_artifact(artifact, container_style, input)
    }

    fn shape(&mut self) -> TextLayout {
        let content = normalize_items(
            self.items.clone(),
            self.container_style.white_space_collapse(),
        );
        shape_content(
            self.context,
            self.container_style,
            content,
            #[cfg(debug_assertions)]
            self.shape_fingerprint(),
        )
    }

    fn retained(&mut self) -> &mut TextLayout {
        let inline_boxes = self
            .items
            .clone()
            .filter_map(|item| match item {
                InlineItem::Text(_) => None,
                InlineItem::Atomic(inline_box) => Some(sanitize_atomic_inline_box(inline_box)),
            })
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        let fingerprint = self.shape_fingerprint();

        let topology_matches = self
            .artifacts
            .artifact
            .as_deref()
            .is_none_or(|artifact| artifact.inline_box_ids_match(inline_boxes.iter().copied()));
        if !topology_matches {
            debug_assert!(
                false,
                "a retained inline layout outlived its atomic-box topology"
            );
            self.artifacts.artifact = None;
        }
        if self.artifacts.artifact.is_none() {
            let shaped = self.shape();
            self.artifacts.artifact = Some(Box::new(shaped));
        }
        let artifact = self
            .artifacts
            .artifact
            .as_deref_mut()
            .expect("the retained inline layout was just installed");
        artifact.sync_inline_boxes(&inline_boxes);
        #[cfg(debug_assertions)]
        artifact.assert_shaped_from(fingerprint);
        artifact
    }

    #[cfg(debug_assertions)]
    fn shape_fingerprint(&self) -> ShapeFingerprint {
        paragraph_shape_fingerprint(self.container_style, self.items.clone())
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
    #[cfg(debug_assertions)] fingerprint: ShapeFingerprint,
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
    TextLayout::shaped_with_inline_boxes(
        layout,
        has_content,
        atomic_boxes,
        #[cfg(debug_assertions)]
        fingerprint,
    )
}

/// Hashes exactly the content and style values that reach Parley's shaper.
/// Atomic geometry is deliberately excluded: width, height, and baseline are
/// line-break inputs updated in place on the one retained shaped layout.
#[cfg(debug_assertions)]
fn paragraph_shape_fingerprint<'source, R, Items>(
    container_style: &impl TextContainerStyle,
    items: Items,
) -> ShapeFingerprint
where
    R: TextRunStyle + 'source,
    Items: Iterator<Item = InlineItem<'source, R>>,
{
    use core::hash::{Hash, Hasher};
    use core::mem::discriminant;
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();
    // Neither `text-indent` nor `text-align` belongs here: the first is a
    // break constraint and the second is re-applied on every commit.
    discriminant(&container_style.white_space_collapse()).hash(&mut hasher);
    discriminant(&container_style.word_break()).hash(&mut hasher);
    discriminant(&container_style.text_wrap_mode()).hash(&mut hasher);

    for item in items {
        let InlineItem::Text(run) = item else {
            1_u8.hash(&mut hasher);
            if let InlineItem::Atomic(inline_box) = item {
                inline_box.id.hash(&mut hasher);
            }
            continue;
        };
        0_u8.hash(&mut hasher);
        run.text.hash(&mut hasher);
        run.preserve_newlines.hash(&mut hasher);
        let style = run.style;
        let owned_family;
        let family = if let Some(family) = style.font_family_ref() {
            family
        } else {
            owned_family = style.font_family();
            &owned_family
        };
        for single in family.families.list.iter() {
            match single {
                SingleFontFamily::FamilyName(name) => {
                    0_u8.hash(&mut hasher);
                    name.name.as_ref().hash(&mut hasher);
                }
                SingleFontFamily::Generic(generic) => {
                    1_u8.hash(&mut hasher);
                    discriminant(generic).hash(&mut hasher);
                }
            }
        }
        style.font_size().to_bits().hash(&mut hasher);
        style.font_weight().value().to_bits().hash(&mut hasher);
        let font_style = style.font_style();
        if font_style == FontStyle::NORMAL {
            0_u8.hash(&mut hasher);
        } else if font_style == FontStyle::ITALIC {
            1_u8.hash(&mut hasher);
        } else {
            2_u8.hash(&mut hasher);
            font_style.oblique_degrees().to_bits().hash(&mut hasher);
        }
        style
            .letter_spacing()
            .0
            .resolve(Length::zero())
            .px()
            .to_bits()
            .hash(&mut hasher);
        match style.line_height() {
            LineHeight::Normal => 0_u8.hash(&mut hasher),
            LineHeight::Number(factor) => {
                1_u8.hash(&mut hasher);
                factor.0.to_bits().hash(&mut hasher);
            }
            LineHeight::Length(length) => {
                2_u8.hash(&mut hasher);
                length.0.px().to_bits().hash(&mut hasher);
            }
        }
        for feature in &style.font_feature_settings().0 {
            feature.tag.0.hash(&mut hasher);
            feature.value.hash(&mut hasher);
        }
        for variation in &style.font_variation_settings().0 {
            variation.tag.0.hash(&mut hasher);
            variation.value.to_bits().hash(&mut hasher);
        }
    }
    ShapeFingerprint::from_hash(hasher.finish())
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
    pub fn measure(&mut self, input: LeafMeasureInput) -> TextMeasurement {
        let container_style = self.container_style;
        let artifact = self.retained();
        measure_retained_artifact(artifact, container_style, input)
    }
}

fn measure_retained_artifact(
    artifact: &mut TextLayout,
    container_style: &impl TextContainerStyle,
    input: LeafMeasureInput,
) -> TextMeasurement {
    let inline_basis = definite_inline_size(input).unwrap_or(0.0).max(0.0);
    let indent = container_style
        .text_indent()
        .length
        .resolve(Length::new(inline_basis))
        .px();
    let alignment = alignment(container_style.text_align(), container_style.direction());

    let max_advance = line_break_width(input, artifact);
    let constraint = BreakConstraint::new(max_advance, indent);
    if !matches!(input.goal, LayoutGoal::Commit) {
        return artifact.probe(constraint);
    }

    let mut measured = artifact.commit_break(constraint);
    // Shrink-to-fit: alignment distributes the leftover of the width lines
    // were broken at, so an auto-sized box that broke against a wider
    // constraint breaks again against its own used width. Parley does this
    // for itself when the break was unconstrained (its line breaker rewrites
    // `inline_max_coord` to the laid-out width), so only a finite constraint
    // wider than the content needs the second pass.
    //
    // Re-breaking at `Layout::width()` is not neutral: that width excludes
    // hanging trailing whitespace while the breaker's fit test includes it,
    // so text ending in a space can gain an empty trailing line, and the f32
    // round trip through a subtracted width can split the widest line one
    // cluster early. Reproducing that is deliberate for now — fixing it needs
    // an alignment width independent of the break width, which Parley 0.11
    // does not expose.
    if input.known_dimensions.width.is_none()
        && max_advance.is_some_and(|limit| limit > measured.size().width)
    {
        measured = artifact.commit_break(BreakConstraint::new(Some(measured.size().width), indent));
    }
    artifact.align(alignment);
    artifact.mark_committed(alignment);
    measured
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
    use crate::text::FontBlob;
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
    fn one_shaped_layout_serves_every_probe_and_commit_constraint() {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
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
        let retained = core::ptr::from_ref(
            measurer
                .artifacts
                .retained()
                .expect("the probe installed the node's one retained layout"),
        );
        assert!(
            measurer.artifacts.committed().is_none(),
            "a probe alone commits nothing"
        );

        // A repeated probe at the same constraint costs nothing at all.
        assert_eq!(measurer.measure(probe).size(), Size::new(80.0, 32.0));
        assert_eq!(measurer.artifacts.retained().unwrap().break_count(), 1);

        let narrower = measure_input(
            AvailableSpace::Definite(48.0),
            LayoutGoal::Measure(RequestedAxis::Both),
        );
        assert_eq!(measurer.measure(narrower).size(), Size::new(48.0, 64.0));
        assert_eq!(measurer.context.shape_count(), 1);
        assert_eq!(measurer.artifacts.retained().unwrap().break_count(), 2);

        // The commit re-uses the same shaped layout — no second slot, no copy.
        let commit = measure_input(AvailableSpace::Definite(80.0), LayoutGoal::Commit);
        assert_eq!(measurer.measure(commit).line_count(), 2);
        assert_eq!(measurer.context.shape_count(), 1);
        let committed = measurer
            .artifacts
            .committed()
            .expect("the retained layout is now committed");
        assert!(core::ptr::eq(retained, core::ptr::from_ref(committed)));
        assert_eq!(committed.break_count(), 3);
        assert!(!measurer.artifacts.is_probe_dirty());

        // A probe at an already-measured constraint answers from the memo and
        // leaves the committed line break in place.
        assert_eq!(measurer.measure(narrower).size(), Size::new(48.0, 64.0));
        assert!(!measurer.artifacts.is_probe_dirty());
        assert_eq!(measurer.artifacts.retained().unwrap().break_count(), 3);

        // A probe at a constraint the memo has forgotten does move it, and
        // owes the restore the pass driver performs.
        let unseen = measure_input(
            AvailableSpace::Definite(32.0),
            LayoutGoal::Measure(RequestedAxis::Both),
        );
        assert_eq!(measurer.measure(unseen).size(), Size::new(32.0, 80.0));
        assert!(measurer.artifacts.is_probe_dirty());
        assert!(measurer.artifacts.restore_committed());
        assert!(!measurer.artifacts.is_probe_dirty());
        assert_eq!(
            measurer.artifacts.committed().unwrap().max_advance(),
            Some(80.0)
        );
        assert_eq!(measurer.context.shape_count(), 1);
    }

    #[test]
    fn a_commit_at_the_probed_width_reuses_the_probe_line_break() {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
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

        // Exactly the sequence a flex container imposes on an auto-sized text
        // child: probe the line width, then commit at it. Start alignment in
        // LTR never reads the leftover space, so the shrink-to-fit rebreak is
        // skipped and the whole pass costs one line break.
        let width = AvailableSpace::Definite(80.0);
        measurer.measure(measure_input(
            width,
            LayoutGoal::Measure(RequestedAxis::Both),
        ));
        measurer.measure(measure_input(width, LayoutGoal::Commit));

        let committed = measurer.artifacts.committed().expect("committed");
        assert_eq!(committed.break_count(), 1);
        assert_eq!(committed.line_count(), 2);
        assert_eq!(committed.size(), Size::new(80.0, 32.0));
    }

    #[test]
    fn an_auto_sized_commit_shrinks_to_its_measured_width() {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        let style = RunStyle {
            family: named_family("Ahem"),
        };
        let container = ContainerStyle {
            align: TextAlign::Center,
            direction: direction::T::Ltr,
        };
        let runs = [TextRun {
            text: "abcd",
            style: &style,
            preserve_newlines: false,
        }];
        let mut artifacts = TextLayoutStore::default();
        let mut measurer =
            TextMeasurer::new(&mut context, &mut artifacts, &container, runs.into_iter());

        measurer.measure(measure_input(
            AvailableSpace::Definite(200.0),
            LayoutGoal::Commit,
        ));

        let committed = measurer.artifacts.committed().expect("committed");
        assert_eq!(committed.max_advance(), Some(64.0));
        assert_eq!(committed.break_count(), 2);
    }

    #[test]
    #[should_panic(expected = "outlived the content or style it was shaped from")]
    #[cfg(debug_assertions)]
    fn a_retained_layout_that_outlived_its_content_is_caught() {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        let style = RunStyle {
            family: named_family("Ahem"),
        };
        let container = ContainerStyle::default();
        let mut artifacts = TextLayoutStore::default();
        let commit = measure_input(AvailableSpace::Definite(80.0), LayoutGoal::Commit);
        {
            let runs = [TextRun {
                text: "abc",
                style: &style,
                preserve_newlines: false,
            }];
            let mut measurer =
                TextMeasurer::new(&mut context, &mut artifacts, &container, runs.into_iter());
            measurer.measure(commit);
        }

        // The host changed the text but no eviction path dropped the artifact.
        let runs = [TextRun {
            text: "abcd",
            style: &style,
            preserve_newlines: false,
        }];
        let mut measurer =
            TextMeasurer::new(&mut context, &mut artifacts, &container, runs.into_iter());
        measurer.measure(commit);
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
        assert_eq!(max_content.line_count(), 1);

        let committed = measurer.measure(measure_input(
            AvailableSpace::Definite(50.0),
            LayoutGoal::Commit,
        ));
        assert_eq!(committed.line_count(), 2);
        assert!(
            committed
                .first_baselines()
                .y
                .is_some_and(|value| value > 0.0)
        );
        let layout = measurer.artifacts.committed().expect("committed");
        let first = layout
            .positioned_inline_box(1)
            .expect("the first atomic box is positioned");
        let second = layout
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
    fn changed_atomic_metrics_update_the_retained_layout_without_reshaping() {
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
        let retained = core::ptr::from_ref(artifacts.retained().expect("retained"));

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
        assert_eq!(context.shape_count(), 1);
        assert!(core::ptr::eq(
            retained,
            core::ptr::from_ref(artifacts.committed().expect("committed"))
        ));
    }

    #[test]
    fn an_atomic_probe_restores_the_committed_metrics_even_at_the_same_width() {
        let mut context = TextContext::without_system_fonts();
        let container = ContainerStyle::default();
        let mut artifacts = TextLayoutStore::default();
        let input = LeafMeasureInput::new(
            Size::new(Some(50.0), None),
            Size::new(AvailableSpace::Definite(50.0), AvailableSpace::MaxContent),
            LayoutGoal::Commit,
        );

        {
            let items = [InlineItem::<RunStyle>::Atomic(AtomicInlineBox::new(
                1, 20.0, 10.0,
            ))];
            InlineMeasurer::new(&mut context, &mut artifacts, &container, items.into_iter())
                .measure(input);
        }
        assert_eq!(
            artifacts
                .committed()
                .and_then(|layout| layout.positioned_inline_box(1))
                .map(|inline_box| inline_box.size),
            Some(Size::new(20.0, 10.0))
        );

        {
            let items = [InlineItem::<RunStyle>::Atomic(AtomicInlineBox::new(
                1, 40.0, 15.0,
            ))];
            let probe = LeafMeasureInput::new(
                Size::new(Some(50.0), None),
                Size::new(AvailableSpace::Definite(50.0), AvailableSpace::MaxContent),
                LayoutGoal::Measure(RequestedAxis::Both),
            );
            InlineMeasurer::new(&mut context, &mut artifacts, &container, items.into_iter())
                .measure(probe);
        }
        assert!(artifacts.is_probe_dirty());
        assert!(artifacts.restore_committed());
        assert!(!artifacts.is_probe_dirty());
        assert_eq!(
            artifacts
                .committed()
                .and_then(|layout| layout.positioned_inline_box(1))
                .map(|inline_box| inline_box.size),
            Some(Size::new(20.0, 10.0))
        );
    }

    #[test]
    fn constraint_and_alignment_mappings_cover_protocol_values() {
        let input = measure_input(AvailableSpace::MinContent, LayoutGoal::Commit);
        let empty = TextLayout::shaped(
            parley::Layout::default(),
            false,
            #[cfg(debug_assertions)]
            super::super::layout::ShapeFingerprint::from_hash(0),
        );
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

    /// A run style that answers from real computed values, the way the
    /// document host does — the branch `font_family_ref` exists for.
    #[derive(Debug)]
    struct ComputedRunStyle(stylo::servo_arc::Arc<stylo::properties::ComputedValues>);

    impl TextRunStyle for ComputedRunStyle {
        fn computed_text_values(&self) -> Option<&stylo::properties::ComputedValues> {
            Some(&self.0)
        }
    }

    #[test]
    fn every_run_style_shape_shapes_and_re_checks_its_fingerprint() {
        fn measure_twice<R: TextRunStyle>(
            context: &mut TextContext,
            container: &ContainerStyle,
            input: LeafMeasureInput,
            style: &R,
            preserve_newlines: bool,
        ) {
            let shaped = context.shape_count();
            let mut artifacts = TextLayoutStore::default();
            let run = TextRun {
                text: "ab",
                style,
                preserve_newlines,
            };
            let mut measurer =
                TextMeasurer::new(context, &mut artifacts, container, [run].into_iter());
            let first = measurer.measure(input);
            // The second pass re-derives the fingerprint and checks it against
            // the layout the first one shaped.
            assert_eq!(measurer.measure(input), first);
            assert_eq!(measurer.context.shape_count(), shaped + 1);
        }

        use stylo::properties::style_structs::Font;

        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        let container = ContainerStyle::default();
        let commit = measure_input(AvailableSpace::Definite(80.0), LayoutGoal::Commit);

        // Borrowed computed values, and a family list long enough to take the
        // multi-name translation.
        let computed = ComputedRunStyle(
            stylo::properties::ComputedValues::initial_values_with_font_override(
                Font::initial_values(),
            ),
        );
        let listed = RunStyle {
            family: FontFamily {
                families: FontFamilyList {
                    list: stylo::ArcSlice::from_iter(
                        [
                            SingleFontFamily::FamilyName(FamilyName {
                                name: Atom::from("Ahem"),
                                syntax: FontFamilyNameSyntax::Identifiers,
                            }),
                            SingleFontFamily::Generic(GenericFontFamily::SansSerif),
                        ]
                        .into_iter(),
                    ),
                },
                is_system_font: false,
                is_initial: false,
            },
        };
        // Owned families, oblique and italic slants, both relative line-height
        // forms.
        let oblique = EmptyRunStyle {
            font_style: FontStyle::oblique(20.0),
            line_height: LineHeight::Length(NonNegativeLength::new(24.0)),
            weight: FontWeight::NORMAL,
            generic: Some(GenericFontFamily::Monospace),
        };
        let italic = EmptyRunStyle {
            font_style: FontStyle::ITALIC,
            line_height: LineHeight::Number(NonNegative(1.5)),
            weight: FontWeight::from_float(700.0),
            generic: None,
        };

        measure_twice(&mut context, &container, commit, &computed, false);
        measure_twice(&mut context, &container, commit, &listed, true);
        measure_twice(&mut context, &container, commit, &oblique, false);
        measure_twice(&mut context, &container, commit, &italic, false);
    }

    #[test]
    fn measurer_debug_is_non_exhaustive_and_stable() {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
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
