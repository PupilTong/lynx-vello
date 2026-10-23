//! HTML's `<dialog>` element (interactive-elements.html, "The dialog
//! element") over the document's top layer (css-position-4 §3,
//! [`Document::add_to_top_layer`]).
//!
//! This module implements HTML's behaviour, reduced where this crate has no
//! counterpart, as listed below. [`Document::define_html_elements`] defines
//! its component; [`UA_RULES`] are its rules for the embedder's UA sheet.
//!
//! # The UA rules
//!
//! [`UA_RULES`] is HTML's UA sheet for the element (rendering.html, "Flow
//! content") adapted to this crate:
//!
//! - **`display`.** HTML gives `dialog` `block`, which no box here lowers to, so the rules leave
//!   `display` out: the embedder writes the display its block-like containers get, ahead of them
//!   ([`crate::html`], "The UA rules"). An author `display: contents` on a modal dialog is
//!   blockified by §3.1's fixup (below) to `flex`, the initial display of the Stylo fork's grammar,
//!   where a browser computes `block`; the dialog then renders as a `flex` box centred in the
//!   viewport.
//! - **Closed is `display: none`.** `dialog:not([open])` as in HTML, plus `dialog[open="false"]`:
//!   `open` is a boolean attribute, and the value `"false"` is read as closed, for an embedder
//!   whose attribute writes stringify a script `false`, where HTML would read any value as open.
//!   The component reads it the same way.
//! - **`dialog:modal`.** HTML writes `overflow: auto` and `inset-block: 0`. `overflow: auto` is
//!   deliberately out of this crate and the Stylo fork disables `inset-block`, so the rule writes
//!   `overflow: scroll` and `top: 0; bottom: 0`. HTML's `position: fixed` is kept. The rule also
//!   declares `-servo-top-layer: auto`, a UA-only longhand (an author or user sheet drops it) that
//!   switches on Stylo's `StyleAdjuster::adjust_for_top_layer`, css-position-4 §3.1's
//!   computed-value fixups: a `position` other than `absolute`/`fixed` computes to `absolute`, as
//!   in a browser, and `display: contents` is blockified. Membership remains this crate's truth for
//!   what the computed value does not carry: paint order, the backdrop, inertness and the initial
//!   containing block.
//! - **Colours.** HTML's `background-color: Canvas; color: CanvasText`. Both compute through
//!   Stylo's Servo device (`device/servo.rs`, `system_color`) under the light colour scheme — this
//!   crate's device has no other — to `rgb(255, 255, 255)` and `rgb(0, 0, 0)`.
//! - **`::backdrop`.** HTML's `position: fixed; inset: 0`, plus `-servo-top-layer: auto` as on
//!   `dialog:modal`, and `display: flex`, because the pseudo-element's style has no other source of
//!   a display this crate lowers. `dialog::backdrop` is HTML's `rgba(0, 0, 0, 0.1)`.
//!
//! Every rule is HTML's selector inside `:where()`, so it has no specificity
//! and every embedder rule that names a tag outranks it, as an author rule
//! outranks a browser's UA sheet; among themselves they keep HTML's
//! precedence by source order ([`crate::html`], "The UA rules").
//!
//! No rule is `!important`. HTML's `width: fit-content; height: fit-content;
//! margin: auto` centres a modal dialog by shrink-to-fit sizing, and so it
//! does here: seldon's absolute pass stretch-fits only an `auto` size
//! between two insets (css-position-3 §4.1), so a dialog with no author size
//! takes its fit-content size (css-sizing-3 §3.2) in the viewport and its
//! `auto` margins centre it.
//!
//! # State
//!
//! The component keeps no Rust state per element. HTML's two facts about a
//! dialog are its `open` attribute and its "is modal" flag:
//!
//! - **Open** is the attribute, present and not `"false"`. The attribute callback is the one path
//!   that sets or clears `ElementState::OPEN` (`:open`), whoever wrote the attribute — the
//!   embedder's script binding or a method below. Removing the attribute (or writing `"false"`)
//!   also takes a modal dialog out of the top layer and clears `ElementState::MODAL`, without a
//!   `close` event: HTML's attribute change steps for `open` fire none, and the dialog simply stops
//!   rendering. A dialog showing as a popover stays in the layer: the popover is the document's
//!   ([`Document::show_popover`]), and `close()` leaves it too.
//! - **Modal** is top-layer membership with the blocks-the-document flag,
//!   [`Document::blocks_document`]. `ElementState::MODAL` (`:modal`) follows it. One case needs the
//!   component's help: a modal dialog removed from the document leaves the top layer in the
//!   document's unlink path (HTML's removing steps), where no callback can mutate
//!   (`disconnected_callback` takes `&Document`). A disconnected element has no style, so the stale
//!   bit is invisible until the element is connected again, and the component's
//!   `connected_callback` clears it there: a re-inserted dialog is open and not modal, as in a
//!   browser.
//!
//! # Methods
//!
//! [`show`], [`show_modal`], [`close`] and [`request_close`] are HTML's
//! `show()`, `showModal()`, `close()` and `requestClose()`. The component
//! answers the same four names from its [`CustomElement::invoke`], which an
//! embedder reaches through [`Document::invoke_element_method`]: an
//! [`InvalidState`] there is [`MethodError::InvalidState`], and every other
//! name is [`MethodOutcome::NotFound`]. Each sets or removes the `open`
//! attribute and lets the attribute callback do the state work, so there is
//! one path for each fact.
//!
//! - `show()`: no-op on an open non-modal dialog; [`InvalidState`] on a modal one; otherwise adds
//!   `open`.
//! - `showModal()`: no-op on a modal dialog; [`InvalidState`] on an open non-modal one, on a
//!   disconnected one and on one showing as a popover ([`super::popover`]); otherwise adds `open`,
//!   enters the top layer blocking the document, and sets `:modal`.
//! - `close()`: no-op on a closed dialog; otherwise removes `open` and queues `close`.
//! - `requestClose()`: no-op on a closed dialog; otherwise queues `cancel`, then closes as
//!   `close()` does.
//!
//! [`InvalidState`] is HTML's `InvalidStateError`. The methods' `params` are
//! not read, so `close(returnValue)` drops its value.
//!
//! # Events
//!
//! `close` and `cancel` go to the document's
//! [`QueuedEventSink`](crate::event::QueuedEventSink) at the moment the
//! method queues them — HTML queues `close` as a task too — so the embedder
//! fires them later, in one order with the events its own components queue.
//! With no sink registered they are dropped. `cancel` is not cancelable:
//! this crate's event model has no `preventDefault`, so `requestClose()`
//! always closes.
//!
//! # What is not implemented
//!
//! - Close requests (Escape, the back gesture), `closedby` and light dismiss: there is no close
//!   watcher, and `closedby` is not parsed.
//! - `beforetoggle` / `toggle`, the focusing steps, `autofocus`, `returnValue`, popovers and
//!   fullscreen.
//! - The `overlay` property and the pending top-layer removals: leaving the top layer is immediate.
//!   `::backdrop` animations and transitions do not run (the lazy pseudo-element cascade carries no
//!   animation declarations).

use stylo_dom::ElementState;

use crate::tree::custom::{CustomElement, MethodCall, MethodError, MethodOutcome};
use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

/// HTML's tag.
pub const DIALOG_TAG: &str = "dialog";
/// The boolean attribute that says the dialog is showing.
const OPEN_ATTRIBUTE: &str = "open";
/// The event [`close`] queues.
const CLOSE_EVENT: &str = "close";
/// The event [`request_close`] queues ahead of `close`.
const CANCEL_EVENT: &str = "cancel";

/// HTML's UA rules for `dialog` and `::backdrop`, adapted as the module
/// documentation lists, each at zero specificity ([`link::html`](crate::link::html), "The UA
/// rules"). The display `dialog` gets is the embedder's, written ahead of
/// them.
pub const UA_RULES: &str = r#"
:where(dialog:not([open]), dialog[open="false"]) { display: none; }
:where(dialog) {
  position: absolute;
  inset-inline-start: 0; inset-inline-end: 0;
  width: fit-content; height: fit-content;
  margin: auto;
  border: solid;
  padding: 1em;
  background-color: Canvas; color: CanvasText;
}
:where(dialog:modal) {
  -servo-top-layer: auto;
  position: fixed;
  overflow: scroll;
  top: 0; bottom: 0;
  max-width: calc(100% - 6px - 2em);
  max-height: calc(100% - 6px - 2em);
}
::backdrop { -servo-top-layer: auto; position: fixed; inset: 0; display: flex; }
:where(dialog)::backdrop { background: rgba(0, 0, 0, 0.1); }
"#;

/// Installs the `dialog` component ([`Document::define_html_elements`]).
pub(super) fn define<T>(document: &mut Document<T>) {
    document.define(DIALOG_TAG, Box::new(Dialog));
}

/// The `dialog` component: keeps `:open` and `:modal` and top-layer
/// membership in step with the `open` attribute, and answers HTML's four
/// methods.
struct Dialog;

impl<T> CustomElement<T> for Dialog {
    fn observed_attributes(&self) -> Vec<String> {
        vec![OPEN_ATTRIBUTE.to_owned()]
    }

    /// Clears a `:modal` left behind by a removal: the unlink took the
    /// element out of the top layer, and HTML's removing steps set "is modal"
    /// to false.
    fn connected_callback(&self, document: &mut Document<T>, element: NodeId) {
        if !document.blocks_document(element) {
            set_state(document, element, ElementState::MODAL, false);
        }
    }

    fn attribute_changed_callback(
        &self,
        document: &mut Document<T>,
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
            // A modal dialog leaves the layer; a dialog showing as a popover
            // is the popover's to take out (`Document::hide_popover`), as
            // HTML's `close()` leaves it.
            if document.blocks_document(element) {
                document.remove_from_top_layer(element);
            }
            set_state(document, element, ElementState::MODAL, false);
        }
    }

    /// `HTMLDialogElement`'s four methods; see the module documentation.
    fn invoke(
        &self,
        document: &mut Document<T>,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        let shown = match call.name {
            "show" => show(document, element),
            "showModal" => show_modal(document, element),
            "close" => {
                close(document, element);
                Ok(())
            }
            "requestClose" => {
                request_close(document, element);
                Ok(())
            }
            _ => return MethodOutcome::NotFound,
        };
        match shown {
            Ok(()) => MethodOutcome::Done,
            Err(InvalidState) => MethodOutcome::Failed(MethodError::InvalidState),
        }
    }
}

/// HTML's `InvalidStateError`: `show()` on a modal dialog, `showModal()` on
/// an open non-modal or a disconnected one.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidState;

/// HTML's `show()`.
pub fn show<T>(document: &mut Document<T>, dialog: NodeId) -> Result<(), InvalidState> {
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
pub fn show_modal<T>(document: &mut Document<T>, dialog: NodeId) -> Result<(), InvalidState> {
    if is_open(document, dialog) {
        return if document.blocks_document(dialog) {
            Ok(())
        } else {
            Err(InvalidState)
        };
    }
    if !document.is_connected(dialog) || document.popover_showing(dialog) {
        return Err(InvalidState);
    }
    document.set_attribute(dialog, OPEN_ATTRIBUTE, "");
    document.add_to_top_layer(dialog, true);
    set_state(document, dialog, ElementState::MODAL, true);
    Ok(())
}

/// HTML's `close()`: closes an open dialog and queues its `close` on the
/// document's [`QueuedEventSink`](crate::event::QueuedEventSink).
pub fn close<T>(document: &mut Document<T>, dialog: NodeId) {
    if !is_open(document, dialog) {
        return;
    }
    document.remove_attribute(dialog, OPEN_ATTRIBUTE);
    document.queue_element_event(dialog, CLOSE_EVENT);
}

/// HTML's `requestClose()` without cancelability: queues `cancel`, then
/// closes an open dialog as [`close`] does.
pub fn request_close<T>(document: &mut Document<T>, dialog: NodeId) {
    if !is_open(document, dialog) {
        return;
    }
    document.queue_element_event(dialog, CANCEL_EVENT);
    close(document, dialog);
}

/// Whether an `open` attribute value opens the dialog: present and not
/// `"false"`.
fn opens(value: Option<&str>) -> bool {
    value.is_some_and(|value| value != "false")
}

fn is_open<T>(document: &Document<T>, dialog: NodeId) -> bool {
    opens(
        document
            .get(dialog)
            .and_then(|node| node.attribute(OPEN_ATTRIBUTE))
            .as_deref(),
    )
}

/// Sets or clears `flags` only when that changes them, so an unchanged state
/// schedules no restyle.
fn set_state<T>(document: &mut Document<T>, element: NodeId, flags: ElementState, on: bool) {
    let current = document
        .get(element)
        .map_or(ElementState::empty(), Node::element_state);
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::{DIALOG_TAG, close, request_close, show_modal};
    use crate::event::QueuedEventSink;
    use crate::test_common::Doc;
    use crate::tree::document::NodeId;

    /// A sink that keeps what it is handed, in order.
    #[derive(Default)]
    struct Recorder(RefCell<Vec<(NodeId, &'static str)>>);

    impl QueuedEventSink for Recorder {
        fn queue(&self, target: NodeId, name: &'static str) {
            self.0.borrow_mut().push((target, name));
        }
    }

    impl Recorder {
        fn taken(&self) -> Vec<(NodeId, &'static str)> {
            std::mem::take(&mut *self.0.borrow_mut())
        }
    }

    /// A document with HTML's elements defined, a modal dialog and a
    /// sibling in it, and `sink` registered when there is one.
    fn modal_dialog(sink: Option<Rc<Recorder>>) -> (Doc, NodeId, NodeId) {
        let mut doc = Doc::new();
        doc.dom.define_html_elements();
        if let Some(sink) = sink {
            doc.dom.set_queued_event_sink(sink);
        }
        let dialog = doc.el(doc.root, DIALOG_TAG);
        let sibling = doc.el(doc.root, "div");
        show_modal(&mut doc.dom, dialog).expect("a closed, connected dialog");
        (doc, dialog, sibling)
    }

    /// `close()` hands its `close` to the sink while it runs, so an event
    /// queued after it lands after it.
    #[test]
    fn close_queues_its_event_on_the_sink_in_order_with_the_next_one() {
        let sink = Rc::new(Recorder::default());
        let (mut doc, dialog, sibling) = modal_dialog(Some(Rc::clone(&sink)));
        assert_eq!(sink.taken(), [], "showModal() queues nothing");

        close(&mut doc.dom, dialog);
        assert_eq!(
            sink.taken(),
            [(dialog, "close")],
            "queued by close() itself"
        );
        assert!(!doc.dom.in_top_layer(dialog));

        show_modal(&mut doc.dom, dialog).expect("a closed, connected dialog");
        close(&mut doc.dom, dialog);
        sink.queue(sibling, "next");
        close(&mut doc.dom, dialog);
        assert_eq!(
            sink.taken(),
            [(dialog, "close"), (sibling, "next")],
            "a closed dialog queues nothing more"
        );
    }

    /// `requestClose()` queues `cancel`, then `close`, each when it happens.
    #[test]
    fn request_close_queues_cancel_then_close_on_the_sink() {
        let sink = Rc::new(Recorder::default());
        let (mut doc, dialog, sibling) = modal_dialog(Some(Rc::clone(&sink)));
        sink.queue(sibling, "before");
        request_close(&mut doc.dom, dialog);
        assert_eq!(
            sink.taken(),
            [(sibling, "before"), (dialog, "cancel"), (dialog, "close")]
        );
    }

    /// With no sink registered, the events are dropped and the dialog
    /// still closes.
    #[test]
    fn with_no_sink_the_events_are_dropped() {
        let (mut doc, dialog, _) = modal_dialog(None);
        request_close(&mut doc.dom, dialog);
        assert!(!doc.dom.in_top_layer(dialog));
        assert!(!doc.matches(dialog, ":open"));
    }
}
