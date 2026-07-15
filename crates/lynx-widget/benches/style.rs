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
use lynx_template_decoder::StyleInfo;
use lynx_widget::{EngineMetrics, Parallelism, WidgetHandle, WidgetTree};

fn main() {
    divan::main();
}

/// 186 real-world class rules (`.class1` … — vendored lynx-stack build
/// artifact).
const LARGE_CSS: &[u8] = include_bytes!(
    "../../lynx-template-decoder/tests/fixtures/basic-performance-large-css.web.bundle"
);

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

/// A document with the large fixture stylesheet loaded.
fn document_with_large_css() -> WidgetTree {
    let mut document = WidgetTree::with_metrics(metrics());
    document.load_style_info(&large_style_info());
    document
}

/// `page > 32 × view > 32 × view`, classes cycling through the fixture's
/// `.classN` rules. Returns the tree and one deep leaf.
fn populate_tree(tree: &mut WidgetTree) -> WidgetHandle {
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

fn build_tree() -> (WidgetTree, WidgetHandle) {
    let mut tree = document_with_large_css();
    let probe = populate_tree(&mut tree);
    (tree, probe)
}

/// `StyleInfo` → stylo rule objects + stylist flush, by direct construction
/// (one selector-list parse per rule, per-property value parses). This is the
/// bundle-load startup path.
#[divan::bench]
fn ingest_large_style_info(bencher: divan::Bencher<'_, '_>) {
    let info = large_style_info();
    bencher
        .with_inputs(|| WidgetTree::with_metrics(metrics()))
        .bench_local_values(|mut document| {
            document.load_style_info(black_box(&info));
            document
        });
}

/// First full style pass over ~1.1k widgets, single-threaded.
#[divan::bench]
fn initial_flush_1k_sequential(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| build_tree().0)
        .bench_local_values(|mut tree| {
            tree.flush_styles_with(Parallelism::Sequential);
            tree
        });
}

/// First full style pass over ~1.1k widgets on the style thread pool
/// (Firefox-style work stealing; stylo falls back to sequential for narrow
/// levels on its own).
#[divan::bench]
fn initial_flush_1k_parallel(bencher: divan::Bencher<'_, '_>) {
    bencher
        .with_inputs(|| build_tree().0)
        .bench_local_values(|mut tree| {
            tree.flush_styles_with(Parallelism::Auto);
            tree
        });
}

/// Class flip on one leaf + flush: the invalidation-set fast path (snapshot
/// diffing restyles only the elements whose rules could be affected).
#[divan::bench]
fn incremental_class_flip(bencher: divan::Bencher<'_, '_>) {
    let (mut tree, probe) = build_tree();
    tree.flush_styles();
    let state = RefCell::new((tree, false));
    bencher.bench_local(|| {
        let (tree, toggle) = &mut *state.borrow_mut();
        *toggle = !*toggle;
        let class = if *toggle { "class7" } else { "class9" };
        tree.set_classes(&probe, black_box(class)).unwrap();
        tree.flush_styles();
    });
}

/// Single-property inline-style update + flush: stylo's style-attribute
/// replacement hint (swaps one cascade level, no selector re-matching).
#[divan::bench]
fn incremental_inline_style(bencher: divan::Bencher<'_, '_>) {
    let (mut tree, probe) = build_tree();
    tree.flush_styles();
    let state = RefCell::new((tree, false));
    bencher.bench_local(|| {
        let (tree, toggle) = &mut *state.borrow_mut();
        *toggle = !*toggle;
        let width = if *toggle { "10px" } else { "20px" };
        tree.add_inline_style(&probe, "width", black_box(width))
            .unwrap();
        tree.flush_styles();
    });
}

/// A flush with nothing scheduled: the per-frame overhead floor.
#[divan::bench]
fn no_op_flush(bencher: divan::Bencher<'_, '_>) {
    let (mut tree, _) = build_tree();
    tree.flush_styles();
    let state = RefCell::new(tree);
    bencher.bench_local(|| {
        state.borrow_mut().flush_styles();
    });
}

/// The standalone per-element resolve (match + cascade, no traversal, no
/// style sharing) — the cache-less baseline the flush path improves on.
#[divan::bench]
fn resolve_single_widget(bencher: divan::Bencher<'_, '_>) {
    let (tree, probe) = build_tree();
    let parent = tree.get_parent(&probe).unwrap();
    let parent_style = tree.resolve_widget(&parent, None).unwrap();
    bencher.bench_local(|| {
        tree.resolve_widget(black_box(&probe), Some(&parent_style))
            .unwrap()
    });
}
