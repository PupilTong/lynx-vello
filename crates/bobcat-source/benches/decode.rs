//! Decode benchmarks over the compiled fixture sources, tracked by
//! `CodSpeed` (walltime mode on the macOS CI runner).

use divan::black_box;
use divan::counter::ItemsCount;

fn main() {
    divan::main();
}

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

const BATCH_SIZE: usize = 256;

fn bench_decode(bencher: divan::Bencher<'_, '_>, bytes: &'static [u8]) {
    bencher
        .counter(ItemsCount::new(BATCH_SIZE))
        .with_inputs(|| Vec::with_capacity(BATCH_SIZE))
        .bench_local_values(|mut templates| {
            for _ in 0..BATCH_SIZE {
                templates.push(bobcat_source::web::decode(black_box(bytes)).unwrap());
            }
            templates
        });
}

#[divan::bench]
fn decode_small_card(bencher: divan::Bencher<'_, '_>) {
    bench_decode(bencher, fixtures::fixture("basic-class-selector").page);
}

#[divan::bench]
fn decode_empty_style_info(bencher: divan::Bencher<'_, '_>) {
    bench_decode(bencher, fixtures::fixture("basic-bindtap").page);
}

#[divan::bench]
fn decode_large_style_info(bencher: divan::Bencher<'_, '_>) {
    bench_decode(
        bencher,
        fixtures::fixture("basic-performance-large-css").page,
    );
}

#[divan::bench]
fn selectors_to_css(bencher: divan::Bencher<'_, '_>) {
    let template =
        bobcat_source::web::decode(fixtures::fixture("basic-performance-large-css").page).unwrap();
    let style_info = template.style_info.unwrap();
    bencher.counter(ItemsCount::new(BATCH_SIZE)).bench(|| {
        let mut total = 0usize;
        for _ in 0..BATCH_SIZE {
            for sheet in black_box(&style_info).css_id_to_style_sheet.values() {
                for rule in &sheet.rules {
                    for selector in &rule.prelude.selectors {
                        total += selector.to_css_string().len();
                    }
                }
            }
        }
        total
    });
}
