//! Styling-system benchmarks, tracked by `CodSpeed` (walltime mode on the
//! macOS CI runner).
//!
//! Covers the hot paths of the high-performance goals in
//! `docs/style-assumptions.md`: `StyleInfo` ingestion by direct construction
//! (§B.5), the initial cascade over a realistic tree — sequential and
//! parallel (§B.6) — and the incremental restyle paths driven by
//! invalidation sets (§B.7). No native-C++-Lynx comparison harness yet; these
//! establish the absolute numbers and guard against regressions.

use std::cell::RefCell;

use divan::black_box;
use divan::counter::ItemsCount;
use lynx_template_decoder::StyleInfo;
use lynx_widget::{EngineMetrics, Parallelism, StyleEngine, WidgetHandle, WidgetTree};

fn main() {
    divan::main();
}

/// 186 real-world class rules (`.class1` … — vendored lynx-stack build
/// artifact).
const LARGE_CSS: &[u8] = include_bytes!(
    "../../lynx-template-decoder/tests/fixtures/basic-performance-large-css.web.bundle"
);

// Keep CodSpeed's measured closure in the millisecond range. Setup for cold
// workloads stays in Divan's input generator; stateful workloads execute the
// same transition repeatedly inside one sample.
const INGEST_BATCH: usize = 16;
const INITIAL_FLUSH_BATCH: usize = 2;
const INCREMENTAL_BATCH: usize = 1_024;
const NO_OP_BATCH: usize = 65_536;

/// A 750×1334 CSS-px view (so `1rpx = 1px`) at DPR 2.
fn metrics() -> EngineMetrics {
    EngineMetrics::new(750.0, 1334.0, 2.0)
}

fn large_style_info() -> StyleInfo {
    lynx_template_decoder::decode(LARGE_CSS)
        .expect("fixture decodes")
        .style_info
        .expect("fixture carries StyleInfo")
}

/// One tree with its private document stylesheet loaded.
fn engine_with_large_css() -> (StyleEngine, WidgetTree) {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_widget_tree();
    engine.load_style_info(&mut tree, &large_style_info());
    (engine, tree)
}

/// `page > 32 × view > 32 × view`, classes cycling through the fixture's
/// `.classN` rules. Returns one deep leaf.
fn build_tree(tree: &mut WidgetTree) -> WidgetHandle {
    let page = tree.create_page();
    let mut class_index = 0usize;
    let mut probe = None;
    for _ in 0..32 {
        let container = tree.create_view();
        tree.append_element(&container, &page).unwrap();
        class_index += 1;
        tree.set_classes(&container, &format!("class{}", class_index % 186 + 1))
            .unwrap();
        for _ in 0..32 {
            let leaf = tree.create_view();
            tree.append_element(&leaf, &container).unwrap();
            class_index += 1;
            tree.set_classes(&leaf, &format!("class{}", class_index % 186 + 1))
                .unwrap();
            probe = Some(leaf);
        }
    }
    probe.expect("tree has leaves")
}

fn unflushed() -> (StyleEngine, WidgetTree, WidgetHandle) {
    let (engine, mut tree) = engine_with_large_css();
    let probe = build_tree(&mut tree);
    (engine, tree, probe)
}

/// `StyleInfo` → stylo rule objects + stylist flush, by direct construction
/// (one selector-list parse per rule, per-property value parses). This is the
/// bundle-load startup path.
#[divan::bench]
fn ingest_large_style_info(bencher: divan::Bencher<'_, '_>) {
    let info = large_style_info();
    bencher
        .counter(ItemsCount::new(INGEST_BATCH))
        .with_inputs(|| {
            (0..INGEST_BATCH)
                .map(|_| {
                    let engine = StyleEngine::new(metrics());
                    let tree = engine.new_widget_tree();
                    (engine, tree)
                })
                .collect::<Vec<_>>()
        })
        .bench_local_values(|mut pairs| {
            for (engine, tree) in &mut pairs {
                engine.load_style_info(tree, black_box(&info));
            }
            pairs
        });
}

/// First full style pass over ~1.1k widgets, single-threaded.
#[divan::bench]
fn initial_flush_1k_sequential(bencher: divan::Bencher<'_, '_>) {
    bencher
        .counter(ItemsCount::new(INITIAL_FLUSH_BATCH))
        .with_inputs(|| {
            (0..INITIAL_FLUSH_BATCH)
                .map(|_| unflushed())
                .collect::<Vec<_>>()
        })
        .bench_local_values(|mut states| {
            for (engine, tree, _) in &mut states {
                engine.flush_widget_tree_with(tree, Parallelism::Sequential);
            }
            states
        });
}

/// First full style pass over ~1.1k widgets on the style thread pool
/// (Firefox-style work stealing; stylo falls back to sequential for narrow
/// levels on its own).
#[divan::bench]
fn initial_flush_1k_parallel(bencher: divan::Bencher<'_, '_>) {
    bencher
        .counter(ItemsCount::new(INITIAL_FLUSH_BATCH))
        .with_inputs(|| {
            (0..INITIAL_FLUSH_BATCH)
                .map(|_| unflushed())
                .collect::<Vec<_>>()
        })
        .bench_local_values(|mut states| {
            for (engine, tree, _) in &mut states {
                engine.flush_widget_tree_with(tree, Parallelism::Auto);
            }
            states
        });
}

/// Class flip on one leaf + flush: the invalidation-set fast path (snapshot
/// diffing restyles only the elements whose rules could be affected).
#[divan::bench]
fn incremental_class_flip(bencher: divan::Bencher<'_, '_>) {
    let (engine, mut tree, probe) = unflushed();
    engine.flush_widget_tree(&mut tree);
    let state = RefCell::new((tree, false));
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let (tree, toggle) = &mut *state.borrow_mut();
                *toggle = !*toggle;
                let class = if *toggle { "class7" } else { "class9" };
                tree.set_classes(&probe, black_box(class)).unwrap();
                engine.flush_widget_tree(tree);
            }
        });
}

/// Single-property inline-style update + flush: stylo's style-attribute
/// replacement hint (swaps one cascade level, no selector re-matching).
#[divan::bench]
fn incremental_inline_style(bencher: divan::Bencher<'_, '_>) {
    let (engine, mut tree, probe) = unflushed();
    engine.flush_widget_tree(&mut tree);
    let state = RefCell::new((tree, false));
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let (tree, toggle) = &mut *state.borrow_mut();
                *toggle = !*toggle;
                let width = if *toggle { "10px" } else { "20px" };
                tree.add_inline_style(&probe, "width", black_box(width))
                    .unwrap();
                engine.flush_widget_tree(tree);
            }
        });
}

/// A flush with nothing scheduled: the per-frame overhead floor.
#[divan::bench]
fn no_op_flush(bencher: divan::Bencher<'_, '_>) {
    let (engine, mut tree, _) = unflushed();
    engine.flush_widget_tree(&mut tree);
    let state = RefCell::new(tree);
    bencher
        .counter(ItemsCount::new(NO_OP_BATCH))
        .bench_local(|| {
            for _ in 0..NO_OP_BATCH {
                engine.flush_widget_tree(&mut state.borrow_mut());
            }
        });
}
