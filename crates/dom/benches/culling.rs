//! Commit benchmarks for culling under exported transform curves: what one
//! commit costs when a page's content moves by a compose-time curve.
//!
//! One iteration is one full commit: a background flip on the document
//! element invalidates the frame, so the build, the cull plan and the encode
//! all rerun, while every curve stays exported. The composition a curve
//! drives per frame is not measured here; a commit is where culling pays.
//!
//! - **`still`** is the floor: 500 boxes in a card that does not animate.
//! - **`rotor`** is the same card rotating: every box's admitted region is pulled back through the
//!   card's curve reach, and all of them are on screen.
//! - **`nested`** puts the boxes under three animated wrappers (rotate, alternating scale,
//!   translate), so each pull-back crosses three reaches.
//! - **`rows`** is 200 list rows each fading in and up: the reach lets the list's window bound the
//!   rows, so only the ones that can reach the screen encode.

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

fn main() {
    divan::main();
}

const CSS: &str = "
page { display: flex; position: relative; width: 800px; height: 600px; }
.list { display: flex; flex-direction: column; overflow: scroll; width: 400px; height: 600px; }
.row { display: flex; flex-shrink: 0; width: 400px; height: 40px; background-color: teal;
       animation: rise 600s linear infinite; }
.card { display: flex; flex-wrap: wrap; position: absolute; left: 200px; top: 100px;
        width: 400px; height: 400px; }
.dot { display: flex; width: 16px; height: 16px; background-color: #3366ff; }
.spin { animation: spin 600s linear infinite; }
.pulse { animation: pulse 600s ease-in-out infinite alternate; }
.slide { animation: slide 600s linear infinite; }
@keyframes rise { from { opacity: 0; transform: translateY(20px); } to { opacity: 1; transform: translateY(0px); } }
@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
@keyframes pulse { from { transform: scale(0.8); } to { transform: scale(1.2); } }
@keyframes slide { from { transform: translateX(0px); } to { transform: translateX(100px); } }
";

fn el(dom: &mut Document<()>, parent: dom::NodeId, class: &str) -> dom::NodeId {
    let node = dom.create_element("view", ());
    dom.add_class(node, class);
    dom.append_child(parent, node);
    node
}

fn page(kind: &str) -> Document<()> {
    let mut dom = Document::new(device(), "page", ());
    dom.add_stylesheet(CSS, StylesheetOrigin::Author);
    let root = dom.document_element().id();
    match kind {
        "rows" => {
            let list = el(&mut dom, root, "list");
            for _ in 0..200 {
                el(&mut dom, list, "row");
            }
        }
        "rotor" | "still" => {
            let card = el(&mut dom, root, "card");
            if kind == "rotor" {
                dom.add_class(card, "spin");
            }
            for _ in 0..500 {
                el(&mut dom, card, "dot");
            }
        }
        "nested" => {
            let a = el(&mut dom, root, "card");
            dom.add_class(a, "spin");
            let b = el(&mut dom, a, "card");
            dom.add_class(b, "pulse");
            let c = el(&mut dom, b, "card");
            dom.add_class(c, "slide");
            for _ in 0..500 {
                el(&mut dom, c, "dot");
            }
        }
        _ => unreachable!(),
    }
    dom.render();
    dom.advance_animations(0.0);
    dom.advance_animations(0.25);
    dom.render();
    dom
}

/// One full commit of a `kind` page; see the module doc.
#[divan::bench(args = ["still", "rotor", "nested", "rows"])]
fn commit(bencher: divan::Bencher<'_, '_>, kind: &str) {
    let dom = RefCell::new(page(kind));
    let flip = Cell::new(false);
    bencher.bench_local(|| {
        let dom = &mut *dom.borrow_mut();
        flip.set(!flip.get());
        let root = dom.document_element().id();
        dom.set_inline_style(
            root,
            if flip.get() {
                "background-color: navy"
            } else {
                "background-color: teal"
            },
        );
        divan::black_box(dom.render());
    });
}
