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

use std::cell::{Cell, RefCell};

use dom::vello::Scene;
use dom::{CommittedFrame, Document, FontBlob, StylesheetOrigin};
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

fn main() {
    divan::main();
}

/// Elements on the page. Fixed, so the `args` axis is purely "how many of them
/// animate" rather than "how big is the document".
const CARDS: usize = 120;

/// Rows in the shimmering list: far more than its scrollport shows.
const SHIMMER_ROWS: usize = 200;

/// Ahem: one filled em box per glyph, so the labels shape the same on every
/// runner.
const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

/// A frame's worth of timeline at 60Hz. Every animation here is `infinite` and
/// runs for far longer than any benchmark, so no iteration ever ends one.
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

fn device() -> dom::Device {
    dom::standards_device(
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

/// The paint benchmark's card page plus one `@keyframes` rule per animatable
/// tier, so every case here paints the same document and differs only in what
/// is animating.
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

/// A laid-out, rendered page whose first `animating` cards carry `animation`.
///
/// Rendered once up front so the benchmark measures steady-state frames rather
/// than the first scene build.
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

/// One whole presenting-thread frame: advance the timeline, then produce the
/// scene. This is the number an embedder pays per animated frame.
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

/// The animation half of a frame on its own: the timeline step plus the cascade
/// over the animating elements. No scene is produced and no layout runs — a
/// tick only *marks* layout dirty, so a reflow shows up in `frame_width` rather
/// than in `tick_width`.
fn bench_tick(bencher: divan::Bencher<'_, '_>, animating: usize, animation: &str) {
    let page = RefCell::new(animated_page(animating, animation));
    let now = Cell::new(0.0_f64);
    bencher.bench_local(|| {
        let page = &mut *page.borrow_mut();
        now.set(now.get() + FRAME_STEP);
        divan::black_box(page.advance_animations(now.get()));
    });
}

/// A frame of a `transform` animation: cannot move a box, so it never reaches
/// layout.
#[divan::bench(args = [1, 8, 32, 120])]
fn frame_transform(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_frame(bencher, animating, "spin");
}

/// A frame of an `opacity` animation: paint-only like `transform`, but it also
/// opens a render layer for the walker to composite.
#[divan::bench(args = [1, 8, 32, 120])]
fn frame_opacity(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_frame(bencher, animating, "fade");
}

/// A frame of a `width` animation: the same work plus the reflow the two above
/// avoid.
#[divan::bench(args = [1, 8, 32, 120])]
fn frame_width(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_frame(bencher, animating, "grow");
}

/// The frame this page costs when nothing animates — the floor the three cases
/// above are measured against. `render` produces nothing without a visual
/// mutation, so this is what an idle animated page pays.
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

/// The Lynx UA rule a `<text>` brings into every card: its `overflow: clip`
/// gives each card's animated subtree a clip node. The clip only has to
/// exist — the export used to refuse any subtree holding one — and no op
/// pushes it: an in-flow `<text>` that is no stacking context records its
/// run under its parent's clip.
const TEXT_UA_CSS: &str = "text { display: -lynx-text; overflow: clip; }";

/// Each card's label: narrower than its run. Only a `.clip` card's own
/// `overflow: hidden` cuts the glyphs.
const LABEL_CSS: &str =
    ".label { width: 120px; height: 20px; font-family: Ahem; font-size: 16px; }";

/// A list scrolling 200 rows through a 600 px scrollport, each row running
/// its own sideways shimmer.
const SHIMMER_CSS: &str = "
page { display: flex; position: relative; width: 800px; height: 600px; }
.list { display: flex; flex-direction: column; overflow: scroll; width: 300px; height: 600px; }
.row { display: flex; flex-shrink: 0; width: 300px; height: 40px;
       background-color: #f6f6f8; border-radius: 6px;
       animation: bench-shimmer 600s linear infinite; }
@keyframes bench-shimmer { from { transform: translateX(-10px); }
                           to { transform: translateX(10px); } }
";

/// Starts every armed animation and commits it running: the first
/// `advance_animations` resolves the start times, the second runs them.
fn commit_running(dom: &mut Document<()>) -> std::sync::Arc<CommittedFrame> {
    dom.render();
    dom.advance_animations(0.0);
    dom.advance_animations(FRAME_STEP);
    dom.render();
    dom.committed_frame()
        .expect("render always leaves a committed frame retained")
}

/// [`animated_page`] with a `<text>` label in place of each card's chip,
/// the first `animating` cards carrying `animation`; [`commit_running`]
/// commits it running.
fn card_text_page(animating: usize, animation: &str) -> Document<()> {
    let mut dom = Document::new(device(), "page", ());
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

/// A page holding one list of [`SHIMMER_ROWS`] shimmering rows.
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

/// One production frame, the way the painter runs it: the main thread ticks
/// and recommits only when the committed frame asks for it, then the frame
/// composes at the instant, sampling its curves when the program draws any.
///
/// `exported` is how many curves the page must export: a page where one
/// fell back to main-thread ticks is refused rather than timed as a
/// different path under the same name.
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
    let mut scene = Scene::new();
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
            &|_| None,
            frame.has_live_curves().then_some(now),
        );
        divan::black_box(scene.encoding().draw_tags.len());
    });
}

/// A production frame of cards spinning with a `<text>` inside each: the
/// label's clip rides the card's curve instead of refusing it.
#[divan::bench(args = [1, 8, 32, 120])]
fn frame_card_text_transform(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_production_frame(bencher, card_text_page(animating, "spin"), animating);
}

/// A production frame of cards fading with a `<text>` inside each.
#[divan::bench(args = [1, 8, 32, 120])]
fn frame_card_text_opacity(bencher: divan::Bencher<'_, '_>, animating: usize) {
    bench_production_frame(bencher, card_text_page(animating, "fade"), animating);
}

/// A production frame of a list whose every row runs its own transform
/// shimmer. Each row's curve reach is its box ± 10 px, so culling keeps the
/// rows that can meet the list's encode window and the frame samples only
/// theirs: the cost tracks the window, not the 200 rows.
#[divan::bench]
fn frame_shimmer_rows(bencher: divan::Bencher<'_, '_>) {
    bench_production_frame(bencher, shimmer_page(), SHIMMER_ROWS);
}
