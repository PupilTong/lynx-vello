//! CSS Grid benchmarks through w3c-dom's production host.

#[path = "scenarios/grid.rs"]
mod scenarios;
#[path = "support/mod.rs"]
mod support;

fn main() {
    divan::main();
}
