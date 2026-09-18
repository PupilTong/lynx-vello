//! CSS Grid Level 3 grid-lanes benchmarks through dom's production host.

#[path = "scenarios/grid_lanes.rs"]
mod scenarios;
#[path = "support/mod.rs"]
mod support;

fn main() {
    divan::main();
}
