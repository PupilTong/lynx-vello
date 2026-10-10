//! The UI methods every element has, whatever its kind: the shared base set
//! the runtime's `callElementMethod` falls back to when the element's own
//! kind answers [`MethodOutcome::NotFound`] (user ruling 2026-10-09: the
//! documented UI methods `boundingClientRect`, `scrollIntoView`,
//! `scrollTo`/`scrollBy` and `focus`/`blur`).
//!
//! Only `scrollIntoView` ([`scroll_into_view`]) is answered here.
//! `boundingClientRect` is answered by `callElementMethod` itself, ahead of
//! the kind, as web-core answers it ahead of the element
//! (`web-core/ts/client/mainthread/elementAPIs/createInvokeUIMethod.ts:16-28`).
//! The base `scrollTo`/`scrollBy` and `focus`/`blur` are not built and stay
//! [`MethodOutcome::NotFound`] (code 3); a `scroll-view`'s own `scrollTo`
//! and `scrollBy` are its kind's ([`super::scroll_container`]).

use dom::scroll::{
    ScrollBehavior, ScrollIntoViewContainer, ScrollIntoViewOptions, ScrollLogicalPosition,
};
use dom::{MethodCall, MethodError, MethodOutcome, NodeId};
use serde_json::{Map, Value};

use super::LynxDocument;
use super::scroll_container::{InvalidParams, object};

/// Runs the base method `call` names on `element`, or answers
/// [`MethodOutcome::NotFound`] for a name the base set does not build.
///
/// It runs outside the `[CEReactions]` scope
/// [`dom::Document::invoke_element_method`] opens for a kind's method, and
/// needs none: a scroll request queues no custom element reaction.
pub(crate) fn invoke_base_method(
    document: &mut LynxDocument,
    element: NodeId,
    call: MethodCall<'_>,
) -> MethodOutcome {
    match call.name {
        "scrollIntoView" => match scroll_into_view(document, element, call.params) {
            Ok(true) => MethodOutcome::Done,
            Ok(false) => MethodOutcome::Failed(MethodError::Operation),
            Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
        },
        _ => MethodOutcome::NotFound,
    }
}

/// `scrollIntoView({scrollIntoViewOptions: {behavior?, block?, inline?}})`
/// on `element`, with `params` as the JSON text the realm serialized:
/// CSSOM-View's `scrollIntoView` ([`dom::Document::scroll_into_view`])
/// with `container: "nearest"`, which is Lynx's method. Answers whether a
/// scroll container was found above the element.
///
/// - `params` must be a JSON object holding a `scrollIntoViewOptions` object, else code 4: iOS
///   (`lynx/platform/darwin/ios/lynx/ui/LynxUI.m:1554-1562`) and Android
///   (`lynx/platform/android/lynx_android/src/main/java/com/lynx/tasm/behavior/ui/LynxBaseUI.java:
///   1056-1069`) answer `PARAM_INVALID` for a missing one. A present value that is not an object
///   (`null`, a string) is code 4 too, where neither native answers anything for it: iOS sends it
///   `allKeys` regardless and Android's `HashMap` cast throws.
/// - `behavior` is `Smooth` when it is the string `"smooth"` and `Instant` otherwise (`"auto"`,
///   `"instant"`, `"none"`, missing), as iOS (`:1573`), Android (`:1080`) and web-core
///   (`lynx-stack/packages/web-platform/web-elements/src/elements/ScrollView/ScrollIntoView.ts:72`)
///   all test for `"smooth"`.
/// - `block` defaults to `"start"` and `inline` to `"nearest"` (iOS `:1551-1552`, Android
///   `:1072-1078`, the draft's defaults). The four draft keywords map to [`ScrollLogicalPosition`];
///   any other value, a non-string included, is `Start`, which is what the natives' string compares
///   and web-core's `switch` default leave.
/// - `container` is always `Nearest`: iOS walks the UI parents to the first scroller and stops
///   (`:1585-1601`), Android the same (`:1093-1109`), and in web-core the first `scroll-view` on
///   the composed path handles the `__scrollIntoView` event and stops its propagation
///   (`web-elements/src/elements/XView/XView.ts:22-37`, `ScrollIntoView.ts:22`).
/// - No scroll container above the element, or an element with no box, is code 8,
///   `OPERATION_ERROR`, as iOS (`:1603-1611`) and Android (`:1110-1115`) answer when their walk
///   finds no scroller. web-core's event reaches no handler and its call answers 0; this follows
///   native (`docs/tracking/deviations.md`).
///
/// The position is the draft's on both axes, not native's on the scrolling
/// axis alone: native's `nearest` on that axis does nothing at all (iOS
/// `scroll_view/LynxUIScroller.m:1033-1035`, `:1049-1051`; Android
/// `scroll/UIScrollView.java:771-773`, `:791-793`; Harmony
/// `lynx/platform/harmony/lynx_harmony/src/main/cpp/ui/ui_scroll.cc:166-168`,
/// `:182-184`), where the draft's scrolls the least distance that shows the
/// element, and the element's `scroll-margin` and the scroller's
/// `scroll-padding` apply. Recorded in `docs/tracking/deviations.md`. It
/// reads the last completed layout and never flushes, as every UI method
/// does here.
pub(super) fn scroll_into_view(
    document: &mut LynxDocument,
    element: NodeId,
    params: &str,
) -> Result<bool, InvalidParams> {
    let params = object(params)?;
    let Some(Value::Object(options)) = params.get("scrollIntoViewOptions") else {
        return Err(InvalidParams);
    };
    let behavior = if options.get("behavior").and_then(Value::as_str) == Some("smooth") {
        ScrollBehavior::Smooth
    } else {
        ScrollBehavior::Instant
    };
    let options = ScrollIntoViewOptions {
        behavior,
        block: logical_position(options, "block", ScrollLogicalPosition::Start),
        inline: logical_position(options, "inline", ScrollLogicalPosition::Nearest),
        container: ScrollIntoViewContainer::Nearest,
    };
    Ok(document.scroll_into_view(element, options))
}

/// The `ScrollLogicalPosition` keyword at `key`, `default` when missing, and
/// `Start` for any value that is not one of the four keywords.
fn logical_position(
    options: &Map<String, Value>,
    key: &str,
    default: ScrollLogicalPosition,
) -> ScrollLogicalPosition {
    match options.get(key) {
        None => default,
        Some(value) => match value.as_str() {
            Some("center") => ScrollLogicalPosition::Center,
            Some("end") => ScrollLogicalPosition::End,
            Some("nearest") => ScrollLogicalPosition::Nearest,
            _ => ScrollLogicalPosition::Start,
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Explicit pixel sizes lay out exactly.

    use dom::{MethodCall, MethodError, MethodOutcome, NodeId, Vector2D};

    use super::super::LynxDocument;
    use super::super::scroll_container::SCROLL_VIEW_TAG;
    use super::super::test_support::{child, document, element_under};
    use super::invoke_base_method;

    /// What `callElementMethod` runs: the element kind's method, then on
    /// `NotFound` the base set.
    fn call(
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        params: &str,
    ) -> MethodOutcome {
        match document.invoke_element_method(element, MethodCall { name, params }) {
            MethodOutcome::NotFound => {
                invoke_base_method(document, element, MethodCall { name, params })
            }
            outcome => outcome,
        }
    }

    /// A 100px square vertical `scroll-view` under the page holding ten 50px
    /// views, so view `n` sits at `50 n` and the range is 400.
    fn scroller() -> (LynxDocument, NodeId, Vec<NodeId>) {
        let mut document = document();
        let view = child(
            &mut document,
            SCROLL_VIEW_TAG,
            "width: 100px; height: 100px; flex-shrink: 0",
        );
        let items = (0..10)
            .map(|_| {
                element_under(
                    &mut document,
                    view,
                    "view",
                    "width: 100px; height: 50px; flex-shrink: 0",
                )
            })
            .collect();
        document.layout();
        (document, view, items)
    }

    fn options(fields: &str) -> String {
        format!(r#"{{"scrollIntoViewOptions":{{{fields}}}}}"#)
    }

    #[test]
    fn a_view_scrolls_to_block_start_by_default() {
        let (mut document, view, items) = scroller();
        assert_eq!(
            call(&mut document, items[4], "scrollIntoView", &options("")),
            MethodOutcome::Done
        );
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 200.0));
        assert!(document.pending_scroll_request(view).is_some());
    }

    #[test]
    fn block_end_and_center_place_the_view() {
        for (block, expected) in [
            ("end", 150.0),
            ("center", 175.0),
            ("start", 200.0),
            ("nearest", 150.0),
            ("bogus", 200.0),
        ] {
            let (mut document, view, items) = scroller();
            let params = options(&format!(r#""block":"{block}""#));
            assert_eq!(
                call(&mut document, items[4], "scrollIntoView", &params),
                MethodOutcome::Done,
                "{block}"
            );
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, expected),
                "{block}"
            );
        }
    }

    #[test]
    fn smooth_records_a_request_and_leaves_the_document_offset() {
        for behavior in ["auto", "instant", "none"] {
            let (mut document, view, items) = scroller();
            let params = options(&format!(r#""behavior":"{behavior}""#));
            call(&mut document, items[4], "scrollIntoView", &params);
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, 200.0),
                "{behavior} is instant"
            );
        }
        let (mut document, view, items) = scroller();
        let params = options(r#""behavior":"smooth""#);
        assert_eq!(
            call(&mut document, items[4], "scrollIntoView", &params),
            MethodOutcome::Done
        );
        assert_eq!(document.scroll_offset(view), Vector2D::zero());
        assert!(document.pending_scroll_request(view).is_some());
        let frame = document.commit();
        let index = frame.slot_of(view).expect("the scroller has a slot");
        let request = frame.scroll_slots()[index as usize]
            .request
            .expect("the frame carries the request");
        assert_eq!(request.target, Vector2D::new(0.0, 200.0));
        assert_eq!(request.behavior, dom::scroll::ScrollBehavior::Smooth);
    }

    /// An outer vertical `scroll-view` holding a 300px view, an inner one
    /// and a 300px view; the inner holds a 300px view, the target and a
    /// 300px view. The inner sits at 300 in the outer, the target at 300 in
    /// the inner.
    fn nested() -> (LynxDocument, NodeId, NodeId, NodeId) {
        let mut document = document();
        let square = "width: 100px; height: 100px; flex-shrink: 0";
        let spacer = "width: 100px; height: 300px; flex-shrink: 0";
        let outer = child(&mut document, SCROLL_VIEW_TAG, square);
        element_under(&mut document, outer, "view", spacer);
        let inner = element_under(&mut document, outer, SCROLL_VIEW_TAG, square);
        element_under(&mut document, outer, "view", spacer);
        element_under(&mut document, inner, "view", spacer);
        let target = element_under(
            &mut document,
            inner,
            "view",
            "width: 100px; height: 50px; flex-shrink: 0",
        );
        element_under(&mut document, inner, "view", spacer);
        document.layout();
        (document, outer, inner, target)
    }

    #[test]
    fn only_the_nearest_of_two_nested_scroll_views_moves() {
        let (mut document, outer, inner, target) = nested();
        assert_eq!(
            call(&mut document, target, "scrollIntoView", &options("")),
            MethodOutcome::Done
        );
        assert_eq!(document.scroll_offset(inner), Vector2D::new(0.0, 300.0));
        assert_eq!(document.scroll_offset(outer), Vector2D::zero());
        assert_eq!(document.pending_scroll_request(outer), None);
    }

    #[test]
    fn a_scroll_view_is_scrolled_into_its_own_scroller() {
        let (mut document, outer, inner, _) = nested();
        assert_eq!(
            call(&mut document, inner, "scrollIntoView", &options("")),
            MethodOutcome::Done,
            "the method is on every element, a scroll-view included"
        );
        assert_eq!(document.scroll_offset(outer), Vector2D::new(0.0, 300.0));
        assert_eq!(document.scroll_offset(inner), Vector2D::zero());
    }

    #[test]
    fn missing_options_are_invalid_params() {
        for params in [
            "{}",
            r#"{"scrollIntoViewOptions":null}"#,
            r#"{"scrollIntoViewOptions":"start"}"#,
            "[]",
            "null",
            "not json",
        ] {
            let (mut document, view, items) = scroller();
            assert_eq!(
                call(&mut document, items[4], "scrollIntoView", params),
                MethodOutcome::Failed(MethodError::InvalidParams),
                "{params}"
            );
            assert_eq!(document.scroll_offset(view), Vector2D::zero(), "{params}");
            assert_eq!(document.pending_scroll_request(view), None, "{params}");
        }
    }

    #[test]
    fn no_scroller_above_is_an_operation_error() {
        let mut document = document();
        let view = child(&mut document, "view", "width: 10px; height: 10px");
        document.layout();
        assert_eq!(
            call(&mut document, view, "scrollIntoView", &options("")),
            MethodOutcome::Failed(MethodError::Operation)
        );
        let page = document.document_element().id();
        assert_eq!(
            call(&mut document, page, "scrollIntoView", &options("")),
            MethodOutcome::Failed(MethodError::Operation)
        );
    }

    #[test]
    fn the_other_base_names_are_not_found() {
        let (mut document, _, items) = scroller();
        for name in [
            "scrollTo",
            "scrollBy",
            "focus",
            "blur",
            "boundingClientRect",
        ] {
            assert_eq!(
                invoke_base_method(&mut document, items[0], MethodCall { name, params: "{}" }),
                MethodOutcome::NotFound,
                "{name}"
            );
        }
    }
}
