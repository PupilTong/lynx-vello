//! Custom-element benchmarks: what the lifecycle machinery costs a document
//! that does not use it, and what it costs one that does.
//!
//! Paired A/B throughout, in three steps rather than two, because there are
//! two separate costs to separate:
//!
//! * `_plain` — no definitions at all, so every hook takes its empty-registry branch. This is the
//!   number that must not move: it is what every existing embedder pays for a feature it never
//!   uses.
//! * `_unmatched` — a definition exists, but for another tag. The gate is open and the
//!   shadow-including walk runs, and nothing matches. This isolates the walk from the callbacks.
//! * `_defined` — the tag is defined, so elements are constructed and their callbacks run.

use std::cell::RefCell;

use divan::black_box;
use divan::counter::ItemsCount;
use dom::{CustomElement, Document, NodeId, StylesheetOrigin};
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

const ELEMENTS: usize = 1_024;
const ROWS: usize = 256;
const ATTRIBUTE_BATCH: usize = 4_096;
const NO_OP_BATCH: usize = 4_096;

const PAGE_CSS: &str = "page { display: linear; }
     x-row { display: linear; width: 40px; height: 12px; }";

/// The tag under test. Hyphenated because that is the shape a component
/// library uses, though with no upgrade the name grammar no longer matters.
const TAG: &str = "x-row";

/// A definition that does the least a definition can do, so the measurement is
/// the machinery rather than the handler.
struct Inert;

impl CustomElement<()> for Inert {
    fn observed_attributes(&self) -> Vec<String> {
        vec!["value".to_owned()]
    }
}

#[derive(Clone, Copy)]
enum Registry {
    /// No definitions: every hook takes its empty-registry branch.
    Empty,
    /// A definition for a tag nothing in the document uses.
    OtherTag,
    /// A definition for the tag under test.
    UnderTest,
}

fn page(registry: Registry) -> Document<()> {
    let mut doc = Document::new(device(), "page", ());
    doc.add_stylesheet(PAGE_CSS, StylesheetOrigin::Author);
    match registry {
        Registry::Empty => {}
        Registry::OtherTag => doc.define("x-unused", Box::new(Inert)),
        Registry::UnderTest => doc.define(TAG, Box::new(Inert)),
    }
    doc
}

/// Creation only, never inserted — the hottest path a runtime takes, and the
/// one that pays for the definition lookup and, when it hits, the constructor.
fn create_elements(registry: Registry) -> Document<()> {
    let mut doc = page(registry);
    for _ in 0..ELEMENTS {
        black_box(doc.create_element(TAG, ()));
    }
    doc
}

/// Creation plus connection, which is what actually raises reactions: each
/// append walks the inserted subtree shadow-including.
fn build_page(registry: Registry) -> (Document<()>, NodeId) {
    let mut doc = page(registry);
    let root = doc.document_element().id();
    let mut probe = root;
    for _ in 0..ROWS {
        let row = doc.create_element(TAG, ());
        doc.append_child(root, row);
        probe = row;
    }
    (doc, probe)
}

// --- Creation -------------------------------------------------------------

#[divan::bench]
fn create_elements_plain(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(ELEMENTS))
        .bench_local(|| black_box(create_elements(Registry::Empty)));
}

#[divan::bench]
fn create_elements_unmatched(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(ELEMENTS))
        .bench_local(|| black_box(create_elements(Registry::OtherTag)));
}

#[divan::bench]
fn create_elements_defined(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(ELEMENTS))
        .bench_local(|| black_box(create_elements(Registry::UnderTest)));
}

// --- Insertion ------------------------------------------------------------

#[divan::bench]
fn build_page_plain(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(ROWS))
        .bench_local(|| black_box(build_page(Registry::Empty)));
}

#[divan::bench]
fn build_page_unmatched(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(ROWS))
        .bench_local(|| black_box(build_page(Registry::OtherTag)));
}

#[divan::bench]
fn build_page_defined(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(ROWS))
        .bench_local(|| black_box(build_page(Registry::UnderTest)));
}

// --- Attribute mutation ---------------------------------------------------

fn attribute_mutation(bencher: divan::Bencher, registry: Registry) {
    let (mut doc, probe) = build_page(registry);
    doc.layout();
    let state = RefCell::new(doc);
    bencher
        .counter(ItemsCount::new(ATTRIBUTE_BATCH))
        .bench_local(|| {
            for _ in 0..ATTRIBUTE_BATCH {
                state
                    .borrow_mut()
                    .set_attribute(probe, "value", black_box("v"));
                // A second, unobserved name: the gate must reject it before it
                // reads an old value or allocates.
                state
                    .borrow_mut()
                    .set_attribute(probe, "unwatched", black_box("v"));
            }
        });
}

#[divan::bench]
fn attribute_mutation_plain(bencher: divan::Bencher) {
    attribute_mutation(bencher, Registry::Empty);
}

#[divan::bench]
fn attribute_mutation_defined(bencher: divan::Bencher) {
    attribute_mutation(bencher, Registry::UnderTest);
}

// --- Commit ---------------------------------------------------------------

fn noop_commit(bencher: divan::Bencher, registry: Registry) {
    let (mut doc, _) = build_page(registry);
    doc.layout();
    let state = RefCell::new(doc);
    bencher
        .counter(ItemsCount::new(NO_OP_BATCH))
        .bench_local(|| {
            for _ in 0..NO_OP_BATCH {
                state.borrow_mut().layout();
            }
        });
}

#[divan::bench]
fn noop_commit_plain(bencher: divan::Bencher) {
    noop_commit(bencher, Registry::Empty);
}

#[divan::bench]
fn noop_commit_defined(bencher: divan::Bencher) {
    noop_commit(bencher, Registry::UnderTest);
}

// --- Removal --------------------------------------------------------------

fn remove_page(bencher: divan::Bencher, registry: Registry) {
    bencher
        .counter(ItemsCount::new(ROWS))
        .with_inputs(|| build_page(registry))
        .bench_local_values(|(mut doc, _)| {
            let root = doc.document_element().id();
            let rows: Vec<NodeId> = doc
                .get(root)
                .expect("the root is live")
                .child_ids()
                .to_vec();
            for row in rows {
                doc.remove_subtree(row);
            }
            doc
        });
}

#[divan::bench]
fn remove_page_plain(bencher: divan::Bencher) {
    remove_page(bencher, Registry::Empty);
}

#[divan::bench]
fn remove_page_defined(bencher: divan::Bencher) {
    remove_page(bencher, Registry::UnderTest);
}
