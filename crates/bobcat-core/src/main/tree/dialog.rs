//! The `dialog` tag: HTML's `<dialog>` element (interactive-elements.html,
//! "The dialog element") over `dom`'s top layer (css-position-4 §3,
//! `dom::tree::top_layer`).
//!
//! A W3C element, so `AGENTS.md`'s standards policy puts it in the first
//! bucket: web-core runs in a browser, where a card that writes `<dialog>`
//! gets the browser's `HTMLDialogElement` — web-elements' `x-overlay-ng` is
//! itself a `<dialog>` opened with `showModal()` (`htmlTemplates.ts:136-172`).
//! Native Lynx has no dialog and no top layer: its `<overlay>` is a zero-sized
//! box whose content is reparented into a platform window. This module
//! implements HTML's behaviour, reduced where the engine has no counterpart,
//! as listed below.
//!
//! # The UA rules
//!
//! [`UA_RULES`] is HTML's UA sheet for the element (rendering.html, "Flow
//! content") adapted to this engine:
//!
//! - **`display`.** HTML gives `dialog` `block`, which no box here lowers to. It gets the display
//!   `defaultDisplayLinear` picks for every container — `linear` when it is on, the fork's initial
//!   `flex` when it is off — in [`super::ua_sheet`]'s display line, so both lay out. It is not in
//!   the container-defaults block (`border-box`, `position: relative`, `overflow: clip`, …): a
//!   browser gives `<dialog>` HTML's defaults, and so does this sheet. An author `display:
//!   contents` on a modal dialog is blockified by §3.1's fixup (below) to the fork's internal
//!   block-flow display, which `dom` does not lower either: `dom::layout::style::display_mode`
//!   panics on it. Which display it should lower to is open (the ignored
//!   `a_display_contents_modal_dialog_is_blockified_and_renders`).
//! - **Closed is `display: none`.** `dialog:not([open])` as in HTML, plus `dialog[open="false"]`:
//!   `open` is a boolean attribute, this engine's `__SetAttribute` stringifies `false`, and
//!   web-core removes an attribute whose value is `"false"` before its CSS sees it (see
//!   [`super::swiper`]'s "Boolean attributes and `"false"`"). The component reads it the same way.
//! - **`dialog:modal`.** HTML writes `overflow: auto` and `inset-block: 0`. `overflow: auto` is
//!   deliberately out of this engine and the fork disables `inset-block`, so the rule writes
//!   `overflow: scroll` and `top: 0; bottom: 0`. HTML's `position: fixed` is kept. The rule also
//!   declares `-servo-top-layer: auto`, a UA-only longhand (an author or user sheet drops it) that
//!   switches on Stylo's `StyleAdjuster::adjust_for_top_layer`, css-position-4 §3.1's
//!   computed-value fixups: a `position` other than `absolute`/`fixed` computes to `absolute`, as
//!   in a browser, and `display: contents` is blockified. Membership remains `dom`'s truth for what
//!   the computed value does not carry: paint order, the backdrop, inertness and the initial
//!   containing block (`dom::tree::top_layer`).
//! - **Colours.** HTML's `background-color: Canvas; color: CanvasText`. Both compute through
//!   Stylo's Servo device (`device/servo.rs`, `system_color`) under the light colour scheme — this
//!   engine's device has no other — to `rgb(255, 255, 255)` and `rgb(0, 0, 0)`.
//! - **`::backdrop`.** HTML's `position: fixed; inset: 0`, plus `-servo-top-layer: auto` as on
//!   `dialog:modal`, and `display: flex`, because the pseudo-element's style has no other source of
//!   a display `dom` lowers. `dialog::backdrop` is HTML's `rgba(0, 0, 0, 0.1)`.
//!
//! No rule is `!important`. HTML's `width: fit-content; height: fit-content;
//! margin: auto` centres a modal dialog by shrink-to-fit sizing, and so it
//! does here: hughie's absolute pass stretch-fits only an `auto` size
//! between two insets (css-position-3 §4.1), so a dialog with no author size
//! takes its fit-content size (css-sizing-3 §3.2) in the viewport and its
//! `auto` margins centre it.
//!
//! # State
//!
//! The component ([`Dialog`]) keeps no Rust state per element. HTML's two facts
//! about a dialog are its `open` attribute and its "is modal" flag:
//!
//! - **Open** is the attribute, present and not `"false"`. The attribute callback is the one path
//!   that sets or clears `ElementState::OPEN` (`:open`), whoever wrote the attribute — script
//!   through `__SetAttribute` or a method below. Removing the attribute (or writing `"false"`) also
//!   takes the dialog out of the top layer and clears `ElementState::MODAL`, without a `close`
//!   event: HTML's attribute change steps for `open` fire none, and the dialog simply stops
//!   rendering.
//! - **Modal** is top-layer membership with the blocks-the-document flag,
//!   [`dom::Document::blocks_document`]. `ElementState::MODAL` (`:modal`) follows it. One case
//!   needs the component's help: a modal dialog removed from the document leaves the top layer in
//!   `dom`'s unlink path (HTML's removing steps), where no callback can mutate
//!   (`disconnected_callback` takes `&Document`). A disconnected element has no style, so the stale
//!   bit is invisible until the element is connected again, and [`Dialog::connected_callback`]
//!   clears it there: a re-inserted dialog is open and not modal, as in a browser.
//!
//! # Methods
//!
//! [`show`], [`show_modal`], [`close`] and [`request_close`] are HTML's
//! `show()`, `showModal()`, `close()` and `requestClose()`, reached through
//! `invoke` / `__InvokeUIMethod` as UI methods (`callElementMethod` in
//! `main/runtime`). Each sets or removes the `open` attribute and lets the
//! attribute callback do the state work, so there is one path for each fact.
//!
//! - `show()`: no-op on an open non-modal dialog; [`InvalidState`] on a modal one; otherwise adds
//!   `open`.
//! - `showModal()`: no-op on a modal dialog; [`InvalidState`] on an open non-modal one and on a
//!   disconnected one; otherwise adds `open`, enters the top layer blocking the document, and sets
//!   `:modal`.
//! - `close()`: no-op on a closed dialog; otherwise removes `open` and queues `close`.
//! - `requestClose()`: no-op on a closed dialog; otherwise queues `cancel`, then closes as
//!   `close()` does.
//!
//! [`InvalidState`] is HTML's `InvalidStateError`. The runtime answers it with
//! the UI-method status 4, `PARAM_INVALID`, which is what web-core's `invoke`
//! reports for any method that throws
//! (`web-core/ts/client/mainthread/elementAPIs/createInvokeUIMethod.ts:12-44`).
//! Native's table has a distinct `7 INVALID_STATE_ERROR`
//! (`lynx/core/renderer/dom/lynx_get_ui_result.h:53-61`); web-core is followed
//! (`docs/tracking/deviations.md`). The methods' `params` are not read:
//! `returnValue` has no reader in Lynx JS, so `close(returnValue)` drops it.
//!
//! # Events
//!
//! `close` and `cancel` are queued on the document's [`ComponentEvents`]
//! beside an image's `load` and `error`, and the page delivers them from an
//! entry of its own, non-bubbling, with a `{}` detail — HTML queues `close` as
//! a task too. `cancel` is not cancelable: this engine's event model has no
//! `preventDefault`, so `requestClose()` always closes.
//!
//! # What is not implemented
//!
//! - Close requests (Escape, the back gesture), `closedby` and light dismiss: no keyboard input, no
//!   close watcher, `closedby` is not parsed.
//! - `beforetoggle` / `toggle`, the focusing steps, `autofocus`, `returnValue`, popovers and
//!   fullscreen.
//! - The `overlay` property and the pending top-layer removals: leaving the top layer is immediate.
//!   `::backdrop` animations and transitions do not run (the lazy pseudo-element cascade carries no
//!   animation declarations).

use dom::{CustomElement, ElementState, NodeId};

use super::{ComponentEvents, LynxDocument};

/// HTML's tag.
pub(super) const DIALOG_TAG: &str = "dialog";
/// The boolean attribute that says the dialog is showing.
const OPEN_ATTRIBUTE: &str = "open";
/// The event [`close`] queues.
const CLOSE_EVENT: &str = "close";
/// The event [`request_close`] queues ahead of `close`.
const CANCEL_EVENT: &str = "cancel";

/// HTML's UA rules for `dialog` and `::backdrop`, adapted as the module
/// documentation lists. The display `dialog` gets otherwise is
/// [`super::ua_sheet`]'s.
pub(super) const UA_RULES: &str = r#"
dialog:not([open]), dialog[open="false"] { display: none; }
dialog {
  position: absolute;
  inset-inline-start: 0; inset-inline-end: 0;
  width: fit-content; height: fit-content;
  margin: auto;
  border: solid;
  padding: 1em;
  background-color: Canvas; color: CanvasText;
}
dialog:modal {
  -servo-top-layer: auto;
  position: fixed;
  overflow: scroll;
  top: 0; bottom: 0;
  max-width: calc(100% - 6px - 2em);
  max-height: calc(100% - 6px - 2em);
}
::backdrop { -servo-top-layer: auto; position: fixed; inset: 0; display: flex; }
dialog::backdrop { background: rgba(0, 0, 0, 0.1); }
"#;

/// Installs the `dialog` component. Must run before any element could carry
/// the tag, which is [`Document::define`](dom::Document::define)'s own
/// precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(DIALOG_TAG, Box::new(Dialog));
}

/// The `dialog` component: keeps `:open` and `:modal` and top-layer
/// membership in step with the `open` attribute.
struct Dialog;

impl CustomElement<()> for Dialog {
    fn observed_attributes(&self) -> Vec<String> {
        vec![OPEN_ATTRIBUTE.to_owned()]
    }

    /// Clears a `:modal` left behind by a removal: the unlink took the
    /// element out of the top layer, and HTML's removing steps set "is modal"
    /// to false.
    fn connected_callback(&self, document: &mut LynxDocument, element: NodeId) {
        if !document.blocks_document(element) {
            set_state(document, element, ElementState::MODAL, false);
        }
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        debug_assert_eq!(name, OPEN_ATTRIBUTE, "`dialog` observes `open` alone");
        if opens(new) {
            set_state(document, element, ElementState::OPEN, true);
        } else {
            set_state(document, element, ElementState::OPEN, false);
            document.remove_from_top_layer(element);
            set_state(document, element, ElementState::MODAL, false);
        }
    }
}

/// HTML's `InvalidStateError`: `show()` on a modal dialog, `showModal()` on
/// an open non-modal or a disconnected one.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvalidState;

/// Whether `node` is a `dialog`.
pub(crate) fn is_dialog(document: &LynxDocument, node: NodeId) -> bool {
    document
        .get(node)
        .and_then(dom::Node::tag_name)
        .is_some_and(|tag| tag == DIALOG_TAG)
}

/// HTML's `show()`.
pub(crate) fn show(document: &mut LynxDocument, dialog: NodeId) -> Result<(), InvalidState> {
    if is_open(document, dialog) {
        return if document.blocks_document(dialog) {
            Err(InvalidState)
        } else {
            Ok(())
        };
    }
    document.set_attribute(dialog, OPEN_ATTRIBUTE, "");
    Ok(())
}

/// HTML's `showModal()`.
pub(crate) fn show_modal(document: &mut LynxDocument, dialog: NodeId) -> Result<(), InvalidState> {
    if is_open(document, dialog) {
        return if document.blocks_document(dialog) {
            Ok(())
        } else {
            Err(InvalidState)
        };
    }
    if !document.is_connected(dialog) {
        return Err(InvalidState);
    }
    document.set_attribute(dialog, OPEN_ATTRIBUTE, "");
    document.add_to_top_layer(dialog, true);
    set_state(document, dialog, ElementState::MODAL, true);
    Ok(())
}

/// HTML's `close()`: closes an open dialog and queues its `close`.
pub(crate) fn close(document: &mut LynxDocument, dialog: NodeId, events: &ComponentEvents) {
    if !is_open(document, dialog) {
        return;
    }
    document.remove_attribute(dialog, OPEN_ATTRIBUTE);
    events.queue(dialog, CLOSE_EVENT);
}

/// HTML's `requestClose()` without cancelability: queues `cancel`, then
/// closes an open dialog as [`close`] does.
pub(crate) fn request_close(document: &mut LynxDocument, dialog: NodeId, events: &ComponentEvents) {
    if !is_open(document, dialog) {
        return;
    }
    events.queue(dialog, CANCEL_EVENT);
    close(document, dialog, events);
}

/// Whether an `open` attribute value opens the dialog: present and not
/// `"false"`.
fn opens(value: Option<&str>) -> bool {
    value.is_some_and(|value| value != "false")
}

fn is_open(document: &LynxDocument, dialog: NodeId) -> bool {
    opens(
        document
            .get(dialog)
            .and_then(|node| node.attribute(OPEN_ATTRIBUTE)),
    )
}

/// Sets or clears `flags` only when that changes them, so an unchanged state
/// schedules no restyle.
fn set_state(document: &mut LynxDocument, element: NodeId, flags: ElementState, on: bool) {
    let current = document
        .get(element)
        .map_or(ElementState::empty(), dom::Node::element_state);
    if current.contains(flags) == on {
        return;
    }
    if on {
        document.add_element_state(element, flags);
    } else {
        document.remove_element_state(element, flags);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Authored pixel sizes lay out exactly.

    use dom::stylo::properties::PropertyId;
    use dom::stylo::values::computed::{Display, Overflow};
    use dom::{NodeId, Point2D};

    use super::super::test_support::{
        child, display, document, element_under, overflow, style_of, with_component_events,
    };
    use super::super::{ComponentEvent, ComponentEvents, LynxDocument, PageConfig};
    use super::{DIALOG_TAG, InvalidState, close, request_close, show, show_modal};

    /// The phone viewport `test_support` builds.
    const VIEWPORT: (f32, f32) = (393.0, 727.0);

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

    fn matches(document: &LynxDocument, element: NodeId, selector: &str) -> bool {
        document
            .matches(element, selector)
            .expect("a valid selector")
    }

    /// What the component queued since the last call, as `(node, name)`.
    fn queued(events: &ComponentEvents) -> Vec<(NodeId, &'static str)> {
        events
            .take()
            .into_iter()
            .map(|event| match event {
                ComponentEvent::Plain { node, name } => (node, name),
                ComponentEvent::Image(outcome) => panic!("no image here: {outcome:?}"),
            })
            .collect()
    }

    /// The elements at `point` in the committed frame, front to back.
    fn hits(document: &mut LynxDocument, x: f32, y: f32) -> Vec<NodeId> {
        document.commit();
        document.elements_from_point(Point2D::new(x, y))
    }

    // --- the UA rules -------------------------------------------------------

    /// An open dialog gets HTML's UA values, `display` aside; a closed one,
    /// and one whose `open` is `"false"`, generates no box.
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
            // `Canvas` and `CanvasText` under the light colour scheme.
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

    /// With `defaultDisplayLinear` off a dialog takes the fork's initial
    /// `flex`, like every other container; a modal one and its backdrop lay
    /// out under either switch.
    #[test]
    fn a_dialog_and_its_backdrop_lay_out_under_either_display_default() {
        for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
            let (mut document, events) = with_component_events(PageConfig {
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
            close(&mut document, dialog, &events);
            document.layout();
            assert_eq!(display(&document, dialog), Display::None);
        }
    }

    /// `dialog:modal`: HTML's values, `overflow: scroll` for `auto` and
    /// `top`/`bottom` for `inset-block`. A dialog wider and taller than the
    /// viewport stops at `max-width`/`max-height`, which leave exactly room
    /// for its border and padding, and scrolls.
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

    /// An author `position: relative` on a modal dialog computes to
    /// `absolute`, as in a browser (css-position-4 §3.1, run by Stylo's
    /// adjuster because `dialog:modal` declares `-servo-top-layer`), and the
    /// dialog is placed against the viewport.
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
        // 101 + 2 × 10 + 2 × 3 = 127 across, 77 down, centred by the margins.
        assert_eq!(rect(&document, dialog), (133.0, 325.0, 127.0, 77.0));
    }

    /// An author `display: contents` on a modal dialog is blockified
    /// (css-position-4 §3.1), so the dialog still generates a box and renders
    /// centred in the viewport.
    #[test]
    #[ignore = "BLOCKED (ruling needed): under `lynx` Stylo's top-layer adjuster blockifies \
                `contents` through `Display::equivalent_block_display` to the fork's internal \
                block-flow display (raw 0x0202), which `dom::layout::style::display_mode` \
                refuses with a panic"]
    fn a_display_contents_modal_dialog_is_blockified_and_renders() {
        let mut document = document();
        let dialog = child(
            &mut document,
            DIALOG_TAG,
            "display: contents; width: 101px; height: 51px; font-size: 10px",
        );
        show_modal(&mut document, dialog).expect("a closed, connected dialog");
        document.layout();
        assert_ne!(display(&document, dialog), Display::Contents);
        assert_eq!(rect(&document, dialog), (133.0, 325.0, 127.0, 77.0));
    }

    // --- the methods --------------------------------------------------------

    #[test]
    fn show_and_show_modal_refuse_each_other_s_open_dialog() {
        let (mut document, events) = with_component_events(PageConfig::default());
        let dialog = child(&mut document, DIALOG_TAG, "");

        assert_eq!(show(&mut document, dialog), Ok(()));
        assert_eq!(document.get(dialog).unwrap().attribute("open"), Some(""));
        assert!(!document.in_top_layer(dialog));
        assert_eq!(show(&mut document, dialog), Ok(()), "already open: no-op");
        assert_eq!(show_modal(&mut document, dialog), Err(InvalidState));
        assert!(!document.in_top_layer(dialog));

        close(&mut document, dialog, &events);
        assert_eq!(document.get(dialog).unwrap().attribute("open"), None);
        assert_eq!(queued(&events), vec![(dialog, "close")]);
        close(&mut document, dialog, &events);
        request_close(&mut document, dialog, &events);
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
        assert_eq!(document.get(dialog).unwrap().attribute("open"), None);
        assert!(!document.in_top_layer(dialog));
        assert_eq!(
            show(&mut document, dialog),
            Ok(()),
            "`show` has no such rule"
        );
    }

    /// `requestClose()` is `cancel` then `close`, always: there is no
    /// `preventDefault` to stop it.
    #[test]
    fn request_close_queues_cancel_then_close_and_closes() {
        let (mut document, events) = with_component_events(PageConfig::default());
        let dialog = child(&mut document, DIALOG_TAG, "");
        show_modal(&mut document, dialog).expect("a closed, connected dialog");
        request_close(&mut document, dialog, &events);
        assert_eq!(queued(&events), vec![(dialog, "cancel"), (dialog, "close")]);
        assert!(!document.in_top_layer(dialog));
        document.layout();
        assert!(!matches(&document, dialog, ":open"));
        assert!(!matches(&document, dialog, ":modal"));
    }

    /// Removing `open` by hand, or writing `"false"`, closes a modal dialog
    /// and takes it out of the top layer, without a `close` event.
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

    /// HTML's removing steps: a modal dialog taken out of the document leaves
    /// the top layer, and comes back open but not modal.
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

    // --- rendering ----------------------------------------------------------

    /// The top layer at work through the UA rules: a modal dialog under a
    /// transformed, clipped, half-transparent ancestor centres in the
    /// viewport, paints above a `z-index: 100` sibling, and its backdrop
    /// covers the viewport, reports the dialog, and makes everything under it
    /// inert.
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
        // 101 + 2 × 10 + 2 × 3 = 127 across, 77 down.
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

        // A non-modal dialog paints in place, under the cover, and blocks
        // nothing.
        let events = ComponentEvents::default();
        close(&mut document, dialog, &events);
        show(&mut document, dialog).expect("a closed dialog");
        document.layout();
        assert_eq!(hits(&mut document, 1.0, 1.0), vec![cover, page]);
    }

    /// HTML's shrink-to-fit centring of a modal dialog with no author size.
    /// The border box is the content plus the UA `padding: 1em` (10px) and
    /// 3px border on each side: 101 + 20 + 6 = 127 by 51 + 20 + 6 = 77,
    /// centred in the 393 x 727 viewport at ((393 - 127) / 2, (727 - 77) / 2).
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
}
