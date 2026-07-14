//! Parley text measurement conformance and host-integration tests.

use neutron_star::cache::Cache;
use neutron_star::compute::{
    LeafMeasureInput, LeafMeasurement, LeafMeasurer, compute_cached_layout, compute_flexbox_layout,
    compute_leaf_layout, compute_root_layout,
};
use neutron_star::geometry::{Edges, Point, Size};
use neutron_star::style::{
    AlignItems, CalcHandle, CoreStyle, Direction, FlexContainerStyle, FlexItemStyle, FontFamily,
    FontFeatureSetting, FontStyle, FontVariationSetting, FontWeight, LengthPercentage, LineHeight,
    TextAlign, TextContainerStyle, TextRun, TextRunStyle, WhiteSpace, WordBreak,
};
use neutron_star::text::{ArtifactSlots, TextContext, TextMeasurer};
use neutron_star::tree::{
    AvailableSpace, CacheState, FlexSource, Layout, LayoutGoal, LayoutInput, LayoutOutput,
    LayoutSession, LayoutSource, LayoutState, NodeId, RequestedAxis, TraverseTree,
};
use parley::layout::BreakReason;

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

#[derive(Debug, Clone)]
struct RunStyle {
    families: Vec<FontFamily<'static>>,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    letter_spacing: f32,
    line_height: LineHeight,
    features: Vec<FontFeatureSetting>,
    variations: Vec<FontVariationSetting>,
}

impl RunStyle {
    fn ahem(font_size: f32) -> Self {
        Self {
            families: vec![FontFamily::Named("Ahem")],
            font_size,
            font_weight: FontWeight::Normal,
            font_style: FontStyle::Normal,
            letter_spacing: 0.0,
            line_height: LineHeight::Normal,
            features: Vec::new(),
            variations: Vec::new(),
        }
    }
}

fn copy_font_family<'a>(family: &'a FontFamily<'static>) -> FontFamily<'a> {
    *family
}

impl TextRunStyle for RunStyle {
    type FontFamilies<'a> = core::iter::Map<
        core::slice::Iter<'a, FontFamily<'static>>,
        fn(&'a FontFamily<'static>) -> FontFamily<'a>,
    >;
    type FontFeatureSettings<'a> = core::iter::Copied<core::slice::Iter<'a, FontFeatureSetting>>;
    type FontVariationSettings<'a> =
        core::iter::Copied<core::slice::Iter<'a, FontVariationSetting>>;

    fn font_families(&self) -> Self::FontFamilies<'_> {
        self.families.iter().map(copy_font_family as _)
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

    fn letter_spacing(&self) -> f32 {
        self.letter_spacing
    }

    fn line_height(&self) -> LineHeight {
        self.line_height
    }

    fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
        self.features.iter().copied()
    }

    fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
        self.variations.iter().copied()
    }
}

#[derive(Debug, Clone, Default)]
struct ContainerStyle {
    align: TextAlign,
    white_space: WhiteSpace,
    word_break: WordBreak,
    indent: LengthPercentage,
    direction: Direction,
    padding: Edges<LengthPercentage>,
}

impl CoreStyle for ContainerStyle {
    fn direction(&self) -> Direction {
        self.direction
    }

    fn padding(&self) -> Edges<LengthPercentage> {
        self.padding
    }
}

impl TextContainerStyle for ContainerStyle {
    fn text_align(&self) -> TextAlign {
        self.align
    }

    fn white_space(&self) -> WhiteSpace {
        self.white_space
    }

    fn word_break(&self) -> WordBreak {
        self.word_break
    }

    fn text_indent(&self) -> LengthPercentage {
        self.indent
    }
}

#[derive(Debug, Clone, Copy)]
struct Observation {
    size: Size<f32>,
    baseline: Option<f32>,
    line_count: usize,
}

fn request(width: AvailableSpace, goal: LayoutGoal) -> LeafMeasureInput {
    LeafMeasureInput::new(
        Size::NONE,
        Size::new(width, AvailableSpace::MaxContent),
        goal,
    )
}

fn observe(
    context: &mut TextContext,
    artifacts: &mut ArtifactSlots,
    container: &ContainerStyle,
    runs: &[TextRun<'_, RunStyle>],
    input: LeafMeasureInput,
) -> Observation {
    let mut measurer = TextMeasurer::new(
        context,
        artifacts,
        container,
        runs.iter().copied(),
        |_, _| unreachable!("test text-indent has no calc()"),
    );
    let measurement = measurer.measure(input);
    Observation {
        size: measurement.size(),
        baseline: measurement.first_baselines().y,
        line_count: measurement.artifact().line_count(),
    }
}

#[test]
fn exact_ahem_geometry_covers_empty_whitespace_single_word_and_wrapping() {
    let style = RunStyle::ahem(16.0);
    let container = ContainerStyle::default();
    let mut context = text_context();
    let mut artifacts = ArtifactSlots::default();

    let empty = [TextRun {
        text: "",
        style: &style,
        preserve_newlines: false,
    }];
    let measured = observe(
        &mut context,
        &mut artifacts,
        &container,
        &empty,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_size(measured.size, Size::ZERO);
    assert_eq!(measured.line_count, 0);
    assert_eq!(measured.baseline, None);

    artifacts.invalidate();
    let whitespace = [TextRun {
        text: " \t \r\n ",
        style: &style,
        preserve_newlines: false,
    }];
    let measured = observe(
        &mut context,
        &mut artifacts,
        &container,
        &whitespace,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_size(measured.size, Size::ZERO);
    assert_eq!(measured.line_count, 0);
    assert_eq!(measured.baseline, None);

    artifacts.invalidate();
    let word = [TextRun {
        text: "abcdefghij",
        style: &style,
        preserve_newlines: false,
    }];
    let unconstrained = observe(
        &mut context,
        &mut artifacts,
        &container,
        &word,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_size(unconstrained.size, Size::new(160.0, 16.0));
    assert_eq!(unconstrained.line_count, 1);
    assert_close(unconstrained.baseline.expect("text has a baseline"), 12.8);

    let wrapped = observe(
        &mut context,
        &mut artifacts,
        &container,
        &word,
        request(
            AvailableSpace::Definite(80.0),
            LayoutGoal::Measure(RequestedAxis::Both),
        ),
    );
    assert_size(wrapped.size, Size::new(80.0, 32.0));
    assert_eq!(wrapped.line_count, 2);

    let narrow = observe(
        &mut context,
        &mut artifacts,
        &container,
        &word,
        request(
            AvailableSpace::Definite(48.0),
            LayoutGoal::Measure(RequestedAxis::Both),
        ),
    );
    assert_size(narrow.size, Size::new(48.0, 64.0));
    assert_eq!(narrow.line_count, 4);
}

#[test]
fn intrinsic_width_and_nowrap_rebreak_retained_shape() {
    let style = RunStyle::ahem(16.0);
    let run = [TextRun {
        text: "abc defgh",
        style: &style,
        preserve_newlines: false,
    }];
    let mut context = text_context();
    let mut artifacts = ArtifactSlots::default();
    let mut container = ContainerStyle::default();

    let maximum = observe(
        &mut context,
        &mut artifacts,
        &container,
        &run,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_size(maximum.size, Size::new(144.0, 16.0));
    assert_eq!(
        artifacts.committed().expect("committed").max_advance(),
        None
    );

    let minimum = observe(
        &mut context,
        &mut artifacts,
        &container,
        &run,
        request(
            AvailableSpace::MinContent,
            LayoutGoal::Measure(RequestedAxis::Horizontal),
        ),
    );
    assert_size(minimum.size, Size::new(80.0, 32.0));
    assert_eq!(minimum.line_count, 2);
    assert_eq!(artifacts.probe().expect("probe").max_advance(), Some(80.0));
    let committed = artifacts.committed().expect("committed survives probe");
    assert_eq!(committed.max_advance(), None);
    assert_eq!(committed.line_count(), 1);

    container.word_break = WordBreak::BreakAll;
    artifacts.invalidate();
    let break_all = observe(
        &mut context,
        &mut artifacts,
        &container,
        &run,
        request(
            AvailableSpace::MinContent,
            LayoutGoal::Measure(RequestedAxis::Horizontal),
        ),
    );
    assert_size(break_all.size, Size::new(16.0, 128.0));
    assert_eq!(break_all.line_count, 8);
    assert_eq!(
        artifacts.probe().expect("break-all probe").max_advance(),
        Some(16.0)
    );

    container.white_space = WhiteSpace::NoWrap;
    artifacts.invalidate();
    let nowrap_intrinsic = observe(
        &mut context,
        &mut artifacts,
        &container,
        &run,
        request(
            AvailableSpace::MinContent,
            LayoutGoal::Measure(RequestedAxis::Horizontal),
        ),
    );
    assert_size(nowrap_intrinsic.size, Size::new(144.0, 16.0));
    assert_eq!(nowrap_intrinsic.line_count, 1);

    artifacts.invalidate();
    let nowrap = observe(
        &mut context,
        &mut artifacts,
        &container,
        &run,
        request(AvailableSpace::Definite(32.0), LayoutGoal::Commit),
    );
    assert_size(nowrap.size, Size::new(144.0, 16.0));
    assert_eq!(nowrap.line_count, 1);
}

#[test]
fn auto_sized_alignment_uses_the_measured_text_width() {
    let style = RunStyle::ahem(16.0);
    let mut container = ContainerStyle {
        align: TextAlign::Right,
        white_space: WhiteSpace::NoWrap,
        ..ContainerStyle::default()
    };
    let run = [TextRun {
        text: "ab",
        style: &style,
        preserve_newlines: false,
    }];
    let mut context = text_context();
    let mut artifacts = ArtifactSlots::default();

    let measured = observe(
        &mut context,
        &mut artifacts,
        &container,
        &run,
        request(AvailableSpace::Definite(80.0), LayoutGoal::Commit),
    );

    assert_size(measured.size, Size::new(32.0, 16.0));
    assert_eq!(measured.line_count, 1);
    let committed = artifacts.committed().expect("auto-sized nowrap commit");
    assert_eq!(committed.max_advance(), Some(32.0));
    assert_close(
        committed
            .parley_layout()
            .get(0)
            .expect("nowrap line")
            .metrics()
            .offset,
        0.0,
    );

    container.white_space = WhiteSpace::Normal;
    artifacts.invalidate();
    let wrapped_run = [TextRun {
        text: "abc de",
        style: &style,
        preserve_newlines: false,
    }];
    let wrapped = observe(
        &mut context,
        &mut artifacts,
        &container,
        &wrapped_run,
        request(AvailableSpace::Definite(80.0), LayoutGoal::Commit),
    );
    assert_size(wrapped.size, Size::new(48.0, 32.0));
    assert_eq!(wrapped.line_count, 2);
    let wrapped = artifacts.committed().expect("auto-sized wrapped commit");
    assert_close(
        wrapped
            .parley_layout()
            .get(0)
            .expect("first line")
            .metrics()
            .offset,
        0.0,
    );
    assert_close(
        wrapped
            .parley_layout()
            .get(1)
            .expect("second line")
            .metrics()
            .offset,
        16.0,
    );
}

#[test]
fn known_inline_size_aligns_without_changing_content_metrics() {
    let style = RunStyle::ahem(16.0);
    let container = ContainerStyle {
        align: TextAlign::Right,
        white_space: WhiteSpace::NoWrap,
        ..ContainerStyle::default()
    };
    let run = [TextRun {
        text: "ab",
        style: &style,
        preserve_newlines: false,
    }];
    let mut context = text_context();
    let mut artifacts = ArtifactSlots::default();
    let input = LeafMeasureInput::new(
        Size::new(Some(80.0), None),
        Size::new(AvailableSpace::Definite(80.0), AvailableSpace::MaxContent),
        LayoutGoal::Commit,
    );

    let measured = observe(&mut context, &mut artifacts, &container, &run, input);

    assert_size(measured.size, Size::new(32.0, 16.0));
    let committed = artifacts.committed().expect("known-width commit");
    assert_eq!(committed.max_advance(), Some(80.0));
    assert_close(
        committed
            .parley_layout()
            .get(0)
            .expect("known-width line")
            .metrics()
            .offset,
        48.0,
    );
}

#[test]
fn word_break_modes_change_regular_break_opportunities() {
    let style = RunStyle::ahem(16.0);
    let run = [TextRun {
        text: "abc defgh",
        style: &style,
        preserve_newlines: false,
    }];
    let mut context = text_context();
    let mut artifacts = ArtifactSlots::default();
    let mut container = ContainerStyle::default();

    for word_break in [WordBreak::BreakAll, WordBreak::KeepAll] {
        container.word_break = word_break;
        artifacts.invalidate();
        let measured = observe(
            &mut context,
            &mut artifacts,
            &container,
            &run,
            request(AvailableSpace::Definite(32.0), LayoutGoal::Commit),
        );
        assert!(measured.line_count > 1);
        assert!(measured.size.width <= 32.0 + EPSILON);
    }

    let latin = [TextRun {
        text: "abcdefgh",
        style: &style,
        preserve_newlines: false,
    }];
    for (word_break, expected_break) in [
        (WordBreak::Normal, BreakReason::Emergency),
        (WordBreak::BreakAll, BreakReason::Regular),
    ] {
        container.word_break = word_break;
        artifacts.invalidate();
        observe(
            &mut context,
            &mut artifacts,
            &container,
            &latin,
            request(AvailableSpace::Definite(32.0), LayoutGoal::Commit),
        );
        let first_break = artifacts
            .committed()
            .expect("latin line breaks")
            .parley_layout()
            .get(0)
            .expect("latin first line")
            .break_reason();
        assert_eq!(first_break, expected_break);
    }

    let cjk = [TextRun {
        text: "你好世界",
        style: &style,
        preserve_newlines: false,
    }];
    for (word_break, expected_break) in [
        (WordBreak::Normal, BreakReason::Regular),
        (WordBreak::KeepAll, BreakReason::Emergency),
    ] {
        container.word_break = word_break;
        artifacts.invalidate();
        observe(
            &mut context,
            &mut artifacts,
            &container,
            &cjk,
            request(AvailableSpace::Definite(32.0), LayoutGoal::Commit),
        );
        let first_break = artifacts
            .committed()
            .expect("CJK line breaks")
            .parley_layout()
            .get(0)
            .expect("CJK first line")
            .break_reason();
        assert_eq!(first_break, expected_break);
    }

    container.word_break = WordBreak::KeepAll;
    artifacts.invalidate();
    let keep_all_intrinsic = observe(
        &mut context,
        &mut artifacts,
        &container,
        &cjk,
        request(
            AvailableSpace::MinContent,
            LayoutGoal::Measure(RequestedAxis::Horizontal),
        ),
    );
    assert_size(keep_all_intrinsic.size, Size::new(64.0, 16.0));
    assert_eq!(keep_all_intrinsic.line_count, 1);
}

#[test]
fn run_spacing_line_height_and_mixed_sizes_affect_exact_geometry() {
    let mut spaced = RunStyle::ahem(16.0);
    spaced.letter_spacing = 2.0;
    spaced.font_weight = FontWeight::Bold;
    spaced.font_style = FontStyle::Italic;
    spaced.features.push((*b"kern", 0));
    spaced.variations.push((*b"wght", 700.0));
    let container = ContainerStyle::default();
    let mut context = text_context();
    let mut artifacts = ArtifactSlots::default();
    let run = [TextRun {
        text: "abc",
        style: &spaced,
        preserve_newlines: false,
    }];
    let measured = observe(
        &mut context,
        &mut artifacts,
        &container,
        &run,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_close(measured.size.width, 54.0);

    let mut factor = RunStyle::ahem(16.0);
    factor.line_height = LineHeight::Factor(2.0);
    artifacts.invalidate();
    let factor_run = [TextRun {
        text: "abc",
        style: &factor,
        preserve_newlines: false,
    }];
    let measured = observe(
        &mut context,
        &mut artifacts,
        &container,
        &factor_run,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_close(measured.size.height, 32.0);

    factor.line_height = LineHeight::Length(24.0);
    artifacts.invalidate();
    let factor_run = [TextRun {
        text: "abc",
        style: &factor,
        preserve_newlines: false,
    }];
    let measured = observe(
        &mut context,
        &mut artifacts,
        &container,
        &factor_run,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_close(measured.size.height, 24.0);

    let small = RunStyle::ahem(16.0);
    let large = RunStyle::ahem(32.0);
    artifacts.invalidate();
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
    let measured = observe(
        &mut context,
        &mut artifacts,
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
        indent: LengthPercentage::Length(16.0),
        ..ContainerStyle::default()
    };
    let mut context = text_context();
    let mut artifacts = ArtifactSlots::default();
    let text = [TextRun {
        text: "abcdefghij",
        style: &style,
        preserve_newlines: false,
    }];
    let measured = observe(
        &mut context,
        &mut artifacts,
        &container,
        &text,
        request(AvailableSpace::Definite(80.0), LayoutGoal::Commit),
    );
    assert_eq!(measured.line_count, 3);
    let committed = artifacts.committed().expect("committed indent layout");
    assert_close(
        committed
            .parley_layout()
            .get(0)
            .expect("first line")
            .metrics()
            .offset,
        16.0,
    );
    assert_close(
        committed
            .parley_layout()
            .get(1)
            .expect("second line")
            .metrics()
            .offset,
        0.0,
    );

    container.indent = LengthPercentage::ZERO;
    artifacts.invalidate();
    let collapsed = [TextRun {
        text: "ab\ncd",
        style: &style,
        preserve_newlines: false,
    }];
    let collapsed = observe(
        &mut context,
        &mut artifacts,
        &container,
        &collapsed,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_size(collapsed.size, Size::new(80.0, 16.0));

    artifacts.invalidate();
    let preserved = [TextRun {
        text: "ab\ncd",
        style: &style,
        preserve_newlines: true,
    }];
    let preserved = observe(
        &mut context,
        &mut artifacts,
        &container,
        &preserved,
        request(AvailableSpace::MaxContent, LayoutGoal::Commit),
    );
    assert_size(preserved.size, Size::new(32.0, 32.0));
    assert_eq!(preserved.line_count, 2);

    container.direction = Direction::Rtl;
    container.align = TextAlign::Start;
    artifacts.invalidate();
    let rtl = [TextRun {
        text: "אבגד",
        style: &style,
        preserve_newlines: false,
    }];
    let rtl = observe(
        &mut context,
        &mut artifacts,
        &container,
        &rtl,
        request(AvailableSpace::Definite(80.0), LayoutGoal::Commit),
    );
    assert_size(rtl.size, Size::new(64.0, 16.0));
    assert_eq!(rtl.line_count, 1);
    let line = artifacts
        .committed()
        .expect("rtl commit")
        .parley_layout()
        .get(0)
        .expect("rtl line");
    assert_close(line.metrics().offset, 0.0);
}

#[test]
fn compute_leaf_layout_adds_box_model_and_exports_first_baseline() {
    let style = RunStyle::ahem(16.0);
    let container = ContainerStyle {
        padding: Edges::uniform(LengthPercentage::Length(2.0)),
        ..ContainerStyle::default()
    };
    let runs = [TextRun {
        text: "abc",
        style: &style,
        preserve_newlines: false,
    }];
    let mut context = text_context();
    let mut artifacts = ArtifactSlots::default();
    let mut measurer = TextMeasurer::new(
        &mut context,
        &mut artifacts,
        &container,
        runs.into_iter(),
        |_, _| unreachable!("no calc()"),
    );

    let output = compute_leaf_layout(
        LayoutInput::default(),
        &container,
        |_, _| unreachable!("no calc()"),
        &mut measurer,
    );

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
enum Display {
    #[default]
    Text,
    Flex,
}

#[derive(Debug, Clone, Default)]
struct HostStyle {
    align_items: Option<AlignItems>,
}

impl CoreStyle for HostStyle {}
impl FlexItemStyle for HostStyle {}
impl TextContainerStyle for HostStyle {}

impl FlexContainerStyle for HostStyle {
    fn align_items(&self) -> Option<AlignItems> {
        self.align_items
    }
}

#[derive(Debug, Clone)]
struct SourceNode {
    display: Display,
    style: HostStyle,
    run_style: RunStyle,
    text: &'static str,
    children: Vec<NodeId>,
}

#[derive(Debug)]
struct Source {
    nodes: Vec<SourceNode>,
}

impl Source {
    fn node(&self, node: NodeId) -> &SourceNode {
        &self.nodes[usize::from(node)]
    }
}

impl TraverseTree for Source {
    type ChildIter<'a> = core::iter::Copied<core::slice::Iter<'a, NodeId>>;

    fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_> {
        self.node(parent).children.iter().copied()
    }

    fn child_count(&self, parent: NodeId) -> usize {
        self.node(parent).children.len()
    }

    fn child_id(&self, parent: NodeId, index: usize) -> NodeId {
        self.node(parent).children[index]
    }
}

impl LayoutSource for Source {
    type CoreStyle<'a> = &'a HostStyle;

    fn core_style(&self, node: NodeId) -> Self::CoreStyle<'_> {
        &self.node(node).style
    }

    fn resolve_calc(&self, _calc: CalcHandle, _basis: f32) -> f32 {
        unreachable!("host test has no calc()")
    }
}

impl FlexSource for Source {
    type ContainerStyle<'a> = &'a HostStyle;
    type ItemStyle<'a> = &'a HostStyle;

    fn flex_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_> {
        &self.node(container).style
    }

    fn flex_item_style(&self, item: NodeId) -> Self::ItemStyle<'_> {
        &self.node(item).style
    }
}

#[derive(Debug, Default)]
struct SessionNode {
    cache: Cache,
    layout: Layout,
    artifacts: ArtifactSlots,
    static_position: Point<f32>,
}

#[derive(Debug)]
struct Session {
    nodes: Vec<SessionNode>,
    text: TextContext,
    leaf_measure_calls: usize,
}

impl Session {
    fn new(node_count: usize) -> Self {
        Self {
            nodes: (0..node_count).map(|_| SessionNode::default()).collect(),
            text: text_context(),
            leaf_measure_calls: 0,
        }
    }

    fn node(&self, node: NodeId) -> &SessionNode {
        &self.nodes[usize::from(node)]
    }
}

impl LayoutState for Session {
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
        self.nodes[usize::from(node)].layout = *layout;
    }

    fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>) {
        self.nodes[usize::from(child)].static_position = static_position;
    }
}

impl CacheState for Session {
    fn cache_get(&self, node: NodeId, input: LayoutInput) -> Option<LayoutOutput> {
        self.node(node).cache.get(input)
    }

    fn cache_store(&mut self, node: NodeId, input: LayoutInput, output: LayoutOutput) {
        self.nodes[usize::from(node)].cache.store(input, output);
    }

    fn cache_clear(&mut self, node: NodeId) {
        let state = &mut self.nodes[usize::from(node)];
        state.cache.clear();
        state.artifacts.invalidate();
    }
}

impl LayoutSession<Source> for Session {
    fn compute_child_layout(
        &mut self,
        source: &Source,
        child: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput {
        let display = source.node(child).display;
        compute_cached_layout(self, child, input, |session, child, input| match display {
            Display::Flex => compute_flexbox_layout(source, session, child, input),
            Display::Text => {
                session.leaf_measure_calls += 1;
                let source_node = source.node(child);
                let run = [TextRun {
                    text: source_node.text,
                    style: &source_node.run_style,
                    preserve_newlines: false,
                }];
                let index = usize::from(child);
                let (text, nodes) = (&mut session.text, &mut session.nodes);
                let mut measurer = TextMeasurer::new(
                    text,
                    &mut nodes[index].artifacts,
                    &source_node.style,
                    run.into_iter(),
                    |_, _| unreachable!("host test has no calc()"),
                );
                compute_leaf_layout(
                    input,
                    &source_node.style,
                    |_, _| unreachable!("host test has no calc()"),
                    &mut measurer,
                )
            }
        })
    }
}

#[test]
fn flex_baseline_integration_reuses_artifacts_and_jointly_invalidates_caches() {
    let root = NodeId::from(0_usize);
    let small = NodeId::from(1_usize);
    let large = NodeId::from(2_usize);
    let source = Source {
        nodes: vec![
            SourceNode {
                display: Display::Flex,
                style: HostStyle {
                    align_items: Some(AlignItems::Baseline),
                },
                run_style: RunStyle::ahem(16.0),
                text: "",
                children: vec![small, large],
            },
            SourceNode {
                display: Display::Text,
                style: HostStyle::default(),
                run_style: RunStyle::ahem(16.0),
                text: "aaa",
                children: Vec::new(),
            },
            SourceNode {
                display: Display::Text,
                style: HostStyle::default(),
                run_style: RunStyle::ahem(32.0),
                text: "BBB",
                children: Vec::new(),
            },
        ],
    };
    let mut session = Session::new(source.nodes.len());

    compute_root_layout(&source, &mut session, root, Size::MAX_CONTENT);

    let small_state = session.node(small);
    let large_state = session.node(large);
    let small_baseline = small_state.layout.location.y
        + small_state
            .artifacts
            .committed()
            .expect("small committed text")
            .first_baseline()
            .expect("small baseline");
    let large_baseline = large_state.layout.location.y
        + large_state
            .artifacts
            .committed()
            .expect("large committed text")
            .first_baseline()
            .expect("large baseline");
    assert_close(small_baseline, large_baseline);
    assert!(small_state.artifacts.probe().is_none());
    assert!(large_state.artifacts.probe().is_none());

    let calls_after_first_layout = session.leaf_measure_calls;
    assert!(calls_after_first_layout >= 4);
    compute_root_layout(&source, &mut session, root, Size::MAX_CONTENT);
    assert_eq!(session.leaf_measure_calls, calls_after_first_layout);

    session.cache_clear(small);
    assert!(session.node(small).cache.is_empty());
    assert!(session.node(small).artifacts.probe().is_none());
    assert!(session.node(small).artifacts.committed().is_none());
    session.cache_clear(root);
    compute_root_layout(&source, &mut session, root, Size::MAX_CONTENT);
    assert!(session.leaf_measure_calls > calls_after_first_layout);
}
