//! CSS commit benchmarks for `dom`, tracked by `CodSpeed` (walltime mode on
//! the macOS CI runner). Commits use the production `Document::layout` path.
//!
//! These documents are given no [`dom::StylePool`], so their traversals run
//! on this thread: what is measured is cascade, matching and invalidation
//! work rather than Rayon dispatch, and a batch built inside `with_inputs`
//! does not pay for a pool per sample. A view in production is given one —
//! its own, never shared — which is what lets two of them restyle at once.

use std::cell::RefCell;
use std::fmt::Write as _;

use divan::black_box;
use divan::counter::ItemsCount;
use dom::{Document, ElementState, NodeId, StylesheetOrigin};
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

fn device(width: f32, height: f32) -> dom::Device {
    dom::standards_device(
        MediaType::screen(),
        Size2D::<f32, CSSPixel>::new(width, height),
        Size2D::<f32, DevicePixel>::new(width, height),
        Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(BenchFontMetricsProvider),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        PrefersColorScheme::Light,
        PointerCapabilities::empty(),
        PointerCapabilities::empty(),
    )
}

const CLASS_RULES: usize = 200;

const PARSE_BATCH: usize = 8;
const INITIAL_FLUSH_BATCH: usize = 2;
const INCREMENTAL_BATCH: usize = 1_024;
const NO_OP_BATCH: usize = 65_536;
const INHERITANCE_BATCH: usize = 32;
const VAR_CHAIN_BATCH: usize = 8;
const MEDIA_BATCH: usize = 2;

fn author_sheet() -> String {
    let mut css = String::with_capacity(64 * 1024);
    for i in 0..CLASS_RULES {
        let _ = write!(
            css,
            ".c{i} {{ color: rgb({}, {}, {}); margin: {}px; padding-left: {}px; }}",
            i % 256,
            (i * 7) % 256,
            (i * 13) % 256,
            i % 32,
            i % 16,
        );
    }
    for i in 0..24 {
        let _ = write!(
            css,
            "section.c{i} > view.c{} {{ background-color: rgb(1, 2, 3); }}\
             view.c{} + view {{ border-top-width: {}px; }}\
             .c{} view:nth-child(2n+1) {{ opacity: 0.9; }}\
             :is(.c{}, .c{}) [data-row] {{ min-width: {}px; }}\
             view:not(.c{}):where(.c{}) {{ max-height: 90px; }}",
            i + 1,
            i + 2,
            i % 8 + 1,
            i,
            i + 3,
            (i * 3) % CLASS_RULES,
            i % 4,
            i + 4,
            i + 5,
        );
    }
    css
}

fn document_with_author_sheet() -> Document<()> {
    let mut doc = Document::new(device(800.0, 600.0), "page", ());
    doc.add_stylesheet(&author_sheet(), StylesheetOrigin::Author);
    doc
}

fn build_tree(doc: &mut Document<()>) -> NodeId {
    let root = doc.document_element().id();
    let mut probe = root;
    let mut class = 0usize;
    for row in 0..32 {
        class += 1;
        let section = doc.create_element("section", ());
        doc.add_class(section, &format!("c{}", class % CLASS_RULES));
        doc.set_attribute(section, "data-row", &row.to_string());
        doc.append_child(root, section);
        for _ in 0..32 {
            class += 1;
            let leaf = doc.create_element("view", ());
            doc.add_class(leaf, &format!("c{}", class % CLASS_RULES));
            doc.append_child(section, leaf);
            probe = leaf;
        }
    }
    probe
}

fn unflushed() -> (Document<()>, NodeId) {
    let mut doc = document_with_author_sheet();
    let probe = build_tree(&mut doc);
    (doc, probe)
}

fn flushed() -> (Document<()>, NodeId) {
    let (mut doc, probe) = unflushed();
    doc.layout();
    (doc, probe)
}

#[divan::bench]
fn parse_author_sheet_text(bencher: divan::Bencher) {
    let css = author_sheet();
    bencher
        .counter(ItemsCount::new(PARSE_BATCH))
        .with_inputs(|| {
            (0..PARSE_BATCH)
                .map(|_| Document::<()>::new(device(800.0, 600.0), "page", ()))
                .collect::<Vec<_>>()
        })
        .bench_local_values(|mut pairs| {
            for doc in &mut pairs {
                doc.add_stylesheet(black_box(&css), StylesheetOrigin::Author);
            }
            pairs
        });
}

#[divan::bench]
fn initial_commit(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(INITIAL_FLUSH_BATCH))
        .with_inputs(|| {
            (0..INITIAL_FLUSH_BATCH)
                .map(|_| unflushed())
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
fn incremental_class_flip(bencher: divan::Bencher) {
    let state = RefCell::new(flushed());
    let mut on = false;
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let (doc, probe) = &mut *state.borrow_mut();
                on = !on;
                if on {
                    doc.add_class(*probe, "c1");
                } else {
                    doc.remove_class(*probe, "c1");
                }
                doc.layout();
            }
        });
}

#[divan::bench]
fn incremental_inline_style(bencher: divan::Bencher) {
    let state = RefCell::new(flushed());
    let mut on = false;
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let (doc, probe) = &mut *state.borrow_mut();
                on = !on;
                let css = if on {
                    "color: rgb(9, 9, 9); width: 10px"
                } else {
                    "color: rgb(3, 3, 3); width: 20px"
                };
                doc.set_inline_style(*probe, black_box(css));
                doc.layout();
            }
        });
}

#[divan::bench]
fn incremental_state_flip(bencher: divan::Bencher) {
    let (mut doc, probe) = unflushed();
    doc.add_stylesheet(
        "view:hover { color: rgb(250, 250, 250); }",
        StylesheetOrigin::Author,
    );
    doc.layout();
    let state = RefCell::new(doc);
    let mut on = false;
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let doc = &mut *state.borrow_mut();
                on = !on;
                if on {
                    doc.add_element_state(probe, ElementState::HOVER);
                } else {
                    doc.remove_element_state(probe, ElementState::HOVER);
                }
                doc.layout();
            }
        });
}

#[divan::bench]
fn incremental_class_flip_repaint_only(bencher: divan::Bencher) {
    let (mut doc, probe) = unflushed();
    doc.add_stylesheet(
        "view.rp-a { color: rgb(17, 17, 17); } view.rp-b { color: rgb(68, 68, 68); }",
        StylesheetOrigin::Author,
    );
    doc.add_class(probe, "rp-a");
    doc.layout();
    let state = RefCell::new(doc);
    let mut on = false;
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let doc = &mut *state.borrow_mut();
                on = !on;
                if on {
                    doc.remove_class(probe, "rp-a");
                    doc.add_class(probe, "rp-b");
                } else {
                    doc.remove_class(probe, "rp-b");
                    doc.add_class(probe, "rp-a");
                }
                doc.layout();
            }
        });
}

#[divan::bench]
fn noop_commit(bencher: divan::Bencher) {
    let state = RefCell::new(flushed());
    bencher
        .counter(ItemsCount::new(NO_OP_BATCH))
        .bench_local(|| {
            for _ in 0..NO_OP_BATCH {
                let (doc, _) = &mut *state.borrow_mut();
                doc.layout();
            }
        });
}

#[divan::bench]
fn inheritance_deep_chain(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(INHERITANCE_BATCH))
        .with_inputs(|| {
            (0..INHERITANCE_BATCH)
                .map(|_| {
                    let mut doc: Document<()> = Document::new(device(800.0, 600.0), "page", ());
                    doc.add_stylesheet(
                        "page { color: rgb(120, 30, 40); font-size: 18px; }",
                        StylesheetOrigin::Author,
                    );
                    let root = doc.document_element().id();
                    let mut parent = root;
                    for _ in 0..256 {
                        let child = doc.create_element("view", ());
                        doc.append_child(parent, child);
                        parent = child;
                    }
                    doc
                })
                .collect::<Vec<_>>()
        })
        .bench_local_values(|mut states| {
            for doc in &mut states {
                doc.layout();
            }
            states
        });
}

#[divan::bench]
fn var_chain_cascade(bencher: divan::Bencher) {
    let mut css = String::from("page { --v0: rgb(1, 2, 3);");
    for i in 1..32 {
        let _ = write!(css, "--v{i}: var(--v{});", i - 1);
    }
    css.push_str("} view { color: var(--v31); }");
    bencher
        .counter(ItemsCount::new(VAR_CHAIN_BATCH))
        .with_inputs(|| {
            (0..VAR_CHAIN_BATCH)
                .map(|_| {
                    let mut doc = Document::new(device(800.0, 600.0), "page", ());
                    doc.add_stylesheet(&css, StylesheetOrigin::Author);
                    let probe = build_tree(&mut doc);
                    (doc, probe)
                })
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
fn media_viewport_flip(bencher: divan::Bencher) {
    let (mut doc, _) = unflushed();
    doc.add_stylesheet(
        "@media (min-width: 700px) { \
             .c1 { color: rgb(200, 100, 50); } view { padding-top: 3px; } \
         }",
        StylesheetOrigin::Author,
    );
    doc.layout();
    let state = RefCell::new(doc);
    let mut wide = true;
    bencher
        .counter(ItemsCount::new(MEDIA_BATCH))
        .bench_local(move || {
            for _ in 0..MEDIA_BATCH {
                let doc = &mut *state.borrow_mut();
                wide = !wide;
                doc.set_viewport(if wide { 800.0 } else { 400.0 }, 600.0);
                doc.layout();
            }
        });
}

const STRUCTURAL_ROWS: usize = 512;
const RESIZE_BATCH: usize = 2;

/// Appending to a list whose parent carries a structural selector flag.
///
/// `:nth-last-child` sets `HAS_SLOW_SELECTOR`, which asks for every child to be
/// restyled on every child-list change — the arm that materialized the whole
/// child list per insertion. The parent only carries the flag after a flush
/// that had children to match, so the seed rows and the flush before the timed
/// loop are load-bearing: without them the structural arm is never entered.
fn structural_list(css: &str) -> (Document<()>, NodeId) {
    let mut doc: Document<()> = Document::new(device(800.0, 600.0), "page", ());
    doc.add_stylesheet(css, StylesheetOrigin::Author);
    let root = doc.document_element().id();
    let list = doc.create_element("view", ());
    doc.add_class(list, "list");
    doc.append_child(root, list);
    for _ in 0..2 {
        let seed = doc.create_element("view", ());
        doc.add_class(seed, "row");
        doc.append_child(list, seed);
    }
    doc.layout();
    (doc, list)
}

fn append_rows(doc: &mut Document<()>, list: NodeId) {
    for _ in 0..STRUCTURAL_ROWS {
        let row = doc.create_element("view", ());
        doc.add_class(row, "row");
        doc.append_child(list, row);
    }
}

#[divan::bench]
fn append_rows_slow_selector(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(STRUCTURAL_ROWS))
        .with_inputs(|| structural_list(".list > .row:nth-last-child(2n+1) { opacity: 0.9 }"))
        .bench_local_values(|(mut doc, list)| {
            append_rows(&mut doc, list);
            (doc, list)
        });
}

#[divan::bench]
fn append_rows_later_siblings(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(STRUCTURAL_ROWS))
        .with_inputs(|| structural_list(".list > .row:nth-child(2n+1) { opacity: 0.9 }"))
        .bench_local_values(|(mut doc, list)| {
            append_rows(&mut doc, list);
            (doc, list)
        });
}

#[divan::bench]
fn append_rows_empty_selector(bencher: divan::Bencher) {
    bencher
        .counter(ItemsCount::new(STRUCTURAL_ROWS))
        .with_inputs(|| structural_list(".list:empty { opacity: 0.9 }"))
        .bench_local_values(|(mut doc, list)| {
            append_rows(&mut doc, list);
            (doc, list)
        });
}

/// A resize that no media query answers differently. Nothing can start or stop
/// matching, so the tree only has to be cascaded again — the case
/// `media_viewport_flip` deliberately does not cover, since it crosses its own
/// breakpoint on every iteration.
#[divan::bench]
fn resize_without_media_change(bencher: divan::Bencher) {
    let (mut doc, _) = unflushed();
    doc.add_stylesheet(
        "@media (min-width: 300px) { .c1 { color: rgb(200, 100, 50) } } \
         view { width: 50vw; padding-top: 1vh }",
        StylesheetOrigin::Author,
    );
    doc.layout();
    let state = RefCell::new(doc);
    let mut wide = true;
    bencher
        .counter(ItemsCount::new(RESIZE_BATCH))
        .bench_local(move || {
            for _ in 0..RESIZE_BATCH {
                let doc = &mut *state.borrow_mut();
                wide = !wide;
                // Both sizes are above the 300px breakpoint, so the media
                // query answers `true` throughout.
                doc.set_viewport(if wide { 800.0 } else { 700.0 }, 600.0);
                doc.layout();
            }
        });
}

/// Writing an attribute no rule mentions, which is the shape every text update
/// takes: `raw-text` carries its whole run in a `text` attribute, and no
/// selector matches on it. The commit that follows has nothing to restyle.
#[divan::bench]
fn attribute_write_without_dependency(bencher: divan::Bencher) {
    let state = RefCell::new(flushed());
    let mut on = false;
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let (doc, probe) = &mut *state.borrow_mut();
                on = !on;
                doc.set_attribute(
                    *probe,
                    black_box("text"),
                    black_box(if on { "a run of text" } else { "another run" }),
                );
                doc.layout();
            }
        });
}

/// The paired case: an attribute the author sheet does select on
/// (`[data-row]`), which must keep its snapshot and its restyle.
#[divan::bench]
fn attribute_write_with_dependency(bencher: divan::Bencher) {
    let state = RefCell::new(flushed());
    let mut on = false;
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(|| {
            for _ in 0..INCREMENTAL_BATCH {
                let (doc, probe) = &mut *state.borrow_mut();
                on = !on;
                doc.set_attribute(
                    *probe,
                    black_box("data-row"),
                    black_box(if on { "1" } else { "2" }),
                );
                doc.layout();
            }
        });
}

/// An element-state flip no rule selects on. `incremental_state_flip` covers
/// the `:hover` case a rule does select on.
#[divan::bench]
fn element_state_flip_without_dependency(bencher: divan::Bencher) {
    let (mut doc, probe) = unflushed();
    doc.add_stylesheet(
        "view:hover { color: rgb(250, 250, 250); }",
        StylesheetOrigin::Author,
    );
    doc.layout();
    let state = RefCell::new(doc);
    let mut on = false;
    bencher
        .counter(ItemsCount::new(INCREMENTAL_BATCH))
        .bench_local(move || {
            for _ in 0..INCREMENTAL_BATCH {
                let doc = &mut *state.borrow_mut();
                on = !on;
                if on {
                    doc.add_element_state(probe, ElementState::FOCUS);
                } else {
                    doc.remove_element_state(probe, ElementState::FOCUS);
                }
                doc.layout();
            }
        });
}
