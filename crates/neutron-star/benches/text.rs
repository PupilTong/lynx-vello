//! Parley shape, rebreak, and cache benchmarks tracked by CodSpeed/Divan.

use std::cell::RefCell;

use divan::counter::ItemsCount;
use neutron_star::cache::Cache;
use neutron_star::compute::{
    LeafMeasureInput, LeafMeasurement, LeafMeasurer, compute_cached_layout,
};
use neutron_star::geometry::{Point, Size};
use neutron_star::style::{CoreStyle, TextContainerStyle, TextRun, TextRunStyle};
use neutron_star::text::{ArtifactSlots, TextContext, TextMeasurer};
use neutron_star::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput,
};
use stylo::values::computed::Display;
use stylo::values::computed::font::{
    FamilyName, FontFamily, FontFamilyList, FontFamilyNameSyntax, SingleFontFamily,
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

/// The one named family every benchmark run resolves against.
fn ahem_family() -> FontFamily {
    FontFamily {
        families: FontFamilyList {
            list: stylo::ArcSlice::from_iter(std::iter::once(SingleFontFamily::FamilyName(
                FamilyName {
                    name: stylo::Atom::from("Ahem"),
                    syntax: FontFamilyNameSyntax::Identifiers,
                },
            ))),
        },
        is_system_font: false,
        is_initial: false,
    }
}

#[derive(Debug, Default)]
struct ContainerStyle;

impl CoreStyle for ContainerStyle {
    fn display(&self) -> Display {
        Display::Flex
    }
}
impl TextContainerStyle for ContainerStyle {}

#[derive(Debug)]
struct RunStyle {
    family: FontFamily,
    font_size: f32,
}

impl TextRunStyle for RunStyle {
    fn font_family(&self) -> FontFamily {
        self.family.clone()
    }

    fn font_size(&self) -> f32 {
        self.font_size
    }
}

#[derive(Debug)]
struct TextCase {
    artifacts: ArtifactSlots,
    container: ContainerStyle,
    run_styles: Vec<RunStyle>,
    spec: &'static [(&'static str, f32)],
}

impl TextCase {
    fn new(spec: &'static [(&'static str, f32)]) -> Self {
        Self {
            artifacts: ArtifactSlots::default(),
            container: ContainerStyle,
            run_styles: spec
                .iter()
                .map(|(_, font_size)| RunStyle {
                    family: ahem_family(),
                    font_size: *font_size,
                })
                .collect(),
            spec,
        }
    }

    fn measure(&mut self, context: &mut TextContext, width: f32, goal: LayoutGoal) -> Size<f32> {
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
            context,
            &mut self.artifacts,
            &self.container,
            runs.into_iter(),
        );
        measurer
            .measure(LeafMeasureInput::new(
                Size::NONE,
                Size::new(AvailableSpace::Definite(width), AvailableSpace::MaxContent),
                goal,
            ))
            .size()
    }
}

/// Independent per-node artifacts sharing the session-level text context used
/// by the production protocol. Keeping the batch in one input avoids creating
/// thousands of duplicate font collections for the sub-microsecond cases.
#[derive(Debug)]
struct TextBatch {
    context: TextContext,
    cases: Vec<TextCase>,
}

impl TextBatch {
    fn new(spec: &'static [(&'static str, f32)], batch_size: usize) -> Self {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(AHEM), 1);
        Self {
            context,
            cases: (0..batch_size).map(|_| TextCase::new(spec)).collect(),
        }
    }

    fn measure_all(&mut self, width: f32, goal: LayoutGoal) -> Size<f32> {
        let mut last = Size::new(0.0, 0.0);
        for case in &mut self.cases {
            last = divan::black_box(case.measure(&mut self.context, width, goal));
        }
        last
    }
}

fn cold(bencher: divan::Bencher<'_, '_>, spec: &'static [(&'static str, f32)], batch_size: usize) {
    bencher
        .counter(ItemsCount::new(batch_size))
        .with_inputs(|| TextBatch::new(spec, batch_size))
        .bench_local_refs(|batch| {
            divan::black_box(batch.measure_all(320.0, LayoutGoal::Commit));
        });
}

fn warm_rebreak(
    bencher: divan::Bencher<'_, '_>,
    spec: &'static [(&'static str, f32)],
    batch_size: usize,
) {
    bencher
        .counter(ItemsCount::new(batch_size))
        .with_inputs(|| {
            let mut batch = TextBatch::new(spec, batch_size);
            divan::black_box(batch.measure_all(320.0, LayoutGoal::Commit));
            batch
        })
        .bench_local_refs(|batch| {
            divan::black_box(batch.measure_all(180.0, LayoutGoal::Commit));
        });
}

macro_rules! text_benchmarks {
    ($cold:ident, $warm:ident, $spec:ident, $cold_batch:expr, $warm_batch:expr) => {
        #[divan::bench]
        fn $cold(bencher: divan::Bencher<'_, '_>) {
            cold(bencher, $spec, $cold_batch);
        }

        #[divan::bench]
        fn $warm(bencher: divan::Bencher<'_, '_>) {
            warm_rebreak(bencher, $spec, $warm_batch);
        }
    };
}

text_benchmarks!(cold_label, warm_rebreak_label, LABEL, 1_024, 8_192);
text_benchmarks!(cold_sentence, warm_rebreak_sentence, SENTENCE, 512, 2_048);
text_benchmarks!(cold_paragraph, warm_rebreak_paragraph, PARAGRAPH, 128, 512);
text_benchmarks!(cold_cjk, warm_rebreak_cjk, CJK_PARAGRAPH, 256, 2_048);
text_benchmarks!(
    cold_multi_run,
    warm_rebreak_multi_run,
    MULTI_RUN,
    256,
    1_024
);

/// A one-leaf cache-hit host: the box cache and the retained text batch live
/// in interior-mutable slots reached through the `Copy` handle [`CachedRef`].
#[derive(Debug)]
struct CachedCase {
    cache: RefCell<Cache>,
    input: LayoutInput,
    text: RefCell<TextBatch>,
}

impl CachedCase {
    fn new() -> Self {
        let mut text = TextBatch::new(PARAGRAPH, 1);
        let size = text.measure_all(320.0, LayoutGoal::Commit);
        let input = LayoutInput::perform_layout(
            Size::NONE,
            Size::NONE,
            Size::new(AvailableSpace::Definite(320.0), AvailableSpace::MaxContent),
        );
        let mut cache = Cache::default();
        cache.store(input, LayoutOutput::new(size, size));
        Self {
            cache: RefCell::new(cache),
            input,
            text: RefCell::new(text),
        }
    }

    fn hit(&mut self) -> LayoutOutput {
        let input = self.input;
        compute_cached_layout(CachedRef { case: self }, input, |node, _| {
            let size = node
                .case
                .text
                .borrow_mut()
                .measure_all(320.0, LayoutGoal::Commit);
            LayoutOutput::new(size, size)
        })
    }
}

#[derive(Clone, Copy)]
struct CachedRef<'t> {
    case: &'t CachedCase,
}

impl std::fmt::Debug for CachedRef<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CachedRef")
    }
}

impl<'t> LayoutNode for CachedRef<'t> {
    type Style = &'t ContainerStyle;
    type ChildIter = std::iter::Empty<Self>;

    fn children(self) -> Self::ChildIter {
        std::iter::empty()
    }

    fn style(self) -> Self::Style {
        &ContainerStyle
    }

    fn compute_child_layout(self, _input: LayoutInput) -> LayoutOutput {
        unreachable!("the cache-hit benchmark drives compute_cached_layout directly")
    }

    fn set_unrounded_layout(self, _layout: &Layout) {
        unreachable!("the cache-hit benchmark stores no durable geometry")
    }

    fn unrounded_layout(self) -> Layout {
        unreachable!("the cache-hit benchmark stores no durable geometry")
    }

    fn set_final_layout(self, _layout: &Layout) {
        unreachable!("the cache-hit benchmark stores no durable geometry")
    }

    fn set_static_position(self, _static_position: Point<f32>) {
        unreachable!("the cache-hit benchmark stores no durable geometry")
    }

    fn cache_get(self, input: LayoutInput) -> Option<LayoutOutput> {
        self.case.cache.borrow().get(input)
    }

    fn cache_store(self, input: LayoutInput, output: LayoutOutput) {
        self.case.cache.borrow_mut().store(input, output);
    }

    fn cache_clear(self) {
        self.case.cache.borrow_mut().clear();
    }
}

const CACHE_HIT_BATCH: usize = 524_288;

#[divan::bench]
fn committed_box_cache_hit(bencher: divan::Bencher<'_, '_>) {
    bencher
        .counter(ItemsCount::new(CACHE_HIT_BATCH))
        .with_inputs(CachedCase::new)
        .bench_local_values(|mut case| {
            for _ in 0..CACHE_HIT_BATCH {
                divan::black_box(divan::black_box(&mut case).hit());
            }
            case
        });
}
