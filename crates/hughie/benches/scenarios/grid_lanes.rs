//! `display: grid-lanes` workloads driven through dom's production layout
//! host.
//!
//! The two sizing paths the algorithm can take are separate cases on purpose.
//! A grid axis whose every live track is sized by its own function alone never
//! materialises the per-candidate-position sizing items css-grid-3 §3.4 asks
//! for; a track that consults the items placed in it — an intrinsic minimum or
//! maximum, or a flexible one — makes the pass clone one sizing item per start
//! position an auto-placed item could take. `repeat(n, 120px)` is the first,
//! `minmax(0, 1fr)` and `minmax(min-content, 1fr)` are both the second.

#![allow(clippy::cast_precision_loss)]

use divan::counter::ItemsCount;
use dom::NodeId;
use hughie::geometry::Size;

use crate::support::LayoutFixture;

fn lanes_fixture(width: f32, height: f32, extra: &str) -> LayoutFixture {
    let style = format!("display:grid-lanes; width:{width}px; height:{height}px; {extra}");
    LayoutFixture::new(Size::new(width.max(1.0), height.max(1.0)), &style)
}

/// Fixed-length lanes: the branch that builds no per-position sizing item at
/// all, so the cost is placement and the per-item commit.
fn fixed_lanes_fixture(item_count: usize) -> LayoutFixture {
    let count = item_count.max(1);
    let mut fixture = lanes_fixture(
        1024.0,
        4096.0,
        "grid-template-columns:repeat(8, 120px); gap:8px; flow-tolerance:0",
    );
    let root = fixture.root();
    for index in 0..count {
        fixture.leaf(
            root,
            &format!("width:auto; height:{}px", 24 + index % 23 * 3),
            Size::new(120.0, 24.0),
            None,
        );
    }
    fixture.prepare()
}

/// The shape the Lynx `<list list-type="waterfall">` component lowers onto:
/// `minmax(0, 1fr)` lanes and a zero tolerance, so every item lands in the
/// strictly shortest lane. The flexible maximum makes every track consult its
/// items, which is what puts the per-position clones on this path.
fn waterfall_fixture(item_count: usize) -> LayoutFixture {
    let count = item_count.max(1);
    let mut fixture = lanes_fixture(
        1024.0,
        4096.0,
        "grid-template-columns:repeat(8, minmax(0, 1fr)); gap:8px; flow-tolerance:0",
    );
    let root = fixture.root();
    for index in 0..count {
        fixture.leaf(
            root,
            &format!("width:auto; height:{}px", 24 + index % 23 * 3),
            Size::new(96.0, 24.0),
            None,
        );
    }
    fixture.prepare()
}

/// Intrinsic lanes whose items overflow their own boxes: every track consults
/// its items, and every item's scrollable overflow escapes into the
/// container's.
fn intrinsic_lanes_fixture(item_count: usize) -> LayoutFixture {
    let count = item_count.max(1);
    let mut fixture = lanes_fixture(
        1024.0,
        4096.0,
        "grid-template-columns:repeat(8, minmax(min-content, 1fr)); gap:4px",
    );
    let root = fixture.root();
    for index in 0..count {
        let item = fixture.container(
            root,
            &format!(
                "overflow:visible; width:auto; height:{}px",
                20 + index % 17 * 2
            ),
        );
        fixture.leaf(
            item,
            "width:auto; height:auto",
            Size::new(56.0 + (index % 31) as f32, 40.0),
            None,
        );
    }
    fixture.prepare()
}

/// Mixed spans with a full-span item every eighth: each full-span item levels
/// every lane, so the stacking pass alternates between a spread state and a
/// flat one.
fn mixed_span_fixture(item_count: usize) -> LayoutFixture {
    let count = item_count.max(1);
    let mut fixture = lanes_fixture(
        1024.0,
        4096.0,
        "grid-template-columns:repeat(8, minmax(0, 1fr)); gap:6px; flow-tolerance:0",
    );
    let root = fixture.root();
    for index in 0..count {
        let placement = if index % 8 == 7 {
            "grid-column:1 / -1".to_owned()
        } else {
            format!("grid-column:span {}", 1 + index % 3)
        };
        fixture.leaf(
            root,
            &format!("{placement}; width:auto; height:{}px", 16 + index % 19 * 2),
            Size::new(96.0, 16.0),
            None,
        );
    }
    fixture.prepare()
}

/// One item in the middle of a waterfall goes dirty between passes. The lane
/// every later item chooses follows its predecessors' sizes, so the pass has
/// to re-place the whole tail even though only one item changed.
#[derive(Debug)]
struct DirtyItemFixture {
    fixture: LayoutFixture,
    dirty: NodeId,
}

impl DirtyItemFixture {
    fn new() -> Self {
        const ITEMS: usize = 512;
        let mut fixture = lanes_fixture(
            1024.0,
            4096.0,
            "grid-template-columns:repeat(8, minmax(0, 1fr)); gap:8px; flow-tolerance:0",
        );
        let root = fixture.root();
        let mut dirty = root;
        for index in 0..ITEMS {
            let item = fixture.leaf(
                root,
                &format!("width:auto; height:{}px", 24 + index % 23 * 3),
                Size::new(96.0, 24.0),
                None,
            );
            if index == ITEMS / 2 {
                dirty = item;
            }
        }
        let mut fixture = fixture.prepare();
        let _ = fixture.run();
        Self { fixture, dirty }
    }

    fn run(&mut self) -> dom::layout::Layout {
        self.fixture.invalidate(self.dirty);
        self.fixture.run()
    }
}

const FIXED_BATCH: usize = 8;
const WATERFALL_BATCH: usize = 4;
const INTRINSIC_BATCH: usize = 2;
const MIXED_SPAN_BATCH: usize = 4;
const DIRTY_ITEM_BATCH: usize = 16;

fn bench_cold<Make>(bencher: divan::Bencher<'_, '_>, batch_size: usize, make_fixture: Make)
where
    Make: Fn() -> LayoutFixture + Copy,
{
    bencher
        .with_inputs(move || (0..batch_size).map(|_| make_fixture()).collect::<Vec<_>>())
        .input_counter(|fixtures: &Vec<LayoutFixture>| {
            ItemsCount::new(
                fixtures
                    .iter()
                    .map(LayoutFixture::node_count)
                    .sum::<usize>(),
            )
        })
        .bench_local_values(|mut fixtures| {
            for fixture in &mut fixtures {
                divan::black_box(fixture.run());
            }
            fixtures
        });
}

#[divan::bench(args = [256, 2_048])]
fn fixed_lanes_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_cold(bencher, FIXED_BATCH, || fixed_lanes_fixture(item_count));
}

#[divan::bench(args = [256, 2_048])]
fn waterfall_lanes_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_cold(bencher, WATERFALL_BATCH, || waterfall_fixture(item_count));
}

#[divan::bench(args = [256, 1_024])]
fn intrinsic_lanes_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_cold(bencher, INTRINSIC_BATCH, || {
        intrinsic_lanes_fixture(item_count)
    });
}

#[divan::bench(args = [256, 2_048])]
fn mixed_span_lanes_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_cold(bencher, MIXED_SPAN_BATCH, || mixed_span_fixture(item_count));
}

#[divan::bench]
fn dirty_item_relayout(bencher: divan::Bencher<'_, '_>) {
    bencher
        .counter(ItemsCount::new(DIRTY_ITEM_BATCH))
        .with_inputs(DirtyItemFixture::new)
        .bench_local_refs(|fixture| {
            for _ in 0..DIRTY_ITEM_BATCH {
                divan::black_box(fixture.run());
            }
        });
}
