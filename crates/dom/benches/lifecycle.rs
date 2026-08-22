//! Node lifecycle benchmarks: what holding, releasing, and freeing cost.
//!
//! A node is kept allocated by its parent or by a holder outside the tree
//! (`Document::release` declares that holder gone), and a node that is both
//! released and detached is freed by the next `Document::collect_unheld`,
//! which Bobcat runs at the end of every script batch. The numbers that
//! matter:
//!
//! * `remove_held_subtree` — the hot path: detaching a subtree whose root is still held frees
//!   nothing, and the only thing the lifecycle adds to it is one bit read on the root. Paired
//!   against re-attaching it, so the tree is the same shape every iteration.
//! * `release_*` — the same detached subtree let go in the two orders a finalizer can deliver, then
//!   collected at the boundary: leaves first, so only the root is queued and one walk frees
//!   everything, or root first, so the root is queued at once and every later release finds its
//!   node attached to a queued ancestor. `drop_subtree` of the same subtree is the floor both are
//!   measured against.
//! * `remove_released_subtree` — the other ending: every handle died while the subtree was
//!   attached, so the removal queues it and the boundary frees it.
//! * `collect_unheld_with_nothing_queued` — what every boundary pays when there is nothing to free.

use divan::black_box;
use divan::counter::ItemsCount;
use dom::{Document, NodeId, StylesheetOrigin};
use euclid::{Scale, Size2D};
use stylo::device::servo::FontMetricsProvider;
use stylo::font_metrics::FontMetrics;
use stylo::media_queries::MediaType;
use stylo::properties::ComputedValues;
use stylo::properties::style_structs::Font;
use stylo::queries::values::PrefersColorScheme;
use stylo::servo::media_features::PointerCapabilities;
use stylo::values::computed::font::GenericFontFamily;
use stylo::values::computed::{CSSPixelLength, Length};
use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
use stylo_traits::{CSSPixel, DevicePixel};

fn main() {
    divan::main();
}

#[derive(Debug)]
struct BenchFontMetricsProvider;

impl FontMetricsProvider for BenchFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &Font,
        base_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics {
            ascent: Length::new(base_size.px()),
            ..FontMetrics::default()
        }
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        Length::new(FONT_MEDIUM_PX)
    }
}

fn device() -> dom::Device {
    dom::standards_device(
        MediaType::screen(),
        Size2D::<f32, CSSPixel>::new(800.0, 600.0),
        Size2D::<f32, DevicePixel>::new(800.0, 600.0),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(BenchFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

const PAGE_CSS: &str = "page { display: linear; }
     row { display: linear; width: 40px; height: 12px; }
     leaf { width: 8px; height: 8px; }";

/// The subtree every benchmark works on: a root, `ROWS` rows, `LEAVES` leaves
/// under each — 1 + 64 + 256 nodes, a list cell's worth.
const ROWS: usize = 64;
const LEAVES: usize = 4;
const SUBTREE: usize = 1 + ROWS + ROWS * LEAVES;

/// How many such subtrees the page carries beside the one under test, so the
/// removal path runs against a page-sized tree rather than an empty one.
const NEIGHBOURS: usize = 15;

fn page() -> Document<()> {
    let mut doc = Document::new(device(), "page", ());
    doc.add_stylesheet(PAGE_CSS, StylesheetOrigin::Author);
    doc
}

/// Builds one subtree under `parent`; returns its nodes in creation order,
/// root first, which is the order a finalizer delivers handle deaths in.
fn subtree(doc: &mut Document<()>, parent: NodeId) -> Vec<NodeId> {
    let mut ids = Vec::with_capacity(SUBTREE);
    let root = doc.create_element("cell", ());
    doc.append_child(parent, root);
    ids.push(root);
    for _ in 0..ROWS {
        let row = doc.create_element("row", ());
        doc.append_child(root, row);
        ids.push(row);
        for _ in 0..LEAVES {
            let leaf = doc.create_element("leaf", ());
            doc.append_child(row, leaf);
            ids.push(leaf);
        }
    }
    ids
}

/// A laid-out page of `NEIGHBOURS + 1` subtrees; returns the last one's ids.
fn populated_page() -> (Document<()>, Vec<NodeId>) {
    let mut doc = page();
    let page_root = doc.document_element().id();
    let mut last = Vec::new();
    for _ in 0..=NEIGHBOURS {
        last = subtree(&mut doc, page_root);
    }
    doc.layout();
    (doc, last)
}

/// The same page with the subtree under test already detached.
fn page_with_detached_subtree() -> (Document<()>, Vec<NodeId>) {
    let (mut doc, ids) = populated_page();
    doc.remove_element(ids[0]);
    (doc, ids)
}

#[divan::bench]
fn remove_held_subtree(bencher: divan::Bencher) {
    let (mut doc, ids) = populated_page();
    let page_root = doc.document_element().id();
    let root = ids[0];
    bencher.bench_local(|| {
        doc.remove_element(black_box(root));
        doc.append_child(page_root, root);
    });
}

#[divan::bench]
fn release_leaves_first_then_root(bencher: divan::Bencher) {
    bencher
        .with_inputs(page_with_detached_subtree)
        .counter(ItemsCount::new(SUBTREE))
        .bench_local_values(|(mut doc, ids)| {
            for &id in ids.iter().rev() {
                doc.release(id);
            }
            doc.collect_unheld();
            black_box(doc)
        });
}

#[divan::bench]
fn release_root_first_then_descendants(bencher: divan::Bencher) {
    bencher
        .with_inputs(page_with_detached_subtree)
        .counter(ItemsCount::new(SUBTREE))
        .bench_local_values(|(mut doc, ids)| {
            for &id in &ids {
                doc.release(id);
            }
            doc.collect_unheld();
            black_box(doc)
        });
}

#[divan::bench]
fn drop_subtree_floor(bencher: divan::Bencher) {
    bencher
        .with_inputs(page_with_detached_subtree)
        .counter(ItemsCount::new(SUBTREE))
        .bench_local_values(|(mut doc, ids)| {
            doc.drop_subtree(ids[0]);
            black_box(doc)
        });
}

#[divan::bench]
fn remove_released_subtree(bencher: divan::Bencher) {
    bencher
        .with_inputs(|| {
            let (mut doc, ids) = populated_page();
            for &id in &ids {
                doc.release(id);
            }
            (doc, ids)
        })
        .counter(ItemsCount::new(SUBTREE))
        .bench_local_values(|(mut doc, ids)| {
            doc.remove_element(ids[0]);
            doc.collect_unheld();
            black_box(doc)
        });
}

#[divan::bench]
fn collect_unheld_with_nothing_queued(bencher: divan::Bencher) {
    let (mut doc, _ids) = populated_page();
    bencher.bench_local(|| black_box(doc.collect_unheld()));
}
