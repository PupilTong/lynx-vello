//! Dedicated Linear layout benchmarks, tracked by `CodSpeed` through Divan.

#[path = "scenarios/linear.rs"]
mod scenarios;
#[path = "../tests/linear_support/mod.rs"]
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

macro_rules! linear_bench {
    ($function:ident, $scenario:literal) => {
        #[divan::bench]
        fn $function(bencher: divan::Bencher<'_, '_>) {
            bench_scenario(bencher, $scenario);
        }
    };
}

linear_bench!(fixed_stack, "fixed_stack");
linear_bench!(ordered_stack, "ordered_stack");
linear_bench!(weighted_distribution, "weighted_distribution");
linear_bench!(weighted_freeze, "weighted_freeze");
linear_bench!(measured_stretch, "measured_stretch");
linear_bench!(mixed_hidden_absolute, "mixed_hidden_absolute");
linear_bench!(linear_gravity_matrix, "linear_gravity_matrix");
linear_bench!(linear_layout_gravity_matrix, "linear_layout_gravity_matrix");
linear_bench!(linear_cross_gravity_matrix, "linear_cross_gravity_matrix");
