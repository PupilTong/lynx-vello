//! Integration tests for the flush damage harvest: a style change classifies
//! into the right [`StyleDamage`] class, damage is reported for exactly the
//! affected nodes, and — the regression this refactor fixes — damage is
//! cleared on harvest so a repeat flush neither re-traverses nor re-reports.

mod common;

use std::collections::BTreeSet;

use common::{Doc, rgb};
use w3c_dom::{FlushSummary, NodeId, Parallelism, StyleDamage};

/// The damage reported for `id` in `summary`, if any.
fn damage_of(summary: &FlushSummary, id: NodeId) -> Option<StyleDamage> {
    summary
        .damage
        .iter()
        .find(|(node, _)| *node == id)
        .map(|(_, damage)| *damage)
}

#[test]
fn class_flip_on_color_rule_reports_repaint_only() {
    // `color` maps to `repaint`, so a rule that only changes color must produce
    // REPAINT damage — never RELAYOUT/overflow/stacking — on exactly the flipped
    // node.
    let mut doc = Doc::with_css(".hot { color: rgb(255, 0, 0) }");
    let el = doc.el(doc.root, "view");
    let initial = doc.flush();
    assert!(initial.is_empty(), "initial styling produces no damage");

    doc.add_class(el, "hot");
    let summary = doc.flush();

    let damage = damage_of(&summary, el).expect("the flipped node carries damage");
    assert!(damage.needs_repaint());
    assert!(!damage.needs_relayout());
    assert!(!damage.needs_overflow_recalculation());
    assert!(!damage.needs_stacking_context_rebuild());
    assert!(!damage.is_reconstruct());
    assert_eq!(summary.damage.len(), 1, "only the flipped node is damaged");
}

#[test]
fn class_flip_on_width_rule_reports_relayout() {
    // `width` maps to `rebuild_box` => RELAYOUT.
    let mut doc = Doc::with_css(".wide { width: 100px }");
    let el = doc.el(doc.root, "view");
    doc.flush();

    doc.add_class(el, "wide");
    let summary = doc.flush();

    let damage = damage_of(&summary, el).expect("the flipped node carries damage");
    assert!(damage.needs_relayout());
    // RELAYOUT is the top class, so every lower predicate is also true.
    assert!(damage.needs_repaint());
    assert!(damage.needs_overflow_recalculation());
}

#[test]
fn inline_width_change_reports_relayout() {
    // The `RESTYLE_STYLE_ATTRIBUTE` recascade path still diffs the box and
    // produces RELAYOUT on a width change.
    let mut doc = Doc::new();
    let el = doc.el(doc.root, "view");
    doc.set_inline(el, "width: 10px");
    let initial = doc.flush();
    assert!(initial.is_empty(), "initial styling produces no damage");

    doc.set_inline(el, "width: 20px");
    let summary = doc.flush();

    let damage = damage_of(&summary, el).expect("the inline width change carries damage");
    assert!(damage.needs_relayout());
}

#[test]
fn empty_flip_structural_path_reports_relayout() {
    // Inserting a child flips `.box:empty` off; the container re-styles and the
    // `height` reverts, producing RELAYOUT on the container.
    let mut doc = Doc::with_css(".box:empty { height: 50px }");
    let box_id = doc.el(doc.root, "view.box");
    doc.flush();
    assert_eq!(doc.value(box_id, "height"), "50px");

    let child = doc.el(box_id, "view");
    let summary = doc.flush();

    let damage = damage_of(&summary, box_id).expect(":empty flip damages the container");
    assert!(damage.needs_relayout());
    assert_eq!(doc.value(box_id, "height"), "auto");
    // The freshly inserted child is initial-styled => no damage.
    assert!(damage_of(&summary, child).is_none());
}

#[test]
fn edge_child_structural_path_reports_relayout() {
    // Prepending a new first child displaces the old `:first-child`, which
    // re-styles and loses its width => RELAYOUT on the displaced child.
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
    // Regression test for the latent never-cleared-damage bug: after a real
    // incremental flush, an immediate second flush must neither re-traverse nor
    // re-report the previous damage.
    let mut doc = Doc::with_css(".wide { width: 100px }");
    let el = doc.el(doc.root, "view");
    doc.flush();

    doc.add_class(el, "wide");
    let first = doc.flush();
    assert!(!first.is_empty(), "the incremental flush reports damage");
    assert!(first.traversed, "the incremental flush ran a traversal");

    let second = doc.flush();
    assert!(second.is_empty(), "damage must not survive the harvest");
    assert!(
        !second.traversed,
        "a clean tree must not re-traverse the previously-damaged node"
    );
}

#[test]
fn display_none_flip_reports_relayout_and_leaves_no_stale_state() {
    // Flipping to `display: none` produces RELAYOUT on the flipped node (which
    // covers its now-hidden subtree); the pruned subtree must not leak stale
    // scheduling state into the next flush.
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
    assert!(second.is_empty(), "no stale damage from the pruned subtree");
    assert!(
        !second.traversed,
        "no stale dirty bits re-trigger a traversal"
    );
}

#[test]
fn sibling_invalidation_damage_is_harvested_and_cleared() {
    // `.a + .b` couples A's class to its following sibling B: flipping A's
    // class invalidates B through stylo's sibling invalidation, and B's
    // REPAINT damage must be harvested (and cleared) even though B was never
    // mutated itself.
    //
    // The pre-w3c-dom arena exposed subtree flushes, where this scenario also
    // exercised stylo's `pre_traverse` root substitution (flushing at A makes
    // stylo raise the traversal to A's *parent*, and the harvest must follow
    // that actual root — `driver::traverse_dom`'s return value — or B's
    // damage and the parent's dirty bit leak, forcing perpetual
    // re-traversal). `flush_styles` is always rooted at the parentless
    // document root, so the substitution cannot fire through this public API;
    // the flush still harvests from the driver-returned actual root by
    // contract (see `Document::flush_styles_with_sink`), and this test
    // pins the observable half: sibling-invalidated damage is reported once
    // and cleared.
    let mut doc = Doc::with_css(".a + .b { color: rgb(255, 0, 0) }");
    let a = doc.el(doc.root, "view");
    let b = doc.el(doc.root, "view.b");
    let initial = doc.flush();
    assert!(initial.is_empty(), "initial styling produces no damage");
    // B carries `.b`, but no `.a` sibling precedes it yet.
    assert_ne!(doc.color(b), rgb(255, 0, 0));

    doc.add_class(a, "a");
    let summary = doc.flush();

    // B — a sibling of the mutated node — is the invalidated node, and its
    // REPAINT damage must appear in the harvest.
    let damage = damage_of(&summary, b).expect("the invalidated sibling carries damage");
    assert!(damage.needs_repaint());
    assert!(!damage.needs_relayout());
    assert_eq!(doc.color(b), rgb(255, 0, 0));

    // The harvest cleared B's damage and the spine's dirty bits, so a
    // follow-up flush neither re-traverses nor re-reports.
    let second = doc.flush();
    assert!(second.is_empty(), "no leaked damage after the flush");
    assert!(
        !second.traversed,
        "no leaked dirty bit re-triggers a traversal"
    );
}

/// Build a tree, flip an inherited property at the root, flush with the given
/// parallelism, and return the resulting damage as a comparable set of
/// `(node id, damage bits)` pairs. `NodeId` is a raw slab index here (no
/// generation), so the id alone identifies the node within this run.
fn incremental_damage_set(parallelism: Parallelism) -> BTreeSet<(NodeId, u16)> {
    let mut doc = Doc::with_css(".theme { color: rgb(9, 9, 9) }");
    for _ in 0..40 {
        doc.el(doc.root, "view");
    }
    doc.flush();

    doc.add_class(doc.root, "theme");
    let summary = doc.dom.flush_styles_with(parallelism);
    summary
        .damage
        .into_iter()
        .map(|(id, damage)| (id, damage.bits()))
        .collect()
}

#[test]
fn parallel_and_sequential_flush_agree_on_damage_sets() {
    // A rayon (`Auto`) traversal may harvest in a different order than a
    // sequential one, but the damage *set* must be identical.
    let sequential = incremental_damage_set(Parallelism::Sequential);
    let auto = incremental_damage_set(Parallelism::Auto);
    assert_eq!(sequential, auto);
    assert!(
        !sequential.is_empty(),
        "an inherited-property flip damages the root and every descendant"
    );
}
