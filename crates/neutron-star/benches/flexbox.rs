//! Rust-only Flex benchmark scenarios migrated from `PupilTong/lynx#25`.

#[path = "scenarios/flexbox.rs"]
mod scenarios;
#[path = "../tests/support/mod.rs"]
mod support;

use divan::counter::ItemsCount;
use scenarios::{BenchCase, scenario_named};

const NODES: usize = 1_000;

fn main() {
    divan::main();
}

fn bench_scenario(bencher: divan::Bencher<'_, '_>, name: &'static str) {
    let scenario = scenario_named(name);
    bencher
        .with_inputs(move || (scenario.build)(NODES))
        .input_counter(|case: &BenchCase| ItemsCount::new(case.node_count()))
        .bench_local_values(|mut case| divan::black_box(case.run()));
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
flex_bench!(
    standalone_owner_direction_inheritance,
    "standalone_owner_direction_inheritance"
);
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
