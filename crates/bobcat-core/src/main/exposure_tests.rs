//! Exposure over real documents: the registrations the listener table makes,
//! the re-arms an element's attributes make, and the changes `terminus`'s
//! intersection updates turn into.
//!
//! Each test plays the page's part by hand: [`deliver`] is one render, one
//! update and one notify loop — what an entry's epilogue and the delivery
//! entry it posts do between them — followed by the drain that entry makes.
//! [`set`], [`remove`] and [`listen`] also play the realm's Lynx layer, which
//! files the record listener on an element while it has a non-empty
//! `exposure-id` or a `uiappear`/`uidisappear` handler ([`follow`]). The
//! viewport is `test_support`'s, 393 by 727.

#![allow(clippy::float_cmp)] // Authored pixel sizes and ratios are exact.

use terminus::{IntersectionMargin, MarginLength, NodeId, Vector2D};

use super::{Exposure, ExposureChange, Length, Margins, fold_margins, parse_area, parse_length};
use crate::main::tree::test_support::{
    child, document as fresh_document, element_under, with_config,
};
use crate::main::tree::{LynxDocument, PageConfig};

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

/// Writes one attribute the way `__SetAttribute` does: the mutation and the
/// re-arm `setAttribute` makes, then the record listener the Lynx layer
/// files or removes.
fn set(
    exposure: &mut Exposure,
    document: &mut LynxDocument,
    node: NodeId,
    name: &str,
    value: &str,
) {
    document.set_attribute(node, name, value);
    exposure.attribute_changed(document, node);
    follow(exposure, document, node, None);
}

/// Removes one attribute the way `__SetAttribute` with a null value does.
fn remove(exposure: &mut Exposure, document: &mut LynxDocument, node: NodeId, name: &str) {
    document.remove_attribute(node, name);
    exposure.attribute_changed(document, node);
    follow(exposure, document, node, None);
}

/// Files (`on`) or removes the element's `uiappear`/`uidisappear` handler,
/// with the record listener the Lynx layer files or removes beside it.
fn listen(exposure: &mut Exposure, document: &mut LynxDocument, node: NodeId, on: bool) {
    follow(exposure, document, node, Some(on));
}

/// What the listener table tells exposure once the Lynx layer has followed
/// `node`: the record listener is there while the element has a non-empty
/// `exposure-id` or a handler. `handler` is the handler's new state, `None`
/// to keep the one the registration has.
fn follow(
    exposure: &mut Exposure,
    document: &mut LynxDocument,
    node: NodeId,
    handler: Option<bool>,
) {
    let handler = handler.unwrap_or_else(|| {
        exposure
            .shared
            .borrow()
            .entries
            .get(&node)
            .is_some_and(|entry| entry.element_event)
    });
    let id = document
        .get(node)
        .and_then(|element| element.attribute("exposure-id"))
        .is_some_and(|id| !id.is_empty());
    exposure.set_registered(document, node, id || handler, handler);
}

/// One render, one update and one notify loop, then every pending
/// change.
fn deliver(exposure: &Exposure, document: &mut LynxDocument) -> Vec<ExposureChange> {
    document.render();
    document.update_intersection_observations(0.0);
    document.notify_intersection_observers();
    exposure.take_changes()
}

/// A record-only change: an element with an `exposure-id` and no
/// listener.
fn record(node: NodeId, appeared: bool, id: &str) -> ExposureChange {
    ExposureChange {
        node,
        appeared,
        exposure_id: Some(id.to_owned()),
        scene: String::new(),
        element_event: false,
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

/// The record listener registers the element, whatever its attributes, and
/// its removal unregisters it; an element registered with no id and no
/// handler queues nothing, as it can deliver neither a record nor an event.
#[test]
fn the_record_listener_registers_and_its_removal_unregisters() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, 0.0, 100.0);
    exposure.set_registered(&mut document, node, true, false);
    assert!(exposure.is_registered(node));
    assert!(exposure.observer_of(node).is_some());
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    exposure.set_registered(&mut document, node, false, false);
    assert!(!exposure.is_registered(node));
    assert_eq!(exposure.observer_of(node), None);
    assert!(!exposure.has_changes());
}

/// An `exposure-id` alone registers nothing: an attribute write only re-arms
/// an element the listener table has registered, and the record listener
/// filed later arms it from the attributes as they are then.
#[test]
fn an_exposure_id_alone_does_not_register() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, 0.0, 100.0);
    document.set_attribute(node, "exposure-id", "a");
    exposure.attribute_changed(&mut document, node);
    assert!(!exposure.is_registered(node));
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    exposure.set_registered(&mut document, node, true, false);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(node, true, "a")]
    );
}

/// A change of whether the element hears its events, with the record
/// listener staying, keeps the observer and changes what the next change
/// says.
#[test]
fn a_handler_change_alone_keeps_the_observer() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    let observer = exposure.observer_of(node);
    listen(&mut exposure, &mut document, node, true);
    assert_eq!(exposure.observer_of(node), observer);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![ExposureChange {
            element_event: true,
            ..record(node, true, "a")
        }]
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
    assert!(!exposure.has_changes());
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
        exposure.has_changes(),
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
        exposure.take_changes(),
        vec![ExposureChange {
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
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, 0.0, 100.0);
    listen(&mut exposure, &mut document, node, true);
    let event = |appeared| ExposureChange {
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

    listen(&mut exposure, &mut document, node, false);
    assert!(!exposure.is_registered(node));
    assert!(!exposure.has_changes());
}

/// A listener and an `exposure-id` on one element make one change that
/// carries both: the element's event and its record.
#[test]
fn a_listener_and_an_id_make_one_change() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-scene", "feed");
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    listen(&mut exposure, &mut document, node, true);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![ExposureChange {
            node,
            appeared: true,
            exposure_id: Some("a".to_owned()),
            scene: "feed".to_owned(),
            element_event: true,
        }]
    );

    // The listener going leaves the element registered by its id, and
    // exposed: nothing is sent.
    listen(&mut exposure, &mut document, node, false);
    assert!(exposure.is_registered(node));
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
}

// --- stop and resume -----------------------------------------------------

/// Three 100px boxes down the page, `a` and `b` shown and `c` below the
/// viewport, with the `exposure-id`s `a`, `b` and `c`, and the first
/// delivery, which exposes `a` and `b`, taken.
fn three_exposed(exposure: &mut Exposure, document: &mut LynxDocument) -> [NodeId; 3] {
    let a = boxed(document, 0.0, 100.0);
    let b = boxed(document, 100.0, 100.0);
    let c = boxed(document, 1000.0, 100.0);
    for (node, id) in [(a, "a"), (b, "b"), (c, "c")] {
        set(exposure, document, node, "exposure-id", id);
    }
    assert_eq!(
        deliver(exposure, document),
        vec![record(a, true, "a"), record(b, true, "b")]
    );
    [a, b, c]
}

/// Moves a [`boxed`] element to `top`.
fn move_to(document: &mut LynxDocument, node: NodeId, top: f32) {
    document.set_inline_style(
        node,
        &format!("position:absolute;left:0;top:{top}px;width:100px;height:100px"),
    );
}

/// `stopExposure({sendEvent: true})` sends one `disexposure` per listed
/// exposed element, in the order listed, answers how many it queued, and
/// forgets them as exposed; while stopped nothing listed is detected, though
/// every observer stays; `resumeExposure()` exposes whatever listed is
/// visible again. A second stop with nothing exposed queues nothing.
#[test]
fn stopping_with_send_event_leaves_everything_and_resuming_finds_it_again() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let [a, b, c] = three_exposed(&mut exposure, &mut document);

    assert_eq!(exposure.switch(&mut document, false, true, &[b, a, c]), 2);
    assert_eq!(
        exposure.take_changes(),
        vec![record(b, false, "b"), record(a, false, "a")]
    );
    assert!(
        exposure.observer_of(a).is_some(),
        "an observer stays, and what it reports is ignored"
    );
    assert!([a, b, c].iter().all(|node| exposure.is_stopped(*node)));

    // Stopped: `c` moving into view is not detected.
    move_to(&mut document, c, 200.0);
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
    assert_eq!(exposure.switch(&mut document, false, true, &[a, b, c]), 0);
    assert!(!exposure.has_changes(), "a second stop owes nothing");

    assert_eq!(exposure.switch(&mut document, true, false, &[a, b, c]), 0);
    assert!([a, b, c].iter().all(|node| !exposure.is_stopped(*node)));
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![
            record(a, true, "a"),
            record(b, true, "b"),
            record(c, true, "c")
        ]
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

    assert_eq!(
        exposure.switch(&mut document, false, false, &[kept, gone]),
        0
    );
    assert!(!exposure.has_changes());
    assert!(exposure.is_exposed(kept) && exposure.is_exposed(gone));
    move_to(&mut document, gone, 1000.0);
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    exposure.switch(&mut document, true, false, &[kept, gone]);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(gone, false, "gone")]
    );
}

/// A switch reaches the elements it lists and no other: a stop of `a` and
/// `b` leaves `c` detected, and a resume of `a` alone leaves `b` stopped.
/// That is how one card's stop leaves the page and every other card running:
/// each realm lists the elements it registered.
#[test]
fn a_switch_reaches_only_the_listed_elements() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let [a, b, c] = three_exposed(&mut exposure, &mut document);
    move_to(&mut document, c, 200.0);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(c, true, "c")]
    );

    assert_eq!(exposure.switch(&mut document, false, true, &[a, b]), 2);
    exposure.take_changes();
    assert!(!exposure.is_stopped(c));

    // `c` leaves and is told; `a` and `b` leaving are not detected.
    for node in [a, b, c] {
        move_to(&mut document, node, 1000.0);
    }
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(c, false, "c")]
    );

    // Back in view; `a` alone resumes.
    for (node, top) in [(a, 0.0), (b, 100.0), (c, 200.0)] {
        move_to(&mut document, node, top);
    }
    let b_observer = exposure.observer_of(b);
    let c_observer = exposure.observer_of(c);
    exposure.switch(&mut document, true, false, &[a]);
    assert_eq!(
        exposure.observer_of(b),
        b_observer,
        "an element the resume does not list keeps its observer"
    );
    assert_eq!(
        exposure.observer_of(c),
        c_observer,
        "and so does one that was never stopped"
    );
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(c, true, "c"), record(a, true, "a")]
    );
    assert!(exposure.is_stopped(b));
}

/// What a stop answers is the records it queued: none without `sendEvent`,
/// none for listed elements with nothing exposed, and none for an exposed
/// element with no `exposure-id`, which is forgotten as exposed all the
/// same. A stop with `sendEvent` after one without sends the records the
/// first kept, as native's sends whatever it has recorded as exposed.
#[test]
fn a_stop_answers_the_records_it_queued() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let [a, b, c] = three_exposed(&mut exposure, &mut document);
    let quiet = boxed(&mut document, 200.0, 10.0);
    listen(&mut exposure, &mut document, quiet, true);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![ExposureChange {
            node: quiet,
            appeared: true,
            exposure_id: None,
            scene: String::new(),
            element_event: true,
        }]
    );

    assert_eq!(exposure.switch(&mut document, false, false, &[a, quiet]), 0);
    assert!(!exposure.has_changes());
    assert_eq!(exposure.switch(&mut document, false, true, &[a, quiet]), 1);
    assert_eq!(exposure.take_changes(), vec![record(a, false, "a")]);
    assert!(!exposure.is_exposed(quiet));

    // `c` is registered and below the viewport: nothing is exposed to send.
    assert_eq!(exposure.switch(&mut document, false, true, &[c]), 0);
    assert!(!exposure.has_changes());
    assert!(
        exposure.is_exposed(b),
        "an element not listed keeps its record"
    );
}

/// An element registered while its realm is stopped is stopped by the realm
/// right after it files the record listener — a stop that lists it alone,
/// without `sendEvent` — so its first report is ignored, and the resume
/// that lists it exposes it.
#[test]
fn an_element_registered_while_its_realm_is_stopped_waits_for_the_resume() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let [a, b, c] = three_exposed(&mut exposure, &mut document);
    assert_eq!(exposure.switch(&mut document, false, true, &[a, b, c]), 2);
    exposure.take_changes();

    let late = boxed(&mut document, 300.0, 100.0);
    set(&mut exposure, &mut document, late, "exposure-id", "late");
    assert!(!exposure.is_stopped(late), "a registration starts running");
    assert_eq!(exposure.switch(&mut document, false, false, &[late]), 0);
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    exposure.switch(&mut document, true, false, &[a, b, c, late]);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![
            record(a, true, "a"),
            record(b, true, "b"),
            record(late, true, "late")
        ]
    );
}

/// A stopped element stays stopped wherever it goes: the mark is the
/// element's, not its tree's, so one moved under another parent while
/// stopped is not detected there, visible and not exposed as it is, and an
/// id write re-arms it without resuming it; the resume that lists it exposes
/// it under its new id.
#[test]
fn a_stopped_element_stays_stopped_wherever_it_moves() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let [a, _b, _c] = three_exposed(&mut exposure, &mut document);
    assert_eq!(exposure.switch(&mut document, false, true, &[a]), 1);
    exposure.take_changes();

    let holder = boxed(&mut document, 300.0, 100.0);
    document.remove_element(a);
    document.append_child(holder, a);
    set(&mut exposure, &mut document, a, "exposure-id", "a2");
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
    assert!(exposure.is_stopped(a));

    exposure.switch(&mut document, true, false, &[a]);
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(a, true, "a2")]
    );
}

/// Ids that name no registered element are skipped, by a stop and a resume
/// alike: an element never registered, and one freed — whose registration
/// went with its handler — even once its arena slot names another element.
#[test]
fn unregistered_and_freed_ids_are_ignored() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let [a, _b, _c] = three_exposed(&mut exposure, &mut document);
    let never = boxed(&mut document, 0.0, 10.0);
    let freed = boxed(&mut document, 0.0, 10.0);
    set(&mut exposure, &mut document, freed, "exposure-id", "freed");
    document.remove_element(freed);
    document.drop_element(freed);
    let next = boxed(&mut document, 0.0, 10.0);
    set(&mut exposure, &mut document, next, "exposure-id", "next");
    deliver(&exposure, &mut document);

    let page = document.document_element().id();
    assert_eq!(
        exposure.switch(&mut document, false, true, &[never, freed, page]),
        0
    );
    assert!(!exposure.has_changes());
    assert!(!exposure.is_registered(never) && !exposure.is_registered(freed));
    assert!(!exposure.is_stopped(next) && exposure.is_exposed(a));
    assert_eq!(
        exposure.switch(&mut document, true, false, &[never, freed, page]),
        0
    );
    assert!(!exposure.has_changes());
}

// --- element lifetime ----------------------------------------------------

/// A freed element that was exposed sends nothing: its observer's drop
/// inside the free only lets the registration go, as there is no target to
/// dispatch a record at.
#[test]
fn a_freed_element_that_was_exposed_sends_nothing() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    listen(&mut exposure, &mut document, node, true);
    assert_eq!(deliver(&exposure, &mut document).len(), 1);

    // The collection path: a detached element whose handle was collected.
    document.remove_element(node);
    document.drop_element(node);
    assert!(!exposure.is_registered(node));
    assert!(!exposure.has_changes());
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

    // Freed while stopped, with the exposed record kept: its observer
    // stayed, so its drop lets the registration go at once, sending nothing
    // either, and the resume that lists it finds nothing of it.
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "b");
    assert_eq!(deliver(&exposure, &mut document).len(), 1);
    exposure.switch(&mut document, false, false, &[node]);
    document.remove_element(node);
    document.drop_element(node);
    assert!(!exposure.is_registered(node));
    assert!(!exposure.has_changes());
    exposure.switch(&mut document, true, false, &[node]);
    assert!(!exposure.has_changes());
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
    listen(&mut exposure, &mut document, node, true);
    assert_eq!(deliver(&exposure, &mut document).len(), 1);

    document.remove_element(node);
    assert!(!exposure.has_changes(), "nothing is owed before the update");
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![ExposureChange {
            element_event: true,
            ..record(node, false, "a")
        }]
    );
    assert!(exposure.is_registered(node));
}
