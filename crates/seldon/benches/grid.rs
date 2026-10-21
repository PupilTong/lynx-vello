//! CSS Grid benchmarks through terminus's production host.

#[path = "scenarios/grid.rs"]
mod scenarios;
#[path = "support/mod.rs"]
mod support;

fn main() {
    divan::main();
}
