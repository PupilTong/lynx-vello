//! Parley shape, rebreak, and cache benchmarks tracked by CodSpeed/Divan.

use neutron_star::cache::Cache;
use neutron_star::compute::{
    LeafMeasureInput, LeafMeasurement, LeafMeasurer, compute_cached_layout,
};
use neutron_star::geometry::Size;
use neutron_star::style::{
    CoreStyle, FontFamily, FontFeatureSetting, FontVariationSetting, TextContainerStyle, TextRun,
    TextRunStyle,
};
use neutron_star::text::{ArtifactSlots, TextContext, TextMeasurer};
use neutron_star::tree::{
    AvailableSpace, CacheState, LayoutGoal, LayoutInput, LayoutOutput, NodeId,
};

const AHEM: &[u8] = include_bytes!("../tests/fixtures/Ahem.ttf");
const LABEL: &[(&str, f32)] = &[("Settings", 16.0)];
const SENTENCE: &[(&str, f32)] = &[("The quick brown fox jumps over the lazy dog.", 16.0)];
const PARAGRAPH: &[(&str, f32)] = &[(
    "Text measurement shapes this paragraph once and repeatedly breaks the retained glyph and cluster data across different inline constraints. The benchmark includes enough words to exercise ordinary and emergency line breaking.",
    16.0,
)];
const CJK_PARAGRAPH: &[(&str, f32)] = &[(
    "排版引擎需要处理复杂文字、自动换行和双向文本。这个基准覆盖中文分词与复杂脚本路径，并在多个宽度之间重复布局。",
    16.0,
)];
const MULTI_RUN: &[(&str, f32)] = &[
    ("A mixed paragraph starts small, ", 14.0),
    ("emphasizes a larger middle run, ", 24.0),
    ("and returns to its label size.", 14.0),
];

fn main() {
    divan::main();
}

#[derive(Debug, Default)]
struct ContainerStyle;

impl CoreStyle for ContainerStyle {}
impl TextContainerStyle for ContainerStyle {}

#[derive(Debug)]
struct RunStyle {
    font_size: f32,
}

impl TextRunStyle for RunStyle {
    type FontFamilies<'a> = core::iter::Once<FontFamily<'a>>;
    type FontFeatureSettings<'a> = core::iter::Empty<FontFeatureSetting>;
    type FontVariationSettings<'a> = core::iter::Empty<FontVariationSetting>;

    fn font_families(&self) -> Self::FontFamilies<'_> {
        core::iter::once(FontFamily::Named("Ahem"))
    }

    fn font_size(&self) -> f32 {
        self.font_size
    }

    fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
        core::iter::empty()
    }

    fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
        core::iter::empty()
    }
}

#[derive(Debug)]
struct TextCase {
    context: TextContext,
    artifacts: ArtifactSlots,
    container: ContainerStyle,
    run_styles: Vec<RunStyle>,
    spec: &'static [(&'static str, f32)],
}

impl TextCase {
    fn new(spec: &'static [(&'static str, f32)]) -> Self {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(AHEM), 1);
        Self {
            context,
            artifacts: ArtifactSlots::default(),
            container: ContainerStyle,
            run_styles: spec
                .iter()
                .map(|(_, font_size)| RunStyle {
                    font_size: *font_size,
                })
                .collect(),
            spec,
        }
    }

    fn measure(&mut self, width: f32, goal: LayoutGoal) -> Size<f32> {
        let runs = self
            .spec
            .iter()
            .zip(&self.run_styles)
            .map(|((text, _), style)| TextRun {
                text,
                style,
                preserve_newlines: false,
            })
            .collect::<Vec<_>>();
        let mut measurer = TextMeasurer::new(
            &mut self.context,
            &mut self.artifacts,
            &self.container,
            runs.into_iter(),
            |_, _| unreachable!("benchmark styles contain no calc()"),
        );
        measurer
            .measure(LeafMeasureInput::new(
                Size::NONE,
                Size::new(Some(width), None),
                Size::new(AvailableSpace::Definite(width), AvailableSpace::MaxContent),
                goal,
            ))
            .size()
    }
}

fn cold(bencher: divan::Bencher<'_, '_>, spec: &'static [(&'static str, f32)]) {
    bencher
        .with_inputs(|| TextCase::new(spec))
        .bench_local_values(|mut case| {
            divan::black_box(case.measure(320.0, LayoutGoal::Commit));
        });
}

fn warm_rebreak(bencher: divan::Bencher<'_, '_>, spec: &'static [(&'static str, f32)]) {
    bencher
        .with_inputs(|| {
            let mut case = TextCase::new(spec);
            divan::black_box(case.measure(320.0, LayoutGoal::Commit));
            case
        })
        .bench_local_values(|mut case| {
            divan::black_box(case.measure(180.0, LayoutGoal::Commit));
        });
}

macro_rules! text_benchmarks {
    ($cold:ident, $warm:ident, $spec:ident) => {
        #[divan::bench]
        fn $cold(bencher: divan::Bencher<'_, '_>) {
            cold(bencher, $spec);
        }

        #[divan::bench]
        fn $warm(bencher: divan::Bencher<'_, '_>) {
            warm_rebreak(bencher, $spec);
        }
    };
}

text_benchmarks!(cold_label, warm_rebreak_label, LABEL);
text_benchmarks!(cold_sentence, warm_rebreak_sentence, SENTENCE);
text_benchmarks!(cold_paragraph, warm_rebreak_paragraph, PARAGRAPH);
text_benchmarks!(cold_cjk, warm_rebreak_cjk, CJK_PARAGRAPH);
text_benchmarks!(cold_multi_run, warm_rebreak_multi_run, MULTI_RUN);

#[derive(Debug)]
struct CachedCase {
    cache: Cache,
    input: LayoutInput,
    text: TextCase,
}

impl CachedCase {
    fn new() -> Self {
        let mut text = TextCase::new(PARAGRAPH);
        let size = text.measure(320.0, LayoutGoal::Commit);
        let input = LayoutInput::perform_layout(
            Size::NONE,
            Size::NONE,
            Size::new(AvailableSpace::Definite(320.0), AvailableSpace::MaxContent),
        );
        let mut cache = Cache::default();
        cache.store(input, LayoutOutput::new(size, size));
        Self { cache, input, text }
    }

    fn hit(&mut self) -> LayoutOutput {
        let input = self.input;
        compute_cached_layout(self, NodeId::new(0), input, |case, _, _| {
            let size = case.text.measure(320.0, LayoutGoal::Commit);
            LayoutOutput::new(size, size)
        })
    }
}

impl CacheState for CachedCase {
    fn cache_get(&self, _node: NodeId, input: LayoutInput) -> Option<LayoutOutput> {
        self.cache.get(input)
    }

    fn cache_store(&mut self, _node: NodeId, input: LayoutInput, output: LayoutOutput) {
        self.cache.store(input, output);
    }

    fn cache_clear(&mut self, _node: NodeId) {
        self.cache.clear();
    }
}

#[divan::bench]
fn committed_box_cache_hit(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(CachedCase::new)
        .bench_local_values(|mut case| divan::black_box(case.hit()));
}
