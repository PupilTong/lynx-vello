//! Cold, Rust-only `display: relative` benchmark workloads migrated from
//! `PupilTong/lynx#25`.

#[path = "scenarios/relative.rs"]
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
        // Moving a freshly generated case into every measured invocation
        // preserves PR #25's one-layout-per-tree cold-cache workload.
        .bench_local_values(|mut case| divan::black_box(case.run()));
}

macro_rules! relative_bench {
    ($function:ident) => {
        #[divan::bench]
        fn $function(bencher: divan::Bencher<'_, '_>) {
            bench_scenario(bencher, stringify!($function));
        }
    };
}

relative_bench!(at_most_owner_matrix);
relative_bench!(baseline_propagation_matrix);
relative_bench!(measured_callback_matrix);
relative_bench!(box_sizing_matrix);
relative_bench!(fit_content_subtrees);
relative_bench!(relative_dependency_graph);
relative_bench!(relative_center_matrix);
relative_bench!(sticky_percent_insets);
relative_bench!(mixed_display_none);
