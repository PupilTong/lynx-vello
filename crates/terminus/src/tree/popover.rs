//! HTML's `popover` attribute (HTML, "Popover"): the attribute's states,
//! every element's popover visibility state, and the show and hide
//! algorithms for a Manual popover, over the top layer
//! ([`crate::tree::top_layer`]).
//!
//! A subset of the browser's capability, and nothing that behaves
//! differently from it:
//!
//! - **The attribute.** An enumerated attribute, read ASCII case-insensitively: `auto` and the
//!   empty string are the Auto state, `manual` Manual, `hint` Hint, any other value Manual (the
//!   invalid value default), and no attribute the No Popover state ([`PopoverState`]).
//! - **The visibility state** is [`ElementState::POPOVER_OPEN`]: set exactly while the element is
//!   *showing*, which is what `:popover-open` matches. Only this module writes it;
//!   [`Document::add_element_state`] refuses it.
//! - **Show and hide.** [`Document::show_popover`] and [`Document::hide_popover`] are HTML's *show
//!   popover* and *hide popover* algorithms with a null source, throwing: the same validity checks,
//!   the same errors, and the same silent return for a popover already in the asked-for state. A
//!   shown popover is a top-layer entry that does not block the document.
//! - **Attribute changes and removal.** Changing a showing popover's attribute to a value in
//!   another state, removing it included, hides it; removing a showing popover from the document
//!   hides it, with no events, as HTML's removing steps do — so it comes back hidden.
//!
//! What is absent, and why its absence changes nothing a browser would show:
//!
//! - **Showing an Auto or Hint popover** is refused with [`PopoverError::AutoOrHintNotImplemented`]
//!   rather than shown as a Manual one: their stacks, light dismiss and close requests are not
//!   implemented. Hiding is never refused for this reason, because nothing can be showing in those
//!   states.
//! - **`beforetoggle` and `toggle`** are the embedder's to fire around these calls: `terminus`
//!   dispatches no script event. An embedder that shows a popover only inside a UA shadow tree,
//!   where neither event — both not composed — can reach a listener, and exposes no popover method
//!   to script, fires neither.
//! - **The popover focusing steps and focus restoration**: the engine has no focus.
//! - **Popover sources and triggers** (`popovertarget`, `command`), and with them the implicit
//!   anchor element: no source is ever passed.
//! - **The `overlay` property and pending removals**: a hidden popover leaves the top layer at
//!   once, which is what a browser does for every popover whose `overlay` is not transitioned — the
//!   only kind this engine can express.
//! - **The document's *showing popover* flag and *hiding popover nesting count*** only refuse a
//!   show or a hide started from a `beforetoggle` listener; with no event fired in between, none
//!   can start.
//!
//! # Where the spec's text and the browsers differ
//!
//! HTML's attribute change steps run *hide popover*, whose first check
//! reads the attribute's state *after* the change, so removing the
//! attribute from a showing popover would throw `NotSupportedError` there
//! and leave it showing. Chromium (`HTMLElement::UpdatePopoverAttribute`,
//! `IsPopoverReady`) and Gecko (`AfterSetPopoverAttr`,
//! `CheckPopoverValidity`) check the state they had stored, which is the
//! old one, and hide it. This module hides it, as they do.

use stylo_dom::ElementState;

use crate::tree::document::{Document, NodeId};

const POPOVER_ATTRIBUTE: &str = "popover";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopoverState {
    NoPopover,
    Auto,
    Manual,
    Hint,
}

impl PopoverState {
    #[must_use]
    pub fn from_attribute(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::NoPopover;
        };
        if value.is_empty() || value.eq_ignore_ascii_case("auto") {
            Self::Auto
        } else if value.eq_ignore_ascii_case("hint") {
            Self::Hint
        } else {
            Self::Manual
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopoverError {
    NotSupported,
    InvalidState,
    AutoOrHintNotImplemented,
}

impl<T> Document<T> {
    #[must_use]
    pub fn popover_state(&self, element: NodeId) -> PopoverState {
        PopoverState::from_attribute(
            self.live_element(element)
                .attribute(POPOVER_ATTRIBUTE)
                .as_deref(),
        )
    }

    #[must_use]
    pub fn popover_showing(&self, element: NodeId) -> bool {
        self.get(element)
            .is_some_and(|node| node.element_state().contains(ElementState::POPOVER_OPEN))
    }

    pub fn show_popover(&mut self, element: NodeId) -> Result<(), PopoverError> {
        if !self.check_popover_validity(element, false)? {
            return Ok(());
        }
        if self.popover_state(element) != PopoverState::Manual {
            return Err(PopoverError::AutoOrHintNotImplemented);
        }
        debug_assert!(
            !self.in_top_layer(element),
            "a hidden popover is not in the top layer"
        );
        self.add_to_top_layer(element, false);
        self.update_element_state(element, ElementState::POPOVER_OPEN, true);
        Ok(())
    }

    pub fn hide_popover(&mut self, element: NodeId) -> Result<(), PopoverError> {
        if self.check_popover_validity(element, true)? {
            self.hide_showing_popover(element);
        }
        Ok(())
    }

    fn check_popover_validity(
        &self,
        element: NodeId,
        expected_to_be_showing: bool,
    ) -> Result<bool, PopoverError> {
        if self.popover_state(element) == PopoverState::NoPopover {
            return Err(PopoverError::NotSupported);
        }
        if self.popover_showing(element) != expected_to_be_showing {
            return Ok(false);
        }
        let modal_dialog = self.live_element(element).tag_name() == Some("dialog")
            && self.blocks_document(element);
        if (!expected_to_be_showing && !self.is_connected(element)) || modal_dialog {
            return Err(PopoverError::InvalidState);
        }
        Ok(true)
    }

    fn hide_showing_popover(&mut self, element: NodeId) {
        self.remove_from_top_layer(element);
        self.update_element_state(element, ElementState::POPOVER_OPEN, false);
    }

    pub(crate) fn note_popover_attribute_change(&mut self, element: NodeId, old: PopoverState) {
        if self.popover_showing(element) && self.popover_state(element) != old {
            self.hide_showing_popover(element);
        }
    }

    pub(crate) fn note_popover_disconnected(&mut self, element: NodeId) {
        if self.popover_showing(element) {
            self.update_element_state(element, ElementState::POPOVER_OPEN, false);
        }
    }
}

pub(crate) fn is_popover_attribute(name: &str) -> bool {
    name == POPOVER_ATTRIBUTE
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{PopoverError, PopoverState};
    use crate::test_common::Doc;
    use crate::tree::document::NodeId;

    const UA: &str = "
        [popover]:not(:popover-open):not(dialog[open]) { display: none; }
        [popover] { position: fixed; }
        dialog:not([open]) { display: none; }";

    fn page() -> Doc {
        let mut doc = Doc::with_css("page { display: flex; width: 800px; height: 600px; }");
        doc.add_ua_css(UA);
        doc
    }

    fn popover(doc: &mut Doc, value: &str) -> NodeId {
        let element = doc.el(doc.root, "view");
        doc.set_attr(element, "popover", value);
        element
    }

    fn showing(doc: &Doc, element: NodeId) -> (bool, bool, bool) {
        (
            doc.dom.popover_showing(element),
            doc.dom.in_top_layer(element),
            doc.matches(element, ":popover-open"),
        )
    }

    #[test]
    fn the_attribute_states_are_html_s_enumerated_ones() {
        for (value, state) in [
            (None, PopoverState::NoPopover),
            (Some(""), PopoverState::Auto),
            (Some("auto"), PopoverState::Auto),
            (Some("AuTo"), PopoverState::Auto),
            (Some("manual"), PopoverState::Manual),
            (Some("MANUAL"), PopoverState::Manual),
            (Some("hint"), PopoverState::Hint),
            (Some("Hint"), PopoverState::Hint),
            (Some("bogus"), PopoverState::Manual),
            (Some(" manual"), PopoverState::Manual),
        ] {
            assert_eq!(PopoverState::from_attribute(value), state, "{value:?}");
        }
    }

    #[test]
    fn a_manual_popover_shows_on_the_top_layer_without_blocking_and_hides() {
        let mut doc = page();
        let element = popover(&mut doc, "manual");
        doc.flush();
        assert_eq!(showing(&doc, element), (false, false, false));
        assert_eq!(doc.value(element, "display"), "none");

        assert_eq!(doc.dom.show_popover(element), Ok(()));
        doc.flush();
        assert_eq!(showing(&doc, element), (true, true, true));
        assert!(!doc.dom.blocks_document(element), "a popover is not modal");
        assert_ne!(doc.value(element, "display"), "none");
        assert_eq!(doc.dom.show_popover(element), Ok(()), "already showing");
        assert_eq!(doc.dom.top_layer().count(), 1);

        assert_eq!(doc.dom.hide_popover(element), Ok(()));
        doc.flush();
        assert_eq!(showing(&doc, element), (false, false, false));
        assert_eq!(doc.value(element, "display"), "none");
        assert_eq!(doc.dom.hide_popover(element), Ok(()), "already hidden");
    }

    #[test]
    fn the_validity_checks_throw_what_html_throws() {
        let mut doc = page();
        let plain = doc.el(doc.root, "view");
        assert_eq!(doc.dom.show_popover(plain), Err(PopoverError::NotSupported));
        assert_eq!(doc.dom.hide_popover(plain), Err(PopoverError::NotSupported));

        let detached = doc.dom.create_element("view", ());
        doc.dom.set_attribute(detached, "popover", "manual");
        assert_eq!(
            doc.dom.show_popover(detached),
            Err(PopoverError::InvalidState)
        );
        assert_eq!(
            doc.dom.hide_popover(detached),
            Ok(()),
            "hidden: nothing to do"
        );

        let dialog = doc.el(doc.root, "dialog");
        doc.set_attr(dialog, "popover", "manual");
        doc.dom.add_to_top_layer(dialog, true);
        assert_eq!(
            doc.dom.show_popover(dialog),
            Err(PopoverError::InvalidState)
        );

        for value in ["", "auto", "hint"] {
            let element = popover(&mut doc, value);
            assert_eq!(
                doc.dom.show_popover(element),
                Err(PopoverError::AutoOrHintNotImplemented),
                "{value:?}"
            );
            assert!(!doc.dom.popover_showing(element));
        }
    }

    #[test]
    fn an_attribute_change_to_another_state_hides_a_showing_popover() {
        for (value, hides) in [
            (Some("MANUAL"), false),
            (Some("bogus"), false),
            (Some("auto"), true),
            (Some("hint"), true),
            (Some(""), true),
            (None, true),
        ] {
            let mut doc = page();
            let element = popover(&mut doc, "manual");
            doc.dom
                .show_popover(element)
                .expect("a connected manual popover");
            match value {
                Some(value) => doc.set_attr(element, "popover", value),
                None => doc.remove_attr(element, "popover"),
            }
            doc.flush();
            let expected = if hides {
                (false, false, false)
            } else {
                (true, true, true)
            };
            assert_eq!(showing(&doc, element), expected, "{value:?}");
        }
    }

    #[test]
    fn leaving_the_document_hides_a_showing_popover() {
        let mut doc = page();
        let wrapper = doc.el(doc.root, "view");
        let element = doc.el(wrapper, "view");
        doc.set_attr(element, "popover", "manual");
        doc.dom
            .show_popover(element)
            .expect("a connected manual popover");
        doc.dom.remove_element(wrapper);
        assert!(!doc.dom.popover_showing(element));
        assert!(!doc.dom.in_top_layer(element));

        doc.dom.append_child(doc.root, wrapper);
        doc.flush();
        assert_eq!(showing(&doc, element), (false, false, false));
        assert_eq!(doc.value(element, "display"), "none");
        assert_eq!(doc.dom.show_popover(element), Ok(()));
        assert!(doc.dom.popover_showing(element));
    }

    #[test]
    #[should_panic(expected = "popover")]
    fn popover_open_is_not_settable_as_element_state() {
        let mut doc = page();
        let element = popover(&mut doc, "manual");
        doc.dom
            .add_element_state(element, stylo_dom::ElementState::POPOVER_OPEN);
    }
}
