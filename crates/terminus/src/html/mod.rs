//! HTML elements this crate implements over its own tree: elements whose
//! behaviour HTML defines over the DOM, the top layer and the popover
//! algorithms that the document already has, so that an embedder gets
//! HTML's element rather than writing its own.
//!
//! - [`dialog`] is HTML's `<dialog>` element: its UA rules, a component that keeps `:open`,
//!   `:modal` and top-layer membership in step with the `open` attribute, and its four methods.
//! - [`popover`] is the UA rules HTML gives the `popover` attribute, whose state machine is the
//!   document's own ([`Document::show_popover`]).
//!
//! Neither is part of a document by itself. [`Document::new`] defines no
//! element, so an embedder that wants HTML's elements calls
//! [`Document::define_html_elements`], and adds the UA rules to its own UA
//! sheet, which also decides where they land among its other rules.
//!
//! # The UA rules
//!
//! Every rule here is HTML's selector inside `:where()`, so it has no
//! specificity (a `::backdrop` rule keeps the pseudo-element's own): in one
//! UA sheet with the embedder's rules, every embedder rule that names a tag
//! outranks it, as an author rule outranks a browser's UA sheet whatever its
//! specificity. This is a cascade layer below the embedder's rules, written
//! by hand: an `@layer` in a UA sheet trips Stylo's rule tree, whose root
//! node carries an unlayered UA rule's priority, so the first layered rule
//! under it fails its ordering assertion.
//!
//! Among themselves the rules then rank by source order alone, and HTML's
//! own precedence is the order an embedder writes them in:
//!
//! 1. `:where(dialog) { display: … }`, the display the embedder's block-like containers get, for
//!    HTML's `block` ([`dialog`], "The UA rules");
//! 2. [`dialog::UA_RULES`], in HTML's order;
//! 3. [`popover::UA_RULES`], whose `[popover]` box beats `dialog`'s as its higher specificity does
//!    in HTML;
//! 4. `:where(dialog:popover-open) { display: … }`, the same display, after the closed `dialog`'s
//!    `none`, as in HTML.
//!
//! The one pair whose order is then the reverse of HTML's, `dialog:modal`
//! before `[popover]`, writes the same values for every property both
//! declare (`position: fixed`, the insets at 0, `overflow: scroll`), so no
//! element can tell. HTML's one `!important`, the popover `::backdrop`'s
//! `pointer-events`, beats every author declaration, as a UA `!important`
//! does in a browser.

pub(crate) mod dialog;
pub(crate) mod popover;

use crate::tree::document::Document;

impl<T> Document<T> {
    /// Defines HTML's built-in elements this crate implements
    /// ([`crate::html`]): today `dialog` ([`dialog`]).
    ///
    /// Like [`Document::define`], it must run before any element carries one
    /// of their tags, and the tags must have no definition yet. Their UA
    /// rules are the embedder's to add ([`crate::html`], "The UA rules").
    pub fn define_html_elements(&mut self) {
        dialog::define(self);
    }
}
