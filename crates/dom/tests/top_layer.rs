//! The style half of the top layer (css-position-4 §3, `dom::tree::top_layer`)
//! seen through the public API: `:modal` / `:open`, §3.1's computed-value
//! fixups (which the fork cannot switch on, so membership stands in for
//! them), and the displays a top-layer box lowers to.
//!
//! The `::backdrop` style itself is never handed out, so its cascade is
//! tested beside it (`crates/dom/src/tree/top_layer.rs`); its geometry and
//! paint order are in `tests/layout.rs` and `src/visual/tests.rs`.

mod common;

use common::Doc;
use dom::ElementState;

/// What bobcat-core's UA sheet gives a modal `<dialog>` and `::backdrop`,
/// reduced to what these tests read.
const UA: &str = "
    dialog { display: flex; }
    ::backdrop { display: flex; position: fixed; inset: 0; }";

fn page(css: &str) -> Doc {
    let mut doc = Doc::with_css(&format!(
        "page {{ display: flex; width: 800px; height: 600px; }} {css}"
    ));
    doc.add_ua_css(UA);
    doc
}

#[test]
fn modal_and_open_match_their_state_bits_and_restyle_on_change() {
    let mut doc = page(
        "dialog:open { width: 10px; }
         dialog:modal { height: 20px; }
         dialog:open:modal { color: rgb(1, 2, 3); }",
    );
    let dialog = doc.el(doc.root, "dialog");
    doc.flush();
    assert!(!doc.matches(dialog, ":open"));
    assert!(!doc.matches(dialog, ":modal"));
    assert_eq!(doc.value(dialog, "width"), "auto");

    doc.dom.add_element_state(dialog, ElementState::OPEN);
    doc.flush();
    assert!(doc.matches(dialog, ":open"));
    assert!(!doc.matches(dialog, ":modal"));
    assert_eq!(doc.value(dialog, "width"), "10px");
    assert_eq!(doc.value(dialog, "height"), "auto");

    doc.dom.add_element_state(dialog, ElementState::MODAL);
    doc.flush();
    assert!(doc.matches(dialog, ":modal"));
    assert_eq!(doc.value(dialog, "height"), "20px");
    assert_eq!(doc.value(dialog, "color"), "rgb(1, 2, 3)");

    doc.dom
        .remove_element_state(dialog, ElementState::OPEN | ElementState::MODAL);
    doc.flush();
    assert!(!doc.matches(dialog, ":open"));
    assert_eq!(doc.value(dialog, "width"), "auto");
    assert_eq!(doc.value(dialog, "height"), "auto");
}

/// `-servo-top-layer: auto` is meant to be UA-only and to make Stylo's
/// adjuster compute a top-layer element's `relative` position to `absolute`
/// (css-position-4 §3.1). The fork's `lynx` build keeps the longhand out of
/// its property-name table, so even a UA sheet's declaration is dropped.
#[test]
#[ignore = "BLOCKED (stylo fork): `-servo-top-layer` is in LYNX_INTERNAL_LONGHANDS \
            (vendor/stylo/style/properties/data.py) — compiled, but absent from the \
            property-name table, so no sheet can declare it and \
            StyleAdjuster::adjust_for_top_layer never runs"]
fn the_ua_top_layer_longhand_computes_a_relative_position_to_absolute() {
    let mut doc = page("dialog { position: relative; }");
    doc.add_ua_css("dialog.modal { -servo-top-layer: auto; }");
    let dialog = doc.el(doc.root, "dialog.modal");
    doc.dom.add_to_top_layer(dialog, true);
    doc.flush();
    assert_eq!(doc.value(dialog, "position"), "absolute");
}

/// What membership stands in for while the fixup cannot run: a top-layer
/// element whose position computes to `relative` (or `static`) is still
/// placed against the viewport and still generates the containing block of
/// its absolutely positioned descendants, as a computed `absolute` would.
#[test]
fn membership_stands_in_for_the_position_fixup() {
    let mut doc = page(
        ".t { display: flex; transform: translate(1px, 1px); margin-left: 40px;
              width: 100px; height: 100px; }
         dialog { position: static; margin-left: 7px; width: 50px; height: 50px; }
         .abs { display: flex; position: absolute; left: 3px; top: 4px;
                width: 5px; height: 5px; }",
    );
    let t = doc.el(doc.root, "view.t");
    let dialog = doc.el(t, "dialog");
    let abs = doc.el(dialog, "view.abs");
    doc.dom.add_to_top_layer(dialog, true);
    doc.flush();
    assert_eq!(doc.value(dialog, "position"), "static", "no fixup runs");
    let rect = |doc: &Doc, id| {
        let rect = doc.dom.bounding_client_rect(id).expect("rendered");
        (
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        )
    };
    assert_eq!(rect(&doc, dialog), (7.0, 0.0, 50.0, 50.0));
    assert_eq!(rect(&doc, abs), (10.0, 4.0, 5.0, 5.0));
}

/// `display_mode` panics on a display this engine does not lay out, so
/// every box the top layer creates must compute to one it does. A
/// `display: contents` top-layer element is not blockified (the fixup
/// cannot run): it generates no box and renders nothing, without a panic.
#[test]
fn top_layer_boxes_compute_to_displays_the_engine_lays_out() {
    let mut doc = page(".contents { display: contents; }");
    let dialog = doc.el(doc.root, "dialog");
    let contents = doc.el(doc.root, "dialog.contents");
    doc.dom.add_to_top_layer(dialog, true);
    doc.dom.add_to_top_layer(contents, false);
    doc.flush();
    assert_eq!(doc.value(dialog, "display"), "flex");
    assert_eq!(doc.value(contents, "display"), "contents");
    assert!(doc.dom.render(), "lays out and paints without a panic");
    assert_eq!(doc.dom.bounding_client_rect(contents), None);
    assert!(doc.dom.in_top_layer(contents));
    assert!(doc.dom.blocks_document(dialog));
}

#[test]
fn the_public_top_layer_api_reports_order_and_flags() {
    let mut doc = page("");
    let first = doc.el(doc.root, "dialog");
    let second = doc.el(doc.root, "dialog");
    doc.dom.add_to_top_layer(first, true);
    doc.dom.add_to_top_layer(second, false);
    let entries: Vec<_> = doc
        .dom
        .top_layer()
        .map(|entry| (entry.element(), entry.blocks_document()))
        .collect();
    assert_eq!(entries, vec![(first, true), (second, false)]);
    assert!(doc.dom.blocks_document(first));
    assert!(!doc.dom.blocks_document(second));
    assert_eq!(
        doc.dom.backdrop_origin(first),
        None,
        "an element is no backdrop"
    );

    doc.dom.remove_from_top_layer(first);
    doc.dom.remove_from_top_layer(first);
    assert_eq!(doc.dom.top_layer().count(), 1);
    // Selector queries never see a backdrop node.
    assert_eq!(
        doc.dom
            .query_selector_all(doc.root, "*")
            .expect("parses")
            .len(),
        2
    );
}
