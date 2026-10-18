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
                    .0
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

// ---------------------------------------------------------------------------
// The Rust walk: the observed-events gate and a walk over caller-held steps
// ---------------------------------------------------------------------------

mod rust_walk {
    use std::sync::{Arc, Mutex};

    use super::super::{ElementEvent, EventDetail, EventPhase, EventStep, EventWalk, StepOutcome};
    use crate::input::PointerKind;
    use crate::tree::document::tests::device;
    use crate::{CustomElement, Document, NodeId, Point2D};

    type Log = Arc<Mutex<Vec<String>>>;

    /// Records `type:phase:timestamp:x` per delivery, and stops the walk at
    /// its own element when `stops` is set.
    struct Listener {
        who: &'static str,
        log: Log,
        stops: bool,
    }

    impl CustomElement<()> for Listener {
        fn observed_events(&self) -> &'static [&'static str] {
            &["pointerdown"]
        }

        fn handle_event(
            &self,
            _document: &mut Document<()>,
            _element: NodeId,
            event: &mut ElementEvent<'_>,
        ) {
            let phase = match event.phase() {
                EventPhase::Capturing => "capture",
                EventPhase::AtTarget => "at-target",
                EventPhase::Bubbling => "bubble",
            };
            let x = event.detail().position().map_or(-1.0, |point| point.x);
            self.log.lock().expect("never poisoned").push(format!(
                "{}:{}:{phase}:{}:{x}",
                self.who,
                event.type_name(),
                event.timestamp()
            ));
            if self.stops {
                event.stop_propagation();
            }
        }
    }

    /// `page > outer(probe-outer) > inner(probe-inner)`, both listening.
    fn page(log: &Log, outer_stops: bool) -> (Document<()>, NodeId, NodeId) {
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.define(
            "probe-outer",
            Box::new(Listener {
                who: "outer",
                log: Arc::clone(log),
                stops: outer_stops,
            }),
        );
        document.define(
            "probe-inner",
            Box::new(Listener {
                who: "inner",
                log: Arc::clone(log),
                stops: false,
            }),
        );
        let page = document.document_element().id();
        let outer = document.create_element("probe-outer", ());
        let inner = document.create_element("probe-inner", ());
        document.append_child(page, outer);
        document.append_child(outer, inner);
        (document, outer, inner)
    }

    fn pointer(x: f32) -> EventDetail {
        EventDetail::Pointer {
            position: Point2D::new(x, 0.0),
            id: 1,
            device: PointerKind::Touch,
            button: 0,
            buttons: 1,
        }
    }

    fn take(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.lock().expect("never poisoned"))
    }

    #[test]
    fn a_type_no_definition_observes_walks_nothing() {
        let log = Log::default();
        let (mut document, _outer, inner) = page(&log, false);
        assert!(document.observes_event("pointerdown"));
        assert!(!document.observes_event("click"));
        assert_eq!(
            document.observed_event_names().collect::<Vec<_>>(),
            ["pointerdown"],
            "the union of the definitions' lists, each name once"
        );

        let steps = document.event_steps(inner, true, true);
        document.dispatch_element_event_over(&steps, "click", &pointer(1.0), 5.0);
        document.dispatch_element_event(inner, "click", &pointer(1.0), true, true, 5.0);
        assert!(
            take(&log).is_empty(),
            "a defined element on the path hears only what its definition observes"
        );
    }

    /// A name is live while any definition observing it has an element: the
    /// first element of either probe makes `pointerdown` live, freeing one of
    /// two changes nothing, freeing the last ends its liveness, and a
    /// create-and-free between two drains reports nothing. The
    /// definition-based gate observes the name throughout.
    #[test]
    fn an_observed_name_is_live_while_an_element_observing_it_exists() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let log = Log::default();
        for name in ["probe-outer", "probe-inner"] {
            document.define(
                name,
                Box::new(Listener {
                    who: name,
                    log: Arc::clone(&log),
                    stops: false,
                }),
            );
        }
        assert!(
            document.take_observed_name_changes().is_empty(),
            "a definition alone makes nothing live"
        );
        assert!(document.observes_event("pointerdown"));

        let page = document.document_element().id();
        let outer = document.create_element("probe-outer", ());
        assert_eq!(
            document.take_observed_name_changes(),
            [("pointerdown", true)]
        );
        let inner = document.create_element("probe-inner", ());
        let second = document.create_element("probe-inner", ());
        document.append_child(page, outer);
        document.append_child(outer, inner);
        assert!(
            document.take_observed_name_changes().is_empty(),
            "already live: connecting and more instances change nothing"
        );

        document.drop_element(inner);
        document.drop_element(outer);
        assert!(
            document.take_observed_name_changes().is_empty(),
            "`second` still observes it, unconnected"
        );
        document.drop_element(second);
        assert_eq!(
            document.take_observed_name_changes(),
            [("pointerdown", false)]
        );
        assert!(document.observes_event("pointerdown"));

        let brief = document.create_element("probe-outer", ());
        document.drop_element(brief);
        assert!(
            document.take_observed_name_changes().is_empty(),
            "a change undone before the drain is not reported"
        );
        assert!(take(&log).is_empty());
    }

    #[test]
    fn a_document_with_no_observer_walks_nothing_for_slotchange() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        assert!(!document.observes_event("slotchange"));
        assert_eq!(document.observed_event_names().count(), 0);
        let page = document.document_element().id();
        // Nothing to call and nothing to fail: the gate returns first.
        document.dispatch_element_event(page, "slotchange", &EventDetail::None, true, false, 0.0);
    }

    #[test]
    fn a_walk_over_caller_held_steps_delivers_in_order_and_leaves_them_reusable() {
        let log = Log::default();
        let (mut document, _outer, inner) = page(&log, false);
        let steps = document.event_steps(inner, true, true);
        document.dispatch_element_event_over(&steps, "pointerdown", &pointer(3.0), 12.5);
        assert_eq!(
            take(&log),
            [
                "outer:pointerdown:capture:12.5:3",
                "inner:pointerdown:at-target:12.5:3",
                "outer:pointerdown:bubble:12.5:3",
            ],
            "capture root-inward, the target once, bubble outward, with the caller's detail and \
             timestamp"
        );
        // The steps were only borrowed: the caller hands the same list on.
        assert_eq!(steps, document.event_steps(inner, true, true));
    }

    #[test]
    fn a_non_bubbling_walk_over_steps_skips_the_bubble_pass_above_the_target() {
        let log = Log::default();
        let (mut document, _outer, inner) = page(&log, false);
        let steps = document.event_steps(inner, false, true);
        document.dispatch_element_event_over(&steps, "pointerdown", &pointer(0.0), 0.0);
        assert_eq!(
            take(&log),
            [
                "outer:pointerdown:capture:0:0",
                "inner:pointerdown:at-target:0:0"
            ]
        );
    }

    #[test]
    fn stop_propagation_ends_the_rust_walk_only() {
        let log = Log::default();
        let (mut document, _outer, inner) = page(&log, true);
        let steps = document.event_steps(inner, true, true);
        document.dispatch_element_event_over(&steps, "pointerdown", &pointer(0.0), 0.0);
        assert_eq!(
            take(&log),
            ["outer:pointerdown:capture:0:0"],
            "the capture-phase stop keeps the target from hearing it"
        );
        // A second walk over the same steps starts afresh: the stop belonged
        // to that one dispatch's event object, not to the path.
        document.dispatch_element_event_over(&steps, "pointerdown", &pointer(0.0), 0.0);
        assert_eq!(take(&log), ["outer:pointerdown:capture:0:0"]);
    }

    fn outcomes(document: &mut Document<()>, walk: &mut EventWalk<'_>) -> Vec<StepOutcome> {
        std::iter::from_fn(|| document.advance(walk)).collect()
    }

    fn outcome(step: &EventStep) -> StepOutcome {
        StepOutcome {
            node: step.node,
            target: step.target,
            capture: step.capture,
        }
    }

    #[test]
    fn a_walk_reports_every_step_and_runs_each_component_before_reporting_its_step() {
        let log = Log::default();
        let (mut document, outer, inner) = page(&log, false);
        let steps = document.event_steps(inner, true, true);
        let detail = pointer(2.0);
        let mut walk = document.event_walk(&steps, "pointerdown", &detail, 1.0);

        let mut seen = Vec::new();
        while let Some(step) = document.advance(&mut walk) {
            seen.push((step, take(&log)));
        }
        let expected: Vec<StepOutcome> = steps.steps().iter().map(outcome).collect();
        assert_eq!(
            seen.iter().map(|(step, _)| *step).collect::<Vec<_>>(),
            expected,
            "every step is reported, the at-target capture step included"
        );
        let delivered: Vec<(NodeId, bool, Vec<String>)> = seen
            .into_iter()
            .filter(|(_, log)| !log.is_empty())
            .map(|(step, log)| (step.node, step.capture, log))
            .collect();
        assert_eq!(
            delivered,
            [
                (
                    outer,
                    true,
                    vec!["outer:pointerdown:capture:1:2".to_owned()]
                ),
                (
                    inner,
                    false,
                    vec!["inner:pointerdown:at-target:1:2".to_owned()]
                ),
                (
                    outer,
                    false,
                    vec!["outer:pointerdown:bubble:1:2".to_owned()]
                ),
            ],
            "a step's component has run by the time the step is reported, and the target's \
             capture step delivers to no component"
        );
        assert!(document.advance(&mut walk).is_none());
        assert!(!walk.propagation_stopped());
    }

    #[test]
    fn a_component_stop_reports_its_own_step_and_ends_the_walk_for_good() {
        let log = Log::default();
        let (mut document, outer, inner) = page(&log, true);
        let steps = document.event_steps(inner, true, true);
        let detail = pointer(0.0);
        let mut walk = document.event_walk(&steps, "pointerdown", &detail, 0.0);

        let reported = outcomes(&mut document, &mut walk);
        assert_eq!(
            reported.last().map(|step| (step.node, step.capture)),
            Some((outer, true)),
            "the step that stopped is the last one reported: the rest of its node's listeners \
             still run"
        );
        assert_eq!(reported.len(), 3, "document node, page, outer");
        assert!(walk.propagation_stopped());
        assert!(
            document.advance(&mut walk).is_none(),
            "a stopped walk stays stopped"
        );
        assert_eq!(take(&log), ["outer:pointerdown:capture:0:0"]);
    }

    /// Stops immediately at its own element.
    struct ImmediateStop {
        log: Log,
    }

    impl CustomElement<()> for ImmediateStop {
        fn observed_events(&self) -> &'static [&'static str] {
            &["pointerdown"]
        }

        fn handle_event(
            &self,
            _document: &mut Document<()>,
            _element: NodeId,
            event: &mut ElementEvent<'_>,
        ) {
            self.log
                .lock()
                .expect("never poisoned")
                .push("immediate".to_owned());
            event.stop_immediate_propagation();
        }
    }

    #[test]
    fn an_immediate_stop_withholds_the_step_it_ran_at() {
        let log = Log::default();
        let mut document: Document<()> = Document::new(device(), "page", ());
        document.define(
            "probe-immediate",
            Box::new(ImmediateStop {
                log: Arc::clone(&log),
            }),
        );
        let page = document.document_element().id();
        let stopper = document.create_element("probe-immediate", ());
        let inner = document.create_element("view", ());
        document.append_child(page, stopper);
        document.append_child(stopper, inner);
        let steps = document.event_steps(inner, true, true);
        let detail = pointer(0.0);
        let mut walk = document.event_walk(&steps, "pointerdown", &detail, 0.0);

        let reported = outcomes(&mut document, &mut walk);
        assert_eq!(
            reported.iter().map(|step| step.node).collect::<Vec<_>>(),
            [document.root_node().id(), page],
            "nothing is reported from the stopping node on"
        );
        assert!(walk.propagation_stopped());
        assert_eq!(take(&log), ["immediate"]);
    }

    #[test]
    fn a_walk_of_a_type_no_definition_observes_still_reports_every_step() {
        let log = Log::default();
        let (mut document, _outer, inner) = page(&log, true);
        let steps = document.event_steps(inner, true, true);
        let detail = EventDetail::None;
        let mut walk = document.event_walk(&steps, "tap", &detail, 0.0);
        assert_eq!(
            outcomes(&mut document, &mut walk),
            steps.steps().iter().map(outcome).collect::<Vec<_>>()
        );
        assert!(take(&log).is_empty(), "no component is looked up for `tap`");
    }

    #[test]
    fn a_step_freed_mid_walk_is_skipped_unreported() {
        let log = Log::default();
        let (mut document, outer, inner) = page(&log, false);
        let steps = document.event_steps(inner, true, true);
        let detail = EventDetail::None;
        let mut walk = document.event_walk(&steps, "tap", &detail, 0.0);
        let first = document
            .advance(&mut walk)
            .expect("the document node's step");
        assert_eq!(first.node, document.root_node().id());

        document.drop_subtree(outer);
        let page = document.document_element().id();
        assert_eq!(
            outcomes(&mut document, &mut walk)
                .iter()
                .map(|step| (step.node, step.capture))
                .collect::<Vec<_>>(),
            [
                (page, true),
                (page, false),
                (document.root_node().id(), false)
            ],
            "the freed nodes' steps resolve to nothing"
        );
    }
}

#[test]
fn the_nearest_common_event_ancestor_is_inclusive_and_crosses_shadow_boundaries() {
    let (mut document, page, outer, inner) = nested();
    let sibling = document.create_element("view", ());
    document.append_child(outer, sibling);
    assert_eq!(
        document.nearest_common_event_ancestor(inner, inner),
        Some(inner)
    );
    assert_eq!(
        document.nearest_common_event_ancestor(inner, outer),
        Some(outer),
        "an ancestor of the other is the answer itself"
    );
    assert_eq!(
        document.nearest_common_event_ancestor(inner, sibling),
        Some(outer)
    );
    assert_eq!(
        document.nearest_common_event_ancestor(sibling, page),
        Some(page)
    );

    // A node inside a shadow tree meets a light node at the host or above.
    let shadow = document.attach_shadow(inner, ShadowRootMode::Open);
    let chrome = document.create_element("view", ());
    document.append_child(shadow, chrome);
    assert_eq!(
        document.nearest_common_event_ancestor(chrome, inner),
        Some(inner)
    );
    assert_eq!(
        document.nearest_common_event_ancestor(chrome, sibling),
        Some(outer)
    );

    // Two detached trees share nothing.
    let loose = document.create_element("view", ());
    assert_eq!(document.nearest_common_event_ancestor(loose, inner), None);
}

// ---------------------------------------------------------------------------
// The listener table
// ---------------------------------------------------------------------------

mod listener_table {
    use super::{nested, shadow_tree};
    use crate::event::{ElementEvent, EventDetail, ListenerKey, ListenerKinds, RegistrationChange};
    use crate::tree::document::tests::device;
    use crate::tree::shadow::ShadowRootMode;
    use crate::{CustomElement, Document, NodeId};

    const PAGE: ListenerKey = ListenerKey(0);
    const CARD: ListenerKey = ListenerKey(7);
    const BUBBLE: ListenerKinds = ListenerKinds::PATH_BUBBLE;
    const CAPTURE: ListenerKinds = ListenerKinds::PATH_CAPTURE;
    const GLOBAL: ListenerKinds = ListenerKinds::GLOBAL;

    fn names(names: &[std::sync::Arc<str>]) -> Vec<&str> {
        names.iter().map(|name| &**name).collect()
    }

    #[test]
    fn a_repeated_add_replaces_the_kinds_and_moves_the_counts_by_the_difference() {
        let (mut document, _page, _outer, inner) = nested();
        assert_eq!(
            document.add_listener(inner, "tap", BUBBLE, PAGE),
            RegistrationChange::FirstAdded
        );
        assert_eq!(document.realm_registration_counts(PAGE, "tap"), (1, 0));
        assert!(!document.realm_has_global(PAGE, "tap"));

        assert_eq!(
            document.add_listener(inner, "tap", BUBBLE | GLOBAL, PAGE),
            RegistrationChange::Unchanged,
            "the same (node, name, registrant) is one registration"
        );
        assert_eq!(document.realm_registration_counts(PAGE, "tap"), (1, 1));
        assert!(document.realm_has_global(PAGE, "tap"));

        assert_eq!(
            document.add_listener(inner, "tap", GLOBAL, PAGE),
            RegistrationChange::Unchanged
        );
        assert_eq!(document.realm_registration_counts(PAGE, "tap"), (0, 1));

        assert_eq!(
            document.add_listener(inner, "tap", ListenerKinds::empty(), PAGE),
            RegistrationChange::LastRemoved,
            "an empty set is a removal"
        );
        assert_eq!(document.realm_registration_counts(PAGE, "tap"), (0, 0));
        assert!(!document.hears("tap"));
    }

    #[test]
    fn a_name_changes_with_its_first_registration_added_and_its_last_removed() {
        let (mut document, _page, outer, inner) = nested();
        assert_eq!(
            document.add_listener(outer, "tap", BUBBLE, PAGE),
            RegistrationChange::FirstAdded
        );
        assert_eq!(
            document.add_listener(inner, "tap", CAPTURE, CARD),
            RegistrationChange::Unchanged,
            "another node and another registrant: the name is heard already"
        );
        assert_eq!(
            document.add_listener(inner, "scroll", BUBBLE, PAGE),
            RegistrationChange::FirstAdded,
            "registration counts are per name"
        );

        assert_eq!(
            document.remove_listener(inner, "tap", PAGE),
            RegistrationChange::Unchanged,
            "not registered by this registrant"
        );
        assert_eq!(
            document.remove_listener(inner, "unheard", PAGE),
            RegistrationChange::Unchanged
        );
        assert_eq!(
            document.remove_listener(outer, "tap", PAGE),
            RegistrationChange::Unchanged
        );
        assert_eq!(
            document.remove_listener(inner, "tap", CARD),
            RegistrationChange::LastRemoved
        );
        assert!(!document.hears("tap"));
        assert!(document.hears("scroll"));
        assert_eq!(
            document.add_listener(inner, "tap", BUBBLE, CARD),
            RegistrationChange::FirstAdded,
            "a name whose last registration was removed is first added again"
        );
    }

    #[test]
    fn a_stale_node_registers_nothing() {
        let (mut document, _page, _outer, inner) = nested();
        document.drop_element(inner);
        assert_eq!(
            document.add_listener(inner, "tap", BUBBLE, PAGE),
            RegistrationChange::Unchanged
        );
        assert!(!document.hears("tap"));
    }

    #[test]
    fn removing_a_registrant_answers_the_names_whose_last_registration_it_removed() {
        let (mut document, _page, outer, inner) = nested();
        document.add_listener(outer, "tap", BUBBLE, PAGE);
        document.add_listener(inner, "scroll", BUBBLE | GLOBAL, PAGE);
        document.add_listener(inner, "keydown", GLOBAL, PAGE);
        document.add_listener(inner, "tap", CAPTURE, CARD);

        let mut last_removed = document.remove_listeners_of(PAGE);
        last_removed.sort();
        assert_eq!(
            names(&last_removed),
            ["keydown", "scroll"],
            "`tap` is still the card's"
        );
        assert!(document.hears("tap"));
        assert!(!document.hears("scroll"));
        assert_eq!(document.realm_registration_counts(PAGE, "tap"), (0, 0));
        assert_eq!(document.realm_registration_counts(CARD, "tap"), (1, 0));
        assert!(!document.has_listener(outer, "tap"));
        assert!(document.has_listener(inner, "tap"));
        assert!(document.remove_listeners_of(PAGE).is_empty());
        assert!(
            document.take_pending_last_removals().is_empty(),
            "a registrant's removal returns its last removals rather than queuing them"
        );
    }

    #[test]
    fn a_freed_node_takes_its_registrations_and_queues_the_last_removal() {
        let (mut document, _page, outer, inner) = nested();
        document.add_listener(inner, "tap", BUBBLE, PAGE);
        document.add_listener(inner, "scroll", GLOBAL, CARD);
        document.add_listener(outer, "scroll", BUBBLE, PAGE);

        // Detaching is not freeing: a re-render moves nodes constantly.
        document.remove_element(inner);
        assert!(document.has_listener(inner, "tap"));
        assert!(document.take_pending_last_removals().is_empty());

        document.drop_element(inner);
        assert!(!document.has_listener(inner, "tap"));
        let last_removals = document.take_pending_last_removals();
        assert_eq!(
            last_removals
                .iter()
                .map(|(name, change)| (&**name, *change))
                .collect::<Vec<_>>(),
            [("tap", RegistrationChange::LastRemoved)],
            "`scroll` is still heard on `outer`"
        );
        assert!(document.take_pending_last_removals().is_empty(), "drained");
        assert_eq!(document.realm_registration_counts(CARD, "scroll"), (0, 0));
        assert_eq!(document.realm_registration_counts(PAGE, "scroll"), (1, 0));
        assert!(!document.hears("tap"));
    }

    #[test]
    fn an_add_that_undoes_a_queued_last_removal_withdraws_it() {
        let (mut document, _page, outer, inner) = nested();
        document.add_listener(inner, "tap", BUBBLE, PAGE);
        document.drop_subtree(inner);
        assert_eq!(
            document.add_listener(outer, "tap", BUBBLE, PAGE),
            RegistrationChange::Unchanged,
            "the host was never told `tap` lost its last registration"
        );
        assert!(
            document.take_pending_last_removals().is_empty(),
            "the last removal is withdrawn rather than reported after the add"
        );
        assert!(document.hears("tap"));
    }

    #[test]
    fn a_shadow_tree_freed_with_its_host_takes_its_registrations() {
        let (mut document, host, _root, button) = shadow_tree(ShadowRootMode::Open);
        document.add_listener(button, "tap", BUBBLE, PAGE);
        document.drop_element(host);
        assert!(document.get(button).is_none());
        assert_eq!(
            document
                .take_pending_last_removals()
                .iter()
                .map(|(name, change)| (&**name, *change))
                .collect::<Vec<_>>(),
            [("tap", RegistrationChange::LastRemoved)]
        );
    }

    #[test]
    fn has_listener_is_per_node_and_per_name() {
        let (mut document, page, outer, inner) = nested();
        document.add_listener(outer, "layout", GLOBAL, PAGE);
        assert!(document.has_listener(outer, "layout"), "any kind counts");
        assert!(!document.has_listener(inner, "layout"));
        assert!(!document.has_listener(page, "layout"));
        assert!(!document.has_listener(outer, "tap"));
    }

    struct Observer;

    impl CustomElement<()> for Observer {
        fn observed_events(&self) -> &'static [&'static str] {
            &["pointerdown"]
        }

        fn handle_event(&self, _: &mut Document<()>, _: NodeId, _: &mut ElementEvent<'_>) {}
    }

    #[test]
    fn a_name_is_heard_by_a_registration_or_an_observing_definition() {
        let mut document: Document<()> = Document::new(device(), "page", ());
        let page = document.document_element().id();
        assert!(!document.hears("pointerdown"));
        document.define("probe-observer", Box::new(Observer));
        assert!(
            document.hears("pointerdown"),
            "a definition that observes the name hears it, with no registration"
        );
        assert!(!document.hears("tap"));
        document.add_listener(page, "tap", CAPTURE, PAGE);
        assert!(document.hears("tap"));
        document.remove_listener(page, "tap", PAGE);
        assert!(!document.hears("tap"));
        assert!(
            !document.has_listeners_for("pointerdown"),
            "a definition is no registration"
        );
        document.add_listener(page, "pointerdown", GLOBAL, CARD);
        assert!(document.has_listeners_for("pointerdown"));
        document.remove_listener(page, "pointerdown", CARD);
        assert!(!document.has_listeners_for("pointerdown"));
        assert!(document.hears("pointerdown"));
        // An observed type still walks the components when nothing registered.
        document.dispatch_element_event(page, "pointerdown", &EventDetail::None, true, true, 0.0);
    }

    #[test]
    fn the_path_reaches_capture_registrations_on_capture_steps_and_bubble_ones_on_bubble_steps() {
        let (mut document, page, outer, inner) = nested();
        let ancestor_bubble = ListenerKey(1);
        let ancestor_capture = ListenerKey(2);
        let target_bubble = ListenerKey(3);
        let global_only = ListenerKey(4);
        document.add_listener(outer, "tap", BUBBLE, ancestor_bubble);
        document.add_listener(outer, "tap", CAPTURE, ancestor_capture);
        document.add_listener(inner, "tap", BUBBLE, target_bubble);
        document.add_listener(inner, "tap", BUBBLE, ancestor_bubble);
        document.add_listener(page, "tap", GLOBAL, global_only);
        document.add_listener(page, "scroll", CAPTURE, ListenerKey(5));

        let bubbling = document.event_steps(inner, true, false);
        assert_eq!(
            document.listeners_on_path(&bubbling, "tap").as_slice(),
            [ancestor_bubble, ancestor_capture, target_bubble],
            "each once, in capture order; a global registration is not a path one"
        );

        let non_bubbling = document.event_steps(inner, false, false);
        assert_eq!(
            document.listeners_on_path(&non_bubbling, "tap").as_slice(),
            [ancestor_capture, target_bubble, ancestor_bubble],
            "an ancestor's bubble registration is not reached; the target keeps its bubble turn"
        );

        let at_outer = document.event_steps(outer, false, false);
        assert_eq!(
            document.listeners_on_path(&at_outer, "tap").as_slice(),
            [ancestor_bubble, ancestor_capture],
            "at the target both kinds run"
        );
        assert!(document.listeners_on_path(&bubbling, "unheard").is_empty());
        assert_eq!(
            document.listeners_on_path(&bubbling, "scroll").as_slice(),
            [ListenerKey(5)],
            "the path is probed for its own name alone"
        );
    }

    /// A global registration in a shadow tree is reached by a path through
    /// that shadow root — the composed path from inside it, and the
    /// non-composed one, which ends there — and not by a path outside it.
    #[test]
    fn a_global_registration_in_a_shadow_tree_is_reached_only_through_its_shadow_root() {
        let (mut document, _host, _root, button) = shadow_tree(ShadowRootMode::Open);
        let page = document.document_element().id();
        let outside = document.create_element("view", ());
        document.append_child(page, outside);
        let inner = document.create_element("view", ());
        document.append_child(button, inner);
        document.add_listener(button, "tap", GLOBAL, CARD);

        for composed in [true, false] {
            let steps = document.event_steps(inner, true, composed);
            assert_eq!(
                document
                    .global_registrants_on_path(&steps, "tap")
                    .as_slice(),
                [CARD],
                "composed {composed}"
            );
        }
        let steps = document.event_steps(outside, true, true);
        assert!(
            document
                .global_registrants_on_path(&steps, "tap")
                .is_empty(),
            "a path outside the shadow tree"
        );
        let steps = document.event_steps(page, true, true);
        assert!(
            document
                .global_registrants_on_path(&steps, "tap")
                .is_empty(),
            "the document element's path"
        );
        assert!(document.realm_has_global(CARD, "tap"), "held all the same");
    }

    /// A global registration in the document tree is reached by every
    /// connected composed path, one from inside a shadow tree included,
    /// since each ends at the document node; not by a detached tree's path,
    /// nor by a non-composed path that ends at a shadow root. Each
    /// registrant once, in the order its first global row was made.
    #[test]
    fn a_light_tree_global_registration_is_reached_by_every_connected_path() {
        let (mut document, _host, _root, button) = shadow_tree(ShadowRootMode::Open);
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.append_child(page, view);
        let loose = document.create_element("view", ());
        document.add_listener(view, "tap", GLOBAL, CARD);
        document.add_listener(page, "tap", GLOBAL | BUBBLE, PAGE);
        document.add_listener(page, "longpress", GLOBAL, CARD);
        document.add_listener(view, "tap", GLOBAL | CAPTURE, CARD);

        for target in [page, view, button] {
            let steps = document.event_steps(target, true, true);
            assert_eq!(
                document
                    .global_registrants_on_path(&steps, "tap")
                    .as_slice(),
                [CARD, PAGE],
                "{target:?}"
            );
        }
        let steps = document.event_steps(loose, true, true);
        assert!(
            document
                .global_registrants_on_path(&steps, "tap")
                .is_empty(),
            "a detached tree's path"
        );
        let steps = document.event_steps(button, true, false);
        assert!(
            document
                .global_registrants_on_path(&steps, "tap")
                .is_empty(),
            "a non-composed path that ends at a shadow root"
        );
    }

    /// The global rows follow every removal: a kinds change that drops
    /// `GLOBAL`, a removal, a freed node and a registrant going away.
    #[test]
    fn the_global_rows_follow_every_removal() {
        let (mut document, page, outer, inner) = nested();
        let reached = |document: &Document<()>| {
            let steps = document.event_steps(inner, true, true);
            document.global_registrants_on_path(&steps, "tap")
        };
        document.add_listener(outer, "tap", GLOBAL, CARD);
        document.add_listener(page, "tap", GLOBAL, PAGE);
        assert_eq!(reached(&document).as_slice(), [CARD, PAGE]);

        document.add_listener(outer, "tap", BUBBLE, CARD);
        assert_eq!(reached(&document).as_slice(), [PAGE], "kinds replaced");
        document.add_listener(outer, "tap", GLOBAL, CARD);
        assert_eq!(
            reached(&document).as_slice(),
            [PAGE, CARD],
            "filed again, last"
        );

        document.remove_listener(page, "tap", PAGE);
        assert_eq!(reached(&document).as_slice(), [CARD]);

        document.add_listener(inner, "tap", GLOBAL, PAGE);
        document.remove_element(inner);
        document.drop_element(inner);
        let steps = document.event_steps(outer, true, true);
        assert_eq!(
            document
                .global_registrants_on_path(&steps, "tap")
                .as_slice(),
            [CARD],
            "a freed node's global row left with it"
        );

        document.remove_listeners_of(CARD);
        assert!(
            document
                .global_registrants_on_path(&steps, "tap")
                .is_empty()
        );
    }

    #[test]
    fn would_reach_listener_answers_by_name_then_global_then_the_path() {
        let (mut document, page, outer, inner) = nested();
        let sibling = document.create_element("view", ());
        document.append_child(page, sibling);

        assert!(
            !document.would_reach_listener(inner, "tap", true, true),
            "no registration of the name"
        );

        document.add_listener(outer, "tap", BUBBLE, PAGE);
        assert!(document.would_reach_listener(inner, "tap", true, true));
        assert!(
            !document.would_reach_listener(inner, "tap", false, true),
            "a non-bubbling event misses an ancestor's bubble registration"
        );
        assert!(
            !document.would_reach_listener(sibling, "tap", true, true),
            "not on the sibling's path"
        );

        document.add_listener(outer, "tap", BUBBLE | CAPTURE, PAGE);
        assert!(
            document.would_reach_listener(inner, "tap", false, true),
            "a capture registration hears it on the way in"
        );

        document.add_listener(inner, "scroll", GLOBAL, CARD);
        assert!(
            document.would_reach_listener(sibling, "scroll", false, false),
            "a global registration hears the name on any path"
        );
    }
}
