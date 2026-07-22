//! Starlight Relative workloads through w3c-dom's production layout host.

#![allow(clippy::cast_precision_loss)]

use divan::counter::ItemsCount;
use neutron_star::geometry::Size;

use crate::support::{LayoutFixture, LeafContent};

#[derive(Debug, Clone, Copy)]
enum GraphKind {
    Independent,
    ReverseChain,
    DisjointCycles,
    DuplicateIds,
}

fn graph_fixture(
    item_count: usize,
    layout_once: bool,
    kind: GraphKind,
    auto_width: bool,
    content: LeafContent,
) -> LayoutFixture {
    let count = item_count.max(1);
    let width = count as f32 * 4.0;
    let root_style = if auto_width {
        format!(
            "display:relative; width:auto; max-width:{width}px; height:256px; relative-layout-once:{layout_once}"
        )
    } else {
        format!(
            "display:relative; width:{width}px; height:256px; relative-layout-once:{layout_once}"
        )
    };
    let mut fixture = LayoutFixture::new(Size::new(width.max(1.0), 256.0), &root_style);
    let root = fixture.root();

    for index in 0..count {
        let relative_id = if matches!(kind, GraphKind::DuplicateIds) {
            index % 32 + 1
        } else {
            index + 1
        };
        let constraints = match kind {
            GraphKind::ReverseChain if index + 1 < count => {
                format!("relative-right-of:{}", index + 2)
            }
            GraphKind::DisjointCycles => {
                let partner = if index.is_multiple_of(2) {
                    (index + 2).min(count)
                } else {
                    index
                };
                format!("relative-right-of:{partner}")
            }
            GraphKind::DuplicateIds => {
                if index >= 32 {
                    format!(
                        "relative-align-left:parent; relative-bottom-of:{}",
                        index % 32 + 1
                    )
                } else {
                    "relative-align-left:parent".to_owned()
                }
            }
            GraphKind::Independent | GraphKind::ReverseChain => String::new(),
        };
        let sizing = if auto_width {
            match index % 3 {
                0 => "width:auto",
                1 => "width:2%",
                _ => "width:auto; relative-align-left:parent; relative-align-right:parent",
            }
        } else {
            "width:4px"
        };
        let style = format!("{sizing}; height:4px; relative-id:{relative_id}; {constraints}");
        fixture.leaf_with_content(root, &style, Size::new(4.0, 4.0), None, content, index);
    }
    fixture.prepare()
}

fn nested_fixture(content: LeafContent) -> LayoutFixture {
    const CONTAINERS: usize = 32;
    const ITEMS: usize = 32;
    let mut fixture = LayoutFixture::new(
        Size::new(1024.0, 768.0),
        "display:relative; width:1024px; height:768px",
    );
    let root = fixture.root();
    for _ in 0..CONTAINERS {
        let container = fixture.container(
            root,
            "display:relative; width:128px; height:128px; relative-layout-once:false",
        );
        for index in 0..ITEMS {
            let constraint = if index > 0 {
                format!("relative-right-of:{index}")
            } else {
                String::new()
            };
            let style = format!(
                "width:4px; height:4px; relative-id:{}; {constraint}",
                index + 1
            );
            fixture.leaf_with_content(container, &style, Size::new(4.0, 4.0), None, content, index);
        }
    }
    fixture.prepare()
}

const SMALL_GRAPH_BATCH: usize = 64;
const LARGE_GRAPH_BATCH: usize = 4;
const NESTED_COLD_BATCH: usize = 8;
const WARM_DESCENDANTS_BATCH: usize = 256;
const ROOT_CACHE_HIT_BATCH: usize = 1_024;

const fn graph_batch_size(item_count: usize) -> usize {
    if item_count <= 256 {
        SMALL_GRAPH_BATCH
    } else {
        LARGE_GRAPH_BATCH
    }
}

fn bench_graph(
    bencher: divan::Bencher<'_, '_>,
    item_count: usize,
    layout_once: bool,
    kind: GraphKind,
    content: LeafContent,
) {
    let batch_size = if content.is_text() {
        1
    } else {
        graph_batch_size(item_count)
    };
    bencher
        .with_inputs(|| {
            (0..batch_size)
                .map(|_| graph_fixture(item_count, layout_once, kind, false, content))
                .collect::<Vec<_>>()
        })
        .input_counter(|fixtures: &Vec<LayoutFixture>| {
            ItemsCount::new(
                fixtures
                    .iter()
                    .map(LayoutFixture::node_count)
                    .sum::<usize>(),
            )
        })
        .bench_local_values(|mut fixtures| {
            for fixture in &mut fixtures {
                divan::black_box(fixture.run());
            }
            fixtures
        });
}

#[divan::bench(args = [256, 4_096])]
fn independent_two_pass_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(
        bencher,
        item_count,
        false,
        GraphKind::Independent,
        LeafContent::Synthetic,
    );
}

fn bench_wrap_width(bencher: divan::Bencher<'_, '_>, item_count: usize, content: LeafContent) {
    let batch_size = if content.is_text() {
        1
    } else {
        graph_batch_size(item_count)
    };
    bencher
        .with_inputs(|| {
            (0..batch_size)
                .map(|_| graph_fixture(item_count, false, GraphKind::Independent, true, content))
                .collect::<Vec<_>>()
        })
        .input_counter(|fixtures: &Vec<LayoutFixture>| {
            ItemsCount::new(
                fixtures
                    .iter()
                    .map(LayoutFixture::node_count)
                    .sum::<usize>(),
            )
        })
        .bench_local_values(|mut fixtures| {
            for fixture in &mut fixtures {
                divan::black_box(fixture.run());
            }
            fixtures
        });
}

#[divan::bench(args = [256, 4_096])]
fn independent_two_pass_wrap_width_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_wrap_width(bencher, item_count, LeafContent::Synthetic);
}

#[divan::bench(args = [256, 4_096])]
fn independent_two_pass_wrap_width_with_text_cold(
    bencher: divan::Bencher<'_, '_>,
    item_count: usize,
) {
    bench_wrap_width(bencher, item_count, LeafContent::Text);
}

#[divan::bench(args = [256, 4_096])]
fn reverse_chain_two_pass_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(
        bencher,
        item_count,
        false,
        GraphKind::ReverseChain,
        LeafContent::Synthetic,
    );
}

#[divan::bench(args = [256, 4_096])]
fn reverse_chain_two_pass_with_text_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(
        bencher,
        item_count,
        false,
        GraphKind::ReverseChain,
        LeafContent::Text,
    );
}

#[divan::bench(args = [256, 4_096])]
fn reverse_chain_one_pass_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(
        bencher,
        item_count,
        true,
        GraphKind::ReverseChain,
        LeafContent::Synthetic,
    );
}

#[divan::bench(args = [256, 4_096])]
fn disjoint_cycles_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(
        bencher,
        item_count,
        true,
        GraphKind::DisjointCycles,
        LeafContent::Synthetic,
    );
}

#[divan::bench(args = [256, 4_096])]
fn disjoint_cycles_with_text_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(
        bencher,
        item_count,
        true,
        GraphKind::DisjointCycles,
        LeafContent::Text,
    );
}

#[divan::bench(args = [256, 4_096])]
fn duplicate_ids_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(
        bencher,
        item_count,
        false,
        GraphKind::DuplicateIds,
        LeafContent::Synthetic,
    );
}

#[divan::bench(args = [256, 4_096])]
fn duplicate_ids_with_text_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_graph(
        bencher,
        item_count,
        false,
        GraphKind::DuplicateIds,
        LeafContent::Text,
    );
}

#[divan::bench]
fn nested_relative_cold(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            (0..NESTED_COLD_BATCH)
                .map(|_| nested_fixture(LeafContent::Synthetic))
                .collect::<Vec<_>>()
        })
        .input_counter(|fixtures: &Vec<LayoutFixture>| {
            ItemsCount::new(
                fixtures
                    .iter()
                    .map(LayoutFixture::node_count)
                    .sum::<usize>(),
            )
        })
        .bench_local_values(|mut fixtures| {
            for fixture in &mut fixtures {
                divan::black_box(fixture.run());
            }
            fixtures
        });
}

#[divan::bench]
fn nested_relative_with_text_cold(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| nested_fixture(LeafContent::Text))
        .input_counter(LayoutFixture::node_count)
        .bench_local_values(|mut fixture| {
            divan::black_box(fixture.run());
            fixture
        });
}

#[divan::bench]
fn nested_relative_warm_descendants(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            let mut fixture = nested_fixture(LeafContent::Synthetic);
            let _ = fixture.run();
            fixture.invalidate_root();
            fixture
        })
        .input_counter(|fixture| ItemsCount::new(fixture.node_count() * WARM_DESCENDANTS_BATCH))
        .bench_local_refs(|fixture| {
            for _ in 0..WARM_DESCENDANTS_BATCH {
                divan::black_box(fixture.run());
                fixture.invalidate_root();
            }
        });
}

#[divan::bench]
fn nested_relative_root_cache_hit(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            let mut fixture = nested_fixture(LeafContent::Synthetic);
            let _ = fixture.run();
            fixture
        })
        .input_counter(|fixture| ItemsCount::new(fixture.node_count() * ROOT_CACHE_HIT_BATCH))
        .bench_local_refs(|fixture| {
            for _ in 0..ROOT_CACHE_HIT_BATCH {
                divan::black_box(divan::black_box(&mut *fixture).run());
            }
        });
}
