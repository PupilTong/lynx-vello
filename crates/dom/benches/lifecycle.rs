//! Node lifecycle benchmarks: what removing and freeing cost.
//!
//! A node is kept allocated by the one owner outside the tree that names it —
//! in Bobcat, its script handle. A removal only unlinks; a `Document::drop_element`
//! frees exactly the node it names, unlinking its element children into detached
//! roots for their own owners, so unmounting an N-node subtree is N drops in
//! whatever order the collector delivers the handle deaths. The numbers that
//! matter:
//!
//! * `remove_subtree` — the hot path: detaching a subtree frees nothing and touches nothing under
//!   its root. Paired against re-attaching it, so the tree is the same shape every iteration.
//! * `drop_*` — the same detached subtree freed one node at a time, in the two orders a finalizer
//!   can deliver: leaves first, so every drop finds its node still linked to a live parent, or root
//!   first, so each drop unlinks the children it is about to orphan. `drop_subtree` of the same
//!   subtree — one walk, no per-node unlink — is the floor both are measured against, and the
//!   difference between them is what the handle-per-node model costs.
//! * `drop_element_one_node` — a single leaf freed: what one finalizer pays.

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
fn remove_subtree(bencher: divan::Bencher) {
    let (mut doc, ids) = populated_page();
    let page_root = doc.document_element().id();
    let root = ids[0];
    bencher.bench_local(|| {
        doc.remove_element(black_box(root));
        doc.append_child(page_root, root);
    });
}

#[divan::bench]
fn drop_leaves_first(bencher: divan::Bencher) {
    bencher
        .with_inputs(page_with_detached_subtree)
        .counter(ItemsCount::new(SUBTREE))
        .bench_local_values(|(mut doc, ids)| {
            for &id in ids.iter().rev() {
                doc.drop_element(id);
            }
            black_box(doc)
        });
}

#[divan::bench]
fn drop_root_first(bencher: divan::Bencher) {
    bencher
        .with_inputs(page_with_detached_subtree)
        .counter(ItemsCount::new(SUBTREE))
        .bench_local_values(|(mut doc, ids)| {
            for &id in &ids {
                doc.drop_element(id);
            }
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

/// One finalizer's worth: a leaf still linked to a live parent.
#[divan::bench]
fn drop_element_one_node(bencher: divan::Bencher) {
    bencher
        .with_inputs(populated_page)
        .bench_local_values(|(mut doc, ids)| {
            doc.drop_element(black_box(ids[SUBTREE - 1]));
            black_box(doc)
        });
}
