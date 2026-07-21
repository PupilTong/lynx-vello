//! Flex benchmarks through w3c-dom's production host.

#[path = "scenarios/flexbox.rs"]
mod scenarios;
#[path = "support/mod.rs"]
mod support;

use divan::counter::ItemsCount;
use scenarios::{BenchCase, scenario_named};

const NODES: usize = 1_000;

fn main() {
    divan::main();
}

fn scenario_batch_size(name: &str) -> usize {
    match name {
        "absolute_children" => 16,
        "flex_at_most_root" => 4,
        "flex_baseline_measured" | "flex_grow_row" | "flex_wrap_gaps" => 8,
        _ => 1,
    }
}

fn bench_scenario(bencher: divan::Bencher<'_, '_>, name: &'static str) {
    let scenario = scenario_named(name);
    let batch_size = scenario_batch_size(name);
    bencher
        .with_inputs(move || {
            (0..batch_size)
                .map(|_| (scenario.build)(NODES))
                .collect::<Vec<_>>()
        })
        .input_counter(|cases: &Vec<BenchCase>| {
            ItemsCount::new(cases.iter().map(BenchCase::node_count).sum::<usize>())
        })
        .bench_local_values(|mut cases| {
            for case in &mut cases {
                divan::black_box(case.run());
            }
            // Preserve one fresh fixture per logical cold layout and move its
            // destruction outside the timed region.
            cases
        });
}

macro_rules! flex_bench {
    ($function:ident, $scenario:literal) => {
        #[divan::bench]
        fn $function(bencher: divan::Bencher<'_, '_>) {
            bench_scenario(bencher, $scenario);
        }
    };
}

flex_bench!(flex_grow_row, "flex_grow_row");
flex_bench!(flex_wrap_gaps, "flex_wrap_gaps");
flex_bench!(flex_at_most_root, "flex_at_most_root");
flex_bench!(at_most_owner_matrix, "at_most_owner_matrix");
flex_bench!(owner_direction_inheritance, "owner_direction_inheritance");
flex_bench!(flex_axis_alignment_matrix, "flex_axis_alignment_matrix");
flex_bench!(flex_distribution_matrix, "flex_distribution_matrix");
flex_bench!(flex_wrap_alignment_matrix, "flex_wrap_alignment_matrix");
flex_bench!(flex_baseline_measured, "flex_baseline_measured");
flex_bench!(baseline_propagation_matrix, "baseline_propagation_matrix");
flex_bench!(measured_callback_matrix, "measured_callback_matrix");
flex_bench!(absolute_children, "absolute_children");
flex_bench!(nested_column_flex, "nested_column_flex");
flex_bench!(in_flow_order_matrix, "in_flow_order_matrix");
flex_bench!(full_value_spacing_matrix, "full_value_spacing_matrix");
flex_bench!(box_sizing_matrix, "box_sizing_matrix");
flex_bench!(fit_content_subtrees, "fit_content_subtrees");
flex_bench!(mixed_display_none, "mixed_display_none");
