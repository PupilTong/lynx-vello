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
    ($function:ident, $scenario:expr) => {
        #[divan::bench]
        fn $function(bencher: divan::Bencher<'_, '_>) {
            bench_scenario(bencher, $scenario);
        }
    };
}

macro_rules! declare_benchmarks {
    ($( $function:ident, $build:ident; )*) => {
        $(linear_bench!($function, stringify!($function));)*
    };
}

scenarios::for_each_linear_scenario!(declare_benchmarks);
