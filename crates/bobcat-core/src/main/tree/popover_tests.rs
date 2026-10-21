//! HTML's popover UA rules ([`terminus::link::html::popover`]) in the Lynx page:
//! under the Lynx UA sheet, against the Lynx tags' own rules, and with
//! HTML's `<dialog>` ([`terminus::link::html::dialog`]).

#![allow(clippy::float_cmp)] // Authored pixel sizes lay out exactly.

use terminus::link::html::dialog::{DIALOG_TAG, InvalidState, close, show, show_modal};
use terminus::link::stylo::properties::PropertyId;
use terminus::link::stylo::values::computed::Display;
use terminus::link::{NodeId, Point2D, PopoverError, StylesheetOrigin};

use super::test_support::{child, display, document, element_under, style_of, with_config};
use super::{LynxDocument, PageConfig};

/// One computed longhand, as CSSOM serializes it.
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

/// The elements at `point` in the committed frame, front to back.
fn hits(document: &mut LynxDocument, x: f32, y: f32) -> Vec<NodeId> {
    document.layout();
    document.commit();
    document.elements_from_point(Point2D::new(x, y))
}

/// A manual popover `dialog` under the page, holding a 100 by 50 box.
fn popover_dialog(document: &mut LynxDocument) -> NodeId {
    let dialog = child(document, DIALOG_TAG, "font-size: 10px");
    document.set_attribute(dialog, "popover", "manual");
    element_under(
        document,
        dialog,
        "view",
        "width: 100px; height: 50px; flex-shrink: 0",
    );
    dialog
}

// --- HTML's rules ---------------------------------------------------------

/// A hidden popover generates no box; a showing one gets HTML's
/// `[popover]` box — fixed, centred at its fit-content size, a solid
/// medium border, `0.25em` of padding, `Canvas`/`CanvasText` — on the
/// top layer, with the display `defaultDisplayLinear` picks for
/// `dialog`. Its 100 by 50 content, 2.5px of padding and 3px of border
/// make 111 by 61, centred in the 393 by 727 viewport.
#[test]
fn a_showing_popover_dialog_gets_html_s_popover_box_on_the_top_layer() {
    for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
        let mut document = with_config(PageConfig {
            default_display_linear: linear,
            ..PageConfig::default()
        });
        let dialog = popover_dialog(&mut document);
        document.layout();
        assert_eq!(
            display(&document, dialog),
            Display::None,
            "linear: {linear}"
        );

        document
            .show_popover(dialog)
            .expect("a connected manual popover");
        document.layout();
        assert_eq!(display(&document, dialog), expected, "linear: {linear}");
        for (property, value_) in [
            ("position", "fixed"),
            ("top", "0px"),
            ("left", "0px"),
            ("width", "fit-content"),
            ("margin-top", "auto"),
            ("border-top-style", "solid"),
            ("border-top-width", "3px"),
            ("padding-top", "2.5px"),
            ("overflow-y", "scroll"),
            ("background-color", "rgb(255, 255, 255)"),
            ("color", "rgb(0, 0, 0)"),
        ] {
            assert_eq!(value(&document, dialog, property), value_, "{property}");
        }
        assert_eq!(rect(&document, dialog), (141.0, 333.0, 111.0, 61.0));
        assert!(document.in_top_layer(dialog));
        assert!(!document.blocks_document(dialog));

        document.hide_popover(dialog).expect("a showing popover");
        document.layout();
        assert_eq!(
            display(&document, dialog),
            Display::None,
            "linear: {linear}"
        );
    }
}

/// A showing popover paints above a `z-index: 100` sibling whatever its
/// ancestors, and makes nothing inert: its `::backdrop` covers the
/// viewport and takes no touch, so a point outside the popover hits what
/// is below.
#[test]
fn a_popover_s_backdrop_takes_no_touch_and_nothing_below_is_inert() {
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
        "transform: translate(30px, 40px) scale(0.5); overflow: clip; width: 20px; height: 20px",
    );
    let dialog = element_under(&mut document, ancestor, DIALOG_TAG, "font-size: 10px");
    document.set_attribute(dialog, "popover", "manual");
    let content = element_under(
        &mut document,
        dialog,
        "view",
        "width: 100px; height: 50px; flex-shrink: 0",
    );
    document
        .show_popover(dialog)
        .expect("a connected manual popover");
    assert_eq!(
        hits(&mut document, 200.0, 360.0),
        vec![content, dialog, cover, page],
        "above the cover, and the cover below is not inert"
    );
    assert_eq!(hits(&mut document, 1.0, 1.0), vec![cover, page]);
}

/// A Lynx tag keeps what web-elements' author rules give it — here
/// `display`, `position: relative`, no border and the overflow
/// `defaultOverflowVisible` releases — as it does under web-core,
/// where those rules outrank the browser's: a `<view popover>` is not
/// hidden. What web-elements leaves alone is the popover's.
#[test]
fn a_lynx_tag_keeps_its_own_rules_over_html_s_popover_rules() {
    let mut document = document();
    let view = child(&mut document, "view", "");
    document.set_attribute(view, "popover", "");
    document.layout();
    assert_eq!(display(&document, view), Display::Linear);
    for (property, expected) in [
        ("position", "relative"),
        ("border-top-width", "0px"),
        ("overflow-y", "visible"),
        ("padding-top", "4px"),
        ("margin-top", "auto"),
        ("width", "fit-content"),
        ("background-color", "rgb(255, 255, 255)"),
    ] {
        assert_eq!(value(&document, view, property), expected, "{property}");
    }
}

/// An author rule wins over every one of them, as over a browser's UA
/// sheet, except the backdrop's `!important`.
#[test]
fn author_rules_win_but_the_backdrop_stays_untouchable() {
    let mut document = document();
    document.add_stylesheet(
        "dialog { padding: 7px; background-color: rgb(1, 2, 3); } \
         dialog::backdrop { pointer-events: auto !important; background-color: red; }",
        StylesheetOrigin::Author,
    );
    let page = document.document_element().id();
    let dialog = popover_dialog(&mut document);
    document
        .show_popover(dialog)
        .expect("a connected manual popover");
    document.layout();
    assert_eq!(value(&document, dialog, "padding-top"), "7px");
    assert_eq!(value(&document, dialog, "background-color"), "rgb(1, 2, 3)");
    assert_eq!(hits(&mut document, 1.0, 1.0), vec![page]);
}

// --- with `dialog` ---------------------------------------------------------

/// HTML's `showModal()` throws `InvalidStateError` on a dialog that is
/// showing as a popover, and *show popover* on a modal one.
#[test]
fn a_dialog_is_modal_or_a_popover_never_both() {
    let mut document = document();
    let dialog = popover_dialog(&mut document);
    document
        .show_popover(dialog)
        .expect("a connected manual popover");
    assert_eq!(show_modal(&mut document, dialog), Err(InvalidState));
    assert!(!document.blocks_document(dialog));
    document.hide_popover(dialog).expect("a showing popover");

    show_modal(&mut document, dialog).expect("a closed, connected dialog");
    assert_eq!(
        document.show_popover(dialog),
        Err(PopoverError::InvalidState)
    );
    assert!(!document.popover_showing(dialog));
}

/// A popover `dialog` shown modally is a modal dialog with `[popover]`'s
/// box, as in HTML: `dialog:modal`'s values (the one rule ordered before
/// `[popover]` here, which writes the same values) and `[popover]`'s
/// padding over `dialog`'s. Its 100 by 50 content, 2.5px of padding and
/// 3px of border make 111 by 61, centred, and the page below is inert.
#[test]
fn a_popover_dialog_shown_modally_is_a_modal_dialog_with_the_popover_box() {
    let mut document = document();
    let page = document.document_element().id();
    let dialog = popover_dialog(&mut document);
    show_modal(&mut document, dialog).expect("a closed, connected dialog");
    document.layout();
    assert_eq!(display(&document, dialog), Display::Linear);
    for (property, expected) in [
        ("position", "fixed"),
        ("top", "0px"),
        ("bottom", "0px"),
        ("left", "0px"),
        ("overflow-y", "scroll"),
        ("padding-top", "2.5px"),
        ("border-top-width", "3px"),
    ] {
        assert_eq!(value(&document, dialog, property), expected, "{property}");
    }
    assert!(
        document
            .matches(dialog, ":modal")
            .expect("a valid selector")
    );
    assert_eq!(rect(&document, dialog), (141.0, 333.0, 111.0, 61.0));
    assert_eq!(
        hits(&mut document, 1.0, 1.0),
        vec![dialog],
        "{page:?} is inert"
    );
}

/// A popover `dialog` can also be opened with `show()`, which HTML does
/// not refuse; closing it then leaves the popover showing and in the top
/// layer, because `close()` takes a dialog out of the layer only when it
/// is modal.
#[test]
fn closing_a_dialog_shown_as_a_popover_leaves_the_popover_showing() {
    let mut document = document();
    let dialog = popover_dialog(&mut document);
    document
        .show_popover(dialog)
        .expect("a connected manual popover");
    show(&mut document, dialog).expect("a closed, non-modal dialog");
    document.layout();
    assert!(document.matches(dialog, ":open").expect("a valid selector"));
    close(&mut document, dialog);
    document.layout();
    assert!(document.popover_showing(dialog));
    assert!(document.in_top_layer(dialog));
    assert_ne!(display(&document, dialog), Display::None);
}
