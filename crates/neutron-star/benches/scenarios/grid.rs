//! CSS Grid workloads driven through w3c-dom's production layout host.

#![allow(clippy::cast_precision_loss)]

use divan::counter::ItemsCount;
use neutron_star::geometry::Size;
use w3c_dom::NodeId;

use crate::support::{LayoutFixture, LeafContent};

fn grid_fixture(width: f32, height: f32, extra: &str) -> LayoutFixture {
    let style = format!("display:grid; width:{width}px; height:{height}px; {extra}");
    LayoutFixture::new(Size::new(width.max(1.0), height.max(1.0)), &style)
}

fn sparse_auto_fixture(item_count: usize) -> LayoutFixture {
    let count = item_count.max(1);
    let rows = count.div_ceil(32);
    let mut fixture = grid_fixture(
        1024.0,
        rows as f32 * 8.0,
        "grid-template-columns:repeat(32, 1fr); grid-auto-rows:8px; grid-auto-flow:row",
    );
    let root = fixture.root();
    for index in 0..count {
        let style = if index % 11 == 0 {
            format!(
                "grid-column-start:{}; width:auto; height:8px",
                index % 32 + 1
            )
        } else {
            "width:auto; height:8px".to_owned()
        };
        fixture.leaf(root, &style, Size::new(8.0, 8.0), None);
    }
    fixture.prepare()
}

fn dense_holes_fixture(item_count: usize, content: LeafContent) -> LayoutFixture {
    let count = item_count.max(1);
    let rows = count.div_ceil(24);
    let mut fixture = grid_fixture(
        960.0,
        rows as f32 * 10.0,
        "grid-template-columns:repeat(24, 1fr); grid-auto-rows:10px; grid-auto-flow:row dense",
    );
    let root = fixture.root();
    for index in 0..count {
        let span = match index % 7 {
            0 => 5,
            1 | 2 => 2,
            _ => 1,
        };
        fixture.leaf_with_content(
            root,
            &format!("grid-column:span {span}; width:auto; height:10px"),
            Size::new(span as f32 * 8.0, 10.0),
            None,
            content,
            index,
        );
    }
    fixture.prepare()
}

fn fixed_fr_fixture() -> LayoutFixture {
    const ITEMS: usize = 1_024;
    let mut fixture = grid_fixture(
        1024.0,
        512.0,
        "grid-template-columns:repeat(8, 32px 1fr minmax(8px, 2fr) 24px); grid-auto-rows:16px; gap:1px",
    );
    let root = fixture.root();
    for index in 0..ITEMS {
        let width = 4.0 + (index % 13) as f32;
        fixture.leaf(root, "width:auto; height:auto", Size::new(width, 8.0), None);
    }
    fixture.prepare()
}

fn intrinsic_spans_fixture(content: LeafContent) -> LayoutFixture {
    const ITEMS: usize = 768;
    let mut fixture = grid_fixture(
        1024.0,
        768.0,
        "grid-template-columns:repeat(32, minmax(min-content, 1fr)); grid-auto-rows:auto; gap:1px",
    );
    let root = fixture.root();
    for index in 0..ITEMS {
        let span = 1 + index % 8;
        let width = 12.0 + (index % 29) as f32;
        fixture.leaf_with_content(
            root,
            &format!("grid-column:span {span}; width:auto; height:auto"),
            Size::new(width, 8.0 + (index % 5) as f32),
            None,
            content,
            index,
        );
    }
    fixture.prepare()
}

fn unique_intrinsic_spans_fixture(track_count: usize, content: LeafContent) -> LayoutFixture {
    let tracks = track_count.max(1);
    let template = format!(
        "grid-template-columns:repeat({tracks}, minmax(min-content, 1fr)); grid-auto-rows:12px"
    );
    let mut fixture = grid_fixture(tracks as f32 * 8.0, tracks as f32 * 12.0, &template);
    let root = fixture.root();
    for span in 1..=tracks {
        fixture.leaf_with_content(
            root,
            &format!("grid-column:span {span}; width:auto; height:auto"),
            Size::new(span as f32 + 8.0, 10.0),
            None,
            content,
            span,
        );
    }
    fixture.prepare()
}

fn flex_freeze_threshold_fixture(track_count: usize, content: LeafContent) -> LayoutFixture {
    let tracks = track_count.max(1);
    let template =
        format!("grid-template-columns:repeat({tracks}, minmax(1px, 1fr)); grid-auto-rows:10px");
    let mut fixture = grid_fixture(tracks as f32 * 12.0, 20.0, &template);
    let root = fixture.root();
    for index in 0..tracks {
        fixture.leaf_with_content(
            root,
            &format!(
                "min-width:{}px; max-width:{}px; width:auto; height:10px",
                1 + index % 9,
                10 + index % 17
            ),
            Size::new(4.0 + (index % 13) as f32, 10.0),
            None,
            content,
            index,
        );
    }
    fixture.prepare()
}

fn nested_fixture(content: LeafContent) -> LayoutFixture {
    const CONTAINERS: usize = 32;
    const ITEMS: usize = 32;
    let mut fixture = grid_fixture(
        1024.0,
        1024.0,
        "grid-template-columns:repeat(8, 1fr); grid-template-rows:repeat(4, 1fr); gap:2px",
    );
    let root = fixture.root();
    for container_index in 0..CONTAINERS {
        let container = fixture.container(
            root,
            "display:grid; grid-template-columns:repeat(8, 1fr); grid-template-rows:repeat(4, 1fr); width:auto; height:auto; gap:1px",
        );
        for item_index in 0..ITEMS {
            let span = 1 + (container_index + item_index) % 3;
            fixture.leaf_with_content(
                container,
                &format!("grid-column:span {span}; width:auto; height:auto"),
                Size::new(3.0 + (item_index % 7) as f32, 3.0),
                None,
                content,
                container_index * ITEMS + item_index,
            );
        }
    }
    fixture.prepare()
}

#[derive(Debug)]
struct DirtyNestedFixture {
    fixture: LayoutFixture,
    dirty: NodeId,
}

impl DirtyNestedFixture {
    fn new() -> Self {
        let mut fixture = grid_fixture(
            1024.0,
            1024.0,
            "grid-template-columns:repeat(8, 1fr); grid-template-rows:repeat(4, 1fr)",
        );
        let root = fixture.root();
        let mut dirty = root;
        for container_index in 0..32 {
            let container = fixture.container(
                root,
                "display:grid; grid-template-columns:repeat(8, 1fr); grid-template-rows:repeat(4, 1fr); width:auto; height:auto",
            );
            for item_index in 0..32 {
                let leaf = fixture.leaf(
                    container,
                    "width:auto; height:auto",
                    Size::new(4.0 + (item_index % 5) as f32, 4.0),
                    None,
                );
                if container_index == 16 && item_index == 16 {
                    dirty = leaf;
                }
            }
        }
        let mut fixture = fixture.prepare();
        let _ = fixture.run();
        Self { fixture, dirty }
    }

    fn run(&mut self) -> w3c_dom::layout::Layout {
        self.fixture.invalidate(self.dirty);
        self.fixture.run()
    }
}

const FIXED_TRACKS_BATCH: usize = 16;
const INTRINSIC_SPANS_BATCH: usize = 16;
const WARM_DESCENDANTS_BATCH: usize = 256;
const ROOT_CACHE_HIT_BATCH: usize = 1_024;
const DIRTY_PATH_BATCH: usize = 32;

const fn sparse_batch_size(item_count: usize) -> usize {
    if item_count <= 256 { 64 } else { 4 }
}

const fn dense_batch_size(item_count: usize) -> usize {
    if item_count <= 256 { 8 } else { 1 }
}

const fn unique_span_batch_size(track_count: usize) -> usize {
    match track_count {
        32 => 256,
        128 => 32,
        _ => 2,
    }
}

const fn flex_freeze_batch_size(track_count: usize) -> usize {
    match track_count {
        32 => 256,
        256 => 32,
        _ => 8,
    }
}

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

#[divan::bench(args = [256, 4_096])]
fn sparse_auto_placement_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_cold(bencher, sparse_batch_size(item_count), || {
        sparse_auto_fixture(item_count)
    });
}

#[divan::bench(args = [256, 1_024])]
fn dense_hole_backfill_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_cold(bencher, dense_batch_size(item_count), || {
        dense_holes_fixture(item_count, LeafContent::Synthetic)
    });
}

#[divan::bench(args = [256, 1_024])]
fn dense_hole_backfill_with_text_cold(bencher: divan::Bencher<'_, '_>, item_count: usize) {
    bench_cold(bencher, 1, || {
        dense_holes_fixture(item_count, LeafContent::Text)
    });
}

#[divan::bench]
fn fixed_and_fractional_tracks_cold(bencher: divan::Bencher<'_, '_>) {
    bench_cold(bencher, FIXED_TRACKS_BATCH, fixed_fr_fixture);
}

#[divan::bench]
fn intrinsic_spanning_items_cold(bencher: divan::Bencher<'_, '_>) {
    bench_cold(bencher, INTRINSIC_SPANS_BATCH, || {
        intrinsic_spans_fixture(LeafContent::Synthetic)
    });
}

#[divan::bench]
fn intrinsic_spanning_items_with_text_cold(bencher: divan::Bencher<'_, '_>) {
    bench_cold(bencher, 1, || intrinsic_spans_fixture(LeafContent::Text));
}

#[divan::bench(args = [32, 128, 512])]
fn unique_intrinsic_span_buckets_cold(bencher: divan::Bencher<'_, '_>, track_count: usize) {
    bench_cold(bencher, unique_span_batch_size(track_count), || {
        unique_intrinsic_spans_fixture(track_count, LeafContent::Synthetic)
    });
}

#[divan::bench(args = [32, 128, 512])]
fn unique_intrinsic_span_buckets_with_text_cold(
    bencher: divan::Bencher<'_, '_>,
    track_count: usize,
) {
    bench_cold(bencher, 1, || {
        unique_intrinsic_spans_fixture(track_count, LeafContent::Text)
    });
}

#[divan::bench(args = [32, 256, 1_024])]
fn flexible_track_freeze_thresholds_cold(bencher: divan::Bencher<'_, '_>, track_count: usize) {
    bench_cold(bencher, flex_freeze_batch_size(track_count), || {
        flex_freeze_threshold_fixture(track_count, LeafContent::Synthetic)
    });
}

#[divan::bench(args = [32, 256, 1_024])]
fn flexible_track_freeze_thresholds_with_text_cold(
    bencher: divan::Bencher<'_, '_>,
    track_count: usize,
) {
    bench_cold(bencher, 1, || {
        flex_freeze_threshold_fixture(track_count, LeafContent::Text)
    });
}

#[divan::bench]
fn nested_grid_cold(bencher: divan::Bencher<'_, '_>) {
    bench_cold(bencher, 1, || nested_fixture(LeafContent::Synthetic));
}

#[divan::bench]
fn nested_grid_with_text_cold(bencher: divan::Bencher<'_, '_>) {
    bench_cold(bencher, 1, || nested_fixture(LeafContent::Text));
}

#[divan::bench]
fn nested_grid_warm_descendants(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            let mut fixture = nested_fixture(LeafContent::Synthetic);
            let _ = fixture.run();
            fixture.invalidate_root();
            fixture
        })
        .input_counter(|fixture| ItemsCount::new(fixture.node_count() * WARM_DESCENDANTS_BATCH))
        .bench_local_refs(|fixture| {
            for _ in 0..WARM_DESCENDANTS_BATCH {
                divan::black_box(fixture.run());
                fixture.invalidate_root();
            }
        });
}

#[divan::bench]
fn nested_grid_warm_root_cache_hit(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| {
            let mut fixture = nested_fixture(LeafContent::Synthetic);
            let _ = fixture.run();
            fixture
        })
        .input_counter(|fixture| ItemsCount::new(fixture.node_count() * ROOT_CACHE_HIT_BATCH))
        .bench_local_refs(|fixture| {
            for _ in 0..ROOT_CACHE_HIT_BATCH {
                divan::black_box(divan::black_box(&mut *fixture).run());
            }
        });
}

#[divan::bench]
fn nested_grid_dirty_leaf_and_ancestors(bencher: divan::Bencher<'_, '_>) {
    bencher
        .counter(ItemsCount::new(DIRTY_PATH_BATCH))
        .with_inputs(DirtyNestedFixture::new)
        .bench_local_refs(|fixture| {
            for _ in 0..DIRTY_PATH_BATCH {
                divan::black_box(fixture.run());
            }
        });
}
