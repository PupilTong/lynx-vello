//! Integration tests for the flush damage harvest: a style change classifies
//! into the right [`StyleDamage`] class, damage is reported for exactly the
//! affected nodes, and — the regression this refactor fixes — damage is
//! cleared on harvest so a repeat flush neither re-traverses nor re-reports.

mod common;

use std::collections::BTreeSet;

use common::{Doc, rgb};
use w3c_dom::{FlushStatus, FlushSummary, NodeId, Parallelism, StyleDamage};

fn damage_of(summary: &FlushSummary, id: NodeId) -> Option<StyleDamage> {
    summary
        .damage
        .iter()
        .find(|entry| entry.node_id == id)
        .map(|entry| entry.damage)
}

#[test]
fn class_flip_on_color_rule_reports_repaint_only() {
    let mut doc = Doc::with_css(".hot { color: rgb(255, 0, 0) }");
    let el = doc.el(doc.root, "view");
    let initial = doc.flush();
    assert!(!initial.has_damage(), "initial styling produces no damage");

    doc.add_class(el, "hot");
    let summary = doc.flush();

    let damage = damage_of(&summary, el).expect("the flipped node carries damage");
    assert!(damage.needs_repaint());
    assert!(!damage.needs_relayout());
    assert!(!damage.needs_overflow_recalculation());
    assert!(!damage.needs_stacking_context_rebuild());
    assert!(!damage.requires_reconstruction());
    assert_eq!(summary.damage.len(), 1, "only the flipped node is damaged");
}

#[test]
fn class_flip_on_width_rule_reports_relayout() {
    let mut doc = Doc::with_css(".wide { width: 100px }");
    let el = doc.el(doc.root, "view");
    doc.flush();

    doc.add_class(el, "wide");
    let summary = doc.flush();

    let damage = damage_of(&summary, el).expect("the flipped node carries damage");
    assert!(damage.needs_relayout());
    assert!(damage.needs_repaint());
    assert!(damage.needs_overflow_recalculation());
}

#[test]
fn inline_width_change_reports_relayout() {
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, "width: 10px");
    let initial = doc.flush();
    assert!(!initial.has_damage(), "initial styling produces no damage");

    doc.set_inline(el, "width: 20px");
    let summary = doc.flush();

    let damage = damage_of(&summary, el).expect("the inline width change carries damage");
    assert!(damage.needs_relayout());
}

#[test]
fn empty_flip_structural_path_reports_relayout() {
    let mut doc = Doc::with_css(".box:empty { height: 50px }");
    let box_id = doc.el(doc.root, "view.box");
    doc.flush();
    assert_eq!(doc.value(box_id, "height"), "50px");

    let child = doc.el(box_id, "view");
    let summary = doc.flush();

    let damage = damage_of(&summary, box_id).expect(":empty flip damages the container");
    assert!(damage.needs_relayout());
    assert_eq!(doc.value(box_id, "height"), "auto");
    assert!(damage_of(&summary, child).is_none());
}

#[test]
fn edge_child_structural_path_reports_relayout() {
    let mut doc = Doc::with_css(".list > view:first-child { width: 30px }");
    let list = doc.el(doc.root, "view.list");
    let first = doc.el(list, "view");
    doc.flush();
    assert_eq!(doc.value(first, "width"), "30px");

    let new_first = doc.dom.create_element("view", ());
    doc.dom.insert_before(list, new_first, Some(first));
    let summary = doc.flush();

    let damage = damage_of(&summary, first).expect("displaced first-child re-styles");
    assert!(damage.needs_relayout());
    assert_eq!(doc.value(first, "width"), "auto");
    assert_eq!(doc.value(new_first, "width"), "30px");
}

#[test]
fn damage_is_cleared_after_harvest() {
    let mut doc = Doc::with_css(".wide { width: 100px }");
    let el = doc.el(doc.root, "view");
    doc.flush();

    doc.add_class(el, "wide");
    let first = doc.flush();
    assert!(first.has_damage(), "the incremental flush reports damage");
    assert_eq!(first.status, FlushStatus::Traversed);

    let second = doc.flush();
    assert!(!second.has_damage(), "damage must not survive the harvest");
    assert_eq!(second.status, FlushStatus::Skipped);
}

#[test]
fn display_none_flip_reports_relayout_and_leaves_no_stale_state() {
    let mut doc = Doc::with_css(".gone { display: none }");
    let parent = doc.el(doc.root, "view");
    let child = doc.el(parent, "view");
    doc.flush();
    assert!(doc.dom.get(child).unwrap().computed_style().is_some());

    doc.add_class(parent, "gone");
    let summary = doc.flush();
    let damage = damage_of(&summary, parent).expect("the display flip damages the node");
    assert!(damage.needs_relayout());

    let second = doc.flush();
    assert!(
        !second.has_damage(),
        "no stale damage from the pruned subtree"
    );
    assert_eq!(second.status, FlushStatus::Skipped);
}

#[test]
fn sibling_invalidation_damage_is_harvested_and_cleared() {
    let mut doc = Doc::with_css(".a + .b { color: rgb(255, 0, 0) }");
    let a = doc.el(doc.root, "view");
    let b = doc.el(doc.root, "view.b");
    let initial = doc.flush();
    assert!(!initial.has_damage(), "initial styling produces no damage");
    assert_ne!(doc.color(b), rgb(255, 0, 0));

    doc.add_class(a, "a");
    let summary = doc.flush();

    let damage = damage_of(&summary, b).expect("the invalidated sibling carries damage");
    assert!(damage.needs_repaint());
    assert!(!damage.needs_relayout());
    assert_eq!(doc.color(b), rgb(255, 0, 0));

    let second = doc.flush();
    assert!(!second.has_damage(), "no leaked damage after the flush");
    assert_eq!(second.status, FlushStatus::Skipped);
}

fn incremental_damage_set(parallelism: Parallelism) -> BTreeSet<(NodeId, u16)> {
    let mut doc = Doc::with_css(".theme { color: rgb(9, 9, 9) }");
    for _ in 0..40 {
        doc.el(doc.root, "view");
    }
    doc.flush();

    doc.add_class(doc.root, "theme");
    let summary = doc.dom.flush_styles_with_parallelism(parallelism);
    summary
        .damage
        .into_iter()
        .map(|entry| (entry.node_id, entry.damage.bits()))
        .collect()
}

#[test]
fn parallel_and_sequential_flush_agree_on_damage_sets() {
    let sequential = incremental_damage_set(Parallelism::Sequential);
    let auto = incremental_damage_set(Parallelism::Auto);
    assert_eq!(sequential, auto);
    assert!(
        !sequential.is_empty(),
        "an inherited-property flip damages the root and every descendant"
    );
}
