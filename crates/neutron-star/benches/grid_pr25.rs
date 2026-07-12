//! Rust-only Grid benchmark scenarios migrated from PupilTong/lynx#25.

#[path = "scenarios/grid.rs"]
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

macro_rules! grid_bench {
    ($function:ident) => {
        #[divan::bench]
        fn $function(bencher: divan::Bencher<'_, '_>) {
            bench_scenario(bencher, stringify!($function));
        }
    };
}

grid_bench!(at_most_owner_matrix);
grid_bench!(baseline_propagation_matrix);
grid_bench!(measured_callback_matrix);
grid_bench!(in_flow_order_matrix);
grid_bench!(full_value_spacing_matrix);
grid_bench!(box_sizing_matrix);
grid_bench!(fit_content_subtrees);
grid_bench!(sticky_percent_insets);
grid_bench!(mixed_display_none);
grid_bench!(grid_out_of_flow_intrinsic);
grid_bench!(grid_out_of_flow_areas);
grid_bench!(grid_item_alignment_matrix);
grid_bench!(grid_content_alignment_matrix);
grid_bench!(grid_auto_flow_matrix);
grid_bench!(grid_auto_margin_alignment);
grid_bench!(grid_minmax_intrinsic_tracks);
grid_bench!(grid_auto_fit_content_max_tracks);
grid_bench!(grid_indefinite_auto_fit_content_max_tracks);
