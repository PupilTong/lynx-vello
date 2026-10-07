//! The `overlay` tag, and `x-overlay-ng`: Lynx's modal layer, over `dom`'s
//! top layer (css-position-4 §3, `dom::tree::top_layer`).
//!
//! A Lynx-only component with no W3C equivalent, so `AGENTS.md`'s standards
//! policy puts it in the second bucket: this module implements what Lynx does
//! with it, through the engine's own top layer. web-core builds it from a
//! `<dialog>` in the element's shadow tree opened with `showModal()`, over a
//! transparent `::backdrop` (`web-elements/src/elements/XOverlayNg/*`,
//! `htmlTemplates.ts:136-172`). Here the **host element itself is the
//! top-layer element**: no shadow tree, no inner dialog.
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
//! # The UA rules
//!
//! [`UA_RULES`] list both tags in every selector.
//!
//! - **Hidden is `display: none`.** `:not([visible])` and `[visible="false"]`: `visible` is a
//!   boolean attribute, this engine's `__SetAttribute` stringifies `false`, and web-core removes an
//!   attribute whose value is `"false"` before its CSS sees it
//!   (`web-elements/src/element-reactive/component.ts:158-205`). The component reads it the same
//!   way.
//! - **The host fills the viewport.** `position: fixed; inset: 0`, as web-core's `#dialog[open]`
//!   with `::part(dialog) { width: 100%; height: 100%; padding: 0; border: 0; margin: 0;
//!   background: transparent }` (`x-overlay-ng.css:6-16`) does. The rule also declares
//!   `-servo-top-layer: auto`, the UA-only longhand that switches on Stylo's §3.1 computed-value
//!   fixups (an author `position` other than `absolute`/`fixed` computes to `absolute`, `display:
//!   contents` is blockified), as `dialog:modal` does ([`super::dialog`]).
//! - **`display`.** Both tags are in [`super::ua_sheet`]'s display line beside `dialog`, so the
//!   host gets `linear` under `defaultDisplayLinear` and the fork's initial `flex` otherwise. They
//!   are not in the common container block: no `overflow: clip` (native never clips the overlay,
//!   and the viewport clips anyway) and no `position: relative` (the host is `fixed`).
//! - **The first child sits at the viewport's top-left.** `position: absolute; top: 0; left: 0`,
//!   web-core's `x-overlay-ng > *:first-child` (`x-overlay-ng.css:31-36`) and native's first child
//!   laid out against the window at (0, 0) (`LynxUIOverlay.m:30-84`, Android
//!   `LynxUIOverlayShadowNode.kt:10-40`). Its containing block is the host, which as a top-layer
//!   element is the initial containing block. web-core's `display: flex` on that child is not
//!   copied: the child keeps the display its own tag gets.
//! - **Only the first child renders.** `display: none !important` on every other child, as
//!   web-core's `x-overlay-ng > *:not(:first-child)` (`x-overlay-ng.css:27-29`), and native
//!   measures child 0 alone. This is the sheet's one `!important` for the tag, recorded in
//!   `docs/style-assumptions.md` §D.15: web-core pins it the same way, and an author `display` on a
//!   second child would otherwise make it render where neither reference does.
//! - **The `::backdrop` is transparent.** No `overlay::backdrop` rule is written: the generic
//!   `::backdrop` rule [`super::dialog`] carries gives it a box and no background, which is
//!   web-core's `#dialog::backdrop { background-color: transparent }`.
//! - **`events-pass-through`.** Present and not `"false"`: `pointer-events: none` on the host and
//!   `auto` on its children. `pointer-events` inherits, so the host and its `::backdrop` (whose
//!   style inherits from the host) are not hit-testable while the children and their subtrees are —
//!   web-core's `.overlay-inner { pointer-events: none } .overlay-inner > * { pointer-events: auto
//!   }` recipe.
//!
//! # State
//!
//! The component ([`Overlay`]) keeps no Rust state per element; it holds the
//! document's [`ComponentEvents`] queue, as [`super::image`]'s does.
//!
//! - **Shown** is the `visible` attribute, present and not `"false"`, on a connected element. A
//!   shown overlay is in the top layer, entered with `blocks_document` true unless
//!   `events-pass-through` is present and not `"false"`.
//! - The `visible` callback enters the layer when the attribute starts to show a connected overlay
//!   that is not in it, and leaves it when the attribute stops showing one that is. A rewrite that
//!   keeps it shown (`"true"` to `""`) does nothing. Writing `visible` on a disconnected element
//!   does nothing until it connects.
//! - The `events-pass-through` callback re-enters a shown overlay with the new blocking flag when
//!   the flag changes. [`dom::Document::add_to_top_layer`] moves an entry to the top with a fresh
//!   `::backdrop`, so toggling the attribute also raises the overlay above any shown after it;
//!   accepted.
//! - [`Overlay::connected_callback`] enters the layer when `visible` shows the overlay: web-core
//!   replays the present attributes at connect (`component.ts:226-256`), and `visible` is a
//!   `noDomMeasure = false` handler, so it runs only once connected (`XOverlayAttributes.ts:51`).
//!   Disconnection is `dom`'s: the unlink drops the entry (HTML's removing steps), with no event. A
//!   reconnected visible overlay enters again.
//! - Overlays stack in the order they were shown: each enters at the top of the layer.
//!
//! No element state (`:open`, `:modal`) is set; selectors key on the
//! attributes. Nothing is handled in `handle_event`, and there is no UI
//! method: native and web-core expose none to script.
//!
//! # Events
//!
//! `showoverlay` when the overlay enters the top layer and `dismissoverlay`
//! when `visible` takes it out, queued on [`ComponentEvents`] like the
//! dialog's `close` and delivered from an entry of their own, non-bubbling,
//! with a `{}` detail — web-core dispatches both with `bubbles: false,
//! composed: false` and no detail (`commonEventInitConfiguration.ts:5-9`).
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
//!   web-core.
//! - `mode`, `status-bar-translucent`, `status-bar-translucent-style`, `cut-out-mode`,
//!   `custom-layout`, `always-show`, `nest-scroll`, `ignore-focus`, `allow-pan-gesture`,
//!   `ios-enable-swipe-back`, the `android-*` attributes, `overlay-id` and `compat-bounding-rect`:
//!   platform window and gesture policy with no counterpart in this engine's one-window view, and
//!   none of them is read by web-core.
//! - web-core's pass-through re-dispatch (close the dialog, `elementFromPoint`, a synthetic
//!   `click`, `showModal()` on the next frame, `XOverlayAttributes.ts:69-88`) is a browser
//!   workaround. Here a pass-through overlay does not block the document and its host is not
//!   hit-testable, so every gesture outside the children, scrolls included, reaches what is below,
//!   as in native (`LynxOverlayContainer.m:169-214`, `LynxOverlayView.kt:726-752`).

use dom::{CustomElement, NodeId};

use super::{ComponentEvents, LynxDocument};

/// Native's tag, and the JSX tag a compiled card writes.
pub(super) const OVERLAY_TAG: &str = "overlay";
/// web-core's tag, native's legacy alias.
pub(super) const X_OVERLAY_TAG: &str = "x-overlay-ng";
/// The boolean attribute that shows the overlay.
const VISIBLE_ATTRIBUTE: &str = "visible";
/// The boolean attribute that lets touches outside the children through.
const EVENTS_PASS_THROUGH_ATTRIBUTE: &str = "events-pass-through";
/// The event queued when the overlay enters the top layer.
const SHOW_EVENT: &str = "showoverlay";
/// The event queued when `visible` takes the overlay out of the top layer.
const DISMISS_EVENT: &str = "dismissoverlay";

/// The overlay's UA rules, as the module documentation lists. The display
/// both tags get otherwise is [`super::ua_sheet`]'s. The one `!important`
/// line stays on a line of its own:
/// `the_ua_sheet_is_important_free_apart_from_the_text_block` reads the sheet
/// line by line.
pub(super) const UA_RULES: &str = r#"
overlay:not([visible]), overlay[visible="false"],
x-overlay-ng:not([visible]), x-overlay-ng[visible="false"] { display: none; }
overlay, x-overlay-ng { -servo-top-layer: auto; position: fixed; inset: 0; }
overlay > :first-child, x-overlay-ng > :first-child { position: absolute; top: 0; left: 0; }
overlay > :not(:first-child), x-overlay-ng > :not(:first-child) { display: none !important; }
overlay[events-pass-through]:not([events-pass-through="false"]),
x-overlay-ng[events-pass-through]:not([events-pass-through="false"]) { pointer-events: none; }
overlay[events-pass-through]:not([events-pass-through="false"]) > *,
x-overlay-ng[events-pass-through]:not([events-pass-through="false"]) > * { pointer-events: auto; }
"#;

/// Installs the component under both tag names, over the queue its events go
/// into. Must run before any element could carry either tag, which is
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

/// The overlay component: keeps top-layer membership in step with `visible`,
/// `events-pass-through` and connection, and queues `showoverlay` and
/// `dismissoverlay` on the transitions.
struct Overlay {
    events: ComponentEvents,
}

impl Overlay {
    /// Enters the top layer and queues `showoverlay`.
    fn show(&self, document: &mut LynxDocument, element: NodeId) {
        let blocks = !passes_through(attribute(document, element, EVENTS_PASS_THROUGH_ATTRIBUTE));
        document.add_to_top_layer(element, blocks);
        self.events.queue(element, SHOW_EVENT);
    }
}

impl CustomElement<()> for Overlay {
    fn observed_attributes(&self) -> Vec<String> {
        vec![
            VISIBLE_ATTRIBUTE.to_owned(),
            EVENTS_PASS_THROUGH_ATTRIBUTE.to_owned(),
        ]
    }

    /// A visible overlay entering the document enters the top layer: on
    /// first insertion, and again after a removal dropped its entry.
    fn connected_callback(&self, document: &mut LynxDocument, element: NodeId) {
        if shows(attribute(document, element, VISIBLE_ATTRIBUTE))
            && document.is_connected(element)
            && !document.in_top_layer(element)
        {
            self.show(document, element);
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
        let in_layer = document.in_top_layer(element);
        if name == VISIBLE_ATTRIBUTE {
            if shows(new) {
                if !in_layer && document.is_connected(element) {
                    self.show(document, element);
                }
            } else if in_layer {
                document.remove_from_top_layer(element);
                self.events.queue(element, DISMISS_EVENT);
            }
        } else {
            debug_assert_eq!(name, EVENTS_PASS_THROUGH_ATTRIBUTE);
            let blocks = !passes_through(new);
            if in_layer && document.blocks_document(element) != blocks {
                document.add_to_top_layer(element, blocks);
            }
        }
    }
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

    use dom::stylo::values::computed::Display;
    use dom::{NodeId, Point2D};

    use super::super::test_support::{
        child, display, document, element_under, with_component_events,
    };
    use super::super::{ComponentEvent, ComponentEvents, LynxDocument, PageConfig};
    use super::{OVERLAY_TAG, X_OVERLAY_TAG};

    /// The phone viewport `test_support` builds.
    const VIEWPORT: (f32, f32) = (393.0, 727.0);

    /// Both tags, which must behave identically.
    const TAGS: [&str; 2] = [OVERLAY_TAG, X_OVERLAY_TAG];

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

    // --- the UA rules -------------------------------------------------------

    /// A hidden overlay generates no box; `visible="false"` is hidden;
    /// `visible=""` and `visible="true"` show it, with the display
    /// `defaultDisplayLinear` picks.
    #[test]
    fn visible_shows_the_overlay_and_false_hides_it() {
        for tag in TAGS {
            for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
                let (mut document, _events) = with_component_events(PageConfig {
                    default_display_linear: linear,
                    ..PageConfig::default()
                });
                let (overlay, content) = overlay(&mut document, tag);
                document.layout();
                assert_eq!(display(&document, overlay), Display::None, "{tag}: absent");
                assert!(document.bounding_client_rect(content).is_none(), "{tag}");

                for (value, shown) in [("false", false), ("", true), ("true", true)] {
                    document.set_attribute(overlay, "visible", value);
                    document.layout();
                    let expected = if shown { expected } else { Display::None };
                    assert_eq!(display(&document, overlay), expected, "{tag}: {value:?}");
                    assert_eq!(document.in_top_layer(overlay), shown, "{tag}: {value:?}");
                }
                document.remove_attribute(overlay, "visible");
                document.layout();
                assert_eq!(display(&document, overlay), Display::None, "{tag}");
            }
        }
    }

    /// The shown host fills the viewport wherever it is written; its first
    /// child sits at the viewport's top-left with its own size; a second
    /// child generates no box, even with an author `display`.
    #[test]
    fn the_host_fills_the_viewport_and_only_its_first_child_renders() {
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
            assert_eq!(
                rect(&document, overlay),
                (0.0, 0.0, VIEWPORT.0, VIEWPORT.1),
                "{tag}"
            );
            assert_eq!(rect(&document, first), (0.0, 0.0, 100.0, 50.0), "{tag}");
            assert_eq!(display(&document, second), Display::None, "{tag}");
            assert!(document.bounding_client_rect(second).is_none(), "{tag}");
        }
    }

    // --- the top layer and hit testing ---------------------------------------

    /// A shown overlay is in the top layer and blocks the document unless
    /// `events-pass-through` says otherwise; toggling the attribute while it
    /// is shown flips the flag.
    #[test]
    fn events_pass_through_decides_whether_the_overlay_blocks_the_document() {
        for tag in TAGS {
            let mut document = document();
            let (overlay, _) = overlay(&mut document, tag);
            document.set_attribute(overlay, "visible", "");
            assert!(document.in_top_layer(overlay), "{tag}");
            assert!(document.blocks_document(overlay), "{tag}");

            document.set_attribute(overlay, "events-pass-through", "");
            assert!(document.in_top_layer(overlay), "{tag}");
            assert!(!document.blocks_document(overlay), "{tag}");
            document.set_attribute(overlay, "events-pass-through", "false");
            assert!(document.blocks_document(overlay), "{tag}: `\"false\"`");
            document.set_attribute(overlay, "events-pass-through", "true");
            assert!(!document.blocks_document(overlay), "{tag}");
            document.remove_attribute(overlay, "events-pass-through");
            assert!(document.blocks_document(overlay), "{tag}");
            assert_eq!(document.top_layer().count(), 1, "{tag}");

            // Shown with the attribute already present.
            document.remove_attribute(overlay, "visible");
            document.set_attribute(overlay, "events-pass-through", "");
            assert!(!document.in_top_layer(overlay), "{tag}: hidden stays out");
            document.set_attribute(overlay, "visible", "");
            assert!(document.in_top_layer(overlay), "{tag}");
            assert!(!document.blocks_document(overlay), "{tag}");
        }
    }

    /// Outside the children, a blocking overlay answers for itself (its host
    /// or its backdrop, which reports the host) and nothing below is hit; a
    /// pass-through overlay lets the page content below answer. A point on
    /// the child hits the child either way.
    #[test]
    fn a_touch_outside_the_children_reaches_the_page_only_through_a_pass_through_overlay() {
        for tag in TAGS {
            let mut document = document();
            let page = document.document_element().id();
            let below = child(
                &mut document,
                "view",
                "position: absolute; left: 0; top: 0; width: 393px; height: 727px",
            );
            let (overlay, content) = overlay(&mut document, tag);
            assert_eq!(
                hits(&mut document, 200.0, 300.0),
                vec![below, page],
                "{tag}"
            );

            document.set_attribute(overlay, "visible", "");
            assert_eq!(
                hits(&mut document, 200.0, 300.0),
                vec![overlay],
                "{tag}: blocked"
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

    // --- events ---------------------------------------------------------------

    /// `showoverlay` when `visible` shows a connected overlay, and
    /// `dismissoverlay` when it stops: removed or `"false"`. A rewrite that
    /// keeps it shown fires nothing.
    #[test]
    fn visible_transitions_queue_showoverlay_and_dismissoverlay() {
        for tag in TAGS {
            for removal in [None, Some("false")] {
                let (mut document, events) = with_component_events(PageConfig::default());
                let (overlay, _) = overlay(&mut document, tag);
                assert_eq!(queued(&events), vec![], "{tag}");
                document.set_attribute(overlay, "visible", "true");
                assert_eq!(queued(&events), vec![(overlay, "showoverlay")], "{tag}");
                document.set_attribute(overlay, "visible", "");
                assert_eq!(queued(&events), vec![], "{tag}: still shown");
                document.set_attribute(overlay, "events-pass-through", "");
                assert_eq!(queued(&events), vec![], "{tag}: no event for pass-through");
                match removal {
                    None => document.remove_attribute(overlay, "visible"),
                    Some(value) => document.set_attribute(overlay, "visible", value),
                }
                assert!(!document.in_top_layer(overlay), "{tag}: {removal:?}");
                assert_eq!(
                    queued(&events),
                    vec![(overlay, "dismissoverlay")],
                    "{tag}: {removal:?}"
                );
                document.set_attribute(overlay, "visible", "false");
                assert_eq!(queued(&events), vec![], "{tag}: already hidden");
            }
        }
    }

    /// `visible` on a disconnected overlay does nothing until it connects;
    /// connecting a visible overlay shows it; removing a shown overlay fires
    /// nothing; reconnecting it shows it again.
    #[test]
    fn connection_shows_a_visible_overlay_and_removal_fires_nothing() {
        for tag in TAGS {
            let (mut document, events) = with_component_events(PageConfig::default());
            let page = document.document_element().id();
            let overlay = document.create_element(tag, ());
            document.set_attribute(overlay, "visible", "");
            assert!(!document.in_top_layer(overlay), "{tag}");
            assert_eq!(queued(&events), vec![], "{tag}: disconnected");

            document.append_child(page, overlay);
            assert!(document.in_top_layer(overlay), "{tag}");
            assert!(document.blocks_document(overlay), "{tag}");
            assert_eq!(queued(&events), vec![(overlay, "showoverlay")], "{tag}");

            document.remove_element(overlay);
            assert!(!document.in_top_layer(overlay), "{tag}");
            assert_eq!(queued(&events), vec![], "{tag}: removal fires nothing");

            document.append_child(page, overlay);
            assert!(document.in_top_layer(overlay), "{tag}");
            assert_eq!(
                queued(&events),
                vec![(overlay, "showoverlay")],
                "{tag}: reconnected"
            );

            // A hidden overlay connecting fires nothing.
            let hidden = document.create_element(tag, ());
            document.set_attribute(hidden, "visible", "false");
            document.append_child(page, hidden);
            assert!(!document.in_top_layer(hidden), "{tag}");
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
        assert_eq!(layer, vec![second, first]);
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
