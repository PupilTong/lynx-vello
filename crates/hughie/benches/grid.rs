//! CSS Grid benchmarks through dom's production host.

#[path = "scenarios/grid.rs"]
mod scenarios;
#[path = "support/mod.rs"]
mod support;

fn main() {
    divan::main();
}
