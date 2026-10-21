//! HTML's `popover` attribute: the UA rules HTML's rendering section gives
//! it (rendering.html, "Flow content"), over the document's popover
//! algorithms ([`Document::show_popover`](crate::tree::document::Document::show_popover))
//! and top layer.
//!
//! The crate's capability is a subset — the document shows and hides Manual
//! popovers, and has no popover method of its own for script and no
//! `popovertarget` (`tree/popover.rs`'s module documentation lists what is
//! absent) — and every rule here is HTML's, unchanged but for the two
//! adaptations every HTML rule in [`crate::html`] gets:
//!
//! - **`display: block`** has no box here, so `dialog:popover-open`'s is left out, as `dialog`'s
//!   own `display` is ([`super::dialog`]): the embedder writes it after these rules
//!   ([`crate::html`], "The UA rules").
//! - **`overflow: auto`** is deliberately out of this crate, which paints no scrollbars; the
//!   `[popover]` box writes `scroll`, as `dialog:modal` does.
//!
//! # The rules
//!
//! [`UA_RULES`] are HTML's, in HTML's order:
//!
//! - **Hidden is `display: none`**: `[popover]:not(:popover-open):not(dialog[open])`. A popover
//!   `dialog` that is open as a dialog renders as one.
//! - **The `[popover]` box** is fixed, centred by `inset: 0` and `auto` margins around its
//!   fit-content size, with a solid medium border, `0.25em` of padding, and `Canvas`/`CanvasText`
//!   (light scheme: white and black).
//! - **`:popover-open` is a top-layer element**: `-servo-top-layer: auto`, the UA-only longhand
//!   that switches on Stylo's §3.1 computed-value fixups, as `dialog:modal` declares it. It is
//!   Gecko's `:popover-open { -moz-top-layer: auto }`; Chromium does the same internally.
//! - **`:popover-open::backdrop`** covers the viewport, is transparent, and is `pointer-events:
//!   none !important`, HTML's own `!important`: a popover's backdrop never takes a touch, and
//!   nothing below a popover is inert, because a showing popover is a top-layer entry that does not
//!   block the document.
//!
//! Every rule is HTML's selector inside `:where()`, so it has no specificity
//! ([`crate::html`], "The UA rules"). The one `!important` beats every
//! author declaration, an author `!important` included, as a UA
//! `!important` does in a browser.

/// HTML's popover UA rules, as the module documentation lists, each HTML's
/// selector inside `:where()` ([`link::html`](crate::link::html), "The UA rules"). The one
/// `!important` line stays whole on a line of its own, so a sheet can be
/// audited for `!important` line by line.
pub const UA_RULES: &str = r"
:where([popover]:not(:popover-open):not(dialog[open])) { display: none; }
:where([popover]) {
  position: fixed;
  inset: 0;
  width: fit-content; height: fit-content;
  margin: auto;
  border: solid;
  padding: 0.25em;
  overflow: scroll;
  color: CanvasText; background-color: Canvas;
}
:where(:popover-open) { -servo-top-layer: auto; }
:where(:popover-open)::backdrop { position: fixed; inset: 0; pointer-events: none !important; background-color: transparent; }
";
