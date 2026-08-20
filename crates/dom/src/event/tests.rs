//! Path-construction behavior, asserted against the WHATWG algorithm. The
//! shadow cases live here rather than in `tree::shadow` because what they
//! exercise is retargeting, which only the event path performs.

use super::EventStep;
use crate::tree::document::tests::device;
use crate::tree::node::Node;
use crate::tree::shadow::ShadowRootMode;
use crate::{Document, NodeId};

/// `page > outer > inner`, returning the two descendants.
fn nested() -> (Document<()>, NodeId, NodeId, NodeId) {
    let mut document: Document<()> = Document::new(device(), "page", ());
    let page = document.document_element().id();
    let outer = document.create_element("view", ());
    let inner = document.create_element("view", ());
    document.append_child(page, outer);
    document.append_child(outer, inner);
    (document, page, outer, inner)
}

fn document_node(document: &Document<()>) -> NodeId {
    document.root_node().id()
}

fn nodes(steps: &[EventStep]) -> Vec<NodeId> {
    steps.iter().map(|step| step.node).collect()
}

#[test]
fn a_bubbling_event_walks_root_inward_then_target_outward() {
    let (document, page, outer, inner) = nested();
    let root = document_node(&document);
    let steps = document.event_steps(inner, true, false);

    assert_eq!(
        nodes(steps.steps()),
        vec![root, page, outer, inner, inner, outer, page, root],
        "capture runs root first, bubble runs target first, and the target is in both"
    );
}

#[test]
fn the_target_appears_once_per_pass_at_target_phase() {
    let (document, _page, _outer, inner) = nested();
    let steps = document.event_steps(inner, true, false);

    let at_target: Vec<EventStep> = steps
        .steps()
        .iter()
        .copied()
        .filter(|step| step.node == inner)
        .collect();
    assert_eq!(at_target.len(), 2);
    assert!(
        at_target.iter().all(|step| step.node == step.target),
        "an at-target step is one whose target is itself"
    );
    assert_eq!(
        at_target
            .iter()
            .map(|step| step.capture)
            .collect::<Vec<_>>(),
        vec![true, false],
        "the capture pass reaches the target before the bubble pass does"
    );
}

#[test]
fn a_non_bubbling_event_still_captures_but_bubbles_only_at_the_target() {
    let (document, page, outer, inner) = nested();
    let root = document_node(&document);
    let steps = document.event_steps(inner, false, false);

    assert_eq!(
        nodes(steps.steps()),
        vec![root, page, outer, inner, inner],
        "the bubble pass drops every non-target step, and the target keeps both turns"
    );
}

#[test]
fn a_detached_subtree_ends_at_its_topmost_ancestor() {
    let mut document: Document<()> = Document::new(device(), "page", ());
    let orphan = document.create_element("view", ());
    let child = document.create_element("view", ());
    document.append_child(orphan, child);

    let steps = document.event_steps(child, true, false);
    assert_eq!(
        nodes(steps.steps()),
        vec![orphan, child, child, orphan],
        "a detached target still produces a path, ending where the parent chain does"
    );
}

#[test]
fn a_step_that_outlived_its_node_names_nothing_rather_than_a_stranger() {
    let (mut document, _page, outer, inner) = nested();
    let steps = document.event_steps(inner, true, false);

    // Detaching keeps every step resolvable — a re-render does this constantly.
    document.remove_element(outer);
    assert!(
        steps
            .steps()
            .iter()
            .all(|step| document.get(step.node).is_some())
    );

    // Freeing retires the ids. The steps naming them resolve to nothing, and
    // no later element can take their place, so a host holding this path needs
    // no staleness protocol of its own.
    document.drop_subtree(outer);
    assert!(document.get(outer).is_none() && document.get(inner).is_none());
    let live = steps
        .steps()
        .iter()
        .filter(|step| document.get(step.node).is_some())
        .count();
    assert_eq!(live, 4, "only the page and document steps still resolve");
}

#[test]
#[should_panic(expected = "stale target NodeId")]
fn a_path_from_a_freed_node_is_refused() {
    let (mut document, _page, _outer, inner) = nested();
    document.drop_element(inner);
    let _ = document.event_steps(inner, true, false);
}

// --- shadow trees -------------------------------------------------------

/// `page > host`, with a shadow tree `#shadow-root > button`.
fn shadow_tree(mode: ShadowRootMode) -> (Document<()>, NodeId, NodeId, NodeId) {
    let mut document: Document<()> = Document::new(device(), "page", ());
    let page = document.document_element().id();
    let host = document.create_element("view", ());
    document.append_child(page, host);
    let root = document.attach_shadow(host, mode);
    let button = document.create_element("view", ());
    document.append_child(root, button);
    (document, host, root, button)
}

#[test]
fn a_composed_event_retargets_to_the_host_outside_the_shadow_tree() {
    let (document, host, shadow_root, button) = shadow_tree(ShadowRootMode::Open);
    let page = document.document_element().id();
    let root = document_node(&document);
    let steps = document.event_steps(button, true, true);

    let bubble: Vec<EventStep> = steps
        .steps()
        .iter()
        .copied()
        .filter(|step| !step.capture)
        .collect();
    assert_eq!(nodes(&bubble), vec![button, shadow_root, host, page, root]);
    assert_eq!(
        bubble.iter().map(|step| step.target).collect::<Vec<_>>(),
        vec![button, button, host, host, host],
        "the outer tree sees the host, never the node inside the shadow tree"
    );
    assert_eq!(
        bubble
            .iter()
            .map(|step| step.node == step.target)
            .collect::<Vec<_>>(),
        vec![true, false, true, false, false],
        "the host is an at-target step: it is the target the outer tree sees"
    );
}

#[test]
fn a_closed_shadow_root_retargets_the_same_way() {
    let (document, host, _shadow_root, button) = shadow_tree(ShadowRootMode::Closed);
    let page = document.document_element().id();
    let root = document_node(&document);
    let steps = document.event_steps(button, true, true);

    // Every step from the host outward — not just the host's own — must report
    // the host. Checking only the host would still pass if the walk reverted
    // to the inner node one step later.
    for node in [host, page, root] {
        let targets: Vec<NodeId> = steps
            .steps()
            .iter()
            .filter(|step| step.node == node)
            .map(|step| step.target)
            .collect();
        assert!(
            !targets.is_empty() && targets.iter().all(|target| *target == host),
            "node {node} was told the target is {targets:?}, not the host"
        );
    }
}

#[test]
fn a_non_composed_event_stops_at_the_shadow_root_it_started_under() {
    let (document, _host, shadow_root, button) = shadow_tree(ShadowRootMode::Open);
    let steps = document.event_steps(button, true, false);

    assert_eq!(
        nodes(steps.steps()),
        vec![shadow_root, button, button, shadow_root],
        "without `composed` the path never reaches the host"
    );
}

#[test]
fn a_slotted_node_walks_up_through_its_slot_without_retargeting() {
    let mut document: Document<()> = Document::new(device(), "page", ());
    let page = document.document_element().id();
    let host = document.create_element("view", ());
    document.append_child(page, host);
    let shadow_root = document.attach_shadow(host, ShadowRootMode::Open);
    let wrapper = document.create_element("view", ());
    document.append_child(shadow_root, wrapper);
    let slot = document.create_element("slot", ());
    document.append_child(wrapper, slot);
    let light = document.create_element("view", ());
    document.append_child(host, light);
    assert_eq!(document.assigned_slot(light), Some(slot));

    // A light-DOM target starts in the document tree, so even a non-composed
    // event walks out through the shadow tree that displays it.
    let steps = document.event_steps(light, true, false);
    let bubble: Vec<EventStep> = steps
        .steps()
        .iter()
        .copied()
        .filter(|step| !step.capture)
        .collect();
    assert_eq!(
        nodes(&bubble),
        vec![
            light,
            slot,
            wrapper,
            shadow_root,
            host,
            page,
            document_node(&document)
        ]
    );
    assert!(
        bubble.iter().all(|step| step.target == light),
        "nothing is retargeted: the target never left the tree it started in"
    );
}

#[test]
fn retargeting_chains_across_two_shadow_boundaries() {
    let mut document: Document<()> = Document::new(device(), "page", ());
    let page = document.document_element().id();
    let outer_host = document.create_element("view", ());
    document.append_child(page, outer_host);
    let outer_root = document.attach_shadow(outer_host, ShadowRootMode::Open);
    let inner_host = document.create_element("view", ());
    document.append_child(outer_root, inner_host);
    let inner_root = document.attach_shadow(inner_host, ShadowRootMode::Open);
    let button = document.create_element("view", ());
    document.append_child(inner_root, button);

    let steps = document.event_steps(button, true, true);
    let bubble: Vec<EventStep> = steps
        .steps()
        .iter()
        .copied()
        .filter(|step| !step.capture)
        .collect();

    assert_eq!(
        nodes(&bubble),
        vec![
            button,
            inner_root,
            inner_host,
            outer_root,
            outer_host,
            page,
            document_node(&document)
        ]
    );
    assert_eq!(
        bubble.iter().map(|step| step.target).collect::<Vec<_>>(),
        vec![
            button, button, inner_host, inner_host, outer_host, outer_host, outer_host
        ],
        "each boundary re-targets again: the outer tree learns only the outermost host"
    );
}

#[test]
fn a_shadow_host_keeps_its_bubble_turn_when_the_event_does_not_bubble() {
    let (document, host, _shadow_root, button) = shadow_tree(ShadowRootMode::Open);
    let steps = document.event_steps(button, false, true);

    // `bubbles: false` drops ancestor steps from the bubble pass, but a
    // shadow-adjusted target is not an ancestor step — the standard keeps it.
    let bubble: Vec<NodeId> = steps
        .steps()
        .iter()
        .filter(|step| !step.capture)
        .map(|step| step.node)
        .collect();
    assert_eq!(bubble, vec![button, host]);
}

// --- the O(1) retargeting rule, checked against the standard's own ---------

/// The standard's predicate, written out: is `ancestor` a shadow-including
/// inclusive ancestor of `node`? A shadow root's parent is its host here, so
/// the plain parent chain is already the shadow-including one.
fn literally_is_ancestor(document: &Document<()>, ancestor: NodeId, node: NodeId) -> bool {
    let mut current = Some(node);
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        current = document.get(id).and_then(Node::parent_id);
    }
    false
}

/// `event_path`, rebuilt against that predicate instead of the
/// one-comparison equivalent the implementation uses.
fn literal_path(
    document: &Document<()>,
    target: NodeId,
    composed: bool,
) -> Vec<(NodeId, NodeId, bool)> {
    let mut path = vec![(target, target, true)];
    let target_root = document.tree_root(target);
    let mut current_target = target;
    let mut current_root = target_root;
    let mut node = target;
    while let Some(parent) = document.event_parent(node, composed, target_root) {
        let at_target = !literally_is_ancestor(document, current_root, parent);
        if at_target {
            current_target = parent;
            current_root = document.tree_root(parent);
        }
        path.push((parent, current_target, at_target));
        node = parent;
    }
    path
}

/// Every shape that can make the two disagree, if they ever do, each with the
/// roots its nodes hang from — a detached subtree is unreachable from the
/// document node and would otherwise go unchecked.
fn shapes() -> Vec<(&'static str, Document<()>, Vec<NodeId>)> {
    let mut shapes = Vec::new();

    let mut flat: Document<()> = Document::new(device(), "page", ());
    let page = flat.document_element().id();
    let mut parent = page;
    for _ in 0..3 {
        let child = flat.create_element("view", ());
        flat.append_child(parent, child);
        parent = child;
    }
    let roots = vec![flat.root_node().id()];
    shapes.push(("flat chain", flat, roots));

    let (shadow, ..) = shadow_tree(ShadowRootMode::Open);
    let roots = vec![shadow.root_node().id()];
    shapes.push(("one shadow root", shadow, roots));
    let (closed, ..) = shadow_tree(ShadowRootMode::Closed);
    let roots = vec![closed.root_node().id()];
    shapes.push(("one closed shadow root", closed, roots));

    // A host whose shadow tree slots light content, so the walk enters a
    // shadow tree going *up* without the target ever leaving its own.
    let mut slotted: Document<()> = Document::new(device(), "page", ());
    let page = slotted.document_element().id();
    let host = slotted.create_element("view", ());
    slotted.append_child(page, host);
    let root = slotted.attach_shadow(host, ShadowRootMode::Open);
    let wrapper = slotted.create_element("view", ());
    slotted.append_child(root, wrapper);
    let slot = slotted.create_element("slot", ());
    slotted.append_child(wrapper, slot);
    let light = slotted.create_element("view", ());
    slotted.append_child(host, light);
    let light_child = slotted.create_element("view", ());
    slotted.append_child(light, light_child);
    let roots = vec![slotted.root_node().id()];
    shapes.push(("slotted light content", slotted, roots));

    // Two boundaries stacked, so the target is retargeted twice.
    let mut nested: Document<()> = Document::new(device(), "page", ());
    let page = nested.document_element().id();
    let outer_host = nested.create_element("view", ());
    nested.append_child(page, outer_host);
    let outer_root = nested.attach_shadow(outer_host, ShadowRootMode::Open);
    let inner_host = nested.create_element("view", ());
    nested.append_child(outer_root, inner_host);
    let inner_root = nested.attach_shadow(inner_host, ShadowRootMode::Closed);
    let button = nested.create_element("view", ());
    nested.append_child(inner_root, button);
    let roots = vec![nested.root_node().id()];
    shapes.push(("nested shadow roots", nested, roots));

    // A slotted element that is itself a host: the walk crosses out of one
    // tree while standing inside another it entered through a slot.
    let mut both: Document<()> = Document::new(device(), "page", ());
    let page = both.document_element().id();
    let outer_host = both.create_element("view", ());
    both.append_child(page, outer_host);
    let outer_root = both.attach_shadow(outer_host, ShadowRootMode::Open);
    let slot = both.create_element("slot", ());
    both.append_child(outer_root, slot);
    let inner_host = both.create_element("view", ());
    both.append_child(outer_host, inner_host);
    let inner_root = both.attach_shadow(inner_host, ShadowRootMode::Open);
    let deep = both.create_element("view", ());
    both.append_child(inner_root, deep);
    let roots = vec![both.root_node().id()];
    shapes.push(("a slotted element that is itself a host", both, roots));

    // A detached subtree, where the path ends before any document node.
    let mut orphan: Document<()> = Document::new(device(), "page", ());
    let top = orphan.create_element("view", ());
    let child = orphan.create_element("view", ());
    orphan.append_child(top, child);
    let host = orphan.create_element("view", ());
    orphan.append_child(child, host);
    orphan.attach_shadow(host, ShadowRootMode::Open);
    let roots = vec![orphan.root_node().id(), top];
    shapes.push(("a detached subtree", orphan, roots));

    shapes
}

/// Every node in a document, including the ones inside shadow trees.
fn every_node(document: &Document<()>, roots: &[NodeId]) -> Vec<NodeId> {
    let mut found = Vec::new();
    let mut stack = roots.to_vec();
    while let Some(id) = stack.pop() {
        let Some(node) = document.get(id) else {
            continue;
        };
        found.push(id);
        stack.extend_from_slice(node.child_ids());
        if let Some(root) = document.shadow_root(id) {
            stack.push(root);
        }
    }
    found
}

#[test]
fn the_one_comparison_retargeting_rule_agrees_with_the_standards_predicate() {
    let mut checked = 0;
    for (name, document, roots) in shapes() {
        // Every live node, as a target, under both composed settings.
        for target in every_node(&document, &roots) {
            for composed in [false, true] {
                let actual: Vec<(NodeId, NodeId, bool)> = document
                    .event_path(target, composed)
                    .into_iter()
                    .map(|entry| (entry.node, entry.target, entry.at_target))
                    .collect();
                assert_eq!(
                    actual,
                    literal_path(&document, target, composed),
                    "{name}: target {target}, composed {composed}"
                );
                // An at-target entry is exactly one whose target is itself:
                // crossing a boundary sets the target to the node it crossed
                // to, and nothing else can make the two equal. Checked here
                // rather than argued, because a step's phase is derived from
                // it rather than carried.
                for (node, entry_target, at_target) in &actual {
                    assert_eq!(
                        *at_target,
                        node == entry_target,
                        "{name}: entry {node} claims at_target {at_target} with target {entry_target}"
                    );
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 80, "only {checked} paths compared");
}
