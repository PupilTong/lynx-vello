//! Linear layout benchmarks through w3c-dom's production host.

#[path = "scenarios/linear.rs"]
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
        "linear_cross_gravity_matrix"
        | "linear_gravity_matrix"
        | "linear_layout_gravity_matrix" => 3,
        _ => 24,
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
            cases
        });
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
