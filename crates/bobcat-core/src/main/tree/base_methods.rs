//! The UI methods every element has, whatever its kind: the shared base set
//! the runtime's `callElementMethod` falls back to when the element's own
//! kind answers [`MethodOutcome::NotFound`] (user ruling 2026-10-09: the
//! documented UI methods `boundingClientRect`, `scrollIntoView`,
//! `scrollTo`/`scrollBy` and `focus`/`blur`).
//!
//! Built here: `boundingClientRect` ([`bounding_client_rect`]),
//! `scrollIntoView` ([`scroll_into_view`]), and CSSOM-View's
//! `Element.scrollTo(options)` and `Element.scrollBy(options)`
//! ([`scroll_with_options`]). A kind's own method of the same name is asked
//! first and wins, as a subclass method shadows the base's: a
//! `scroll-view`'s `scrollTo` and `scrollBy` ([`super::scroll_container`])
//! and an `x-swiper`'s `scrollTo` ([`super::swiper`]) answer for those
//! elements, and the base `scrollTo`/`scrollBy` never see them. No kind
//! answers `boundingClientRect`, and none may: it is the same method on
//! every element, as web-core answers it ahead of the element
//! (`web-core/ts/client/mainthread/elementAPIs/createInvokeUIMethod.ts:16-28`).
//!
//! `focus`/`blur` are not built and stay [`MethodOutcome::NotFound`] (code
//! 3): `dom` has no focus model, no focused element and no `focus()` step
//! to run. `takeScreenshot`, which the Lynx docs list on every element, is
//! code 3 too: it needs a pixel readback from the painter, which does not
//! exist.

use dom::scroll::{
    ScrollBehavior, ScrollIntoViewContainer, ScrollIntoViewOptions, ScrollLogicalPosition,
};
use dom::{MethodCall, MethodError, MethodOutcome, NodeId, Vector2D};
use serde_json::{Map, Value};

use super::scroll_container::{clamp_to_range, object};
use super::{InvalidParams, LynxDocument};
use crate::main::record::write_record_field;

/// Runs the base method `call` names on `element`, or answers
/// [`MethodOutcome::NotFound`] for a name the base set does not build.
///
/// It runs after the kind's [`dom::CustomElement::invoke`] has answered
/// `NotFound`, outside the `[CEReactions]` scope
/// [`dom::Document::invoke_element_method`] opens for the kind's method,
/// and needs none: a scroll request and a geometry readback queue no custom
/// element reaction.
pub(crate) fn invoke_base_method(
    document: &mut LynxDocument,
    element: NodeId,
    call: MethodCall<'_>,
) -> MethodOutcome {
    match call.name {
        "boundingClientRect" => MethodOutcome::Data(bounding_client_rect(document, element)),
        "scrollIntoView" => match scroll_into_view(document, element, call.params) {
            Ok(true) => MethodOutcome::Done,
            Ok(false) => MethodOutcome::Failed(MethodError::Operation),
            Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
        },
        "scrollTo" | "scrollBy" => {
            let method = if call.name == "scrollTo" {
                ScrollMethod::To
            } else {
                ScrollMethod::By
            };
            match scroll_with_options(document, element, call.params, method) {
                Ok(()) => MethodOutcome::Done,
                Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
            }
        }
        _ => MethodOutcome::NotFound,
    }
}

/// `boundingClientRect()` on `element`: the data fields `left`, `top`,
/// `width`, `height`, in that order, of its border box in viewport CSS px
/// ([`dom::Document::bounding_client_rect`]), ancestor scroll offsets
/// applied and transforms ignored, as native's own conversion ignores them.
/// An element with no box answers zeros rather than nothing, which is what
/// both references report for one. `params` is unread: the options the
/// platforms take (`relativeTo`, `androidEnableTransformProps`,
/// `iOSEnableAnimationProps`) each name behaviour this engine does not have.
///
/// `element-papi.ts` derives `right` and `bottom` from these, and adds the
/// element's `id` and `dataset`, which the realm holds. It reads the last
/// completed layout and never flushes, as every UI method does here.
fn bounding_client_rect(document: &LynxDocument, element: NodeId) -> String {
    let rect = document
        .bounding_client_rect(element)
        .unwrap_or_else(dom::Rect::zero);
    let mut record = String::new();
    for value in [
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    ] {
        write_record_field(&mut record, &value.to_string());
    }
    record
}

/// Which of CSSOM-View's two `ScrollToOptions` methods runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollMethod {
    /// `scrollTo(options)`: a missing member keeps that axis where it is.
    To,
    /// `scrollBy(options)`: a missing member moves that axis by 0.
    By,
}

/// CSSOM-View's `Element.scrollTo(options)` or `Element.scrollBy(options)`
/// on `element`, with `params` as the JSON text the realm serialized, read
/// as a `ScrollToOptions` dictionary `{left?, top?, behavior?}`.
///
/// - `params` is converted to the dictionary first, before any step of the algorithm, as Web IDL
///   converts arguments before the method runs. `null` reads as the dictionary with no members, as
///   web-core's `element.scrollTo(null)` does, and so does an array, an object whose `left` and
///   `top` are undefined. A JSON number, string or boolean is code 4 and nothing moves: Web IDL
///   throws a `TypeError` converting it, which web-core's `__InvokeUIMethod` answers as 4
///   (`createInvokeUIMethod.ts:35-41`). Text that is no JSON, which the realm never sends, is code
///   4 too.
/// - `left` and `top` are CSS px, converted as Web IDL converts an `unrestricted double`
///   ([`unrestricted_double`]) and then normalised as CSSOM-View does: a non-finite value is 0. For
///   `scrollTo` a missing member keeps that axis's current offset; for `scrollBy` it is 0, and the
///   target is the current offset plus the two values.
/// - `behavior` is one of the `ScrollBehavior` enum's strings `"auto"`, `"instant"` and `"smooth"`,
///   or missing, which is `"auto"`. Any other present value, a non-string included, is code 4 and
///   nothing moves, where Web IDL's enum conversion throws a `TypeError` and web-core answers 4.
///   `"smooth"` is `Smooth`; `"auto"` is instant because this engine has no `scroll-behavior`
///   property for `auto` to read (`docs/tracking/deviations.md`).
/// - The target clamps to the scroll range per axis, and the scroll goes through
///   [`dom::Document::scroll_to_with`], so the painter is told and a smooth one is recorded for the
///   painter to animate. As [`dom::Document::scroll_into_view`] does, a target equal to the current
///   offset records nothing unless a request is pending, which this one then replaces (CSSOM-View's
///   abort of the ongoing smooth scroll).
/// - After the params convert, an element with no box ([`dom::Document::bounding_client_rect`] is
///   `None`) or that is no scroll container ([`dom::Document::scroll_box`] is `None`) does nothing,
///   which is CSSOM-View's "terminate these steps"; the call answers code 0, as web-core's does for
///   a plain box. Params that do not convert are code 4 on such an element too.
///
/// CSSOM-View's step for the root element, which scrolls the viewport
/// instead, needs nothing of its own: the page element is the scroll
/// container this engine scrolls the page by when it is one, and is
/// otherwise a box with nothing to scroll, which falls in the branch above.
/// It reads the last completed layout and never flushes, as every UI method
/// does here.
fn scroll_with_options(
    document: &mut LynxDocument,
    element: NodeId,
    params: &str,
    method: ScrollMethod,
) -> Result<(), InvalidParams> {
    let options = match serde_json::from_str(params) {
        Ok(Value::Object(options)) => options,
        Ok(Value::Null | Value::Array(_)) => Map::new(),
        _ => return Err(InvalidParams),
    };
    let behavior = match options.get("behavior") {
        None => ScrollBehavior::Instant,
        Some(value) => match value.as_str() {
            Some("auto" | "instant") => ScrollBehavior::Instant,
            Some("smooth") => ScrollBehavior::Smooth,
            _ => return Err(InvalidParams),
        },
    };
    if document.bounding_client_rect(element).is_none() {
        return Ok(());
    }
    let Some(scroll_box) = document.scroll_box(element) else {
        return Ok(());
    };
    let current = scroll_box.offset;
    let axis = |key: &str, current: f32| {
        let value = options.get(key).map(unrestricted_double);
        match method {
            ScrollMethod::To => value.unwrap_or_else(|| f64::from(current)),
            ScrollMethod::By => f64::from(current) + value.unwrap_or(0.0),
        }
    };
    let max = scroll_box.max_offset();
    let target = Vector2D::new(
        clamp_to_range(axis("left", current.x), max.x),
        clamp_to_range(axis("top", current.y), max.y),
    );
    if target != current || document.pending_scroll_request(element).is_some() {
        document.scroll_to_with(element, target, behavior);
    }
    Ok(())
}

/// A JSON value as Web IDL converts it to an `unrestricted double` (ECMAScript
/// `ToNumber`), then normalised as CSSOM-View normalises a scroll position:
/// a non-finite result is 0.
///
/// A number is itself, `true` is 1, `false` and `null` are 0, and a string
/// is its trimmed text read as a decimal number, the empty string being 0.
/// ECMAScript's hexadecimal, octal and binary string forms read as 0 here,
/// and an array or object reads as 0 where `ToNumber` would read a
/// one-element array through its text.
fn unrestricted_double(value: &Value) -> f64 {
    let number = match value {
        Value::Number(number) => number.as_f64().unwrap_or(0.0),
        Value::Bool(value) => f64::from(u8::from(*value)),
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                0.0
            } else {
                text.parse().unwrap_or(f64::NAN)
            }
        }
        Value::Null | Value::Array(_) | Value::Object(_) => 0.0,
    };
    if number.is_finite() { number } else { 0.0 }
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

    use dom::scroll::{ScrollBehavior, ScrollRequest};
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
        assert_eq!(request.behavior, ScrollBehavior::Smooth);
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
    fn focus_and_blur_are_not_found() {
        let (mut document, _, items) = scroller();
        for name in ["focus", "blur", "takeScreenshot"] {
            assert_eq!(
                invoke_base_method(&mut document, items[0], MethodCall { name, params: "{}" }),
                MethodOutcome::NotFound,
                "{name}"
            );
        }
    }

    /// A 100px square `view` with `overflow: scroll` under the page, holding
    /// a 300px tall and 250px wide child, so its range is (150, 200).
    fn overflowing() -> (LynxDocument, NodeId) {
        let mut document = document();
        let view = child(
            &mut document,
            "view",
            "width: 100px; height: 100px; overflow: scroll; flex-shrink: 0",
        );
        element_under(
            &mut document,
            view,
            "view",
            "width: 250px; height: 300px; flex-shrink: 0",
        );
        document.layout();
        (document, view)
    }

    /// The request the next committed frame carries for `view`, if any.
    fn carried(document: &mut LynxDocument, view: NodeId) -> Option<ScrollRequest> {
        let frame = document.commit();
        let index = frame.slot_of(view)?;
        frame.scroll_slots()[index as usize].request
    }

    #[test]
    fn the_layout_these_tests_assume() {
        let (document, view) = overflowing();
        let scroll_box = document.scroll_box(view).expect("a scroll container");
        assert_eq!(scroll_box.max_offset(), Vector2D::new(150.0, 200.0));
    }

    #[test]
    fn scroll_to_moves_to_the_given_offset_at_once() {
        let (mut document, view) = overflowing();
        assert_eq!(
            call(&mut document, view, "scrollTo", r#"{"top":50}"#),
            MethodOutcome::Done
        );
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 50.0));
        let request = carried(&mut document, view).expect("the frame carries the request");
        assert_eq!(request.target, Vector2D::new(0.0, 50.0));
        assert_eq!(request.behavior, ScrollBehavior::Instant);
    }

    #[test]
    fn scroll_to_keeps_an_axis_without_a_member() {
        let (mut document, view) = overflowing();
        call(&mut document, view, "scrollTo", r#"{"left":30,"top":40}"#);
        call(&mut document, view, "scrollTo", r#"{"top":10}"#);
        assert_eq!(document.scroll_offset(view), Vector2D::new(30.0, 10.0));
    }

    #[test]
    fn smooth_records_a_request_and_leaves_the_offset() {
        let (mut document, view) = overflowing();
        assert_eq!(
            call(
                &mut document,
                view,
                "scrollTo",
                r#"{"left":10,"top":20,"behavior":"smooth"}"#
            ),
            MethodOutcome::Done
        );
        assert_eq!(document.scroll_offset(view), Vector2D::zero());
        let request = carried(&mut document, view).expect("the frame carries the request");
        assert_eq!(request.target, Vector2D::new(10.0, 20.0));
        assert_eq!(request.behavior, ScrollBehavior::Smooth);
    }

    #[test]
    fn auto_and_instant_behaviors_are_instant() {
        for behavior in ["auto", "instant"] {
            let (mut document, view) = overflowing();
            let params = format!(r#"{{"top":20,"behavior":"{behavior}"}}"#);
            call(&mut document, view, "scrollTo", &params);
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, 20.0),
                "{behavior}"
            );
            let request = carried(&mut document, view).expect("the frame carries the request");
            assert_eq!(request.behavior, ScrollBehavior::Instant, "{behavior}");
        }
    }

    #[test]
    fn an_unknown_behavior_is_invalid_params_and_moves_nothing() {
        for behavior in [r#""bogus""#, r#""Smooth""#, "null", "1", "true", "{}"] {
            let params = format!(r#"{{"top":20,"behavior":{behavior}}}"#);
            let (mut document, view) = overflowing();
            for name in ["scrollTo", "scrollBy"] {
                assert_eq!(
                    call(&mut document, view, name, &params),
                    MethodOutcome::Failed(MethodError::InvalidParams),
                    "{name} {behavior}"
                );
            }
            assert_eq!(document.scroll_offset(view), Vector2D::zero(), "{behavior}");
            assert_eq!(document.pending_scroll_request(view), None, "{behavior}");
        }
    }

    #[test]
    fn params_that_are_no_dictionary_are_invalid_params() {
        for params in ["5", r#""top""#, "true", "not json"] {
            let (mut document, view) = overflowing();
            for name in ["scrollTo", "scrollBy"] {
                assert_eq!(
                    call(&mut document, view, name, params),
                    MethodOutcome::Failed(MethodError::InvalidParams),
                    "{name} {params}"
                );
            }
            assert_eq!(document.scroll_offset(view), Vector2D::zero(), "{params}");
            assert_eq!(document.pending_scroll_request(view), None, "{params}");
        }
    }

    #[test]
    fn scroll_to_clamps_to_the_range() {
        let (mut document, view) = overflowing();
        call(&mut document, view, "scrollTo", r#"{"left":-5,"top":1e9}"#);
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 200.0));
        call(
            &mut document,
            view,
            "scrollTo",
            r#"{"left":1e308,"top":-1e308}"#,
        );
        assert_eq!(document.scroll_offset(view), Vector2D::new(150.0, 0.0));
    }

    #[test]
    fn params_without_members_move_nothing() {
        for params in ["{}", "null", "[]"] {
            let (mut document, view) = overflowing();
            for name in ["scrollTo", "scrollBy"] {
                assert_eq!(
                    call(&mut document, view, name, params),
                    MethodOutcome::Done,
                    "{name} {params}"
                );
            }
            assert_eq!(document.scroll_offset(view), Vector2D::zero(), "{params}");
            assert_eq!(document.pending_scroll_request(view), None, "{params}");
        }
    }

    #[test]
    fn a_value_that_is_no_number_reads_as_webidl_converts_it() {
        for (top, expected) in [
            (r#""abc""#, 0.0),
            ("null", 0.0),
            (r#""""#, 0.0),
            (r#"" 60 ""#, 60.0),
            (r#""Infinity""#, 0.0),
            (r#""1e1""#, 10.0),
            ("true", 1.0),
            ("false", 0.0),
            ("[70]", 0.0),
            ("{}", 0.0),
        ] {
            let (mut document, view) = overflowing();
            call(&mut document, view, "scrollTo", r#"{"top":50}"#);
            let params = format!(r#"{{"top":{top}}}"#);
            assert_eq!(
                call(&mut document, view, "scrollTo", &params),
                MethodOutcome::Done,
                "{top}"
            );
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, expected),
                "{top}"
            );
        }
    }

    #[test]
    fn scroll_by_adds_to_the_current_offset() {
        let (mut document, view) = overflowing();
        for _ in 0..2 {
            assert_eq!(
                call(&mut document, view, "scrollBy", r#"{"top":30}"#),
                MethodOutcome::Done
            );
        }
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 60.0));
        call(&mut document, view, "scrollBy", r#"{"left":20,"top":-100}"#);
        assert_eq!(document.scroll_offset(view), Vector2D::new(20.0, 0.0));
        call(&mut document, view, "scrollBy", r#"{"left":1000}"#);
        assert_eq!(document.scroll_offset(view), Vector2D::new(150.0, 0.0));
        call(&mut document, view, "scrollBy", "{}");
        assert_eq!(document.scroll_offset(view), Vector2D::new(150.0, 0.0));
    }

    #[test]
    fn a_box_that_cannot_scroll_does_nothing() {
        let mut document = document();
        let plain = child(&mut document, "view", "width: 100px; height: 100px");
        element_under(&mut document, plain, "view", "width: 100px; height: 300px");
        let hidden = child(
            &mut document,
            "view",
            "display: none; width: 100px; height: 100px; overflow: scroll",
        );
        element_under(&mut document, hidden, "view", "width: 100px; height: 300px");
        document.layout();
        for view in [plain, hidden] {
            for name in ["scrollTo", "scrollBy"] {
                assert_eq!(
                    call(
                        &mut document,
                        view,
                        name,
                        r#"{"top":50,"behavior":"smooth"}"#
                    ),
                    MethodOutcome::Done,
                    "{name}"
                );
                assert_eq!(document.scroll_offset(view), Vector2D::zero(), "{name}");
                assert_eq!(document.pending_scroll_request(view), None, "{name}");
                assert_eq!(
                    call(&mut document, view, name, r#"{"behavior":"bogus"}"#),
                    MethodOutcome::Failed(MethodError::InvalidParams),
                    "the params convert before the box is looked at: {name}"
                );
                assert_eq!(
                    call(&mut document, view, name, "5"),
                    MethodOutcome::Failed(MethodError::InvalidParams),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn a_scroll_view_keeps_its_own_scroll_to() {
        let (mut document, view, _) = scroller();
        assert_eq!(
            call(&mut document, view, "scrollTo", r#"{"index":2}"#),
            MethodOutcome::Done
        );
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 100.0));
        assert_eq!(
            call(&mut document, view, "scrollTo", r#"{"top":50}"#),
            MethodOutcome::Done,
            "the kind's scrollTo reads no `top`: its missing `offset` is 0"
        );
        assert_eq!(document.scroll_offset(view), Vector2D::zero());
        assert_eq!(
            call(&mut document, view, "scrollBy", r#"{"top":50}"#),
            MethodOutcome::Failed(MethodError::InvalidParams),
            "the kind's scrollBy wants a numeric `offset`"
        );
    }

    #[test]
    fn bounding_client_rect_answers_the_four_fields() {
        let mut document = document();
        let view = child(
            &mut document,
            "view",
            "width: 40px; height: 30px; margin-left: 5px; margin-top: 7px",
        );
        let hidden = child(&mut document, "view", "display: none");
        document.layout();
        assert_eq!(
            call(&mut document, view, "boundingClientRect", ""),
            MethodOutcome::Data("1:51:72:402:30".to_owned())
        );
        assert_eq!(
            call(&mut document, hidden, "boundingClientRect", ""),
            MethodOutcome::Data("1:01:01:01:0".to_owned()),
            "an element with no box answers zeros"
        );
    }

    #[test]
    fn a_kind_with_methods_leaves_bounding_client_rect_to_the_base_set() {
        let (mut document, view, _) = scroller();
        let call_rect = MethodCall {
            name: "boundingClientRect",
            params: "",
        };
        assert_eq!(
            document.invoke_element_method(view, call_rect),
            MethodOutcome::NotFound
        );
        assert_eq!(
            call(&mut document, view, "boundingClientRect", ""),
            MethodOutcome::Data("1:01:03:1003:100".to_owned())
        );
    }
}
