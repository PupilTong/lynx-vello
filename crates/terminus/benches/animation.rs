//! Animation frame benchmarks: what one frame of a running `@keyframes`
//! animation costs, CPU-side only (no GPU dispatch).
//!
//! One iteration is one frame, so the reported time reads directly as per-frame
//! cost. Four axes matter:
//!
//! - **How much of a main-thread frame is the animation.** `frame_*` advances the timeline and
//!   produces the scene on the document's owner thread — the Lynx main thread — the path of every
//!   animation the commit does not export as a composite curve, and of an exported one at its
//!   boundaries. `tick_*` measures only `Document::advance_animations`; the gap between the two is
//!   the scene rebuild.
//! - **What a production frame costs.** `frame_card_text_*` and `frame_shimmer_rows` run the frame
//!   the painter runs: a main-thread tick only when the committed frame asks for one, then the
//!   composition at the frame's instant. An exported curve makes that a composition and nothing
//!   else: per composed curve, stylo's own `progress_at`/`sample_at` over the cloned animation
//!   state into one value map the pass reuses, and for a transform one f32 world fold.
//! - **How the animation scales.** The `args` are how many of the page's elements animate. A
//!   frame's animation cost should be O(animating), not O(document), and a composed frame's cost
//!   O(what can reach the screen).
//! - **What a reflow costs.** `frame_transform` cannot move a box and never reaches layout;
//!   `frame_width` does both. Their difference is the reflow the paint-only path avoids.
//! - **What a scroll-driven frame costs.** `frame_scroll_driven_composite` composes a list whose
//!   rows and header animate on scroll timelines at a moving offset, with no clock and no tick: per
//!   composed curve, `iteration_progress` from the offset, then stylo's `sample_at`.
//!   `frame_scroll_driven_main` is the path of a dependent that does not export: the adopted
//!   offset, `Document::advance_scroll_timelines`, and the commit its restyle owes.

use std::cell::{Cell, RefCell};

use euclid::{Scale, Size2D};
use stylo::device::servo::FontMetricsProvider;
use stylo::font_metrics::FontMetrics;
use stylo::media_queries::MediaType;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::queries::values::PrefersColorScheme;
use stylo::servo::media_features::PointerCapabilities;
use stylo::values::computed::font::GenericFontFamily;
use stylo::values::computed::{CSSPixelLength, Length};
use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
use stylo_traits::{CSSPixel, DevicePixel};
use terminus::link::{CommittedFrame, DisplayList, Document, FontBlob, StylesheetOrigin};

fn main() {
    divan::main();
}

const CARDS: usize = 120;

const SHIMMER_ROWS: usize = 200;

const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");

const FRAME_STEP: f64 = 1.0 / 60.0;

#[derive(Debug)]
struct BenchFontMetricsProvider;

impl FontMetricsProvider for BenchFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        base_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics {
            ascent: Length::new(base_size.px()),
            ..FontMetrics::default()
        }
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        Length::new(FONT_MEDIUM_PX)
    }
}

fn device() -> terminus::link::Device {
    terminus::link::standards_device(
        MediaType::screen(),
        Size2D::<f32, CSSPixel>::new(800.0, 600.0),
        Size2D::<f32, DevicePixel>::new(800.0, 600.0),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(BenchFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

const PAGE_CSS: &str = "
page { display: flex; position: relative; width: 800px; height: 600px; }
.card { display: flex; position: absolute; width: 180px; height: 80px;
        background-color: #f6f6f8; border: 2px solid #cccccc;
        border-radius: 10px; box-shadow: 0px 2px 6px rgba(0,0,0,0.25); }
.dim { opacity: 0.85; }
.clip { overflow: hidden; }
.chip { display: flex; width: 60px; height: 20px;
        background-color: #3366ff; border-radius: 10px; }

@keyframes bench-spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
@keyframes bench-fade { from { opacity: 1; } to { opacity: 0.2; } }
@keyframes bench-grow { from { width: 180px; } to { width: 260px; } }

.spin { animation: bench-spin 600s linear infinite; }
.fade { animation: bench-fade 600s linear infinite; }
.grow { animation: bench-grow 600s linear infinite; }
";

fn animated_page(animating: usize, animation: &str) -> Document<()> {
    let mut dom = Document::new(device(), "page", ());
    dom.add_stylesheet(PAGE_CSS, StylesheetOrigin::Author);
    let root = dom.document_element().id();
    for index in 0..CARDS {
        let card = dom.create_element("view", ());
        dom.add_class(card, "card");
        if index % 3 == 0 {
            dom.add_class(card, "dim");
        }
        if index % 2 == 0 {
            dom.add_class(card, "clip");
        }
        if index < animating {
            dom.add_class(card, animation);
        }
        dom.append_child(root, card);
        let chip = dom.create_element("view", ());
        dom.add_class(chip, "chip");
        dom.append_child(card, chip);
    }
    dom.render();
    assert!(
        dom.has_active_animations() == (animating > 0),
        "the benchmark must animate exactly when it claims to"
    );
    dom
}

fn bench_frame(bencher: divan::Bencher<'_, '_>, animating: usize, animation: &str) {
    let page = RefCell::new(animated_page(animating, animation));
    let now = Cell::new(0.0_f64);
    bencher.bench_local(|| {
        let page = &mut *page.borrow_mut();
        now.set(now.get() + FRAME_STEP);
        divan::black_box(page.advance_animations(now.get()));
        divan::black_box(page.render());
    });
}

fn bench_tick(bencher: divan::Bencher<'_, '_>, animating: usize, animation: &str) {
    let page = RefCell::new(animated_page(animating, animation));
    let now = Cell::new(0.0_f64);
    bencher.bench_local(|| {
        let page = &mut *page.borrow_mut();
        now.set(now.get() + FRAME_STEP);
        divan::black_box(page.advance_animations(now.get()));
    });
}

#[divan::bench(args = [1, 8, 32, 120])]
fn frame_transform(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_frame(bencher, animating, "spin");
}

#[divan::bench(args = [1, 8, 32, 120])]
fn frame_opacity(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_frame(bencher, animating, "fade");
}

#[divan::bench(args = [1, 8, 32, 120])]
fn frame_width(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_frame(bencher, animating, "grow");
}

#[divan::bench]
fn frame_idle(bencher: divan::Bencher<'_, '_>) {
    let page = RefCell::new(animated_page(0, "spin"));
    let now = Cell::new(0.0_f64);
    bencher.bench_local(|| {
        let page = &mut *page.borrow_mut();
        now.set(now.get() + FRAME_STEP);
        divan::black_box(page.advance_animations(now.get()));
        divan::black_box(page.render());
    });
}

#[divan::bench(args = [1, 8, 32, 120])]
fn tick_transform(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_tick(bencher, animating, "spin");
}

#[divan::bench(args = [1, 8, 32, 120])]
fn tick_width(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_tick(bencher, animating, "grow");
}

const TEXT_UA_CSS: &str = "text { overflow: clip; }";

struct TextKind;

impl terminus::link::CustomElement<()> for TextKind {
    fn is_lynx_text(&self) -> bool {
        true
    }
}

const LABEL_CSS: &str =
    ".label { width: 120px; height: 20px; font-family: Ahem; font-size: 16px; }";

const SHIMMER_CSS: &str = "
page { display: flex; position: relative; width: 800px; height: 600px; }
.list { display: flex; flex-direction: column; overflow: scroll; width: 300px; height: 600px; }
.row { display: flex; flex-shrink: 0; width: 300px; height: 40px;
       background-color: #f6f6f8; border-radius: 6px;
       animation: bench-shimmer 600s linear infinite; }
@keyframes bench-shimmer { from { transform: translateX(-10px); }
                           to { transform: translateX(10px); } }
";

fn scroll_driven_css(header_animation: &str) -> String {
    format!(
        "page {{ display: flex; flex-direction: column; position: relative;
                 width: 800px; height: 600px; }}
         .header {{ flex-shrink: 0; width: 300px; height: 60px; background-color: #333333;
                    animation: {header_animation} linear both; animation-timeline: --list; }}
         .list {{ display: flex; flex-direction: column; overflow: scroll;
                  width: 300px; height: 540px; scroll-timeline: --list; }}
         .row {{ display: flex; flex-shrink: 0; width: 300px; height: 40px;
                 background-color: #f6f6f8; border-radius: 6px;
                 animation: bench-enter linear both; animation-timeline: view();
                 animation-range: entry; }}
         @keyframes bench-enter {{ from {{ opacity: 0.2; transform: translateY(20px); }}
                                   to {{ opacity: 1; transform: translateY(0px); }} }}
         @keyframes bench-lift {{ from {{ transform: translateY(0px); }}
                                  to {{ transform: translateY(-40px); }} }}
         @keyframes bench-tint {{ from {{ background-color: #333333; }}
                                  to {{ background-color: #3355aa; }} }}"
    )
}

fn scroll_driven_page(header_animation: &str) -> (Document<()>, terminus::link::NodeId) {
    let mut dom = Document::new(device(), "page", ());
    dom.add_stylesheet(
        &scroll_driven_css(header_animation),
        StylesheetOrigin::Author,
    );
    let root = dom.document_element().id();
    let header = dom.create_element("view", ());
    dom.add_class(header, "header");
    dom.append_child(root, header);
    let list = dom.create_element("view", ());
    dom.add_class(list, "list");
    dom.append_child(root, list);
    for _ in 0..SHIMMER_ROWS {
        let row = dom.create_element("view", ());
        dom.add_class(row, "row");
        dom.append_child(list, row);
    }
    (dom, list)
}

fn swept_offset(frame: u32) -> f32 {
    let phase = frame % 96;
    #[expect(clippy::cast_precision_loss, reason = "a small frame index")]
    let step = if phase < 48 { phase } else { 96 - phase } as f32;
    step * 10.0
}

#[divan::bench]
fn frame_scroll_driven_composite(bencher: divan::Bencher<'_, '_>) {
    let (mut page, list) = scroll_driven_page("bench-lift");
    let frame = page.commit();
    assert_eq!(
        frame.animation_slots().len(),
        SHIMMER_ROWS + 1,
        "every scroll-driven animation must export"
    );
    assert!(!frame.has_live_curves() && !frame.needs_main_ticks());
    let mut scene = DisplayList::new();
    let mut index = 0_u32;
    bencher.bench_local(move || {
        index += 1;
        let offset = terminus::link::Vector2D::new(0.0, swept_offset(index));
        scene.reset();
        frame.compose_into(
            &mut scene,
            &[],
            &[],
            &[],
            &|slot| (slot.node == list).then_some(offset),
            None,
        );
        divan::black_box(scene.len());
    });
}

#[divan::bench]
fn frame_scroll_driven_main(bencher: divan::Bencher<'_, '_>) {
    let (mut page, list) = scroll_driven_page("bench-tint");
    let frame = page.commit();
    assert_eq!(
        frame.animation_slots().len(),
        SHIMMER_ROWS,
        "the rows export and the header does not"
    );
    let mut index = 0_u32;
    bencher.bench_local(move || {
        index += 1;
        page.scroll_to(
            list,
            terminus::link::Vector2D::new(0.0, swept_offset(index)),
        );
        page.advance_scroll_timelines(&[list]);
        divan::black_box(page.render());
    });
}

fn commit_running(dom: &mut Document<()>) -> std::sync::Arc<CommittedFrame> {
    dom.render();
    dom.advance_animations(0.0);
    dom.advance_animations(FRAME_STEP);
    dom.render();
    dom.committed_frame()
        .expect("render always leaves a committed frame retained")
}

fn card_text_page(animating: usize, animation: &str) -> Document<()> {
    let mut dom = Document::new(device(), "page", ());
    dom.define("text", Box::new(TextKind));
    dom.add_stylesheet(TEXT_UA_CSS, StylesheetOrigin::UserAgent);
    dom.add_stylesheet(PAGE_CSS, StylesheetOrigin::Author);
    dom.add_stylesheet(LABEL_CSS, StylesheetOrigin::Author);
    assert!(
        dom.register_fonts(FontBlob::from_static(AHEM)) >= 1,
        "the labels need Ahem registered before the first layout"
    );
    let root = dom.document_element().id();
    let run = "e".repeat(12);
    for index in 0..CARDS {
        let card = dom.create_element("view", ());
        dom.add_class(card, "card");
        if index % 3 == 0 {
            dom.add_class(card, "dim");
        }
        if index % 2 == 0 {
            dom.add_class(card, "clip");
        }
        if index < animating {
            dom.add_class(card, animation);
        }
        dom.append_child(root, card);
        let label = dom.create_element("text", ());
        dom.add_class(label, "label");
        dom.append_child(card, label);
        let text = dom.create_text_node(run.as_str(), ());
        dom.append_child(label, text);
    }
    dom
}

fn shimmer_page() -> Document<()> {
    let mut dom = Document::new(device(), "page", ());
    dom.add_stylesheet(SHIMMER_CSS, StylesheetOrigin::Author);
    let root = dom.document_element().id();
    let list = dom.create_element("view", ());
    dom.add_class(list, "list");
    dom.append_child(root, list);
    for _ in 0..SHIMMER_ROWS {
        let row = dom.create_element("view", ());
        dom.add_class(row, "row");
        dom.append_child(list, row);
    }
    dom
}

fn bench_production_frame(
    bencher: divan::Bencher<'_, '_>,
    mut page: Document<()>,
    exported: usize,
) {
    let mut frame = commit_running(&mut page);
    assert_eq!(
        frame.animation_slots().len(),
        exported,
        "every animation must export as a composite curve"
    );
    assert!(
        !frame.needs_main_ticks(),
        "nothing may be left to main-thread ticks"
    );
    let mut scene = DisplayList::new();
    let mut now = FRAME_STEP;
    bencher.bench_local(move || {
        now += FRAME_STEP;
        if frame.needs_main_ticks() || frame.animation_boundary_passed(now) {
            page.advance_animations(now);
            page.render();
            frame = page
                .committed_frame()
                .expect("render always leaves a committed frame retained");
        }
        scene.reset();
        frame.compose_into(
            &mut scene,
            &[],
            &[],
            &[],
            &|_| None,
            frame.has_live_curves().then_some(now),
        );
        divan::black_box(scene.len());
    });
}

#[divan::bench(args = [1, 8, 32, 120])]
fn frame_card_text_transform(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_production_frame(bencher, card_text_page(animating, "spin"), animating);
}

#[divan::bench(args = [1, 8, 32, 120])]
fn frame_card_text_opacity(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_production_frame(bencher, card_text_page(animating, "fade"), animating);
}

#[divan::bench]
fn frame_shimmer_rows(bencher: divan::Bencher<'_, '_>) {
    bench_production_frame(bencher, shimmer_page(), SHIMMER_ROWS);
}
