//! Lynx text-block layout benchmarks tracked by CodSpeed/Divan.
//!
//! Every scenario is lifted from the reference test suites so the workloads
//! measure content shapes the engine actually faces:
//!
//! - `lynx-stack/packages/web-platform/web-elements/tests/fixtures/x-text/` (`text-maxline-basic`,
//!   `text-maxline-with-inline-view-and-custom- truncation`, `truncation-first-element-is-image`,
//!   `inline-truncation-with-inline-image`, `text-maxlength`,
//!   `text-maxline-with-custom-truncation-innertext-change`);
//! - `lynx-stack/packages/web-platform/web-core-e2e/tests/reactlynx/`
//!   (`basic-element-text-word-break`, `basic-element-text-maxline`);
//! - the per-node throughput content of `tests/fixtures/performance/x-text-large.html`;
//! - the paragraph benchmarks the native repo vendors (Lynx itself ships no text benchmarks; its
//!   Textra engine is an external binary):
//!   `lynx/clay/third_party/txt/benchmarks/paragraph_benchmarks.cc` (`ShortLayout`, `LongLayout`,
//!   `JustifyLayout`, `ManyStylesLayout`, `RepeatLayoutParagraph`'s 300→600 relayout) and the
//!   upstream unit scenarios `ChineseParagraph` (no-space CJK + justify + letter-spacing) and the
//!   `InlinePlaceholder` family (the stand-in for Lynx inline-image/inline-view).
//!
//! Three cost tiers of the retained [`TextBlock`] are measured separately:
//! `cold` pays flatten + normalize + shape + break; `rebreak` re-breaks the
//! retained layout at an alternating width (plus, when a cut exists, the
//! bounded display re-shape); `resize` drives the Lynx measure/align round
//! trip through `set_box_size`.

use core::num::NonZeroU32;

use divan::counter::ItemsCount;
use seldon::geometry::Size;
use seldon::text::block::{
    BlockConstraint, BlockStyle, InlineBoxSpec, InlineItem, RunStyle, TextAlign, TextBlock,
    TextOverflow, TextRunItem, TextSecurity, VerticalAlign, WhiteSpaceCollapse, WordBreak,
};
use seldon::text::{FontBlob, TextContext};
use stylo::values::computed::font::{
    FamilyName, FontFamily, FontFamilyList, FontFamilyNameSyntax, SingleFontFamily,
};

const AHEM: &[u8] = include_bytes!("../tests/fixtures/Ahem.ttf");

fn main() {
    divan::main();
}

enum ItemSpec {
    Run {
        text: &'static str,
        font_size: f32,
        letter_spacing: f32,
    },
    Box {
        width: f32,
        height: f32,
        vertical_align: VerticalAlign,
    },
}

const fn run(text: &'static str, font_size: f32) -> ItemSpec {
    ItemSpec::Run {
        text,
        font_size,
        letter_spacing: 0.0,
    }
}

const fn spaced_run(text: &'static str, font_size: f32, letter_spacing: f32) -> ItemSpec {
    ItemSpec::Run {
        text,
        font_size,
        letter_spacing,
    }
}

const fn boxed(width: f32, height: f32) -> ItemSpec {
    aligned_box(width, height, VerticalAlign::Baseline)
}

const fn aligned_box(width: f32, height: f32, vertical_align: VerticalAlign) -> ItemSpec {
    ItemSpec::Box {
        width,
        height,
        vertical_align,
    }
}

struct Spec {
    items: &'static [ItemSpec],
    truncation: Option<&'static [ItemSpec]>,
    max_lines: Option<u32>,
    max_chars: Option<u32>,
    overflow: TextOverflow,
    word_break: WordBreak,
    text_align: TextAlign,
    width: f32,
    alternate_width: f32,
}

impl Spec {
    const fn plain(items: &'static [ItemSpec], width: f32, alternate_width: f32) -> Self {
        Self {
            items,
            truncation: None,
            max_lines: None,
            max_chars: None,
            overflow: TextOverflow::Clip,
            word_break: WordBreak::Normal,
            text_align: TextAlign::Start,
            width,
            alternate_width,
        }
    }
}

const LABEL: &[ItemSpec] = &[run("hello lynx", 16.0)];

const PLAIN_PARAGRAPH: &[ItemSpec] = &[run(
    "The layout of the text component is different from that of the view component. It does not \
     support setting display and related properties for layout, and has its own text layout \
     method internally. Currently, native layout and rendering are used.",
    16.0,
)];

const STYLED_MULTI_RUN: &[ItemSpec] = &[
    run(
        "Nhello world, this is a long enough text without any limitation.",
        16.0,
    ),
    run("we could use inline-text to set color of some text", 16.0),
    run("also, font-size could be different", 48.0),
    spaced_run("additionally, letter-space could be different", 16.0, 7.0),
];

const WORD_BREAK_MATRIX: &[&[ItemSpec]] = &[
    &[run("12345678901234567890", 16.0)],
    &[run("12345 67890 12345 67890", 16.0)],
    &[run("你好世界你好世界你好世界你好世界", 16.0)],
    &[run("abcdefghijklmnopqrstu", 16.0)],
    &[run("abcde fghij klmno pqrstu", 16.0)],
    &[run("你好世界 你好世界 你好世界 你好世界", 16.0)],
];

const CJK_ANNOUNCEMENT: &[ItemSpec] = &[run(
    "活动规则：在04.17-05.17期间，带话题#暑假科普手抄报 发布优质原创图文作品，图片数量≥3张，上榜作者能赢取周边",
    13.0,
)];

const DENSE_CJK_LEADING_IMAGE: &[ItemSpec] = &[
    boxed(32.0, 18.0),
    run("大学大学大学大学大学", 24.0),
    run(
        "大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学\
         大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学大学",
        24.0,
    ),
];

const AVATAR_QUOTE: &[ItemSpec] = &[
    boxed(25.0, 18.0),
    run(
        "零跑C10开了17000公里，8295车机流畅，智享版辅助驾驶完全够用，没必要硬上顶配。",
        15.0,
    ),
    boxed(17.0, 17.0),
];

const AVATAR_QUOTE_TRUNCATION: &[ItemSpec] = &[run("更多", 15.0), boxed(12.0, 12.0)];

const NESTED_MAXLENGTH: &[ItemSpec] = &[
    run("1", 16.0),
    run("2", 16.0),
    run("3", 16.0),
    run("4", 16.0),
    run("5", 16.0),
    run("简", 16.0),
    run("体", 16.0),
    run("中文", 16.0),
];

const LONG_WORD_STRESS: &[ItemSpec] = &[run(
    "long long longlong long long longlong long long longlong long long longlong long long \
     longlong long long longlong long long longlong long long longlong long long longlong long \
     long longlong long long longlong long long longlong long long longlong long long longlong \
     long long longlong long long longlong long long longlong long long longlong long long \
     longlong long long longlong long long longlong long long longlong long long longlong long \
     long longlong long long longlong long long longlong long long longlong long long longlong \
     long long longlong long long longlong long long longlong",
    16.0,
)];

const LONG_WORD_TRUNCATION: &[ItemSpec] = &[run("inline-truncation", 16.0)];

const MAXLINE_STYLED: Spec = Spec {
    text_align: TextAlign::Start,
    items: STYLED_MULTI_RUN,
    truncation: None,
    max_lines: Some(2),
    max_chars: None,
    overflow: TextOverflow::Ellipsis,
    word_break: WordBreak::BreakAll,
    width: 300.0,
    alternate_width: 200.0,
};

const MAXLINE_CJK_DENSE: Spec = Spec {
    text_align: TextAlign::Start,
    items: DENSE_CJK_LEADING_IMAGE,
    truncation: None,
    max_lines: Some(3),
    max_chars: None,
    overflow: TextOverflow::Ellipsis,
    word_break: WordBreak::Normal,
    width: 300.0,
    alternate_width: 260.0,
};

const INLINE_TRUNCATION_FIT: Spec = Spec {
    text_align: TextAlign::Start,
    items: AVATAR_QUOTE,
    truncation: Some(AVATAR_QUOTE_TRUNCATION),
    max_lines: Some(2),
    max_chars: None,
    overflow: TextOverflow::Ellipsis,
    word_break: WordBreak::Normal,
    width: 375.0,
    alternate_width: 320.0,
};

const MAXLINE_ANNOUNCEMENT: Spec = Spec {
    text_align: TextAlign::Start,
    items: CJK_ANNOUNCEMENT,
    truncation: None,
    max_lines: Some(1),
    max_chars: None,
    overflow: TextOverflow::Ellipsis,
    word_break: WordBreak::Normal,
    width: 390.0,
    alternate_width: 300.0,
};

const MAXLENGTH_NESTED: Spec = Spec {
    text_align: TextAlign::Start,
    items: NESTED_MAXLENGTH,
    truncation: None,
    max_lines: None,
    max_chars: Some(6),
    overflow: TextOverflow::Ellipsis,
    word_break: WordBreak::Normal,
    width: 300.0,
    alternate_width: 200.0,
};

const BREAK_ALL_TRUNCATION: Spec = Spec {
    text_align: TextAlign::Start,
    items: LONG_WORD_STRESS,
    truncation: Some(LONG_WORD_TRUNCATION),
    max_lines: Some(2),
    max_chars: None,
    overflow: TextOverflow::Ellipsis,
    word_break: WordBreak::BreakAll,
    width: 300.0,
    alternate_width: 240.0,
};

const LONG_LATIN: &[ItemSpec] = &[run(
    concat!(
        "This is a very long sentence to test if the text will properly wrap around and go to the ",
        "next line. Sometimes, short sentence. Longer sentences are okay too because they are ",
        "necessary. Very short. ",
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ",
        "ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ",
        "ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in ",
        "reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur ",
        "sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id ",
        "est laborum. ",
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ",
        "ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ",
        "ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in ",
        "reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur ",
        "sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id ",
        "est laborum.",
    ),
    16.0,
)];

const CJK_NO_SPACE: &[ItemSpec] = &[spaced_run(
    concat!(
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
        "左線読設重説切後碁給能上目秘使約。",
    ),
    35.0,
    2.0,
)];

const PLACEHOLDER_HEAVY: &[ItemSpec] = &[
    run("012 34", 16.0),
    aligned_box(50.0, 50.0, VerticalAlign::Baseline),
    run("012 34", 16.0),
    aligned_box(5.0, 50.0, VerticalAlign::Middle),
    run("012 34", 16.0),
    aligned_box(50.0, 50.0, VerticalAlign::Top),
    run("012 34", 16.0),
    aligned_box(5.0, 50.0, VerticalAlign::Bottom),
    run("012 34", 16.0),
    aligned_box(50.0, 50.0, VerticalAlign::TextBottom),
    run("012 34", 16.0),
    aligned_box(5.0, 50.0, VerticalAlign::Baseline),
    run("012 34", 16.0),
    aligned_box(50.0, 50.0, VerticalAlign::Middle),
    run("012 34", 16.0),
    aligned_box(5.0, 50.0, VerticalAlign::Top),
    run("012 34", 16.0),
    aligned_box(50.0, 50.0, VerticalAlign::Bottom),
    run("012 34", 16.0),
    aligned_box(5.0, 50.0, VerticalAlign::TextBottom),
    run("012 34", 16.0),
    aligned_box(50.0, 50.0, VerticalAlign::Baseline),
    run("012 34", 16.0),
    aligned_box(5.0, 50.0, VerticalAlign::Middle),
    run("012 34", 16.0),
    aligned_box(50.0, 50.0, VerticalAlign::Top),
    run("012 34", 16.0),
    aligned_box(5.0, 50.0, VerticalAlign::Bottom),
    run("012 34", 16.0),
    aligned_box(50.0, 50.0, VerticalAlign::TextBottom),
    run("012 34", 16.0),
];

static WORD_BREAK_SPECS: [Spec; 6] = [
    Spec::plain(WORD_BREAK_MATRIX[0], 100.0, 100.0),
    Spec::plain(WORD_BREAK_MATRIX[1], 100.0, 100.0),
    Spec::plain(WORD_BREAK_MATRIX[2], 100.0, 100.0),
    Spec::plain(WORD_BREAK_MATRIX[3], 100.0, 100.0),
    Spec::plain(WORD_BREAK_MATRIX[4], 100.0, 100.0),
    Spec::plain(WORD_BREAK_MATRIX[5], 100.0, 100.0),
];

const PLAIN_LONG_LATIN: Spec = Spec::plain(LONG_LATIN, 300.0, 600.0);

const JUSTIFY_LONG_LATIN: Spec = Spec {
    text_align: TextAlign::Justify,
    ..Spec::plain(LONG_LATIN, 300.0, 600.0)
};

const JUSTIFY_CJK_NO_SPACE: Spec = Spec {
    text_align: TextAlign::Justify,
    ..Spec::plain(CJK_NO_SPACE, 900.0, 600.0)
};

const PLACEHOLDER_SPEC: Spec = Spec::plain(PLACEHOLDER_HEAVY, 300.0, 500.0);

const PLAIN_LABEL: Spec = Spec::plain(LABEL, 100.0, 80.0);
const PLAIN_PARAGRAPH_SPEC: Spec = Spec::plain(PLAIN_PARAGRAPH, 300.0, 600.0);
const PLAIN_STYLED: Spec = Spec::plain(STYLED_MULTI_RUN, 300.0, 200.0);
const PLAIN_ANNOUNCEMENT: Spec = Spec::plain(CJK_ANNOUNCEMENT, 390.0, 300.0);

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

fn block_style(spec: &Spec) -> BlockStyle {
    BlockStyle {
        overflow: spec.overflow,
        max_lines: spec.max_lines.and_then(NonZeroU32::new),
        max_chars: spec.max_chars,
        word_break: spec.word_break,
        text_align: spec.text_align,
        ..BlockStyle::default()
    }
}

fn run_styles(items: &[ItemSpec]) -> Vec<RunStyle> {
    items
        .iter()
        .filter_map(|item| match item {
            ItemSpec::Run {
                font_size,
                letter_spacing,
                ..
            } => Some(RunStyle {
                font_family: ahem_family(),
                font_size: *font_size,
                letter_spacing: *letter_spacing,
                ..RunStyle::default()
            }),
            ItemSpec::Box { .. } => None,
        })
        .collect()
}

fn inline_items<'case>(
    items: &'static [ItemSpec],
    styles: &'case [RunStyle],
    first_box_id: u64,
) -> Vec<InlineItem<'case>> {
    let mut style_index = 0;
    let mut box_id = first_box_id;
    items
        .iter()
        .map(|item| match item {
            ItemSpec::Run { text, .. } => {
                let style = &styles[style_index];
                style_index += 1;
                InlineItem::Run(TextRunItem {
                    text,
                    style,
                    white_space: WhiteSpaceCollapse::Collapse,
                    text_security: TextSecurity::None,
                })
            }
            ItemSpec::Box {
                width,
                height,
                vertical_align,
            } => {
                let spec = InlineBoxSpec {
                    id: box_id,
                    size: Size::new(*width, *height),
                    baseline: None,
                    vertical_align: *vertical_align,
                };
                box_id += 1;
                InlineItem::Box(spec)
            }
        })
        .collect()
}

struct Case {
    spec: &'static Spec,
    styles: Vec<RunStyle>,
    truncation_styles: Vec<RunStyle>,
}

impl Case {
    fn new(spec: &'static Spec) -> Self {
        Self {
            spec,
            styles: run_styles(spec.items),
            truncation_styles: spec.truncation.map(run_styles).unwrap_or_default(),
        }
    }

    fn build(&self, context: &mut TextContext) -> TextBlock {
        let items = inline_items(self.spec.items, &self.styles, 0);
        let truncation = self
            .spec
            .truncation
            .map(|spec| inline_items(spec, &self.truncation_styles, 1_000));
        TextBlock::new(
            context,
            block_style(self.spec),
            &items,
            truncation.as_deref(),
        )
    }
}

struct Batch {
    context: TextContext,
    cases: Vec<Case>,
    blocks: Vec<TextBlock>,
}

impl Batch {
    fn new(spec: &'static Spec, batch_size: usize) -> Self {
        let mut context = TextContext::without_system_fonts();
        assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
        Self {
            context,
            cases: (0..batch_size).map(|_| Case::new(spec)).collect(),
            blocks: Vec::new(),
        }
    }

    fn build_and_layout_all(&mut self, width: f32) -> Size<f32> {
        self.blocks.clear();
        let mut last = Size::new(0.0, 0.0);
        for case in &self.cases {
            let mut block = case.build(&mut self.context);
            block.commit(&mut self.context, BlockConstraint::new(Some(width), 0.0));
            last = divan::black_box(block.size());
            self.blocks.push(block);
        }
        last
    }

    fn layout_all(&mut self, width: f32) -> Size<f32> {
        let mut last = Size::new(0.0, 0.0);
        for block in &mut self.blocks {
            block.commit(&mut self.context, BlockConstraint::new(Some(width), 0.0));
            last = divan::black_box(block.size());
        }
        last
    }

    fn resize_boxes_all(&mut self, size: Size<f32>, width: f32) -> Size<f32> {
        let mut last = Size::new(0.0, 0.0);
        for block in &mut self.blocks {
            block.set_box_size(0, size, None);
            block.commit(&mut self.context, BlockConstraint::new(Some(width), 0.0));
            last = divan::black_box(block.size());
        }
        last
    }
}

fn cold(bencher: divan::Bencher<'_, '_>, spec: &'static Spec, batch_size: usize) {
    bencher
        .counter(ItemsCount::new(batch_size))
        .with_inputs(|| Batch::new(spec, batch_size))
        .bench_local_refs(|batch| {
            divan::black_box(batch.build_and_layout_all(spec.width));
        });
}

fn rebreak(bencher: divan::Bencher<'_, '_>, spec: &'static Spec, batch_size: usize) {
    bencher
        .counter(ItemsCount::new(batch_size))
        .with_inputs(|| {
            let mut batch = Batch::new(spec, batch_size);
            divan::black_box(batch.build_and_layout_all(spec.width));
            divan::black_box(batch.layout_all(spec.alternate_width));
            (batch, false)
        })
        .bench_local_refs(|(batch, at_alternate)| {
            let width = if *at_alternate {
                spec.alternate_width
            } else {
                spec.width
            };
            *at_alternate = !*at_alternate;
            divan::black_box(batch.layout_all(width));
        });
}

fn steady(bencher: divan::Bencher<'_, '_>, spec: &'static Spec, batch_size: usize) {
    bencher
        .counter(ItemsCount::new(batch_size))
        .with_inputs(|| {
            let mut batch = Batch::new(spec, batch_size);
            divan::black_box(batch.build_and_layout_all(spec.width));
            batch
        })
        .bench_local_refs(|batch| {
            divan::black_box(batch.layout_all(spec.width));
        });
}

macro_rules! block_benchmarks {
    ($cold:ident, $rebreak:ident, $spec:ident, $cold_batch:expr, $warm_batch:expr) => {
        #[divan::bench]
        fn $cold(bencher: divan::Bencher<'_, '_>) {
            cold(bencher, &$spec, $cold_batch);
        }

        #[divan::bench]
        fn $rebreak(bencher: divan::Bencher<'_, '_>) {
            rebreak(bencher, &$spec, $warm_batch);
        }
    };
}

block_benchmarks!(cold_label, rebreak_label, PLAIN_LABEL, 1_024, 8_192);
block_benchmarks!(
    cold_plain_paragraph,
    rebreak_plain_paragraph,
    PLAIN_PARAGRAPH_SPEC,
    128,
    512
);
block_benchmarks!(
    cold_styled_multi_run,
    rebreak_styled_multi_run,
    PLAIN_STYLED,
    128,
    512
);
block_benchmarks!(
    cold_cjk_announcement,
    rebreak_cjk_announcement,
    PLAIN_ANNOUNCEMENT,
    256,
    1_024
);
block_benchmarks!(
    cold_maxline_styled,
    rebreak_maxline_styled,
    MAXLINE_STYLED,
    128,
    256
);
block_benchmarks!(
    cold_maxline_cjk_dense,
    rebreak_maxline_cjk_dense,
    MAXLINE_CJK_DENSE,
    128,
    256
);
block_benchmarks!(
    cold_maxline_announcement,
    rebreak_maxline_announcement,
    MAXLINE_ANNOUNCEMENT,
    256,
    512
);
block_benchmarks!(
    cold_inline_truncation_fit,
    rebreak_inline_truncation_fit,
    INLINE_TRUNCATION_FIT,
    128,
    256
);
block_benchmarks!(
    cold_maxlength_nested,
    rebreak_maxlength_nested,
    MAXLENGTH_NESTED,
    256,
    1_024
);
block_benchmarks!(
    cold_break_all_truncation,
    rebreak_break_all_truncation,
    BREAK_ALL_TRUNCATION,
    64,
    128
);
block_benchmarks!(
    cold_long_latin,
    rebreak_long_latin,
    PLAIN_LONG_LATIN,
    32,
    128
);
block_benchmarks!(
    cold_justify_long_latin,
    rebreak_justify_long_latin,
    JUSTIFY_LONG_LATIN,
    32,
    128
);
block_benchmarks!(
    cold_justify_cjk_no_space,
    rebreak_justify_cjk_no_space,
    JUSTIFY_CJK_NO_SPACE,
    64,
    256
);
block_benchmarks!(
    cold_placeholder_heavy,
    rebreak_placeholder_heavy,
    PLACEHOLDER_SPEC,
    128,
    512
);

#[divan::bench]
fn cold_thousand_style_runs(bencher: divan::Bencher<'_, '_>) {
    const RUNS: usize = 1_000;
    const BATCH: usize = 8;
    bencher
        .counter(ItemsCount::new(BATCH))
        .with_inputs(|| {
            let mut context = TextContext::without_system_fonts();
            assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
            let styles: Vec<RunStyle> = (0..RUNS)
                .map(|_| RunStyle {
                    font_family: ahem_family(),
                    font_size: 16.0,
                    ..RunStyle::default()
                })
                .collect();
            (context, styles)
        })
        .bench_local_refs(|(context, styles)| {
            for _ in 0..BATCH {
                let items: Vec<InlineItem<'_>> = styles
                    .iter()
                    .map(|style| {
                        InlineItem::Run(TextRunItem {
                            text: "-",
                            style,
                            white_space: WhiteSpaceCollapse::Collapse,
                            text_security: TextSecurity::None,
                        })
                    })
                    .collect();
                let mut block = TextBlock::new(context, BlockStyle::default(), &items, None);
                block.commit(context, BlockConstraint::new(Some(300.0), 0.0));
                divan::black_box(block.size());
            }
        });
}

#[divan::bench]
fn steady_plain_paragraph(bencher: divan::Bencher<'_, '_>) {
    steady(bencher, &PLAIN_PARAGRAPH_SPEC, 2_048);
}

#[divan::bench]
fn steady_inline_truncation_fit(bencher: divan::Bencher<'_, '_>) {
    steady(bencher, &INLINE_TRUNCATION_FIT, 1_024);
}

#[divan::bench]
fn resize_avatar_box(bencher: divan::Bencher<'_, '_>) {
    const BATCH: usize = 256;
    bencher
        .counter(ItemsCount::new(BATCH))
        .with_inputs(|| {
            let mut batch = Batch::new(&INLINE_TRUNCATION_FIT, BATCH);
            divan::black_box(batch.build_and_layout_all(INLINE_TRUNCATION_FIT.width));
            (batch, false)
        })
        .bench_local_refs(|(batch, grown)| {
            let side = if *grown { 24.0 } else { 18.0 };
            *grown = !*grown;
            divan::black_box(
                batch.resize_boxes_all(Size::new(side, side), INLINE_TRUNCATION_FIT.width),
            );
        });
}

#[divan::bench]
fn cold_word_break_matrix(bencher: divan::Bencher<'_, '_>) {
    const BATCH: usize = 64;
    struct MatrixBatch {
        context: TextContext,
        cases: Vec<Vec<Case>>,
    }
    bencher
        .counter(ItemsCount::new(BATCH * WORD_BREAK_MATRIX.len()))
        .with_inputs(|| {
            let mut context = TextContext::without_system_fonts();
            assert_eq!(context.register_fonts(FontBlob::from_static(AHEM)), 1);
            MatrixBatch {
                context,
                cases: (0..BATCH)
                    .map(|_| WORD_BREAK_SPECS.iter().map(Case::new).collect())
                    .collect(),
            }
        })
        .bench_local_refs(|batch| {
            for row in &batch.cases {
                for case in row {
                    let mut block = case.build(&mut batch.context);
                    block.commit(&mut batch.context, BlockConstraint::new(Some(100.0), 0.0));
                    divan::black_box(block.size());
                }
            }
        });
}
