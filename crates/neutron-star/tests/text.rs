//! Parley text measurement conformance and host-integration tests.

use neutron_star::compute::{
    LeafMeasureInput, compute_cached_layout, compute_flexbox_layout, compute_root_layout,
};
use neutron_star::geometry::{Edges, Size};
use neutron_star::style::{CoreStyle, TextContainerStyle, TextRun, TextRunStyle};
use neutron_star::text::{TextContext, TextLayoutStore, TextMeasurer};
use neutron_star::tree::{
    AvailableSpace, LayoutGoal, LayoutInput, LayoutOutput, LayoutSlot, LayoutTree, RequestedAxis,
};
use parley::layout::BreakReason;
use stylo::Atom;
use stylo::computed_values::{direction, text_wrap_mode, white_space_collapse};
use stylo::values::computed::font::{
    FamilyName, FontFamily, FontFamilyList, FontFamilyNameSyntax, SingleFontFamily,
};
use stylo::values::computed::text::GenericLetterSpacing;
use stylo::values::computed::{
    Display, FontFeatureSettings, FontStyle, FontVariationSettings, FontWeight, ItemPlacement,
    Length, LengthPercentage, LetterSpacing, LineHeight, NonNegativeLength,
    NonNegativeLengthPercentage, NonNegativeNumber, TextAlign, TextIndent, WordBreak,
};
use stylo::values::generics::NonNegative;
use stylo::values::generics::font::{FeatureTagValue, FontSettings, FontTag, VariationValue};
use stylo::values::specified::align::AlignFlags;

const AHEM: &[u8] = include_bytes!("fixtures/Ahem.ttf");
const EPSILON: f32 = 0.01;

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= EPSILON,
        "expected {expected}, got {actual}"
    );
}

fn assert_size(actual: Size<f32>, expected: Size<f32>) {
    assert_close(actual.width, expected.width);
    assert_close(actual.height, expected.height);
}

fn text_context() -> TextContext {
    let mut context = TextContext::without_system_fonts();
    assert_eq!(context.register_fonts(AHEM), 1);
    context
}

fn ahem_family() -> FontFamily {
    FontFamily {
        families: FontFamilyList {
            list: stylo::ArcSlice::from_iter(std::iter::once(SingleFontFamily::FamilyName(
                FamilyName {
                    name: Atom::from("Ahem"),
                    syntax: FontFamilyNameSyntax::Identifiers,
                },
            ))),
        },
        is_system_font: false,
        is_initial: false,
    }
}

fn px(value: f32) -> LengthPercentage {
    LengthPercentage::new_length(Length::new(value))
}

fn npx(value: f32) -> NonNegativeLengthPercentage {
    NonNegative(px(value))
}

fn indent_px(value: f32) -> TextIndent {
    TextIndent {
        length: px(value),
        hanging: false,
        each_line: false,
    }
}

fn font_tag(tag: [u8; 4]) -> FontTag {
    FontTag(u32::from_be_bytes(tag))
}

#[derive(Debug, Clone)]
struct RunStyle {
    family: FontFamily,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    letter_spacing: LetterSpacing,
    line_height: LineHeight,
    features: FontFeatureSettings,
    variations: FontVariationSettings,
}

impl RunStyle {
    fn ahem(font_size: f32) -> Self {
        Self {
            family: ahem_family(),
            font_size,
            font_weight: FontWeight::NORMAL,
            font_style: FontStyle::NORMAL,
            letter_spacing: LetterSpacing::normal(),
            line_height: LineHeight::normal(),
            features: FontFeatureSettings::normal(),
            variations: FontVariationSettings::normal(),
        }
    }
}

impl TextRunStyle for RunStyle {
    fn font_family(&self) -> FontFamily {
        self.family.clone()
    }

    fn font_size(&self) -> f32 {
        self.font_size
    }

    fn font_weight(&self) -> FontWeight {
        self.font_weight
    }

    fn font_style(&self) -> FontStyle {
        self.font_style
    }

    fn letter_spacing(&self) -> LetterSpacing {
        self.letter_spacing.clone()
    }

    fn line_height(&self) -> LineHeight {
        self.line_height
    }

    fn font_feature_settings(&self) -> FontFeatureSettings {
        self.features.clone()
    }

    fn font_variation_settings(&self) -> FontVariationSettings {
        self.variations.clone()
    }
}

#[derive(Debug, Clone)]
struct ContainerStyle {
    align: TextAlign,
    wrap: text_wrap_mode::T,
    collapse: white_space_collapse::T,
    word_break: WordBreak,
    indent: TextIndent,
    direction: direction::T,
    padding: Edges<NonNegativeLengthPercentage>,
}

impl Default for ContainerStyle {
    fn default() -> Self {
        Self {
            align: TextAlign::Start,
            wrap: text_wrap_mode::T::Wrap,
            collapse: white_space_collapse::T::Collapse,
            word_break: WordBreak::Normal,
            indent: TextIndent::zero(),
            direction: direction::T::Ltr,
            padding: Edges {
                left: npx(0.0),
                right: npx(0.0),
                top: npx(0.0),
                bottom: npx(0.0),
            },
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

    fn padding(&self) -> Edges<&NonNegativeLengthPercentage> {
        self.padding.as_ref()
    }
}

impl TextContainerStyle for ContainerStyle {
    fn text_align(&self) -> TextAlign {
        self.align
    }

    fn text_wrap_mode(&self) -> text_wrap_mode::T {
        self.wrap
    }

    fn white_space_collapse(&self) -> white_space_collapse::T {
        self.collapse
    }

    fn word_break(&self) -> WordBreak {
        self.word_break
    }

    fn text_indent(&self) -> TextIndent {
        self.indent.clone()
    }
}

#[derive(Debug, Clone, Copy)]
struct Observation {
    size: Size<f32>,
    baseline: Option<f32>,
    line_count: usize,
}

struct TextFixture {
    context: TextContext,
    artifacts: TextLayoutStore,
}

impl TextFixture {
    fn new() -> Self {
        let context = text_context();
        Self {
            context,
            artifacts: TextLayoutStore::default(),
        }
    }

    fn observe(
        &mut self,
        container: &ContainerStyle,
        runs: &[TextRun<'_, RunStyle>],
        input: LeafMeasureInput,
    ) -> Observation {
        let mut measurer = TextMeasurer::new(
            &mut self.context,
            &mut self.artifacts,
            container,
            runs.iter().copied(),
        );
        let measurement = measurer.measure(input);
        Observation {
            size: measurement.size(),
            baseline: measurement.first_baselines().y,
            line_count: measurement.layout().line_count(),
        }
    }

    fn observe_text(
        &mut self,
        container: &ContainerStyle,
        style: &RunStyle,
        text: &str,
        preserve_newlines: bool,
        input: LeafMeasureInput,
    ) -> Observation {
        self.observe(
            container,
            &[TextRun {
                text,
                style,
                preserve_newlines,
            }],
            input,
        )
    }

    fn commit(
        &mut self,
        container: &ContainerStyle,
        style: &RunStyle,
        text: &str,
        width: AvailableSpace,
    ) -> Observation {
        self.observe_text(
            container,
            style,
            text,
            false,
            request(width, LayoutGoal::Commit),
        )
    }

    fn intrinsic(
        &mut self,
        container: &ContainerStyle,
        style: &RunStyle,
        text: &str,
        width: AvailableSpace,
    ) -> Observation {
        self.observe_text(
            container,
            style,
            text,
            false,
            request(width, LayoutGoal::Measure(RequestedAxis::Horizontal)),
        )
    }

    fn first_break(
        &mut self,
        container: &mut ContainerStyle,
        style: &RunStyle,
        text: &str,
        word_break: WordBreak,
        case: &str,
    ) -> BreakReason {
        container.word_break = word_break;
        self.artifacts.invalidate();
        self.commit(container, style, text, AvailableSpace::Definite(32.0));
        self.artifacts
            .committed()
            .unwrap_or_else(|| panic!("{case}: missing commit"))
            .parley_layout()
            .get(0)
            .unwrap_or_else(|| panic!("{case}: missing first line"))
            .break_reason()
    }
}

fn request(width: AvailableSpace, goal: LayoutGoal) -> LeafMeasureInput {
    let available_space = Size::new(width, AvailableSpace::MaxContent);
    LeafMeasureInput::new(Size::NONE, available_space, goal)
}

fn committed_line_offset(artifacts: &TextLayoutStore, line: usize, case: &str) -> f32 {
    artifacts
        .committed()
        .expect("committed text layout")
        .parley_layout()
        .get(line)
        .unwrap_or_else(|| panic!("{case}: missing line {line}"))
        .metrics()
        .offset
}

#[test]
fn exact_ahem_geometry_covers_empty_whitespace_single_word_and_wrapping() {
    let style = RunStyle::ahem(16.0);
    let container = ContainerStyle::default();
    let mut fixture = TextFixture::new();

    for (index, (case, text, size, lines, baseline)) in [
        ("empty", "", Size::ZERO, 0, None),
        ("collapsible whitespace", " \t \r\n ", Size::ZERO, 0, None),
        (
            "single word",
            "abcdefghij",
            Size::new(160.0, 16.0),
            1,
            Some(12.8),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            fixture.artifacts.invalidate();
        }
        let measured = fixture.commit(&container, &style, text, AvailableSpace::MaxContent);
        assert_size(measured.size, size);
        assert_eq!(measured.line_count, lines, "{case}: line count");
        match baseline {
            Some(expected) => assert_close(measured.baseline.expect("baseline"), expected),
            None => assert_eq!(measured.baseline, None, "{case}: baseline"),
        }
    }

    for (case, width, size, lines) in [
        ("wrapped at 80px", 80.0, Size::new(80.0, 32.0), 2),
        ("wrapped at 48px", 48.0, Size::new(48.0, 64.0), 4),
    ] {
        let input = request(
            AvailableSpace::Definite(width),
            LayoutGoal::Measure(RequestedAxis::Both),
        );
        let measured = fixture.observe_text(&container, &style, "abcdefghij", false, input);
        assert_size(measured.size, size);
        assert_eq!(measured.line_count, lines, "{case}: line count");
    }
}

#[test]
fn intrinsic_width_and_nowrap_rebreak_retained_shape() {
    let style = RunStyle::ahem(16.0);
    let mut fixture = TextFixture::new();
    let mut container = ContainerStyle::default();

    let maximum = fixture.commit(&container, &style, "abc defgh", AvailableSpace::MaxContent);
    assert_size(maximum.size, Size::new(144.0, 16.0));
    assert_eq!(
        fixture
            .artifacts
            .committed()
            .expect("committed")
            .max_advance(),
        None
    );

    let minimum = fixture.intrinsic(&container, &style, "abc defgh", AvailableSpace::MinContent);
    assert_size(minimum.size, Size::new(80.0, 32.0));
    assert_eq!(minimum.line_count, 2);
    assert_eq!(
        fixture.artifacts.probe().expect("probe").max_advance(),
        Some(80.0)
    );
    let committed = fixture
        .artifacts
        .committed()
        .expect("committed survives probe");
    assert_eq!(committed.max_advance(), None);
    assert_eq!(committed.line_count(), 1);

    container.word_break = WordBreak::BreakAll;
    fixture.artifacts.invalidate();
    let break_all = fixture.intrinsic(&container, &style, "abc defgh", AvailableSpace::MinContent);
    assert_size(break_all.size, Size::new(16.0, 128.0));
    assert_eq!(break_all.line_count, 8);
    assert_eq!(
        fixture
            .artifacts
            .probe()
            .expect("break-all probe")
            .max_advance(),
        Some(16.0)
    );

    container.wrap = text_wrap_mode::T::Nowrap;
    fixture.artifacts.invalidate();
    let nowrap_intrinsic =
        fixture.intrinsic(&container, &style, "abc defgh", AvailableSpace::MinContent);
    assert_size(nowrap_intrinsic.size, Size::new(144.0, 16.0));
    assert_eq!(nowrap_intrinsic.line_count, 1);

    fixture.artifacts.invalidate();
    let nowrap = fixture.commit(
        &container,
        &style,
        "abc defgh",
        AvailableSpace::Definite(32.0),
    );
    assert_size(nowrap.size, Size::new(144.0, 16.0));
    assert_eq!(nowrap.line_count, 1);
}

#[test]
fn auto_sized_alignment_uses_the_measured_text_width() {
    let style = RunStyle::ahem(16.0);
    let mut container = ContainerStyle {
        align: TextAlign::Right,
        wrap: text_wrap_mode::T::Nowrap,
        ..ContainerStyle::default()
    };
    let mut fixture = TextFixture::new();
    let measured = fixture.commit(&container, &style, "ab", AvailableSpace::Definite(80.0));

    assert_size(measured.size, Size::new(32.0, 16.0));
    assert_eq!(measured.line_count, 1);
    let committed = fixture
        .artifacts
        .committed()
        .expect("auto-sized nowrap commit");
    assert_eq!(committed.max_advance(), Some(32.0));
    assert_close(committed_line_offset(&fixture.artifacts, 0, "nowrap"), 0.0);

    container.wrap = text_wrap_mode::T::Wrap;
    fixture.artifacts.invalidate();
    let wrapped = fixture.commit(&container, &style, "abc de", AvailableSpace::Definite(80.0));
    assert_size(wrapped.size, Size::new(48.0, 32.0));
    assert_eq!(wrapped.line_count, 2);
    assert_close(
        committed_line_offset(&fixture.artifacts, 0, "wrapped first"),
        0.0,
    );
    assert_close(
        committed_line_offset(&fixture.artifacts, 1, "wrapped second"),
        16.0,
    );
}

#[test]
fn known_inline_size_aligns_without_changing_content_metrics() {
    let style = RunStyle::ahem(16.0);
    let container = ContainerStyle {
        align: TextAlign::Right,
        wrap: text_wrap_mode::T::Nowrap,
        ..ContainerStyle::default()
    };
    let mut fixture = TextFixture::new();
    let input = LeafMeasureInput::new(
        Size::new(Some(80.0), None),
        Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
        LayoutGoal::Commit,
    );
    let measured = fixture.observe_text(&container, &style, "ab", false, input);

    assert_size(measured.size, Size::new(32.0, 16.0));
    let committed = fixture.artifacts.committed().expect("known-width commit");
    assert_eq!(committed.max_advance(), Some(80.0));
    assert_close(
        committed_line_offset(&fixture.artifacts, 0, "known width"),
        48.0,
    );
}

#[test]
fn word_break_modes_change_regular_break_opportunities() {
    let style = RunStyle::ahem(16.0);
    let mut fixture = TextFixture::new();
    let mut container = ContainerStyle::default();

    for (case, word_break) in [
        ("break-all spaced Latin", WordBreak::BreakAll),
        ("keep-all spaced Latin", WordBreak::KeepAll),
    ] {
        container.word_break = word_break;
        fixture.artifacts.invalidate();
        let measured = fixture.commit(
            &container,
            &style,
            "abc defgh",
            AvailableSpace::Definite(32.0),
        );
        assert!(measured.line_count > 1, "{case}: line count");
        assert!(measured.size.width <= 32.0 + EPSILON, "{case}: inline size");
    }

    for (case, text, word_break, expected_break) in [
        (
            "normal unspaced Latin",
            "abcdefgh",
            WordBreak::Normal,
            BreakReason::Emergency,
        ),
        (
            "break-all unspaced Latin",
            "abcdefgh",
            WordBreak::BreakAll,
            BreakReason::Regular,
        ),
        (
            "normal CJK",
            "你好世界",
            WordBreak::Normal,
            BreakReason::Regular,
        ),
        (
            "keep-all CJK",
            "你好世界",
            WordBreak::KeepAll,
            BreakReason::Emergency,
        ),
    ] {
        assert_eq!(
            fixture.first_break(&mut container, &style, text, word_break, case),
            expected_break,
            "{case}"
        );
    }

    container.word_break = WordBreak::KeepAll;
    fixture.artifacts.invalidate();
    let keep_all_intrinsic =
        fixture.intrinsic(&container, &style, "你好世界", AvailableSpace::MinContent);
    assert_size(keep_all_intrinsic.size, Size::new(64.0, 16.0));
    assert_eq!(keep_all_intrinsic.line_count, 1);
}

#[test]
fn run_spacing_line_height_and_mixed_sizes_affect_exact_geometry() {
    let mut spaced = RunStyle::ahem(16.0);
    spaced.letter_spacing = GenericLetterSpacing(px(2.0));
    spaced.font_weight = FontWeight::BOLD;
    spaced.font_style = FontStyle::ITALIC;
    spaced.features = FontSettings(
        vec![FeatureTagValue {
            tag: font_tag(*b"kern"),
            value: 0,
        }]
        .into(),
    );
    spaced.variations = FontSettings(
        vec![VariationValue {
            tag: font_tag(*b"wght"),
            value: 700.0,
        }]
        .into(),
    );
    let container = ContainerStyle::default();
    let mut fixture = TextFixture::new();
    let measured = fixture.commit(&container, &spaced, "abc", AvailableSpace::MaxContent);
    assert_close(measured.size.width, 54.0);

    for (case, line_height, expected_height) in [
        (
            "numeric line height",
            LineHeight::Number(NonNegativeNumber::from(2.0)),
            32.0,
        ),
        (
            "absolute line height",
            LineHeight::Length(NonNegativeLength::new(24.0)),
            24.0,
        ),
    ] {
        let mut style = RunStyle::ahem(16.0);
        style.line_height = line_height;
        fixture.artifacts.invalidate();
        let measured = fixture.commit(&container, &style, "abc", AvailableSpace::MaxContent);
        assert!(
            (measured.size.height - expected_height).abs() <= EPSILON,
            "{case}: expected {expected_height}, got {}",
            measured.size.height
        );
    }

    let small = RunStyle::ahem(16.0);
    let large = RunStyle::ahem(32.0);
    fixture.artifacts.invalidate();
    let mixed = [
        TextRun {
            text: "aa",
            style: &small,
            preserve_newlines: false,
        },
        TextRun {
            text: "BB",
            style: &large,
            preserve_newlines: false,
        },
    ];
    let measured = fixture.observe(
        &container,
        &mixed,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_size(measured.size, Size::new(96.0, 32.0));
}

#[test]
fn indent_newline_preservation_alignment_and_rtl_are_exported() {
    let style = RunStyle::ahem(16.0);
    let mut container = ContainerStyle {
        indent: indent_px(16.0),
        ..ContainerStyle::default()
    };
    let mut fixture = TextFixture::new();
    let measured = fixture.commit(
        &container,
        &style,
        "abcdefghij",
        AvailableSpace::Definite(80.0),
    );
    assert_eq!(measured.line_count, 3);
    assert_close(
        committed_line_offset(&fixture.artifacts, 0, "indented first"),
        16.0,
    );
    assert_close(
        committed_line_offset(&fixture.artifacts, 1, "indented second"),
        0.0,
    );

    container.indent = TextIndent::zero();
    for (case, preserve_newlines, expected_size, expected_lines) in [
        ("collapsed newline", false, Size::new(80.0, 16.0), None),
        ("preserved newline", true, Size::new(32.0, 32.0), Some(2)),
    ] {
        fixture.artifacts.invalidate();
        let measured = fixture.observe_text(
            &container,
            &style,
            "ab\ncd",
            preserve_newlines,
            request(AvailableSpace::MaxContent, LayoutGoal::Commit),
        );
        assert_size(measured.size, expected_size);
        if let Some(expected_lines) = expected_lines {
            assert_eq!(measured.line_count, expected_lines, "{case}: line count");
        }
    }

    container.direction = direction::T::Rtl;
    container.align = TextAlign::Start;
    fixture.artifacts.invalidate();
    let rtl = fixture.commit(&container, &style, "אבגד", AvailableSpace::Definite(80.0));
    assert_size(rtl.size, Size::new(64.0, 16.0));
    assert_eq!(rtl.line_count, 1);
    assert_close(committed_line_offset(&fixture.artifacts, 0, "RTL"), 0.0);
}

#[test]
fn compute_leaf_layout_adds_box_model_and_exports_first_baseline() {
    let style = RunStyle::ahem(16.0);
    let container = ContainerStyle {
        padding: Edges {
            left: npx(2.0),
            right: npx(2.0),
            top: npx(2.0),
            bottom: npx(2.0),
        },
        ..ContainerStyle::default()
    };
    let runs = [TextRun {
        text: "abc",
        style: &style,
        preserve_newlines: false,
    }];
    let mut context = text_context();
    let mut artifacts = TextLayoutStore::default();
    let mut measurer =
        TextMeasurer::new(&mut context, &mut artifacts, &container, runs.into_iter());

    let output = measurer.compute_layout(LayoutInput::default());

    assert_size(output.size, Size::new(52.0, 20.0));
    assert_close(
        output
            .first_baselines
            .y
            .expect("leaf layout exports a baseline"),
        14.8,
    );
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum HostDisplay {
    #[default]
    Text,
    Flex,
}

#[derive(Debug, Clone)]
struct HostStyle {
    display: Display,
    align_items: ItemPlacement,
}

impl Default for HostStyle {
    fn default() -> Self {
        Self {
            display: Display::Flex,
            align_items: ItemPlacement::normal(),
        }
    }
}

impl CoreStyle for HostStyle {
    fn display(&self) -> Display {
        self.display
    }

    fn align_items(&self) -> ItemPlacement {
        self.align_items
    }
}

impl TextContainerStyle for HostStyle {}

#[derive(Debug, Clone)]
struct SourceNode {
    display: HostDisplay,
    style: HostStyle,
    run_style: RunStyle,
    text: &'static str,
    children: Vec<usize>,
}

#[derive(Debug)]
struct HostState {
    slots: Vec<LayoutSlot>,
    artifacts: Vec<TextLayoutStore>,
    text: TextContext,
    leaf_measure_calls: usize,
}

/// Immutable source tree. Layout slots, retained text artifacts, and the
/// shared text context live in the separately borrowed [`HostState`].
#[derive(Debug)]
struct HostTree {
    nodes: Vec<SourceNode>,
}

impl HostTree {
    fn new(nodes: Vec<SourceNode>) -> (Self, HostState) {
        let state = HostState {
            slots: nodes.iter().map(|_| LayoutSlot::default()).collect(),
            artifacts: nodes.iter().map(|_| TextLayoutStore::default()).collect(),
            text: text_context(),
            leaf_measure_calls: 0,
        };
        (Self { nodes }, state)
    }

    fn node(&self, index: usize) -> HostRef {
        debug_assert!(index < self.nodes.len());
        HostRef(index)
    }
}

/// The `Copy` node id.
#[derive(Clone, Copy)]
struct HostRef(usize);

impl core::fmt::Debug for HostRef {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_tuple("HostRef").field(&self.0).finish()
    }
}

struct HostChildren<'t> {
    ids: core::slice::Iter<'t, usize>,
}

impl Iterator for HostChildren<'_> {
    type Item = HostRef;

    fn next(&mut self) -> Option<HostRef> {
        let index = *self.ids.next()?;
        Some(HostRef(index))
    }
}

impl LayoutTree for HostTree {
    type NodeId = HostRef;
    type State = HostState;
    type Style<'tree> = &'tree HostStyle;
    type ChildIter<'tree> = HostChildren<'tree>;

    fn children(&self, node: HostRef) -> HostChildren<'_> {
        HostChildren {
            ids: self.nodes[node.0].children.iter(),
        }
    }

    fn child_count(&self, node: HostRef) -> usize {
        self.nodes[node.0].children.len()
    }

    fn style(&self, node: HostRef) -> &HostStyle {
        &self.nodes[node.0].style
    }

    fn layout<'state>(&self, state: &'state HostState, node: HostRef) -> &'state LayoutSlot {
        &state.slots[node.0]
    }

    fn layout_mut<'state>(
        &self,
        state: &'state mut HostState,
        node: HostRef,
    ) -> &'state mut LayoutSlot {
        &mut state.slots[node.0]
    }

    fn compute_layout(
        &self,
        state: &mut HostState,
        node: HostRef,
        input: LayoutInput,
    ) -> LayoutOutput {
        let source = &self.nodes[node.0];
        let display = source.display;
        compute_cached_layout(
            self,
            state,
            node,
            input,
            |tree, state, node, input| match display {
                HostDisplay::Flex => compute_flexbox_layout(tree, state, node, input),
                HostDisplay::Text => {
                    state.leaf_measure_calls += 1;
                    let run = [TextRun {
                        text: source.text,
                        style: &source.run_style,
                        preserve_newlines: false,
                    }];
                    let mut measurer = TextMeasurer::new(
                        &mut state.text,
                        &mut state.artifacts[node.0],
                        &source.style,
                        run.into_iter(),
                    );
                    measurer.compute_layout(input)
                }
            },
        )
    }

    fn clear_layout_cache(&self, state: &mut HostState, node: HostRef) {
        state.slots[node.0].clear_layout_cache();
        state.artifacts[node.0].invalidate();
    }
}

#[test]
fn flex_baseline_integration_reuses_artifacts_and_jointly_invalidates_caches() {
    let root = 0;
    let small = 1;
    let large = 2;
    let (tree, mut state) = HostTree::new(vec![
        SourceNode {
            display: HostDisplay::Flex,
            style: HostStyle {
                display: Display::Flex,
                align_items: ItemPlacement(AlignFlags::BASELINE),
            },
            run_style: RunStyle::ahem(16.0),
            text: "",
            children: vec![small, large],
        },
        SourceNode {
            display: HostDisplay::Text,
            style: HostStyle::default(),
            run_style: RunStyle::ahem(16.0),
            text: "aaa",
            children: Vec::new(),
        },
        SourceNode {
            display: HostDisplay::Text,
            style: HostStyle::default(),
            run_style: RunStyle::ahem(32.0),
            text: "BBB",
            children: Vec::new(),
        },
    ]);

    compute_root_layout(&tree, &mut state, tree.node(root), Size::MAX_CONTENT);

    let small_baseline = state.slots[small].unrounded().location.y
        + state.artifacts[small]
            .committed()
            .expect("small committed text")
            .first_baseline()
            .expect("small baseline");
    let large_baseline = state.slots[large].unrounded().location.y
        + state.artifacts[large]
            .committed()
            .expect("large committed text")
            .first_baseline()
            .expect("large baseline");
    assert_close(small_baseline, large_baseline);
    assert!(state.artifacts[small].probe().is_none());
    assert!(state.artifacts[large].probe().is_none());

    let calls_after_first_layout = state.leaf_measure_calls;
    assert!(calls_after_first_layout >= 4);
    compute_root_layout(&tree, &mut state, tree.node(root), Size::MAX_CONTENT);
    assert_eq!(state.leaf_measure_calls, calls_after_first_layout);

    tree.clear_layout_cache(&mut state, tree.node(small));
    assert!(state.slots[small].layout_cache_is_empty());
    assert!(state.artifacts[small].probe().is_none());
    assert!(state.artifacts[small].committed().is_none());
    tree.clear_layout_cache(&mut state, tree.node(root));
    compute_root_layout(&tree, &mut state, tree.node(root), Size::MAX_CONTENT);
    assert!(state.leaf_measure_calls > calls_after_first_layout);
}
