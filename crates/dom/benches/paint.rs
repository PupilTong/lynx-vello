//! Document render benchmarks: style/layout/visual-order/private paint →
//! retained `vello::Scene`, CPU-side only (no GPU dispatch),
//! CodSpeed-compatible.

use dom::{Document, StylesheetOrigin};
use euclid::{Scale, Size2D};
use stylo::context::QuirksMode;
use stylo::device::Device;
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

fn device() -> Device {
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
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

/// A page of decorated cards: backgrounds, borders, radii, shadows, and a
/// sprinkling of opacity groups and overflow clips — the shape of a typical
/// Lynx card list.
fn card_page(cards: usize) -> Document<()> {
    let mut dom = Document::new(device());
    dom.add_stylesheet(
        "page { display: flex; position: relative; width: 800px; height: 600px; }
         .card { display: flex; position: absolute; width: 180px; height: 80px;
                 background-color: #f6f6f8; border: 2px solid #cccccc;
                 border-radius: 10px; box-shadow: 0px 2px 6px rgba(0,0,0,0.25); }
         .fade { opacity: 0.85; }
         .clip { overflow: hidden; }
         .chip { display: flex; width: 60px; height: 20px;
                 background-color: #3366ff; border-radius: 10px; }",
        StylesheetOrigin::Author,
    );
    let root = dom.create_element("page", ());
    dom.append_document_element(root);
    for index in 0..cards {
        let card = dom.create_element("view", ());
        dom.add_class(card, "card");
        if index % 3 == 0 {
            dom.add_class(card, "fade");
        }
        if index % 2 == 0 {
            dom.add_class(card, "clip");
        }
        dom.append_child(root, card);
        let chip = dom.create_element("view", ());
        dom.add_class(chip, "chip");
        dom.append_child(card, chip);
    }
    dom
}

#[divan::bench(args = [24, 120])]
fn render_document(bencher: divan::Bencher<'_, '_>, cards: usize) {
    bencher
        .with_inputs(|| card_page(cards))
        .bench_local_values(|mut dom| {
            divan::black_box(dom.render());
            divan::black_box(dom.scene().encoding().draw_tags.len());
            dom
        });
}
