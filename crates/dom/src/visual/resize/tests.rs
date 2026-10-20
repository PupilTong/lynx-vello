//! Resize observers through a real document: the registry, the gather step
//! ([`Document::update_resize_observations`]), the broadcast loop
//! ([`Document::notify_resize_observers`]) over boxed handlers, the stale
//! bit, and node lifetime.

use std::cell::RefCell;
use std::rc::Rc;

use euclid::default::Vector2D;
use stylo::queries::values::PrefersColorScheme;

use super::*;
use crate::test_common::{Doc, device_with};

/// Every box is a flex container, and a `view` a non-shrinking one, so the
/// sizes a case writes are the sizes it gets. `.cell` is a 100 × 50 border
/// box with 10px/5px padding and a 2px border, so its content box is
/// 76 × 36 at (10, 5).
const BASE: &str = "page { display: flex; flex-direction: column; } \
                    view { display: flex; flex-shrink: 0; } \
                    .cell { box-sizing: border-box; width: 100px; height: 50px; \
                            padding: 5px 10px; border: 2px solid black; }";

type Log = Rc<RefCell<Vec<(ResizeObserverId, Vec<ResizeObserverEntry>)>>>;

/// Logs what it heard, checking that its own handler is out of its observer
/// while it runs; bound to `bound`, if any.
struct Recorder {
    log: Log,
    bound: Option<NodeId>,
}

impl ResizeEventHandler<()> for Recorder {
    fn notify(
        &mut self,
        document: &mut Document<()>,
        observer: ResizeObserverId,
        entries: Vec<ResizeObserverEntry>,
    ) {
        assert!(
            document
                .resize_observer(observer)
                .is_some_and(|observer| observer.handler().is_none()),
            "a handler is out of its observer while its notification runs"
        );
        self.log.borrow_mut().push((observer, entries));
    }

    fn bound_to(&self) -> Option<NodeId> {
        self.bound
    }
}

/// Logs, then drops its own observer.
struct SelfDropper {
    log: Log,
}

impl ResizeEventHandler<()> for SelfDropper {
    fn notify(
        &mut self,
        document: &mut Document<()>,
        observer: ResizeObserverId,
        entries: Vec<ResizeObserverEntry>,
    ) {
        self.log.borrow_mut().push((observer, entries));
        document.drop_resize_observer(observer);
    }
}

/// Logs, resizes `target`, lays out, updates, and runs the loop again from
/// inside its own notification.
struct Renotifier {
    log: Log,
    target: NodeId,
}

impl ResizeEventHandler<()> for Renotifier {
    fn notify(
        &mut self,
        document: &mut Document<()>,
        observer: ResizeObserverId,
        entries: Vec<ResizeObserverEntry>,
    ) {
        self.log.borrow_mut().push((observer, entries));
        document.set_inline_style(self.target, "width: 300px");
        document.layout();
        assert!(document.update_resize_observations());
        document.notify_resize_observers();
    }
}

fn page(css: &str) -> Doc {
    let mut doc = Doc::with_css(&format!("{BASE}\n{css}"));
    // A first pass, so every case starts from a document that has laid out.
    doc.flush();
    doc
}

fn recording(doc: &mut Doc, log: &Log, bound: Option<NodeId>) -> ResizeObserverId {
    doc.dom.create_resize_observer(Box::new(Recorder {
        log: Rc::clone(log),
        bound,
    }))
}

/// Each queued entry as `(target, border-box width, border-box height)`.
fn queued(doc: &Doc, observer: ResizeObserverId) -> Vec<(NodeId, f32, f32)> {
    doc.dom
        .resize_observer(observer)
        .expect("observer is live")
        .queued()
        .iter()
        .map(|entry| {
            (
                entry.target,
                entry.border_box_size.width,
                entry.border_box_size.height,
            )
        })
        .collect()
}

fn zero_entry(target: NodeId) -> ResizeObserverEntry {
    ResizeObserverEntry {
        target,
        content_rect: Rect::zero(),
        border_box_size: Size::new(0.0, 0.0),
        content_box_size: Size::new(0.0, 0.0),
        device_pixel_content_box_size: Size::new(0.0, 0.0),
    }
}

#[test]
fn the_first_update_after_observe_reports_each_target_once_a_zero_sized_one_included() {
    let mut doc = page(".empty { width: 0; height: 0; }");
    let cell = doc.el(doc.root, "view.cell");
    let empty = doc.el(doc.root, "view.empty");
    let log = Log::default();
    let observer = recording(&mut doc, &log, None);
    doc.dom
        .observe_resize(observer, cell, ObservedBox::ContentBox);
    doc.dom
        .observe_resize(observer, empty, ObservedBox::ContentBox);
    doc.flush();

    assert!(doc.dom.update_resize_observations());
    assert_eq!(
        queued(&doc, observer),
        vec![(cell, 100.0, 50.0), (empty, 0.0, 0.0)],
        "the initial lastReportedSizes is -1 × -1, which no box has"
    );
    assert_eq!(
        doc.dom.resize_observer(observer).unwrap().queued()[1],
        zero_entry(empty)
    );
    assert!(doc.dom.has_pending_resize_notifications());
    doc.dom.notify_resize_observers();
    assert_eq!(log.borrow().len(), 1, "one notification for the observer");
    assert_eq!(log.borrow()[0].1.len(), 2);
    assert!(!doc.dom.has_pending_resize_notifications());

    doc.flush();
    assert!(
        !doc.dom.update_resize_observations(),
        "each is reported once"
    );
}

#[test]
fn a_width_change_queues_one_entry_with_every_box_size() {
    let mut doc = Doc::with_device(device_with(800.0, 600.0, 2.0, PrefersColorScheme::Light));
    doc.add_css(BASE);
    let cell = doc.el(doc.root, "view.cell");
    let observer = recording(&mut doc, &Log::default(), None);
    doc.dom
        .observe_resize(observer, cell, ObservedBox::ContentBox);
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    assert_eq!(
        doc.dom.take_resize_records(observer),
        vec![ResizeObserverEntry {
            target: cell,
            content_rect: Rect::new(Point2D::new(10.0, 5.0), Size2D::new(76.0, 36.0)),
            border_box_size: Size::new(100.0, 50.0),
            content_box_size: Size::new(76.0, 36.0),
            device_pixel_content_box_size: Size::new(152.0, 72.0),
        }]
    );

    doc.set_inline(cell, "width: 200px");
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    assert_eq!(
        doc.dom.take_resize_records(observer),
        vec![ResizeObserverEntry {
            target: cell,
            content_rect: Rect::new(Point2D::new(10.0, 5.0), Size2D::new(176.0, 36.0)),
            border_box_size: Size::new(200.0, 50.0),
            content_box_size: Size::new(176.0, 36.0),
            device_pixel_content_box_size: Size::new(352.0, 72.0),
        }]
    );
}

/// At a ratio of 1.5, an 11px box ends at 16.5 device px, which the rounding
/// tail rounds up to 17: the layout holds 17 / 1.5 CSS px, and the
/// device-pixel size is the whole 17.
#[test]
fn device_pixel_sizes_are_whole_device_pixels() {
    let mut doc = Doc::with_device(device_with(800.0, 600.0, 1.5, PrefersColorScheme::Light));
    doc.add_css(&format!("{BASE}\n.odd {{ width: 11px; height: 3px; }}"));
    let odd = doc.el(doc.root, "view.odd");
    let observer = recording(&mut doc, &Log::default(), None);
    doc.dom
        .observe_resize(observer, odd, ObservedBox::DevicePixelContentBox);
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    let entries = doc.dom.take_resize_records(observer);
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].device_pixel_content_box_size,
        Size::new(17.0, 5.0)
    );
    assert!((entries[0].content_box_size.width - 17.0 / 1.5).abs() < 1e-4);
}

#[test]
fn a_layout_that_moves_no_observed_size_queues_nothing() {
    let mut doc = page(".tall { height: 10px; }");
    let above = doc.el(doc.root, "view.tall");
    let cell = doc.el(doc.root, "view.cell");
    let observer = recording(&mut doc, &Log::default(), None);
    for box_kind in [ObservedBox::BorderBox, ObservedBox::ContentBox] {
        doc.dom.observe_resize(observer, cell, box_kind);
    }
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    let _ = doc.dom.take_resize_records(observer);

    // The cell moves down 30px and keeps its size.
    doc.set_inline(above, "height: 40px");
    doc.flush();
    assert!(doc.dom.resizes.stale, "a pass that ran sets the bit");
    assert!(!doc.dom.update_resize_observations());
    assert!(!doc.dom.resizes.stale, "an update that ran clears it");

    doc.flush();
    assert!(
        !doc.dom.resizes.stale,
        "a layout() with nothing to lay out runs no pass and sets nothing"
    );
}

#[test]
fn a_target_that_stops_being_rendered_reports_zero_once() {
    let mut doc = page(".gone { display: none; }");
    let cell = doc.el(doc.root, "view.cell");
    let observer = recording(&mut doc, &Log::default(), None);
    doc.dom
        .observe_resize(observer, cell, ObservedBox::BorderBox);
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    let _ = doc.dom.take_resize_records(observer);

    doc.add_class(cell, "gone");
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    assert_eq!(
        doc.dom.take_resize_records(observer),
        vec![zero_entry(cell)]
    );
    doc.set_inline(doc.root, "padding: 1px");
    doc.flush();
    assert!(!doc.dom.update_resize_observations(), "and only once");

    doc.remove_class(cell, "gone");
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    assert_eq!(queued(&doc, observer), vec![(cell, 100.0, 50.0)]);
}

/// Chromium's locked subtree: a target whose contents an ancestor skips is
/// neither compared nor updated, so it reports nothing while skipped and,
/// once laid out again, only a size that differs from the one it last
/// reported. The skipping element's own box is still observed.
#[test]
fn a_target_a_skipping_ancestor_hides_is_left_alone() {
    let mut doc = page(
        ".holder { width: 120px; height: 60px; } \
         .skip { content-visibility: hidden; } .gone { display: none; }",
    );
    let outer = doc.el(doc.root, "view");
    let holder = doc.el(outer, "view.holder");
    let inner = doc.el(holder, "view.cell");
    let observer = recording(&mut doc, &Log::default(), None);
    for target in [holder, inner] {
        doc.dom
            .observe_resize(observer, target, ObservedBox::BorderBox);
    }
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    let _ = doc.dom.take_resize_records(observer);

    doc.add_class(holder, "skip");
    doc.flush();
    assert_eq!(
        doc.dom.observed_box_state(inner),
        BoxState::Skipped,
        "an ancestor skips it"
    );
    assert!(
        !doc.dom.update_resize_observations(),
        "no entry while skipped, and the skipping box kept its size"
    );

    doc.set_inline(holder, "width: 130px");
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    assert_eq!(
        doc.dom
            .take_resize_records(observer)
            .iter()
            .map(|entry| entry.target)
            .collect::<Vec<_>>(),
        vec![holder],
        "the skipping element's own box is observed"
    );

    doc.remove_class(holder, "skip");
    doc.flush();
    assert!(
        !doc.dom.update_resize_observations(),
        "back at the size it last reported: nothing"
    );

    doc.add_class(holder, "skip");
    doc.set_inline(inner, "width: 150px");
    doc.flush();
    let late = recording(&mut doc, &Log::default(), None);
    doc.dom.observe_resize(late, inner, ObservedBox::BorderBox);
    assert!(
        !doc.dom.update_resize_observations(),
        "a resize under a skipping ancestor waits, as does a new observation"
    );
    doc.remove_class(holder, "skip");
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    assert_eq!(queued(&doc, observer), vec![(inner, 150.0, 50.0)]);
    assert_eq!(queued(&doc, late), vec![(inner, 150.0, 50.0)]);
    let _ = doc.dom.take_resize_records(observer);

    // A `display: none` ancestor above the skipping one: the skipping one
    // has no box, so it skips nothing, and the target is not rendered.
    doc.add_class(holder, "skip");
    doc.add_class(outer, "gone");
    doc.flush();
    assert_eq!(doc.dom.observed_box_state(inner), BoxState::NotRendered);
    assert!(doc.dom.update_resize_observations());
    assert_eq!(
        doc.dom.take_resize_records(observer),
        vec![zero_entry(holder), zero_entry(inner)]
    );
}

/// An unlinked node keeps the layout it last had; only the walk up its
/// containing-block chain tells it is no longer in the document.
#[test]
fn an_unlinked_live_target_reports_zero() {
    let mut doc = page("");
    let cell = doc.el(doc.root, "view.cell");
    let observer = recording(&mut doc, &Log::default(), None);
    doc.dom
        .observe_resize(observer, cell, ObservedBox::ContentBox);
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    let _ = doc.dom.take_resize_records(observer);

    doc.dom.remove_element(cell);
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    assert_eq!(
        doc.dom.take_resize_records(observer),
        vec![zero_entry(cell)]
    );
    assert_eq!(
        doc.dom
            .resize_observer(observer)
            .unwrap()
            .observed_box(cell),
        Some(ObservedBox::ContentBox),
        "a live node stays observed"
    );
}

#[test]
fn a_scroll_resizes_nothing_and_leaves_the_bit_clear() {
    let mut doc = page(
        ".scroller { overflow: scroll; width: 100px; height: 100px; } \
         .content { width: 100px; height: 400px; }",
    );
    let scroller = doc.el(doc.root, "view.scroller");
    let content = doc.el(scroller, "view.content");
    let observer = recording(&mut doc, &Log::default(), None);
    for target in [scroller, content] {
        doc.dom
            .observe_resize(observer, target, ObservedBox::BorderBox);
    }
    doc.dom.render();
    assert!(doc.dom.update_resize_observations());
    let _ = doc.dom.take_resize_records(observer);

    assert_eq!(
        doc.dom.scroll_to(scroller, Vector2D::new(0.0, 120.0)),
        Vector2D::new(0.0, 120.0)
    );
    doc.dom.render();
    assert!(
        !doc.dom.resizes.stale,
        "neither the scroll nor a render that laid nothing out sets the bit"
    );
    assert!(!doc.dom.update_resize_observations());
}

#[test]
fn no_update_runs_before_the_first_layout() {
    let mut doc = Doc::with_css(BASE);
    let cell = doc.el(doc.root, "view.cell");
    let observer = recording(&mut doc, &Log::default(), None);
    doc.dom
        .observe_resize(observer, cell, ObservedBox::ContentBox);
    assert!(!doc.dom.update_resize_observations());
    assert!(doc.dom.resizes.stale, "the bit waits for the first layout");

    doc.flush();
    assert!(doc.dom.update_resize_observations());
    assert_eq!(
        queued(&doc, observer),
        vec![(cell, 100.0, 50.0)],
        "no 0 × 0 entry from before the layout"
    );
}

#[test]
fn reobserving_through_another_box_reports_again_and_the_same_box_is_a_no_op() {
    let mut doc = page("");
    let cell = doc.el(doc.root, "view.cell");
    let other = doc.el(doc.root, "view.cell");
    let observer = recording(&mut doc, &Log::default(), None);
    doc.dom
        .observe_resize(observer, cell, ObservedBox::ContentBox);
    doc.dom
        .observe_resize(observer, other, ObservedBox::ContentBox);
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    let _ = doc.dom.take_resize_records(observer);

    doc.dom
        .observe_resize(observer, cell, ObservedBox::ContentBox);
    assert!(
        !doc.dom.update_resize_observations(),
        "the same box keeps its registration"
    );

    doc.dom
        .observe_resize(observer, cell, ObservedBox::BorderBox);
    let resized = doc.dom.resize_observer(observer).unwrap();
    assert_eq!(resized.observed_box(cell), Some(ObservedBox::BorderBox));
    assert_eq!(
        resized
            .targets
            .iter()
            .map(|registration| registration.target)
            .collect::<Vec<_>>(),
        vec![other, cell],
        "a new registration, at the end of the observe order"
    );
    assert!(doc.dom.update_resize_observations());
    assert_eq!(queued(&doc, observer), vec![(cell, 100.0, 50.0)]);
}

#[test]
fn freeing_an_observed_node_drops_its_registrations_entries_and_bound_observers() {
    let mut doc = page("");
    let kept = doc.el(doc.root, "view.cell");
    let freed = doc.el(doc.root, "view.cell");
    let log = Log::default();
    let unbound = recording(&mut doc, &log, None);
    let bound = recording(&mut doc, &log, Some(freed));
    for observer in [unbound, bound] {
        doc.dom
            .observe_resize(observer, kept, ObservedBox::ContentBox);
        doc.dom
            .observe_resize(observer, freed, ObservedBox::ContentBox);
    }
    doc.flush();
    assert!(doc.dom.update_resize_observations());

    doc.dom.drop_element(freed);
    assert!(
        doc.dom.resize_observer(bound).is_none(),
        "an observer bound to the node goes with it"
    );
    let observer = doc.dom.resize_observer(unbound).unwrap();
    assert_eq!(observer.observed_box(freed), None);
    assert_eq!(observer.observed_box(kept), Some(ObservedBox::ContentBox));
    assert_eq!(
        queued(&doc, unbound),
        vec![(kept, 100.0, 50.0)],
        "no entry names a freed node"
    );
    doc.dom.notify_resize_observers();
    assert_eq!(log.borrow().len(), 1);
    assert_eq!(log.borrow()[0].0, unbound);
}

#[test]
fn dropping_the_observer_from_its_own_notification_is_safe() {
    let mut doc = page("");
    let cell = doc.el(doc.root, "view.cell");
    let log = Log::default();
    let dropper = doc.dom.create_resize_observer(Box::new(SelfDropper {
        log: Rc::clone(&log),
    }));
    let after = recording(&mut doc, &log, None);
    for observer in [dropper, after] {
        doc.dom
            .observe_resize(observer, cell, ObservedBox::ContentBox);
    }
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    doc.dom.notify_resize_observers();

    assert!(doc.dom.resize_observer(dropper).is_none());
    assert_eq!(
        log.borrow()
            .iter()
            .map(|(observer, _)| *observer)
            .collect::<Vec<_>>(),
        vec![dropper, after],
        "the loop goes on to the next observer"
    );
    let reissued = recording(&mut doc, &log, None);
    assert!(reissued > after, "a dropped id is never reissued");
}

#[test]
fn a_loop_run_from_inside_a_notification_leaves_its_own_entries_queued() {
    let mut doc = page("");
    let cell = doc.el(doc.root, "view.cell");
    let log = Log::default();
    let observer = doc.dom.create_resize_observer(Box::new(Renotifier {
        log: Rc::clone(&log),
        target: cell,
    }));
    doc.dom
        .observe_resize(observer, cell, ObservedBox::BorderBox);
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    doc.dom.notify_resize_observers();

    assert_eq!(log.borrow().len(), 1, "the inner loop skipped the observer");
    assert_eq!(queued(&doc, observer), vec![(cell, 300.0, 50.0)]);
    assert!(
        doc.dom
            .resize_observer(observer)
            .unwrap()
            .handler()
            .is_some(),
        "its handler is back"
    );
}

/// `unobserve` leaves the queue, as the spec's steps leave
/// `[[activeTargets]]`; `disconnect` clears both lists.
#[test]
fn unobserve_keeps_the_queue_and_disconnect_drops_it() {
    let mut doc = page("");
    let cell = doc.el(doc.root, "view.cell");
    let observer = recording(&mut doc, &Log::default(), None);
    doc.dom
        .observe_resize(observer, cell, ObservedBox::ContentBox);
    doc.flush();
    assert!(doc.dom.update_resize_observations());

    doc.dom.unobserve_resize(observer, cell);
    assert_eq!(
        doc.dom
            .resize_observer(observer)
            .unwrap()
            .observed_box(cell),
        None
    );
    assert_eq!(doc.dom.take_resize_records(observer).len(), 1);

    doc.dom
        .observe_resize(observer, cell, ObservedBox::ContentBox);
    doc.flush();
    assert!(doc.dom.update_resize_observations());
    doc.dom.disconnect_resize_observer(observer);
    assert!(doc.dom.take_resize_records(observer).is_empty());
    assert!(
        doc.dom.resize_observer(observer).is_some(),
        "a disconnected observer lives on"
    );
}
