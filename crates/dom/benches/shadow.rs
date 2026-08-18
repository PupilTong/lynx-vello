//! Shadow-DOM benchmarks: what the flat tree costs on top of a plain one.
//!
//! Every measurement here is a **paired A/B**. The `_plain` half builds the
//! same number of nodes in the same rendered shape with no shadow root at
//! all; the `_shadow` half routes the identical flat tree through hosts and
//! slots. The pair is the measurement — a `_shadow` number on its own says
//! nothing about whether shadow DOM is what made it slow.
//!
//! Commits go through the production `Document::layout`, and frames through
//! the production `Document::render` (CPU side only, no GPU dispatch).

use std::cell::RefCell;

use divan::black_box;
use divan::counter::ItemsCount;
use dom::{Document, NodeId, ShadowRootMode, StylesheetOrigin};
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

const HOSTS: usize = 128;
const ROWS_PER_HOST: usize = 8;
const WIDE_ROWS: usize = 1_024;
const NESTING_DEPTH: usize = 48;

const BUILD_BATCH: usize = 4;
const COMMIT_BATCH: usize = 2;
const NO_OP_BATCH: usize = 4_096;
const INCREMENTAL_BATCH: usize = 512;
const RENDER_BATCH: usize = 64;

const PAGE_CSS: &str = "page { display: linear; }
     host { display: linear; width: 400px; }
     frame { display: linear; }
     slot { display: linear; }
     row { display: linear; width: 40px; height: 12px; background-color: rgb(20, 40, 60); }
     row.hot { background-color: rgb(200, 40, 60); }";

const COMPONENT_CSS: &str = "frame { display: linear; }
     slot { display: linear; }
     ::slotted(row) { margin-left: 1px; }";

fn page() -> Document<()> {
    let mut doc = Document::new(device(), "page", ());
    doc.add_stylesheet(PAGE_CSS, StylesheetOrigin::Author);
    doc
}

fn plain_host(doc: &mut Document<()>, parent: NodeId, rows: usize) -> NodeId {
    let host = doc.create_element("host", ());
    doc.append_child(parent, host);
    let frame = doc.create_element("frame", ());
    doc.append_child(host, frame);
    let slot = doc.create_element("slot", ());
    doc.append_child(frame, slot);
    for _ in 0..rows {
        let row = doc.create_element("row", ());
        doc.append_child(slot, row);
    }
    host
}

fn shadow_host(doc: &mut Document<()>, parent: NodeId, rows: usize) -> NodeId {
    let host = doc.create_element("host", ());
    doc.append_child(parent, host);
    let shadow = doc.attach_shadow(host, ShadowRootMode::Open);
    doc.add_shadow_stylesheet(shadow, COMPONENT_CSS);
    let frame = doc.create_element("frame", ());
    doc.append_child(shadow, frame);
    let slot = doc.create_element("slot", ());
    doc.append_child(frame, slot);
    for _ in 0..rows {
        let row = doc.create_element("row", ());
        doc.append_child(host, row);
    }
    host
}

fn plain_page(hosts: usize, rows: usize) -> (Document<()>, NodeId) {
    let mut doc = page();
    let root = doc.document_element().id();
    let mut probe = root;
    for _ in 0..hosts {
        let host = plain_host(&mut doc, root, rows);
        probe = doc
            .get(host)
            .and_then(|node| {
                node.child_ids()
                    .next()
                    .and_then(|frame| doc.get(frame))
                    .and_then(|frame| frame.child_ids().next())
            })
            .and_then(|slot| doc.get(slot).and_then(|slot| slot.child_ids().next()))
            .unwrap_or(probe);
    }
    (doc, probe)
}

fn shadow_page(hosts: usize, rows: usize) -> (Document<()>, NodeId) {
    let mut doc = page();
    let root = doc.document_element().id();
    let mut probe = root;
    for _ in 0..hosts {
        let host = shadow_host(&mut doc, root, rows);
        probe = doc
            .get(host)
            .and_then(|node| node.child_ids().next())
            .unwrap_or(probe);
    }
    (doc, probe)
}

fn shadow_page_unstyled(hosts: usize, rows: usize) -> (Document<()>, NodeId) {
    let mut doc = page();
    let root = doc.document_element().id();
    let mut probe = root;
    for _ in 0..hosts {
        let host = doc.create_element("host", ());
        doc.append_child(root, host);
        let shadow = doc.attach_shadow(host, ShadowRootMode::Open);
        let frame = doc.create_element("frame", ());
        doc.append_child(shadow, frame);
        let slot = doc.create_element("slot", ());
        doc.append_child(frame, slot);
        for _ in 0..rows {
            let row = doc.create_element("row", ());
            doc.append_child(host, row);
            probe = row;
        }
    }
    (doc, probe)
}

fn committed(build: fn(usize, usize) -> (Document<()>, NodeId)) -> (Document<()>, NodeId) {
    let (mut doc, probe) = build(HOSTS, ROWS_PER_HOST);
    doc.layout();
    (doc, probe)
}

#[divan::bench]
fn build_plain(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(BUILD_BATCH))
        .bench_local(|| {
            for _ in 0..BUILD_BATCH {
                black_box(plain_page(HOSTS, ROWS_PER_HOST));
            }
        });
}

#[divan::bench]
fn build_shadow(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(BUILD_BATCH))
        .bench_local(|| {
            for _ in 0..BUILD_BATCH {
                black_box(shadow_page(HOSTS, ROWS_PER_HOST));
            }
        });
}

#[divan::bench]
fn build_shadow_no_scoped_css(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(BUILD_BATCH))
        .bench_local(|| {
            for _ in 0..BUILD_BATCH {
                black_box(shadow_page_unstyled(HOSTS, ROWS_PER_HOST));
            }
        });
}

#[divan::bench]
fn build_wide_host_plain(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(WIDE_ROWS))
        .bench_local(|| black_box(plain_page(1, WIDE_ROWS)));
}

#[divan::bench]
fn build_wide_host_shadow(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(WIDE_ROWS))
        .bench_local(|| black_box(shadow_page(1, WIDE_ROWS)));
}

#[divan::bench]
fn initial_commit_plain(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(COMMIT_BATCH))
        .with_inputs(|| {
            (0..COMMIT_BATCH)
                .map(|_| plain_page(HOSTS, ROWS_PER_HOST))
                .collect::<Vec<_>>()
        })
        .bench_local_values(|mut states| {
            for (doc, _) in &mut states {
                doc.layout();
            }
            states
        });
}

#[divan::bench]
fn initial_commit_shadow(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(COMMIT_BATCH))
        .with_inputs(|| {
            (0..COMMIT_BATCH)
                .map(|_| shadow_page(HOSTS, ROWS_PER_HOST))
                .collect::<Vec<_>>()
        })
        .bench_local_values(|mut states| {
            for (doc, _) in &mut states {
                doc.layout();
            }
            states
        });
}

#[divan::bench]
fn noop_commit_plain(bencher: divan::Bencher) {
    let state = RefCell::new(committed(plain_page));
    bencher
        .counter(ItemsCount::new(NO_OP_BATCH))
        .bench_local(|| {
            for _ in 0..NO_OP_BATCH {
                state.borrow_mut().0.layout();
            }
        });
}

#[divan::bench]
fn noop_commit_shadow(bencher: divan::Bencher) {
    let state = RefCell::new(committed(shadow_page));
    bencher
        .counter(ItemsCount::new(NO_OP_BATCH))
        .bench_local(|| {
            for _ in 0..NO_OP_BATCH {
                state.borrow_mut().0.layout();
            }
        });
}

fn class_flip(bencher: divan::Bencher, build: fn(usize, usize) -> (Document<()>, NodeId)) {
    let state = RefCell::new(committed(build));
    let mut on = false;
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let (doc, probe) = &mut *state.borrow_mut();
                on = !on;
                if on {
                    doc.add_class(*probe, "hot");
                } else {
                    doc.remove_class(*probe, "hot");
                }
                doc.layout();
            }
        });
}

#[divan::bench]
fn incremental_class_flip_plain(bencher: divan::Bencher) {
    class_flip(bencher, plain_page);
}

#[divan::bench]
fn incremental_class_flip_shadow(bencher: divan::Bencher) {
    class_flip(bencher, shadow_page);
}

#[divan::bench]
fn slot_attribute_flip(bencher: divan::Bencher) {
    let mut doc = page();
    let root = doc.document_element().id();
    let host = doc.create_element("host", ());
    doc.append_child(root, host);
    let shadow = doc.attach_shadow(host, ShadowRootMode::Open);
    doc.add_shadow_stylesheet(
        shadow,
        "slot { display: linear; } slot[name=b] { margin-top: 2px; }",
    );
    for name in ["a", "b"] {
        let slot = doc.create_element("slot", ());
        doc.set_attribute(slot, "name", name);
        doc.append_child(shadow, slot);
    }
    let mut probe = host;
    for _ in 0..ROWS_PER_HOST {
        let row = doc.create_element("row", ());
        doc.set_attribute(row, "slot", "a");
        doc.append_child(host, row);
        probe = row;
    }
    doc.layout();

    let state = RefCell::new(doc);
    let mut on = false;
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let doc = &mut *state.borrow_mut();
                on = !on;
                doc.set_attribute(probe, "slot", if on { "b" } else { "a" });
                doc.layout();
            }
        });
}

fn nested_components(depth: usize) -> Document<()> {
    let mut doc = page();
    let mut parent = doc.document_element().id();
    for _ in 0..depth {
        let host = doc.create_element("host", ());
        doc.append_child(parent, host);
        let shadow = doc.attach_shadow(host, ShadowRootMode::Open);
        doc.add_shadow_stylesheet(shadow, COMPONENT_CSS);
        let frame = doc.create_element("frame", ());
        doc.append_child(shadow, frame);
        let slot = doc.create_element("slot", ());
        doc.append_child(frame, slot);
        parent = host;
    }
    let row = doc.create_element("row", ());
    doc.append_child(parent, row);
    doc
}

#[divan::bench]
fn deep_nesting_commit(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(COMMIT_BATCH))
        .with_inputs(|| {
            (0..COMMIT_BATCH)
                .map(|_| nested_components(NESTING_DEPTH))
                .collect::<Vec<_>>()
        })
        .bench_local_values(|mut docs| {
            for doc in &mut docs {
                doc.layout();
            }
            docs
        });
}

#[divan::bench]
fn scoped_stylesheet_install(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(HOSTS))
        .with_inputs(|| {
            let mut doc = page();
            let root = doc.document_element().id();
            let roots = (0..HOSTS)
                .map(|_| {
                    let host = doc.create_element("host", ());
                    doc.append_child(root, host);
                    doc.attach_shadow(host, ShadowRootMode::Open)
                })
                .collect::<Vec<_>>();
            (doc, roots)
        })
        .bench_local_values(|(mut doc, roots)| {
            for shadow in roots {
                doc.add_shadow_stylesheet(shadow, black_box(COMPONENT_CSS));
            }
            doc
        });
}

fn render(bencher: divan::Bencher, build: fn(usize, usize) -> (Document<()>, NodeId)) {
    let (mut doc, probe) = committed(build);
    doc.render();
    let state = RefCell::new((doc, probe));
    let mut on = false;
    bencher
        .counter(ItemsCount::new(RENDER_BATCH))
        .bench_local(|| {
            for _ in 0..RENDER_BATCH {
                let (doc, probe) = &mut *state.borrow_mut();
                on = !on;
                if on {
                    doc.add_class(*probe, "hot");
                } else {
                    doc.remove_class(*probe, "hot");
                }
                doc.layout();
                doc.render();
            }
        });
}

#[divan::bench]
fn render_plain(bencher: divan::Bencher) {
    render(bencher, plain_page);
}

#[divan::bench]
fn render_shadow(bencher: divan::Bencher) {
    render(bencher, shadow_page);
}
