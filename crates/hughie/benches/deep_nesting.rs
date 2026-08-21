//! Deep single-child container chains, where a container's own size comes only
//! from its content.
//!
//! Each level re-asks its one child for a size, so a chain is the shape that
//! exposes any per-level multiplication in the measurement protocol: work that
//! is not linear in the chain's depth here is exponential or polynomial in it.
//! `dom`'s `css::inheritance_deep_chain` builds the same 256-level shape with
//! the initial `display`, which generates no box tree below the root; these
//! cases give every level a real formatting context.

#[path = "support/mod.rs"]
mod support;

use divan::counter::ItemsCount;
use hughie::geometry::Size;
use support::LayoutFixture;

const DEPTH: usize = 256;

fn main() {
    divan::main();
}

fn bench_chain(bencher: divan::Bencher<'_, '_>, display: &'static str) {
    bencher
        .with_inputs(move || build_chain(display))
        .input_counter(|fixture: &LayoutFixture| ItemsCount::new(fixture.node_count()))
        .bench_local_values(|mut fixture| {
            divan::black_box(fixture.run());
            fixture
        });
}

/// A `DEPTH`-level chain in which no level declares a size, so every level
/// sizes itself from the level below it.
fn build_chain(display: &'static str) -> LayoutFixture {
    let style = format!("display:{display}; box-sizing:border-box");
    let mut fixture = LayoutFixture::new(Size::new(800.0, 600.0), &style);
    let mut parent = fixture.root();
    for _ in 1..DEPTH {
        parent = fixture.container(parent, &style);
    }
    assert_eq!(fixture.node_count(), DEPTH);
    fixture.prepare()
}

#[divan::bench]
fn flex_row_chain(bencher: divan::Bencher<'_, '_>) {
    bench_chain(bencher, "flex");
}

#[divan::bench]
fn grid_chain(bencher: divan::Bencher<'_, '_>) {
    bench_chain(bencher, "grid");
}

#[divan::bench]
fn linear_chain(bencher: divan::Bencher<'_, '_>) {
    bench_chain(bencher, "linear");
}

#[divan::bench]
fn relative_chain(bencher: divan::Bencher<'_, '_>) {
    bench_chain(bencher, "relative");
}
