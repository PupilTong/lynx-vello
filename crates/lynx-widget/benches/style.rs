//! Styling-system benchmarks, tracked by `CodSpeed` (walltime mode on the
//! macOS CI runner).

use std::cell::RefCell;

use divan::black_box;
use divan::counter::ItemsCount;
use lynx_template_decoder::StyleInfo;
use lynx_widget::{Parallelism, StyleEngine, ViewMetrics, WidgetHandle, WidgetTree};

fn main() {
    divan::main();
}

const LARGE_CSS: &[u8] = include_bytes!(
    "../../lynx-template-decoder/tests/fixtures/basic-performance-large-css.web.bundle"
);

const INGEST_BATCH: usize = 16;
const INITIAL_FLUSH_BATCH: usize = 2;
const INCREMENTAL_BATCH: usize = 1_024;
const NO_OP_BATCH: usize = 65_536;
const TREE_WIDGETS: usize = 1 + 32 + 32 * 32;
const TREE_BUILD_BATCH: usize = 8;

fn metrics() -> ViewMetrics {
    ViewMetrics::new(750.0, 1334.0, 2.0)
}

fn large_style_info() -> StyleInfo {
    lynx_template_decoder::decode(LARGE_CSS)
        .expect("fixture decodes")
        .style_info
        .expect("fixture carries StyleInfo")
}

fn engine_with_large_css() -> (StyleEngine, WidgetTree) {
    let engine = StyleEngine::new(metrics());
    let mut tree = engine.new_tree();
    engine.load_style_info(&mut tree, &large_style_info());
    (engine, tree)
}

fn build_tree(tree: &mut WidgetTree) -> WidgetHandle {
    let page = tree.create_page();
    let mut class_index = 0usize;
    let mut probe = None;
    for _ in 0..32 {
        let container = tree.create_view();
        tree.append_child(&page, &container).unwrap();
        class_index += 1;
        tree.set_classes(&container, &format!("class{}", class_index % 186 + 1))
            .unwrap();
        for _ in 0..32 {
            let leaf = tree.create_view();
            tree.append_child(&container, &leaf).unwrap();
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

#[divan::bench]
fn build_drop_widget_tree_1k(bencher: divan::Bencher<'_, '_>) {
    let engine = StyleEngine::new(metrics());
    bencher
        .counter(ItemsCount::new(TREE_WIDGETS * TREE_BUILD_BATCH))
        .bench_local(|| {
            for _ in 0..TREE_BUILD_BATCH {
                let mut tree = engine.new_tree();
                let probe = build_tree(&mut tree);
                black_box(&tree);
                black_box(&probe);
                drop(probe);
                drop(tree);
            }
        });
}

#[divan::bench]
fn ingest_large_style_info(bencher: divan::Bencher<'_, '_>) {
    let info = large_style_info();
    bencher
        .counter(ItemsCount::new(INGEST_BATCH))
        .with_inputs(|| {
            (0..INGEST_BATCH)
                .map(|_| {
                    let engine = StyleEngine::new(metrics());
                    let tree = engine.new_tree();
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
                engine.flush_styles_with_parallelism(tree, Parallelism::Sequential);
            }
            states
        });
}

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
                engine.flush_styles_with_parallelism(tree, Parallelism::Auto);
            }
            states
        });
}

#[divan::bench]
fn incremental_class_flip(bencher: divan::Bencher<'_, '_>) {
    let (engine, mut tree, probe) = unflushed();
    engine.flush_styles(&mut tree);
    let state = RefCell::new((tree, false));
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let (tree, toggle) = &mut *state.borrow_mut();
                *toggle = !*toggle;
                let class = if *toggle { "class7" } else { "class9" };
                tree.set_classes(&probe, black_box(class)).unwrap();
                engine.flush_styles(tree);
            }
        });
}

#[divan::bench]
fn incremental_inline_style(bencher: divan::Bencher<'_, '_>) {
    let (engine, mut tree, probe) = unflushed();
    engine.flush_styles(&mut tree);
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
                engine.flush_styles(tree);
            }
        });
}

#[divan::bench]
fn no_op_flush(bencher: divan::Bencher<'_, '_>) {
    let (engine, mut tree, _) = unflushed();
    engine.flush_styles(&mut tree);
    let state = RefCell::new(tree);
    bencher
        .counter(ItemsCount::new(NO_OP_BATCH))
        .bench_local(|| {
            for _ in 0..NO_OP_BATCH {
                engine.flush_styles(&mut state.borrow_mut());
            }
        });
}
