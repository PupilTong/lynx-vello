//! CSS-engine benchmarks for `w3c-dom`, tracked by `CodSpeed` (walltime
//! mode on the macOS CI runner).
//!
//! Pure engine-level cases — stylesheet text parsing, initial cascade
//! (sequential and parallel), invalidation-driven incremental restyles,
//! inheritance and `var()` chains, selector-stress matching, and media
//! re-evaluation — mirroring the CSS behavior surface ported from the
//! `LynxJS`
//! C++ engine tests. **No comparison harness** (native C++ Lynx or
//! otherwise) at this stage: these establish absolute numbers and guard
//! against regressions.

use std::cell::RefCell;
use std::fmt::Write as _;

use divan::black_box;
use euclid::{Scale, Size2D};
use stylo::context::QuirksMode;
use stylo::device::Device;
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
use w3c_dom::{Document, ElementState, NodeId, Parallelism, StyleEngine, StylesheetOrigin};

fn main() {
    divan::main();
}

// --- fixtures ---------------------------------------------------------------

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

fn device(width: f32, height: f32) -> Device {
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
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

/// `CLASS_RULES` simple class rules plus a band of combinator/pseudo-heavy
/// rules — the same selector families the ported behavior tests exercise.
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

fn engine_with_author_sheet() -> StyleEngine {
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_str(&author_sheet(), StylesheetOrigin::Author);
    engine
}

/// `page > 32 × section > 32 × view`, classes cycling through the rule set,
/// every section carrying a `data-row` attribute. ~1.1k nodes.
fn build_tree(engine: &StyleEngine) -> (Document<()>, NodeId) {
    let mut doc: Document<()> = engine.new_document();
    let root = doc.create_node("page", ());
    doc.append_child(root);
    let mut probe = root;
    let mut class = 0usize;
    for row in 0..32 {
        class += 1;
        let section = doc.create_node("section", ());
        doc.add_class(section, &format!("c{}", class % CLASS_RULES));
        doc.set_attribute(section, "data-row", &row.to_string());
        doc.append(root, section);
        for _ in 0..32 {
            class += 1;
            let leaf = doc.create_node("view", ());
            doc.add_class(leaf, &format!("c{}", class % CLASS_RULES));
            doc.append(section, leaf);
            probe = leaf;
        }
    }
    (doc, probe)
}

/// A flushed document plus a probe leaf, ready for incremental cases.
fn flushed() -> (StyleEngine, Document<()>, NodeId) {
    let engine = engine_with_author_sheet();
    let (mut doc, probe) = build_tree(&engine);
    engine.flush_document(&mut doc);
    (engine, doc, probe)
}

// --- stylesheet parsing ------------------------------------------------------

/// Parse + register the generated author sheet from CSS text (one stylist
/// flush included), on a fresh engine per iteration.
#[divan::bench]
fn parse_author_sheet_text(bencher: divan::Bencher) {
    let css = author_sheet();
    bencher
        .with_inputs(|| StyleEngine::new(device(800.0, 600.0)))
        .bench_local_values(|mut engine| {
            engine.add_stylesheet_str(black_box(&css), StylesheetOrigin::Author);
            engine
        });
}

// --- initial cascade ---------------------------------------------------------

#[divan::bench]
fn initial_flush_sequential(bencher: divan::Bencher) {
    let engine = engine_with_author_sheet();
    bencher
        .with_inputs(|| build_tree(&engine))
        .bench_local_values(|(mut doc, _)| {
            engine.flush_document_with(&mut doc, Parallelism::Sequential);
            doc
        });
}

#[divan::bench]
fn initial_flush_parallel(bencher: divan::Bencher) {
    let engine = engine_with_author_sheet();
    bencher
        .with_inputs(|| build_tree(&engine))
        .bench_local_values(|(mut doc, _)| {
            engine.flush_document_with(&mut doc, Parallelism::Auto);
            doc
        });
}

// --- incremental restyles (invalidation sets) --------------------------------

/// Class flip on one deep leaf: snapshot, invalidate, restyle the affected
/// nodes only.
#[divan::bench]
fn incremental_class_flip(bencher: divan::Bencher) {
    let state = RefCell::new(flushed());
    let mut on = false;
    bencher.bench_local(|| {
        let (engine, doc, probe) = &mut *state.borrow_mut();
        on = !on;
        if on {
            doc.add_class(*probe, "c1");
        } else {
            doc.remove_class(*probe, "c1");
        }
        engine.flush_document(doc);
    });
}

/// Inline `style` update on one deep leaf.
#[divan::bench]
fn incremental_inline_style(bencher: divan::Bencher) {
    let state = RefCell::new(flushed());
    let mut on = false;
    bencher.bench_local(|| {
        let (engine, doc, probe) = &mut *state.borrow_mut();
        on = !on;
        let css = if on {
            "color: rgb(9, 9, 9); width: 10px"
        } else {
            "color: rgb(3, 3, 3); width: 20px"
        };
        doc.set_inline_style(*probe, black_box(css));
        engine.flush_document(doc);
    });
}

/// `:hover` state flip on one deep leaf (state-keyed invalidation).
#[divan::bench]
fn incremental_state_flip(bencher: divan::Bencher) {
    let mut engine = engine_with_author_sheet();
    engine.add_stylesheet_str(
        "view:hover { color: rgb(250, 250, 250); }",
        StylesheetOrigin::Author,
    );
    let (mut doc, probe) = build_tree(&engine);
    engine.flush_document(&mut doc);
    let state = RefCell::new((engine, doc));
    let mut on = false;
    bencher.bench_local(|| {
        let (engine, doc) = &mut *state.borrow_mut();
        on = !on;
        doc.set_state(probe, ElementState::HOVER, on);
        engine.flush_document(doc);
    });
}

/// A flush with nothing scheduled — the per-frame floor.
#[divan::bench]
fn noop_flush(bencher: divan::Bencher) {
    let state = RefCell::new(flushed());
    bencher.bench_local(|| {
        let (engine, doc, _) = &mut *state.borrow_mut();
        engine.flush_document(doc);
    });
}

// --- inheritance & custom properties -----------------------------------------

/// Initial cascade down a 256-deep inheritance chain (`color` set at the
/// root, inherited by every descendant).
#[divan::bench]
fn inheritance_deep_chain(bencher: divan::Bencher) {
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_str(
        "page { color: rgb(120, 30, 40); font-size: 18px; }",
        StylesheetOrigin::Author,
    );
    bencher
        .with_inputs(|| {
            let mut doc: Document<()> = engine.new_document();
            let root = doc.create_node("page", ());
            doc.append_child(root);
            let mut parent = root;
            for _ in 0..256 {
                let child = doc.create_node("view", ());
                doc.append(parent, child);
                parent = child;
            }
            doc
        })
        .bench_local_values(|mut doc| {
            engine.flush_document(&mut doc);
            doc
        });
}

/// Initial cascade with a 32-link `var()` chain feeding `color` on ~1.1k
/// nodes (registration + substitution cost).
#[divan::bench]
fn var_chain_cascade(bencher: divan::Bencher) {
    let mut css = String::from("page { --v0: rgb(1, 2, 3);");
    for i in 1..32 {
        let _ = write!(css, "--v{i}: var(--v{});", i - 1);
    }
    css.push_str("} view { color: var(--v31); }");
    let mut engine = StyleEngine::new(device(800.0, 600.0));
    engine.add_stylesheet_str(&css, StylesheetOrigin::Author);
    bencher
        .with_inputs(|| build_tree(&engine))
        .bench_local_values(|(mut doc, _)| {
            engine.flush_document(&mut doc);
            doc
        });
}

// --- media re-evaluation ------------------------------------------------------

/// Viewport flip across a `@media` boundary: stylist re-flush plus the
/// device-change restyle.
#[divan::bench]
fn media_viewport_flip(bencher: divan::Bencher) {
    let mut engine = engine_with_author_sheet();
    engine.add_stylesheet_with_media(
        ".c1 { color: rgb(200, 100, 50); } view { padding-top: 3px; }",
        StylesheetOrigin::Author,
        "(min-width: 700px)",
    );
    let (mut doc, _) = build_tree(&engine);
    engine.flush_document(&mut doc);
    let root = doc
        .document_element()
        .expect("document has an element child");
    let state = RefCell::new((engine, doc));
    let mut wide = true;
    bencher.bench_local(move || {
        let (engine, doc) = &mut *state.borrow_mut();
        wide = !wide;
        engine.set_viewport(if wide { 800.0 } else { 400.0 }, 600.0);
        doc.mark_subtree_dirty(black_box(root));
        engine.flush_document(doc);
    });
}

// --- standalone resolve baseline ----------------------------------------------

/// Match + cascade one node outside the traversal (the `resolve` path the
/// media/value helpers use).
#[divan::bench]
fn resolve_single_element(bencher: divan::Bencher) {
    let engine = engine_with_author_sheet();
    let (doc, probe) = build_tree(&engine);
    bencher.bench_local(|| {
        let node = doc.get(black_box(probe)).expect("probe is live");
        black_box(engine.resolve(node, None));
    });
}
