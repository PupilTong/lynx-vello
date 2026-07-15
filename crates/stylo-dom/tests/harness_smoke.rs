//! Smoke checks for the shared test harness (`tests/common/mod.rs`).

mod common;

use common::{Doc, media_matches, parses, rgb, specificity, specified};
use stylo_dom::ElementState;

#[test]
fn doc_builds_flushes_and_reads_computed_values() {
    let mut doc = Doc::with_css(".a { color: rgb(10, 20, 30); margin-left: 7px; }");
    let el = doc.el(doc.root, "view.a#main[data-x=1]");
    doc.flush();
    assert_eq!(doc.color(el), rgb(10, 20, 30));
    assert_eq!(doc.value(el, "margin-left"), "7px");
}

#[test]
fn spec_dsl_and_raw_matching() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "text#t.a.b[title='hi there'][flag]");
    assert!(doc.matches(el, "text#t.a.b[title=\"hi there\"][flag]"));
    assert!(doc.matches(el, "page > text"));
    assert!(!doc.matches(el, "view"));
}

#[test]
fn mutation_helpers_restyle_incrementally() {
    let mut doc = Doc::with_css(".hot { color: rgb(255, 0, 0) }");
    let el = doc.el(doc.root, "view");
    doc.flush();
    assert_ne!(doc.color(el), rgb(255, 0, 0));
    doc.add_class(el, "hot");
    doc.flush();
    assert_eq!(doc.color(el), rgb(255, 0, 0));
}

#[cfg(debug_assertions)]
#[test]
fn debug_owner_checks_accept_wide_parallel_traversal() {
    let mut doc = Doc::with_css(".even { color: rgb(10, 20, 30) }");
    let mut probe = None;
    for index in 0..512 {
        let element = doc.el(
            doc.root,
            if index % 2 == 0 {
                "view.even"
            } else {
                "view.odd"
            },
        );
        if index == 510 {
            probe = Some(element);
        }
    }

    doc.flush();

    assert_eq!(
        doc.color(probe.expect("probe must be created")),
        rgb(10, 20, 30)
    );
}

#[test]
fn state_bits_participate_in_matching() {
    let mut doc = Doc::with_css("view:hover { color: rgb(0, 255, 0) }");
    let el = doc.el(doc.root, "view");
    doc.flush();
    doc.set_state(el, ElementState::HOVER, true);
    doc.flush();
    assert_eq!(doc.color(el), rgb(0, 255, 0));
}

#[test]
fn specified_value_roundtrip_accepts_and_rejects() {
    assert_eq!(specified("margin-left", "10px").as_deref(), Some("10px"));
    assert!(specified("margin-left", "10furlongs").is_none());
    assert!(parses("border-radius", "1px 2px 3px 4px"));
    assert!(!parses("border-radius", "banana"));
}

#[test]
fn specificity_triples() {
    assert_eq!(specificity("#a .b view"), Some((1, 1, 1)));
    assert_eq!(specificity(":where(#a)"), Some((0, 0, 0)));
    assert_eq!(specificity(":is(#a, .b)"), Some((1, 0, 0)));
}

#[test]
fn media_evaluation_end_to_end() {
    assert!(media_matches("(min-width: 600px)"));
    assert!(!media_matches("(min-width: 900px)"));
    assert!(media_matches(""));
}
