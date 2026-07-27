//! Starlight Relative benchmarks through w3c-dom's production host.

#[path = "scenarios/relative.rs"]
mod scenarios;
#[path = "support/mod.rs"]
mod support;

fn main() {
    divan::main();
}
