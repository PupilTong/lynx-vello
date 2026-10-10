//! Exposure over real documents: the registrations an element's attributes
//! and listeners make, and the transitions `dom`'s intersection updates turn
//! into.
//!
//! Each test plays the page's part by hand: [`deliver`] is one render, one
//! update and one notify loop — what an entry's epilogue and the delivery
//! entry it posts do between them — followed by the drain that entry makes.
//! The viewport is `test_support`'s, 393 by 727.

#![allow(clippy::float_cmp)] // Authored pixel sizes and ratios are exact.

use dom::{MarginLength, NodeId, RootMargin, Vector2D};

use super::{Exposure, Length, Transition, fold_margins, parse_area, parse_length};
use crate::main::tree::LynxDocument;
use crate::main::tree::test_support::{child, document as fresh_document, element_under};

/// `test_support`'s viewport.
const VIEWPORT_WIDTH: f32 = 393.0;
const VIEWPORT_HEIGHT: f32 = 727.0;

/// A `height`-tall box `top` px down the page, positioned out of flow so
/// its geometry is exactly what is written.
fn boxed(document: &mut LynxDocument, top: f32, height: f32) -> NodeId {
    child(
        document,
        "view",
        &format!("position:absolute;left:0;top:{top}px;width:100px;height:{height}px"),
    )
}

/// Writes one attribute the way `setAttribute` does: the mutation, then the
/// registration.
fn set(
    exposure: &mut Exposure,
    document: &mut LynxDocument,
    node: NodeId,
    name: &str,
    value: &str,
) {
    document.set_attribute(node, name, value);
    exposure.attribute_changed(document, node);
}

/// Removes one attribute the way `removeAttribute` does.
fn remove(exposure: &mut Exposure, document: &mut LynxDocument, node: NodeId, name: &str) {
    document.remove_attribute(node, name);
    exposure.attribute_changed(document, node);
}

/// One render, one update and one notify loop, then every pending
/// transition.
fn deliver(exposure: &Exposure, document: &mut LynxDocument) -> Vec<Transition> {
    document.render();
    document.update_intersection_observations(0.0);
    document.notify_intersection_observers();
    exposure.take_transitions()
}

/// A record-only transition: an element with an `exposure-id` and no
/// listener.
fn record(node: NodeId, appeared: bool, id: &str) -> Transition {
    Transition {
        node,
        appeared,
        exposure_id: Some(id.to_owned()),
        scene: String::new(),
        element_event: false,
    }
}

/// The root margin the registration of `node` made its observer with.
fn root_margin(exposure: &Exposure, document: &LynxDocument, node: NodeId) -> RootMargin {
    let observer = exposure
        .observer_of(node)
        .expect("the element is registered and detection runs");
    *document.intersection_observer(observer).root_margin()
}

/// `nodes` in `NodeId` order, the order a switch visits registrations in.
fn in_node_order(mut nodes: Vec<NodeId>) -> Vec<NodeId> {
    nodes.sort_unstable_by_key(|node| node.to_bits());
    nodes
}

// --- registration and the first update -----------------------------------

/// An `exposure-id` registers the element; the first update after it reports
/// whatever is visible, and an element that is not visible says nothing.
#[test]
fn an_exposure_id_registers_and_the_first_update_reports_what_is_visible() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let shown = boxed(&mut document, 0.0, 100.0);
    let below = boxed(&mut document, 1000.0, 100.0);
    set(&mut exposure, &mut document, shown, "exposure-id", "shown");
    set(&mut exposure, &mut document, below, "exposure-id", "below");
    assert!(exposure.is_registered(shown) && exposure.is_registered(below));
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(shown, true, "shown")]
    );
}

/// An update with nothing moved queues nothing, and a re-arm whose element
/// is where it was recorded queues nothing either: the observer is new, its
/// first entry is reported, and it agrees with the record.
#[test]
fn nothing_is_queued_when_nothing_moved() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    assert_eq!(deliver(&exposure, &mut document).len(), 1);

    assert_eq!(deliver(&exposure, &mut document), Vec::new());
    set(&mut exposure, &mut document, node, "exposure-scene", "feed");
    assert!(!exposure.has_transitions());
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
}

/// `exposure-area` asks for that share of the element to be visible: a
/// 40px element in a 100px scrollport, half of it at offset 20.
#[test]
fn exposure_area_asks_for_that_share_of_the_element() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let scroller = child(
        &mut document,
        "view",
        "position:absolute;left:0;top:0;width:100px;height:100px;overflow:hidden;\
         display:flex;flex-direction:column",
    );
    element_under(
        &mut document,
        scroller,
        "view",
        "flex-shrink:0;width:100px;height:100px",
    );
    let target = element_under(
        &mut document,
        scroller,
        "view",
        "flex-shrink:0;width:100px;height:40px",
    );
    set(&mut exposure, &mut document, target, "exposure-area", "50%");
    set(&mut exposure, &mut document, target, "exposure-id", "half");
    // On the scrollport's edge: intersecting, with ratio 0.
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    // (offset, whether the element is exposed after it)
    for (offset, expected) in [
        (19.0, None),
        (20.0, Some(true)),
        (30.0, None),
        (10.0, Some(false)),
    ] {
        document.scroll_to(scroller, Vector2D::new(0.0, offset));
        let expected: Vec<_> = expected
            .into_iter()
            .map(|appeared| record(target, appeared, "half"))
            .collect();
        assert_eq!(
            deliver(&exposure, &mut document),
            expected,
            "at offset {offset}"
        );
    }
}

// --- lengths and margins -------------------------------------------------

/// `<n>px`, `<n>rpx` (the viewport width over 750), `<n>%`, a bare number
/// as px, and 0 for everything else; white space around either part is
/// ignored and a negative length is kept.
#[test]
fn margin_lengths_read_px_rpx_percent_and_bare_numbers() {
    let width = VIEWPORT_WIDTH;
    for (value, expected) in [
        (None, Length::Px(0.0)),
        (Some("20px"), Length::Px(20.0)),
        (Some(" 20rpx "), Length::Px(20.0 * width / 750.0)),
        (Some("10%"), Length::Percent(10.0)),
        (Some("15"), Length::Px(15.0)),
        (Some("-30px"), Length::Px(-30.0)),
        (Some("2.5 px"), Length::Px(2.5)),
        (Some(""), Length::Px(0.0)),
        (Some("px"), Length::Px(0.0)),
        (Some("1em"), Length::Px(0.0)),
        (Some("abc"), Length::Px(0.0)),
        (Some("NaNpx"), Length::Px(0.0)),
        (Some("infpx"), Length::Px(0.0)),
    ] {
        assert_eq!(parse_length(value, width), expected, "{value:?}");
    }
}

/// The screen margins, through an element: each side as its attribute
/// names it, a percentage left for the primitive to resolve.
#[test]
fn screen_margins_reach_the_observer() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let node = boxed(&mut document, 0.0, 100.0);
    for (name, value) in [
        ("exposure-screen-margin-top", "20rpx"),
        ("exposure-screen-margin-right", "10%"),
        ("exposure-screen-margin-bottom", "15"),
        ("exposure-screen-margin-left", "-30px"),
        ("exposure-id", "a"),
    ] {
        set(&mut exposure, &mut document, node, name, value);
    }
    assert_eq!(
        root_margin(&exposure, &document, node),
        RootMargin {
            top: MarginLength::Px(20.0 * VIEWPORT_WIDTH / 750.0),
            right: MarginLength::Percent(10.0),
            bottom: MarginLength::Px(15.0),
            left: MarginLength::Px(-30.0),
        }
    );
}

/// A positive screen margin grows the screen on its side and a negative one
/// shrinks it: an element 10px below the viewport is exposed by a 20px
/// bottom margin, and one at the top leaves when the top shrinks past it.
#[test]
fn screen_margins_grow_and_shrink_the_screen() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let below = boxed(&mut document, VIEWPORT_HEIGHT + 10.0, 20.0);
    let top = boxed(&mut document, 0.0, 20.0);
    set(&mut exposure, &mut document, below, "exposure-id", "below");
    set(&mut exposure, &mut document, top, "exposure-id", "top");
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(top, true, "top")]
    );

    set(
        &mut exposure,
        &mut document,
        below,
        "exposure-screen-margin-bottom",
        "20px",
    );
    set(
        &mut exposure,
        &mut document,
        top,
        "exposure-screen-margin-top",
        "-30px",
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(below, true, "below"), record(top, false, "top")]
    );
}

/// The ui margins apply only when switched on: by the element's own
/// `enable-exposure-ui-margin`, present and not `"false"`, or, where it
/// names none, by the page's `enableExposureUIMargin`. An element 10px below
/// the viewport whose own top edge a 20px ui margin moves up into it.
#[test]
fn ui_margins_apply_only_when_switched_on() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let node = boxed(&mut document, VIEWPORT_HEIGHT + 10.0, 20.0);
    set(
        &mut exposure,
        &mut document,
        node,
        "exposure-ui-margin-top",
        "20px",
    );
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
    assert_eq!(
        root_margin(&exposure, &document, node).bottom,
        MarginLength::Px(0.0)
    );

    set(
        &mut exposure,
        &mut document,
        node,
        "enable-exposure-ui-margin",
        "true",
    );
    assert_eq!(
        root_margin(&exposure, &document, node).bottom,
        MarginLength::Px(20.0)
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, true, "a")]
    );
    set(
        &mut exposure,
        &mut document,
        node,
        "enable-exposure-ui-margin",
        "false",
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, false, "a")]
    );

    // The page's switch, which the element's own overrides either way.
    let mut document = fresh_document();
    let mut exposure = Exposure::new(true);
    let node = boxed(&mut document, VIEWPORT_HEIGHT + 10.0, 20.0);
    set(
        &mut exposure,
        &mut document,
        node,
        "exposure-ui-margin-top",
        "20px",
    );
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, true, "a")]
    );
    set(
        &mut exposure,
        &mut document,
        node,
        "enable-exposure-ui-margin",
        "false",
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, false, "a")]
    );
}

/// Growing an element's edge is growing the root's opposite edge: each ui
/// margin lands on the root margin of the side across from it, added to
/// that side's screen margin. A screen percentage survives only where no ui
/// margin is added to it, and a ui percentage is 0.
#[test]
fn ui_margins_fold_onto_the_opposite_root_side() {
    let px = |px| MarginLength::Px(px);
    assert_eq!(
        fold_margins(
            [
                Length::Px(1.0),
                Length::Px(2.0),
                Length::Px(3.0),
                Length::Px(4.0)
            ],
            [10.0, 20.0, 30.0, 40.0],
        ),
        RootMargin {
            top: px(1.0 + 30.0),
            right: px(2.0 + 40.0),
            bottom: px(3.0 + 10.0),
            left: px(4.0 + 20.0),
        }
    );
    assert_eq!(
        fold_margins(
            [
                Length::Percent(10.0),
                Length::Percent(20.0),
                Length::Px(0.0),
                Length::Px(0.0)
            ],
            [0.0, 0.0, 0.0, 5.0],
        ),
        RootMargin {
            top: MarginLength::Percent(10.0),
            right: px(5.0),
            bottom: px(0.0),
            left: px(0.0),
        }
    );

    // Through an element above the viewport, whose bottom edge a 20px ui
    // margin moves down into it: the root's top grows instead. A percentage
    // ui margin, which natively is of the element's own size, is 0.
    let mut document = fresh_document();
    let mut exposure = Exposure::new(true);
    let node = boxed(&mut document, -30.0, 20.0);
    set(
        &mut exposure,
        &mut document,
        node,
        "exposure-ui-margin-left",
        "50%",
    );
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
    set(
        &mut exposure,
        &mut document,
        node,
        "exposure-ui-margin-bottom",
        "20px",
    );
    assert_eq!(
        root_margin(&exposure, &document, node),
        RootMargin {
            top: px(20.0),
            right: px(0.0),
            bottom: px(0.0),
            left: px(0.0),
        }
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, true, "a")]
    );
}

/// `exposure-area` is `parseFloat(value) / 100`, clamped to `[0, 1]`, and 0
/// for what does not parse.
#[test]
fn exposure_area_is_parse_float_over_a_hundred() {
    for (value, expected) in [
        (None, 0.0),
        (Some("50%"), 0.5),
        (Some("50"), 0.5),
        (Some(" 25px"), 0.25),
        (Some("12.5"), 0.125),
        (Some(".5"), 0.005),
        (Some("5."), 0.05),
        (Some("1e2"), 1.0),
        (Some("2e"), 0.02),
        (Some("150%"), 1.0),
        (Some("-10"), 0.0),
        (Some("Infinity"), 1.0),
        (Some("abc"), 0.0),
        (Some("."), 0.0),
        (Some(""), 0.0),
    ] {
        assert_eq!(parse_area(value), expected, "{value:?}");
    }
}

// --- teardown and re-arm -------------------------------------------------

/// A changed `exposure-id` sends a `disexposure` under the id the element
/// was exposed under, at once, and the next update exposes it under the new
/// one.
#[test]
fn a_changed_exposure_id_leaves_under_the_old_id_and_appears_under_the_new() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "old");
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, true, "old")]
    );

    set(&mut exposure, &mut document, node, "exposure-id", "new");
    assert!(
        exposure.has_transitions(),
        "the disexposure is owed before any update"
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, false, "old"), record(node, true, "new")]
    );
}

/// Removing the `exposure-id` of an element nothing else registers sends a
/// `disexposure` with no element event and unregisters it.
#[test]
fn removing_the_exposure_id_leaves_and_unregisters() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-scene", "feed");
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    assert_eq!(deliver(&exposure, &mut document).len(), 1);

    remove(&mut exposure, &mut document, node, "exposure-id");
    assert!(!exposure.is_registered(node));
    assert_eq!(
        exposure.take_transitions(),
        vec![Transition {
            node,
            appeared: false,
            exposure_id: Some("a".to_owned()),
            scene: "feed".to_owned(),
            element_event: false,
        }]
    );
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
}

/// A listener alone registers an element, which hears its own events and
/// has no record; removing the listener of an element that is not exposed
/// says nothing.
#[test]
fn a_listener_alone_registers_and_hears_its_element_events() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let node = boxed(&mut document, 0.0, 100.0);
    exposure.set_listens(&mut document, node, true);
    let event = |appeared| Transition {
        node,
        appeared,
        exposure_id: None,
        scene: String::new(),
        element_event: true,
    };
    assert_eq!(deliver(&exposure, &mut document), vec![event(true)]);

    document.set_inline_style(
        node,
        "position:absolute;left:0;top:1000px;width:100px;height:100px",
    );
    assert_eq!(deliver(&exposure, &mut document), vec![event(false)]);

    exposure.set_listens(&mut document, node, false);
    assert!(!exposure.is_registered(node));
    assert!(!exposure.has_transitions());
}

/// A listener and an `exposure-id` on one element make one transition that
/// carries both: the element's event and its record.
#[test]
fn a_listener_and_an_id_make_one_transition() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-scene", "feed");
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    exposure.set_listens(&mut document, node, true);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![Transition {
            node,
            appeared: true,
            exposure_id: Some("a".to_owned()),
            scene: "feed".to_owned(),
            element_event: true,
        }]
    );

    // The listener going leaves the element registered by its id, and
    // exposed: nothing is sent.
    exposure.set_listens(&mut document, node, false);
    assert!(exposure.is_registered(node));
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
}

// --- stop and resume -----------------------------------------------------

/// `stopExposure({sendEvent: true})` sends one `disexposure` per exposed
/// element and forgets them as exposed; while stopped nothing is detected,
/// a registration made then included; `resumeExposure()` exposes whatever is
/// visible again, in `NodeId` order.
#[test]
fn stopping_with_send_event_leaves_everything_and_resuming_finds_it_again() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let a = boxed(&mut document, 0.0, 100.0);
    let b = boxed(&mut document, 100.0, 100.0);
    let c = boxed(&mut document, 1000.0, 100.0);
    for (node, id) in [(a, "a"), (b, "b"), (c, "c")] {
        set(&mut exposure, &mut document, node, "exposure-id", id);
    }
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(a, true, "a"), record(b, true, "b")]
    );

    exposure.switch(&mut document, false, true);
    let ids = |nodes: Vec<NodeId>, appeared| -> Vec<Transition> {
        nodes
            .into_iter()
            .map(|node| {
                let id = if node == a {
                    "a"
                } else if node == b {
                    "b"
                } else if node == c {
                    "c"
                } else {
                    "d"
                };
                record(node, appeared, id)
            })
            .collect()
    };
    assert_eq!(
        exposure.take_transitions(),
        ids(in_node_order(vec![a, b]), false)
    );
    assert_eq!(exposure.observer_of(a), None, "every observer is dropped");

    // Stopped: `c` moving into view and `d` registering are not detected.
    document.set_inline_style(
        c,
        "position:absolute;left:0;top:200px;width:100px;height:100px",
    );
    let d = boxed(&mut document, 300.0, 100.0);
    set(&mut exposure, &mut document, d, "exposure-id", "d");
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
    exposure.switch(&mut document, false, true);
    assert!(!exposure.has_transitions(), "a second stop does nothing");

    exposure.switch(&mut document, true, true);
    assert_eq!(
        deliver(&exposure, &mut document),
        ids(in_node_order(vec![a, b, c, d]), true)
    );
    let observer = exposure.observer_of(a);
    exposure.switch(&mut document, true, true);
    assert_eq!(
        exposure.observer_of(a),
        observer,
        "resuming a running page does nothing"
    );
}

/// `stopExposure({sendEvent: false})` sends nothing and keeps the exposed
/// record: a resume that finds an element still visible sends nothing for
/// it, and one that went away while stopped leaves.
#[test]
fn stopping_without_send_event_keeps_the_exposed_record() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let kept = boxed(&mut document, 0.0, 100.0);
    let gone = boxed(&mut document, 100.0, 100.0);
    set(&mut exposure, &mut document, kept, "exposure-id", "kept");
    set(&mut exposure, &mut document, gone, "exposure-id", "gone");
    assert_eq!(deliver(&exposure, &mut document).len(), 2);

    exposure.switch(&mut document, false, false);
    assert!(!exposure.has_transitions());
    document.set_inline_style(
        gone,
        "position:absolute;left:0;top:1000px;width:100px;height:100px",
    );
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    exposure.switch(&mut document, true, true);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(gone, false, "gone")]
    );
}

// --- element lifetime ----------------------------------------------------

/// A freed element that was exposed sends one `disexposure`, from its
/// observer's drop inside the free, and is unregistered; the next update
/// says nothing more of it.
#[test]
fn a_freed_element_that_was_exposed_leaves_once() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    exposure.set_listens(&mut document, node, true);
    assert_eq!(deliver(&exposure, &mut document).len(), 1);

    // The collection path: a detached element whose handle was collected.
    document.remove_element(node);
    document.drop_element(node);
    assert!(!exposure.is_registered(node));
    assert_eq!(
        exposure.take_transitions(),
        vec![record(node, false, "a")],
        "a record and no element event: there is no element to hear it"
    );
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    // Freed while stopped, with the exposed record kept: the resume lets
    // the registration go and sends what the free would have.
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "b");
    assert_eq!(deliver(&exposure, &mut document).len(), 1);
    exposure.switch(&mut document, false, false);
    document.remove_element(node);
    document.drop_element(node);
    assert!(
        !exposure.has_transitions(),
        "no observer to drop while stopped"
    );
    exposure.switch(&mut document, true, true);
    assert!(!exposure.is_registered(node));
    assert_eq!(exposure.take_transitions(), vec![record(node, false, "b")]);
}

/// An element unlinked but alive stays registered and leaves through the
/// primitive: it is no longer rendered, so its update reports it not
/// intersecting, and its listener hears it.
#[test]
fn an_unlinked_element_leaves_through_the_primitive() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new(false);
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    exposure.set_listens(&mut document, node, true);
    assert_eq!(deliver(&exposure, &mut document).len(), 1);

    document.remove_element(node);
    assert!(
        !exposure.has_transitions(),
        "nothing is owed before the update"
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![Transition {
            element_event: true,
            ..record(node, false, "a")
        }]
    );
    assert!(exposure.is_registered(node));
}
