//! Decode benchmarks over the vendored real-world bundles, tracked by
//! `CodSpeed` (walltime mode on the macOS CI runner).

use divan::black_box;

fn main() {
    divan::main();
}

/// Small regular card: one CSS rule, ~26 KB of main-thread JS.
const SMALL: &[u8] = include_bytes!("../tests/fixtures/basic-class-selector.web.bundle");
/// Card with an empty stylesheet map — measures the container + string-map
/// paths in isolation.
const EMPTY_STYLE: &[u8] = include_bytes!("../tests/fixtures/basic-bindtap.web.bundle");
/// 24 KB rkyv `StyleInfo` section (~200 rules) — dominated by the validated
/// rkyv decode.
const LARGE_CSS: &[u8] = include_bytes!("../tests/fixtures/basic-performance-large-css.web.bundle");

#[divan::bench]
fn decode_small_card() -> lynx_template_decoder::WebTemplate {
    lynx_template_decoder::decode(black_box(SMALL)).unwrap()
}

#[divan::bench]
fn decode_empty_style_info() -> lynx_template_decoder::WebTemplate {
    lynx_template_decoder::decode(black_box(EMPTY_STYLE)).unwrap()
}

#[divan::bench]
fn decode_large_style_info() -> lynx_template_decoder::WebTemplate {
    lynx_template_decoder::decode(black_box(LARGE_CSS)).unwrap()
}

/// Selector reassembly over every rule of the large stylesheet — the hot
/// text-generation path a renderer would hit.
#[divan::bench]
fn selectors_to_css(bencher: divan::Bencher<'_, '_>) {
    let template = lynx_template_decoder::decode(LARGE_CSS).unwrap();
    let style_info = template.style_info.unwrap();
    bencher.bench(|| {
        let mut total = 0usize;
        for sheet in black_box(&style_info).css_id_to_style_sheet.values() {
            for rule in &sheet.rules {
                for selector in &rule.prelude.selector_list {
                    total += selector.to_css_string().len();
                }
            }
        }
        total
    });
}
