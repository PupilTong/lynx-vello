//! Standalone Hughie benchmarks for five representative Flex tree shapes.

#![allow(clippy::cast_precision_loss)]

#[path = "support/mod.rs"]
mod support;

use divan::counter::ItemsCount;
use hughie::geometry::Size;
use support::LayoutFixture;

const NODES: usize = 4_095;

fn main() {
    divan::main();
}

fn bench_case(bencher: divan::Bencher<'_, '_>, build: fn() -> LayoutFixture) {
    bencher
        .with_inputs(build)
        .input_counter(|fixture| ItemsCount::new(fixture.node_count()))
        .bench_local_values(|mut fixture| {
            divan::black_box(fixture.run());
            fixture
        });
}

#[divan::bench]
fn wide_active_grow(bencher: divan::Bencher<'_, '_>) {
    bench_case(bencher, build_wide_active_grow);
}

#[divan::bench]
fn wide_wrap_gap(bencher: divan::Bencher<'_, '_>) {
    bench_case(bencher, build_wide_wrap_gap);
}

#[divan::bench]
fn wide_grow_freeze(bencher: divan::Bencher<'_, '_>) {
    bench_case(bencher, build_wide_grow_freeze);
}

#[divan::bench]
fn wide_shrink_freeze(bencher: divan::Bencher<'_, '_>) {
    bench_case(bencher, build_wide_shrink_freeze);
}

#[divan::bench]
fn balanced_deep(bencher: divan::Bencher<'_, '_>) {
    bench_case(bencher, build_balanced_deep);
}

fn flex_fixture(viewport: Size<f32>, extra: &str) -> LayoutFixture {
    LayoutFixture::new(
        viewport,
        &format!(
            "display:flex; box-sizing:border-box; flex-direction:row; \
             justify-content:flex-start; align-content:flex-start; \
             align-items:flex-start; {extra}"
        ),
    )
}

fn prepare(fixture: LayoutFixture) -> LayoutFixture {
    assert_eq!(fixture.node_count(), NODES);
    fixture.prepare()
}

fn build_wide_active_grow() -> LayoutFixture {
    let leaves = NODES - 1;
    let width = 2.0 * leaves as f32;
    let mut fixture = flex_fixture(
        Size::new(width, 8.0),
        &format!("width:{width}px; height:8px; flex-wrap:nowrap"),
    );
    let root = fixture.root();
    for leaf in 0..leaves {
        fixture.leaf(
            root,
            &format!(
                "box-sizing:border-box; min-width:0; height:8px; \
                 flex:{} 0 1px",
                1 + leaf % 3
            ),
            Size::ZERO,
            None,
        );
    }
    prepare(fixture)
}

fn build_wide_wrap_gap() -> LayoutFixture {
    let leaves = NODES - 1;
    let rows = leaves.div_ceil(16) as f32;
    let mut fixture = flex_fixture(
        Size::new(320.0, rows * 10.0),
        "width:320px; flex-wrap:wrap; gap:1px",
    );
    let root = fixture.root();
    for leaf in 0..leaves {
        let width = (16 + leaf % 5) as f32;
        let height = (6 + leaf % 3) as f32;
        fixture.leaf(
            root,
            &format!(
                "box-sizing:border-box; min-width:0; width:{width}px; \
                 height:{height}px; flex:0 0 {width}px"
            ),
            Size::ZERO,
            None,
        );
    }
    prepare(fixture)
}

fn build_wide_grow_freeze() -> LayoutFixture {
    let leaves = NODES - 1;
    let width = 150.0 * leaves as f32;
    let mut fixture = flex_fixture(
        Size::new(width, 8.0),
        &format!("width:{width}px; height:8px; flex-wrap:nowrap"),
    );
    let root = fixture.root();
    for leaf in 0..leaves {
        let max_width = if leaf.is_multiple_of(2) {
            "max-width:120px"
        } else {
            "max-width:none"
        };
        fixture.leaf(
            root,
            &format!(
                "box-sizing:border-box; min-width:0; {max_width}; height:8px; \
                 flex:1 0 100px"
            ),
            Size::ZERO,
            None,
        );
    }
    prepare(fixture)
}

fn build_wide_shrink_freeze() -> LayoutFixture {
    let leaves = NODES - 1;
    let width = 80.0 * leaves as f32;
    let mut fixture = flex_fixture(
        Size::new(width, 8.0),
        &format!("width:{width}px; height:8px; flex-wrap:nowrap"),
    );
    let root = fixture.root();
    for leaf in 0..leaves {
        let min_width = if leaf.is_multiple_of(2) { 90 } else { 0 };
        fixture.leaf(
            root,
            &format!(
                "box-sizing:border-box; min-width:{min_width}px; height:8px; \
                 flex:0 1 100px"
            ),
            Size::ZERO,
            None,
        );
    }
    prepare(fixture)
}

fn build_balanced_deep() -> LayoutFixture {
    let width = NODES as f32;
    let mut fixture = flex_fixture(
        Size::new(width, 8.0),
        &format!("width:{width}px; height:8px; flex-wrap:nowrap"),
    );
    let mut nodes = Vec::with_capacity(NODES);
    nodes.push(fixture.root());
    for index in 1..NODES {
        let parent = nodes[(index - 1) / 2];
        let grow = 1 + index % 3;
        let style = format!("box-sizing:border-box; min-width:0; height:8px; flex:{grow} 0 1px");
        let node = if index * 2 + 1 < NODES {
            fixture.container(
                parent,
                &format!(
                    "display:flex; flex-direction:row; flex-wrap:nowrap; \
                     justify-content:flex-start; align-items:flex-start; {style}"
                ),
            )
        } else {
            fixture.leaf(parent, &style, Size::ZERO, None)
        };
        nodes.push(node);
    }
    prepare(fixture)
}
