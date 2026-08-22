//! Shadow roots, slot assignment, and the flat tree, end to end: what a
//! shadow tree styles, inherits, lays out, paints, and hit-tests as.
//!
//! The behavior asserted here is the W3C one (DOM §shadow trees, CSS Scoping,
//! CSS Shadow Parts), not any one engine's approximation of it.

mod common;

use common::Doc;
use dom::{NodeId, ShadowRootMode, StylesheetOrigin};

/// A host with a shadow tree, plus the ids of interest.
struct Harness {
    doc: Doc,
    host: NodeId,
    shadow: NodeId,
}

impl Harness {
    fn new() -> Self {
        let mut doc = Doc::new();
        let host = doc.el(doc.root, "host");
        let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
        Self { doc, host, shadow }
    }

    fn shadow_el(&mut self, parent: NodeId, spec: &str) -> NodeId {
        self.doc.el(parent, spec)
    }

    fn rect(&self, id: NodeId) -> (f32, f32, f32, f32) {
        let layout = self
            .doc
            .dom
            .rounded_layout(id)
            .expect("node id is live after layout");
        (
            layout.location.x,
            layout.location.y,
            layout.size.width,
            layout.size.height,
        )
    }
}

#[test]
fn a_host_renders_its_shadow_tree_and_not_its_own_children() {
    let mut harness = Harness::new();
    let light = harness.doc.el(harness.host, "light");
    let shadowed = harness.shadow_el(harness.shadow, "shadowed");
    harness.doc.add_css(
        "page { display: linear; }
         host { display: linear; width: 100px; height: 100px; }
         light { width: 70px; height: 70px; }",
    );
    harness
        .doc
        .dom
        .add_shadow_stylesheet(harness.shadow, "shadowed { width: 40px; height: 20px; }");
    harness.doc.flush();

    assert_eq!(harness.rect(shadowed), (0.0, 0.0, 40.0, 20.0));
    assert!(
        harness
            .doc
            .dom
            .get(light)
            .unwrap()
            .computed_style()
            .is_none(),
        "an unassigned light child is not in the flat tree, so it is never styled"
    );
    assert_eq!(
        harness.rect(light),
        (0.0, 0.0, 0.0, 0.0),
        "and it generates no box"
    );
}

#[test]
fn the_shadow_root_is_reachable_only_through_its_host() {
    let harness = Harness::new();
    let dom = &harness.doc.dom;

    assert_eq!(dom.shadow_root(harness.host), Some(harness.shadow));
    assert_eq!(dom.shadow_host(harness.shadow), Some(harness.host));
    assert_eq!(
        dom.shadow_root_mode(harness.shadow),
        Some(ShadowRootMode::Open)
    );
    assert!(
        dom.get(harness.host).unwrap().child_ids().is_empty(),
        "a shadow root is not one of its host's children"
    );
    assert!(
        dom.is_connected(harness.shadow),
        "a shadow root of a connected host is itself connected"
    );
    assert!(dom.get(harness.shadow).unwrap().is_shadow_root());
    assert!(!dom.get(harness.shadow).unwrap().is_element());
}

#[test]
fn document_rules_stop_at_the_boundary_and_scoped_rules_stay_inside() {
    let mut harness = Harness::new();
    let inside = harness.shadow_el(harness.shadow, "target");
    let outside = harness.doc.el(harness.doc.root, "target");
    harness
        .doc
        .add_css("target { color: rgb(255, 0, 0); width: 5px; }");
    harness
        .doc
        .dom
        .add_shadow_stylesheet(harness.shadow, "target { width: 9px; }");
    harness.doc.flush();

    assert_eq!(
        harness.doc.value(inside, "width"),
        "9px",
        "the shadow tree's own rules match inside it"
    );
    assert_eq!(
        harness.doc.value(outside, "width"),
        "5px",
        "and do not leak out of it"
    );
    assert_eq!(
        harness.doc.value(inside, "color"),
        "rgb(0, 0, 0)",
        "document author rules do not match inside a shadow tree"
    );
    assert_eq!(harness.doc.value(outside, "color"), "rgb(255, 0, 0)");
}

#[test]
fn host_matches_the_host_from_inside_the_shadow_tree_only() {
    let mut harness = Harness::new();
    harness.doc.add_class(harness.host, "themed");
    let inside = harness.shadow_el(harness.shadow, "target");
    harness.doc.dom.add_shadow_stylesheet(
        harness.shadow,
        ":host { width: 11px; }
         :host(.themed) target { width: 13px; }
         :host(.other) target { height: 99px; }",
    );
    harness.doc.add_css(":host { height: 77px; }");
    harness.doc.flush();

    assert_eq!(harness.doc.value(harness.host, "width"), "11px");
    assert_eq!(harness.doc.value(inside, "width"), "13px");
    assert_eq!(
        harness.doc.value(inside, "height"),
        "auto",
        "a :host() condition that the host fails must not match"
    );
    assert_eq!(
        harness.doc.value(harness.host, "height"),
        "auto",
        ":host in a document stylesheet matches nothing"
    );
}

#[test]
fn a_descendant_selector_cannot_reach_across_the_boundary() {
    let mut harness = Harness::new();
    let inside = harness.shadow_el(harness.shadow, "target");
    harness
        .doc
        .dom
        .add_shadow_stylesheet(harness.shadow, "page target { width: 3px; }");
    harness.doc.flush();

    assert_eq!(
        harness.doc.value(inside, "width"),
        "auto",
        "the document element is not an ancestor inside the shadow tree"
    );
}

#[test]
fn slots_render_assigned_nodes_in_host_order_and_fall_back_when_empty() {
    let mut harness = Harness::new();
    let named_slot = harness.shadow_el(harness.shadow, "slot[name=head]");
    let default_slot = harness.shadow_el(harness.shadow, "slot");
    let empty_slot = harness.shadow_el(harness.shadow, "slot[name=nobody]");
    let fallback = harness.shadow_el(empty_slot, "fallback");

    let second = harness.doc.el(harness.host, "b");
    let first = harness.doc.el(harness.host, "a[slot=head]");
    let third = harness.doc.el(harness.host, "c");
    harness.doc.flush();

    let dom = &harness.doc.dom;
    assert_eq!(dom.assigned_nodes(named_slot), &[first]);
    assert_eq!(
        dom.assigned_nodes(default_slot),
        &[second, third],
        "unnamed slottables land in the default slot, in the host's child order"
    );
    assert_eq!(dom.assigned_nodes(empty_slot), &[] as &[NodeId]);
    assert_eq!(dom.assigned_slot(first), Some(named_slot));
    assert_eq!(dom.assigned_slot(second), Some(default_slot));
    assert!(
        dom.get(fallback).unwrap().computed_style().is_some(),
        "a slot with nothing assigned renders its own children as fallback content"
    );
}

#[test]
fn an_unmatched_slot_name_leaves_its_slottable_out_of_the_flat_tree() {
    let mut harness = Harness::new();
    harness.shadow_el(harness.shadow, "slot[name=head]");
    let orphan = harness.doc.el(harness.host, "a[slot=missing]");
    harness.doc.flush();

    assert_eq!(harness.doc.dom.assigned_slot(orphan), None);
    assert!(
        harness
            .doc
            .dom
            .get(orphan)
            .unwrap()
            .computed_style()
            .is_none()
    );
}

#[test]
fn changing_the_slot_attribute_reassigns_and_restyles() {
    let mut harness = Harness::new();
    let head = harness.shadow_el(harness.shadow, "slot[name=head]");
    let body = harness.shadow_el(harness.shadow, "slot");
    let moving = harness.doc.el(harness.host, "a");
    harness.doc.dom.add_shadow_stylesheet(
        harness.shadow,
        "slot[name=head] { color: rgb(255, 0, 0); }
         slot { color: rgb(0, 128, 0); }",
    );
    harness.doc.flush();

    assert_eq!(harness.doc.dom.assigned_slot(moving), Some(body));
    assert_eq!(
        harness.doc.value(moving, "color"),
        "rgb(0, 128, 0)",
        "a slotted node inherits through its slot, not its host"
    );

    harness.doc.set_attr(moving, "slot", "head");
    harness.doc.flush();

    assert_eq!(harness.doc.dom.assigned_slot(moving), Some(head));
    assert_eq!(harness.doc.dom.assigned_nodes(body), &[] as &[NodeId]);
    assert_eq!(
        harness.doc.value(moving, "color"),
        "rgb(255, 0, 0)",
        "and follows the slot it moved to"
    );
}

#[test]
fn an_empty_slot_name_is_the_default_slot() {
    let mut harness = Harness::new();
    let default_slot = harness.shadow_el(harness.shadow, "slot[name=]");
    let explicit = harness.doc.el(harness.host, "a[slot=]");
    let implicit = harness.doc.el(harness.host, "b");
    harness.doc.flush();

    assert_eq!(
        harness.doc.dom.assigned_nodes(default_slot),
        &[explicit, implicit],
        "an empty `slot`/`name` is the same name as an absent one"
    );
}

#[test]
fn renaming_a_slot_reassigns_the_tree() {
    let mut harness = Harness::new();
    let slot = harness.shadow_el(harness.shadow, "slot");
    let slottable = harness.doc.el(harness.host, "a[slot=head]");
    harness.doc.flush();
    assert_eq!(harness.doc.dom.assigned_slot(slottable), None);

    harness.doc.set_attr(slot, "name", "head");
    harness.doc.flush();
    assert_eq!(harness.doc.dom.assigned_slot(slottable), Some(slot));

    harness.doc.remove_attr(slot, "name");
    harness.doc.flush();
    assert_eq!(harness.doc.dom.assigned_slot(slottable), None);
}

#[test]
fn a_slot_added_later_claims_the_children_already_there() {
    let mut harness = Harness::new();
    let slottable = harness.doc.el(harness.host, "a");
    harness.doc.flush();
    assert_eq!(harness.doc.dom.assigned_slot(slottable), None);

    let slot = harness.shadow_el(harness.shadow, "slot");
    harness.doc.flush();

    assert_eq!(harness.doc.dom.assigned_slot(slottable), Some(slot));
    assert_eq!(harness.doc.dom.assigned_nodes(slot), &[slottable]);

    let wrapper = harness.shadow_el(harness.shadow, "wrapper");
    let nested_slot = harness.shadow_el(wrapper, "slot[name=extra]");
    let extra = harness.doc.el(harness.host, "b[slot=extra]");
    harness.doc.flush();

    assert_eq!(harness.doc.dom.assigned_slot(extra), Some(nested_slot));
}

#[test]
fn slotted_matches_a_slots_assigned_nodes_from_inside_the_shadow_tree() {
    let mut harness = Harness::new();
    harness.shadow_el(harness.shadow, "slot");
    let assigned = harness.doc.el(harness.host, "a");
    let nested = harness.doc.el(assigned, "a");
    harness.doc.dom.add_shadow_stylesheet(
        harness.shadow,
        "::slotted(a) { width: 17px; }
         ::slotted(b) { height: 17px; }",
    );
    harness.doc.flush();

    assert_eq!(harness.doc.value(assigned, "width"), "17px");
    assert_eq!(
        harness.doc.value(assigned, "height"),
        "auto",
        "::slotted only matches on the element type it names"
    );
    assert_eq!(
        harness.doc.value(nested, "width"),
        "auto",
        "::slotted matches assigned nodes only, not their descendants"
    );
}

#[test]
fn part_is_stylable_from_the_outer_tree_and_exportparts_forwards_it_outward() {
    let mut harness = Harness::new();
    let inner_host = harness.shadow_el(harness.shadow, "inner[exportparts=deep: outer-deep]");
    let plain = harness.shadow_el(harness.shadow, "plain[part=knob]");
    let unexposed = harness.shadow_el(harness.shadow, "hidden");
    let inner_shadow = harness
        .doc
        .dom
        .attach_shadow(inner_host, ShadowRootMode::Open);
    let deep = harness.doc.el(inner_shadow, "deep[part=deep]");

    harness.doc.add_css(
        "host::part(knob) { width: 21px; }
         host::part(outer-deep) { width: 23px; }
         host::part(hidden) { width: 25px; }",
    );
    harness.doc.flush();

    assert_eq!(harness.doc.value(plain, "width"), "21px");
    assert_eq!(
        harness.doc.value(deep, "width"),
        "23px",
        "exportparts forwards a nested tree's part under its exported name"
    );
    assert_eq!(
        harness.doc.value(unexposed, "width"),
        "auto",
        "an element with no part attribute is not reachable by ::part()"
    );
}

#[test]
fn inherited_properties_follow_the_flat_tree() {
    let mut harness = Harness::new();
    let slot = harness.shadow_el(harness.shadow, "slot");
    let slotted = harness.doc.el(harness.host, "a");
    let descendant = harness.doc.el(slotted, "b");
    harness.doc.add_css("host { color: rgb(0, 0, 255); }");
    harness
        .doc
        .dom
        .add_shadow_stylesheet(harness.shadow, "slot { color: rgb(0, 128, 0); }");
    harness.doc.flush();

    assert_eq!(harness.doc.value(slot, "color"), "rgb(0, 128, 0)");
    assert_eq!(
        harness.doc.value(slotted, "color"),
        "rgb(0, 128, 0)",
        "a slotted node's inherited values come from its slot"
    );
    assert_eq!(
        harness.doc.value(descendant, "color"),
        "rgb(0, 128, 0)",
        "and carry on down its own subtree"
    );
}

#[test]
fn a_shadow_tree_lays_out_and_hit_tests_as_the_hosts_content() {
    let mut harness = Harness::new();
    let frame = harness.shadow_el(harness.shadow, "frame");
    let slot = harness.shadow_el(frame, "slot");
    let slotted = harness.doc.el(harness.host, "a");
    harness.doc.add_css(
        "page { display: linear; }
         host { display: linear; width: 200px; height: 200px; }
         a { width: 30px; height: 30px; }",
    );
    harness.doc.dom.add_shadow_stylesheet(
        harness.shadow,
        "frame { display: linear; margin-left: 40px; margin-top: 10px; }
         slot { display: linear; margin-left: 5px; width: 30px; }",
    );
    harness.doc.flush();

    assert_eq!(harness.rect(frame), (40.0, 10.0, 160.0, 30.0));
    assert_eq!(harness.rect(slot), (5.0, 0.0, 30.0, 30.0));
    assert_eq!(
        harness.rect(slotted),
        (0.0, 0.0, 30.0, 30.0),
        "a slotted box is laid out inside the slot's formatting context"
    );

    assert!(harness.doc.dom.render());
    let hits = harness
        .doc
        .dom
        .elements_from_point(dom::Point2D::new(50.0, 20.0));
    assert_eq!(
        hits.first().copied(),
        Some(slotted),
        "the topmost hit is the slotted element, painted at its flat-tree position"
    );
    assert!(
        hits.contains(&slot) && hits.contains(&frame) && hits.contains(&harness.host),
        "and the hit chain runs out through the shadow tree to the host: {hits:?}"
    );
}

#[test]
fn removing_a_host_removes_its_shadow_tree_with_it() {
    let mut harness = Harness::new();
    let inside = harness.shadow_el(harness.shadow, "inside");
    harness.doc.flush();

    let removed = harness.doc.dom.drop_subtree(harness.host);
    assert_eq!(
        removed.len(),
        2,
        "the host and its one shadow-tree element carry payloads; the shadow root does not"
    );
    assert!(harness.doc.dom.get(harness.shadow).is_none());
    assert!(harness.doc.dom.get(inside).is_none());
    assert!(harness.doc.dom.get(harness.host).is_none());
}

#[test]
#[should_panic(expected = "cannot drop a shadow host on its own")]
fn a_host_cannot_be_dropped_without_its_shadow_tree() {
    let mut harness = Harness::new();
    harness.doc.dom.drop_element(harness.host);
}

#[test]
fn nested_shadow_trees_scope_independently() {
    let mut harness = Harness::new();
    let inner_host = harness.shadow_el(harness.shadow, "inner");
    let inner_shadow = harness
        .doc
        .dom
        .attach_shadow(inner_host, ShadowRootMode::Open);
    let deep = harness.doc.el(inner_shadow, "target");
    let shallow = harness.shadow_el(harness.shadow, "target");

    harness
        .doc
        .dom
        .add_shadow_stylesheet(harness.shadow, "target { width: 4px; }");
    harness
        .doc
        .dom
        .add_shadow_stylesheet(inner_shadow, "target { width: 8px; }");
    harness.doc.flush();

    assert_eq!(harness.doc.value(shallow, "width"), "4px");
    assert_eq!(
        harness.doc.value(deep, "width"),
        "8px",
        "the outer tree's rules do not reach into the inner one"
    );
}

#[test]
fn a_closed_shadow_root_styles_and_renders_the_same_way() {
    let mut doc = Doc::new();
    let host = doc.el(doc.root, "host");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Closed);
    let inside = doc.el(shadow, "inside");
    doc.dom
        .add_shadow_stylesheet(shadow, "inside { width: 6px; }");
    doc.flush();

    assert_eq!(
        doc.dom.shadow_root_mode(shadow),
        Some(ShadowRootMode::Closed)
    );
    assert_eq!(doc.value(inside, "width"), "6px");
}

#[test]
fn a_class_change_inside_a_shadow_tree_restyles_against_its_scoped_rules() {
    let mut harness = Harness::new();
    let inside = harness.shadow_el(harness.shadow, "target");
    harness
        .doc
        .dom
        .add_shadow_stylesheet(harness.shadow, ".hot { width: 15px; }");
    harness.doc.flush();
    assert_eq!(harness.doc.value(inside, "width"), "auto");

    harness.doc.add_class(inside, "hot");
    harness.doc.flush();
    assert_eq!(harness.doc.value(inside, "width"), "15px");

    harness.doc.remove_class(inside, "hot");
    harness.doc.flush();
    assert_eq!(harness.doc.value(inside, "width"), "auto");
}

#[test]
fn a_class_change_on_the_host_restyles_the_shadow_tree_through_host() {
    let mut harness = Harness::new();
    let inside = harness.shadow_el(harness.shadow, "target");
    harness
        .doc
        .dom
        .add_shadow_stylesheet(harness.shadow, ":host(.wide) target { width: 19px; }");
    harness.doc.flush();
    assert_eq!(harness.doc.value(inside, "width"), "auto");

    harness.doc.add_class(harness.host, "wide");
    harness.doc.flush();
    assert_eq!(
        harness.doc.value(inside, "width"),
        "19px",
        "a host state change invalidates the shadow tree that :host() selects on"
    );
}

#[test]
fn document_rules_still_style_the_light_children_a_slot_renders() {
    let mut harness = Harness::new();
    harness.shadow_el(harness.shadow, "slot");
    let slotted = harness.doc.el(harness.host, "a");
    harness.doc.add_css("host > a { width: 12px; }");
    harness.doc.flush();

    assert_eq!(
        harness.doc.value(slotted, "width"),
        "12px",
        "a slotted node stays in the document tree, so document rules match it"
    );
}

#[test]
fn a_slotted_text_node_lays_out_under_its_slot() {
    let mut harness = Harness::new();
    let slot = harness.shadow_el(harness.shadow, "slot");
    let text = harness.doc.dom.create_text_node("hello", ());
    harness.doc.dom.append_child(harness.host, text);
    harness.doc.flush();

    assert_eq!(harness.doc.dom.assigned_slot(text), Some(slot));
    assert_eq!(
        harness.doc.dom.assigned_nodes(slot),
        &[text],
        "a text node has no slot attribute, so it takes the default slot"
    );
}

/// A slot that is detached but kept alive must let go of what it was
/// assigning. Its links name arena storage that the assigned nodes can be
/// freed out of while the detached slot is still readable, and an unassigned
/// slot renders fallback content anyway — so the empty list is both the safe
/// answer and the correct one.
#[test]
fn a_detached_slot_drops_its_assignments_and_survives_their_removal() {
    let mut harness = Harness::new();
    let slot = harness.shadow_el(harness.shadow, "slot");
    let slottable = harness.doc.el(harness.host, "a");
    harness.doc.flush();
    assert_eq!(harness.doc.dom.assigned_slot(slottable), Some(slot));
    assert_eq!(harness.doc.dom.assigned_nodes(slot), [slottable]);

    // Detach the slot without freeing it: `assign_slots` can no longer reach
    // it, so nothing else will rewrite what it holds.
    harness.doc.dom.remove_element(slot);
    assert!(
        harness.doc.dom.assigned_nodes(slot).is_empty(),
        "a slot outside the shadow tree assigns nothing"
    );
    assert_eq!(harness.doc.dom.assigned_slot(slottable), None);

    // Freeing what it used to hold must leave the detached slot readable.
    harness.doc.dom.drop_element(slottable);
    assert!(harness.doc.dom.get(slottable).is_none());
    assert!(harness.doc.dom.assigned_nodes(slot).is_empty());

    // And it must still be usable as an ordinary element afterwards.
    harness.doc.dom.append_child(harness.doc.root, slot);
    harness.doc.flush();
    assert!(harness.doc.dom.get(slot).is_some());
}

/// The same hazard one level down: the slot leaves inside a detached
/// container rather than on its own.
#[test]
fn a_slot_detached_inside_a_container_also_drops_its_assignments() {
    let mut harness = Harness::new();
    let container = harness.shadow_el(harness.shadow, "div");
    let slot = harness.shadow_el(container, "slot");
    let slottable = harness.doc.el(harness.host, "a");
    harness.doc.flush();
    assert_eq!(harness.doc.dom.assigned_nodes(slot), [slottable]);

    harness.doc.dom.remove_element(container);
    assert!(harness.doc.dom.assigned_nodes(slot).is_empty());
    assert_eq!(harness.doc.dom.assigned_slot(slottable), None);

    harness.doc.dom.drop_element(slottable);
    assert!(harness.doc.dom.assigned_nodes(slot).is_empty());
    harness.doc.flush();
}

#[test]
fn removing_a_slot_stops_rendering_what_it_held() {
    let mut harness = Harness::new();
    let slot = harness.shadow_el(harness.shadow, "slot");
    let slottable = harness.doc.el(harness.host, "a");
    harness.doc.add_css(
        "page { display: linear; }
         host { display: linear; width: 100px; height: 100px; }
         a { width: 50px; height: 50px; }",
    );
    harness
        .doc
        .dom
        .add_shadow_stylesheet(harness.shadow, "slot { display: linear; }");
    harness.doc.flush();
    assert!(harness.doc.dom.render());
    assert_eq!(harness.doc.dom.assigned_slot(slottable), Some(slot));
    assert!(
        harness
            .doc
            .dom
            .elements_from_point(dom::Point2D::new(10.0, 10.0))
            .contains(&slottable)
    );

    harness.doc.dom.drop_subtree(slot);
    harness.doc.flush();
    assert!(harness.doc.dom.render());

    assert_eq!(harness.doc.dom.assigned_slot(slottable), None);
    assert!(
        !harness
            .doc
            .dom
            .elements_from_point(dom::Point2D::new(10.0, 10.0))
            .contains(&slottable),
        "with no slot left to render it, the slottable is out of the frame"
    );
}

#[test]
fn detaching_a_slottable_clears_its_assignment() {
    let mut harness = Harness::new();
    let slot = harness.shadow_el(harness.shadow, "slot");
    let slottable = harness.doc.el(harness.host, "a");
    harness.doc.flush();
    assert_eq!(harness.doc.dom.assigned_slot(slottable), Some(slot));

    harness.doc.dom.remove_element(slottable);
    assert_eq!(harness.doc.dom.assigned_slot(slottable), None);
    assert_eq!(harness.doc.dom.assigned_nodes(slot), &[] as &[NodeId]);

    harness.doc.dom.append_child(harness.host, slottable);
    assert_eq!(harness.doc.dom.assigned_slot(slottable), Some(slot));
}

const COMPONENT_CSS: &str = "frame { display: linear; } slot { display: linear; }";

fn component(doc: &mut Doc, parent: NodeId, rows: usize) -> (NodeId, NodeId, Vec<NodeId>) {
    let host = doc.el(parent, "host");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
    doc.dom.add_shadow_stylesheet(shadow, COMPONENT_CSS);
    let frame = doc.el(shadow, "frame");
    let slot = doc.el(frame, "slot");
    let rows = (0..rows).map(|_| doc.el(host, "row")).collect();
    (host, slot, rows)
}

const HOSTS: usize = 256;
const ROWS: usize = 8;

#[test]
fn a_page_of_many_hosts_slots_and_styles_every_one() {
    let mut doc = Doc::with_css(
        "page { display: linear; }
         host { display: linear; }
         row { width: 10px; height: 4px; }",
    );
    let root = doc.root;
    let components: Vec<_> = (0..HOSTS)
        .map(|_| component(&mut doc, root, ROWS))
        .collect();
    doc.flush();

    for (host, slot, rows) in &components {
        assert_eq!(
            doc.dom.assigned_nodes(*slot),
            rows.as_slice(),
            "every host's rows land in its own slot, in order"
        );
        assert!(doc.dom.shadow_root(*host).is_some());
    }
    for (_, _, rows) in components.iter().step_by(37) {
        assert_eq!(doc.value(rows[ROWS - 1], "width"), "10px");
    }
    let (_, _, last) = components.last().expect("the page has hosts");
    let bottom = doc
        .dom
        .rounded_layout(last[ROWS - 1])
        .expect("the last row of the last host is laid out");
    assert_eq!((bottom.size.width, bottom.size.height), (10.0, 4.0));
}

#[test]
fn appending_a_long_child_list_agrees_with_a_full_reassignment() {
    let mut doc = Doc::new();
    let root = doc.root;
    let (_, slot, rows) = component(&mut doc, root, 512);
    doc.flush();

    let incremental = doc.dom.assigned_nodes(slot).to_vec();
    assert_eq!(incremental, rows, "appends keep the host's child order");

    doc.set_attr(slot, "name", "");
    doc.remove_attr(slot, "name");
    doc.flush();

    assert_eq!(
        doc.dom.assigned_nodes(slot),
        incremental,
        "a full reassignment reproduces what the appends built"
    );
}

#[test]
fn mid_list_insertion_and_removal_keep_slot_order() {
    let mut doc = Doc::new();
    let root = doc.root;
    let (host, slot, mut rows) = component(&mut doc, root, 64);
    doc.flush();

    let inserted = doc.dom.create_element("row", ());
    doc.dom.insert_before(host, inserted, Some(rows[32]));
    rows.insert(32, inserted);
    assert_eq!(doc.dom.assigned_nodes(slot), rows.as_slice());

    let removed = rows.remove(16);
    doc.dom.drop_subtree(removed);
    assert_eq!(doc.dom.assigned_nodes(slot), rows.as_slice());

    doc.flush();
    assert_eq!(doc.dom.assigned_slot(rows[32]), Some(slot));
}

#[test]
fn a_late_slot_claims_a_long_existing_child_list() {
    let mut doc = Doc::new();
    let root = doc.root;
    let host = doc.el(root, "host");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
    let frame = doc.el(shadow, "frame");
    let rows: Vec<_> = (0..256).map(|_| doc.el(host, "row")).collect();
    doc.flush();
    assert_eq!(doc.dom.assigned_slot(rows[0]), None);

    let slot = doc.el(frame, "slot");
    doc.flush();

    assert_eq!(doc.dom.assigned_nodes(slot), rows.as_slice());
    assert!(
        doc.dom.get(rows[255]).unwrap().computed_style().is_some(),
        "every row is in the flat tree once a slot claims it"
    );
}

#[test]
fn hundreds_of_scoped_stylesheets_stay_scoped_to_their_own_tree() {
    let mut doc = Doc::new();
    let root = doc.root;
    let components: Vec<_> = (0..HOSTS).map(|_| component(&mut doc, root, 1)).collect();
    for (index, (host, ..)) in components.iter().enumerate() {
        let shadow = doc.dom.shadow_root(*host).expect("each host has a root");
        doc.dom
            .add_shadow_stylesheet(shadow, &format!("frame {{ width: {}px; }}", index + 1));
    }
    doc.flush();

    for (index, (host, ..)) in components.iter().enumerate() {
        let frame = doc
            .dom
            .shadow_root(*host)
            .and_then(|shadow| doc.dom.get(shadow))
            .and_then(|shadow| shadow.child_ids().first().copied())
            .expect("each shadow tree has its frame");
        assert_eq!(doc.value(frame, "width"), format!("{}px", index + 1));
    }
}

#[test]
fn deeply_nested_components_style_inherit_and_lay_out_through_every_boundary() {
    const DEPTH: usize = 64;
    let mut doc = Doc::with_css(
        "page { display: linear; color: rgb(0, 0, 255); }
         host { display: linear; }
         leaf { width: 7px; height: 3px; }",
    );
    let mut parent = doc.root;
    let mut hosts = Vec::with_capacity(DEPTH);
    for _ in 0..DEPTH {
        let host = doc.el(parent, "host");
        let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
        doc.dom.add_shadow_stylesheet(shadow, COMPONENT_CSS);
        let frame = doc.el(shadow, "frame");
        let slot = doc.el(frame, "slot");
        hosts.push((host, slot));
        parent = host;
    }
    let leaf = doc.el(parent, "leaf");
    doc.flush();

    for (host, slot) in &hosts {
        assert_eq!(
            doc.dom.shadow_host(doc.dom.shadow_root(*host).unwrap()),
            Some(*host)
        );
        assert_eq!(doc.dom.assigned_nodes(*slot).len(), 1);
    }
    assert_eq!(
        doc.value(leaf, "color"),
        "rgb(0, 0, 255)",
        "an inherited value crosses 64 host/slot boundaries"
    );
    let rect = doc
        .dom
        .rounded_layout(leaf)
        .expect("the innermost leaf is laid out");
    assert_eq!((rect.size.width, rect.size.height), (7.0, 3.0));
}

#[test]
fn removing_a_host_with_a_large_shadow_tree_frees_every_node() {
    let mut doc = Doc::new();
    let root = doc.root;
    let host = doc.el(root, "host");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
    let slot = doc.el(shadow, "slot");
    let shadow_only: Vec<_> = (0..128).map(|_| doc.el(shadow, "decor")).collect();
    let rows: Vec<_> = (0..128).map(|_| doc.el(host, "row")).collect();
    doc.flush();

    assert_eq!(doc.dom.drop_subtree(host).len(), 2 + 128 + 128);
    for id in shadow_only
        .into_iter()
        .chain(rows)
        .chain([shadow, slot, host])
    {
        assert!(doc.dom.get(id).is_none(), "node {id} outlived its host");
    }
    doc.flush();
}

#[test]
fn a_document_with_no_shadow_root_keeps_its_child_list_as_the_flat_tree() {
    let mut doc = Doc::new();
    let parent = doc.el(doc.root, "parent");
    let child = doc.el(parent, "child");
    doc.dom
        .add_stylesheet("child { width: 2px; }", StylesheetOrigin::Author);
    doc.flush();

    assert_eq!(doc.value(child, "width"), "2px");
    assert_eq!(doc.dom.assigned_slot(child), None);
    assert_eq!(doc.dom.shadow_root(parent), None);
}

/// A structural selector on a shadow host still invalidates the light children
/// whose sibling positions moved.
///
/// `note_child_list_change` collapses "restyle every child" into a single
/// `RESTYLE_DESCENDANTS` on the parent only in a document with no shadow root,
/// because Stylo propagates that hint along the flat tree: a host's flat
/// children are its shadow root's children, so the hint would arrive at the
/// slotted light children by way of the shadow tree, restyling the shadow
/// tree's own elements on the way. This pins the light children's side of that
/// — the part a reader of the collapsed form would expect to be at risk.
#[test]
fn a_structural_selector_on_a_shadow_host_still_restyles_its_light_children() {
    let mut doc = Doc::new();
    doc.add_css(
        "item { color: rgb(0, 0, 255) } \
         item:nth-last-child(2) { color: rgb(255, 0, 0) }",
    );
    let host = doc.el(doc.root, "host");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);
    doc.el(shadow, "slot");

    let first = doc.el(host, "item");
    let second = doc.el(host, "item");
    doc.flush();
    assert_eq!(
        doc.color(first),
        common::rgb(255, 0, 0),
        "the first of two is second-from-last"
    );

    // A third light child shifts every position counted from the end.
    let third = doc.el(host, "item");
    doc.flush();

    assert_eq!(
        doc.color(first),
        common::rgb(0, 0, 255),
        "the displaced child must lose :nth-last-child(2)"
    );
    assert_eq!(
        doc.color(second),
        common::rgb(255, 0, 0),
        "and its successor must gain it"
    );
    assert_eq!(doc.color(third), common::rgb(0, 0, 255));
}
