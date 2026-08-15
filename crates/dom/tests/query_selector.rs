//! `querySelector`, `querySelectorAll`, `matches`, and `closest`.
//!
//! The behavior asserted here is the DOM Standard's (§4.2.6 "interface
//! `ParentNode`", §4.9 "interface Element"), which is also what the three
//! Stylo-based engines produce: Gecko, Servo, and Blitz all answer these four
//! APIs through `style::dom_apis`, and so does this crate.

mod common;

use common::Doc;
use dom::{NodeId, ShadowRootMode};

/// The document node — `document.querySelector(...)`'s receiver, as opposed to
/// [`Doc::root`], which is the document *element*.
fn document(doc: &Doc) -> NodeId {
    doc.dom.root_node().id()
}

fn first(doc: &Doc, root: NodeId, selectors: &str) -> Option<NodeId> {
    doc.dom
        .query_selector(root, selectors)
        .unwrap_or_else(|error| panic!("{error}"))
}

fn all(doc: &Doc, root: NodeId, selectors: &str) -> Vec<NodeId> {
    doc.dom
        .query_selector_all(root, selectors)
        .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn query_selector_returns_the_first_match_in_tree_order() {
    let mut doc = Doc::new();
    let branch = doc.el(doc.root, "view.branch");
    let deep = doc.el(branch, "view.target#deep");
    let later = doc.el(doc.root, "view.target#later");

    assert_eq!(first(&doc, doc.root, ".target"), Some(deep));
    assert_eq!(all(&doc, doc.root, ".target"), vec![deep, later]);
    assert_eq!(first(&doc, doc.root, "#later"), Some(later));
}

#[test]
fn query_selector_all_is_in_tree_order_not_selector_order() {
    let mut doc = Doc::new();
    let ids = doc.els(doc.root, &["view.a", "view.b", "view.a"]);

    // A selector list is a set: `.b, .a` reports the document's order, not the
    // list's. `dom_apis` refuses its single-selector fast paths for lists for
    // exactly this reason.
    assert_eq!(all(&doc, doc.root, ".b, .a"), ids);
    assert_eq!(all(&doc, doc.root, ".a, .b"), ids);
}

#[test]
fn the_query_root_is_never_its_own_match() {
    let mut doc = Doc::new();
    let outer = doc.el(doc.root, "view.a");
    let inner = doc.el(outer, "view.a");

    // Descendants only, so `outer` is excluded from its own query even though
    // it matches, and `:scope` alone can never match anything.
    assert_eq!(all(&doc, outer, ".a"), vec![inner]);
    assert_eq!(first(&doc, outer, ":scope"), None);
}

#[test]
fn scope_is_the_query_root() {
    let mut doc = Doc::new();
    let outer = doc.el(doc.root, "view#outer");
    let middle = doc.el(outer, "view.x");
    let deep = doc.el(middle, "view.x");

    assert_eq!(all(&doc, outer, ":scope > .x"), vec![middle]);
    assert_eq!(all(&doc, outer, ":scope .x"), vec![middle, deep]);
    // Rooted at the document there is no scoping element, so `:scope` falls
    // back to the document element.
    assert_eq!(all(&doc, document(&doc), ":scope > #outer"), vec![outer]);
}

#[test]
fn selectors_may_match_through_ancestors_outside_the_query_root() {
    let mut doc = Doc::new();
    let a = doc.el(doc.root, "view#a");
    let b = doc.el(a, "view#b");
    let c = doc.el(b, "view#c");

    // The DOM Standard's canonical scoped-query case: candidates are limited
    // to `b`'s descendants, but the selector is still evaluated against the
    // real tree, so `#a` outside the root still satisfies the combinator.
    assert_eq!(first(&doc, b, "#a view"), Some(c));
    assert_eq!(first(&doc, b, "#b > view"), Some(c));
}

#[test]
fn combinators_resolve_against_the_whole_tree() {
    let mut doc = Doc::new();
    let parent = doc.el(doc.root, "view.parent");
    let kids = doc.els(parent, &["view.a", "text", "view.b", "view.c"]);
    let grandchild = doc.el(kids[0], "view.b");

    assert_eq!(all(&doc, doc.root, ".parent > .b"), vec![kids[2]]);
    assert_eq!(all(&doc, doc.root, ".parent .b"), vec![grandchild, kids[2]]);
    assert_eq!(all(&doc, doc.root, ".a + *"), vec![kids[1]]);
    assert_eq!(all(&doc, doc.root, ".a ~ .c"), vec![kids[3]]);
    assert_eq!(all(&doc, doc.root, "text + .b"), vec![kids[2]]);
}

#[test]
fn structural_and_attribute_selectors_match() {
    let mut doc = Doc::new();
    let parent = doc.el(doc.root, "view");
    let kids = doc.els(
        parent,
        &["view[k=one]", "view[k=two]", "text", "view[k=three]"],
    );

    assert_eq!(all(&doc, parent, "[k]"), vec![kids[0], kids[1], kids[3]]);
    assert_eq!(all(&doc, parent, "[k=two]"), vec![kids[1]]);
    assert_eq!(all(&doc, parent, "[k^=t]"), vec![kids[1], kids[3]]);
    assert_eq!(all(&doc, parent, ":nth-child(2)"), vec![kids[1]]);
    assert_eq!(all(&doc, parent, "view:last-child"), vec![kids[3]]);
    assert_eq!(
        all(&doc, parent, "view:not([k=two])"),
        vec![kids[0], kids[3]]
    );
    assert_eq!(
        all(&doc, parent, ":is([k=one], [k=three])"),
        vec![kids[0], kids[3]]
    );
}

#[test]
fn only_elements_are_candidates() {
    let mut doc = Doc::new();
    let parent = doc.el(doc.root, "view");
    let element = doc.el(parent, "view");
    let text = doc.dom.create_text_node("hello", ());
    doc.dom.append_child(parent, text);

    assert_eq!(all(&doc, parent, "*"), vec![element]);
    assert_eq!(
        all(&doc, document(&doc), "*"),
        vec![doc.root, parent, element]
    );
}

#[test]
fn pseudo_elements_parse_but_never_match() {
    let mut doc = Doc::new();
    doc.el(doc.root, "view");

    // Per spec `querySelector` accepts a pseudo-element and then matches
    // nothing — it is not a parse error.
    assert_eq!(first(&doc, doc.root, "view::before"), None);
    assert_eq!(all(&doc, doc.root, "view::marker"), Vec::new());
}

#[test]
fn an_invalid_selector_is_reported_rather_than_matching_nothing() {
    let mut doc = Doc::new();
    doc.el(doc.root, "view");

    for selectors in ["view >", "!!", ":nonsense-pseudo-class", "::before *"] {
        let error = doc
            .dom
            .query_selector(doc.root, selectors)
            .expect_err("invalid selector must not parse");
        assert_eq!(error.selectors(), selectors);
        assert!(doc.dom.query_selector_all(doc.root, selectors).is_err());
        assert!(doc.dom.matches(doc.root, selectors).is_err());
        assert!(doc.dom.closest(doc.root, selectors).is_err());
    }

    assert_eq!(
        doc.dom
            .query_selector(doc.root, "view >")
            .expect_err("invalid selector must not parse")
            .to_string(),
        "`view >` is not a valid selector"
    );
}

#[test]
fn matches_tests_the_element_itself_with_itself_as_scope() {
    let mut doc = Doc::new();
    let parent = doc.el(doc.root, "view#parent");
    let child = doc.el(parent, "view.child[k=v]");

    assert!(doc.dom.matches(child, ".child").expect("valid selector"));
    assert!(doc.dom.matches(child, "#parent > [k=v]").expect("valid"));
    assert!(doc.dom.matches(child, ":scope").expect("valid selector"));
    assert!(!doc.dom.matches(child, "#parent").expect("valid selector"));
    assert!(!doc.dom.matches(child, "text .child").expect("valid"));
}

#[test]
fn closest_walks_inclusive_ancestors_and_stops_at_the_tree_root() {
    let mut doc = Doc::new();
    let outer = doc.el(doc.root, "view.match#outer");
    let middle = doc.el(outer, "view#middle");
    let inner = doc.el(middle, "view.match#inner");

    assert_eq!(
        doc.dom.closest(inner, ".match").expect("valid"),
        Some(inner)
    );
    assert_eq!(
        doc.dom.closest(middle, ".match").expect("valid"),
        Some(outer)
    );
    assert_eq!(
        doc.dom.closest(inner, "page").expect("valid"),
        Some(doc.root)
    );
    assert_eq!(doc.dom.closest(inner, ".absent").expect("valid"), None);
}

#[test]
fn a_detached_subtree_is_queryable() {
    let mut doc = Doc::new();
    let detached = doc.dom.create_element("view", ());
    let child = doc.dom.create_element("view", ());
    doc.dom.add_class(child, "target");
    doc.dom.append_child(detached, child);

    assert!(!doc.dom.is_connected(detached));
    assert_eq!(first(&doc, detached, ".target"), Some(child));
    assert_eq!(first(&doc, doc.root, ".target"), None);
}

#[test]
fn a_light_tree_query_never_descends_into_a_shadow_tree() {
    let mut doc = Doc::new();
    let host = doc.el(doc.root, "host");
    let light = doc.el(host, "view.target");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
    let shadowed = doc.el(shadow, "view.target");

    // A shadow root is not among its host's children, so the node-tree walk
    // the DOM Standard specifies for `querySelector` cannot reach it.
    assert_eq!(all(&doc, document(&doc), ".target"), vec![light]);
    assert_eq!(all(&doc, host, ".target"), vec![light]);
    // Rooting the query at the shadow root queries that tree instead, and it
    // does not escape back out into the light tree.
    assert_eq!(all(&doc, shadow, ".target"), vec![shadowed]);
}

#[test]
fn a_slotted_light_child_stays_in_the_light_tree_for_queries() {
    let mut doc = Doc::new();
    let host = doc.el(doc.root, "host");
    let slotted = doc.el(host, "view.slotted");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
    let slot = doc.el(shadow, "slot");
    doc.flush();

    assert_eq!(doc.dom.assigned_nodes(slot), [slotted]);
    // Selectors run on the node tree, not the flat tree: assignment moves
    // where `slotted` renders, never where it is found or what it descends
    // from.
    assert_eq!(all(&doc, host, ".slotted"), vec![slotted]);
    assert_eq!(all(&doc, shadow, ".slotted"), Vec::new());
    assert!(doc.dom.matches(slotted, "host > .slotted").expect("valid"));
    assert!(!doc.dom.matches(slotted, "slot > .slotted").expect("valid"));
}

#[test]
fn closest_does_not_escape_a_shadow_tree() {
    let mut doc = Doc::new();
    let host = doc.el(doc.root, "host.match");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
    let outer = doc.el(shadow, "view.match");
    let inner = doc.el(outer, "view");

    assert_eq!(
        doc.dom.closest(inner, ".match").expect("valid"),
        Some(outer)
    );
    // The host matches too, but the walk stops at the shadow root.
    assert_eq!(doc.dom.closest(outer, "host").expect("valid"), None);
    assert_eq!(doc.dom.closest(host, "host").expect("valid"), Some(host));
}
