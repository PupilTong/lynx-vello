//! The `scroll-view` tag: a main-thread scroll container, and the component
//! that answers four of its UI methods.
//!
//! The UA sheet carries the container itself: which axis scrolls, which one
//! clips, and which way the subtree stacks ([`UA_RULES`]). Its container
//! defaults (border box, the configured display mode) ride the same rule as
//! `view`'s, in [`super::ua_sheet`]. The component ([`ScrollView`]) adds
//! four of the methods Lynx documents for the tag
//! (lynxjs.org/api/elements/built-in/scroll-view), which the runtime's
//! `callElementMethod` reaches through
//! [`terminus::link::Document::invoke_element_method`]: `scrollTo` ([`scroll_to`]),
//! `scrollBy` ([`scroll_by`]), `autoScroll` ([`auto_scroll`]) and
//! `getScrollInfo` ([`scroll_info`]). It also fires the two threshold
//! events, `scrolltoupper` and `scrolltolower`, from the `scroll` it
//! observes at the element ([`ScrollEdges`], below); what neither the sheet
//! nor this module covers is recorded in `docs/tracking/deviations.md`.
//!
//! `list` is the other Lynx scroller and carries the same axis rules, written
//! against its own attribute spellings. They live with the rest of that tag's
//! policy in [`super::list`], one module per tag, as do its own methods,
//! which share this module's `scrollBy` ([`scroll_by`]) and `autoScroll`
//! ([`auto_scroll`]) whole, its offset reader ([`offset_px`]), its cell
//! geometry ([`rect_in_scroller`]) and its threshold events
//! ([`ScrollEdges`]).
//!
//! # The references
//!
//! - iOS `lynx/platform/darwin/ios/lynx/ui/scroll_view/LynxUIScroller.m`: `scrollBy`, `scrollTo`,
//!   `getScrollInfo`, `takeContentScreenshot` and `autoScroll` at `:1219-1360`,
//!   `clampScrollToPosition` at `:1078-1118`.
//! - Android `lynx/platform/android/lynx_android/src/main/java/com/lynx/tasm/behavior/ui/scroll/`
//!   `UIScrollView.java`: `autoScroll` at `:746-749`, `getScrollInfo` at `:922-931`, `scrollBy` and
//!   `scrollTo` at `:960-1050`; `AndroidScrollView.java:142-235` is the auto-scroll itself.
//! - Harmony `lynx/platform/harmony/lynx_harmony/src/main/cpp/ui/ui_scroll.cc`: the method switch
//!   at `:64-155`, `InvokeScrollTo` and `InvokeGetScrollInfo` at `:750-799`; the auto-scroll both
//!   its tags share is `ui/utils/auto_scroller.cc`.
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
//! # The threshold events
//!
//! `scrolltoupper` fires when a `scroll` leaves the element within
//! `upper-threshold` px of its start edge on the scrolling axis, having
//! been outside that zone at the previous `scroll` (or at none);
//! `scrolltolower` the same within `lower-threshold` px of its end edge.
//! That is native's rule, `updateScrollPosition` in iOS
//! `LynxUIScrollViewInternal.m:87-109` and Android
//! `LynxUIScrollViewInternal.java:90-109`: `offset <= upper` is the upper
//! zone, `offset >= max_offset - lower` the lower one, both edges start
//! outside their zone, and each event fires on entering its zone only — so
//! the first `scroll` that leaves a container still at its start fires a
//! `scrolltoupper`. Both thresholds are read at each `scroll` as web-core's
//! `parseInt(value)` px reads them (`ScrollViewEvents.ts:95-107`;
//! [`threshold_px`]); a missing, unreadable or negative one is 0.
//!
//! The events are decided on a `scroll` the element itself fired and on
//! nothing else, as native decides them: web-core's intersection observers
//! also fire when a layout change or the first render moves a sentinel into
//! the scrollport, which these never do. Geometry is the last completed
//! layout's ([`terminus::link::Document::scroll_box`]), read when the `scroll` is
//! delivered. Each event is queued only when a listener on the element
//! registered for its name ([`terminus::link::Document::has_listener`]), at the
//! element as the runtime queues a kernel `scroll` — not bubbling, not
//! composed — so its `detail` is the same scroll metrics, read at its own
//! delivery, with the change in offset since the element's previous
//! `scroll` as `deltaX`/`deltaY`. Whether each edge's zone holds the
//! offset is recorded at every `scroll` whether or not anything listens,
//! so a listener added while the container stands in a zone hears nothing
//! until the container leaves the zone and comes back. A `display: none`
//! element, or one that is no scroll container, decides nothing
//! ([`rendered_scroll_box`]).
//!
//! # The initial child
//!
//! `initial-scroll-to-index` names the child the view starts scrolled to:
//! web-core scrolls `children.item(index)` to the scrollport's start,
//! `scrollTop = child.offsetTop`, or `scrollLeft = child.offsetLeft` under
//! `scroll-x` (`ScrollAttributes.ts:64-81`). Android, iOS and Harmony read
//! the same name (`UIScrollView.java:699-707`, `AbsLynxUIScroller.m:35`,
//! `ui_scroll.h:33-34`) and also put the child's origin at the start
//! (`LynxUIScroller.m:577-589`, `ui_scroll.cc:297-304`).
//!
//! Three UA rules and no component code, the [`super::viewpager`] recipe
//! (`docs/style-assumptions.md` §27): a view with the attribute sets the
//! registered, inherited `<integer>` `--scroll-view-initial-index` from
//! `attr(initial-scroll-to-index type(<integer>), -1)`, and each of its
//! element children is its `scroll-initial-target` exactly when that number
//! equals the child's `sibling-index() - 1`. css-scroll-snap-2's initial
//! target does the rest (`crates/terminus/src/scroll/initial_target.rs`):
//! the first layout with a target scrolls the view to it, instantly, in the
//! same commit, and the frame carries the position to the painter as an
//! instant scroll request.
//!
//! - **Alignment.** The target is placed as `scrollIntoView` places it with `block: start` and
//!   `inline: nearest` (css-scroll-snap-2 §3.1.1). On a vertical view the child's top meets the
//!   scrollport's top, web-core's position. On a horizontal view the x axis is the inline axis: a
//!   child as wide as the scrollport or wider lands at its start, but a narrower child already
//!   inside the scrollport moves nothing and one past its end lands with its end edge at the
//!   scrollport's end, where web-core puts its start edge at the scrollport's start. CSS has no way
//!   to ask the initial target for `inline: start` without adding snapping or changing the
//!   snapport, so this gap is recorded in `docs/tracking/deviations.md`.
//! - **The index** counts the view's element children, web-core's `children.item`, whether or not
//!   they render. An index that is negative, past the last child or no integer (`"abc"`, `"1.5"`,
//!   `""`) names no child: the view starts at 0. web-core's `parseFloat` makes `"1.5"` child 1.
//! - **A later change** names a new target, which is honoured once, so the view scrolls to the new
//!   child, instantly. web-core does the same, its handler running on every change of the
//!   attribute, and so does Android outside a list (`UIScrollView.java:653-667`); iOS
//!   (`LynxUIScroller.m:591-614`) and Harmony (`ui_scroll.cc:298-299`) honour the attribute once. A
//!   commit that leaves the target where it was moves nothing, so a user who scrolled away stays,
//!   and removing the attribute moves nothing.
//! - **Nesting.** Only a view that carries the attribute sets the property, and only its children
//!   are tested against it, so a nested view without the attribute keeps the outer view's index for
//!   its own target and gives its children none. A nested view that is the direct child of a view
//!   and carries the attribute itself tests its own index, not its parent's, for its own target:
//!   one inherited property cannot hold both numbers.
//! - `scroll-to-index`, which web-core binds to the same handler, re-scrolls on every write and is
//!   no initial target; it is not built.
//!
//! # Not built
//!
//! `takeContentScreenshot` stays unanswered (code 3): it needs a pixel
//! readback of the scroller's whole content from the painter, which does not
//! exist.

use std::cell::RefCell;

use rustc_hash::FxHashMap;
use serde_json::{Map, Value};
use terminus::link::event::ElementEvent;
use terminus::link::scroll::{ScrollBehavior, ScrollBox, SnapAxis, SnapPoint};
use terminus::link::{
    CustomElement, MethodCall, MethodError, MethodOutcome, NodeId, Point2D, Rect, Size2D, Vector2D,
};

use super::{ComponentEvents, InvalidParams, LynxDocument, element_child, is_truthy};
use crate::main::record::write_record_field;

pub(super) const SCROLL_VIEW_TAG: &str = "scroll-view";

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
@property --scroll-view-initial-index { syntax: "<integer>"; inherits: true; initial-value: -1; }
scroll-view[initial-scroll-to-index] {
  --scroll-view-initial-index: attr(initial-scroll-to-index type(<integer>), -1);
}
scroll-view[initial-scroll-to-index] > * {
  scroll-initial-target:
    if(style(--scroll-view-initial-index: calc(sibling-index() - 1)): nearest; else: none);
}
"#;

pub(super) const SCROLL_EVENT: &str = "scroll";

pub(super) const SCROLL_END_EVENT: &str = "scrollend";

pub(super) const SCROLL_TO_UPPER_EVENT: &str = "scrolltoupper";

pub(super) const SCROLL_TO_LOWER_EVENT: &str = "scrolltolower";

pub(super) const UPPER_THRESHOLD: &str = "upper-threshold";

pub(super) const LOWER_THRESHOLD: &str = "lower-threshold";

pub(super) fn define(document: &mut LynxDocument, events: ComponentEvents) {
    document.define(
        SCROLL_VIEW_TAG,
        Box::new(ScrollView {
            events,
            edges: ScrollEdges::default(),
        }),
    );
}

struct ScrollView {
    events: ComponentEvents,
    edges: ScrollEdges,
}

impl CustomElement<()> for ScrollView {
    fn observed_events(&self) -> &'static [&'static str] {
        &[SCROLL_EVENT]
    }

    fn freed(&self, element: NodeId) {
        self.edges.forget(element);
    }

    fn handle_event(&self, document: &mut LynxDocument, element: NodeId, event: &mut ElementEvent) {
        if !is_own_scroll(element, event) {
            return;
        }
        let Some(scroll_box) = rendered_scroll_box(document, element) else {
            return;
        };
        let horizontal = is_horizontal(document, element);
        let offset = along(scroll_box.offset, horizontal);
        let max = along(scroll_box.max_offset(), horizontal);
        let zones = Zones {
            upper: offset <= threshold_px(document, element, UPPER_THRESHOLD),
            lower: offset >= max - threshold_px(document, element, LOWER_THRESHOLD),
        };
        self.edges
            .record(document, &self.events, element, scroll_box.offset, zones);
    }

    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        let answer = match call.name {
            "scrollTo" => scroll_to(document, element, call.params).map(|()| None),
            "scrollBy" => scroll_by(
                document,
                element,
                call.params,
                is_horizontal(document, element),
            )
            .map(|by| Some(by.record())),
            "autoScroll" => {
                let horizontal = is_horizontal(document, element);
                auto_scroll(document, element, call.params, horizontal).map(|()| None)
            }
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

fn is_horizontal(document: &LynxDocument, view: NodeId) -> bool {
    document.get(view).is_some_and(|node| {
        node.attribute("scroll-x").is_some()
            || node.attribute("scroll-orientation").as_deref() == Some("horizontal")
    })
}

pub(super) fn along(vector: Vector2D<f32>, horizontal: bool) -> f32 {
    if horizontal { vector.x } else { vector.y }
}

pub(super) fn with_axis(vector: Vector2D<f32>, horizontal: bool, value: f32) -> Vector2D<f32> {
    if horizontal {
        Vector2D::new(value, vector.y)
    } else {
        Vector2D::new(vector.x, value)
    }
}

pub(super) fn is_own_scroll(element: NodeId, event: &ElementEvent<'_>) -> bool {
    event.type_name() == SCROLL_EVENT && event.target() == element
}

pub(super) fn rendered_scroll_box(document: &LynxDocument, scroller: NodeId) -> Option<ScrollBox> {
    document
        .generates_rendered_box(scroller)
        .then(|| document.scroll_box(scroller))
        .flatten()
}

pub(super) fn nearest_snap_position(axis: &SnapAxis<'_>, offset: f32) -> Option<usize> {
    let offset = if offset.is_finite() { offset } else { 0.0 };
    let period = axis
        .period
        .filter(|period| period.is_finite() && *period > 0.0);
    let at = period.map_or(offset, |period| {
        let at = offset.rem_euclid(period);
        if at < period { at } else { 0.0 }
    });
    let distance = |point: &SnapPoint| {
        let from = |shift: f32| {
            (point.min + shift - at)
                .max(at - point.max - shift)
                .max(0.0)
        };
        period.map_or_else(
            || from(0.0),
            |period| from(0.0).min(from(-period)).min(from(period)),
        )
    };
    (0..axis.points.len())
        .min_by(|&a, &b| distance(&axis.points[a]).total_cmp(&distance(&axis.points[b])))
}

pub(super) fn threshold_px(document: &LynxDocument, scroller: NodeId, name: &str) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a px threshold; one past f32 saturates to infinity, as far as any range"
    )]
    document
        .get(scroller)
        .and_then(|node| node.attribute(name))
        .and_then(|value| parse_int(&value))
        .map_or(0.0, |value| value.max(0.0) as f32)
}

pub(super) fn parse_int(text: &str) -> Option<f64> {
    let text = text.trim_start();
    let (negative, text) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (radix, digits) = match text.get(..2) {
        Some("0x" | "0X") => (16, &text[2..]),
        _ => (10, text),
    };
    let mut value: Option<f64> = None;
    for digit in digits.chars().map_while(|digit| digit.to_digit(radix)) {
        value = Some(value.unwrap_or(0.0) * f64::from(radix) + f64::from(digit));
    }
    value.map(|value| if negative { -value } else { value })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Zones {
    pub(super) upper: bool,
    pub(super) lower: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct EdgeState {
    zones: Zones,
    offset: Vector2D<f32>,
}

#[derive(Debug, Default)]
pub(super) struct ScrollEdges(RefCell<FxHashMap<NodeId, EdgeState>>);

impl ScrollEdges {
    pub(super) fn record(
        &self,
        document: &LynxDocument,
        events: &ComponentEvents,
        scroller: NodeId,
        offset: Vector2D<f32>,
        zones: Zones,
    ) {
        let (entered, delta) = {
            let mut table = self.0.borrow_mut();
            let state = table.entry(scroller).or_default();
            let entered = Zones {
                upper: zones.upper && !state.zones.upper,
                lower: zones.lower && !state.zones.lower,
            };
            let delta = offset - state.offset;
            *state = EdgeState { zones, offset };
            (entered, delta)
        };
        for (name, entered) in [
            (SCROLL_TO_UPPER_EVENT, entered.upper),
            (SCROLL_TO_LOWER_EVENT, entered.lower),
        ] {
            if entered && document.has_listener(scroller, name) {
                events.queue_scroll(scroller, name, delta);
            }
        }
    }

    pub(super) fn forget(&self, scroller: NodeId) {
        self.0.borrow_mut().remove(&scroller);
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "clamped to the scroll range, which is an f32"
)]
pub(super) fn clamp_to_range(value: f64, max: f32) -> f32 {
    value.clamp(0.0, f64::from(max)) as f32
}

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

pub(super) fn object(params: &str) -> Result<Map<String, Value>, InvalidParams> {
    match serde_json::from_str(params) {
        Ok(Value::Object(params)) => Ok(params),
        _ => Err(InvalidParams),
    }
}

fn child_position(
    document: &LynxDocument,
    view: NodeId,
    index: f64,
    horizontal: bool,
) -> Result<f32, InvalidParams> {
    let child = element_child(document, view, index).ok_or(InvalidParams)?;
    Ok(rect_in_scroller(document, view, child)
        .map_or(0.0, |rect| along(rect.origin.to_vector(), horizontal)))
}

pub(super) fn rect_in_scroller(
    document: &LynxDocument,
    scroller: NodeId,
    child: NodeId,
) -> Option<Rect<f32>> {
    let layout = document.rounded_layout(child)?;
    let container = document.rounded_layout(scroller)?;
    Some(Rect::new(
        Point2D::new(
            layout.location.x - container.border.left,
            layout.location.y - container.border.top,
        ),
        Size2D::new(layout.size.width, layout.size.height),
    ))
}

pub(crate) fn relevance_span(
    document: &LynxDocument,
    scroller: NodeId,
    child: NodeId,
) -> Option<[f32; 4]> {
    let scroll_box = rendered_scroll_box(document, scroller)?;
    if !document.generates_rendered_box(child) {
        return None;
    }
    let (low, high) = document.scroll_relevance_window(scroller)?;
    let rect = rect_in_scroller(document, scroller, child)?;
    Some([
        high.x + scroll_box.scrollport.width - rect.min_x(),
        rect.max_x() - low.x,
        high.y + scroll_box.scrollport.height - rect.min_y(),
        rect.max_y() - low.y,
    ])
}

pub(super) fn offset_px(document: &LynxDocument, offset: &Value) -> Option<f64> {
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrolledBy {
    pub(super) consumed: Vector2D<f32>,
    pub(super) unconsumed: Vector2D<f32>,
}

impl ScrolledBy {
    pub(super) fn record(self) -> String {
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

pub(super) fn scroll_by(
    document: &mut LynxDocument,
    view: NodeId,
    params: &str,
    horizontal: bool,
) -> Result<ScrolledBy, InvalidParams> {
    let params = object(params)?;
    let delta = params
        .get("offset")
        .and_then(Value::as_f64)
        .ok_or(InvalidParams)?;
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

pub(super) fn auto_scroll(
    document: &mut LynxDocument,
    view: NodeId,
    params: &str,
    horizontal: bool,
) -> Result<(), InvalidParams> {
    let params = object(params)?;
    if !params.get("start").is_some_and(is_truthy) {
        document.auto_scroll(view, 0.0, horizontal, true);
        return Ok(());
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a rate beyond f32 is refused just below"
    )]
    let rate = params
        .get("rate")
        .and_then(|rate| offset_px(document, rate))
        .ok_or(InvalidParams)? as f32;
    if rate == 0.0 || !rate.is_finite() {
        return Err(InvalidParams);
    }
    let stop_at_edge = params.get("autoStop") != Some(&Value::Bool(false));
    document.auto_scroll(view, rate, horizontal, stop_at_edge);
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollInfo {
    pub(super) offset: Vector2D<f32>,
    pub(super) max_offset: f32,
}

impl ScrollInfo {
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
pub(super) mod tests {
    #![allow(clippy::float_cmp)]

    use terminus::link::event::{EventDetail, ListenerKey, ListenerKinds};
    use terminus::link::scroll::ScrollBehavior;
    use terminus::link::stylo::computed_values::{flex_direction, linear_direction};
    use terminus::link::stylo::properties::PropertyId;
    use terminus::link::stylo::values::computed::Overflow;
    use terminus::link::{MethodCall, MethodError, MethodOutcome, NodeId, Vector2D};

    use super::super::test_support::{
        child, document, element_under, overflow, style_of, with_component_events,
    };
    use super::super::{ComponentEvent, ComponentEvents, LynxDocument, PageConfig};
    use super::{
        EdgeState, InvalidParams, LOWER_THRESHOLD, SCROLL_TO_LOWER_EVENT, SCROLL_TO_UPPER_EVENT,
        SCROLL_VIEW_TAG, ScrollEdges, ScrollInfo, ScrolledBy, UPPER_THRESHOLD, Zones, auto_scroll,
        parse_int, scroll_by, scroll_info, scroll_to, threshold_px,
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

    #[test]
    fn scroll_to_counts_element_children_and_places_a_boxless_one_at_zero() {
        let mut document = document();
        let view = child(
            &mut document,
            SCROLL_VIEW_TAG,
            "width: 100px; height: 100px",
        );
        let text = document.create_text_node(" \n ", ());
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

    #[test]
    fn a_vertical_scroller_keeps_its_x_offset() {
        let (mut document, view) = vertical(5);
        document.scroll_to(view, Vector2D::new(25.0, 0.0));
        scroll_to(&mut document, view, r#"{"index":3}"#).expect("valid params");
        assert_eq!(document.scroll_offset(view), Vector2D::new(25.0, 120.0));
    }

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
                scroll_by(&mut document, view, r#"{"offset":10}"#, false),
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

    #[test]
    fn scroll_by_answers_what_it_consumed() {
        let (mut document, view) = vertical(5);
        assert_eq!(
            scroll_by(&mut document, view, r#"{"offset":100}"#, false),
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
            scroll_by(&mut document, view, r#"{"offset":-200.5}"#, false),
            Ok(ScrolledBy {
                consumed: Vector2D::new(0.0, -120.0),
                unconsumed: Vector2D::new(-200.5, -80.5),
            })
        );
        assert_eq!(document.scroll_offset(view), Vector2D::zero());

        let (mut document, view) = scroller(Some(("scroll-x", "")), 5);
        document.scroll_to(view, Vector2D::new(0.0, 30.0));
        assert_eq!(
            scroll_by(&mut document, view, r#"{"offset":130}"#, true),
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

    fn auto(document: &LynxDocument, view: NodeId) -> Option<(f32, bool, bool)> {
        document
            .pending_auto_scroll(view)
            .map(|request| (request.rate, request.horizontal, request.stop_at_edge))
    }

    #[test]
    fn auto_scroll_starts_on_the_scrolling_axis_and_stops_without_start() {
        let (mut document, view) = vertical(5);
        assert_eq!(
            invoke(
                &mut document,
                view,
                "autoScroll",
                r#"{"rate":60,"start":true}"#
            ),
            MethodOutcome::Done
        );
        assert_eq!(auto(&document, view), Some((60.0, false, true)));
        assert_eq!(document.scroll_offset(view), Vector2D::zero());
        assert_eq!(document.pending_scroll_request(view), None);
        let started = document.pending_auto_scroll(view).expect("recorded").serial;

        for stop in [r#"{"rate":60}"#, r#"{"rate":60,"start":false}"#, "{}"] {
            auto_scroll(&mut document, view, stop, false).expect("valid params");
            let stopped = document.pending_auto_scroll(view).expect("recorded");
            assert_eq!(stopped.rate, 0.0, "{stop}");
            assert!(stopped.serial > started, "{stop}: a newer change");
        }

        let (mut document, view) = scroller(Some(("scroll-x", "")), 5);
        assert_eq!(
            invoke(
                &mut document,
                view,
                "autoScroll",
                r#"{"rate":-45.5,"start":1}"#
            ),
            MethodOutcome::Done
        );
        assert_eq!(auto(&document, view), Some((-45.5, true, true)));
    }

    #[test]
    fn auto_scroll_reads_unit_rates_and_auto_stop() {
        let (mut document, view) = vertical(5);
        for (params, expected) in [
            (r#"{"rate":"80px","start":true}"#, (80.0, false, true)),
            (r#"{"rate":"150rpx","start":true}"#, (78.6, false, true)),
            (r#"{"rate":"-12","start":true}"#, (-12.0, false, true)),
            (
                r#"{"rate":20,"start":true,"autoStop":false}"#,
                (20.0, false, false),
            ),
            (
                r#"{"rate":20,"start":true,"autoStop":0}"#,
                (20.0, false, true),
            ),
            (
                r#"{"rate":20,"start":true,"autoStop":true}"#,
                (20.0, false, true),
            ),
        ] {
            auto_scroll(&mut document, view, params, false).expect("valid params");
            let (rate, horizontal, stop_at_edge) = auto(&document, view).expect("recorded");
            assert!((rate - expected.0).abs() < 0.001, "{params}: got {rate}");
            assert_eq!(
                (horizontal, stop_at_edge),
                (expected.1, expected.2),
                "{params}"
            );
        }
    }

    #[test]
    fn auto_scroll_refuses_a_start_without_a_rate() {
        let (mut document, view) = vertical(5);
        auto_scroll(&mut document, view, r#"{"rate":60,"start":true}"#, false)
            .expect("valid params");
        let pending = document.pending_auto_scroll(view);
        for params in [
            r#"{"start":true}"#,
            r#"{"rate":0,"start":true}"#,
            r#"{"rate":"0px","start":true}"#,
            r#"{"rate":"fast","start":true}"#,
            r#"{"rate":true,"start":true}"#,
            r#"{"rate":1e300,"start":true}"#,
            "[]",
            "not json",
        ] {
            assert_eq!(
                invoke(&mut document, view, "autoScroll", params),
                MethodOutcome::Failed(MethodError::InvalidParams),
                "{params}"
            );
            assert_eq!(document.pending_auto_scroll(view), pending, "{params}");
        }
    }

    #[test]
    fn auto_scroll_on_a_scroll_view_with_no_box_starts_nothing() {
        let (mut document, view) = vertical(5);
        document.set_inline_style(view, "display: none");
        document.layout();
        assert_eq!(
            invoke(
                &mut document,
                view,
                "autoScroll",
                r#"{"rate":60,"start":true}"#
            ),
            MethodOutcome::Done
        );
        let frame = document.commit();
        assert_eq!(frame.slot_of(view), None);
        assert_eq!(frame.auto_scroll_requests(), []);
        assert_eq!(document.pending_auto_scroll(view), None);
    }

    #[test]
    fn methods_not_built_are_not_found() {
        let (mut document, view) = vertical(5);
        for name in [
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

    pub(in crate::main::tree) fn heard(
        events: &ComponentEvents,
        scroller: NodeId,
    ) -> Vec<(&'static str, Vector2D<f32>)> {
        events
            .take()
            .into_iter()
            .map(|event| match event {
                ComponentEvent::Scroll { node, name, delta } => {
                    assert_eq!(node, scroller, "queued at the scroller itself");
                    (name, delta)
                }
                other => panic!("only scroll-shaped events are queued: {other:?}"),
            })
            .collect()
    }

    pub(in crate::main::tree) fn scrolled(
        document: &mut LynxDocument,
        scroller: NodeId,
        offset: Vector2D<f32>,
    ) {
        document.scroll_to(scroller, offset);
        document.dispatch_element_event(scroller, "scroll", &EventDetail::None, false, false, 0.0);
    }

    pub(in crate::main::tree) fn listen(document: &mut LynxDocument, node: NodeId, name: &str) {
        document.add_listener(node, name, ListenerKinds::PATH_BUBBLE, ListenerKey(0));
    }

    fn edge_scroller(attributes: &[(&str, &str)]) -> (LynxDocument, ComponentEvents, NodeId) {
        let (mut document, events) = with_component_events(PageConfig::default());
        let view = child(
            &mut document,
            SCROLL_VIEW_TAG,
            "width: 100px; height: 100px; border: 10px solid",
        );
        let mut horizontal = false;
        for (name, value) in attributes {
            document.set_attribute(view, name, value);
            horizontal |= *name == "scroll-x";
        }
        for _ in 0..5 {
            let style = if horizontal {
                "width: 40px; height: 150px; flex-shrink: 0"
            } else {
                "width: 150px; height: 40px; flex-shrink: 0"
            };
            element_under(&mut document, view, "view", style);
        }
        document.layout();
        (document, events, view)
    }

    fn y(offset: f32) -> Vector2D<f32> {
        Vector2D::new(0.0, offset)
    }

    #[test]
    fn a_scroll_with_no_listener_queues_nothing_and_records_only_the_zones() {
        let (mut document, events, view) = edge_scroller(&[]);
        for offset in [0.0, 60.0, 120.0, 0.0] {
            scrolled(&mut document, view, y(offset));
        }
        assert_eq!(heard(&events, view), [], "nobody listens on the element");

        let edges = ScrollEdges::default();
        edges.record(
            &document,
            &events,
            view,
            y(0.0),
            Zones {
                upper: true,
                lower: false,
            },
        );
        assert!(events.is_empty());
        assert_eq!(
            edges.0.borrow().get(&view),
            Some(&EdgeState {
                zones: Zones {
                    upper: true,
                    lower: false,
                },
                offset: y(0.0),
            })
        );
        edges.forget(view);
        assert!(
            edges.0.borrow().is_empty(),
            "a freed element leaves no entry"
        );
    }

    #[test]
    fn entering_the_upper_zone_fires_once_and_again_after_leaving_it() {
        let (mut document, events, view) = edge_scroller(&[(UPPER_THRESHOLD, "10")]);
        listen(&mut document, view, SCROLL_TO_UPPER_EVENT);
        scrolled(&mut document, view, y(5.0));
        assert_eq!(heard(&events, view), [(SCROLL_TO_UPPER_EVENT, y(5.0))]);
        scrolled(&mut document, view, y(3.0));
        scrolled(&mut document, view, y(10.0));
        assert_eq!(heard(&events, view), [], "still inside the zone");
        scrolled(&mut document, view, y(60.0));
        assert_eq!(heard(&events, view), [], "leaving the zone fires nothing");
        scrolled(&mut document, view, y(8.0));
        assert_eq!(
            heard(&events, view),
            [(SCROLL_TO_UPPER_EVENT, y(-52.0))],
            "back in the zone, with the change since the previous scroll"
        );
    }

    #[test]
    fn entering_the_lower_zone_counts_its_threshold_from_the_end() {
        let (mut document, events, view) = edge_scroller(&[(LOWER_THRESHOLD, "20px")]);
        listen(&mut document, view, SCROLL_TO_LOWER_EVENT);
        listen(&mut document, view, SCROLL_TO_UPPER_EVENT);
        scrolled(&mut document, view, y(90.0));
        assert_eq!(heard(&events, view), [], "short of 120 - 20");
        scrolled(&mut document, view, y(100.0));
        assert_eq!(heard(&events, view), [(SCROLL_TO_LOWER_EVENT, y(10.0))]);
        scrolled(&mut document, view, y(120.0));
        scrolled(&mut document, view, y(60.0));
        assert_eq!(heard(&events, view), []);
        scrolled(&mut document, view, y(110.0));
        assert_eq!(heard(&events, view), [(SCROLL_TO_LOWER_EVENT, y(50.0))]);
    }

    #[test]
    fn a_horizontal_scroller_decides_on_its_x_offset() {
        let (mut document, events, view) =
            edge_scroller(&[("scroll-x", ""), (LOWER_THRESHOLD, "0")]);
        listen(&mut document, view, SCROLL_TO_UPPER_EVENT);
        listen(&mut document, view, SCROLL_TO_LOWER_EVENT);
        scrolled(&mut document, view, Vector2D::new(120.0, 0.0));
        assert_eq!(
            heard(&events, view),
            [(SCROLL_TO_LOWER_EVENT, Vector2D::new(120.0, 0.0))],
            "a y offset of 0 is no upper edge on an x scroller"
        );
        scrolled(&mut document, view, Vector2D::new(0.0, 70.0));
        assert_eq!(
            heard(&events, view),
            [(SCROLL_TO_UPPER_EVENT, Vector2D::new(-120.0, 70.0))],
            "a y offset at its end is no lower edge either"
        );
    }

    #[test]
    fn content_that_fits_is_at_both_edges_upper_first() {
        let (mut document, events) = with_component_events(PageConfig::default());
        let view = child(
            &mut document,
            SCROLL_VIEW_TAG,
            "width: 100px; height: 100px",
        );
        element_under(&mut document, view, "view", "height: 40px");
        document.layout();
        listen(&mut document, view, SCROLL_TO_UPPER_EVENT);
        listen(&mut document, view, SCROLL_TO_LOWER_EVENT);
        scrolled(&mut document, view, y(0.0));
        assert_eq!(
            heard(&events, view),
            [
                (SCROLL_TO_UPPER_EVENT, y(0.0)),
                (SCROLL_TO_LOWER_EVENT, y(0.0)),
            ]
        );
    }

    #[test]
    fn a_listener_added_inside_a_zone_waits_for_the_next_entry() {
        let (mut document, events, view) = edge_scroller(&[]);
        scrolled(&mut document, view, y(0.0));
        listen(&mut document, view, SCROLL_TO_UPPER_EVENT);
        scrolled(&mut document, view, y(0.0));
        assert_eq!(
            heard(&events, view),
            [],
            "the zone was entered before anyone listened"
        );
        scrolled(&mut document, view, y(40.0));
        scrolled(&mut document, view, y(0.0));
        assert_eq!(heard(&events, view), [(SCROLL_TO_UPPER_EVENT, y(-40.0))]);
    }

    #[test]
    fn a_nested_scrollers_scroll_is_not_its_ancestors() {
        let (mut document, events, outer) = edge_scroller(&[]);
        let inner = element_under(
            &mut document,
            outer,
            SCROLL_VIEW_TAG,
            "width: 50px; height: 50px; flex-shrink: 0",
        );
        element_under(
            &mut document,
            inner,
            "view",
            "height: 200px; flex-shrink: 0",
        );
        document.layout();
        listen(&mut document, outer, SCROLL_TO_UPPER_EVENT);
        listen(&mut document, inner, SCROLL_TO_LOWER_EVENT);
        scrolled(&mut document, inner, y(150.0));
        assert_eq!(
            heard(&events, inner),
            [(SCROLL_TO_LOWER_EVENT, y(150.0))],
            "the outer scroller, at its own start, hears the inner one's scroll in the capture \
             pass and decides nothing"
        );
    }

    #[test]
    fn a_scroller_with_no_box_decides_nothing() {
        let (mut document, events, view) = edge_scroller(&[]);
        listen(&mut document, view, SCROLL_TO_UPPER_EVENT);
        listen(&mut document, view, SCROLL_TO_LOWER_EVENT);
        document.set_inline_style(view, "display: none");
        document.layout();
        scrolled(&mut document, view, y(0.0));
        assert_eq!(heard(&events, view), []);

        document.set_inline_style(view, "width: 100px; height: 100px; border: 10px solid");
        document.layout();
        scrolled(&mut document, view, y(0.0));
        assert_eq!(heard(&events, view), [(SCROLL_TO_UPPER_EVENT, y(0.0))]);
    }

    #[test]
    fn a_threshold_is_read_as_parse_int_reads_it() {
        for (text, expected) in [
            ("50", Some(50.0)),
            ("50px", Some(50.0)),
            ("  12.9", Some(12.0)),
            ("+7rpx", Some(7.0)),
            ("-3", Some(-3.0)),
            ("0x1A", Some(26.0)),
            ("", None),
            ("px", None),
            ("-", None),
            ("0x", None),
        ] {
            assert_eq!(parse_int(text), expected, "{text:?}");
        }
        let (mut document, _events, view) = edge_scroller(&[]);
        for (value, expected) in [("30", 30.0), ("-30", 0.0), ("auto", 0.0)] {
            document.set_attribute(view, UPPER_THRESHOLD, value);
            assert_eq!(
                threshold_px(&document, view, UPPER_THRESHOLD),
                expected,
                "{value}"
            );
        }
        assert_eq!(
            threshold_px(&document, view, LOWER_THRESHOLD),
            0.0,
            "missing"
        );
    }

    #[test]
    fn the_nearest_snap_position_wraps_on_a_circular_axis() {
        use terminus::link::scroll::{SnapAxis, SnapPoint, SnapStrictness};

        use super::nearest_snap_position;

        let area = document().document_element().id();
        let point = |min: f32, max: f32| SnapPoint {
            min,
            max,
            stop: false,
            area,
            order: 0,
        };
        let points = [point(0.0, 0.0), point(200.0, 300.0), point(500.0, 500.0)];
        let axis = |period| SnapAxis {
            strictness: SnapStrictness::Mandatory,
            points: &points,
            period,
        };
        let bounded = axis(None);
        for (offset, expected) in [
            (0.0, Some(0)),
            (100.0, Some(0)),
            (101.0, Some(1)),
            (250.0, Some(1)),
            (400.0, Some(1)),
            (401.0, Some(2)),
            (700.0, Some(2)),
            (f32::NAN, Some(0)),
        ] {
            assert_eq!(
                nearest_snap_position(&bounded, offset),
                expected,
                "{offset}"
            );
        }
        let circular = axis(Some(600.0));
        for (offset, expected) in [
            (540.0, Some(2)),
            (560.0, Some(0)),
            (550.0, Some(0)),
            (1140.0, Some(2)),
            (-30.0, Some(0)),
            (f32::INFINITY, Some(0)),
        ] {
            assert_eq!(
                nearest_snap_position(&circular, offset),
                expected,
                "{offset}"
            );
        }
        let empty = SnapAxis {
            strictness: SnapStrictness::Mandatory,
            points: &[],
            period: None,
        };
        assert_eq!(nearest_snap_position(&empty, 10.0), None);
    }

    fn computed(document: &LynxDocument, element: NodeId, property: &str) -> String {
        let id = PropertyId::parse_enabled_for_all_content(property)
            .unwrap_or_else(|()| panic!("unknown property `{property}`"));
        let declaration = id
            .as_shorthand()
            .err()
            .unwrap_or_else(|| panic!("`{property}` is a shorthand"));
        style_of(document, element).computed_value_to_string(declaration)
    }

    fn targets(document: &LynxDocument, children: &[NodeId]) -> Vec<String> {
        children
            .iter()
            .map(|child| computed(document, *child, "scroll-initial-target"))
            .collect()
    }

    fn indexed(
        view_style: &str,
        child_style: &str,
        attributes: &[(&str, &str)],
        count: usize,
    ) -> (LynxDocument, NodeId, Vec<NodeId>) {
        let mut document = document();
        let view = child(&mut document, SCROLL_VIEW_TAG, view_style);
        for (name, value) in attributes {
            document.set_attribute(view, name, value);
        }
        let children = (0..count)
            .map(|_| element_under(&mut document, view, "view", child_style))
            .collect();
        (document, view, children)
    }

    fn vertical_indexed(
        attributes: &[(&str, &str)],
        count: usize,
    ) -> (LynxDocument, NodeId, Vec<NodeId>) {
        let mut all = vec![("scroll-y", "")];
        all.extend_from_slice(attributes);
        indexed(
            "width: 300px; height: 600px",
            "width: 300px; height: 300px; flex-shrink: 0",
            &all,
            count,
        )
    }

    #[test]
    fn initial_scroll_to_index_starts_a_vertical_view_on_the_child() {
        for (index, offset) in [("0", 0.0), ("1", 300.0), ("2", 600.0)] {
            let (mut document, view, children) =
                vertical_indexed(&[("initial-scroll-to-index", index)], 4);
            let request = carried(&mut document, view);
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(0.0, offset),
                "{index}"
            );
            assert_eq!(
                request,
                Some((Vector2D::new(0.0, offset), ScrollBehavior::Instant)),
                "{index}"
            );
            assert_eq!(
                computed(&document, view, "--scroll-view-initial-index"),
                index
            );
            let expected: Vec<&str> = (0..4)
                .map(|child| {
                    if child.to_string() == index {
                        "nearest"
                    } else {
                        "none"
                    }
                })
                .collect();
            assert_eq!(targets(&document, &children), expected, "{index}");
        }
    }

    #[test]
    fn the_scroll_to_index_card_starts_on_its_second_child() {
        let (mut document, view, _) = vertical_indexed(&[("initial-scroll-to-index", "1")], 3);
        document.commit();
        assert_eq!(document.scroll_offset(view), Vector2D::new(0.0, 300.0));
        let scroll_box = document.scroll_box(view).expect("a scroll container");
        assert_eq!(scroll_box.max_offset().y, 300.0);
    }

    #[test]
    fn initial_scroll_to_index_starts_a_horizontal_view_on_the_child() {
        for axis in [("scroll-x", ""), ("scroll-orientation", "horizontal")] {
            let (mut document, view, children) = indexed(
                "width: 300px; height: 300px",
                "width: 300px; height: 300px; flex-shrink: 0",
                &[axis, ("initial-scroll-to-index", "1")],
                3,
            );
            let request = carried(&mut document, view);
            assert_eq!(
                document.scroll_offset(view),
                Vector2D::new(300.0, 0.0),
                "{axis:?}"
            );
            assert_eq!(
                request,
                Some((Vector2D::new(300.0, 0.0), ScrollBehavior::Instant))
            );
            assert_eq!(targets(&document, &children), ["none", "nearest", "none"]);
        }
    }

    #[test]
    fn a_narrow_child_on_x_is_aligned_nearest_not_at_the_start() {
        for (index, offset) in [("1", 0.0), ("4", 200.0)] {
            let (mut document, view, _) = indexed(
                "width: 300px; height: 100px",
                "width: 100px; height: 100px; flex-shrink: 0",
                &[("scroll-x", ""), ("initial-scroll-to-index", index)],
                6,
            );
            document.commit();
            assert_eq!(document.scroll_offset(view).x, offset, "{index}");
        }
    }

    #[test]
    fn an_index_naming_no_child_scrolls_nothing() {
        for value in [None, Some("-1"), Some("-5"), Some("4"), Some("10")]
            .into_iter()
            .chain(["abc", "1.5", ""].map(Some))
        {
            let attributes: Vec<(&str, &str)> = value
                .map(|value| ("initial-scroll-to-index", value))
                .into_iter()
                .collect();
            let (mut document, view, children) = vertical_indexed(&attributes, 4);
            assert_eq!(carried(&mut document, view), None, "{value:?}");
            assert_eq!(document.scroll_offset(view), Vector2D::zero(), "{value:?}");
            assert_eq!(targets(&document, &children), ["none"; 4], "{value:?}");
        }
    }

    #[test]
    fn a_later_change_scrolls_to_the_new_child_and_nothing_else_does() {
        let (mut document, view, children) =
            vertical_indexed(&[("initial-scroll-to-index", "1")], 4);
        document.commit();
        assert_eq!(document.scroll_offset(view).y, 300.0);

        document.set_attribute(view, "initial-scroll-to-index", "2");
        assert_eq!(
            carried(&mut document, view),
            Some((Vector2D::new(0.0, 600.0), ScrollBehavior::Instant))
        );
        assert_eq!(document.scroll_offset(view).y, 600.0);

        document.scroll_to(view, Vector2D::new(0.0, 50.0));
        let serial = document
            .pending_scroll_request(view)
            .expect("not yet acknowledged");
        document.acknowledge_scroll_request(view, serial);
        document.set_attribute(view, "initial-scroll-to-index", "2");
        document.set_inline_style(
            children[0],
            "width: 300px; height: 300px; flex-shrink: 0; opacity: 0.5",
        );
        assert_eq!(carried(&mut document, view), None);
        assert_eq!(document.scroll_offset(view).y, 50.0);

        document.remove_attribute(view, "initial-scroll-to-index");
        assert_eq!(carried(&mut document, view), None);
        assert_eq!(document.scroll_offset(view).y, 50.0);
        assert_eq!(targets(&document, &children), ["none"; 4]);
    }

    #[test]
    fn a_nested_view_is_its_parents_target_and_keeps_its_own_index() {
        let full = "width: 300px; height: 300px; flex-shrink: 0";
        let (mut document, outer, _) = vertical_indexed(&[("initial-scroll-to-index", "1")], 0);
        let first = element_under(&mut document, outer, "view", full);
        let inner = element_under(&mut document, outer, SCROLL_VIEW_TAG, full);
        document.set_attribute(inner, "scroll-x", "");
        let inner_children: Vec<NodeId> = (0..3)
            .map(|_| element_under(&mut document, inner, "view", full))
            .collect();
        element_under(&mut document, outer, "view", full);
        document.commit();
        assert_eq!(targets(&document, &[first, inner]), ["none", "nearest"]);
        assert_eq!(targets(&document, &inner_children), ["none"; 3]);
        assert_eq!(document.scroll_offset(outer).y, 300.0);
        assert_eq!(document.scroll_offset(inner).x, 0.0);

        let (mut document, outer, _) = vertical_indexed(&[], 0);
        let inner = element_under(&mut document, outer, SCROLL_VIEW_TAG, full);
        document.set_attribute(inner, "scroll-x", "");
        document.set_attribute(inner, "initial-scroll-to-index", "2");
        let inner_children: Vec<NodeId> = (0..3)
            .map(|_| element_under(&mut document, inner, "view", full))
            .collect();
        document.commit();
        assert_eq!(targets(&document, &[inner]), ["none"]);
        assert_eq!(
            targets(&document, &inner_children),
            ["none", "none", "nearest"]
        );
        assert_eq!(document.scroll_offset(inner).x, 600.0);
        assert_eq!(document.scroll_offset(outer).y, 0.0);
    }
}
