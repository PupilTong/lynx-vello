//! Cold, Rust-only benchmark workloads whose PR #25 source builders contain
//! at least one `display: linear` branch.

#[path = "scenarios/linear_pr25.rs"]
mod migrated_scenarios;
#[path = "../tests/support/mod.rs"]
mod support;

use divan::counter::ItemsCount;

const NODES: usize = 1_000;
const SOURCE_ITERATIONS: usize = 200;

fn main() {
    divan::main();
}

fn bench_migrated_scenario(bencher: divan::Bencher<'_, '_>, name: &'static str) {
    let scenario = migrated_scenarios::scenario_named(name);
    bencher
        // PR #25 starts its timer before `build_trees`, so construction and
        // the cold one-layout-per-tree pass belong to the measured region.
        // Keep its default B...B,L...L batch order as well: building every
        // tree first prevents layout from reading a just-built hot tree.
        .counter(ItemsCount::new(NODES * SOURCE_ITERATIONS))
        .bench_local(|| {
            let mut cases = (0..SOURCE_ITERATIONS)
                .map(|_| (scenario.build)(NODES))
                .collect::<Vec<_>>();
            for case in &mut cases {
                divan::black_box(case.run());
            }
        });
}

macro_rules! migrated_bench {
    ($function:ident) => {
        #[divan::bench(sample_count = 1, sample_size = 1)]
        fn $function(bencher: divan::Bencher<'_, '_>) {
            bench_migrated_scenario(bencher, stringify!($function));
        }
    };
}

// Keep the source PR's exact Linear-tagged scenario order.
migrated_bench!(at_most_owner_matrix);
migrated_bench!(baseline_propagation_matrix);
migrated_bench!(measured_callback_matrix);
migrated_bench!(in_flow_order_matrix);
migrated_bench!(full_value_spacing_matrix);
migrated_bench!(staggered_linear_list);
migrated_bench!(staggered_linear_raw_list_gaps);
migrated_bench!(linear_gravity_matrix);
migrated_bench!(linear_layout_gravity_matrix);
migrated_bench!(linear_cross_gravity_matrix);
migrated_bench!(box_sizing_matrix);
migrated_bench!(fit_content_subtrees);
migrated_bench!(sticky_percent_insets);
migrated_bench!(mixed_display_none);
