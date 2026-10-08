//! The `overlay` tag, and `x-overlay-ng`: Lynx's modal layer, built the way
//! web-core builds it — a host that generates no box, and a `<dialog>` in its
//! UA shadow tree that enters `dom`'s top layer (css-position-4 §3,
//! `dom::tree::top_layer`) and carries the content.
//!
//! A Lynx-only component with no W3C equivalent, so `AGENTS.md`'s standards
//! policy puts it in the second bucket: this module implements what Lynx does
//! with it. web-core's structure is the one followed
//! (`web-elements/src/elements/XOverlayNg/*`, `htmlTemplates.ts:136-172`).
//! `overlay` and `<dialog>` are peers here, not one inside the other's
//! policy: what they share is the top layer itself, the generic
//! `::backdrop` rule, and the [`ComponentEvents`] queue their events go on.
//!
//! # Two tag names, one component
//!
//! Native registers `overlay`, with `x-overlay-ng` as its legacy alias
//! (`lynx/platform/darwin/ios/lynx/ui/LynxUIOwner.m:1616`, Android
//! `LynxUIOwner.java:1891`). web-core maps the JSX tag `overlay` to
//! `x-overlay-ng` (`web-core/ts/constants.ts:111`), and `@lynx-js/types`
//! declares `<overlay>` (`types/common/element/overlay.d.ts:52-130`). A
//! compiled `.web.bundle` writes the JSX tag verbatim here (`bobcat-element`
//! maps no tag), and a card may write either, so both are defined, as
//! [`super::blur_view`] defines its two.
//!
//! # The structure
//!
//! [`Overlay`] attaches a UA shadow tree when the element is constructed —
//! web-core's template, part for part, less one:
//!
//! - `<dialog id="dialog" part="dialog" popover="manual">`, the top-layer element. It is this
//!   engine's own `dialog` ([`super::dialog`]), driven only through HTML's API for it, as web-core
//!   drives its own: `showModal()` and `close()`, and — for a pass-through overlay —
//!   `showPopover()` and `hidePopover()` (`dom::tree::popover`). The `popover` attribute is the one
//!   part web-core's template lacks; see "Shown, modal or as a popover" below.
//! - `<div class="overlay-inner">` inside it, filling it: the containing block of the first child.
//! - The default `<slot>` inside that, which every child of the host is assigned to.
//!
//! web-core's `.overlay-placeholder`, a 1px box after `.overlay-inner` that keeps the dialog
//! scrollable so its `overscroll-behavior: contain` stops a browser's scroll chain there, is not
//! built: a top-layer element ends its scroll chain by membership here
//! (`dom::Document::scroll_parent`), with nothing to scroll. With it goes `.overlay-inner`'s
//! `position: sticky; top: 0`, which only pinned it while that 1px scrolled: it is `relative`,
//! the same box and the same containing block.
//!
//! **The host generates no box** (`display: contents`, as web-core's): its children render in the
//! dialog, through the slot, and nothing of the host itself is painted. Two facts follow, and they
//! are why the host is not itself the top-layer element:
//!
//! - **Hidden is the dialog's state.** A hidden overlay's dialog is closed and not showing as a
//!   popover, `display: none` by HTML's rules for both ([`super::dialog`], [`super::popover`]), and
//!   the dialog is in a shadow tree no document rule reaches. An author `display` on the host — a
//!   card's `.dialog-overlay { display: flex }` — gives the host a box, which the host rule keeps
//!   zero-sized and paint-contained, and the children stay hidden.
//! - **What fills the viewport is the dialog, and it is transparent.** An author `background-color`
//!   on the host paints nowhere: under `display: contents` the host has no box, and under an author
//!   `display` its box is zero-sized. The dialog's own style is reachable only through
//!   `::part(dialog)`, as in web-core.
//!
//! Inherited style still flows down the flat tree: host, dialog, `.overlay-inner`, slot, child.
//!
//! # The UA rules
//!
//! [`UA_RULES`] style the host and its light children, both tags in every
//! selector:
//!
//! - **The host.** `display: contents`, with web-core's `position: fixed; top: 0; width: 0;
//!   max-width: 0; max-height: 0; contain: strict` (`x-overlay-ng.css:17-26`), which apply only
//!   once an author `display` gives the host a box: a zero-sized, paint-contained box at the top of
//!   the viewport. The host is in neither [`super::ua_sheet`]'s display line nor its common
//!   container block; web-core's `overflow: visible` is the initial value here.
//! - **The first child sits at the viewport's top-left.** `position: absolute; top: 0; left: 0`,
//!   web-core's `x-overlay-ng > *:first-child` (`x-overlay-ng.css:31-36`) and native's first child
//!   laid out against the window at (0, 0) (`LynxUIOverlay.m:30-84`, Android
//!   `LynxUIOverlayShadowNode.kt:10-40`). Its containing block is `.overlay-inner`, which fills the
//!   dialog, which fills the viewport. web-core's `display: flex` on that child is not copied: the
//!   child keeps the display its own tag gets.
//! - **Through a `wrapper` too.** `ReactLynx` writes a `wrapper` (`__CreateWrapperElement`,
//!   `display: contents` in [`super::ua_sheet`]) around conditional and list children, so an
//!   `<overlay>{shown && <view/>}</overlay>` card makes the wrapper the overlay's first child and
//!   the panel would lay out in `.overlay-inner`'s flow. `overlay > wrapper > :first-child` gets
//!   the same `position: absolute; top: 0; left: 0`, web-core's `x-overlay-ng > lynx-wrapper >
//!   *:first-child` (`x-overlay-ng.css:31`) with this engine's tag. The wrapper's later children
//!   are not hidden: web-core hides only the overlay's direct children, and so does this sheet.
//! - **Only the first child renders.** `display: none !important` on every other child, as
//!   web-core's `x-overlay-ng > *:not(:first-child)` (`x-overlay-ng.css:27-29`), and native
//!   measures child 0 alone. This is the sheet's one `!important` for the tag, recorded in
//!   `docs/style-assumptions.md` §D.15: web-core pins it the same way, and an author `display` on a
//!   second child would otherwise make it render where neither reference does.
//!
//! [`SHADOW_RULES`] are web-core's template `<style>` with its
//! `x-overlay-ng::part(dialog)` rule folded in. A shadow sheet's rules are
//! author origin, so they beat the UA sheet's `dialog`, `dialog:modal` and
//! `dialog::backdrop` rules wherever both speak:
//!
//! - **`#dialog:is([open], :popover-open)` fills the viewport**, shown either way: `position:
//!   fixed` with all four insets 0, `width` and `height` 100% with `max-width`/`max-height` 100%,
//!   no padding, border or margin, a transparent background (over `dialog`'s and `[popover]`'s
//!   `Canvas`); `overflow: scroll` — a browser's modal `overflow: auto`, written `scroll` here as
//!   `dialog:modal` and `[popover]` write it — and `overscroll-behavior: contain`. Its display is
//!   the one `defaultDisplayLinear` picks for `dialog`; its one child fills it whatever the mode.
//! - **`#dialog::backdrop` is transparent**, over `dialog::backdrop`'s 10% black (a popover's
//!   `::backdrop` is already, by HTML's own rule).
//! - **`.overlay-inner`** is `position: relative` (see above), 100% by 100%, and a column flex box:
//!   web-core's is a `div`, a block, and a column stacks and stretches the in-flow children a
//!   `wrapper` leaves the way a block does.
//! - **Hit testing.** `.overlay-inner` is `pointer-events: none` and the slot inside it `auto`,
//!   which the slotted children inherit through the flat tree: a touch on the overlay outside its
//!   content hits the dialog — or its `::backdrop`, which reports it — and a composed event
//!   retargets that to the host for every listener outside the shadow tree.
//! - **`events-pass-through`**, present and not `"false"` on the host, makes the dialog
//!   `pointer-events: none` (`:host(…) #dialog`); its `::backdrop`, a popover's, is `none` by
//!   HTML's own `!important`. Neither is hit-testable while the content still is.
//! - The slot is `display: contents`. web-core's `scrollbar-width: none` and `outline: none` are
//!   not written: this engine paints no scrollbars, and no focus ring.
//!
//! # State
//!
//! The component ([`Overlay`]) keeps no Rust state per element; it holds the
//! document's [`ComponentEvents`] queue, as [`super::image`]'s does.
//!
//! - **Shown** is the `visible` attribute, present and not `"false"`, on a connected host: its
//!   dialog is in the top layer. The boolean reading is web-core's: it removes an attribute whose
//!   value is `"false"` before a handler sees it
//!   (`web-elements/src/element-reactive/component.ts:158-205`), and this engine's `__SetAttribute`
//!   stringifies `false`.
//! - **Shown, modal or as a popover.** A blocking overlay's dialog is shown with HTML's
//!   `showModal()` ([`super::dialog::show_modal`]): modal, `:modal`, everything below inert. A
//!   pass-through overlay's — `events-pass-through` present and not `"false"` — is shown as a
//!   Manual popover (`dom::Document::show_popover`): on the top layer, painted above everything,
//!   and blocking nothing, because a popover is the one top-layer element HTML does not make the
//!   document inert for. web-core shows both modally and forwards a `click` by hand
//!   (`XOverlayAttributes.ts:69-88`); a popover is the standard's way to the pass-through native
//!   has (`docs/tracking/deviations.md`).
//! - The `visible` callback shows the dialog when the attribute starts to show a connected overlay
//!   whose dialog is not in the layer, and hides it when the attribute stops showing one: HTML's
//!   `close()` ([`super::dialog::close`]) for a modal dialog, `hidePopover()` for a popover. A
//!   rewrite that keeps it shown (`"true"` to `""`) does nothing. Writing `visible` on a
//!   disconnected host does nothing until it connects.
//! - The `events-pass-through` callback hides a shown overlay's dialog and shows it again the other
//!   way when the flag changes. Showing puts it at the top of the layer, so toggling the attribute
//!   also raises the overlay above any shown after it; accepted.
//! - `close()` queues a `close` at the dialog, as web-core's does. It is not composed, so it stays
//!   in the shadow tree, where script names nothing: no listener can hear it.
//! - [`Overlay::connected_callback`] opens the dialog when `visible` shows the overlay: web-core
//!   replays the present attributes at connect (`component.ts:226-256`), and `visible` is a
//!   `noDomMeasure = false` handler, so it runs only once connected (`XOverlayAttributes.ts:51`).
//!   Disconnection is `dom`'s: the unlink drops the dialog's entry, since a shadow tree is
//!   connected only through its host (HTML's removing steps), with no event — a popover comes back
//!   hidden, a modal dialog open and not modal. A reconnected visible overlay enters again; a modal
//!   dialog left open is closed first, as `showModal()` refuses an open one.
//! - Overlays stack in the order they were shown: each dialog enters at the top of the layer.
//!
//! The host carries no element state (`:open`, `:modal`); selectors key on
//! its attributes. Nothing is handled in `handle_event`, and there is no UI
//! method: native and web-core expose none to script.
//!
//! # Events
//!
//! `showoverlay` when the overlay enters the top layer and `dismissoverlay`
//! when `visible` takes it out, queued at the host on [`ComponentEvents`] like
//! the dialog's `close` and delivered from an entry of their own,
//! non-bubbling, with a `{}` detail — web-core dispatches both at the host
//! with `bubbles: false, composed: false` and no detail
//! (`commonEventInitConfiguration.ts:5-9`). The dialog's own `close`, which
//! `close()` queues as web-core's does, is not composed and never leaves the
//! shadow tree, where nothing listens.
//! They fire on transitions only, where web-core re-runs `showModal()` and
//! fires `showoverlay` again for every write of a showing value.
//! Removing a shown overlay from the document fires nothing, as in web-core
//! (native Android fires `dismissoverlay`); `docs/tracking/deviations.md`
//! lists the conflicts.
//!
//! # What is not implemented
//!
//! - `requestclose`: there is no keyboard or back input to send it (native Android and Harmony send
//!   it, web-core never does). No `overlaytouch`, `overlaymoved`, `error` or `layoutchange`.
//! - `level`: native has four tiers (level 1 frontmost, last in first out within one); web-core's
//!   `z-index` ladder (`x-overlay-ng.css:42-53`) has no effect under its `display: contents` host,
//!   and its overlays stack in `showModal()` order. Overlays here stack in show order, as in
//!   web-core, and the ladder is not written.
//! - web-core's `x-overlay-ng [event-through] { pointer-events: none }` (`x-overlay-ng.css:38-40`):
//!   `event-through` is a general Lynx attribute this engine does not read anywhere yet.
//! - `mode`, `status-bar-translucent`, `status-bar-translucent-style`, `cut-out-mode`,
//!   `custom-layout`, `always-show`, `nest-scroll`, `ignore-focus`, `allow-pan-gesture`,
//!   `ios-enable-swipe-back`, the `android-*` attributes, `overlay-id` and `compat-bounding-rect`:
//!   platform window and gesture policy with no counterpart in this engine's one-window view, and
//!   none of them is read by web-core.
//! - web-core's pass-through re-dispatch (close the dialog, `elementFromPoint`, a synthetic
//!   `click`, `showModal()` on the next frame, `XOverlayAttributes.ts:69-88`) is a browser
//!   workaround. Here a pass-through overlay's dialog is a popover, which blocks nothing, and is
//!   not hit-testable, so every gesture outside the content, scrolls included, reaches what is
//!   below, as in native (`LynxOverlayContainer.m:169-214`, `LynxOverlayView.kt:726-752`).

use dom::{CustomElement, Node, NodeId, ShadowRootMode};

use super::dialog::{DIALOG_TAG, close, show_modal};
use super::{ComponentEvents, LynxDocument};

/// Native's tag, and the JSX tag a compiled card writes.
pub(super) const OVERLAY_TAG: &str = "overlay";
/// web-core's tag, native's legacy alias.
pub(super) const X_OVERLAY_TAG: &str = "x-overlay-ng";
/// The boolean attribute that shows the overlay.
const VISIBLE_ATTRIBUTE: &str = "visible";
/// The boolean attribute that lets touches outside the content through.
const EVENTS_PASS_THROUGH_ATTRIBUTE: &str = "events-pass-through";
/// The event queued when the overlay enters the top layer.
const SHOW_EVENT: &str = "showoverlay";
/// The event queued when `visible` takes the overlay out of the top layer.
const DISMISS_EVENT: &str = "dismissoverlay";
/// The shadow dialog's id, and the part name web-core exports it under.
const DIALOG_ID: &str = "dialog";
/// The class of the box inside the dialog that holds the slot.
const INNER_CLASS: &str = "overlay-inner";

/// The overlay's UA rules on the host and its light children, as the module
/// documentation lists. The one `!important` line stays on a line of its
/// own: `the_ua_sheet_is_important_free_apart_from_the_text_block` reads the
/// sheet line by line.
pub(super) const UA_RULES: &str = r"
overlay, x-overlay-ng {
  display: contents; position: fixed; top: 0;
  width: 0; max-width: 0; max-height: 0; contain: strict;
}
overlay > :first-child, x-overlay-ng > :first-child { position: absolute; top: 0; left: 0; }
overlay > wrapper > :first-child, x-overlay-ng > wrapper > :first-child { position: absolute; top: 0; left: 0; }
overlay > :not(:first-child), x-overlay-ng > :not(:first-child) { display: none !important; }
";

/// The shadow sheet, added to each instance's shadow root: see the module
/// documentation.
const SHADOW_RULES: &str = r#"
#dialog:is([open], :popover-open) {
  position: fixed; top: 0; right: 0; bottom: 0; left: 0;
  width: 100%; height: 100%; max-width: 100%; max-height: 100%;
  padding: 0; border: 0; margin: 0; background: transparent;
  overflow: scroll; overscroll-behavior: contain;
}
#dialog::backdrop { background-color: transparent; }
:host([events-pass-through]:not([events-pass-through="false"])) #dialog { pointer-events: none; }
.overlay-inner {
  display: flex; flex-direction: column; position: relative;
  width: 100%; height: 100%; pointer-events: none;
}
.overlay-inner > * { pointer-events: auto; }
slot { display: contents; }
"#;

/// Installs the component under both tag names, over the queue its events go
/// into. Must run after [`super::dialog::define`], whose component the shadow
/// dialog is, and before any element could carry either tag, which is
/// [`Document::define`](dom::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument, events: ComponentEvents) {
    document.define(
        OVERLAY_TAG,
        Box::new(Overlay {
            events: events.clone(),
        }),
    );
    document.define(X_OVERLAY_TAG, Box::new(Overlay { events }));
}

/// The overlay component: builds the shadow tree when the element is
/// constructed, keeps the shadow dialog's top-layer membership in step with
/// `visible`, `events-pass-through` and connection, and queues `showoverlay`
/// and `dismissoverlay` on the transitions.
///
/// The shadow tree is built in `constructed`, as the swiper's is: the
/// element is new and detached then, so every child arrives at the slot.
struct Overlay {
    events: ComponentEvents,
}

impl Overlay {
    /// Shows the shadow dialog the way the host's `events-pass-through`
    /// asks — `showModal()`, or as a Manual popover — and queues
    /// `showoverlay`.
    fn show(&self, document: &mut LynxDocument, overlay: NodeId) {
        let dialog = shadow_dialog(document, overlay);
        self.show_dialog(document, overlay, dialog);
        self.events.queue(overlay, SHOW_EVENT);
    }

    fn show_dialog(&self, document: &mut LynxDocument, overlay: NodeId, dialog: NodeId) {
        if passes_through(attribute(document, overlay, EVENTS_PASS_THROUGH_ATTRIBUTE)) {
            document
                .show_popover(dialog)
                .expect("a hidden manual popover in a connected host shows");
        } else {
            // A removal left a modal dialog open and not modal (HTML's
            // removing steps); `showModal()` refuses an open one.
            close(document, dialog, &self.events);
            show_modal(document, dialog).expect("a closed dialog in a connected host shows");
        }
    }

    /// Hides the shadow dialog the way it was shown: `hidePopover()`, or
    /// `close()`, which also closes a dialog a removal left open.
    fn hide_dialog(&self, document: &mut LynxDocument, dialog: NodeId) {
        if document.popover_showing(dialog) {
            document
                .hide_popover(dialog)
                .expect("a showing manual popover hides");
        } else {
            close(document, dialog, &self.events);
        }
    }
}

impl CustomElement<()> for Overlay {
    fn observed_attributes(&self) -> Vec<String> {
        vec![
            VISIBLE_ATTRIBUTE.to_owned(),
            EVENTS_PASS_THROUGH_ATTRIBUTE.to_owned(),
        ]
    }

    fn constructed(&self, document: &mut LynxDocument, element: NodeId) {
        let shadow = document.attach_shadow(element, ShadowRootMode::Open);
        document.add_shadow_stylesheet(shadow, SHADOW_RULES);
        let dialog = document.create_element(DIALOG_TAG, ());
        document.set_id_attribute(dialog, Some(DIALOG_ID));
        document.set_attribute(dialog, "part", DIALOG_ID);
        document.set_attribute(dialog, "popover", "manual");
        let inner = document.create_element("div", ());
        document.set_classes(inner, INNER_CLASS);
        let slot = document.create_element("slot", ());
        document.append_child(inner, slot);
        document.append_child(dialog, inner);
        document.append_child(shadow, dialog);
    }

    /// A visible overlay entering the document enters the top layer: on
    /// first insertion, and again after a removal dropped its entry.
    fn connected_callback(&self, document: &mut LynxDocument, element: NodeId) {
        if shows(attribute(document, element, VISIBLE_ATTRIBUTE))
            && document.is_connected(element)
            && !document.in_top_layer(shadow_dialog(document, element))
        {
            self.show(document, element);
        }
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        old: Option<&str>,
        new: Option<&str>,
    ) {
        let dialog = shadow_dialog(document, element);
        let shown = document.in_top_layer(dialog);
        if name == VISIBLE_ATTRIBUTE {
            if shows(new) {
                if !shown && document.is_connected(element) {
                    self.show(document, element);
                }
            } else {
                self.hide_dialog(document, dialog);
                if shown {
                    self.events.queue(element, DISMISS_EVENT);
                }
            }
        } else {
            debug_assert_eq!(name, EVENTS_PASS_THROUGH_ATTRIBUTE);
            if shown && passes_through(old) != passes_through(new) {
                self.hide_dialog(document, dialog);
                self.show_dialog(document, element, dialog);
            }
        }
    }
}

/// The overlay's shadow `#dialog`, the first element of its shadow tree,
/// which [`Overlay::constructed`] builds for every overlay.
fn shadow_dialog(document: &LynxDocument, overlay: NodeId) -> NodeId {
    document
        .shadow_root(overlay)
        .and_then(|root| document.get(root))
        .and_then(|root| root.children().find(|child| child.is_element()))
        .map(Node::id)
        .expect("every overlay has its shadow dialog")
}

/// Whether a `visible` value shows the overlay: present and not `"false"`.
fn shows(value: Option<&str>) -> bool {
    value.is_some_and(|value| value != "false")
}

/// Whether an `events-pass-through` value lets touches through: present and
/// not `"false"`, the boolean reading `visible` has.
fn passes_through(value: Option<&str>) -> bool {
    shows(value)
}

fn attribute<'a>(document: &'a LynxDocument, element: NodeId, name: &str) -> Option<&'a str> {
    document.get(element).and_then(|node| node.attribute(name))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Authored pixel sizes lay out exactly.

    use dom::stylo::properties::PropertyId;
    use dom::stylo::values::computed::Display;
    use dom::{Node, NodeId, Point2D, StylesheetOrigin, Vector2D};

    use super::super::test_support::{
        child, display, document, element_under, style_of, with_component_events,
    };
    use super::super::{ComponentEvent, ComponentEvents, LynxDocument, PageConfig};
    use super::{OVERLAY_TAG, X_OVERLAY_TAG, shadow_dialog};

    /// The phone viewport `test_support` builds.
    const VIEWPORT: (f32, f32) = (393.0, 727.0);

    /// Both tags, which must behave identically.
    const TAGS: [&str; 2] = [OVERLAY_TAG, X_OVERLAY_TAG];

    /// A counter card's overlay class: a `display` and a translucent green
    /// background on the host, which must neither show a hidden overlay nor
    /// paint over the viewport.
    const HOST_CLASS: &str = ".host { display: flex; background-color: rgba(0, 255, 0, 0.2); \
                              width: 100%; height: 100%; }";

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

    fn matches(document: &LynxDocument, element: NodeId, selector: &str) -> bool {
        document
            .matches(element, selector)
            .expect("a valid selector")
    }

    /// The first element child of `parent`.
    fn first_element(document: &LynxDocument, parent: NodeId) -> NodeId {
        document
            .get(parent)
            .expect("live")
            .children()
            .find(|child| child.is_element())
            .map(Node::id)
            .expect("an element child")
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
        document.layout();
        document.commit();
        document.elements_from_point(Point2D::new(x, y))
    }

    /// An overlay under the page holding a 100 by 50 first child.
    fn overlay(document: &mut LynxDocument, tag: &str) -> (NodeId, NodeId) {
        let overlay = child(document, tag, "");
        let content = element_under(
            document,
            overlay,
            "view",
            "width: 100px; height: 50px; background-color: red",
        );
        (overlay, content)
    }

    // --- the shadow tree ----------------------------------------------------

    /// web-core's template less its placeholder: `dialog#dialog[part=dialog]`
    /// holding `div.overlay-inner` holding the default slot, which every
    /// child of the host is assigned to. The host's child list is its light
    /// children alone, and the host generates no box, shown or hidden.
    #[test]
    fn the_shadow_tree_is_a_dialog_holding_the_slot_and_the_host_has_no_box() {
        for tag in TAGS {
            let mut document = document();
            let (overlay, content) = overlay(&mut document, tag);
            let second = element_under(&mut document, overlay, "view", "");
            let dialog = shadow_dialog(&document, overlay);
            let inner = first_element(&document, dialog);
            let slot = first_element(&document, inner);
            let node = document.get(dialog).expect("live");
            assert_eq!(node.tag_name(), Some("dialog"), "{tag}");
            assert_eq!(node.attribute("id"), Some("dialog"), "{tag}");
            assert_eq!(node.attribute("part"), Some("dialog"), "{tag}");
            assert_eq!(node.attribute("popover"), Some("manual"), "{tag}");
            assert_eq!(
                document.get(inner).expect("live").attribute("class"),
                Some("overlay-inner"),
                "{tag}"
            );
            assert_eq!(
                document.get(slot).expect("live").tag_name(),
                Some("slot"),
                "{tag}"
            );
            assert_eq!(document.assigned_nodes(slot), [content, second], "{tag}");
            assert_eq!(
                document.get(overlay).expect("live").child_ids(),
                [content, second],
                "{tag}"
            );

            for value in [None, Some("")] {
                if let Some(value) = value {
                    document.set_attribute(overlay, "visible", value);
                }
                document.layout();
                assert_eq!(display(&document, overlay), Display::Contents, "{tag}");
                assert!(
                    document.bounding_client_rect(overlay).is_none(),
                    "{tag}: {value:?}"
                );
                assert!(!document.in_top_layer(overlay), "{tag}: {value:?}");
            }
        }
    }

    // --- showing and hiding --------------------------------------------------

    /// A hidden overlay's dialog is closed and generates no box, so neither
    /// does its content; `visible="false"` is hidden; `visible=""` and
    /// `visible="true"` open the dialog in the top layer, with the display
    /// `defaultDisplayLinear` picks for `dialog`.
    #[test]
    fn visible_opens_the_shadow_dialog_and_false_closes_it() {
        for tag in TAGS {
            for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
                let (mut document, _events) = with_component_events(PageConfig {
                    default_display_linear: linear,
                    ..PageConfig::default()
                });
                let (overlay, content) = overlay(&mut document, tag);
                let dialog = shadow_dialog(&document, overlay);
                document.layout();
                assert_eq!(display(&document, dialog), Display::None, "{tag}: absent");
                assert!(document.bounding_client_rect(content).is_none(), "{tag}");

                for (value, shown) in [("false", false), ("", true), ("true", true)] {
                    document.set_attribute(overlay, "visible", value);
                    document.layout();
                    let expected = if shown { expected } else { Display::None };
                    assert_eq!(display(&document, dialog), expected, "{tag}: {value:?}");
                    assert_eq!(document.in_top_layer(dialog), shown, "{tag}: {value:?}");
                    assert_eq!(
                        matches(&document, dialog, ":open"),
                        shown,
                        "{tag}: {value:?}"
                    );
                    assert_eq!(
                        document.bounding_client_rect(content).is_some(),
                        shown,
                        "{tag}: {value:?}"
                    );
                }
                document.remove_attribute(overlay, "visible");
                document.layout();
                assert_eq!(display(&document, dialog), Display::None, "{tag}");
                assert!(!document.in_top_layer(dialog), "{tag}");
                assert!(document.bounding_client_rect(content).is_none(), "{tag}");
            }
        }
    }

    /// An author `display` and background on the host — a counter card's
    /// overlay class — do not show a hidden overlay: hidden is the shadow
    /// dialog's state, which no document rule reaches. The host gets a box,
    /// and web-core's host rule keeps it zero-sized.
    #[test]
    fn an_author_display_on_the_host_does_not_show_a_hidden_overlay() {
        for tag in TAGS {
            let mut document = document();
            document.add_stylesheet(HOST_CLASS, StylesheetOrigin::Author);
            let (overlay, content) = overlay(&mut document, tag);
            document.set_classes(overlay, "host");
            document.layout();
            assert_eq!(display(&document, overlay), Display::Flex, "{tag}");
            assert_eq!(
                display(&document, shadow_dialog(&document, overlay)),
                Display::None,
                "{tag}"
            );
            assert!(
                document.bounding_client_rect(content).is_none(),
                "{tag}: the content stays hidden"
            );
            let (_, top, width, height) = rect(&document, overlay);
            assert_eq!((top, width, height), (0.0, 0.0, 0.0), "{tag}");
            for (property, expected) in [
                ("position", "fixed"),
                ("contain", "strict"),
                ("max-width", "0px"),
                ("max-height", "0px"),
            ] {
                assert_eq!(
                    value(&document, overlay, property),
                    expected,
                    "{tag}: {property}"
                );
            }

            document.set_attribute(overlay, "visible", "");
            document.layout();
            assert_eq!(rect(&document, content), (0.0, 0.0, 100.0, 50.0), "{tag}");
            document.set_attribute(overlay, "visible", "false");
            document.layout();
            assert!(document.bounding_client_rect(content).is_none(), "{tag}");
        }
    }

    /// The box that fills the viewport is the shadow dialog, and it is
    /// transparent, its `::backdrop` too: an author background on the host
    /// paints on a zero-sized box, and an author rule for `dialog` does not
    /// reach the shadow one. Outside the content, a point hits the dialog
    /// and nothing of the host.
    #[test]
    fn the_viewport_is_filled_by_a_transparent_dialog_the_host_s_style_does_not_reach() {
        for tag in TAGS {
            let mut document = document();
            document.add_stylesheet(HOST_CLASS, StylesheetOrigin::Author);
            document.add_stylesheet(
                "dialog { background-color: rgb(255, 0, 0); padding: 7px; } \
                 dialog::backdrop { background-color: rgb(255, 0, 0); }",
                StylesheetOrigin::Author,
            );
            let (overlay, _) = overlay(&mut document, tag);
            document.set_classes(overlay, "host");
            document.set_attribute(overlay, "visible", "");
            document.layout();
            let dialog = shadow_dialog(&document, overlay);
            assert_eq!(rect(&document, dialog), (0.0, 0.0, VIEWPORT.0, VIEWPORT.1));
            for (property, expected) in [
                ("background-color", "rgba(0, 0, 0, 0)"),
                ("padding-top", "0px"),
                ("border-top-width", "0px"),
                ("margin-top", "0px"),
                ("max-width", "100%"),
            ] {
                assert_eq!(
                    value(&document, dialog, property),
                    expected,
                    "{tag}: {property}"
                );
            }
            let (_, _, width, height) = rect(&document, overlay);
            assert_eq!((width, height), (0.0, 0.0), "{tag}");
            assert_eq!(
                hits(&mut document, 200.0, 300.0),
                vec![dialog],
                "{tag}: the dialog, not the host or a red backdrop below it"
            );
        }
    }

    /// `::part(dialog)` is the way to the shadow dialog, as in web-core.
    #[test]
    fn part_dialog_reaches_the_shadow_dialog() {
        for tag in TAGS {
            let mut document = document();
            document.add_stylesheet(
                &format!("{tag}::part(dialog) {{ background-color: rgb(1, 2, 3); }}"),
                StylesheetOrigin::Author,
            );
            let (overlay, _) = overlay(&mut document, tag);
            document.set_attribute(overlay, "visible", "");
            document.layout();
            let dialog = shadow_dialog(&document, overlay);
            assert_eq!(
                value(&document, dialog, "background-color"),
                "rgb(1, 2, 3)",
                "{tag}"
            );
        }
    }

    // --- the content ---------------------------------------------------------

    /// The shown dialog fills the viewport wherever the overlay is written;
    /// the first child sits at the viewport's top-left with its own size; a
    /// second child generates no box, even with an author `display`.
    #[test]
    fn the_dialog_fills_the_viewport_and_only_the_first_child_renders() {
        for tag in TAGS {
            let mut document = document();
            let wrapper = child(
                &mut document,
                "view",
                "margin: 300px 0 0 40px; width: 50px; height: 50px; \
                 transform: translate(10px, 10px); overflow: clip",
            );
            let overlay = element_under(&mut document, wrapper, tag, "");
            let first = element_under(&mut document, overlay, "view", "width: 100px; height: 50px");
            let second = element_under(
                &mut document,
                overlay,
                "view",
                "display: flex; width: 30px; height: 30px",
            );
            document.set_attribute(overlay, "visible", "true");
            document.layout();
            let dialog = shadow_dialog(&document, overlay);
            assert_eq!(
                rect(&document, dialog),
                (0.0, 0.0, VIEWPORT.0, VIEWPORT.1),
                "{tag}"
            );
            assert_eq!(
                rect(&document, first_element(&document, dialog)),
                (0.0, 0.0, VIEWPORT.0, VIEWPORT.1),
                "{tag}: `.overlay-inner` fills the dialog"
            );
            assert_eq!(rect(&document, first), (0.0, 0.0, 100.0, 50.0), "{tag}");
            assert_eq!(display(&document, second), Display::None, "{tag}");
            assert!(document.bounding_client_rect(second).is_none(), "{tag}");
        }
    }

    /// A `wrapper` as the overlay's first child: the wrapper's own first
    /// child sits at the viewport's top-left with its own size, and the
    /// wrapper's later children still render (web-core hides only the
    /// overlay's direct children), in `.overlay-inner`'s flow, which the
    /// absolute first child has left.
    #[test]
    fn the_first_child_through_a_wrapper_sits_at_the_viewport_s_top_left() {
        for tag in TAGS {
            let mut document = document();
            let overlay = child(&mut document, tag, "");
            let wrapper = element_under(&mut document, overlay, "wrapper", "");
            let first = element_under(&mut document, wrapper, "view", "width: 100px; height: 50px");
            let sibling =
                element_under(&mut document, wrapper, "view", "width: 30px; height: 20px");
            document.set_attribute(overlay, "visible", "");
            document.layout();
            assert_eq!(display(&document, wrapper), Display::Contents, "{tag}");
            assert_eq!(rect(&document, first), (0.0, 0.0, 100.0, 50.0), "{tag}");
            assert_ne!(display(&document, sibling), Display::None, "{tag}");
            // The first child is out of flow, so the sibling starts the
            // flow at the top-left too; in flow it would sit at y 50.
            assert_eq!(
                rect(&document, sibling),
                (0.0, 0.0, 30.0, 20.0),
                "{tag}: the sibling renders, in `.overlay-inner`'s flow"
            );
        }
    }

    // --- the top layer and hit testing ---------------------------------------

    /// A shown overlay's dialog is in the top layer: a modal dialog, which
    /// blocks the document, unless `events-pass-through` says otherwise,
    /// when it is a popover, which blocks nothing; toggling the attribute
    /// while it is shown switches between the two.
    #[test]
    fn events_pass_through_decides_whether_the_overlay_blocks_the_document() {
        const MODAL: (bool, bool, bool) = (true, false, true);
        const POPOVER: (bool, bool, bool) = (false, true, false);
        for tag in TAGS {
            let mut document = document();
            let (overlay, _) = overlay(&mut document, tag);
            let dialog = shadow_dialog(&document, overlay);
            // `(:modal, :popover-open, :open)`.
            let modal = |document: &mut LynxDocument| {
                document.layout();
                (
                    matches(document, dialog, ":modal"),
                    matches(document, dialog, ":popover-open"),
                    matches(document, dialog, ":open"),
                )
            };
            document.set_attribute(overlay, "visible", "");
            assert!(document.in_top_layer(dialog), "{tag}");
            assert!(document.blocks_document(dialog), "{tag}");
            assert_eq!(modal(&mut document), MODAL, "{tag}");

            assert_eq!(value(&document, dialog, "pointer-events"), "auto", "{tag}");

            document.set_attribute(overlay, "events-pass-through", "");
            assert!(document.in_top_layer(dialog), "{tag}");
            assert!(!document.blocks_document(dialog), "{tag}");
            assert_eq!(modal(&mut document), POPOVER, "{tag}: a popover");
            // The popover is laid out as the modal dialog is.
            for (property, expected) in [
                ("pointer-events", "none"),
                ("position", "fixed"),
                ("overflow-y", "scroll"),
                ("overscroll-behavior-y", "contain"),
            ] {
                assert_eq!(
                    value(&document, dialog, property),
                    expected,
                    "{tag}: {property}"
                );
            }
            assert_eq!(rect(&document, dialog), (0.0, 0.0, VIEWPORT.0, VIEWPORT.1));
            document.set_attribute(overlay, "events-pass-through", "false");
            assert!(document.blocks_document(dialog), "{tag}: `\"false\"`");
            document.set_attribute(overlay, "events-pass-through", "true");
            assert!(!document.blocks_document(dialog), "{tag}");
            document.remove_attribute(overlay, "events-pass-through");
            assert!(document.blocks_document(dialog), "{tag}");
            assert_eq!(modal(&mut document), MODAL, "{tag}");
            assert_eq!(document.top_layer().count(), 1, "{tag}");

            // Shown with the attribute already present.
            document.remove_attribute(overlay, "visible");
            document.set_attribute(overlay, "events-pass-through", "");
            assert!(!document.in_top_layer(dialog), "{tag}: hidden stays out");
            document.set_attribute(overlay, "visible", "");
            assert!(document.in_top_layer(dialog), "{tag}");
            assert!(!document.blocks_document(dialog), "{tag}");
            assert_eq!(modal(&mut document), POPOVER, "{tag}");
            document.remove_attribute(overlay, "visible");
            assert_eq!(modal(&mut document), (false, false, false), "{tag}");
            assert!(!document.in_top_layer(dialog), "{tag}");
        }
    }

    /// Outside the content, a blocking overlay answers with its dialog — the
    /// target a composed event retargets to the host — and nothing below is
    /// hit; a pass-through overlay lets the page content below answer. A
    /// point on the content hits the content either way.
    #[test]
    fn a_touch_outside_the_content_reaches_the_page_only_through_a_pass_through_overlay() {
        for tag in TAGS {
            let mut document = document();
            let page = document.document_element().id();
            let below = child(
                &mut document,
                "view",
                "position: absolute; left: 0; top: 0; width: 393px; height: 727px",
            );
            let (overlay, content) = overlay(&mut document, tag);
            let dialog = shadow_dialog(&document, overlay);
            assert_eq!(
                hits(&mut document, 200.0, 300.0),
                vec![below, page],
                "{tag}"
            );

            document.set_attribute(overlay, "visible", "");
            assert_eq!(
                hits(&mut document, 200.0, 300.0),
                vec![dialog],
                "{tag}: blocked"
            );
            let steps = document.event_steps(dialog, true, true);
            let light: Vec<(NodeId, NodeId)> = steps
                .steps()
                .iter()
                .filter(|step| !step.capture && [overlay, page].contains(&step.node))
                .map(|step| (step.node, step.target))
                .collect();
            assert_eq!(
                light,
                vec![(overlay, overlay), (page, overlay)],
                "{tag}: the host is the target outside the shadow tree"
            );
            assert_eq!(
                hits(&mut document, 10.0, 10.0).first(),
                Some(&content),
                "{tag}"
            );

            document.set_attribute(overlay, "events-pass-through", "");
            assert_eq!(
                hits(&mut document, 200.0, 300.0),
                vec![below, page],
                "{tag}: passed through"
            );
            assert_eq!(
                hits(&mut document, 10.0, 10.0).first(),
                Some(&content),
                "{tag}"
            );
        }
    }

    /// A drag on the overlay's content ends its scroll chain at the dialog,
    /// a top-layer element, and never reaches a scroller the overlay is
    /// written in: what web-core's 1px placeholder does in a browser.
    #[test]
    fn a_drag_on_the_overlay_does_not_scroll_the_page_below() {
        for tag in TAGS {
            let mut document = document();
            let scroller = child(
                &mut document,
                "scroll-view",
                "width: 393px; height: 727px; overflow-y: scroll",
            );
            element_under(
                &mut document,
                scroller,
                "view",
                "flex-shrink: 0; height: 2000px",
            );
            let overlay = element_under(&mut document, scroller, tag, "");
            let content =
                element_under(&mut document, overlay, "view", "width: 100px; height: 50px");
            document.set_attribute(overlay, "visible", "");
            document.commit();
            assert!(document.scroll_box(scroller).is_some(), "{tag}");
            let moved = document.scroll_chain(content, Vector2D::new(0.0, 100.0));
            assert_eq!(moved, None, "{tag}");
            assert_eq!(document.scroll_offset(scroller), Vector2D::zero(), "{tag}");
        }
    }

    // --- events ---------------------------------------------------------------

    /// `showoverlay` when `visible` shows a connected overlay, and
    /// `dismissoverlay` when it stops: removed or `"false"`. A rewrite that
    /// keeps it shown fires nothing, and so does switching it to a popover.
    /// Both are queued at the host. `close()` on the modal dialog queues the
    /// dialog's own `close` at the dialog, as web-core's does; it is not
    /// composed, so it never leaves the shadow tree
    /// (`an_overlay_shows_and_dismisses_through_set_attribute` in
    /// `main/runtime` delivers it to nothing).
    #[test]
    fn visible_transitions_queue_showoverlay_and_dismissoverlay() {
        for tag in TAGS {
            for removal in [None, Some("false")] {
                let (mut document, events) = with_component_events(PageConfig::default());
                let (overlay, _) = overlay(&mut document, tag);
                let dialog = shadow_dialog(&document, overlay);
                assert_eq!(queued(&events), vec![], "{tag}");
                document.set_attribute(overlay, "visible", "true");
                assert_eq!(queued(&events), vec![(overlay, "showoverlay")], "{tag}");
                document.set_attribute(overlay, "visible", "");
                assert_eq!(queued(&events), vec![], "{tag}: still shown");
                document.set_attribute(overlay, "events-pass-through", "");
                assert_eq!(
                    queued(&events),
                    vec![(dialog, "close")],
                    "{tag}: nothing at the host for pass-through"
                );
                match removal {
                    None => document.remove_attribute(overlay, "visible"),
                    Some(value) => document.set_attribute(overlay, "visible", value),
                }
                assert!(!document.in_top_layer(dialog), "{tag}: {removal:?}");
                assert_eq!(
                    queued(&events),
                    vec![(overlay, "dismissoverlay")],
                    "{tag}: {removal:?}: a popover hides with no `close`"
                );
                document.set_attribute(overlay, "visible", "false");
                assert_eq!(queued(&events), vec![], "{tag}: already hidden");

                // A modal one dismissed.
                document.remove_attribute(overlay, "events-pass-through");
                document.set_attribute(overlay, "visible", "");
                assert_eq!(queued(&events), vec![(overlay, "showoverlay")], "{tag}");
                match removal {
                    None => document.remove_attribute(overlay, "visible"),
                    Some(value) => document.set_attribute(overlay, "visible", value),
                }
                assert_eq!(
                    queued(&events),
                    vec![(dialog, "close"), (overlay, "dismissoverlay")],
                    "{tag}: {removal:?}"
                );
            }
        }
    }

    /// `visible` on a disconnected overlay does nothing until it connects;
    /// connecting a visible overlay shows it; removing a shown overlay fires
    /// nothing; reconnecting it shows it again; one hidden while disconnected
    /// comes back closed.
    #[test]
    fn connection_shows_a_visible_overlay_and_removal_fires_nothing() {
        for tag in TAGS {
            let (mut document, events) = with_component_events(PageConfig::default());
            let page = document.document_element().id();
            let overlay = document.create_element(tag, ());
            let dialog = shadow_dialog(&document, overlay);
            document.set_attribute(overlay, "visible", "");
            assert!(!document.in_top_layer(dialog), "{tag}");
            assert_eq!(queued(&events), vec![], "{tag}: disconnected");

            document.append_child(page, overlay);
            assert!(document.in_top_layer(dialog), "{tag}");
            assert!(document.blocks_document(dialog), "{tag}");
            assert_eq!(queued(&events), vec![(overlay, "showoverlay")], "{tag}");

            document.remove_element(overlay);
            assert!(!document.in_top_layer(dialog), "{tag}");
            assert_eq!(queued(&events), vec![], "{tag}: removal fires nothing");

            // The removal left the dialog open and not modal, so it is
            // closed before `showModal()`, with its own `close`.
            document.append_child(page, overlay);
            assert!(document.in_top_layer(dialog), "{tag}");
            assert!(document.blocks_document(dialog), "{tag}");
            assert_eq!(
                queued(&events),
                vec![(dialog, "close"), (overlay, "showoverlay")],
                "{tag}: reconnected"
            );

            // Hidden while out of the document: back closed, and nothing at
            // the host.
            document.remove_element(overlay);
            document.remove_attribute(overlay, "visible");
            document.append_child(page, overlay);
            document.layout();
            assert!(!document.in_top_layer(dialog), "{tag}");
            assert!(!matches(&document, dialog, ":open"), "{tag}");
            assert_eq!(display(&document, dialog), Display::None, "{tag}");
            assert_eq!(queued(&events), vec![(dialog, "close")], "{tag}");

            // A pass-through one comes back from a removal hidden, as a
            // popover does, and shows again.
            document.set_attribute(overlay, "events-pass-through", "");
            document.set_attribute(overlay, "visible", "");
            assert!(document.popover_showing(dialog), "{tag}");
            document.remove_element(overlay);
            assert!(!document.popover_showing(dialog), "{tag}");
            document.append_child(page, overlay);
            assert!(document.popover_showing(dialog), "{tag}");
            assert_eq!(
                queued(&events),
                vec![(overlay, "showoverlay"), (overlay, "showoverlay")],
                "{tag}"
            );

            // A hidden overlay connecting fires nothing.
            let hidden = document.create_element(tag, ());
            document.set_attribute(hidden, "visible", "false");
            document.append_child(page, hidden);
            assert!(
                !document.in_top_layer(shadow_dialog(&document, hidden)),
                "{tag}"
            );
            assert_eq!(queued(&events), vec![], "{tag}");
        }
    }

    /// Overlays stack in show order, the later one on top.
    #[test]
    fn overlays_stack_in_the_order_they_were_shown() {
        let mut document = document();
        let (first, first_content) = overlay(&mut document, OVERLAY_TAG);
        let (second, second_content) = overlay(&mut document, X_OVERLAY_TAG);
        document.set_attribute(second, "visible", "");
        document.set_attribute(first, "visible", "");
        let layer: Vec<NodeId> = document
            .top_layer()
            .map(dom::TopLayerEntry::element)
            .collect();
        assert_eq!(
            layer,
            vec![
                shadow_dialog(&document, second),
                shadow_dialog(&document, first)
            ]
        );
        assert_eq!(
            hits(&mut document, 10.0, 10.0).first(),
            Some(&first_content)
        );
        document.remove_attribute(first, "visible");
        assert_eq!(
            hits(&mut document, 10.0, 10.0).first(),
            Some(&second_content)
        );
    }
}
