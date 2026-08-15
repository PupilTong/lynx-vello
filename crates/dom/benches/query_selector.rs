//! Selector-query benchmarks: `querySelector`, `querySelectorAll`, `matches`,
//! and `closest`.
//!
//! All four are `&self` reads, so every case shares one committed document and
//! times nothing but the query.
//!
//! Four of the groups are **paired A/B**, because each one prices something
//! `src/style/query.rs` records as deliberately absent, and a lone number would
//! not say whether the absence is what made it slow:
//!
//! - `query_selector_id_{first,last}` — **no id map.** Both resolve a unique `#id` through
//!   `QueryFirst`. The first is the first row in tree order and the second is the last, so one
//!   exits on the first element it visits and the other walks every row. With
//!   [`TDocument::elements_with_id`](stylo::dom::TDocument::elements_with_id) implemented both
//!   would be a hash lookup, so the gap between the two — and how it grows with the row argument —
//!   is exactly what that index would buy.
//! - `query_selector_all_class_{present,absent}` — **no subtree bloom filter.** Both are a bare
//!   `.class`, the shape `query_selector_single_query` filters through
//!   [`bloom_may_have_hash`](stylo::dom::TElement::bloom_may_have_hash). One class is on a quarter
//!   of the rows, the other is on nothing, and both pay the same full walk: the default
//!   [`subtree_bloom_filter`](stylo::dom::TElement::subtree_bloom_filter) is `u64::MAX`, so the
//!   check is always true and `RejectSkippingChildren` never fires. A populated filter would
//!   collapse the absent side to a single rejected root, so that number is what it would save,
//!   against a present-class control where the walk is required either way.
//! - `query_selector_all_descendant_{hit,miss}` — the **ancestor walk**, which is a different thing
//!   from the filter above. `.hot row` and `.absent row` share the rightmost `row`, and the filter
//!   is built from the *rightmost* compound, so no bloom would separate these two; what differs is
//!   only how far the ancestor walk climbs before it succeeds or gives up.
//! - `query_selector_{first,all}_class` — `QueryFirst` against `QueryAll` on one selector and one
//!   tree: the cost of an early exit against a full walk plus result collection.
//!
//! `matches_{simple,complex}` is the per-call floor — one element, no traversal
//! — so it is dominated by the parse all four entry points repeat on every
//! call; nothing here caches a parsed `SelectorList` the way Gecko's
//! `nsINode::QuerySelector` does. The `query_selector_all_*` selector shapes
//! are that same parse plus a full walk, so the difference is what the matcher
//! costs per element.

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

/// Rows per section, so a row count implies how wide the tree is.
const FAN_OUT: usize = 32;
/// The tree the selector-shape and floor groups share.
const ROWS: usize = 1_024;
/// Ancestors between the chain's deepest node and the element `closest` finds.
const DEPTH: usize = 64;

/// Queries per timed iteration for the whole-tree cases, each of which is
/// already linear in the tree.
const QUERY_BATCH: usize = 4;
/// Queries per timed iteration for the single-element cases.
const ELEMENT_BATCH: usize = 512;

const PAGE_CSS: &str = "page { display: linear; }
     section { display: linear; }
     row { display: linear; width: 40px; height: 12px; }
     chain { display: linear; }";

/// A committed page plus the probes the query cases start from.
struct Page {
    doc: Document<()>,
    /// The first row in tree order, carrying `id=first-row`.
    first_row: NodeId,
    /// The last row of the last section, carrying `id=last-row`.
    last_row: NodeId,
    /// A row whose own section matches `.hot`: two `closest` checks.
    shallow_row: NodeId,
    /// The bottom of the nesting chain: `DEPTH` `closest` checks.
    deep_leaf: NodeId,
}

/// `rows` rows spread `FAN_OUT` to a section, then one `DEPTH`-deep chain.
///
/// Sections alternate `.hot` and `.cold` so an ancestor compound matches about
/// half of them; rows carry a `data-row` attribute and a class of their own.
/// Nothing anywhere carries `.absent`.
fn page(rows: usize) -> Page {
    let mut doc = Document::new(device(), "page", ());
    doc.add_stylesheet(PAGE_CSS, StylesheetOrigin::Author);
    let root = doc.document_element().id();

    let mut first_row = None;
    let mut last_row = root;
    let mut shallow_row = root;
    let mut section = root;
    for index in 0..rows {
        if index.is_multiple_of(FAN_OUT) {
            let section_index = index / FAN_OUT;
            section = doc.create_element("section", ());
            doc.add_class(
                section,
                if section_index.is_multiple_of(2) {
                    "hot"
                } else {
                    "cold"
                },
            );
            doc.set_attribute(section, "data-section", &section_index.to_string());
            doc.append_child(root, section);
        }
        let row = doc.create_element("row", ());
        doc.add_class(
            row,
            if index.is_multiple_of(4) {
                "warm"
            } else {
                "cool"
            },
        );
        doc.set_attribute(row, "data-row", &index.to_string());
        doc.append_child(section, row);

        if first_row.is_none() {
            doc.set_id_attribute(row, Some("first-row"));
            first_row = Some(row);
        }
        // The first row of the second `.hot` section: its own section matches
        // `.hot`, so `closest` stops one step up.
        if index == FAN_OUT * 2 {
            shallow_row = row;
        }
        last_row = row;
    }
    doc.set_id_attribute(last_row, Some("last-row"));

    let chain_top = doc.create_element("chain", ());
    doc.add_class(chain_top, "anchor");
    doc.append_child(root, chain_top);
    let mut deep_leaf = chain_top;
    for _ in 0..DEPTH {
        let link = doc.create_element("chain", ());
        doc.append_child(deep_leaf, link);
        deep_leaf = link;
    }

    doc.layout();
    Page {
        doc,
        first_row: first_row.unwrap_or(root),
        last_row,
        shallow_row,
        deep_leaf,
    }
}

/// The document node — `document.querySelector(...)`'s receiver, and the
/// widest root a query can have.
fn document(page: &Page) -> NodeId {
    page.doc.root_node().id()
}

fn query_first(page: &Page, selectors: &str) -> Option<NodeId> {
    page.doc
        .query_selector(document(page), selectors)
        .expect("selector parses")
}

fn query_all(page: &Page, selectors: &str) -> Vec<NodeId> {
    page.doc
        .query_selector_all(document(page), selectors)
        .expect("selector parses")
}

/// `QUERY_BATCH` whole-tree queries per iteration.
fn bench_query(bencher: divan::Bencher, page: &Page, selectors: &str, all: bool) {
    bencher
        .counter(ItemsCount::new(QUERY_BATCH))
        .bench_local(|| {
            for _ in 0..QUERY_BATCH {
                if all {
                    black_box(query_all(page, black_box(selectors)));
                } else {
                    black_box(query_first(page, black_box(selectors)));
                }
            }
        });
}

// --- no id map: one unique id, reached first versus last ---------------------

#[divan::bench(args = [256, 4_096])]
fn query_selector_id_first(bencher: divan::Bencher, rows: usize) {
    let page = page(rows);
    assert_eq!(query_first(&page, "#first-row"), Some(page.first_row));
    bench_query(bencher, &page, "#first-row", false);
}

#[divan::bench(args = [256, 4_096])]
fn query_selector_id_last(bencher: divan::Bencher, rows: usize) {
    let page = page(rows);
    assert_eq!(query_first(&page, "#last-row"), Some(page.last_row));
    bench_query(bencher, &page, "#last-row", false);
}

// --- no subtree bloom filter: a class that exists against one that cannot ----

#[divan::bench(args = [256, 4_096])]
fn query_selector_all_class_present(bencher: divan::Bencher, rows: usize) {
    let page = page(rows);
    assert!(!query_all(&page, ".warm").is_empty());
    bench_query(bencher, &page, ".warm", true);
}

#[divan::bench(args = [256, 4_096])]
fn query_selector_all_class_absent(bencher: divan::Bencher, rows: usize) {
    let page = page(rows);
    // The whole walk is dead work: a populated subtree bloom filter rejects
    // this at the root instead.
    assert!(query_all(&page, ".absent").is_empty());
    bench_query(bencher, &page, ".absent", true);
}

// --- the ancestor walk: a reachable versus an unreachable ancestor compound --

#[divan::bench(args = [256, 4_096])]
fn query_selector_all_descendant_hit(bencher: divan::Bencher, rows: usize) {
    let page = page(rows);
    assert!(!query_all(&page, ".hot row").is_empty());
    bench_query(bencher, &page, ".hot row", true);
}

#[divan::bench(args = [256, 4_096])]
fn query_selector_all_descendant_miss(bencher: divan::Bencher, rows: usize) {
    let page = page(rows);
    // Every row matches the rightmost compound, then climbs to the root before
    // giving up — the same rightmost `row` as the hit case, so the difference
    // is the ancestor walk alone.
    assert!(query_all(&page, ".absent row").is_empty());
    bench_query(bencher, &page, ".absent row", true);
}

// --- early exit versus full walk on one selector -----------------------------

#[divan::bench]
fn query_selector_first_class(bencher: divan::Bencher) {
    let page = page(ROWS);
    bench_query(bencher, &page, ".cool", false);
}

#[divan::bench]
fn query_selector_all_class(bencher: divan::Bencher) {
    let page = page(ROWS);
    bench_query(bencher, &page, ".cool", true);
}

// --- what the matcher costs per element, by selector shape -------------------

#[divan::bench]
fn query_selector_all_type(bencher: divan::Bencher) {
    let page = page(ROWS);
    bench_query(bencher, &page, "row", true);
}

#[divan::bench]
fn query_selector_all_descendant(bencher: divan::Bencher) {
    let page = page(ROWS);
    bench_query(bencher, &page, "section row", true);
}

#[divan::bench]
fn query_selector_all_child(bencher: divan::Bencher) {
    let page = page(ROWS);
    bench_query(bencher, &page, "section > row", true);
}

#[divan::bench]
fn query_selector_all_attribute(bencher: divan::Bencher) {
    let page = page(ROWS);
    bench_query(bencher, &page, "row[data-row=\"7\"]", true);
}

#[divan::bench]
fn query_selector_all_nth_child(bencher: divan::Bencher) {
    let page = page(ROWS);
    bench_query(bencher, &page, "row:nth-child(2n+1)", true);
}

#[divan::bench]
fn query_selector_all_selector_list(bencher: divan::Bencher) {
    let page = page(ROWS);
    // A list refuses `dom_apis`' single-selector fast paths, so this is the
    // shape that always walks and always matches the whole list per element.
    bench_query(
        bencher,
        &page,
        ":is(.hot, .cold) row, section > .warm",
        true,
    );
}

// --- a query rooted at an element, not the document --------------------------

#[divan::bench]
fn query_selector_all_scoped_to_section(bencher: divan::Bencher) {
    let page = page(ROWS);
    let section = page
        .doc
        .get(page.shallow_row)
        .and_then(dom::Node::parent_id)
        .expect("a row has a section");
    assert_eq!(
        page.doc
            .query_selector_all(section, "row")
            .expect("selector parses")
            .len(),
        FAN_OUT,
        "the scoped query sees one section's rows, not the document's",
    );
    bencher
        .counter(ItemsCount::new(QUERY_BATCH))
        .bench_local(|| {
            for _ in 0..QUERY_BATCH {
                black_box(
                    page.doc
                        .query_selector_all(section, black_box("row"))
                        .expect("selector parses"),
                );
            }
        });
}

// --- the ancestor walk, by depth ---------------------------------------------

#[divan::bench]
fn closest_shallow(bencher: divan::Bencher) {
    let page = page(ROWS);
    assert_eq!(
        page.doc
            .closest(page.shallow_row, ".hot")
            .expect("selector parses"),
        page.doc
            .get(page.shallow_row)
            .and_then(dom::Node::parent_id),
        "the shallow probe stops at its own section, one step up",
    );
    bencher
        .counter(ItemsCount::new(ELEMENT_BATCH))
        .bench_local(|| {
            for _ in 0..ELEMENT_BATCH {
                black_box(
                    page.doc
                        .closest(page.shallow_row, black_box(".hot"))
                        .expect("selector parses"),
                );
            }
        });
}

#[divan::bench]
fn closest_deep(bencher: divan::Bencher) {
    let page = page(ROWS);
    bencher
        .counter(ItemsCount::new(ELEMENT_BATCH))
        .bench_local(|| {
            for _ in 0..ELEMENT_BATCH {
                black_box(
                    page.doc
                        .closest(page.deep_leaf, black_box(".anchor"))
                        .expect("selector parses"),
                );
            }
        });
}

// --- the per-call floor: one element, parse included -------------------------

#[divan::bench]
fn matches_simple(bencher: divan::Bencher) {
    let page = page(ROWS);
    bencher
        .counter(ItemsCount::new(ELEMENT_BATCH))
        .bench_local(|| {
            for _ in 0..ELEMENT_BATCH {
                black_box(
                    page.doc
                        .matches(page.first_row, black_box("row"))
                        .expect("selector parses"),
                );
            }
        });
}

#[divan::bench]
fn matches_complex(bencher: divan::Bencher) {
    let page = page(ROWS);
    bencher
        .counter(ItemsCount::new(ELEMENT_BATCH))
        .bench_local(|| {
            for _ in 0..ELEMENT_BATCH {
                black_box(
                    page.doc
                        .matches(
                            page.first_row,
                            black_box(
                                ":is(.hot, .cold) > row[data-row]:nth-child(2n+1):not(.absent)",
                            ),
                        )
                        .expect("selector parses"),
                );
            }
        });
}

// --- a selector that does not parse ------------------------------------------

#[divan::bench]
fn invalid_selector(bencher: divan::Bencher) {
    let page = page(ROWS);
    bencher
        .counter(ItemsCount::new(ELEMENT_BATCH))
        .bench_local(|| {
            for _ in 0..ELEMENT_BATCH {
                // The error owns a copy of the selector text, so a caller that
                // probes with bad selectors allocates on every call.
                black_box(
                    page.doc
                        .matches(page.first_row, black_box("!!"))
                        .unwrap_err(),
                );
            }
        });
}
