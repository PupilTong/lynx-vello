//! Intersection observers (`dom::visual::intersection`) through a real
//! document: the registry, the spec's "update intersection observations"
//! steps (§3.2.10), the "notify intersection observers" loop (§3.2.5) over
//! boxed handlers, and node lifetime.
//!
//! Every handler here is a test struct implementing
//! `IntersectionEventHandler<()>`, several types in one document: a
//! [`Recorder`] that only logs, and handlers that log and then mutate the
//! tree, drop an observer, create one, or run the loop again. Observers whose
//! handler only logs are also read through `take_intersection_records`; an
//! engine component's deliver through `ElementHandler` to a recording
//! component.

#![allow(clippy::float_cmp)]

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use common::{Doc, device};
use dom::{
    CustomElement, Document, ElementHandler, IntersectionEventHandler, IntersectionObserverEntry,
    IntersectionObserverId, IntersectionObserverRoot, NodeId, Rect, RootMargin, Vector2D,
};

/// Every box is a flex container, and a `view` a non-shrinking one, so the
/// sizes a case writes are the sizes it gets.
const BASE: &str = "page { display: flex; } view { display: flex; flex-shrink: 0; }";

/// The time every update in these cases runs with.
const TIME: f64 = 12.5;

type Log = Rc<RefCell<Vec<String>>>;

/// What a handler writes for one notification:
/// `observer N: target:intersecting,…`.
fn heard(observer: IntersectionObserverId, entries: &[IntersectionObserverEntry]) -> String {
    let targets: Vec<String> = entries
        .iter()
        .map(|entry| format!("{}:{}", entry.target, entry.is_intersecting))
        .collect();
    format!("observer {}: {}", observer.get(), targets.join(","))
}

/// What every test handler does first: checks that its own handler is out of
/// its observer while it runs, and logs what it heard.
fn hear(
    log: &Log,
    document: &Document<()>,
    observer: IntersectionObserverId,
    entries: &[IntersectionObserverEntry],
) {
    assert!(
        document.intersection_observer(observer).handler().is_none(),
        "a handler is out of its observer while its notification runs"
    );
    log.borrow_mut().push(heard(observer, entries));
}

/// Only logs; bound to `bound`, if any.
struct Recorder {
    log: Log,
    bound: Option<NodeId>,
}

impl IntersectionEventHandler<()> for Recorder {
    fn notify(
        &mut self,
        document: &mut Document<()>,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        hear(&self.log, document, observer, &entries);
    }

    fn bound_to(&self) -> Option<NodeId> {
        self.bound
    }
}

/// Logs, then appends a `view` to `parent`.
struct Appender {
    log: Log,
    parent: NodeId,
}

impl IntersectionEventHandler<()> for Appender {
    fn notify(
        &mut self,
        document: &mut Document<()>,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        hear(&self.log, document, observer, &entries);
        let child = document.create_element("view", ());
        document.append_child(self.parent, child);
        self.log.borrow_mut().push(format!("appended {child}"));
    }
}

/// Logs, then drops the observer `victim` names — its own when `None`. The
/// victim is a cell so a test can name an observer created after this one.
/// `_alive` counts the droppers not yet dropped.
struct Dropper {
    log: Log,
    victim: Rc<Cell<Option<IntersectionObserverId>>>,
    _alive: Rc<()>,
}

impl IntersectionEventHandler<()> for Dropper {
    fn notify(
        &mut self,
        document: &mut Document<()>,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        hear(&self.log, document, observer, &entries);
        let victim = self.victim.get().unwrap_or(observer);
        document.drop_intersection_observer(victim);
        self.log
            .borrow_mut()
            .push(format!("dropped {}", victim.get()));
    }
}

/// Logs, then creates a [`Recorder`] observer of the viewport and observes
/// `target` through it.
struct Spawner {
    log: Log,
    target: NodeId,
}

impl IntersectionEventHandler<()> for Spawner {
    fn notify(
        &mut self,
        document: &mut Document<()>,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        hear(&self.log, document, observer, &entries);
        let spawned = document.create_intersection_observer(
            Box::new(Recorder {
                log: Rc::clone(&self.log),
                bound: None,
            }),
            None,
            RootMargin::ZERO,
            Vec::new(),
        );
        document.observe_intersection(spawned, self.target);
        self.log
            .borrow_mut()
            .push(format!("spawned {}", spawned.get()));
    }
}

/// Logs, then moves `target` out of the viewport, renders, updates, and runs
/// the notify loop again from inside its own notification.
struct Renotifier {
    log: Log,
    target: NodeId,
}

impl IntersectionEventHandler<()> for Renotifier {
    fn notify(
        &mut self,
        document: &mut Document<()>,
        observer: IntersectionObserverId,
        entries: Vec<IntersectionObserverEntry>,
    ) {
        hear(&self.log, document, observer, &entries);
        document.set_inline_style(self.target, "transform: translateY(1000px)");
        document.render();
        document.update_intersection_observations(TIME);
        document.notify_intersection_observers();
        self.log.borrow_mut().push("renotified".to_owned());
    }
}

/// Runs the notify loop and answers what the handlers logged, leaving `log`
/// empty.
fn notify(doc: &mut Doc, log: &Log) -> Vec<String> {
    doc.dom.notify_intersection_observers();
    std::mem::take(&mut *log.borrow_mut())
}

fn page(css: &str) -> Doc {
    Doc::with_css(&format!("{BASE}\n{css}"))
}

/// An observer whose [`Recorder`] logs into `log` and is bound to `bound`.
fn recording(
    doc: &mut Doc,
    log: &Log,
    bound: Option<NodeId>,
    root: Option<NodeId>,
) -> IntersectionObserverId {
    doc.dom.create_intersection_observer(
        Box::new(Recorder {
            log: Rc::clone(log),
            bound,
        }),
        root,
        RootMargin::ZERO,
        Vec::new(),
    )
}

/// An unbound observer whose [`Recorder`] logs where no one reads, for the
/// cases that read it through `take_intersection_records`.
fn unbound(doc: &mut Doc, root: Option<NodeId>, thresholds: &[f64]) -> IntersectionObserverId {
    doc.dom.create_intersection_observer(
        Box::new(Recorder {
            log: Log::default(),
            bound: None,
        }),
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
        self.log
            .borrow_mut()
            .push(format!("{} at {element}", heard(observer, &entries)));
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

/// An observer of the viewport delivering through an [`ElementHandler`] to
/// `element`'s component, and so bound to it.
fn element_owned(doc: &mut Doc, element: NodeId) -> IntersectionObserverId {
    doc.dom.create_intersection_observer(
        Box::new(ElementHandler(element)),
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
    let observer = unbound(&mut doc, None, &[]);
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
    let observer = unbound(&mut doc, None, &[]);
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
    let observer = unbound(&mut doc, None, &[1.0, 0.0, 0.5]);
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
    let observer = unbound(&mut doc, None, &[]);
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
    let observer = unbound(&mut doc, None, &[]);
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
    let observer = unbound(&mut doc, Some(root), &[]);
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
    let observer = unbound(&mut doc, None, &[]);
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
    let observer = unbound(&mut doc, None, &[]);
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
    let observer = unbound(&mut doc, None, &[]);
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
    let observer = unbound(&mut doc, None, &[]);
    doc.dom.observe_intersection(observer, cell);
    doc.dom.render();
    update(&mut doc);
    assert!(doc.dom.has_pending_intersection_notifications());
    assert_eq!(records(&mut doc, observer), vec![(cell, true, 1.0)]);
    assert!(doc.dom.take_intersection_records(observer).is_empty());
    assert!(!doc.dom.has_pending_intersection_notifications());
    let log = Log::default();
    assert!(
        notify(&mut doc, &log).is_empty(),
        "nothing is left to notify"
    );
}

#[test]
fn an_observer_reads_back_what_it_was_created_with() {
    let (mut doc, log) = components(".cell { width: 100px; height: 100px; }");
    let probe = doc.el(doc.root, "x-probe.cell");
    let root = doc.el(doc.root, "view.cell");
    let owned = element_owned(&mut doc, probe);
    let bound = recording(&mut doc, &log, Some(root), Some(root));
    let free = unbound(&mut doc, None, &[1.0, 0.0, 0.5]);
    assert!(owned < bound && bound < free, "ids are monotone");

    let observer = doc.dom.intersection_observer(owned);
    assert_eq!(observer.id(), owned);
    assert_eq!(observer.root(), IntersectionObserverRoot::Implicit);
    assert_eq!(observer.thresholds(), [0.0], "an empty list is [0]");
    assert_eq!(
        observer.bound_to(),
        Some(probe),
        "an ElementHandler's element"
    );
    assert!(observer.handler().is_some());

    let observer = doc.dom.intersection_observer(bound);
    assert_eq!(observer.root(), IntersectionObserverRoot::Element(root));
    assert_eq!(
        observer.bound_to(),
        Some(root),
        "whatever the handler names"
    );

    let observer = doc.dom.intersection_observer_mut(free);
    assert_eq!(observer.thresholds(), [0.0, 0.5, 1.0], "sorted");
    assert_eq!(observer.bound_to(), None);
    assert_eq!(
        observer
            .handler_mut()
            .and_then(|handler| handler.bound_to()),
        None
    );
}

// --- The notify loop --------------------------------------------------------

/// Two handler types in one document — a [`Recorder`] and an engine
/// component's [`ElementHandler`] — heard in creation order, not observe
/// order, with an observer that has nothing queued left out.
#[test]
fn notifications_come_in_creation_order_over_any_handler_types() {
    let (mut doc, log) = components(".cell { width: 100px; height: 100px; }");
    let probe = doc.el(doc.root, "x-probe.cell");
    let cell = doc.el(doc.root, "view.cell");
    let first = recording(&mut doc, &log, None, None);
    let second = element_owned(&mut doc, probe);
    let idle = recording(&mut doc, &log, None, None);
    let third = recording(&mut doc, &log, None, None);
    for observer in [third, second, first] {
        doc.dom.observe_intersection(observer, cell);
    }
    doc.dom.render();
    assert!(update(&mut doc));
    log.borrow_mut().clear();

    let heard = notify(&mut doc, &log);
    let child = doc
        .dom
        .get(probe)
        .and_then(dom::Node::last_child)
        .map(dom::Node::id)
        .expect("the hook appended a child");
    assert_eq!(
        heard,
        vec![
            format!("observer {}: {cell}:true", first.get()),
            format!("observer {}: {cell}:true at {probe}", second.get()),
            format!("connected {child}"),
            "hook returned".to_owned(),
            format!("observer {}: {cell}:true", third.get()),
        ],
    );
    assert!(!doc.dom.has_pending_intersection_notifications());
    assert!(notify(&mut doc, &log).is_empty(), "the queues drained");
    assert!(doc.dom.take_intersection_records(idle).is_empty());
    for observer in [first, second, third] {
        assert!(
            doc.dom.intersection_observer(observer).handler().is_some(),
            "every handler is back in its observer"
        );
    }
}

/// A handler that mutates the tree from inside its notification: the loop
/// goes on, and what it wrote is the next render's.
#[test]
fn a_handler_may_mutate_the_tree() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let log = Log::default();
    let cell = doc.el(doc.root, "view.cell");
    let appender = doc.dom.create_intersection_observer(
        Box::new(Appender {
            log: Rc::clone(&log),
            parent: doc.root,
        }),
        None,
        RootMargin::ZERO,
        Vec::new(),
    );
    let after = recording(&mut doc, &log, None, None);
    doc.dom.observe_intersection(appender, cell);
    doc.dom.observe_intersection(after, cell);
    doc.dom.render();
    assert!(update(&mut doc));

    let heard = notify(&mut doc, &log);
    let child = doc
        .dom
        .get(doc.root)
        .and_then(dom::Node::last_child)
        .map(dom::Node::id)
        .expect("the handler appended a child");
    assert_eq!(
        heard,
        vec![
            format!("observer {}: {cell}:true", appender.get()),
            format!("appended {child}"),
            format!("observer {}: {cell}:true", after.get()),
        ]
    );
    assert!(doc.dom.render(), "the append is the next render's");
}

/// [`ElementHandler`] runs the hook in its own reaction scope: the child it
/// appends is connected before the notification returns.
#[test]
fn an_element_handler_runs_the_hook_in_its_own_reaction_scope() {
    let (mut doc, log) = components(".cell { width: 100px; height: 100px; }");
    let probe = doc.el(doc.root, "x-probe.cell");
    let observer = element_owned(&mut doc, probe);
    doc.dom.observe_intersection(observer, probe);
    doc.dom.render();
    assert!(update(&mut doc));
    log.borrow_mut().clear();

    let heard = notify(&mut doc, &log);
    let child = doc
        .dom
        .get(probe)
        .and_then(dom::Node::last_child)
        .map(dom::Node::id)
        .expect("the hook appended a child");
    assert_eq!(
        heard,
        vec![
            format!("observer {}: {probe}:true at {probe}", observer.get()),
            format!("connected {child}"),
            "hook returned".to_owned(),
        ]
    );
}

/// An element that is not a constructed component hears nothing through the
/// loop, and neither does a freed one called directly — its free dropped the
/// observer, so only a direct call can name it.
#[test]
fn an_element_handler_drops_entries_for_a_freed_or_unconstructed_element() {
    let (mut doc, log) = components(".cell { width: 100px; height: 100px; }");
    let probe = doc.el(doc.root, "x-probe.cell");
    let plain = doc.el(doc.root, "view.cell");
    let observer = element_owned(&mut doc, plain);
    doc.dom.observe_intersection(observer, plain);
    doc.dom.render();
    assert!(update(&mut doc));
    log.borrow_mut().clear();

    assert!(notify(&mut doc, &log).is_empty());
    assert!(!doc.dom.has_pending_intersection_notifications());

    doc.set_inline(plain, "transform: translateY(1000px)");
    doc.dom.render();
    assert!(update(&mut doc));
    let entries = doc.dom.take_intersection_records(observer);
    assert_eq!(entries.len(), 1);
    doc.dom.drop_element(probe);
    ElementHandler(probe).notify(&mut doc.dom, observer, entries);
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
}

/// A handler drops its own observer, and another drops one later in the
/// list: the later one is skipped, its queue gone with it, the own one's
/// handler is dropped once its notification returns, and the loop panics on
/// neither.
#[test]
fn a_handler_may_drop_its_own_observer_or_a_later_one() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let log = Log::default();
    let alive = Rc::new(());
    let cell = doc.el(doc.root, "view.cell");
    let dropper = |doc: &mut Doc, victim: &Rc<Cell<Option<IntersectionObserverId>>>| {
        doc.dom.create_intersection_observer(
            Box::new(Dropper {
                log: Rc::clone(&log),
                victim: Rc::clone(victim),
                _alive: Rc::clone(&alive),
            }),
            None,
            RootMargin::ZERO,
            Vec::new(),
        )
    };
    let own = dropper(&mut doc, &Rc::default());
    let victim = Rc::default();
    let survivor = dropper(&mut doc, &victim);
    let later = recording(&mut doc, &log, None, None);
    victim.set(Some(later));
    for observer in [own, survivor, later] {
        doc.dom.observe_intersection(observer, cell);
    }
    doc.dom.render();
    assert!(update(&mut doc));
    assert_eq!(Rc::strong_count(&alive), 3);

    assert_eq!(
        notify(&mut doc, &log),
        vec![
            format!("observer {}: {cell}:true", own.get()),
            format!("dropped {}", own.get()),
            format!("observer {}: {cell}:true", survivor.get()),
            format!("dropped {}", later.get()),
        ]
    );
    assert_eq!(
        Rc::strong_count(&alive),
        2,
        "the self-dropped observer's handler went when it returned"
    );
    assert!(doc.dom.intersection_observer(survivor).handler().is_some());
    assert!(!doc.dom.has_pending_intersection_notifications());
    doc.set_inline(cell, "transform: translateY(1000px)");
    doc.dom.render();
    assert!(update(&mut doc), "the survivor still observes");
    assert_eq!(records(&mut doc, survivor), vec![(cell, false, 0.0)]);
}

/// A handler creates an observer and observes through it: the new one is
/// not in the loop that created it, and the next update reports to it.
#[test]
fn a_handler_may_create_and_observe() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let log = Log::default();
    let cell = doc.el(doc.root, "view.cell");
    let other = doc.el(doc.root, "view.cell");
    let spawner = doc.dom.create_intersection_observer(
        Box::new(Spawner {
            log: Rc::clone(&log),
            target: other,
        }),
        None,
        RootMargin::ZERO,
        Vec::new(),
    );
    doc.dom.observe_intersection(spawner, cell);
    doc.dom.render();
    assert!(update(&mut doc));

    let heard = notify(&mut doc, &log);
    assert_eq!(heard.len(), 2);
    assert_eq!(heard[0], format!("observer {}: {cell}:true", spawner.get()));
    let created = heard[1]
        .strip_prefix("spawned ")
        .expect("the handler created one");
    assert!(update(&mut doc), "the observe made the update stale");
    assert_eq!(
        notify(&mut doc, &log),
        vec![format!("observer {created}: {other}:true")]
    );
}

/// A loop run from inside a notification: the running observer's handler is
/// out, so what its own update queued waits for the next loop, while a later
/// observer is delivered by the inner loop and skipped by the outer one.
#[test]
fn a_loop_inside_a_notification_leaves_the_running_observers_entries_queued() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let log = Log::default();
    let cell = doc.el(doc.root, "view.cell");
    let renotifier = doc.dom.create_intersection_observer(
        Box::new(Renotifier {
            log: Rc::clone(&log),
            target: cell,
        }),
        None,
        RootMargin::ZERO,
        Vec::new(),
    );
    let later = recording(&mut doc, &log, None, None);
    doc.dom.observe_intersection(renotifier, cell);
    doc.dom.observe_intersection(later, cell);
    doc.dom.render();
    assert!(update(&mut doc));

    assert_eq!(
        notify(&mut doc, &log),
        vec![
            format!("observer {}: {cell}:true", renotifier.get()),
            format!("observer {}: {cell}:true,{cell}:false", later.get()),
            "renotified".to_owned(),
        ]
    );
    assert!(doc.dom.has_pending_intersection_notifications());
    assert_eq!(
        notify(&mut doc, &log),
        vec![
            format!("observer {}: {cell}:false", renotifier.get()),
            "renotified".to_owned(),
        ]
    );
    assert!(!doc.dom.has_pending_intersection_notifications());
}

// --- Node lifetime ----------------------------------------------------------

#[test]
fn a_freed_target_leaves_its_registration_and_queue() {
    let mut doc = page(".cell { width: 100px; height: 100px; }");
    let kept = doc.el(doc.root, "view.cell");
    let freed = doc.el(doc.root, "view.cell");
    let observer = unbound(&mut doc, None, &[]);
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

/// Freeing a node drops the observers bound to it — an [`ElementHandler`]'s
/// and a [`Recorder`] bound to it alike, queues and all — while an unbound
/// one lives until `drop_intersection_observer`.
#[test]
fn a_freed_node_takes_its_bound_observers_while_an_unbound_one_stays() {
    let (mut doc, log) = components(".cell { width: 100px; height: 100px; }");
    let cell = doc.el(doc.root, "view.cell");
    let probe = doc.el(doc.root, "x-probe.cell");
    let owned = element_owned(&mut doc, probe);
    let bound = recording(&mut doc, &log, Some(probe), None);
    let free = recording(&mut doc, &log, None, None);
    for observer in [owned, bound, free] {
        doc.dom.observe_intersection(observer, cell);
    }
    doc.dom.render();
    assert!(update(&mut doc));
    log.borrow_mut().clear();

    doc.dom.drop_element(probe);
    assert_eq!(
        notify(&mut doc, &log),
        vec![format!("observer {}: {cell}:true", free.get())],
        "the bound observers went, queues and all"
    );

    doc.set_inline(cell, "transform: translateY(1000px)");
    doc.dom.render();
    assert!(update(&mut doc));
    assert_eq!(records(&mut doc, free), vec![(cell, false, 0.0)]);
    assert!(
        !doc.dom.has_pending_intersection_notifications(),
        "no bound observer is left to queue"
    );

    doc.dom.drop_intersection_observer(free);
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
    let observer = unbound(&mut doc, Some(root), &[]);
    doc.dom.observe_intersection(observer, target);
    doc.dom.render();
    update(&mut doc);
    assert_eq!(records(&mut doc, observer), vec![(target, true, 1.0)]);

    doc.dom.drop_element(root);
    assert_eq!(
        doc.dom.intersection_observer(observer).root(),
        IntersectionObserverRoot::Freed
    );
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
    let observer = unbound(&mut doc, None, &[]);
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
    unbound(&mut doc, None, &[0.5, 1.5]);
}

#[test]
#[should_panic(expected = "which must be a live node")]
fn a_handler_must_be_bound_to_a_live_node() {
    let mut doc = page("");
    let freed = doc.el(doc.root, "view");
    doc.dom.drop_element(freed);
    element_owned(&mut doc, freed);
}

#[test]
#[should_panic(expected = "names no live intersection observer")]
fn a_dropped_observer_names_nothing() {
    let mut doc = page("");
    let observer = unbound(&mut doc, None, &[]);
    doc.dom.drop_intersection_observer(observer);
    let _ = doc.dom.intersection_observer(observer);
}
