//! Exposure over real documents: the registrations an element's attributes
//! and listeners make, and the transitions `dom`'s intersection updates turn
//! into.
//!
//! Each test plays the page's part by hand: [`deliver`] is one render, one
//! update and one notify loop — what an entry's epilogue and the delivery
//! entry it posts do between them — followed by the drain that entry makes.
//! The viewport is `test_support`'s, 393 by 727.

#![allow(clippy::float_cmp)] // Authored pixel sizes and ratios are exact.

use dom::{IntersectionMargin, MarginLength, NodeId, Vector2D};

use super::{
    Exposure, HostChain, Length, Margins, Transition, fold_margins, parse_area, parse_length,
};
use crate::main::tree::test_support::{
    child, document as fresh_document, element_under, with_config,
};
use crate::main::tree::{LynxDocument, PageConfig, nested_page};

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

/// A record-only transition of the document tree: an element with an
/// `exposure-id` and no listener.
fn record(node: NodeId, appeared: bool, id: &str) -> Transition {
    Transition {
        node,
        appeared,
        exposure_id: Some(id.to_owned()),
        scene: String::new(),
        hosts: HostChain::new(),
        element_event: false,
    }
}

/// [`record`], for an element under the shadow hosts `hosts`, innermost
/// first.
fn record_under(node: NodeId, appeared: bool, id: &str, hosts: &[NodeId]) -> Transition {
    Transition {
        hosts: hosts.to_vec(),
        ..record(node, appeared, id)
    }
}

/// The root margin the registration of `node` made its observer with.
fn root_margin(exposure: &Exposure, document: &LynxDocument, node: NodeId) -> IntersectionMargin {
    let observer = exposure
        .observer_of(node)
        .expect("the element is registered and detection runs");
    *document.intersection_observer(observer).root_margin()
}

/// The target margin the registration of `node` made its observer with.
fn target_margin(exposure: &Exposure, document: &LynxDocument, node: NodeId) -> IntersectionMargin {
    let observer = exposure
        .observer_of(node)
        .expect("the element is registered and detection runs");
    *document.intersection_observer(observer).target_margin()
}

/// A margin with `0px` on every side but those `sides` writes.
fn margin(sides: impl FnOnce(&mut IntersectionMargin)) -> IntersectionMargin {
    let mut margin = IntersectionMargin::ZERO;
    sides(&mut margin);
    margin
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
    let mut exposure = Exposure::new();
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
    let mut exposure = Exposure::new();
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
    let mut exposure = Exposure::new();
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

/// The screen margins, through an element: a positive one on the target
/// margin of the opposite side, a negative one on the root margin of its
/// own side, and a percentage left for the primitive to resolve.
#[test]
fn screen_margins_reach_the_observer() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
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
        target_margin(&exposure, &document, node),
        IntersectionMargin {
            top: MarginLength::Px(15.0),
            right: MarginLength::Px(0.0),
            bottom: MarginLength::Px(20.0 * VIEWPORT_WIDTH / 750.0),
            left: MarginLength::Percent(10.0),
        }
    );
    assert_eq!(
        root_margin(&exposure, &document, node),
        margin(|margin| margin.left = MarginLength::Px(-30.0))
    );
}

/// A positive screen margin grows the screen on its side and a negative one
/// shrinks it: an element 10px below the viewport is exposed by a 20px
/// bottom margin, and one at the top leaves when the top shrinks past it.
#[test]
fn screen_margins_grow_and_shrink_the_screen() {
    grow_and_shrink_the_screen(fresh_document());
}

/// The same answers on a page that clips, as native's page does: the
/// element below the viewport is cut off by the page before the root is
/// seen, and the 20px bottom margin still exposes it, because it grows the
/// element, whose grown rect the page's clip cuts.
#[test]
fn screen_margins_reach_past_a_clipping_page() {
    grow_and_shrink_the_screen(with_config(PageConfig {
        default_overflow_visible: false,
        ..PageConfig::default()
    }));
}

/// [`screen_margins_grow_and_shrink_the_screen`] over `document`.
fn grow_and_shrink_the_screen(mut document: LynxDocument) {
    let mut exposure = Exposure::new();
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

/// A positive screen margin reaches an element a scrollport cuts off, the
/// card `api-exposure-dynamic-screen-margin`'s case: an element scrolled
/// 10px below a `scroll-view`'s scrollport, on screen but cut by it, is
/// not exposed, and a 20px bottom margin grows the element's top back into
/// the scrollport.
#[test]
fn screen_margins_reach_past_a_scrollport() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let scroller = child(
        &mut document,
        "scroll-view",
        "position:absolute;left:0;top:0;width:100px;height:100px",
    );
    // At 140 in the scroller's scroll coordinates, past 140px of filler,
    // and followed by enough content for an offset of 30.
    let [_, node, _] = [140.0, 20.0, 100.0].map(|height| {
        element_under(
            &mut document,
            scroller,
            "view",
            &format!("flex-shrink:0;width:100px;height:{height}px"),
        )
    });
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    // 40px below the scrollport before the scroll, which needs a layout to
    // have a range, and 10px after it.
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
    document.scroll_to(scroller, Vector2D::new(0.0, 30.0));
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    set(
        &mut exposure,
        &mut document,
        node,
        "exposure-screen-margin-bottom",
        "20px",
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, true, "a")]
    );
}

/// The ui margins apply with no switch: an element's own
/// `enable-exposure-ui-margin` is not read, `"false"` included. An element
/// 10px below the viewport whose own top edge a 20px ui margin moves up into
/// it.
#[test]
fn ui_margins_apply_whatever_the_element_s_switch_says() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, VIEWPORT_HEIGHT + 10.0, 20.0);
    set(
        &mut exposure,
        &mut document,
        node,
        "exposure-ui-margin-top",
        "20px",
    );
    set(
        &mut exposure,
        &mut document,
        node,
        "enable-exposure-ui-margin",
        "false",
    );
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    assert_eq!(
        target_margin(&exposure, &document, node).top,
        MarginLength::Px(20.0)
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, true, "a")]
    );
}

/// The fold: a ui margin lands on the target margin of its own side, a
/// positive screen margin on the target margin of the opposite side, and a
/// negative screen margin on the root margin of its own side. A
/// percentage stays one, of the rect it lands on; a percentage and a
/// length that land on one side keep the length.
#[test]
fn margins_fold_onto_the_target_and_the_root() {
    let px = |px| MarginLength::Px(px);
    let percent = |percent| MarginLength::Percent(percent);
    let zero = [Length::Px(0.0); 4];

    // ui margins, same side, either sign.
    assert_eq!(
        fold_margins(
            zero,
            [
                Length::Px(10.0),
                Length::Px(-20.0),
                Length::Px(30.0),
                Length::Percent(40.0)
            ],
        ),
        Margins {
            root: IntersectionMargin::ZERO,
            target: IntersectionMargin {
                top: px(10.0),
                right: px(-20.0),
                bottom: px(30.0),
                left: percent(40.0),
            },
        }
    );
    // Positive screen margins, opposite side, a percentage included.
    assert_eq!(
        fold_margins(
            [
                Length::Px(1.0),
                Length::Px(2.0),
                Length::Percent(3.0),
                Length::Px(4.0)
            ],
            zero,
        ),
        Margins {
            root: IntersectionMargin::ZERO,
            target: IntersectionMargin {
                top: percent(3.0),
                right: px(4.0),
                bottom: px(1.0),
                left: px(2.0),
            },
        }
    );
    // Negative screen margins, same side on the root, a percentage
    // included; zero contributes nothing.
    assert_eq!(
        fold_margins(
            [
                Length::Px(-1.0),
                Length::Percent(-2.0),
                Length::Px(0.0),
                Length::Percent(0.0)
            ],
            zero,
        ),
        Margins {
            root: margin(|margin| {
                margin.top = px(-1.0);
                margin.right = percent(-2.0);
            }),
            target: IntersectionMargin::ZERO,
        }
    );
    // Both kinds on one side add: ui top plus screen bottom in px, ui left
    // plus screen right in percent. On the right a ui length and the
    // screen-left percentage meet, and the length is kept. Screen top, which
    // is negative, leaves the target's bottom to the ui margin alone.
    assert_eq!(
        fold_margins(
            [
                Length::Px(-5.0),
                Length::Percent(6.0),
                Length::Px(7.0),
                Length::Percent(8.0)
            ],
            [
                Length::Px(10.0),
                Length::Px(20.0),
                Length::Px(30.0),
                Length::Percent(40.0)
            ],
        ),
        Margins {
            root: margin(|margin| margin.top = px(-5.0)),
            target: IntersectionMargin {
                top: px(10.0 + 7.0),
                right: px(20.0),
                bottom: px(30.0),
                left: percent(40.0 + 6.0),
            },
        }
    );
}

/// Through an element above the viewport, whose bottom edge a 20px ui
/// margin moves down into it: the target margin of the same side, and no
/// root margin.
#[test]
fn a_ui_margin_grows_the_element_on_its_own_side() {
    let px = |px| MarginLength::Px(px);
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, -30.0, 20.0);
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
        target_margin(&exposure, &document, node),
        margin(|margin| margin.bottom = px(20.0))
    );
    assert_eq!(
        root_margin(&exposure, &document, node),
        IntersectionMargin::ZERO
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, true, "a")]
    );
}

/// A ui percentage is of the element's own size: an element 20px tall and
/// 10px below the viewport, whose top a `100%` ui margin moves up by 20px.
#[test]
fn a_ui_percentage_is_of_the_element_s_own_size() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, VIEWPORT_HEIGHT + 10.0, 20.0);
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    set(
        &mut exposure,
        &mut document,
        node,
        "exposure-ui-margin-top",
        "100%",
    );
    assert_eq!(
        target_margin(&exposure, &document, node).top,
        MarginLength::Percent(100.0)
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, true, "a")]
    );

    // `25%`, 5px of 20, leaves it 5px short.
    set(
        &mut exposure,
        &mut document,
        node,
        "exposure-ui-margin-top",
        "25%",
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, false, "a")]
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
    let mut exposure = Exposure::new();
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
    let mut exposure = Exposure::new();
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
            hosts: HostChain::new(),
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
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, 0.0, 100.0);
    exposure.set_listens(&mut document, node, true);
    let event = |appeared| Transition {
        node,
        appeared,
        exposure_id: None,
        scene: String::new(),
        hosts: HostChain::new(),
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
    let mut exposure = Exposure::new();
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
            hosts: HostChain::new(),
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
/// visible again, in `NodeId` order. All of it for the document tree, the
/// page realm's scope.
#[test]
fn stopping_with_send_event_leaves_everything_and_resuming_finds_it_again() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
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

    exposure.switch(&mut document, false, true, None);
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
    assert!(
        exposure.observer_of(a).is_some(),
        "an observer stays, and what it reports is ignored"
    );

    // Stopped: `c` moving into view and `d` registering are not detected.
    document.set_inline_style(
        c,
        "position:absolute;left:0;top:200px;width:100px;height:100px",
    );
    let d = boxed(&mut document, 300.0, 100.0);
    set(&mut exposure, &mut document, d, "exposure-id", "d");
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
    exposure.switch(&mut document, false, true, None);
    assert!(!exposure.has_transitions(), "a second stop does nothing");

    exposure.switch(&mut document, true, true, None);
    assert_eq!(
        deliver(&exposure, &mut document),
        ids(in_node_order(vec![a, b, c, d]), true)
    );
    let observer = exposure.observer_of(a);
    exposure.switch(&mut document, true, true, None);
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
    let mut exposure = Exposure::new();
    let kept = boxed(&mut document, 0.0, 100.0);
    let gone = boxed(&mut document, 100.0, 100.0);
    set(&mut exposure, &mut document, kept, "exposure-id", "kept");
    set(&mut exposure, &mut document, gone, "exposure-id", "gone");
    assert_eq!(deliver(&exposure, &mut document).len(), 2);

    exposure.switch(&mut document, false, false, None);
    assert!(!exposure.has_transitions());
    document.set_inline_style(
        gone,
        "position:absolute;left:0;top:1000px;width:100px;height:100px",
    );
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    exposure.switch(&mut document, true, true, None);
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
    let mut exposure = Exposure::new();
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
    // the registration go and sends what the free would have, as native's
    // next pass after a resume does.
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "b");
    assert_eq!(deliver(&exposure, &mut document).len(), 1);
    exposure.switch(&mut document, false, false, None);
    document.remove_element(node);
    document.drop_element(node);
    assert!(
        !exposure.has_transitions(),
        "nothing is sent for it while its scope is stopped"
    );
    exposure.switch(&mut document, true, true, None);
    assert!(!exposure.is_registered(node));
    assert_eq!(exposure.take_transitions(), vec![record(node, false, "b")]);
}

/// An element unlinked but alive stays registered and leaves through the
/// primitive: it is no longer rendered, so its update reports it not
/// intersecting, and its listener hears it.
#[test]
fn an_unlinked_element_leaves_through_the_primitive() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
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

// --- shadow hosts and scopes ---------------------------------------------

/// A page holding, at its top left, a 200×300 frame `frame` whose nested
/// page holds a 100×100 `card` element and, below it, a 200×150 frame
/// `inner`, whose nested page holds a 100×100 `grand` element; and the
/// page's own 100×100 `own` to their right. Each element is shown whole.
struct Nested {
    own: NodeId,
    frame: NodeId,
    card: NodeId,
    inner: NodeId,
    grand: NodeId,
}

/// The `flex-shrink:0` box every nested element is, `top` px down its
/// nested page's flow.
fn nested_box(height: f32, top: f32) -> String {
    format!("flex-shrink:0;width:100px;height:{height}px;margin-top:{top}px")
}

fn nested(document: &mut LynxDocument) -> Nested {
    let frame = child(
        document,
        "frame",
        "position:absolute;left:0;top:0;width:200px;height:300px",
    );
    let page = nested_page(document, frame).expect("a frame has its nested page");
    let card = element_under(document, page, "view", &nested_box(100.0, 0.0));
    let inner = element_under(
        document,
        page,
        "frame",
        "flex-shrink:0;width:200px;height:150px",
    );
    let inner_page = nested_page(document, inner).expect("a frame has its nested page");
    let grand = element_under(document, inner_page, "view", &nested_box(100.0, 0.0));
    let own = child(
        document,
        "view",
        "position:absolute;left:250px;top:0;width:100px;height:100px",
    );
    Nested {
        own,
        frame,
        card,
        inner,
        grand,
    }
}

/// [`nested`], with `exposure-id`s `card`, `grand` and `own` written in that
/// order and the first delivery, which exposes all three, taken.
fn nested_and_exposed(exposure: &mut Exposure, document: &mut LynxDocument) -> Nested {
    let tree = nested(document);
    for (node, id) in [
        (tree.card, "card"),
        (tree.grand, "grand"),
        (tree.own, "own"),
    ] {
        set(exposure, document, node, "exposure-id", id);
    }
    assert_eq!(
        deliver(exposure, document),
        vec![
            record_under(tree.card, true, "card", &[tree.frame]),
            record_under(tree.grand, true, "grand", &[tree.inner, tree.frame]),
            record(tree.own, true, "own"),
        ]
    );
    tree
}

/// An exposure carries the shadow hosts above its element, innermost first:
/// none for the document tree, the frame for a card's element, the inner
/// frame and then the outer for a grand-card's.
#[test]
fn an_exposure_carries_the_shadow_hosts_above_its_element() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let tree = nested_and_exposed(&mut exposure, &mut document);
    assert_eq!(exposure.exposed_hosts(tree.own), Some(Vec::new()));
    assert_eq!(exposure.exposed_hosts(tree.card), Some(vec![tree.frame]));
    assert_eq!(
        exposure.exposed_hosts(tree.grand),
        Some(vec![tree.inner, tree.frame])
    );
}

/// The hosts are the exposure's, kept until its `disexposure`: a card
/// element detached, a grand-card element freed, an id changed, and a stop
/// each send the `disexposure` under the hosts it was exposed under, though
/// the element is in no shadow tree any more, or none at all.
#[test]
fn the_hosts_are_kept_through_detach_free_id_change_and_stop() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let tree = nested_and_exposed(&mut exposure, &mut document);
    let card_page = nested_page(&document, tree.frame).unwrap();

    // Detached: the primitive reports it not intersecting.
    document.remove_element(tree.card);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record_under(tree.card, false, "card", &[tree.frame])]
    );

    // Freed: the observer's drop inside the free.
    document.remove_element(tree.grand);
    document.drop_element(tree.grand);
    assert_eq!(
        exposure.take_transitions(),
        vec![record_under(
            tree.grand,
            false,
            "grand",
            &[tree.inner, tree.frame]
        )]
    );

    // Back in the card, then its id changed: the disexposure under the old
    // id is the card's, and so is the exposure under the new one.
    document.append_child(card_page, tree.card);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record_under(tree.card, true, "card", &[tree.frame])]
    );
    set(
        &mut exposure,
        &mut document,
        tree.card,
        "exposure-id",
        "renamed",
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![
            record_under(tree.card, false, "card", &[tree.frame]),
            record_under(tree.card, true, "renamed", &[tree.frame]),
        ]
    );

    // Stopped with `sendEvent`.
    exposure.switch(&mut document, false, true, Some(tree.frame));
    assert_eq!(
        exposure.take_transitions(),
        vec![record_under(tree.card, false, "renamed", &[tree.frame])]
    );
}

/// The page's `stopExposure` stops the document tree's elements alone: its
/// `disexposure` names the page's own element, a card's and a grand-card's
/// elements go on being detected while it is stopped, and its resume finds
/// only its own again.
#[test]
fn stopping_the_page_leaves_every_card_detected() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let tree = nested_and_exposed(&mut exposure, &mut document);

    exposure.switch(&mut document, false, true, None);
    assert_eq!(
        exposure.take_transitions(),
        vec![record(tree.own, false, "own")]
    );

    // Scrolled out of their frames: both cards are still detected.
    document.set_inline_style(tree.card, &nested_box(100.0, 1000.0));
    document.set_inline_style(tree.grand, &nested_box(100.0, 1000.0));
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![
            record_under(tree.card, false, "card", &[tree.frame]),
            record_under(tree.grand, false, "grand", &[tree.inner, tree.frame]),
        ]
    );

    exposure.switch(&mut document, true, true, None);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(tree.own, true, "own")]
    );
}

/// A card's `stopExposure` with `sendEvent` disexposes its own element
/// alone; the page's element and the grand-card's go on being detected; and
/// its resume exposes its own element again and nothing else.
#[test]
fn stopping_a_card_disexposes_and_stops_its_own_elements_alone() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let tree = nested_and_exposed(&mut exposure, &mut document);

    exposure.switch(&mut document, false, true, Some(tree.frame));
    assert_eq!(
        exposure.take_transitions(),
        vec![record_under(tree.card, false, "card", &[tree.frame])]
    );
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    // The grand-card's own scope and the page's are running.
    document.set_inline_style(tree.grand, &nested_box(100.0, 1000.0));
    document.set_inline_style(
        tree.own,
        "position:absolute;left:250px;top:1000px;width:100px;height:100px",
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![
            record_under(tree.grand, false, "grand", &[tree.inner, tree.frame]),
            record(tree.own, false, "own"),
        ]
    );

    exposure.switch(&mut document, true, true, Some(tree.frame));
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record_under(tree.card, true, "card", &[tree.frame])]
    );
}

/// An element registered while detached belongs to no scope yet, and is
/// judged by the tree it is in when it appears: one appended to a stopped
/// card says nothing until that card resumes, one appended to the running
/// page is exposed at once.
#[test]
fn an_element_registered_while_detached_is_judged_where_it_appears() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let tree = nested(&mut document);
    let card_page = nested_page(&document, tree.frame).unwrap();
    exposure.switch(&mut document, false, true, Some(tree.frame));

    let late_card = document.create_element("view", ());
    document.set_inline_style(late_card, &nested_box(50.0, 0.0));
    set(
        &mut exposure,
        &mut document,
        late_card,
        "exposure-id",
        "late-card",
    );
    let late_own = document.create_element("view", ());
    document.set_inline_style(
        late_own,
        "position:absolute;left:250px;top:200px;width:50px;height:50px",
    );
    set(
        &mut exposure,
        &mut document,
        late_own,
        "exposure-id",
        "late-own",
    );
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    document.append_child(card_page, late_card);
    let page = document.document_element().id();
    document.append_child(page, late_own);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(late_own, true, "late-own")]
    );

    exposure.switch(&mut document, true, true, Some(tree.frame));
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record_under(late_card, true, "late-card", &[tree.frame])]
    );
}

/// An element freed while its card is stopped with its record kept sends
/// its `disexposure` when that card resumes — not when another scope does.
#[test]
fn a_card_element_freed_while_stopped_leaves_when_that_card_resumes() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let tree = nested_and_exposed(&mut exposure, &mut document);

    exposure.switch(&mut document, false, false, Some(tree.frame));
    exposure.switch(&mut document, false, false, None);
    document.remove_element(tree.card);
    document.drop_element(tree.card);
    assert!(!exposure.has_transitions());

    exposure.switch(&mut document, true, true, None);
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
    assert!(exposure.is_registered(tree.card));

    exposure.switch(&mut document, true, true, Some(tree.frame));
    assert!(!exposure.is_registered(tree.card));
    assert_eq!(
        exposure.take_transitions(),
        vec![record_under(tree.card, false, "card", &[tree.frame])]
    );
}
