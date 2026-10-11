//! The `scroll-view` tag: a main-thread scroll container, and the component
//! that answers three of its UI methods.
//!
//! The UA sheet carries the container itself: which axis scrolls, which one
//! clips, and which way the subtree stacks ([`UA_RULES`]). Its container
//! defaults (border box, the configured display mode) ride the same rule as
//! `view`'s, in [`super::ua_sheet`]. The component ([`ScrollView`]) adds
//! three of the methods Lynx documents for the tag
//! (lynxjs.org/api/elements/built-in/scroll-view), which the runtime's
//! `callElementMethod` reaches through
//! [`dom::Document::invoke_element_method`]: `scrollTo` ([`scroll_to`]),
//! `scrollBy` ([`scroll_by`]) and `getScrollInfo` ([`scroll_info`]). It
//! reacts to no attribute and adds no event; the threshold events and the
//! rest of what neither the sheet nor this module covers are recorded in
//! `docs/tracking/deviations.md`.
//!
//! `list` is the other Lynx scroller and carries the same axis rules, written
//! against its own attribute spellings. They live with the rest of that tag's
//! policy in [`super::list`], one module per tag.
//!
//! # The references
//!
//! - iOS `lynx/platform/darwin/ios/lynx/ui/scroll_view/LynxUIScroller.m`: `scrollBy`, `scrollTo`,
//!   `getScrollInfo`, `takeContentScreenshot` and `autoScroll` at `:1219-1360`,
//!   `clampScrollToPosition` at `:1078-1118`.
//! - Android `lynx/platform/android/lynx_android/src/main/java/com/lynx/tasm/behavior/ui/scroll/`
//!   `UIScrollView.java`: `autoScroll` at `:746-749`, `getScrollInfo` at `:922-931`, `scrollBy` and
//!   `scrollTo` at `:960-1050`.
//! - Harmony `lynx/platform/harmony/lynx_harmony/src/main/cpp/ui/ui_scroll.cc`: the method switch
//!   at `:64-155`, `InvokeScrollTo` and `InvokeGetScrollInfo` at `:750-799`.
//! - web-core `lynx-stack/packages/web-platform/web-elements/src/elements/ScrollView/`
//!   `ScrollView.ts`: the `scrollTo` overload (`:32-83`) and `autoScroll` (`:84-112`). It has no
//!   `scrollBy` of its own, so `HTMLElement.scrollBy` answers one, and no `getScrollInfo`, so that
//!   name is code 3 there
//!   (`web-core/ts/client/mainthread/elementAPIs/createInvokeUIMethod.ts:29-34`).
//!
//! # The scrolling axis
//!
//! Every method works on the axis the UA rules give the tag: x when the
//! element has the `scroll-x` attribute or `scroll-orientation="horizontal"`,
//! else y ([`is_horizontal`]), which is iOS's `_enableScrollY` and Harmony's
//! `IsHorizontal()`. A method moves that axis alone and keeps the other
//! axis's offset; native writes 0 there, which is the same offset on a
//! scroller whose cross axis never moves. web-core writes the one value to
//! both axes and lets the browser clamp the cross axis. Right-to-left is not
//! handled: native mirrors an indexed x target (iOS `:1282-1284`, Android
//! `:1028-1032`), web-core does not.
//!
//! Each method reads the last completed layout and never flushes, as every
//! UI method does here.
//!
//! # Not built
//!
//! `autoScroll({rate, start})` and `takeContentScreenshot` stay unanswered
//! (code 3). `autoScroll` moves the scroller at a constant rate, frame after
//! frame, until it reaches the boundary or a second call stops it (iOS's
//! `CADisplayLink` at `:1345-1360`, web-core's 100 ms `scrollBy` interval at
//! `:84-112`). Nothing here can carry that: a script-facing scroll is one
//! [`dom::scroll::ScrollRequest`], whose behaviors are `Instant` and
//! `Smooth` to one target, and a component has no per-frame hook to issue a
//! new request each frame. It needs a request kind the painter drives at a
//! rate, with a way to end it. `takeContentScreenshot` needs a pixel
//! readback of the scroller's whole content from the painter, which does not
//! exist.

use dom::scroll::{ScrollBehavior, ScrollBox};
use dom::{CustomElement, MethodCall, MethodError, MethodOutcome, NodeId, Vector2D};
use serde_json::{Map, Value};

use super::{LynxDocument, element_child, is_truthy};
use crate::main::record::write_record_field;

/// The tag native registers and web-core defines (`@Component('scroll-view',
/// …)`).
pub(super) const SCROLL_VIEW_TAG: &str = "scroll-view";

/// Where a scroller scrolls, from `web-elements`' `scroll-view.css`.
///
/// A Lynx scroller scrolls one axis and clips the other, and stacks its
/// children along the axis it scrolls — which takes two declarations, because
/// the axis lives in a different property per display mode: `flex-direction`
/// for `display: flex`, `linear-direction` for `display: linear`. That is the
/// pair web-elements drives through one `--lynx-linear-orientation` custom
/// property. Vertical is the default in both worlds, and
/// `enable-scroll="false"` leaves the box a scroll container that only script
/// can move (`overflow: hidden`) rather than one the user can drag.
///
/// The authored `clip` never survives computation here: css-overflow-3 turns a
/// `clip` axis into `hidden` when the other axis scrolls, so a scroller's cross
/// axis is a scroll container that only script can move rather than a plain
/// clip. That is what a browser makes of the same declaration in
/// `scroll-view.css`, so it is parity rather than a shortcut.
pub(super) const UA_RULES: &str = r#"
scroll-view, scroll-view[scroll-y], scroll-view[scroll-orientation="vertical"] {
  overflow-x: clip; overflow-y: scroll;
  flex-direction: column; linear-direction: column;
}
scroll-view[scroll-x], scroll-view[scroll-orientation="horizontal"] {
  overflow-x: scroll; overflow-y: clip;
  flex-direction: row; linear-direction: row;
}
scroll-view[scroll-y][enable-scroll="false"],
scroll-view[scroll-orientation="vertical"][enable-scroll="false"] { overflow-y: hidden; }
scroll-view[scroll-x][enable-scroll="false"],
scroll-view[scroll-orientation="horizontal"][enable-scroll="false"] { overflow-x: hidden; }
"#;

/// Installs the `scroll-view` component. Must run before any element could
/// carry the tag, which is [`Document::define`](dom::Document::define)'s own
/// precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(SCROLL_VIEW_TAG, Box::new(ScrollView));
}

/// The `scroll-view` component: `scrollTo`, `scrollBy` and
/// `getScrollInfo`.
struct ScrollView;

impl CustomElement<()> for ScrollView {
    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        let answer = match call.name {
            "scrollTo" => scroll_to(document, element, call.params).map(|()| None),
            "scrollBy" => scroll_by(document, element, call.params).map(|by| Some(by.record())),
            "getScrollInfo" => Ok(Some(scroll_info(document, element).record())),
            _ => return MethodOutcome::NotFound,
        };
        match answer {
            Ok(None) => MethodOutcome::Done,
            Ok(Some(fields)) => MethodOutcome::Data(fields),
            Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
        }
    }
}

/// A method's params do not have the shape it reads: the UI-method status
/// table's code 4, `PARAM_INVALID`. Nothing moved.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct InvalidParams;

/// Whether `view` scrolls x rather than y: the attributes [`UA_RULES`] select
/// the horizontal rule by. `scroll-x` wins over `scroll-y` when both are
/// present, as the later rule of equal specificity does.
fn is_horizontal(document: &LynxDocument, view: NodeId) -> bool {
    document.get(view).is_some_and(|node| {
        node.attribute("scroll-x").is_some()
            || node.attribute("scroll-orientation") == Some("horizontal")
    })
}

/// The scrolling axis's component of `vector`.
fn along(vector: Vector2D<f32>, horizontal: bool) -> f32 {
    if horizontal { vector.x } else { vector.y }
}

/// `vector` with its scrolling-axis component replaced by `value`.
fn with_axis(vector: Vector2D<f32>, horizontal: bool, value: f32) -> Vector2D<f32> {
    if horizontal {
        Vector2D::new(value, vector.y)
    } else {
        Vector2D::new(vector.x, value)
    }
}

/// `value` clamped to `0..=max`, in f64 first: a number as large as JSON
/// allows would be infinite in f32, and the range is all the document keeps.
#[expect(
    clippy::cast_possible_truncation,
    reason = "clamped to the scroll range, which is an f32"
)]
fn clamp_to_range(value: f64, max: f32) -> f32 {
    value.clamp(0.0, f64::from(max)) as f32
}

/// `scrollTo({index?, offset = 0, smooth = false})` on `view`, with `params`
/// as the JSON text the realm serialized.
///
/// - `params` must be a JSON object, else code 4 (Harmony `:68-71`).
/// - `offset` is a JSON number in CSS px, or a string in `px`, `rpx` or `ppx`, or with no unit
///   ([`offset_px`]); missing is 0, anything else is code 4. iOS reads a string through
///   `toPtFromIDUnitValue` (`:1263-1264`), which also knows `rem`, `em`, `vw` and `vh` and reads
///   anything it cannot parse as 0; Android and Harmony read a number only (`:1001`, `:92-93`);
///   web-core `parseFloat`s a string and ignores its unit (`ScrollView.ts:48-53`).
/// - Without `index`, the target is `offset` as an absolute position, in all four references.
/// - `index`, when present, must be a JSON number, truncated toward zero as `integerValue`,
///   `getInt` and `static_cast<int>` truncate it; anything else is code 4, where Harmony ignores a
///   non-numeric index (`:89-91`). An index below 0, or not below the number of element children,
///   is code 4 and moves nothing, as all three natives answer (iOS `:1265-1272`, Android
///   `:1005-1011`, Harmony `:81-88`); web-core ignores it (`:55-73`). In range, the target is that
///   child's position on the axis in the scroller's scroll coordinates plus `offset`, the child's
///   layout location less the scroller's border, and 0 for a child with no box, which is web-core's
///   `offsetTop` of a `display: none` child. web-core's index 0 scrolls to `offset` alone
///   (`:57-59`) rather than to the first child's position plus it.
/// - With no element children the natives answer code 4 whatever the params (iOS `:1258-1261`,
///   Android `:995-999`, Harmony `:752-756`); here that is so only when an `index` is given, where
///   it is out of range anyway, and an offset-only call scrolls, as web-core's does.
/// - The target clamps to the scroll range and the call succeeds, which is iOS (`:1096-1103`) and
///   web-core. Android scrolls to the clamped target and answers code 4 (`:1019-1023`,
///   `:1048-1050`); Harmony answers code 4 and does not scroll, its upper bound being the content
///   size rather than the range (`:759`, `:773-777`).
/// - `smooth` is read with JavaScript truthiness, as web-core's `smooth ? 'smooth' : 'auto'` reads
///   it, and a missing one is `false` (Android `getBoolean("smooth", false)`, Harmony's
///   `smooth{false}`, which both read a boolean only). A smooth scroll is animated by the painter
///   and leaves the document's offset where it is until the painter posts it back; an instant one
///   moves the document at once ([`dom::Document::scroll_to_with`]). Either way the call answers at
///   once, as web-core's does; native answers a smooth scroll when its animation ends.
///
/// A `display: none` `scroll-view` has a zero scrollport, so it scrolls to
/// offset 0 in a request no frame carries, and one restyled into no scroll
/// container records nothing; either way the call succeeds, as `selectTab`
/// does.
pub(super) fn scroll_to(
    document: &mut LynxDocument,
    view: NodeId,
    params: &str,
) -> Result<(), InvalidParams> {
    let params = object(params)?;
    let offset = match params.get("offset") {
        None => 0.0,
        Some(offset) => offset_px(document, offset).ok_or(InvalidParams)?,
    };
    let index = match params.get("index") {
        None => None,
        Some(index) => Some(index.as_f64().ok_or(InvalidParams)?.trunc()),
    };
    let behavior = if params.get("smooth").is_some_and(is_truthy) {
        ScrollBehavior::Smooth
    } else {
        ScrollBehavior::Instant
    };
    let horizontal = is_horizontal(document, view);
    let start = match index {
        None => 0.0,
        Some(index) => f64::from(child_position(document, view, index, horizontal)?),
    };
    let Some(scroll_box) = document.scroll_box(view) else {
        return Ok(());
    };
    let max = along(scroll_box.max_offset(), horizontal);
    let target = clamp_to_range(start + offset, max);
    document.scroll_to_with(
        view,
        with_axis(scroll_box.offset, horizontal, target),
        behavior,
    );
    Ok(())
}

/// `params` as a JSON object, or code 4. Shared with the base set's
/// `scrollIntoView` ([`super::base_methods`]).
pub(super) fn object(params: &str) -> Result<Map<String, Value>, InvalidParams> {
    match serde_json::from_str(params) {
        Ok(Value::Object(params)) => Ok(params),
        _ => Err(InvalidParams),
    }
}

/// The position on the scrolling axis, in `view`'s scroll coordinates, of
/// its element child number `index` ([`element_child`]), or code 4 when
/// there is none.
fn child_position(
    document: &LynxDocument,
    view: NodeId,
    index: f64,
    horizontal: bool,
) -> Result<f32, InvalidParams> {
    let child = element_child(document, view, index).ok_or(InvalidParams)?;
    let (Some(layout), Some(container)) = (
        document.rounded_layout(child),
        document.rounded_layout(view),
    ) else {
        return Ok(0.0);
    };
    Ok(if horizontal {
        layout.location.x - container.border.left
    } else {
        layout.location.y - container.border.top
    })
}

/// A `scrollTo` offset in CSS px: a JSON number as given, or a string that
/// is a number followed by `px`, `rpx` (the viewport width over 750, the
/// unit [`super::blur_view`] also resolves), `ppx` (device pixels, divided
/// by the device pixel ratio) or nothing. Anything else, a non-finite number
/// included, is `None`.
fn offset_px(document: &LynxDocument, offset: &Value) -> Option<f64> {
    let text = match offset {
        Value::Number(number) => return number.as_f64(),
        Value::String(text) => text.as_str(),
        _ => return None,
    };
    let number = |digits: &str| digits.parse::<f64>().ok().filter(|value| value.is_finite());
    if let Some(digits) = text.strip_suffix("rpx") {
        Some(number(digits)? * f64::from(document.viewport_size().width) / 750.0)
    } else if let Some(digits) = text.strip_suffix("ppx") {
        Some(number(digits)? / f64::from(document.device_pixel_ratio()))
    } else if let Some(digits) = text.strip_suffix("px") {
        number(digits)
    } else {
        number(text)
    }
}

/// What `scrollBy` moved, in CSS px per axis: the
/// `{consumedX, consumedY, unconsumedX, unconsumedY}` all three natives
/// answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrolledBy {
    pub(super) consumed: Vector2D<f32>,
    pub(super) unconsumed: Vector2D<f32>,
}

impl ScrolledBy {
    /// The data fields, in the order `element-papi.ts` reads them:
    /// `consumedX`, `consumedY`, `unconsumedX`, `unconsumedY`.
    fn record(self) -> String {
        let mut record = String::new();
        for value in [
            self.consumed.x,
            self.consumed.y,
            self.unconsumed.x,
            self.unconsumed.y,
        ] {
            write_record_field(&mut record, &value.to_string());
        }
        record
    }
}

/// `scrollBy({offset})` on `view`: scrolls the axis by `offset` CSS px at
/// once, clamped to the scroll range, and answers what moved.
///
/// `params` must be a JSON object with a numeric `offset`, else code 4 and
/// nothing moves, as Android (`:965-968`) and iOS (`:1224-1228`) refuse a
/// missing one; Harmony reads a missing or non-numeric one as 0
/// (`:139-142`), and web-core's `HTMLElement.scrollBy` reads no `offset` at
/// all and scrolls nowhere. The scroll is instant, as every native's is,
/// through [`dom::Document::scroll_to_with`], so the painter is told.
///
/// The natives hand the one `offset` to both axes and scroll the scrolling
/// one (Android `UIScrollView.java:1394-1423`). The answer is theirs: the
/// scrolling axis consumes what it moved and leaves the rest unconsumed;
/// the cross axis consumes nothing and leaves the whole `offset`
/// unconsumed. The values are CSS px as `f32`; Android and iOS truncate them
/// to integers (`(int)` at `:976-979`, `int` at `:1236-1239`).
///
/// A `display: none` `scroll-view`, or one that is no scroll container,
/// moves nothing and consumes nothing.
pub(super) fn scroll_by(
    document: &mut LynxDocument,
    view: NodeId,
    params: &str,
) -> Result<ScrolledBy, InvalidParams> {
    let params = object(params)?;
    let delta = params
        .get("offset")
        .and_then(Value::as_f64)
        .ok_or(InvalidParams)?;
    let horizontal = is_horizontal(document, view);
    let (before, after) = match document.scroll_box(view) {
        None => (0.0, 0.0),
        Some(scroll_box) => {
            let before = along(scroll_box.offset, horizontal);
            let max = along(scroll_box.max_offset(), horizontal);
            let target = clamp_to_range(f64::from(before) + delta, max);
            let applied = document.scroll_to_with(
                view,
                with_axis(scroll_box.offset, horizontal, target),
                ScrollBehavior::Instant,
            );
            (before, along(applied, horizontal))
        }
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the delta as the f32 the answer is written in"
    )]
    let delta = delta as f32;
    let consumed = after - before;
    let axis = |value: f32, cross: f32| {
        if horizontal {
            Vector2D::new(value, cross)
        } else {
            Vector2D::new(cross, value)
        }
    };
    Ok(ScrolledBy {
        consumed: axis(consumed, 0.0),
        unconsumed: axis(delta - consumed, delta),
    })
}

/// What `getScrollInfo` reports, in CSS px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollInfo {
    pub(super) offset: Vector2D<f32>,
    /// The scrolling axis's maximum offset, written as both `scrollRange`
    /// and `maxScrollOffset`.
    pub(super) max_offset: f32,
}

impl ScrollInfo {
    /// The data fields, in the order `element-papi.ts` reads them:
    /// `scrollX`, `scrollY`, `scrollRange`, `maxScrollOffset`.
    fn record(self) -> String {
        let mut record = String::new();
        for value in [
            self.offset.x,
            self.offset.y,
            self.max_offset,
            self.max_offset,
        ] {
            write_record_field(&mut record, &value.to_string());
        }
        record
    }
}

/// `getScrollInfo()` on `view`: its offset, and the scrolling axis's maximum
/// offset as both `scrollRange` and `maxScrollOffset`. Android's
/// `scrollRange` is that maximum (`getScrollRange()`, `:929`, `:1294-1297`),
/// as is Harmony's (`scroll_range - GetViewPortSize()`, `:798`) and iOS's
/// `maxScrollOffset` (`:1323`). iOS's own `scrollRange` is the content
/// size, marked there as a legacy field kept to avoid a breaking change
/// (`:1324-1325`); that
/// is the one reading not followed. Android answers integers (`putInt`);
/// these are `f32`. web-core has no `getScrollInfo`.
///
/// A `display: none` `scroll-view`, or one that is no scroll container,
/// answers zeros.
pub(super) fn scroll_info(document: &LynxDocument, view: NodeId) -> ScrollInfo {
    let horizontal = is_horizontal(document, view);
    document.scroll_box(view).map_or(
        ScrollInfo {
            offset: Vector2D::zero(),
            max_offset: 0.0,
        },
        |scroll_box: ScrollBox| ScrollInfo {
            offset: scroll_box.offset,
            max_offset: along(scroll_box.max_offset(), horizontal),
        },
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Explicit pixel sizes lay out exactly.

    use dom::scroll::ScrollBehavior;
    use dom::stylo::computed_values::{flex_direction, linear_direction};
    use dom::stylo::values::computed::Overflow;
    use dom::{MethodCall, MethodError, MethodOutcome, NodeId, Vector2D};

    use super::super::LynxDocument;
    use super::super::test_support::{child, document, element_under, overflow, style_of};
    use super::{
        InvalidParams, SCROLL_VIEW_TAG, ScrollInfo, ScrolledBy, scroll_by, scroll_info, scroll_to,
    };

    #[test]
    fn a_scroller_scrolls_one_axis_and_clips_the_other() {
        let mut document = document();
        let vertical = [
            child(&mut document, "scroll-view", ""),
            child(&mut document, "scroll-view", ""),
        ];
        let horizontal = [
            child(&mut document, "scroll-view", ""),
            child(&mut document, "scroll-view", ""),
        ];
        document.set_attribute(vertical[1], "scroll-orientation", "vertical");
        document.set_attribute(horizontal[0], "scroll-x", "");
        document.set_attribute(horizontal[1], "scroll-orientation", "horizontal");
        document.layout();

        for scroller in vertical {
            let style = style_of(&document, scroller);
            assert_eq!(
                (*style.get_overflow_x(), *style.get_overflow_y()),
                (Overflow::Hidden, Overflow::Scroll),
                "the clipped axis computes to `hidden` beside a scrolling one"
            );
            assert_eq!(*style.get_flex_direction(), flex_direction::T::Column);
            assert_eq!(*style.get_linear_direction(), linear_direction::T::Column);
        }
        for scroller in horizontal {
            let style = style_of(&document, scroller);
            assert_eq!(
                (*style.get_overflow_x(), *style.get_overflow_y()),
                (Overflow::Scroll, Overflow::Hidden)
            );
            assert_eq!(*style.get_flex_direction(), flex_direction::T::Row);
            assert_eq!(*style.get_linear_direction(), linear_direction::T::Row);
        }
    }

    #[test]
    fn enable_scroll_false_leaves_a_scroller_only_script_can_move() {
        let mut document = document();
        let vertical = child(&mut document, "scroll-view", "");
        let horizontal = child(&mut document, "scroll-view", "");
        document.set_attribute(vertical, "scroll-y", "");
        document.set_attribute(vertical, "enable-scroll", "false");
        document.set_attribute(horizontal, "scroll-x", "");
        document.set_attribute(horizontal, "enable-scroll", "false");
        document.layout();

        for scroller in [vertical, horizontal] {
            assert_eq!(
                overflow(&document, scroller),
                (Overflow::Hidden, Overflow::Hidden),
                "neither axis is `scroll` any more, so no drag reaches either"
            );
        }
    }

    /// The layout half of the UA gap: before these rules existed the tag fell
    /// to the bare Lynx initial values, so the subtree stacked on the wrong axis
    /// inside a content box.
    #[test]
    fn a_scroller_lays_its_subtree_out_along_the_axis_it_scrolls() {
        for (tag, attribute, horizontal) in [
            ("scroll-view", None, false),
            ("scroll-view", Some(("scroll-y", "")), false),
            ("scroll-view", Some(("scroll-x", "")), true),
            (
                "scroll-view",
                Some(("scroll-orientation", "horizontal")),
                true,
            ),
        ] {
            let mut document = document();
            let scroller = child(
                &mut document,
                tag,
                "width: 100px; height: 100px; border: 10px solid",
            );
            if let Some((name, value)) = attribute {
                document.set_attribute(scroller, name, value);
            }
            let items: Vec<_> = (0..2)
                .map(|_| {
                    element_under(&mut document, scroller, "view", "width: 30px; height: 30px")
                })
                .collect();
            document.layout();

            let border_box = document
                .rounded_layout(scroller)
                .expect("the scroller is laid out");
            assert_eq!(
                (border_box.size.width, border_box.size.height),
                (100.0, 100.0),
                "border-box sizing puts the border inside the declared size: {tag}"
            );

            let offsets: Vec<_> = items
                .iter()
                .map(|item| {
                    let layout = document
                        .rounded_layout(*item)
                        .expect("a scroller lays its subtree out");
                    assert_eq!(
                        (layout.size.width, layout.size.height),
                        (30.0, 30.0),
                        "{tag}"
                    );
                    (layout.location.x, layout.location.y)
                })
                .collect();
            let stacked = if horizontal {
                [(10.0, 10.0), (40.0, 10.0)]
            } else {
                [(10.0, 10.0), (10.0, 40.0)]
            };
            assert_eq!(offsets, stacked, "{tag} horizontal={horizontal}");
        }
    }

    // --- the component ------------------------------------------------------

    /// A laid-out scroller of `count` element children under the page:
    /// 100px square with a 10px border, so an 80px scrollport, each child
    /// 40px along the scrolling axis, so the children sit at 0, 40, 80, … in
    /// the scroller's scroll coordinates, and five of them give a range of
    /// 120. `attribute` is written before the layout.
    fn scroller(attribute: Option<(&str, &str)>, count: usize) -> (LynxDocument, NodeId) {
        let mut document = document();
        let view = child(
            &mut document,
            SCROLL_VIEW_TAG,
            "width: 100px; height: 100px; border: 10px solid",
        );
        if let Some((name, value)) = attribute {
            document.set_attribute(view, name, value);
        }
        let horizontal = attribute.is_some();
        for _ in 0..count {
            // Across the axis each child overflows by 70px, so the cross axis
            // has a range of its own that no method may move.
            let style = if horizontal {
                "width: 40px; height: 150px; flex-shrink: 0"
            } else {
                "width: 150px; height: 40px; flex-shrink: 0"
            };
            element_under(&mut document, view, "view", style);
        }
        document.layout();
        (document, view)
    }

    fn vertical(count: usize) -> (LynxDocument, NodeId) {
        scroller(None, count)
    }

    /// The request the committed frame carries to the painter for `view`.
    fn carried(
        document: &mut LynxDocument,
        view: NodeId,
    ) -> Option<(Vector2D<f32>, ScrollBehavior)> {
        let frame = document.commit();
        let index = frame.slot_of(view).expect("the scroller has a slot");
        frame.scroll_slots()[index as usize]
            .request
            .map(|request| (request.target, request.behavior))
    }

    fn invoke(
        document: &mut LynxDocument,
        view: NodeId,
        name: &str,
        params: &str,
    ) -> MethodOutcome {
        document.invoke_element_method(view, MethodCall { name, params })
    }

    #[test]
    fn the_layout_these_tests_assume() {
        let (document, view) = vertical(5);
        let scroll_box = document.scroll_box(view).expect("a scroll container");
        assert_eq!(scroll_box.max_offset(), Vector2D::new(70.0, 120.0));
        let (document, view) = scroller(Some(("scroll-x", "")), 5);
        let scroll_box = document.scroll_box(view).expect("a scroll container");
        assert_eq!(scroll_box.max_offset(), Vector2D::new(120.0, 70.0));
    }

    #[test]
    fn a_scroll_view_is_a_defined_element() {
        let (document, view) = vertical(0);
        assert!(
            document
                .matches(view, ":defined")
                .expect("a valid selector")
        );
    }

    #[test]
    fn scroll_to_lands_on_the_indexed_child_plus_the_offset() {
        for (params, expected) in [
            (r#"{"index":0}"#, 0.0),
            (r#"{"index":2}"#, 80.0),
            (r#"{"index":1,"offset":30}"#, 70.0),
            (r#"{"index":1,"offset":"30px"}"#, 70.0),
            (r#"{"index":2,"offset":-30}"#, 50.0),
            // An index is truncated toward zero, as every native truncates it.
            (r#"{"index":2.9}"#, 80.0),
            (r#"{"index":-0.5,"offset":5}"#, 5.0),
        ] {
            let (mut document, view) = vertical(5);
            assert_eq!(scroll_to(&mut document, view, params), Ok(()), "{params}");
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, expected),
                "{params}"
            );
        }
    }

    #[test]
    fn scroll_to_without_an_index_scrolls_to_the_offset() {
        for (params, expected) in [
            ("{}", 0.0),
            (r#"{"offset":55}"#, 55.0),
            (r#"{"offset":"55"}"#, 55.0),
            (r#"{"offset":"55px"}"#, 55.0),
            (r#"{"offset":12.5,"smooth":false}"#, 12.5),
        ] {
            let (mut document, view) = vertical(5);
            document.scroll_to(view, Vector2D::new(0.0, 100.0));
            assert_eq!(scroll_to(&mut document, view, params), Ok(()), "{params}");
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, expected),
                "an absolute position, not a delta: {params}"
            );
        }
    }

    /// `1rpx` is the viewport width over 750 (393px here), `1ppx` one device
    /// pixel.
    #[test]
    fn scroll_to_converts_rpx_and_ppx_offsets() {
        let (mut document, view) = vertical(5);
        scroll_to(&mut document, view, r#"{"offset":"150rpx"}"#).expect("valid params");
        let offset = document.scroll_offset(view).y;
        assert!((offset - 78.6).abs() < 1e-4, "{offset}");

        let mut document = document_with_ratio(3.0);
        let view = document
            .get(document.document_element().id())
            .expect("the page")
            .child_ids()[0];
        scroll_to(&mut document, view, r#"{"offset":"60ppx"}"#).expect("valid params");
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 20.0));
        scroll_to(&mut document, view, r#"{"index":1,"offset":"30ppx"}"#).expect("valid params");
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 50.0));
    }

    /// [`vertical`]'s five children on a device with `ratio` device pixels
    /// per CSS pixel.
    fn document_with_ratio(ratio: f32) -> LynxDocument {
        let mut document = document();
        document.set_device_pixel_ratio(ratio);
        let view = child(
            &mut document,
            SCROLL_VIEW_TAG,
            "width: 100px; height: 100px; border: 10px solid",
        );
        for _ in 0..5 {
            element_under(&mut document, view, "view", "height: 40px; flex-shrink: 0");
        }
        document.layout();
        document
    }

    /// Instant by default and for a falsy `smooth`: the document moves at
    /// once. A truthy `smooth` is the painter's to animate, so only the
    /// request records the target.
    #[test]
    fn scroll_to_is_instant_unless_smooth_is_truthy() {
        for (smooth, behavior) in [
            (None, ScrollBehavior::Instant),
            (Some("false"), ScrollBehavior::Instant),
            (Some("0"), ScrollBehavior::Instant),
            (Some("\"\""), ScrollBehavior::Instant),
            (Some("null"), ScrollBehavior::Instant),
            (Some("true"), ScrollBehavior::Smooth),
            (Some("1"), ScrollBehavior::Smooth),
            (Some("\"no\""), ScrollBehavior::Smooth),
        ] {
            let (mut document, view) = vertical(5);
            let params = smooth.map_or_else(
                || r#"{"index":2}"#.to_owned(),
                |smooth| format!(r#"{{"index":2,"smooth":{smooth}}}"#),
            );
            assert_eq!(scroll_to(&mut document, view, &params), Ok(()), "{params}");
            let moved = if behavior == ScrollBehavior::Instant {
                80.0
            } else {
                0.0
            };
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, moved),
                "{params}"
            );
            assert_eq!(
                carried(&mut document, view),
                Some((Vector2D::new(0.0, 80.0), behavior)),
                "{params}"
            );
        }
    }

    /// A target past either end clamps to the range and the call succeeds
    /// (iOS and web-core).
    #[test]
    fn a_clamped_scroll_to_target_still_succeeds() {
        for (params, expected) in [
            (r#"{"index":4}"#, 120.0),
            (r#"{"index":3,"offset":1000}"#, 120.0),
            (r#"{"offset":-5}"#, 0.0),
            (r#"{"offset":1e308}"#, 120.0),
            (r#"{"offset":"1e300px"}"#, 120.0),
        ] {
            let (mut document, view) = vertical(5);
            assert_eq!(
                invoke(&mut document, view, "scrollTo", params),
                MethodOutcome::Done,
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, expected),
                "{params}"
            );
        }
    }

    #[test]
    fn scroll_to_refuses_bad_params_and_moves_nothing() {
        for params in [
            r#"{"index":-1}"#,
            r#"{"index":5}"#,
            r#"{"index":1e300}"#,
            r#"{"index":"1"}"#,
            r#"{"index":null}"#,
            r#"{"index":true}"#,
            r#"{"offset":"abc"}"#,
            r#"{"offset":"10vw"}"#,
            r#"{"offset":"10 px"}"#,
            r#"{"offset":""}"#,
            r#"{"offset":"NaN"}"#,
            r#"{"offset":"infpx"}"#,
            r#"{"offset":null}"#,
            r#"{"offset":true}"#,
            r#"{"offset":[10]}"#,
            "null",
            "[1]",
            "1",
            "\"index\"",
            "not json",
        ] {
            let (mut document, view) = vertical(5);
            document.scroll_to(view, Vector2D::new(0.0, 30.0));
            assert_eq!(
                invoke(&mut document, view, "scrollTo", params),
                MethodOutcome::Failed(MethodError::InvalidParams),
                "{params}"
            );
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, 30.0),
                "{params}"
            );
            assert_eq!(document.pending_scroll_request(view), None, "{params}");
        }
    }

    /// Native refuses `scrollTo` on a scroller with no children whatever the
    /// params; here an index is out of range there anyway, and an offset
    /// alone is a scroll, as web-core's is: to the clamped target, which with
    /// no content is 0.
    #[test]
    fn an_empty_scroller_refuses_an_index_and_takes_an_offset() {
        let mut document = document();
        let view = child(
            &mut document,
            SCROLL_VIEW_TAG,
            "width: 100px; height: 100px",
        );
        document.layout();
        assert_eq!(
            scroll_to(&mut document, view, r#"{"index":0}"#),
            Err(InvalidParams)
        );
        assert_eq!(document.pending_scroll_request(view), None);
        assert_eq!(scroll_to(&mut document, view, r#"{"offset":40}"#), Ok(()));
        assert_eq!(
            carried(&mut document, view),
            Some((Vector2D::zero(), ScrollBehavior::Instant))
        );
    }

    /// Only element children count, and a child with no box sits at 0.
    #[test]
    fn scroll_to_counts_element_children_and_places_a_boxless_one_at_zero() {
        let mut document = document();
        let view = child(
            &mut document,
            SCROLL_VIEW_TAG,
            "width: 100px; height: 100px",
        );
        let text = document.create_text_node("between", ());
        document.append_child(view, text);
        for style in [
            "height: 40px",
            "display: none",
            "height: 40px",
            "height: 200px",
        ] {
            element_under(&mut document, view, "view", style);
        }
        document.layout();
        assert_eq!(scroll_to(&mut document, view, r#"{"index":2}"#), Ok(()));
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 40.0));
        assert_eq!(
            scroll_to(&mut document, view, r#"{"index":1,"offset":10}"#),
            Ok(())
        );
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 10.0));
        assert_eq!(
            scroll_to(&mut document, view, r#"{"index":4}"#),
            Err(InvalidParams),
            "four element children, whatever else the scroller holds"
        );
    }

    /// The axis is the attributes': x moves and y keeps the offset it had.
    #[test]
    fn a_horizontal_scroller_moves_x_alone() {
        for attribute in [("scroll-x", ""), ("scroll-orientation", "horizontal")] {
            let (mut document, view) = scroller(Some(attribute), 5);
            document.scroll_to(view, Vector2D::new(0.0, 30.0));
            assert_eq!(
                scroll_to(&mut document, view, r#"{"index":1,"offset":"20px"}"#),
                Ok(()),
                "{attribute:?}"
            );
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(60.0, 30.0),
                "{attribute:?}"
            );
            assert_eq!(
                scroll_to(&mut document, view, r#"{"offset":500}"#),
                Ok(()),
                "{attribute:?}"
            );
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(120.0, 30.0),
                "{attribute:?}"
            );
        }
    }

    /// The cross axis's offset survives a vertical move too.
    #[test]
    fn a_vertical_scroller_keeps_its_x_offset() {
        let (mut document, view) = vertical(5);
        document.scroll_to(view, Vector2D::new(25.0, 0.0));
        scroll_to(&mut document, view, r#"{"index":3}"#).expect("valid params");
        assert_eq!(document.scroll_offset(view), Vector2D::new(25.0, 120.0));
    }

    /// A `scroll-view` with no box, or one that is no scroll container,
    /// succeeds, moves nothing, and no frame carries a request for it.
    #[test]
    fn a_scroller_that_cannot_scroll_moves_nothing() {
        for style in ["display: none", "overflow: visible"] {
            let mut document = document();
            let view = child(&mut document, SCROLL_VIEW_TAG, style);
            element_under(&mut document, view, "view", "height: 40px");
            document.layout();
            assert_eq!(
                invoke(&mut document, view, "scrollTo", r#"{"index":0,"offset":9}"#),
                MethodOutcome::Done,
                "{style}"
            );
            assert_eq!(
                scroll_by(&mut document, view, r#"{"offset":10}"#),
                Ok(ScrolledBy {
                    consumed: Vector2D::zero(),
                    unconsumed: Vector2D::new(10.0, 10.0),
                }),
                "{style}"
            );
            assert_eq!(document.scroll_offset(view), Vector2D::zero(), "{style}");
            let frame = document.commit();
            assert_eq!(frame.slot_of(view), None, "{style}");
            assert_eq!(document.pending_scroll_request(view), None, "{style}");
            assert_eq!(
                scroll_info(&document, view),
                ScrollInfo {
                    offset: Vector2D::zero(),
                    max_offset: 0.0,
                },
                "{style}"
            );
        }
    }

    /// The scrolling axis consumes what it moved; the cross axis consumes
    /// nothing and leaves the whole offset unconsumed, as the natives
    /// answer.
    #[test]
    fn scroll_by_answers_what_it_consumed() {
        let (mut document, view) = vertical(5);
        assert_eq!(
            scroll_by(&mut document, view, r#"{"offset":100}"#),
            Ok(ScrolledBy {
                consumed: Vector2D::new(0.0, 100.0),
                unconsumed: Vector2D::new(100.0, 0.0),
            })
        );
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 100.0));
        assert_eq!(
            carried(&mut document, view),
            Some((Vector2D::new(0.0, 100.0), ScrollBehavior::Instant)),
            "the painter is told, instantly"
        );
        assert_eq!(
            invoke(&mut document, view, "scrollBy", r#"{"offset":50}"#),
            MethodOutcome::Data("1:02:202:502:30".to_owned()),
            "consumedX 0, consumedY 20, unconsumedX 50, unconsumedY 30"
        );
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 120.0));
        assert_eq!(
            scroll_by(&mut document, view, r#"{"offset":-200.5}"#),
            Ok(ScrolledBy {
                consumed: Vector2D::new(0.0, -120.0),
                unconsumed: Vector2D::new(-200.5, -80.5),
            })
        );
        assert_eq!(document.scroll_offset(view), Vector2D::zero());

        let (mut document, view) = scroller(Some(("scroll-x", "")), 5);
        document.scroll_to(view, Vector2D::new(0.0, 30.0));
        assert_eq!(
            scroll_by(&mut document, view, r#"{"offset":130}"#),
            Ok(ScrolledBy {
                consumed: Vector2D::new(120.0, 0.0),
                unconsumed: Vector2D::new(10.0, 130.0),
            })
        );
        assert_eq!(document.scroll_offset(view), Vector2D::new(120.0, 30.0));
    }

    #[test]
    fn scroll_by_refuses_params_without_a_numeric_offset() {
        for params in [
            "{}",
            r#"{"offset":null}"#,
            r#"{"offset":"10"}"#,
            r#"{"offset":"10px"}"#,
            r#"{"offset":true}"#,
            "null",
            "10",
            "not json",
        ] {
            let (mut document, view) = vertical(5);
            assert_eq!(
                invoke(&mut document, view, "scrollBy", params),
                MethodOutcome::Failed(MethodError::InvalidParams),
                "{params}"
            );
            assert_eq!(document.scroll_offset(view), Vector2D::zero(), "{params}");
            assert_eq!(document.pending_scroll_request(view), None, "{params}");
        }
    }

    #[test]
    fn get_scroll_info_answers_the_offset_and_the_axis_range() {
        let (mut document, view) = vertical(5);
        document.scroll_to(view, Vector2D::new(15.0, 70.5));
        assert_eq!(
            scroll_info(&document, view),
            ScrollInfo {
                offset: Vector2D::new(15.0, 70.5),
                max_offset: 120.0,
            }
        );
        assert_eq!(
            invoke(&mut document, view, "getScrollInfo", "{}"),
            MethodOutcome::Data("2:154:70.53:1203:120".to_owned()),
            "scrollX, scrollY, scrollRange, maxScrollOffset"
        );
        assert_eq!(
            invoke(&mut document, view, "getScrollInfo", "not json"),
            MethodOutcome::Data("2:154:70.53:1203:120".to_owned()),
            "params are not read"
        );

        let (document, view) = scroller(Some(("scroll-orientation", "horizontal")), 3);
        assert_eq!(
            scroll_info(&document, view),
            ScrollInfo {
                offset: Vector2D::zero(),
                max_offset: 40.0,
            }
        );
    }

    /// `autoScroll` and `takeContentScreenshot` are not built, and every
    /// other name is left to the host.
    #[test]
    fn methods_not_built_are_not_found() {
        let (mut document, view) = vertical(5);
        for name in [
            "autoScroll",
            "takeContentScreenshot",
            "selectTab",
            "boundingClientRect",
            "scrollIntoView",
            "",
        ] {
            assert_eq!(
                invoke(&mut document, view, name, r#"{"rate":60,"start":true}"#),
                MethodOutcome::NotFound,
                "{name}"
            );
        }
        assert_eq!(document.pending_scroll_request(view), None);
    }
}
