//! HTML's `<dialog>` ([`terminus::link::html::dialog`]) in the Lynx page: under
//! the Lynx UA sheet, with its `close` and `cancel` on the document's
//! [`ComponentEvents`] queue, which `new_document` registers as the
//! document's queued-event sink.

#![allow(clippy::float_cmp)] // Authored pixel sizes lay out exactly.

use terminus::link::html::dialog::{
    DIALOG_TAG, InvalidState, close, request_close, show, show_modal,
};
use terminus::link::stylo::properties::PropertyId;
use terminus::link::stylo::values::computed::{Display, Overflow};
use terminus::link::{MethodCall, MethodError, MethodOutcome, NodeId, Point2D};

use super::test_support::{
    child, display, document, element_under, overflow, style_of, with_component_events, with_config,
};
use super::{ComponentEvent, ComponentEvents, LynxDocument, PageConfig};

const VIEWPORT: (f32, f32) = (393.0, 727.0);

fn value(document: &LynxDocument, element: NodeId, property: &str) -> String {
    let id = PropertyId::parse_enabled_for_all_content(property)
        .unwrap_or_else(|()| panic!("unknown property `{property}`"));
    let declaration = id
        .as_shorthand()
        .err()
        .unwrap_or_else(|| panic!("`{property}` is a shorthand"));
    style_of(document, element).computed_value_to_string(declaration)
}

fn rect(document: &LynxDocument, element: NodeId) -> (f32, f32, f32, f32) {
    let rect = document
        .bounding_client_rect(element)
        .expect("a rendered element");
    (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
}

fn matches(document: &LynxDocument, element: NodeId, selector: &str) -> bool {
    document
        .matches(element, selector)
        .expect("a valid selector")
}

fn queued(events: &ComponentEvents) -> Vec<(NodeId, &'static str)> {
    events
        .take()
        .into_iter()
        .map(|event| match event {
            ComponentEvent::Plain { node, name } => (node, name),
            other => panic!("no such event here: {other:?}"),
        })
        .collect()
}

fn hits(document: &mut LynxDocument, x: f32, y: f32) -> Vec<NodeId> {
    document.commit();
    document.elements_from_point(Point2D::new(x, y))
}

#[test]
fn an_open_dialog_gets_html_s_ua_values_and_a_closed_one_no_box() {
    let mut document = document();
    let dialog = child(&mut document, DIALOG_TAG, "font-size: 10px");
    document.layout();
    assert_eq!(display(&document, dialog), Display::None, "no `open`");

    document.set_attribute(dialog, "open", "");
    document.layout();
    assert_eq!(display(&document, dialog), Display::Linear);
    for (property, expected) in [
        ("position", "absolute"),
        ("left", "0px"),
        ("right", "0px"),
        ("top", "auto"),
        ("bottom", "auto"),
        ("width", "fit-content"),
        ("height", "fit-content"),
        ("margin-top", "auto"),
        ("margin-left", "auto"),
        ("border-top-style", "solid"),
        ("border-top-width", "3px"),
        ("border-left-width", "3px"),
        ("padding-top", "10px"),
        ("padding-left", "10px"),
        ("box-sizing", "content-box"),
        ("background-color", "rgb(255, 255, 255)"),
        ("color", "rgb(0, 0, 0)"),
    ] {
        assert_eq!(value(&document, dialog, property), expected, "{property}");
    }
    assert_eq!(
        overflow(&document, dialog),
        (Overflow::Visible, Overflow::Visible)
    );
    assert!(matches(&document, dialog, ":open"));
    assert!(!matches(&document, dialog, ":modal"));

    document.set_attribute(dialog, "open", "false");
    document.layout();
    assert_eq!(display(&document, dialog), Display::None, "`\"false\"`");
    assert!(!matches(&document, dialog, ":open"));
    document.set_attribute(dialog, "open", "true");
    document.layout();
    assert_eq!(display(&document, dialog), Display::Linear);
    document.remove_attribute(dialog, "open");
    document.layout();
    assert_eq!(display(&document, dialog), Display::None);
}

#[test]
fn a_dialog_and_its_backdrop_lay_out_under_either_display_default() {
    for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
        let mut document = with_config(PageConfig {
            default_display_linear: linear,
            ..PageConfig::default()
        });
        let dialog = child(&mut document, DIALOG_TAG, "width: 100px; height: 50px");
        show_modal(&mut document, dialog).expect("a closed, connected dialog");
        document.layout();
        assert_eq!(display(&document, dialog), expected, "linear: {linear}");
        assert_eq!(
            hits(&mut document, 1.0, 1.0),
            vec![dialog],
            "the backdrop has a box"
        );
        close(&mut document, dialog);
        document.layout();
        assert_eq!(display(&document, dialog), Display::None);
    }
}

#[test]
fn a_modal_dialog_is_fixed_scrolls_and_stops_at_the_viewport() {
    let mut document = document();
    let dialog = child(&mut document, DIALOG_TAG, "font-size: 10px");
    element_under(
        &mut document,
        dialog,
        "view",
        "width: 1000px; height: 2000px; flex-shrink: 0",
    );
    show_modal(&mut document, dialog).expect("a closed, connected dialog");
    document.layout();
    for (property, expected) in [
        ("position", "fixed"),
        ("top", "0px"),
        ("bottom", "0px"),
        ("left", "0px"),
        ("right", "0px"),
    ] {
        assert_eq!(value(&document, dialog, property), expected, "{property}");
    }
    assert_eq!(
        overflow(&document, dialog),
        (Overflow::Scroll, Overflow::Scroll)
    );
    assert!(matches(&document, dialog, ":modal"));
    assert!(matches(&document, dialog, ":open"));
    assert_eq!(rect(&document, dialog), (0.0, 0.0, VIEWPORT.0, VIEWPORT.1));
    assert!(
        document.scroll_box(dialog).is_some(),
        "a modal dialog is a scroll container"
    );
}

#[test]
fn an_author_position_does_not_take_a_modal_dialog_out_of_the_viewport() {
    let mut document = document();
    let wrapper = child(&mut document, "view", "margin-top: 300px; width: 50px");
    let dialog = element_under(
        &mut document,
        wrapper,
        DIALOG_TAG,
        "position: relative; inset: 0; width: 101px; height: 51px; font-size: 10px",
    );
    show_modal(&mut document, dialog).expect("a closed, connected dialog");
    document.layout();
    assert_eq!(value(&document, dialog, "position"), "absolute");
    assert_eq!(rect(&document, dialog), (133.0, 325.0, 127.0, 77.0));
}

#[test]
fn a_display_contents_modal_dialog_is_blockified_and_renders() {
    let mut document = document();
    let dialog = child(
        &mut document,
        DIALOG_TAG,
        "display: contents; width: 101px; height: 51px; font-size: 10px",
    );
    show_modal(&mut document, dialog).expect("a closed, connected dialog");
    document.layout();
    assert_eq!(display(&document, dialog), Display::Flex);
    assert_eq!(rect(&document, dialog), (133.0, 325.0, 127.0, 77.0));
}

#[test]
fn show_and_show_modal_refuse_each_other_s_open_dialog() {
    let (mut document, events) = with_component_events(PageConfig::default());
    let dialog = child(&mut document, DIALOG_TAG, "");

    assert_eq!(show(&mut document, dialog), Ok(()));
    assert_eq!(
        document.get(dialog).unwrap().attribute("open").as_deref(),
        Some("")
    );
    assert!(!document.in_top_layer(dialog));
    assert_eq!(show(&mut document, dialog), Ok(()), "already open: no-op");
    assert_eq!(show_modal(&mut document, dialog), Err(InvalidState));
    assert!(!document.in_top_layer(dialog));

    close(&mut document, dialog);
    assert_eq!(
        document.get(dialog).unwrap().attribute("open").as_deref(),
        None
    );
    assert_eq!(queued(&events), vec![(dialog, "close")]);
    close(&mut document, dialog);
    request_close(&mut document, dialog);
    assert_eq!(queued(&events), vec![], "a closed dialog owes nothing");

    assert_eq!(show_modal(&mut document, dialog), Ok(()));
    assert!(document.blocks_document(dialog));
    assert_eq!(show_modal(&mut document, dialog), Ok(()), "already modal");
    assert_eq!(document.top_layer().count(), 1);
    assert_eq!(show(&mut document, dialog), Err(InvalidState));
    assert!(document.blocks_document(dialog));
    assert_eq!(queued(&events), vec![]);
}

#[test]
fn show_modal_refuses_a_disconnected_dialog() {
    let mut document = document();
    let dialog = document.create_element(DIALOG_TAG, ());
    assert_eq!(show_modal(&mut document, dialog), Err(InvalidState));
    assert_eq!(
        document.get(dialog).unwrap().attribute("open").as_deref(),
        None
    );
    assert!(!document.in_top_layer(dialog));
    assert_eq!(
        show(&mut document, dialog),
        Ok(()),
        "`show` has no such rule"
    );
}

#[test]
fn invoke_answers_the_four_methods_and_nothing_else() {
    let (mut document, events) = with_component_events(PageConfig::default());
    let dialog = child(&mut document, DIALOG_TAG, "");
    let invoke = |document: &mut LynxDocument, name: &str| {
        document.invoke_element_method(dialog, MethodCall { name, params: "{}" })
    };

    assert_eq!(invoke(&mut document, "showModal"), MethodOutcome::Done);
    assert!(document.blocks_document(dialog));
    assert_eq!(
        invoke(&mut document, "show"),
        MethodOutcome::Failed(MethodError::InvalidState)
    );
    assert_eq!(invoke(&mut document, "requestClose"), MethodOutcome::Done);
    assert_eq!(queued(&events), vec![(dialog, "cancel"), (dialog, "close")]);
    assert_eq!(invoke(&mut document, "show"), MethodOutcome::Done);
    assert_eq!(invoke(&mut document, "close"), MethodOutcome::Done);
    assert_eq!(queued(&events), vec![(dialog, "close")]);
    for name in ["boundingClientRect", "selectTab", "remove", ""] {
        assert_eq!(
            invoke(&mut document, name),
            MethodOutcome::NotFound,
            "{name}"
        );
    }
}

#[test]
fn request_close_queues_cancel_then_close_and_closes() {
    let (mut document, events) = with_component_events(PageConfig::default());
    let dialog = child(&mut document, DIALOG_TAG, "");
    show_modal(&mut document, dialog).expect("a closed, connected dialog");
    request_close(&mut document, dialog);
    assert_eq!(queued(&events), vec![(dialog, "cancel"), (dialog, "close")]);
    assert!(!document.in_top_layer(dialog));
    document.layout();
    assert!(!matches(&document, dialog, ":open"));
    assert!(!matches(&document, dialog, ":modal"));
}

#[test]
fn removing_open_by_hand_leaves_the_top_layer_without_an_event() {
    for removal in [None, Some("false")] {
        let (mut document, events) = with_component_events(PageConfig::default());
        let dialog = child(&mut document, DIALOG_TAG, "");
        show_modal(&mut document, dialog).expect("a closed, connected dialog");
        match removal {
            None => document.remove_attribute(dialog, "open"),
            Some(value) => document.set_attribute(dialog, "open", value),
        }
        assert!(!document.in_top_layer(dialog), "{removal:?}");
        document.layout();
        assert!(!matches(&document, dialog, ":modal"), "{removal:?}");
        assert!(!matches(&document, dialog, ":open"), "{removal:?}");
        assert_eq!(queued(&events), vec![], "{removal:?}");
        assert_eq!(show_modal(&mut document, dialog), Ok(()), "{removal:?}");
    }
}

#[test]
fn a_removed_modal_dialog_comes_back_open_and_not_modal() {
    let mut document = document();
    let dialog = child(&mut document, DIALOG_TAG, "");
    show_modal(&mut document, dialog).expect("a closed, connected dialog");
    document.remove_element(dialog);
    assert!(!document.in_top_layer(dialog));

    let page = document.document_element().id();
    document.append_child(page, dialog);
    document.layout();
    assert!(matches(&document, dialog, ":open"));
    assert!(!matches(&document, dialog, ":modal"));
    assert_eq!(value(&document, dialog, "position"), "absolute");
    assert_eq!(show(&mut document, dialog), Ok(()), "open and not modal");
    assert_eq!(show_modal(&mut document, dialog), Err(InvalidState));
}

#[test]
fn a_modal_dialog_centres_and_paints_above_everything_with_an_inert_page_below() {
    let mut document = document();
    let page = document.document_element().id();
    let cover = child(
        &mut document,
        "view",
        "position: absolute; z-index: 100; left: 0; top: 0; width: 393px; height: 727px",
    );
    let ancestor = child(
        &mut document,
        "view",
        "transform: translate(30px, 40px) scale(0.5); overflow: clip; opacity: 0.5; \
         width: 20px; height: 20px",
    );
    let dialog = element_under(
        &mut document,
        ancestor,
        DIALOG_TAG,
        "width: 101px; height: 51px; font-size: 10px",
    );
    document.layout();
    assert_eq!(hits(&mut document, 200.0, 360.0), vec![cover, page]);

    show_modal(&mut document, dialog).expect("a closed, connected dialog");
    document.layout();
    assert_eq!(rect(&document, dialog), (133.0, 325.0, 127.0, 77.0));
    assert_eq!(
        hits(&mut document, 200.0, 360.0),
        vec![dialog],
        "the dialog"
    );
    for (x, y) in [(1.0, 1.0), (392.0, 726.0), (20.0, 360.0)] {
        assert_eq!(
            hits(&mut document, x, y),
            vec![dialog],
            "({x}, {y}): the backdrop reports its dialog; nothing below, the page \
             included, is hit"
        );
    }
    let frame = document.commit();
    let hit = frame
        .hit(Point2D::new(200.0, 360.0), &|_| None, None)
        .expect("the dialog is hit");
    assert_eq!(hit.node, dialog);
    assert_eq!(
        hit.scroll,
        frame.slot_of(dialog),
        "a touch on the dialog scrolls the dialog"
    );

    close(&mut document, dialog);
    show(&mut document, dialog).expect("a closed dialog");
    document.layout();
    assert_eq!(hits(&mut document, 1.0, 1.0), vec![cover, page]);
}

#[test]
fn a_modal_dialog_with_no_size_shrinks_to_its_content_and_centres() {
    let mut document = document();
    let dialog = child(&mut document, DIALOG_TAG, "font-size: 10px");
    element_under(
        &mut document,
        dialog,
        "view",
        "width: 101px; height: 51px; flex-shrink: 0",
    );
    show_modal(&mut document, dialog).expect("a closed, connected dialog");
    document.layout();
    assert_eq!(rect(&document, dialog), (133.0, 325.0, 127.0, 77.0));
}
