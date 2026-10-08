//! Intersection observers (`dom::visual::intersection`) through a real
//! document: the registry, the spec's "update intersection observations"
//! steps (§3.2.10), the owner-tagged delivery, and node lifetime.
//!
//! `Host`-owned observers are read through `take_intersection_records` and
//! `take_intersection_notifications`; the `Element` owner's delivery runs
//! through a recording component.

#![allow(clippy::float_cmp)]

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{Doc, device};
use dom::{
    CustomElement, Document, IntersectionObserverEntry, IntersectionObserverId,
    IntersectionObserverOwner, NodeId, Rect, RootMargin, Vector2D,
};

/// Every box is a flex container, and a `view` a non-shrinking one, so the
/// sizes a case writes are the sizes it gets.
const BASE: &str = "page { display: flex; } view { display: flex; flex-shrink: 0; }";

/// The time every update in these cases runs with.
const TIME: f64 = 12.5;

fn page(css: &str) -> Doc {
    Doc::with_css(&format!("{BASE}\n{css}"))
}

fn host(doc: &mut Doc, root: Option<NodeId>, thresholds: &[f64]) -> IntersectionObserverId {
    doc.dom.create_intersection_observer(
        IntersectionObserverOwner::Host,
        root,
        RootMargin::ZERO,
        thresholds.to_vec(),
    )
}

fn update(doc: &mut Doc) -> bool {
    doc.dom.update_intersection_observations(TIME)
}

fn tuple(rect: Rect<f32>) -> (f32, f32, f32, f32) {
    (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

/// `(target, is_intersecting, ratio)` per entry.
fn states(entries: &[IntersectionObserverEntry]) -> Vec<(NodeId, bool, f64)> {
    entries
        .iter()
        .map(|entry| {
            (
                entry.target,
                entry.is_intersecting,
                entry.intersection_ratio,
            )
        })
        .collect()
}

fn records(doc: &mut Doc, observer: IntersectionObserverId) -> Vec<(NodeId, bool, f64)> {
    states(&doc.dom.take_intersection_records(observer))
}

type Log = Rc<RefCell<Vec<String>>>;

/// A component that records its connection and every delivery it hears,
/// and appends a `child` element from its hook when it has one.
struct Probe {
    log: Log,
    child: Option<&'static str>,
}

impl CustomElement<()> for Probe {
    fn connected_callback(&self, _: &mut Document<()>, element: NodeId) {
        self.log.borrow_mut().push(format!("connected {element}"));
    }

    fn intersections_changed(
        &self,
        document: &mut Document<()>,
        element: NodeId,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        let targets: Vec<String> = entries
            .iter()
            .map(|entry| format!("{}:{}", entry.target, entry.is_intersecting))
            .collect();
        self.log.borrow_mut().push(format!(
            "observer {} at {element}: {}",
            observer.get(),
            targets.join(",")
        ));
        if let Some(tag) = self.child {
            let child = document.create_element(tag, ());
            document.append_child(element, child);
        }
        self.log.borrow_mut().push("hook returned".to_owned());
    }
}

/// A page defining `x-probe` (which appends an `x-child` from its hook) and
/// `x-child`, both recording into the returned log.
fn components(css: &str) -> (Doc, Log) {
    let log = Log::default();
    let mut doc = page(css);
    doc.dom.define(
        "x-probe",
        Box::new(Probe {
            log: Rc::clone(&log),
            child: Some("x-child"),
        }),
    );
    doc.dom.define(
        "x-child",
        Box::new(Probe {
            log: Rc::clone(&log),
            child: None,
        }),
    );
    (doc, log)
}

fn element_owned(doc: &mut Doc, owner: NodeId) -> IntersectionObserverId {
    doc.dom.create_intersection_observer(
        IntersectionObserverOwner::Element(owner),
        None,
        RootMargin::ZERO,
        Vec::new(),
    )
}

// --- The update -------------------------------------------------------------

#[test]
fn the_first_update_after_observe_queues_one_entry_per_target() {
    let mut doc = page(
        ".cell { width: 100px; height: 100px; }
         .below { position: relative; top: 700px; }",
    );
    let inside = doc.el(doc.root, "view.cell");
    let outside = doc.el(doc.root, "view.cell.below");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, inside);
    doc.dom.observe_intersection(observer, outside);
    doc.dom.render();
    assert!(update(&mut doc));

    let entries = doc.dom.take_intersection_records(observer);
    assert_eq!(entries.len(), 2, "one per target, in observe order");
    let (seen, unseen) = (entries[0], entries[1]);
    assert_eq!(seen.target, inside);
    assert_eq!(seen.time, TIME);
    assert!(seen.is_intersecting);
    assert_eq!(seen.intersection_ratio, 1.0);
    assert_eq!(tuple(seen.bounding_client_rect), (0.0, 0.0, 100.0, 100.0));
    assert_eq!(tuple(seen.intersection_rect), (0.0, 0.0, 100.0, 100.0));
    assert_eq!(tuple(seen.root_bounds), (0.0, 0.0, 800.0, 600.0));

    assert_eq!(unseen.target, outside);
    assert_eq!(unseen.time, TIME);
    assert!(!unseen.is_intersecting);
    assert_eq!(unseen.intersection_ratio, 0.0);
    assert_eq!(
        tuple(unseen.bounding_client_rect),
        (100.0, 700.0, 100.0, 100.0)
    );
    assert_eq!(tuple(unseen.intersection_rect), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(tuple(unseen.root_bounds), (0.0, 0.0, 800.0, 600.0));

    // Nothing went stale since.
    assert!(!update(&mut doc));
    assert!(!doc.dom.has_pending_intersection_notifications());
    // A render that moves neither target runs the update and queues nothing.
    doc.el(doc.root, "view.cell");
    assert!(doc.dom.render());
    assert!(!update(&mut doc));
    assert!(doc.dom.take_intersection_records(observer).is_empty());
}

#[test]
fn no_update_runs_before_the_first_render() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let cell = doc.el(doc.root, "view.cell");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, cell);
    doc.flush();
    assert!(!update(&mut doc), "no frame was ever committed");
    assert!(doc.dom.take_intersection_records(observer).is_empty());

    doc.dom.render();
    assert!(
        update(&mut doc),
        "the observe is still owed its first update"
    );
    assert_eq!(records(&mut doc, observer), vec![(cell, true, 1.0)]);
}

/// `[0, 0.5, 1]` crossed both ways by `scroll_to` alone — no render between
/// — over a 20px cell under a 100px `overflow: hidden` scrollport; a move
/// that crosses nothing queues nothing.
#[test]
fn thresholds_are_crossed_both_ways_by_scrolling_alone() {
    let mut doc = page(
        ".scroller { flex-direction: column; overflow: hidden; width: 100px; height: 100px; }
         .spacer { width: 100px; height: 110px; }
         .cell { width: 60px; height: 20px; }",
    );
    let scroller = doc.el(doc.root, "view.scroller");
    doc.el(scroller, "view.spacer");
    let cell = doc.el(scroller, "view.cell");
    let observer = host(&mut doc, None, &[1.0, 0.0, 0.5]);
    doc.dom.observe_intersection(observer, cell);
    doc.dom.render();
    assert!(update(&mut doc));
    assert_eq!(records(&mut doc, observer), vec![(cell, false, 0.0)]);

    // (offset, the entry it queues): the cell spans `110 - offset` to
    // `130 - offset` against a scrollport of 0 to 100.
    for (offset, expected) in [
        (10.0, Some((true, 0.0))),
        (15.0, None),
        (20.0, Some((true, 0.5))),
        (30.0, Some((true, 1.0))),
        (25.0, Some((true, 0.75))),
        (5.0, Some((false, 0.0))),
    ] {
        doc.dom.scroll_to(scroller, Vector2D::new(0.0, offset));
        let queued = update(&mut doc);
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(intersecting, ratio)| (cell, intersecting, ratio))
            .collect();
        assert_eq!(queued, !expected.is_empty(), "at {offset}");
        assert_eq!(records(&mut doc, observer), expected, "at {offset}");
    }
    assert!(
        !doc.dom.render(),
        "every scroll composed in the retained frame: nothing committed"
    );
}

/// A zero-area target on the viewport's edge intersects with ratio 1, and a
/// pixel past it leaves.
#[test]
fn a_zero_area_target_on_the_edge_intersects_with_ratio_one() {
    let mut doc = page(
        "page { flex-direction: column; width: 800px; height: 600px; }
         .spacer { width: 10px; height: 600px; }
         .sentinel { width: 100px; height: 0; }",
    );
    let spacer = doc.el(doc.root, "view.spacer");
    let sentinel = doc.el(doc.root, "view.sentinel");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, sentinel);
    doc.dom.render();
    assert!(update(&mut doc));
    assert_eq!(records(&mut doc, observer), vec![(sentinel, true, 1.0)]);

    doc.set_inline(spacer, "height: 601px");
    doc.dom.render();
    assert!(update(&mut doc));
    assert_eq!(records(&mut doc, observer), vec![(sentinel, false, 0.0)]);
}

/// The transform ruling, through the observer: moved out by a transform, the
/// target leaves, while its `bounding_client_rect` does not move.
#[test]
fn a_transform_moves_a_target_out() {
    let mut doc = Doc::with_device(device(300.0, 300.0));
    doc.add_css(&format!(
        "{BASE} page {{ width: 300px; height: 300px; }}
         .target {{ width: 100px; height: 100px; }}"
    ));
    let target = doc.el(doc.root, "view.target");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, target);
    doc.dom.render();
    update(&mut doc);
    assert_eq!(records(&mut doc, observer), vec![(target, true, 1.0)]);

    doc.set_inline(target, "transform: translateX(500px)");
    doc.dom.render();
    assert!(update(&mut doc));
    let entries = doc.dom.take_intersection_records(observer);
    assert_eq!(states(&entries), vec![(target, false, 0.0)]);
    assert_eq!(
        tuple(entries[0].bounding_client_rect),
        (500.0, 0.0, 100.0, 100.0)
    );
    assert_eq!(
        doc.dom.bounding_client_rect(target).map(tuple),
        Some((0.0, 0.0, 100.0, 100.0))
    );
}

#[test]
fn a_target_outside_its_root_reports_once_with_the_roots_bounds() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let target = doc.el(doc.root, "view.cell");
    let root = doc.el(doc.root, "view.cell");
    let observer = host(&mut doc, Some(root), &[]);
    doc.dom.observe_intersection(observer, target);
    doc.dom.render();
    assert!(update(&mut doc));
    let entries = doc.dom.take_intersection_records(observer);
    assert_eq!(states(&entries), vec![(target, false, 0.0)]);
    assert_eq!(tuple(entries[0].root_bounds), (100.0, 0.0, 100.0, 100.0));
    assert_eq!(tuple(entries[0].bounding_client_rect), (0.0, 0.0, 0.0, 0.0));

    doc.el(doc.root, "view.cell");
    doc.dom.render();
    assert!(!update(&mut doc), "nothing about it changed");
}

// --- The registry -----------------------------------------------------------

#[test]
fn observing_twice_is_observing_once() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let cell = doc.el(doc.root, "view.cell");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, cell);
    doc.dom.render();
    update(&mut doc);
    doc.dom.observe_intersection(observer, cell);
    assert!(!update(&mut doc), "the registration and its state are kept");
    assert_eq!(records(&mut doc, observer), vec![(cell, true, 1.0)]);
}

#[test]
fn unobserving_stops_entries() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let cell = doc.el(doc.root, "view.cell");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, cell);
    doc.dom.render();
    update(&mut doc);
    assert_eq!(records(&mut doc, observer).len(), 1);

    doc.dom.unobserve_intersection(observer, cell);
    doc.dom.unobserve_intersection(observer, doc.root);
    doc.set_inline(cell, "transform: translateY(1000px)");
    doc.dom.render();
    assert!(!update(&mut doc));
    assert!(doc.dom.take_intersection_records(observer).is_empty());
}

#[test]
fn disconnecting_clears_targets_but_keeps_the_queue() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let first = doc.el(doc.root, "view.cell");
    let second = doc.el(doc.root, "view.cell");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, first);
    doc.dom.observe_intersection(observer, second);
    doc.dom.render();
    assert!(update(&mut doc));

    doc.dom.disconnect_intersection_observer(observer);
    assert!(
        doc.dom.has_pending_intersection_notifications(),
        "entries queued before the disconnect stay queued"
    );
    assert_eq!(
        records(&mut doc, observer),
        vec![(first, true, 1.0), (second, true, 1.0)]
    );
    doc.set_inline(first, "transform: translateY(1000px)");
    doc.dom.render();
    assert!(!update(&mut doc), "no target is registered");
    assert!(doc.dom.take_intersection_records(observer).is_empty());

    // Still alive, and a target observed again starts from scratch.
    doc.dom.observe_intersection(observer, first);
    assert!(update(&mut doc));
    assert_eq!(records(&mut doc, observer), vec![(first, false, 0.0)]);
}

#[test]
fn taking_records_empties_the_queue() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let cell = doc.el(doc.root, "view.cell");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, cell);
    doc.dom.render();
    update(&mut doc);
    assert!(doc.dom.has_pending_intersection_notifications());
    assert_eq!(records(&mut doc, observer), vec![(cell, true, 1.0)]);
    assert!(doc.dom.take_intersection_records(observer).is_empty());
    assert!(!doc.dom.has_pending_intersection_notifications());
    assert!(doc.dom.take_intersection_notifications().is_empty());
}

#[test]
fn notifications_come_in_creation_order_and_drain() {
    let (mut doc, _) = components(".cell { width: 100px; height: 100px; }");
    let probe = doc.el(doc.root, "x-probe.cell");
    let cell = doc.el(doc.root, "view.cell");
    let first = host(&mut doc, None, &[]);
    let second = element_owned(&mut doc, probe);
    let idle = host(&mut doc, None, &[]);
    for observer in [second, first] {
        doc.dom.observe_intersection(observer, cell);
    }
    doc.dom.render();
    assert!(update(&mut doc));

    let notifications = doc.dom.take_intersection_notifications();
    let summary: Vec<_> = notifications
        .iter()
        .map(|notification| {
            (
                notification.observer,
                notification.owner,
                states(&notification.entries),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                first,
                IntersectionObserverOwner::Host,
                vec![(cell, true, 1.0)]
            ),
            (
                second,
                IntersectionObserverOwner::Element(probe),
                vec![(cell, true, 1.0)]
            ),
        ],
        "creation order, and an observer with nothing queued is left out",
    );
    assert!(doc.dom.take_intersection_notifications().is_empty());
    assert!(doc.dom.take_intersection_records(idle).is_empty());
}

#[test]
fn an_elements_observers_are_listed_in_creation_order() {
    let (mut doc, _) = components(".cell { width: 100px; height: 100px; }");
    let probe = doc.el(doc.root, "x-probe.cell");
    let other = doc.el(doc.root, "x-probe.cell");
    let first = element_owned(&mut doc, probe);
    host(&mut doc, None, &[]);
    element_owned(&mut doc, other);
    let second = element_owned(&mut doc, probe);
    assert_eq!(
        doc.dom
            .intersection_observers_owned_by(probe)
            .collect::<Vec<_>>(),
        vec![first, second]
    );
    assert!(first < second, "ids are monotone");
    assert_eq!(
        doc.dom
            .intersection_observers_owned_by(doc.root)
            .collect::<Vec<_>>(),
        Vec::new()
    );
}

// --- Delivery to an element owner -------------------------------------------

/// The hook runs in its own reaction scope: the child it appends is
/// connected before the delivery returns.
#[test]
fn delivery_runs_the_elements_hook_in_its_own_reaction_scope() {
    let (mut doc, log) = components(".cell { width: 100px; height: 100px; }");
    let probe = doc.el(doc.root, "x-probe.cell");
    let observer = element_owned(&mut doc, probe);
    doc.dom.observe_intersection(observer, probe);
    doc.dom.render();
    assert!(update(&mut doc));
    log.borrow_mut().clear();

    for notification in doc.dom.take_intersection_notifications() {
        let IntersectionObserverOwner::Element(element) = notification.owner else {
            panic!("only the probe's observer exists");
        };
        doc.dom.deliver_intersections_to_element(
            element,
            notification.observer,
            notification.entries,
        );
    }
    let child = doc
        .dom
        .get(probe)
        .and_then(dom::Node::last_child)
        .map(dom::Node::id)
        .expect("the hook appended a child");
    assert_eq!(
        *log.borrow(),
        vec![
            format!("observer {} at {probe}: {probe}:true", observer.get()),
            format!("connected {child}"),
            "hook returned".to_owned(),
        ]
    );
}

#[test]
fn delivery_to_a_freed_or_undefined_element_is_dropped() {
    let (mut doc, log) = components(".cell { width: 100px; height: 100px; }");
    let probe = doc.el(doc.root, "x-probe.cell");
    let plain = doc.el(doc.root, "view.cell");
    let observer = element_owned(&mut doc, probe);
    doc.dom.observe_intersection(observer, plain);
    doc.dom.render();
    update(&mut doc);
    let entries = doc.dom.take_intersection_records(observer);
    assert_eq!(entries.len(), 1);
    log.borrow_mut().clear();

    doc.dom
        .deliver_intersections_to_element(plain, observer, entries.clone());
    doc.dom.drop_element(probe);
    doc.dom
        .deliver_intersections_to_element(probe, observer, entries);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
}

// --- Node lifetime ----------------------------------------------------------

#[test]
fn a_freed_target_leaves_its_registration_and_queue() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let kept = doc.el(doc.root, "view.cell");
    let freed = doc.el(doc.root, "view.cell");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, kept);
    doc.dom.observe_intersection(observer, freed);
    doc.dom.render();
    assert!(update(&mut doc));

    doc.dom.drop_element(freed);
    assert_eq!(
        records(&mut doc, observer),
        vec![(kept, true, 1.0)],
        "no entry names a freed node",
    );
    doc.dom.render();
    assert!(!update(&mut doc), "and it is observed no more");
}

#[test]
fn a_freed_owner_takes_its_observers_while_a_host_observer_stays() {
    let (mut doc, _) = components(".cell { width: 100px; height: 100px; }");
    let cell = doc.el(doc.root, "view.cell");
    let probe = doc.el(doc.root, "x-probe.cell");
    let owned = element_owned(&mut doc, probe);
    let hosted = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(owned, cell);
    doc.dom.observe_intersection(hosted, cell);
    doc.dom.render();
    assert!(update(&mut doc));

    doc.dom.drop_element(probe);
    assert_eq!(doc.dom.intersection_observers_owned_by(probe).count(), 0);
    let notifications = doc.dom.take_intersection_notifications();
    assert_eq!(
        notifications.len(),
        1,
        "the owned observer went, queue and all"
    );
    assert_eq!(notifications[0].observer, hosted);

    doc.set_inline(cell, "transform: translateY(1000px)");
    doc.dom.render();
    assert!(update(&mut doc));
    assert_eq!(records(&mut doc, hosted), vec![(cell, false, 0.0)]);

    doc.dom.drop_intersection_observer(hosted);
    doc.set_inline(cell, "transform: none");
    doc.dom.render();
    assert!(!update(&mut doc), "no observer is left");
}

/// A freed element root leaves its targets one leave entry, with no render
/// needed to say so, and then nothing.
#[test]
fn a_freed_root_reports_its_targets_leaving_once() {
    let mut doc = page(
        ".root { overflow: hidden; width: 200px; height: 200px; }
         .cell { width: 100px; height: 100px; }",
    );
    let root = doc.el(doc.root, "view.root");
    let target = doc.el(root, "view.cell");
    let observer = host(&mut doc, Some(root), &[]);
    doc.dom.observe_intersection(observer, target);
    doc.dom.render();
    update(&mut doc);
    assert_eq!(records(&mut doc, observer), vec![(target, true, 1.0)]);

    doc.dom.drop_element(root);
    assert!(update(&mut doc));
    let entries = doc.dom.take_intersection_records(observer);
    assert_eq!(states(&entries), vec![(target, false, 0.0)]);
    assert_eq!(tuple(entries[0].root_bounds), (0.0, 0.0, 0.0, 0.0));

    doc.dom.render();
    assert!(!update(&mut doc));
    doc.el(doc.root, "view.cell");
    doc.dom.render();
    assert!(!update(&mut doc));
}

#[test]
fn a_shrunk_viewport_leaves_targets_behind() {
    let mut doc = page(
        "page { flex-direction: column; width: 800px; height: 600px; }
         .spacer { width: 10px; height: 450px; }
         .cell { width: 100px; height: 100px; }",
    );
    doc.el(doc.root, "view.spacer");
    let cell = doc.el(doc.root, "view.cell");
    let observer = host(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, cell);
    doc.dom.render();
    update(&mut doc);
    assert_eq!(records(&mut doc, observer), vec![(cell, true, 1.0)]);

    doc.dom.set_viewport(800.0, 400.0);
    assert!(doc.dom.render());
    assert!(update(&mut doc));
    let entries = doc.dom.take_intersection_records(observer);
    assert_eq!(states(&entries), vec![(cell, false, 0.0)]);
    assert_eq!(tuple(entries[0].root_bounds), (0.0, 0.0, 800.0, 400.0));
}

// --- Let it crash -----------------------------------------------------------

#[test]
#[should_panic(expected = "finite and in [0, 1]")]
fn a_threshold_out_of_range_panics() {
    let mut doc = page("");
    host(&mut doc, None, &[0.5, 1.5]);
}

#[test]
#[should_panic(expected = "constructed custom element")]
fn an_element_owner_must_be_a_constructed_component() {
    let mut doc = page("");
    let plain = doc.el(doc.root, "view");
    element_owned(&mut doc, plain);
}

#[test]
#[should_panic(expected = "names no live intersection observer")]
fn a_dropped_observer_names_nothing() {
    let mut doc = page("");
    let observer = host(&mut doc, None, &[]);
    doc.dom.drop_intersection_observer(observer);
    let _ = doc.dom.take_intersection_records(observer);
}
