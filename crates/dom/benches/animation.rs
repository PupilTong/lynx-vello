//! Animation frame benchmarks: what one frame of a running `@keyframes`
//! animation costs on the presenting thread, CPU-side only (no GPU dispatch).
//!
//! One iteration is one frame — advance the timeline, then produce the scene —
//! so the reported time reads directly as per-frame cost. Three axes matter:
//!
//! - **How much of a frame is the animation.** `frame_*` measures the whole frame; `tick_*`
//!   measures only `Document::advance_animations`. The gap between them is the scene rebuild, which
//!   no amount of animation work can avoid while the engine has no compose-time layers.
//! - **How the animation scales.** The `args` are how many of the page's elements animate. A
//!   frame's animation cost should be O(animating), not O(document).
//! - **What a reflow costs.** `frame_transform` cannot move a box and never reaches layout;
//!   `frame_width` does both. Their difference is the reflow the paint-only path avoids.

use std::cell::{Cell, RefCell};

use dom::{Document, StylesheetOrigin};
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
