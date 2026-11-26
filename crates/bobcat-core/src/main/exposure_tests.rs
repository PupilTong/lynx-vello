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

use std::rc::Rc;

use terminus::link::{IntersectionMargin, MarginLength, NodeId, Vector2D};

use super::{Exposure, ExposureChange, Length, Margins, fold_margins, parse_area, parse_length};
use crate::main::tree::test_support::{
    child, document as fresh_document, element_under, with_config,
};
use crate::main::tree::{LynxDocument, PageConfig};

const VIEWPORT_WIDTH: f32 = 393.0;
const VIEWPORT_HEIGHT: f32 = 727.0;

fn boxed(document: &mut LynxDocument, top: f32, height: f32) -> NodeId {
    child(
        document,
        "view",
        &format!("position:absolute;left:0;top:{top}px;width:100px;height:{height}px"),
    )
}

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

fn remove(exposure: &mut Exposure, document: &mut LynxDocument, node: NodeId, name: &str) {
    document.remove_attribute(node, name);
    exposure.attribute_changed(document, node);
    follow(exposure, document, node, None);
}

fn listen(exposure: &mut Exposure, document: &mut LynxDocument, node: NodeId, on: bool) {
    follow(exposure, document, node, Some(on));
}

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

fn deliver(exposure: &Exposure, document: &mut LynxDocument) -> Vec<ExposureChange> {
    document.render();
    document.update_intersection_observations(0.0);
    document.notify_intersection_observers();
    exposure.take_changes()
}

fn record(node: NodeId, appeared: bool, id: &str) -> ExposureChange {
    ExposureChange {
        node,
        appeared,
        exposure_id: Some(Rc::from(id)),
        scene: None,
        element_event: false,
    }
}

fn root_margin(exposure: &Exposure, document: &LynxDocument, node: NodeId) -> IntersectionMargin {
    let observer = exposure
        .observer_of(node)
        .expect("the element is registered and detection runs");
    *document.intersection_observer(observer).root_margin()
}

fn target_margin(exposure: &Exposure, document: &LynxDocument, node: NodeId) -> IntersectionMargin {
    let observer = exposure
        .observer_of(node)
        .expect("the element is registered and detection runs");
    *document.intersection_observer(observer).target_margin()
}

fn margin(sides: impl FnOnce(&mut IntersectionMargin)) -> IntersectionMargin {
    let mut margin = IntersectionMargin::ZERO;
    sides(&mut margin);
    margin
}

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
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

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

#[test]
fn screen_margins_grow_and_shrink_the_screen() {
    grow_and_shrink_the_screen(fresh_document());
}

#[test]
fn screen_margins_reach_past_a_clipping_page() {
    grow_and_shrink_the_screen(with_config(PageConfig {
        default_overflow_visible: false,
        ..PageConfig::default()
    }));
}

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

#[test]
fn screen_margins_reach_past_a_scrollport() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let scroller = child(
        &mut document,
        "scroll-view",
        "position:absolute;left:0;top:0;width:100px;height:100px",
    );
    let [_, node, _] = [140.0, 20.0, 100.0].map(|height| {
        element_under(
            &mut document,
            scroller,
            "view",
            &format!("flex-shrink:0;width:100px;height:{height}px"),
        )
    });
    set(&mut exposure, &mut document, node, "exposure-id", "a");
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

#[test]
fn margins_fold_onto_the_target_and_the_root() {
    let px = |px| MarginLength::Px(px);
    let percent = |percent| MarginLength::Percent(percent);
    let zero = [Length::Px(0.0); 4];

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
            exposure_id: Some(Rc::from("a")),
            scene: Some(Rc::from("feed")),
            element_event: false,
        }]
    );
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
}

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
        scene: None,
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
            exposure_id: Some(Rc::from("a")),
            scene: Some(Rc::from("feed")),
            element_event: true,
        }]
    );

    listen(&mut exposure, &mut document, node, false);
    assert!(exposure.is_registered(node));
    assert_eq!(deliver(&exposure, &mut document), Vec::new());
}

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

fn move_to(document: &mut LynxDocument, node: NodeId, top: f32) {
    document.set_inline_style(
        node,
        &format!("position:absolute;left:0;top:{top}px;width:100px;height:100px"),
    );
}

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

    for node in [a, b, c] {
        move_to(&mut document, node, 1000.0);
    }
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![record(c, false, "c")]
    );

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
            scene: None,
            element_event: true,
        }]
    );

    assert_eq!(exposure.switch(&mut document, false, false, &[a, quiet]), 0);
    assert!(!exposure.has_changes());
    assert_eq!(exposure.switch(&mut document, false, true, &[a, quiet]), 1);
    assert_eq!(exposure.take_changes(), vec![record(a, false, "a")]);
    assert!(!exposure.is_exposed(quiet));

    assert_eq!(exposure.switch(&mut document, false, true, &[c]), 0);
    assert!(!exposure.has_changes());
    assert!(
        exposure.is_exposed(b),
        "an element not listed keeps its record"
    );
}

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

#[test]
fn a_freed_element_that_was_exposed_sends_nothing() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let node = boxed(&mut document, 0.0, 100.0);
    set(&mut exposure, &mut document, node, "exposure-id", "a");
    listen(&mut exposure, &mut document, node, true);
    assert_eq!(deliver(&exposure, &mut document).len(), 1);

    document.remove_element(node);
    document.drop_element(node);
    assert!(!exposure.is_registered(node));
    assert!(!exposure.has_changes());
    assert_eq!(deliver(&exposure, &mut document), Vec::new());

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

#[test]
fn one_observer_per_option_set_and_one_delivery_groups_by_it() {
    let mut document = fresh_document();
    let mut exposure = Exposure::new();
    let x = boxed(&mut document, 0.0, 100.0);
    let y = boxed(&mut document, 100.0, 100.0);
    let z = boxed(&mut document, 200.0, 100.0);
    for (node, id, area) in [(x, "x", Some("50%")), (y, "y", None), (z, "z", Some("50"))] {
        if let Some(area) = area {
            document.set_attribute(node, "exposure-area", area);
        }
        set(&mut exposure, &mut document, node, "exposure-id", id);
    }
    assert_eq!(exposure.observer_of(x), exposure.observer_of(z));
    assert_ne!(exposure.observer_of(x), exposure.observer_of(y));
    assert_eq!(
        deliver(&exposure, &mut document),
        vec![
            record(x, true, "x"),
            record(z, true, "z"),
            record(y, true, "y")
        ]
    );
}
